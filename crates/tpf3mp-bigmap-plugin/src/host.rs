//! The hook as Big Maps sees it (`tpf3mp-plugin-abi`): the calls a plugin
//! makes while it starts, and the services it keeps for the game's run.
//!
//! Outside the game (the tests, the packager) there are no services: a
//! read finds nothing, the log goes nowhere and the update count is
//! unknown, so every patch fails closed.

use std::{
    ffi::c_void,
    sync::atomic::{AtomicPtr, AtomicUsize, Ordering},
};

use tpf3mp_plugin_abi as abi;

/// The hook's services, once it started the plugin.
static SERVICES: AtomicPtr<abi::Services> = AtomicPtr::new(std::ptr::null_mut());

/// The hook, while it starts the plugin.
pub struct Host<'a> {
    raw: &'a abi::Host,
}

impl Host<'_> {
    /// The host the hook passed to the plugin's entry, if it offers this
    /// plugin's interface. Keeps its services for the game's run.
    ///
    /// # Safety
    ///
    /// `host` is the pointer the hook passed, used only during the entry.
    #[allow(unsafe_code)]
    pub unsafe fn new<'a>(host: *const abi::Host) -> Option<Host<'a>> {
        if host.is_null() {
            return None;
        }
        // SAFETY: the caller's promise.
        let raw = unsafe { &*host };
        if raw.abi != abi::ABI {
            return None;
        }
        SERVICES.store(raw.services.cast_mut(), Ordering::Release);
        Some(Host { raw })
    }

    /// `len` bytes of UTF-8 at `ptr`, or nothing.
    #[allow(unsafe_code)]
    fn text(ptr: *const u8, len: usize) -> &'static str {
        if ptr.is_null() {
            return "";
        }
        // SAFETY: the host's own string, valid while it starts the plugin;
        // read before it returns.
        std::str::from_utf8(unsafe { std::slice::from_raw_parts(ptr, len) }).unwrap_or("")
    }

    /// The package's settings, as JSON.
    pub fn settings(&self) -> &str {
        Self::text(self.raw.settings, self.raw.settings_len)
    }

    fn status(code: i32, target: &str) -> Result<(), String> {
        match code {
            abi::OK => Ok(()),
            abi::error::UNDECLARED => Err(format!("{target} is not declared in the package")),
            abi::error::WRONG_KIND => Err(format!("{target} is declared as another kind")),
            abi::error::WRONG_LENGTH => {
                Err(format!("{target}'s bytes are not as many as it expects"))
            }
            other => Err(format!("{target}: the hook said {other}")),
        }
    }

    /// Runs `detour` at the declared `detour`, `call` or `slot` hook
    /// `target`; what was there goes into `next`, which the detour calls
    /// on through.
    #[allow(unsafe_code)]
    pub fn attach(
        &self,
        target: &str,
        detour: *const (),
        next: &'static AtomicUsize,
    ) -> Result<(), String> {
        // SAFETY: the host's own call, with a static slot as wide as a
        // pointer.
        let code = unsafe {
            (self.raw.attach)(
                self.raw.ctx,
                target.as_ptr(),
                target.len(),
                detour.cast(),
                next.as_ptr().cast(),
            )
        };
        Self::status(code, target)
    }

    /// Runs `callback` at the declared `splice` hook `target`.
    #[allow(unsafe_code)]
    pub fn splice(&self, target: &str, callback: abi::SpliceFn) -> Result<(), String> {
        // SAFETY: the host's own call.
        let code = unsafe {
            (self.raw.splice)(
                self.raw.ctx,
                target.as_ptr(),
                target.len(),
                callback,
                std::ptr::null_mut(),
            )
        };
        Self::status(code, target)
    }

    /// Writes `bytes` at the declared `rewrite` hook `target`.
    #[allow(unsafe_code)]
    pub fn rewrite(&self, target: &str, bytes: &[u8]) -> Result<(), String> {
        // SAFETY: the host's own call.
        let code = unsafe {
            (self.raw.rewrite)(
                self.raw.ctx,
                target.as_ptr(),
                target.len(),
                bytes.as_ptr(),
                bytes.len(),
            )
        };
        Self::status(code, target)
    }

    /// Where the declared hook `target` is in the game: a site, or a
    /// slot's table entry.
    #[allow(unsafe_code)]
    pub fn address(&self, target: &str) -> Result<usize, String> {
        // SAFETY: the host's own call.
        let at = unsafe { (self.raw.address)(self.raw.ctx, target.as_ptr(), target.len()) };
        if at.is_null() {
            Err(format!("{target} is not declared in the package"))
        } else {
            Ok(at as usize)
        }
    }

    /// Runs `chunk` in each of the game's Lua states.
    #[allow(unsafe_code)]
    pub fn lua(&self, chunk: &str) -> Result<(), String> {
        // SAFETY: the host's own call.
        let code = unsafe { (self.raw.lua)(self.raw.ctx, chunk.as_ptr(), chunk.len()) };
        Self::status(code, "the menu's Lua")
    }
}

fn services() -> Option<&'static abi::Services> {
    let services = SERVICES.load(Ordering::Acquire);
    // SAFETY: the hook's own, for the game's run.
    #[allow(unsafe_code)]
    (!services.is_null()).then(|| unsafe { &*services })
}

/// `len` bytes of the game's memory at `address`, if they can be read.
#[allow(unsafe_code)]
pub fn read_bytes(address: usize, len: usize) -> Option<Vec<u8>> {
    let services = services()?;
    if address == 0 {
        return None;
    }
    let mut bytes = vec![0u8; len];
    // SAFETY: the hook's guarded read into a buffer of `len` bytes.
    let code = unsafe { (services.read)(address as *const c_void, bytes.as_mut_ptr(), len) };
    (code == abi::OK).then_some(bytes)
}

/// A value of the game's memory at `address`, if it can be read.
#[allow(unsafe_code)]
pub fn read<T: Copy>(address: usize) -> Option<T> {
    let bytes = read_bytes(address, std::mem::size_of::<T>())?;
    // SAFETY: as many bytes as a `T`, read whole.
    Some(unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<T>()) })
}

/// Whether `len` bytes at `address` can be read.
pub fn readable(address: usize, len: usize) -> bool {
    read_bytes(address, len).is_some()
}

/// Writes a line to the hook's log.
#[allow(unsafe_code)]
pub fn log(line: &str) {
    if let Some(services) = services() {
        // SAFETY: the hook's own call, with this text.
        unsafe { (services.log)(line.as_ptr(), line.len()) };
    }
}

/// The game's update count while its step runs.
#[allow(unsafe_code)]
pub fn update_count() -> Option<u32> {
    let services = services()?;
    // SAFETY: the hook's own call.
    u32::try_from(unsafe { (services.update_count)() }).ok()
}
