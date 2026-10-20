//! Native mods' plugins in the game (proposed D29 amendment,
//! docs/NATIVE_MODS.md, "Plugins"): the code a package carries, loaded only
//! when the launcher enabled the package, and patched into the game only
//! where the package declares it.
//!
//! At bootstrap, while the game is still suspended, and before the hook's
//! own multiplayer patches, for the plan's packages in the load order:
//!
//! 1. every declared hook is found in the running build by its signature,
//!    and the bytes at its site checked ([`locate`]); a call's old callee
//!    and a slot's old function are read there;
//! 2. the hooks are checked against each other and against the code the
//!    hook patches itself ([`arrange`]): overlapping bytes refuse, unless
//!    they are one site the hooks may share (the same kind, all `chain`,
//!    a rewrite's very same bytes);
//! 3. each plugin file is checked against the SHA-256 its package pins
//!    ([`verify`]), loaded, and started ([`start`]): it attaches its
//!    functions, splices its callbacks and gives its rewrites' bytes, by
//!    the hooks' names, and may hand over Lua for the game's Lua states;
//! 4. each site is patched once ([`patch`]): detours, calls and slots
//!    chain the plugins' functions in the load order, the last calling on
//!    to what was there; a splice runs every callback in turn; a rewrite
//!    writes its bytes. A hook its plugin did not ask for this time (a
//!    switch it left off) stays as the game has it.
//!
//! Any step that fails stops them all before anything is patched (a patch
//! that fails takes back those made), and keeps the game out of rooms: a
//! plugin's code changes what nobody can tell.

use std::{collections::BTreeMap, ffi::c_void, path::Path, sync::Mutex};

use tpf3mp_hookcore::pattern::{Pattern, ScanError};
use tpf3mp_nativemods::{
    enabled::EnabledPackage,
    index::{HookKind, HookUse},
};
use tpf3mp_plugin_abi as abi;

/// A declared hook, found in the running game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub package: String,
    /// `<package id>::<name>`.
    pub target: String,
    pub kind: HookKind,
    pub access: HookUse,
    /// Where the patch writes: the site, or a slot's table entry.
    pub address: usize,
    /// How many bytes from `address` it writes.
    pub len: usize,
    /// The bytes there now: what the site holds, or the slot's pointer.
    pub expected: Vec<u8>,
    /// What runs there now: a call's callee, a slot's function.
    pub old: usize,
    /// A splice's stolen bytes.
    pub steal: usize,
    /// A rewrite's bytes its package gives.
    pub replacement: Option<Vec<u8>>,
}

/// The bytes a patch made by the hook's own code may cover from one of its
/// targets, at most: no plugin may write there.
const CORE_SPAN: usize = 32;

/// Finds `package`'s declared hooks in `code`, whose first byte is at
/// `start`, each by its signature, checking the bytes at its site;
/// `pointer` reads a pointer from the game's memory (a slot's table lies
/// outside its code).
pub fn locate(
    package: &EnabledPackage,
    code: &[u8],
    start: usize,
    pointer: &dyn Fn(usize) -> Option<usize>,
) -> Result<Vec<Found>, String> {
    package
        .hooks
        .iter()
        .map(|hook| {
            let pattern = Pattern::parse(&hook.signature)
                .map_err(|error| format!("{}: the signature: {error:?}", hook.target))?;
            let at = match pattern.find_unique(code) {
                Ok(at) => at,
                Err(ScanError::NotFound) => {
                    return Err(format!("{} is not in this build of the game", hook.target));
                }
                Err(ScanError::Ambiguous { count }) => {
                    return Err(format!(
                        "{}'s signature matches {count} places in the game",
                        hook.target
                    ));
                }
            };
            let expected = hook
                .expected_bytes()
                .ok_or_else(|| format!("{}: the expected bytes are not hex", hook.target))?;
            let site = i64::try_from(at)
                .ok()
                .and_then(|at| at.checked_add(hook.offset))
                .and_then(|at| usize::try_from(at).ok())
                .filter(|at| {
                    at.checked_add(expected.len())
                        .is_some_and(|end| end <= code.len())
                })
                .ok_or_else(|| format!("{} lies outside the game's code", hook.target))?;
            if code[site..site + expected.len()] != expected[..] {
                return Err(format!(
                    "{} does not hold the bytes its package expects",
                    hook.target
                ));
            }
            let disp = |from: usize| {
                i32::from_le_bytes([code[from], code[from + 1], code[from + 2], code[from + 3]])
            };
            let address = start + site;
            let mut found = Found {
                package: package.id.clone(),
                target: hook.target.clone(),
                kind: hook.kind,
                access: hook.access,
                address,
                len: hook.written_len(),
                expected: expected.clone(),
                old: 0,
                steal: usize::from(hook.steal.unwrap_or(0)),
                replacement: hook
                    .replacement
                    .as_deref()
                    .and_then(tpf3mp_nativemods::index::hex_bytes),
            };
            match hook.kind {
                HookKind::Detour | HookKind::Splice | HookKind::Rewrite | HookKind::Find => {}
                HookKind::Call => {
                    found.old = (address + 5).wrapping_add_signed(disp(site + 1) as isize);
                }
                HookKind::Slot => {
                    let slot = hook
                        .slot
                        .ok_or_else(|| format!("{} names no slot", hook.target))?;
                    let table = (address + usize::from(slot.length))
                        .wrapping_add_signed(disp(site + usize::from(slot.disp_at)) as isize);
                    found.address = table + 8 * usize::from(slot.index);
                    found.old = pointer(found.address)
                        .ok_or_else(|| format!("{}'s slot cannot be read", hook.target))?;
                    found.expected = (found.old as u64).to_le_bytes().to_vec();
                }
            }
            Ok(found)
        })
        .collect()
}

/// Whether hooks `a` and `b`, on one site, may both patch it.
fn may_share(a: &Found, b: &Found) -> bool {
    a.kind == b.kind
        && a.access == HookUse::Chain
        && b.access == HookUse::Chain
        && (a.kind != HookKind::Rewrite
            || (a.replacement.is_some() && a.replacement == b.replacement))
}

/// The found hooks by site, each site's in the load order (the order of
/// `found`), unless one writes over code the hook patches itself (within
/// [`CORE_SPAN`] of a target in `core`), two overlap at different sites,
/// or two on one site may not share it.
pub fn arrange(found: Vec<Found>, core: &[usize]) -> Result<BTreeMap<usize, Vec<Found>>, String> {
    let overlaps = |a: usize, a_len: usize, b: usize, b_len: usize| a < b + b_len && b < a + a_len;
    let mut sites: BTreeMap<usize, Vec<Found>> = BTreeMap::new();
    // A `find` writes nothing: it overlaps nothing and shares with anything.
    for hook in found.into_iter().filter(|hook| hook.kind != HookKind::Find) {
        if core
            .iter()
            .any(|&target| overlaps(hook.address, hook.len, target, CORE_SPAN))
        {
            return Err(format!(
                "{} patches code TPF3-MP patches for multiplayer",
                hook.target
            ));
        }
        sites.entry(hook.address).or_default().push(hook);
    }
    for users in sites.values() {
        if let Some(pair) = users.windows(2).find(|pair| !may_share(&pair[0], &pair[1])) {
            return Err(format!(
                "{} and {} patch the same place in the game, and cannot share it",
                pair[0].target, pair[1].target
            ));
        }
    }
    let spans: Vec<(&Found, usize)> = sites
        .values()
        .map(|users| (&users[0], users.iter().map(|u| u.len).max().unwrap_or(0)))
        .collect();
    for pair in spans.windows(2) {
        let ((a, a_len), (b, b_len)) = (pair[0], pair[1]);
        if overlaps(a.address, a_len, b.address, b_len) {
            return Err(format!(
                "{} and {} patch overlapping bytes of the game",
                a.target, b.target
            ));
        }
    }
    Ok(sites)
}

/// Whether the file at `path` is the bytes whose SHA-256 is `sha256`.
pub fn verify(path: &Path, sha256: &str) -> Result<(), String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
    let actual: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
    if actual.eq_ignore_ascii_case(sha256) {
        Ok(())
    } else {
        Err(format!(
            "{} is not the plugin its package pins: it changed since it was installed",
            path.display()
        ))
    }
}

/// A function a plugin attached, and the slot it calls on through.
#[derive(Debug)]
pub struct Attachment {
    pub target: String,
    pub detour: *const c_void,
    pub next: *mut *const c_void,
}

/// What the plugins asked for as they started.
#[derive(Debug, Default)]
pub struct Started {
    pub attached: Vec<Attachment>,
    /// Splice callbacks by hook, with their `user` pointers.
    pub splices: Vec<(String, abi::SpliceFn, *mut c_void)>,
    /// Rewrites' bytes by hook.
    pub rewrites: Vec<(String, Vec<u8>)>,
    /// Lua for every Lua state.
    pub lua: Vec<String>,
}

/// What a plugin's [`abi::Host`] calls reach while it starts.
struct Starting<'a> {
    package: &'a str,
    found: &'a [Found],
    started: &'a mut Started,
    log: &'a mut dyn FnMut(&str),
}

impl Starting<'_> {
    /// The declared hook `target`, if it is one of `kinds` and was not
    /// asked for already.
    fn hook(&self, target: &str, kinds: &[HookKind]) -> Result<&Found, i32> {
        let found = self
            .found
            .iter()
            .find(|found| found.target == target)
            .ok_or(abi::error::UNDECLARED)?;
        if !kinds.contains(&found.kind) {
            return Err(abi::error::WRONG_KIND);
        }
        let asked = self.started.attached.iter().any(|a| a.target == target)
            || self.started.splices.iter().any(|s| s.0 == target)
            || self.started.rewrites.iter().any(|r| r.0 == target);
        if asked {
            return Err(abi::error::TWICE);
        }
        Ok(found)
    }
}

/// Starts a plugin of `package`: calls its `init` with its found hooks,
/// adding what it asked for to `started`. One that returns other than
/// [`abi::OK`] refuses.
///
/// # Safety
///
/// `init` is a plugin's entry, which runs its code.
#[allow(unsafe_code)]
pub unsafe fn start(
    package: &EnabledPackage,
    found: &[Found],
    init: abi::Init,
    started: &mut Started,
    log: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let root = package.root.to_string_lossy().into_owned();
    let settings = serde_json::to_string(&package.settings).unwrap_or_default();
    let mut starting = Starting {
        package: &package.id,
        found,
        started,
        log,
    };
    let host = abi::Host {
        abi: abi::ABI,
        ctx: (&raw mut starting).cast(),
        root: root.as_ptr(),
        root_len: root.len(),
        settings: settings.as_ptr(),
        settings_len: settings.len(),
        log: host_log,
        attach: host_attach,
        address: host_address,
        splice: host_splice,
        rewrite: host_rewrite,
        lua: host_lua,
        services: &raw const SERVICES,
    };
    // SAFETY: the caller vouches for `init`; `host` and what it points to
    // outlive the call.
    let status = unsafe { init(&raw const host) };
    if status != abi::OK {
        return Err(format!(
            "{}'s plugin refused to start ({status})",
            package.id
        ));
    }
    Ok(())
}

/// `len` bytes of UTF-8 at `text`.
///
/// # Safety
///
/// `text` points to `len` readable bytes, or is null.
#[allow(unsafe_code)]
unsafe fn text<'a>(text: *const u8, len: usize) -> Option<&'a str> {
    if text.is_null() {
        return None;
    }
    // SAFETY: the caller's promise.
    std::str::from_utf8(unsafe { std::slice::from_raw_parts(text, len) }).ok()
}

/// The `Starting` behind a host call's `ctx`.
///
/// # Safety
///
/// `ctx` is the one [`start`] passed, during the plugin's `init`.
#[allow(unsafe_code)]
unsafe fn starting<'a>(ctx: *mut c_void) -> &'a mut Starting<'a> {
    // SAFETY: the caller's promise.
    unsafe { &mut *ctx.cast::<Starting<'_>>() }
}

#[allow(unsafe_code)]
unsafe extern "C" fn host_log(ctx: *mut c_void, line: *const u8, len: usize) {
    // SAFETY: the host's own context, and the plugin's own text.
    let (starting, line) = unsafe { (starting(ctx), text(line, len)) };
    if let Some(line) = line {
        (starting.log)(&format!("plugin {}: {line}", starting.package));
    }
}

#[allow(unsafe_code)]
unsafe extern "C" fn host_attach(
    ctx: *mut c_void,
    target: *const u8,
    target_len: usize,
    detour: *const c_void,
    next: *mut *const c_void,
) -> i32 {
    // SAFETY: as in `host_log`.
    let (starting, target) = unsafe { (starting(ctx), text(target, target_len)) };
    let Some(target) = target.filter(|_| !detour.is_null() && !next.is_null()) else {
        return abi::error::BAD_ARGUMENT;
    };
    let kinds = [HookKind::Detour, HookKind::Call, HookKind::Slot];
    if let Err(code) = starting.hook(target, &kinds) {
        return code;
    }
    starting.started.attached.push(Attachment {
        target: target.to_owned(),
        detour,
        next,
    });
    abi::OK
}

#[allow(unsafe_code)]
unsafe extern "C" fn host_splice(
    ctx: *mut c_void,
    target: *const u8,
    target_len: usize,
    callback: abi::SpliceFn,
    user: *mut c_void,
) -> i32 {
    // SAFETY: as in `host_log`.
    let (starting, target) = unsafe { (starting(ctx), text(target, target_len)) };
    let Some(target) = target else {
        return abi::error::BAD_ARGUMENT;
    };
    if let Err(code) = starting.hook(target, &[HookKind::Splice]) {
        return code;
    }
    starting
        .started
        .splices
        .push((target.to_owned(), callback, user));
    abi::OK
}

#[allow(unsafe_code)]
unsafe extern "C" fn host_rewrite(
    ctx: *mut c_void,
    target: *const u8,
    target_len: usize,
    bytes: *const u8,
    bytes_len: usize,
) -> i32 {
    // SAFETY: as in `host_log`.
    let (starting, target) = unsafe { (starting(ctx), text(target, target_len)) };
    let Some(target) = target.filter(|_| !bytes.is_null()) else {
        return abi::error::BAD_ARGUMENT;
    };
    let expected_len = match starting.hook(target, &[HookKind::Rewrite]) {
        Ok(found) if found.replacement.is_some() => return abi::error::TWICE,
        Ok(found) => found.expected.len(),
        Err(code) => return code,
    };
    if bytes_len != expected_len {
        return abi::error::WRONG_LENGTH;
    }
    // SAFETY: the plugin passes `bytes_len` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, bytes_len) }.to_vec();
    starting.started.rewrites.push((target.to_owned(), bytes));
    abi::OK
}

#[allow(unsafe_code)]
unsafe extern "C" fn host_lua(ctx: *mut c_void, chunk: *const u8, len: usize) -> i32 {
    // SAFETY: as in `host_log`.
    let (starting, chunk) = unsafe { (starting(ctx), text(chunk, len)) };
    let Some(chunk) = chunk else {
        return abi::error::BAD_ARGUMENT;
    };
    starting.started.lua.push(chunk.to_owned());
    abi::OK
}

#[allow(unsafe_code)]
unsafe extern "C" fn host_address(
    ctx: *mut c_void,
    target: *const u8,
    target_len: usize,
) -> *const c_void {
    // SAFETY: as in `host_log`.
    let (starting, target) = unsafe { (starting(ctx), text(target, target_len)) };
    target
        .and_then(|target| starting.found.iter().find(|found| found.target == target))
        .map_or(std::ptr::null(), |found| found.address as *const c_void)
}

/// What plugins keep for the game's whole run.
pub static SERVICES: abi::Services = abi::Services {
    read: service_read,
    update_count: service_update_count,
    log: service_log,
};

#[allow(unsafe_code)]
unsafe extern "C" fn service_read(address: *const c_void, out: *mut u8, len: usize) -> i32 {
    if out.is_null() || !crate::image::readable(address as usize, len) {
        return abi::error::UNREADABLE;
    }
    // SAFETY: `len` readable bytes at `address` (checked), and the plugin's
    // own `len` bytes at `out`.
    unsafe { std::ptr::copy_nonoverlapping(address.cast::<u8>(), out, len) };
    abi::OK
}

#[allow(unsafe_code)]
unsafe extern "C" fn service_update_count() -> i64 {
    crate::install::update_count_now().map_or(-1, i64::from)
}

#[allow(unsafe_code)]
unsafe extern "C" fn service_log(line: *const u8, len: usize) {
    // SAFETY: the plugin's own text.
    if let Some(line) = unsafe { text(line, len) } {
        crate::log::line(&format!("plugin: {line}"));
    }
}

/// The Lua the plugins handed over, run in each of the game's Lua states
/// after the hook's own (`crate::menu_entry`).
static LUA: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The plugins' Lua, in the load order.
pub fn lua_chunks() -> Vec<String> {
    LUA.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Keeps `chunks` for the game's Lua states.
pub fn keep_lua(chunks: Vec<String>) {
    *LUA.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = chunks;
}

/// One site's splice callbacks, called in turn.
#[cfg(target_arch = "x86_64")]
pub struct SpliceSite {
    callbacks: Vec<(abi::SpliceFn, *mut c_void)>,
}

/// Every callback of the splice at `site`, in the load order.
#[cfg(target_arch = "x86_64")]
#[allow(unsafe_code)]
unsafe extern "system" fn dispatch(
    regs: *mut tpf3mp_hookcore::detour::SavedRegs,
    site: *const SpliceSite,
) {
    // SAFETY: the site the thunk was built for, kept with the splice.
    for (callback, user) in unsafe { &(*site).callbacks } {
        // SAFETY: the registers block, as `abi::Registers` lays it out;
        // the callback is its plugin's, loaded for good.
        unsafe { callback(regs.cast::<abi::Registers>(), *user) };
    }
}

/// The thunk a splice calls: puts `site` in the second argument's register
/// and jumps to [`dispatch`], the registers block still in the first.
#[cfg(target_arch = "x86_64")]
fn thunk(site: usize) -> Vec<u8> {
    // `mov rdx, imm64` on Windows, `mov rsi, imm64` elsewhere: the second
    // argument of the platform's C convention.
    let mut code = if cfg!(windows) {
        vec![0x48, 0xBA]
    } else {
        vec![0x48, 0xBE]
    };
    code.extend_from_slice(&(site as u64).to_le_bytes());
    code.extend_from_slice(&[0x48, 0xB8]); // mov rax, imm64
    code.extend_from_slice(&(dispatch as *const () as usize as u64).to_le_bytes());
    code.extend_from_slice(&[0xFF, 0xE0]); // jmp rax
    code
}

/// A patch made, taken back when dropped.
#[cfg(target_arch = "x86_64")]
pub enum Patch {
    Detour(tpf3mp_hookcore::detour::InlineDetour),
    Call(tpf3mp_hookcore::detour::CallRedirect),
    Rewrite(tpf3mp_hookcore::detour::Rewrite),
    /// The splice first, so it goes before the thunk and site it calls.
    Splice(
        tpf3mp_hookcore::detour::Splice,
        tpf3mp_hookcore::detour::ExecBuffer,
        Box<SpliceSite>,
    ),
}

/// Patches each site the plugins asked for, once, in `sites`' order. All
/// or nothing: a patch that fails takes back those made.
///
/// # Safety
///
/// The game's code at each site is what the hook found and checked, not
/// running on another thread; each plugin's functions, callbacks and slots
/// are its own, loaded for good.
#[cfg(target_arch = "x86_64")]
#[allow(unsafe_code)]
pub unsafe fn patch(
    sites: &BTreeMap<usize, Vec<Found>>,
    started: Started,
) -> Result<Vec<Patch>, String> {
    use tpf3mp_hookcore::detour::{CallRedirect, InlineDetour, Rewrite, Splice, SpliceHook};

    let mut patches = Vec::new();
    for (&address, users) in sites {
        let first = &users[0];
        let failed = |error: tpf3mp_hookcore::detour::DetourError| {
            format!("{} cannot be patched: {error}", first.target)
        };
        match first.kind {
            // Found and checked; nothing to write (`arrange` leaves them out).
            HookKind::Find => {}
            HookKind::Detour | HookKind::Call | HookKind::Slot => {
                // The functions in the load order: `users`' order.
                let chain: Vec<&Attachment> = users
                    .iter()
                    .filter_map(|u| started.attached.iter().find(|a| a.target == u.target))
                    .collect();
                let Some(head) = chain.first() else {
                    continue;
                };
                for pair in chain.windows(2) {
                    // SAFETY: the plugin handed this slot over to be filled.
                    unsafe { *pair[0].next = pair[1].detour };
                }
                let last = chain[chain.len() - 1];
                let patch = match first.kind {
                    HookKind::Detour => {
                        // SAFETY: the caller's promise for this function.
                        let detour = unsafe {
                            InlineDetour::install(address as *mut u8, head.detour.cast())
                        }
                        .map_err(failed)?;
                        // SAFETY: as above; nothing runs before the hook is done.
                        unsafe { *last.next = detour.trampoline().cast() };
                        Patch::Detour(detour)
                    }
                    HookKind::Call => {
                        // SAFETY: as above.
                        unsafe { *last.next = first.old as *const c_void };
                        // SAFETY: the call was found and checked; the head has
                        // the callee's signature.
                        Patch::Call(
                            unsafe {
                                CallRedirect::install(
                                    address as *mut u8,
                                    first.old,
                                    head.detour.cast(),
                                )
                            }
                            .map_err(failed)?,
                        )
                    }
                    _ => {
                        // SAFETY: as above.
                        unsafe { *last.next = first.old as *const c_void };
                        // SAFETY: the slot holds `old` (read, and checked again
                        // by the rewrite); the head has its signature.
                        Patch::Rewrite(
                            unsafe {
                                Rewrite::install(
                                    address as *mut u8,
                                    &first.expected,
                                    &(head.detour as u64).to_le_bytes(),
                                )
                            }
                            .map_err(failed)?,
                        )
                    }
                };
                patches.push(patch);
            }
            HookKind::Splice => {
                let callbacks: Vec<(abi::SpliceFn, *mut c_void)> = users
                    .iter()
                    .filter_map(|u| started.splices.iter().find(|s| s.0 == u.target))
                    .map(|(_, callback, user)| (*callback, *user))
                    .collect();
                if callbacks.is_empty() {
                    continue;
                }
                let site = Box::new(SpliceSite { callbacks });
                let stub = tpf3mp_hookcore::detour::executable(&thunk(&raw const *site as usize))
                    .map_err(failed)?;
                // SAFETY: the thunk takes the registers block as a
                // `SpliceHook` would, and adds the site.
                let hook = unsafe { std::mem::transmute::<*const u8, SpliceHook>(stub.as_ptr()) };
                // SAFETY: the caller's promise for this site; the stolen
                // bytes are whole instructions, as its package declares.
                let splice = unsafe {
                    Splice::install(address as *mut u8, &first.expected, first.steal, hook)
                }
                .map_err(failed)?;
                patches.push(Patch::Splice(splice, stub, site));
            }
            HookKind::Rewrite => {
                let bytes = first.replacement.clone().or_else(|| {
                    started
                        .rewrites
                        .iter()
                        .find(|(target, _)| users.iter().any(|u| &u.target == target))
                        .map(|(_, bytes)| bytes.clone())
                });
                // Declared, but its plugin left it as it is this time.
                let Some(bytes) = bytes else {
                    continue;
                };
                // SAFETY: the caller's promise; the bytes are as many as
                // those replaced.
                patches.push(Patch::Rewrite(
                    unsafe { Rewrite::install(address as *mut u8, &first.expected, &bytes) }
                        .map_err(failed)?,
                ));
            }
        }
    }
    Ok(patches)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tpf3mp_nativemods::index::{HookDecl, SlotDecl};

    use super::*;

    fn package(id: &str, hooks: Vec<HookDecl>) -> EnabledPackage {
        EnabledPackage {
            id: id.into(),
            version: "1.0.0".into(),
            simulation: true,
            features: Vec::new(),
            settings: BTreeMap::new(),
            files_sha256: tpf3mp_nativemods::terms::files_digest(&[]),
            identity_sha256: String::new(),
            depends: Vec::new(),
            root: std::path::PathBuf::from(format!("native-mods/{id}/1.0.0")),
            plugins: Vec::new(),
            hooks,
        }
    }

    fn hook(
        target: &str,
        kind: HookKind,
        signature: &str,
        offset: i64,
        expected: &str,
    ) -> HookDecl {
        HookDecl {
            target: target.into(),
            kind,
            signature: signature.into(),
            offset,
            expected: expected.into(),
            access: HookUse::Chain,
            steal: (kind == HookKind::Splice).then_some(5),
            replacement: None,
            slot: None,
        }
    }

    /// A function that returns its argument plus one, padded so a detour
    /// can take its first 14 bytes, after some other code.
    const ADD: &[u8] = &[
        0xCC, 0xCC, 0xCC, 0xCC, // padding
        0x89, 0xC8, // mov eax, ecx
        0x83, 0xC0, 0x01, // add eax, 1
        0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // nops
        0xC3, // ret
    ];
    const ADD_SIGNATURE: &str = "89 C8 83 C0 01 90 90 90";

    fn no_memory(_: usize) -> Option<usize> {
        None
    }

    #[test]
    fn hooks_are_found_by_signature_and_checked() {
        let one = |decl: HookDecl| locate(&package("a", vec![decl]), ADD, 0x1000, &no_memory);
        let found = one(hook("a::add", HookKind::Detour, ADD_SIGNATURE, 0, "89C8")).unwrap();
        assert_eq!((found[0].address, found[0].len), (0x1004, 14));
        let splice = one(hook(
            "a::mid",
            HookKind::Splice,
            ADD_SIGNATURE,
            2,
            "83C0019090",
        ))
        .unwrap();
        assert_eq!((splice[0].address, splice[0].steal), (0x1006, 5));
        assert!(
            one(hook(
                "a::add",
                HookKind::Detour,
                "89 C8 83 C0 02 90 90 90",
                0,
                "89"
            ))
            .unwrap_err()
            .contains("not in this build")
        );
        assert!(
            one(hook("a::add", HookKind::Detour, ADD_SIGNATURE, 0, "C3"))
                .unwrap_err()
                .contains("does not hold")
        );
        assert!(
            one(hook("a::nop", HookKind::Detour, "90 90 90 90", 0, "90"))
                .unwrap_err()
                .contains("matches")
        );
        // A slot whose table cannot be read: refused.
        let mut slot = hook("a::slot", HookKind::Slot, ADD_SIGNATURE, 0, "89C883C001");
        slot.slot = Some(SlotDecl {
            disp_at: 1,
            length: 5,
            index: 0,
        });
        assert!(one(slot).unwrap_err().contains("cannot be read"));
    }

    fn at(target: &str, kind: HookKind, access: HookUse, address: usize, len: usize) -> Found {
        Found {
            package: target.split("::").next().unwrap_or_default().into(),
            target: target.into(),
            kind,
            access,
            address,
            len,
            expected: Vec::new(),
            old: 0,
            steal: 0,
            replacement: None,
        }
    }

    #[test]
    fn sites_are_shared_by_their_own_kind_and_never_overlapped() {
        let sites = arrange(
            vec![
                at("a::add", HookKind::Detour, HookUse::Chain, 0x100, 14),
                at("b::add", HookKind::Detour, HookUse::Chain, 0x100, 14),
                at("c::mid", HookKind::Splice, HookUse::Chain, 0x200, 5),
                at("d::mid", HookKind::Splice, HookUse::Chain, 0x200, 5),
            ],
            &[0x999],
        )
        .unwrap();
        let order: Vec<&str> = sites[&0x100].iter().map(|f| f.target.as_str()).collect();
        assert_eq!(order, ["a::add", "b::add"], "in the load order");
        assert_eq!(sites[&0x200].len(), 2, "splices share a site");

        let refused = |found: Vec<Found>| arrange(found, &[0x999]).unwrap_err();
        assert!(
            refused(vec![
                at("a::add", HookKind::Detour, HookUse::Chain, 0x100, 14),
                at("b::own", HookKind::Detour, HookUse::Replace, 0x100, 14),
            ])
            .contains("cannot share")
        );
        assert!(
            refused(vec![
                at("a::add", HookKind::Detour, HookUse::Chain, 0x100, 14),
                at("b::mid", HookKind::Splice, HookUse::Chain, 0x108, 5),
            ])
            .contains("overlapping")
        );
        assert!(
            refused(vec![at(
                "a::mid",
                HookKind::Splice,
                HookUse::Chain,
                0x9A0,
                5
            )])
            .contains("for multiplayer")
        );
        let mut one = at("a::size", HookKind::Rewrite, HookUse::Chain, 0x300, 1);
        one.replacement = Some(vec![7]);
        let mut same = at("b::size", HookKind::Rewrite, HookUse::Chain, 0x300, 1);
        same.replacement = Some(vec![7]);
        assert!(arrange(vec![one.clone(), same.clone()], &[]).is_ok());
        same.replacement = Some(vec![8]);
        assert!(arrange(vec![one, same], &[]).is_err());
    }

    #[test]
    fn a_plugin_file_must_be_the_pinned_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugin.dll");
        std::fs::write(&path, b"plugin").unwrap();
        let pinned: String = ring::digest::digest(&ring::digest::SHA256, b"plugin")
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(verify(&path, &pinned), Ok(()));
        std::fs::write(&path, b"plugin, changed").unwrap();
        assert!(verify(&path, &pinned).is_err());
    }

    #[test]
    fn the_registers_are_the_splices_own() {
        use std::mem::{offset_of, size_of};

        use tpf3mp_hookcore::detour::SavedRegs;
        assert_eq!(size_of::<abi::Registers>(), size_of::<SavedRegs>());
        assert_eq!(
            offset_of!(abi::Registers, rflags),
            offset_of!(SavedRegs, rflags)
        );
        assert_eq!(offset_of!(abi::Registers, r8), offset_of!(SavedRegs, r8));
        assert_eq!(offset_of!(abi::Registers, rcx), offset_of!(SavedRegs, rcx));
        assert_eq!(offset_of!(abi::Registers, rax), offset_of!(SavedRegs, rax));
    }

    // Plugins patching live code, as the game would run it.
    #[cfg(all(windows, target_arch = "x86_64"))]
    mod live {
        use std::ffi::c_void;

        use super::*;

        type Add = unsafe extern "C" fn(i32) -> i32;

        struct Slots {
            ten: *const c_void,
            hundred: *const c_void,
            triple: *const c_void,
            quadruple: *const c_void,
        }
        static mut NEXT: Slots = Slots {
            ten: std::ptr::null(),
            hundred: std::ptr::null(),
            triple: std::ptr::null(),
            quadruple: std::ptr::null(),
        };

        macro_rules! calls_on {
            ($name:ident, $slot:ident, $how:expr) => {
                #[allow(unsafe_code)]
                unsafe extern "C" fn $name(x: i32) -> i32 {
                    // SAFETY: the hook filled the slot before the code ran.
                    let next: Add = unsafe { std::mem::transmute(NEXT.$slot) };
                    // SAFETY: `next` has the same signature.
                    let on = unsafe { next(x) };
                    $how(on)
                }
            };
        }
        calls_on!(add_ten, ten, |on| on + 10);
        calls_on!(add_hundred, hundred, |on| on + 100);
        calls_on!(times_three, triple, |on| on * 3);
        calls_on!(times_four, quadruple, |on| on * 4);

        #[allow(unsafe_code)]
        unsafe extern "C" fn plus_hundred(regs: *mut abi::Registers, _: *mut c_void) {
            // SAFETY: the registers at the site.
            unsafe { (*regs).rax += 100 };
        }

        #[allow(unsafe_code)]
        unsafe extern "C" fn doubled(regs: *mut abi::Registers, _: *mut c_void) {
            // SAFETY: as above.
            unsafe { (*regs).rax *= 2 };
        }

        #[allow(unsafe_code)]
        fn host<'a>(host: *const abi::Host) -> &'a abi::Host {
            // SAFETY: the host the hook passed, during init.
            unsafe { &*host }
        }

        #[allow(unsafe_code)]
        fn attach(
            h: &abi::Host,
            name: &str,
            detour: *const c_void,
            slot: *mut *const c_void,
        ) -> i32 {
            // SAFETY: the host's own call.
            unsafe { (h.attach)(h.ctx, name.as_ptr(), name.len(), detour, slot) }
        }

        #[allow(unsafe_code)]
        fn splice(h: &abi::Host, name: &str, callback: abi::SpliceFn) -> i32 {
            // SAFETY: the host's own call.
            unsafe {
                (h.splice)(
                    h.ctx,
                    name.as_ptr(),
                    name.len(),
                    callback,
                    std::ptr::null_mut(),
                )
            }
        }

        #[allow(unsafe_code)]
        unsafe extern "C" fn init_ten(h: *const abi::Host) -> i32 {
            let h = host(h);
            assert_eq!(
                attach(h, "ten::add", std::ptr::null(), std::ptr::null_mut()),
                abi::error::BAD_ARGUMENT
            );
            assert_eq!(
                attach(h, "ten::nothing", add_ten as *const c_void, unsafe {
                    &raw mut NEXT.ten
                }),
                abi::error::UNDECLARED
            );
            assert_eq!(splice(h, "ten::add", plus_hundred), abi::error::WRONG_KIND);
            attach(h, "ten::add", add_ten as *const c_void, unsafe {
                &raw mut NEXT.ten
            })
        }

        #[allow(unsafe_code)]
        unsafe extern "C" fn init_hundred(h: *const abi::Host) -> i32 {
            attach(
                host(h),
                "hundred::add",
                add_hundred as *const c_void,
                unsafe { &raw mut NEXT.hundred },
            )
        }

        #[allow(unsafe_code)]
        unsafe extern "C" fn init_refusing(_: *const abi::Host) -> i32 {
            7
        }

        /// Locates, arranges, starts and patches `packages` in `image` at
        /// `base`, each with its init.
        #[allow(unsafe_code)]
        fn run(
            image: &[u8],
            base: usize,
            packages: &[(&EnabledPackage, abi::Init)],
            pointer: &dyn Fn(usize) -> Option<usize>,
        ) -> Vec<Patch> {
            let mut found = Vec::new();
            for (package, _) in packages {
                found.extend(locate(package, image, base, pointer).unwrap());
            }
            let sites = arrange(found.clone(), &[]).unwrap();
            let mut started = Started::default();
            let mut log = |_: &str| {};
            for (package, init) in packages {
                let own: Vec<Found> = found
                    .iter()
                    .filter(|f| f.package == package.id)
                    .cloned()
                    .collect();
                // SAFETY: the inits are this test's own.
                unsafe { start(package, &own, *init, &mut started, &mut log) }.unwrap();
            }
            // SAFETY: the code is this test's, running on no other thread.
            unsafe { patch(&sites, started) }.unwrap()
        }

        #[allow(unsafe_code)]
        fn image(code: &tpf3mp_hookcore::detour::ExecBuffer, len: usize) -> &[u8] {
            // SAFETY: the buffer's own bytes, read only.
            unsafe { std::slice::from_raw_parts(code.as_ptr(), len) }
        }

        #[test]
        #[allow(unsafe_code)]
        fn chained_detours_each_run_then_the_games_own_code() {
            let code = tpf3mp_hookcore::detour::executable(ADD).unwrap();
            let base = code.as_ptr() as usize;
            // SAFETY: `ADD` holds a function at offset 4.
            let game: Add = unsafe { std::mem::transmute(base + 4) };
            let declared = |id: &str| {
                package(
                    id,
                    vec![hook(
                        &format!("{id}::add"),
                        HookKind::Detour,
                        ADD_SIGNATURE,
                        0,
                        "89C8",
                    )],
                )
            };
            let (ten, hundred) = (declared("ten"), declared("hundred"));
            let patches = run(
                image(&code, ADD.len()),
                base,
                &[(&ten, init_ten), (&hundred, init_hundred)],
                &no_memory,
            );
            // SAFETY: patched, it runs ten's, hundred's, then its own.
            assert_eq!(unsafe { game(5) }, 116);
            drop(patches);
            // SAFETY: unpatched again.
            assert_eq!(unsafe { game(5) }, 6);

            let mut started = Started::default();
            // SAFETY: the init is this test's.
            let refused = unsafe { start(&ten, &[], init_refusing, &mut started, &mut |_| {}) };
            assert!(refused.unwrap_err().contains("refused to start (7)"));
        }

        #[allow(unsafe_code)]
        unsafe extern "C" fn init_plus(h: *const abi::Host) -> i32 {
            splice(host(h), "plus::mid", plus_hundred)
        }

        #[allow(unsafe_code)]
        unsafe extern "C" fn init_double(h: *const abi::Host) -> i32 {
            splice(host(h), "double::mid", doubled)
        }

        #[test]
        #[allow(unsafe_code)]
        fn splices_change_registers_in_the_middle_of_a_function() {
            let code = tpf3mp_hookcore::detour::executable(ADD).unwrap();
            let base = code.as_ptr() as usize;
            // SAFETY: as above.
            let game: Add = unsafe { std::mem::transmute(base + 4) };
            // At `add eax, 1`: eax holds the argument there.
            let declared = |id: &str| {
                package(
                    id,
                    vec![hook(
                        &format!("{id}::mid"),
                        HookKind::Splice,
                        ADD_SIGNATURE,
                        2,
                        "83C0019090",
                    )],
                )
            };
            let (plus, double) = (declared("plus"), declared("double"));
            let patches = run(
                image(&code, ADD.len()),
                base,
                &[(&plus, init_plus), (&double, init_double)],
                &no_memory,
            );
            // (5 + 100) * 2, then the function's own + 1.
            // SAFETY: as above.
            assert_eq!(unsafe { game(5) }, 211);
            drop(patches);
            // SAFETY: as above.
            assert_eq!(unsafe { game(5) }, 6);
        }

        #[allow(unsafe_code)]
        unsafe extern "C" fn init_none(_: *const abi::Host) -> i32 {
            abi::OK
        }

        #[allow(unsafe_code)]
        unsafe extern "C" fn init_rewrite(h: *const abi::Host) -> i32 {
            let h = host(h);
            let name = "seven::step";
            // SAFETY: the host's own call.
            let wrong =
                unsafe { (h.rewrite)(h.ctx, name.as_ptr(), name.len(), [7u8, 7].as_ptr(), 2) };
            assert_eq!(wrong, abi::error::WRONG_LENGTH);
            let bytes = [0x83u8, 0xC0, 0x07];
            // SAFETY: as above.
            unsafe { (h.rewrite)(h.ctx, name.as_ptr(), name.len(), bytes.as_ptr(), 3) }
        }

        #[test]
        #[allow(unsafe_code)]
        fn rewrites_change_bytes_given_by_the_package_or_its_plugin() {
            let code = tpf3mp_hookcore::detour::executable(ADD).unwrap();
            let base = code.as_ptr() as usize;
            // SAFETY: as above.
            let game: Add = unsafe { std::mem::transmute(base + 4) };
            // The package's own bytes: + 3.
            let mut given = hook("three::step", HookKind::Rewrite, ADD_SIGNATURE, 2, "83C001");
            given.replacement = Some("83C003".into());
            let three = package("three", vec![given]);
            let patches = run(
                image(&code, ADD.len()),
                base,
                &[(&three, init_none)],
                &no_memory,
            );
            // SAFETY: as above.
            assert_eq!(unsafe { game(5) }, 8);
            drop(patches);
            // The plugin's: + 7.
            let asked = hook("seven::step", HookKind::Rewrite, ADD_SIGNATURE, 2, "83C001");
            let seven = package("seven", vec![asked]);
            let patches = run(
                image(&code, ADD.len()),
                base,
                &[(&seven, init_rewrite)],
                &no_memory,
            );
            // SAFETY: as above.
            assert_eq!(unsafe { game(5) }, 12);
            drop(patches);
            // SAFETY: as above.
            assert_eq!(unsafe { game(5) }, 6);
        }

        /// `outer` calls `inner`, which returns its argument plus one.
        const CALLS: &[u8] = &[
            0x48, 0x83, 0xEC, 0x28, // sub rsp, 0x28
            0xE8, 0x17, 0x00, 0x00, 0x00, // call inner (0x20)
            0x48, 0x83, 0xC4, 0x28, // add rsp, 0x28
            0xC3, // ret
            0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC,
            0xCC, 0xCC, 0xCC, 0xCC, // padding
            0x8D, 0x41, 0x01, // inner: lea eax, [rcx + 1]
            0xC3, // ret
        ];

        #[allow(unsafe_code)]
        unsafe extern "C" fn init_triple(h: *const abi::Host) -> i32 {
            attach(
                host(h),
                "triple::inner",
                times_three as *const c_void,
                unsafe { &raw mut NEXT.triple },
            )
        }

        #[test]
        #[allow(unsafe_code)]
        fn a_call_is_redirected_and_calls_on_to_the_old_callee() {
            let code = tpf3mp_hookcore::detour::executable(CALLS).unwrap();
            let base = code.as_ptr() as usize;
            // SAFETY: `CALLS` holds `outer` at 0.
            let outer: Add = unsafe { std::mem::transmute(base) };
            // SAFETY: as above.
            assert_eq!(unsafe { outer(5) }, 6);
            let call = hook(
                "triple::inner",
                HookKind::Call,
                "48 83 EC 28 E8 17 00 00 00 48 83 C4",
                4,
                "E817000000",
            );
            let triple = package("triple", vec![call]);
            let found = locate(&triple, image(&code, CALLS.len()), base, &no_memory).unwrap();
            assert_eq!(found[0].old, base + 0x20, "the old callee");
            let patches = run(
                image(&code, CALLS.len()),
                base,
                &[(&triple, init_triple)],
                &no_memory,
            );
            // SAFETY: as above.
            assert_eq!(unsafe { outer(5) }, 18);
            drop(patches);
            // SAFETY: as above.
            assert_eq!(unsafe { outer(5) }, 6);
        }

        extern "C" fn plus_one(x: i32) -> i32 {
            x + 1
        }

        #[allow(unsafe_code)]
        unsafe extern "C" fn init_quadruple(h: *const abi::Host) -> i32 {
            attach(
                host(h),
                "four::entry",
                times_four as *const c_void,
                unsafe { &raw mut NEXT.quadruple },
            )
        }

        #[test]
        #[allow(unsafe_code)]
        fn a_slot_points_at_the_plugin_which_calls_on_to_the_old_function() {
            // `lea rax, [rip + 0x39]` addresses the table at 0x40, whose
            // entry 1 is `plus_one`.
            let mut bytes = vec![0x48, 0x8D, 0x05, 0x39, 0x00, 0x00, 0x00, 0xC3];
            bytes.resize(0x40, 0xCC);
            bytes.extend_from_slice(&0u64.to_le_bytes());
            bytes.extend_from_slice(&(plus_one as *const () as usize as u64).to_le_bytes());
            let code = tpf3mp_hookcore::detour::executable(&bytes).unwrap();
            let base = code.as_ptr() as usize;
            let entry = base + 0x48;
            // SAFETY: the table's entry, in the buffer.
            let pointer = |at: usize| Some(unsafe { std::ptr::read_unaligned(at as *const usize) });
            let call = || {
                // SAFETY: the entry holds a function of this signature.
                let f: Add = unsafe { std::mem::transmute(pointer(entry).unwrap_or_default()) };
                // SAFETY: as above.
                unsafe { f(5) }
            };
            assert_eq!(call(), 6);
            let mut slot = hook(
                "four::entry",
                HookKind::Slot,
                "48 8D 05 39 00 00 00 C3",
                0,
                "488D0539000000",
            );
            slot.slot = Some(SlotDecl {
                disp_at: 3,
                length: 7,
                index: 1,
            });
            let four = package("four", vec![slot]);
            let found = locate(&four, image(&code, bytes.len()), base, &pointer).unwrap();
            assert_eq!(
                (found[0].address, found[0].old),
                (entry, plus_one as *const () as usize)
            );
            let patches = run(
                image(&code, bytes.len()),
                base,
                &[(&four, init_quadruple)],
                &pointer,
            );
            assert_eq!(call(), 24);
            drop(patches);
            assert_eq!(call(), 6);
        }
    }
}
