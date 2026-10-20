//! The interface between TPF3-MP's hook and a native mod's plugin
//! (proposed D29 amendment, docs/NATIVE_MODS.md, "Plugins").
//!
//! A plugin is a native library (a Windows DLL, a Linux shared object)
//! that a native mod's package carries, pinned in the index by size and
//! SHA-256. The hook loads it into the game only when the launcher enabled
//! the package, checks its SHA-256 again, and calls its one export,
//! [`ENTRY`], an [`Init`], once, while the game is still suspended at
//! start. The plugin then **attaches** to the hooks its package declares:
//!
//! - it names a declared hook (`<package id>::<name>`), never an address:
//!   the hook finds the function by the declared signature in the running
//!   build and checks its first bytes;
//! - it hands over its detour, a function with the game function's own
//!   signature and calling convention, and a slot the hook fills with
//!   what the detour calls on to: the next plugin's detour on that
//!   function, or the game's own code;
//! - the hook patches each function once, after every plugin has
//!   attached, and only if every one of them could: a plugin that fails
//!   leaves the game unpatched and out of rooms.
//!
//! A package declares each hook's kind, and the plugin uses the call for
//! it:
//!
//! - `detour` (a function's start), `call` (one `call` instruction) and
//!   `slot` (one entry of a table of function pointers, such as a
//!   vtable): [`Host::attach`] a function with the game's own signature,
//!   which calls on through its slot. Several plugins on one run in the
//!   load order, each calling the next, the last what was there;
//! - `splice` (the middle of a function): [`Host::splice`] a callback
//!   that gets every register as they are there, and may change them;
//!   several run in the load order, then the stolen instructions, then the
//!   function goes on;
//! - `rewrite` (bytes replaced one for one): [`Host::rewrite`] the bytes,
//!   unless the package gives them itself.
//!
//! One whose package declares it takes the place over (`"use": "replace"`)
//! is the only one there. The hook refuses a plugin's hook on bytes the
//! hook itself patches for multiplayer.
//!
//! Beside patches, a plugin may hand the hook Lua to run in each of the
//! game's Lua states as it starts ([`Host::lua`]), and keeps
//! [`Services`] for the game's whole run: guarded reads of the game's
//! memory, the game's update count, the log.
//!
//! Everything here is `#[repr(C)]` and `extern "C"`, so a plugin can be
//! written in any language. Strings are UTF-8, passed as a pointer and a
//! length, never NUL-terminated.

#![no_std]

use core::ffi::c_void;

/// The interface's version. A plugin built for another is not loaded.
pub const ABI: u32 = 1;

/// The name of the function every plugin exports, an [`Init`], NUL
/// terminated for the platform's symbol lookup.
pub const ENTRY: &[u8] = b"tpf3mp_plugin_init\0";

/// What [`Init`] and the host's calls return when all went well.
pub const OK: i32 = 0;

/// What the host's calls return when they did not.
pub mod error {
    /// The package declares no hook of that name.
    pub const UNDECLARED: i32 = 1;
    /// The plugin attached to that hook already.
    pub const TWICE: i32 = 2;
    /// A pointer was null, or a string not UTF-8.
    pub const BAD_ARGUMENT: i32 = 3;
    /// Too late: the game's functions are patched already.
    pub const TOO_LATE: i32 = 4;
    /// The memory cannot be read.
    pub const UNREADABLE: i32 = 5;
    /// The hook is declared, but as another kind.
    pub const WRONG_KIND: i32 = 6;
    /// The bytes are not as many as the hook expects.
    pub const WRONG_LENGTH: i32 = 7;
}

/// The registers a splice's callback gets: every general-purpose register
/// and the flags as they were at the site. A field changed is what the
/// code after the site sees. They lie on the game's own stack, just below
/// the site's `rsp`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Registers {
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

impl Registers {
    /// The stack pointer the code at the site had.
    pub fn rsp(this: *const Self) -> u64 {
        this as u64 + core::mem::size_of::<Self>() as u64
    }
}

/// A splice's callback: it runs on the game's thread at the site, with the
/// `user` pointer the plugin gave. It must not unwind.
pub type SpliceFn = unsafe extern "C" fn(regs: *mut Registers, user: *mut c_void);

/// What the hook offers a plugin for the game's whole run: the plugin may
/// keep this pointer.
#[repr(C)]
pub struct Services {
    /// Copies `len` bytes of the game's memory at `address` to `out`, if
    /// they can be read: [`OK`], or [`error::UNREADABLE`] without reading.
    pub read: unsafe extern "C" fn(address: *const c_void, out: *mut u8, len: usize) -> i32,
    /// The game's update count while its step runs, or -1 outside it.
    pub update_count: unsafe extern "C" fn() -> i64,
    /// Writes a line to the hook's log.
    pub log: unsafe extern "C" fn(text: *const u8, len: usize),
}

/// What the hook offers a plugin while it starts. Valid only during
/// [`Init`]; a plugin keeps none of its pointers but the slots it hands to
/// [`Host::attach`].
#[repr(C)]
pub struct Host {
    /// The interface's version: [`ABI`].
    pub abi: u32,
    /// The hook's own, passed back on every call.
    pub ctx: *mut c_void,
    /// The package's folder, where its files are.
    pub root: *const u8,
    pub root_len: usize,
    /// The package's settings, as JSON: its defaults with the player's (or
    /// the room's) choices over them.
    pub settings: *const u8,
    pub settings_len: usize,
    /// Writes a line to the hook's log, after the package's name.
    pub log: unsafe extern "C" fn(ctx: *mut c_void, text: *const u8, len: usize),
    /// Attaches `detour` to the declared hook `target`. Before the game
    /// runs the function, the hook writes to `*next` what the detour calls
    /// on to: the next plugin's detour on it, or the game's own code. The
    /// slot must stay valid while the game runs. Returns [`OK`] or an
    /// [`error`].
    pub attach: unsafe extern "C" fn(
        ctx: *mut c_void,
        target: *const u8,
        target_len: usize,
        detour: *const c_void,
        next: *mut *const c_void,
    ) -> i32,
    /// Where the declared hook `target` is in the running game, for a
    /// plugin that reads near it (a slot's: the table entry); null for a
    /// hook not declared.
    pub address: unsafe extern "C" fn(
        ctx: *mut c_void,
        target: *const u8,
        target_len: usize,
    ) -> *const c_void,
    /// Runs `callback` at the declared `splice` hook `target`, with `user`.
    pub splice: unsafe extern "C" fn(
        ctx: *mut c_void,
        target: *const u8,
        target_len: usize,
        callback: SpliceFn,
        user: *mut c_void,
    ) -> i32,
    /// Writes `bytes`, as many as the hook expects, at the declared
    /// `rewrite` hook `target` whose package does not give them itself.
    pub rewrite: unsafe extern "C" fn(
        ctx: *mut c_void,
        target: *const u8,
        target_len: usize,
        bytes: *const u8,
        bytes_len: usize,
    ) -> i32,
    /// Runs the Lua `chunk` once in each of the game's Lua states, as the
    /// state's loader first runs, after the hook's own.
    pub lua: unsafe extern "C" fn(ctx: *mut c_void, chunk: *const u8, len: usize) -> i32,
    /// What the plugin may keep for the game's whole run.
    pub services: *const Services,
}

/// A plugin's [`ENTRY`]: it attaches to its hooks and returns [`OK`], or
/// anything else to refuse to run.
pub type Init = unsafe extern "C" fn(host: *const Host) -> i32;
