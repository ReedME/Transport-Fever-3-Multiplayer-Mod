//! An x86-64 inline detour engine.
//!
//! Installing a detour overwrites a function's first instructions with a jump to
//! a replacement, after copying those instructions into a *trampoline* that ends
//! by jumping back into the function. Calling the trampoline therefore runs the
//! original code. The stolen instructions are relocated with iced-x86's block
//! encoder, so a RIP-relative operand still addresses the same absolute memory
//! from its new home; a prologue that cannot be relocated (it branches, returns,
//! or is undecodable) is refused rather than patched wrong.
//!
//! # Why iced-x86 and our own trampoline, not `retour`
//!
//! `retour` (stable 0.3.1; its 0.4 line is alpha) wraps this whole dance behind
//! `GenericDetour`, but it owns trampoline allocation and instruction relocation
//! internally, which is exactly the part we must be able to test and reason
//! about byte for byte on a game binary that shifts every patch. iced-x86 is a
//! pure-Rust decoder and block encoder with no build script and no runtime
//! allocation of its own; we drive the decode/relocate step directly and keep
//! the trampoline and patch code here, where the tests can see them. The engine
//! is x86-64 only by construction; iced-x86 is pulled in only on that target.
//!
//! # Thread-safety
//!
//! Patching overwrites up to fourteen live code bytes with a non-atomic copy.
//! The caller must guarantee the target cannot execute during install or
//! uninstall: either install before the target's first run (the launcher
//! loads the hook before the game's entry point runs: into the suspended game
//! on Windows, by `LD_PRELOAD` on Linux), or park every thread that could
//! reach it first. The engine does not itself stop threads.

use thiserror::Error;

/// Why a detour could not be installed or removed.
#[derive(Debug, Error)]
pub enum DetourError {
    #[error("the inline detour engine supports x86-64 only; this build targets {arch}")]
    UnsupportedArchitecture { arch: &'static str },
    #[error("unsupported prologue: {reason}")]
    UnsupportedPrologue { reason: String },
    #[error("could not re-encode the relocated prologue: {0}")]
    Encode(String),
    #[error("could not change memory protection (os error {0})")]
    Protect(i32),
    #[error("could not allocate executable memory (os error {0})")]
    Alloc(i32),
}

/// The registers a [`Splice`] hands its hook: every general-purpose
/// register and the flags as they were at the spliced site, in the order the
/// stub pushed them (the last push is first in memory). The hook may change
/// a field; the stub restores the registers from this block, so the change
/// reaches the code after the site. The block lies on the game's own stack:
/// the site's `rsp` is [`SavedRegs::rsp`].
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedRegs {
    pub rflags: u64,
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64,
}

impl SavedRegs {
    /// The stack pointer the spliced code had: the block sits right below it.
    pub fn rsp(this: *const Self) -> u64 {
        this as u64 + core::mem::size_of::<Self>() as u64
    }
}

/// What a [`Splice`] calls at its site, with the site's registers. It runs
/// on the thread that reached the site, with a 16-byte-aligned stack and
/// the platform's C calling convention; it must not unwind.
pub type SpliceHook = unsafe extern "system" fn(regs: *mut SavedRegs);

#[cfg(target_arch = "x86_64")]
mod sys;
#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use sys::ExecBuffer;
#[cfg(target_arch = "x86_64")]
pub use x86_64::{CallRedirect, InlineDetour, Rewrite, Splice};

/// `code`, copied into fresh executable memory: a function of its own, as
/// tests detour one.
#[cfg(target_arch = "x86_64")]
#[allow(unsafe_code)]
pub fn executable(code: &[u8]) -> Result<ExecBuffer, DetourError> {
    let buffer = sys::alloc_near(code.as_ptr() as usize, code.len())?;
    // SAFETY: `buffer` is a fresh writable region of `code.len()` bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(code.as_ptr(), buffer.as_mut_ptr(), code.len());
    }
    sys::make_executable(&buffer)?;
    // SAFETY: the buffer now holds the code just written.
    unsafe {
        sys::flush_icache(buffer.as_mut_ptr(), code.len());
    }
    Ok(buffer)
}

#[cfg(not(target_arch = "x86_64"))]
mod unsupported;
#[cfg(not(target_arch = "x86_64"))]
pub use unsupported::{CallRedirect, InlineDetour, Rewrite, Splice};
