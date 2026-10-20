//! The Multiplayer entry on the game's main menu (D17), on Windows x86-64,
//! and the request channel the lobby window talks to the hook over.
//!
//! Transport Fever 3 draws its main menu from a Teal script,
//! `gui/menu/main_page.tl`, which the game loads through its Lua loader.
//! A mod cannot replace that file: the game applies a mod's files only once
//! a game is loaded, and the menu offers mods no extension point. So the hook
//! steps in at the loader. The game's Lua `resolveutil.loadfile(path)` is a C++
//! lambda ([`PATCH`] explains the Lua side); its body is the profile's
//! `lua_loadfile` target, and this module detours it with a thunk that:
//!
//! 1. reads the Lua state out of the lambda's closure (`**(closure + 0x10)`,
//!    the layout `tpfre` showed at the body's own `lua_pcallk` call) and the
//!    requested path (`*(closure + 8) + 0x20`, a `std::string`),
//! 2. the first time it sees a state, runs [`PATCH`] in it with `lua_load` and
//!    `lua_pcallk` - Lua that wraps `resolveutil.loadfile` so a request for the
//!    game's `gui/menu/main_page.tl` is answered with the mod's copy
//!    (`tpf3mp_1::/gui/menu/main_page.tl`, which adds the Multiplayer card and
//!    window), and every other request passes through untouched,
//! 3. when the path starts with [`REQUEST_PREFIX`], answers it itself: the
//!    reply is pushed with `lua_pushlstring` and a stub returns to the loader's
//!    glue, which hands Lua two results (it always does); the lobby window
//!    asks for its state and sends its actions this way (`docs/LOBBY.md`),
//! 4. otherwise restores the argument registers and tail-jumps into the
//!    original body, so the loader itself runs exactly as before.
//!
//! The loader's first call in a state comes from `base/init.lua`'s own
//! requires, long before the menu is built, and `resolveutil` is registered
//! before any Lua runs, so the wrap is in place when the menu asks for its page.
//! A game started by Steam has no hook and keeps the plain menu (D11).
//!
//! Everything here is resolved by signature from the build's profile
//! (`profiles/*.toml`, `docs/HOOKS.md`); nothing is pinned to an address.

#![allow(unsafe_code)]

use std::ffi::{c_char, c_int, c_void};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};

use tpf3mp_hookcore::detour::InlineDetour;
use tpf3mp_hookcore::pe::PeHeaders;
use tpf3mp_hookcore::profile::{self, Profile};

use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

use crate::lobby;

/// The Lua run once in each of the game's Lua states, the first time the
/// loader runs there. It wraps `resolveutil.loadfile` (the function
/// `base/init.lua` calls for every `ug_require`) so that the game's
/// `gui/menu/main_page.tl` is served from the mod's copy, and so is the line
/// manager's window, `gui/line_vehicle_mgmt/manager_window.tl` (the game's,
/// with the stations clicked while a new line is being created kept for it,
/// not dropped: in a room that takes the room's round trip). Either copy that
/// does not load leaves the game's own. The mod's own copies are never
/// redirected, so the wrap cannot loop, and the original resolved
/// path stays the module's cache key, so the rest of the menu sees the same
/// `MainPage` value it always did. Requests to the hook pass straight
/// through to the original, where the detour answers them.
const MANAGER_WINDOW_BUILD_SHA256: &str =
    "74861ac43b041aebc5179154345b3cf1ec83154c8e6cc58e0d9e02ff5fa602e4";

pub fn manager_window_supported(profile: &Profile) -> bool {
    profile.build.sha256 == MANAGER_WINDOW_BUILD_SHA256
}

fn menu_patch(manager_window_supported: bool) -> String {
    format!("local tpf3mp_manager_window_supported = {manager_window_supported}\n{PATCH}")
}

const PATCH: &str = r#"
local ru = resolveutil
if type(ru) ~= "table" then
	pcall(debugPrint, "[tpf3mp] main menu: resolveutil is not here yet; the menu stays the game's")
	return
end
if ru.__tpf3mp_menu then return end
ru.__tpf3mp_menu = true
local orig = ru.loadfile
ru.loadfile = function(path, ...)
	if type(path) == "string" and path:find("gui/menu/main_page%.tl$") and not path:find("^tpf3mp_1::") then
		local ok, chunk, err = pcall(orig, "tpf3mp_1::/gui/menu/main_page.tl", ...)
		if ok and chunk then
			pcall(debugPrint, "[tpf3mp] main menu: " .. path .. " is served from tpf3mp_1::/gui/menu/main_page.tl")
			return chunk, err
		end
		pcall(debugPrint, "[tpf3mp] main menu: the mod's main_page.tl is not loadable (" .. tostring(ok and err or chunk) .. "); the menu stays the game's")
	end
	if tpf3mp_manager_window_supported and type(path) == "string" and path:find("gui/line_vehicle_mgmt/manager_window%.tl$") and not path:find("^tpf3mp_1::") then
		local ok, chunk, err = pcall(orig, "tpf3mp_1::/gui/line_vehicle_mgmt/manager_window.tl", ...)
		if ok and chunk then
			pcall(debugPrint, "[tpf3mp] line manager: " .. path .. " is served from tpf3mp_1::/gui/line_vehicle_mgmt/manager_window.tl")
			return chunk, err
		end
		pcall(debugPrint, "[tpf3mp] line manager: the mod's manager_window.tl is not loadable (" .. tostring(ok and err or chunk) .. "); the window stays the game's")
	end
	return orig(path, ...)
end
pcall(debugPrint, "[tpf3mp] main menu: resolveutil.loadfile is wrapped")
"#;

// Construction workers can load industryutil as their very first file.
// Intercept at the native boundary: a Lua wrapper installed during that
// first call cannot wrap the call already in progress.
const INDUSTRY_LOADER: &str = concat!(
    include_str!("industry_order.lua"),
    r#"
return function(...)
    local ru = resolveutil
    ru.__tpf3mp_industry_raw = true
    local ok, chunk, err = pcall(ru.loadfile, "::/industries/industryutil.lua")
    ru.__tpf3mp_industry_raw = nil
    if not ok then error(chunk) end
    if not chunk then error(err or "TPF3-MP: industry utility unavailable") end
    return industry_order(chunk(...))
end, nil
"#
);
const INDUSTRY_RAW: &str =
    "return resolveutil and resolveutil.__tpf3mp_industry_raw and 'raw' or nil";

/// The two files of the mod whose loading is really a request to the hook.
/// The loader's glue parses the path as a mod URI and checks the file exists
/// before the body runs, so they are real files (`mod/tpf3mp_1/content/tpf3mp/`);
/// the hook answers before the body would read them. The body sees the URI's
/// path component, which is what these are compared with.
pub const STATE_FILE: &str = "tpf3mp/state.lua";
/// The action request. The loader takes exactly one argument, so the window
/// leaves the action's JSON in `resolveutil.__tpf3mp_action` first and the
/// hook reads it from there ([`ACTION_CHUNK`]).
pub const ACT_FILE: &str = "tpf3mp/act.lua";

/// Run in the state to fetch a pending action; leaves the JSON (or nil) on the
/// stack and clears the field.
const ACTION_CHUNK: &str =
    "local a = resolveutil.__tpf3mp_action; resolveutil.__tpf3mp_action = nil; return a";

/// The addresses the profile resolved, as this process sees them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Targets {
    pub loadfile: usize,
    pub cached_loadfile: usize,
    pub lua_load: usize,
    pub lua_pcallk: usize,
    pub lua_settop: usize,
    pub lua_pushlstring: usize,
    pub lua_tolstring: usize,
}

/// The original loader body (the detour's trampoline); null until installed.
static ORIGINAL: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static LUA_LOAD: AtomicUsize = AtomicUsize::new(0);
static LUA_PCALLK: AtomicUsize = AtomicUsize::new(0);
static LUA_SETTOP: AtomicUsize = AtomicUsize::new(0);
static LUA_PUSHLSTRING: AtomicUsize = AtomicUsize::new(0);
static LUA_TOLSTRING: AtomicUsize = AtomicUsize::new(0);
static MANAGER_WINDOW_SUPPORTED: AtomicBool = AtomicBool::new(false);
/// The Lua states already patched.
static PATCHED: Mutex<Vec<usize>> = Mutex::new(Vec::new());
/// Keeps the detour armed for the life of the process.
static DETOUR: Mutex<Option<InlineDetour>> = Mutex::new(None);
static CACHE_DETOUR: Mutex<Option<InlineDetour>> = Mutex::new(None);
static CACHE_ORIGINAL: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
/// The hook's log, for what happens at run time.
static LOG: Mutex<Option<File>> = Mutex::new(None);

type LuaReader = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut usize) -> *const c_char;
type LuaLoad = unsafe extern "C" fn(
    *mut c_void,
    LuaReader,
    *mut c_void,
    *const c_char,
    *const c_char,
) -> c_int;
type LuaPcallk =
    unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int, c_int, *const c_void) -> c_int;
type LuaSettop = unsafe extern "C" fn(*mut c_void, c_int);
type LuaPushlstring = unsafe extern "C" fn(*mut c_void, *const c_char, usize) -> *const c_char;
type LuaTolstring = unsafe extern "C" fn(*mut c_void, c_int, *mut usize) -> *const c_char;

/// The profile targets the entry uses, none of which another part of the
/// hook detours.
pub const TARGETS: [&str; 7] = [
    "lua_loadfile",
    "lua_cached_loadfile",
    "lua_load",
    "lua_pcallk",
    "lua_settop",
    "lua_pushlstring",
    "lua_tolstring",
];

/// `profile` with only the entry's own targets ([`TARGETS`]). The hook's
/// other detours (the step gate first) are installed before the entry, so
/// their signatures no longer match the running code, and a required one
/// would refuse the whole profile (seen in the game, 2026-09-30).
pub fn own_targets(profile: &Profile) -> Profile {
    let mut own = profile.clone();
    own.targets
        .retain(|target| TARGETS.contains(&target.name.as_str()));
    own
}

/// Resolves the targets against the running image's `.text`.
pub fn resolve_targets(profile: &Profile) -> Result<Targets, String> {
    let (text_addr, text_len) =
        main_module_text().ok_or_else(|| "cannot find the main module's .text".to_owned())?;
    // SAFETY: `text_addr..+text_len` is the mapped, readable code section.
    let image = unsafe { std::slice::from_raw_parts(text_addr as *const u8, text_len) };
    let resolved = profile::resolve(&own_targets(profile), image, text_addr as u64)
        .map_err(|refusal| refusal.to_string())?;
    let address = |name: &str| -> Result<usize, String> {
        resolved
            .get(name)
            .map(|target| target.address as usize)
            .ok_or_else(|| format!("the profile has no target {name:?}"))
    };
    Ok(Targets {
        loadfile: address("lua_loadfile")?,
        cached_loadfile: address("lua_cached_loadfile")?,
        lua_load: address("lua_load")?,
        lua_pcallk: address("lua_pcallk")?,
        lua_settop: address("lua_settop")?,
        lua_pushlstring: address("lua_pushlstring")?,
        lua_tolstring: address("lua_tolstring")?,
    })
}

/// Arms the menu entry: detours the loader body at `targets.loadfile`.
///
/// # Safety
///
/// Call once, before the game's code runs (the launcher loads the hook into
/// the suspended game), with targets resolved from a matching profile.
pub unsafe fn install(
    targets: &Targets,
    log_path: Option<&Path>,
    manager_window_supported: bool,
) -> Result<(), String> {
    MANAGER_WINDOW_SUPPORTED.store(manager_window_supported, Ordering::Release);
    if let Some(path) = log_path {
        let file = OpenOptions::new().create(true).append(true).open(path).ok();
        *LOG.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = file;
    }
    LUA_LOAD.store(targets.lua_load, Ordering::Release);
    LUA_PCALLK.store(targets.lua_pcallk, Ordering::Release);
    LUA_SETTOP.store(targets.lua_settop, Ordering::Release);
    LUA_PUSHLSTRING.store(targets.lua_pushlstring, Ordering::Release);
    LUA_TOLSTRING.store(targets.lua_tolstring, Ordering::Release);
    // SAFETY: the caller guarantees a quiescent, prologue-verified target; the
    // thunk forwards every argument and the stack untouched.
    let detour = unsafe {
        InlineDetour::install(
            targets.loadfile as *mut u8,
            entry_thunk as *const () as *const u8,
        )
    }
    .map_err(|error| error.to_string())?;
    ORIGINAL.store(detour.trampoline() as *mut u8, Ordering::Release);
    *DETOUR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(detour);
    // SAFETY: the matching profile verifies the cache entry prologue and ABI.
    let cache = unsafe {
        InlineDetour::install(
            targets.cached_loadfile as *mut u8,
            cached_loadfile as *const () as *const u8,
        )
    }
    .map_err(|error| error.to_string())?;
    CACHE_ORIGINAL.store(cache.trampoline() as *mut u8, Ordering::Release);
    *CACHE_DETOUR.lock().unwrap_or_else(|e| e.into_inner()) = Some(cache);
    Ok(())
}

/// Cache lookup precedes the loader body, including in brand-new Lua states.
/// Intercept before lookup so a cached original chunk cannot bypass the fix.
unsafe extern "system" fn cached_loadfile(
    cache: *mut c_void,
    holder: *mut *mut c_void,
    uri: *const u8,
) -> u8 {
    if uri_part(uri, 0x20).as_deref() == Some("industries/industryutil.lua")
        && uri_part(uri, 0).as_deref() == Some("")
        && !holder.is_null()
    {
        // SAFETY: the native caller supplies a live lua::State holder.
        let state = unsafe { *holder };
        if !state.is_null()
            && eval_string(state, INDUSTRY_RAW).as_deref() != Some("raw")
            && push_industry_loader(state)
        {
            return 1;
        }
    }
    let original = CACHE_ORIGINAL.load(Ordering::Acquire);
    // SAFETY: installed before the suspended game resumes; exact native ABI.
    let original = unsafe {
        std::mem::transmute::<
            *mut u8,
            unsafe extern "system" fn(*mut c_void, *mut *mut c_void, *const u8) -> u8,
        >(original)
    };
    unsafe { original(cache, holder, uri) }
}

fn note(message: &str) {
    if let Some(file) = LOG
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_mut()
    {
        let _ = writeln!(file, "[menu] {message}");
    }
}

/// The main module's `.text` section in memory: address and length.
fn main_module_text() -> Option<(usize, usize)> {
    // SAFETY: a null module name asks for the process's own image base.
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    if base == 0 {
        return None;
    }
    // SAFETY: the PE headers are mapped and readable at the image base; 0x1000
    // bytes covers the DOS header, PE header and section table of a game exe.
    let header = unsafe { std::slice::from_raw_parts(base as *const u8, 0x1000) };
    let pe = PeHeaders::parse(header).ok()?;
    let text = pe.section(".text")?;
    let addr = base.checked_add(text.virtual_address as usize)?;
    Some((addr, text.virtual_size as usize))
}

/// The Lua state the loader lambda works on: `**(closure + 0x10)`.
fn lua_state_of(closure: *const u8) -> Option<*mut c_void> {
    if closure.is_null() {
        return None;
    }
    // SAFETY: the closure is the lambda's own object; its field at +0x10 points
    // at a holder whose first word is the lua_State*, as the loader body reads
    // it before its own lua_pcallk call.
    let holder = unsafe { *(closure.add(0x10) as *const *const *mut c_void) };
    if holder.is_null() {
        return None;
    }
    let state = unsafe { *holder };
    (!state.is_null()).then_some(state)
}

/// The path the loader was asked for: the `std::string` at `*(closure + 8) +
/// 0x20`, read the way the body reads it (MSVC layout: a heap pointer at +0
/// when the capacity at +0x18 is 16 or more, else the bytes inline; the size
/// at +0x10). `None` for anything that does not look like one.
fn request_path(closure: *const u8) -> Option<String> {
    request_part(closure, 0x20)
}

/// Loader.cpp's translation preamble (0x2fa4fc0) reads the URI's owning
/// mod ID from its first std::string. Empty is the base namespace.
fn request_namespace(closure: *const u8) -> Option<String> {
    request_part(closure, 0)
}

fn request_part(closure: *const u8, offset: usize) -> Option<String> {
    if closure.is_null() {
        return None;
    }
    // SAFETY: as in `lua_state_of`; the field at +8 points at the object that
    // holds the requested path, `tpfre` showed the body adding 0x20 to it.
    let holder = unsafe { *(closure.add(8) as *const *const u8) };
    uri_part(holder, offset)
}

fn uri_part(holder: *const u8, offset: usize) -> Option<String> {
    if holder.is_null() {
        return None;
    }
    let string = unsafe { holder.add(offset) };
    let size = unsafe { *(string.add(0x10) as *const usize) };
    let capacity = unsafe { *(string.add(0x18) as *const usize) };
    if size > capacity || size > 64 * 1024 {
        return None;
    }
    let data = if capacity >= 16 {
        unsafe { *(string as *const *const u8) }
    } else {
        string
    };
    if data.is_null() {
        return None;
    }
    // SAFETY: `size` bytes at `data` are the string's contents.
    let bytes = unsafe { std::slice::from_raw_parts(data, size) };
    String::from_utf8(bytes.to_vec()).ok()
}

/// One buffer handed to `lua_load` through [`read_chunk`], whole, once.
struct Chunk {
    ptr: *const u8,
    len: usize,
    done: bool,
}

unsafe extern "C" fn read_chunk(
    _state: *mut c_void,
    data: *mut c_void,
    size: *mut usize,
) -> *const c_char {
    // SAFETY: `data` is the `Chunk` the caller passed to lua_load.
    let chunk = unsafe { &mut *data.cast::<Chunk>() };
    if chunk.done {
        // SAFETY: `size` is lua_load's out-parameter.
        unsafe { *size = 0 };
        return std::ptr::null();
    }
    chunk.done = true;
    // SAFETY: as above.
    unsafe { *size = chunk.len };
    chunk.ptr.cast::<c_char>()
}

/// Runs [`PATCH`] in `state` the first time the state is seen.
fn patch_once(state: *mut c_void) {
    let mut seen = PATCHED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if seen.contains(&(state as usize)) {
        return;
    }
    seen.push(state as usize);

    let load = LUA_LOAD.load(Ordering::Acquire);
    let pcallk = LUA_PCALLK.load(Ordering::Acquire);
    let settop = LUA_SETTOP.load(Ordering::Acquire);
    if load == 0 || pcallk == 0 || settop == 0 {
        note("Lua API not resolved; the menu stays the game's");
        return;
    }
    // SAFETY: the addresses are the profile-verified Lua 5.2 API functions.
    let (load, pcallk, settop) = unsafe {
        (
            std::mem::transmute::<usize, LuaLoad>(load),
            std::mem::transmute::<usize, LuaPcallk>(pcallk),
            std::mem::transmute::<usize, LuaSettop>(settop),
        )
    };
    let patch = menu_patch(MANAGER_WINDOW_SUPPORTED.load(Ordering::Acquire));
    let mut chunk = Chunk {
        ptr: patch.as_ptr(),
        len: patch.len(),
        done: false,
    };
    // SAFETY: a live lua_State on its own thread, a reader over a static
    // buffer, and NUL-terminated chunk name and mode.
    let status = unsafe {
        load(
            state,
            read_chunk,
            (&mut chunk as *mut Chunk).cast::<c_void>(),
            c"=tpf3mp-menu".as_ptr(),
            c"t".as_ptr(),
        )
    };
    if status != 0 {
        // The error message is on the stack; pop it.
        unsafe { settop(state, -2) };
        note(&format!(
            "lua_load of the menu patch failed (status {status}) in state {state:p}"
        ));
        return;
    }
    // SAFETY: the compiled chunk is on the stack; no arguments, no results.
    let status = unsafe { pcallk(state, 0, 0, 0, 0, std::ptr::null()) };
    if status != 0 {
        unsafe { settop(state, -2) };
        note(&format!(
            "the menu patch raised (status {status}) in state {state:p}"
        ));
        return;
    }
    note(&format!("menu patch installed in Lua state {state:p}"));
    run_plugin_lua(state, load, pcallk, settop);
    // A main page this state loaded before the patch was in it is the
    // game's own: say so plainly.
    if let Some(key) = eval_string(state, MAIN_PAGE_LOADED) {
        note(&format!(
            "main_page.tl MISSED: {key} was loaded in state {state:p} before the menu patch; the main menu is the game's own, without the Multiplayer entry"
        ));
    }
}

/// Runs the Lua native mods' plugins handed over (`crate::plugins`), in the
/// load order, in `state`, after the hook's own patch. One that fails to
/// load or raises is logged and the next still runs: a plugin's Lua is its
/// own, and so is its failure.
#[allow(unsafe_code)]
fn run_plugin_lua(state: *mut c_void, load: LuaLoad, pcallk: LuaPcallk, settop: LuaSettop) {
    for (at, text) in crate::plugins::lua_chunks().iter().enumerate() {
        let mut chunk = Chunk {
            ptr: text.as_ptr(),
            len: text.len(),
            done: false,
        };
        // SAFETY: as for the hook's own patch: a live lua_State on its own
        // thread, a reader over a buffer that outlives the call, and
        // NUL-terminated name and mode.
        let status = unsafe {
            load(
                state,
                read_chunk,
                (&mut chunk as *mut Chunk).cast::<c_void>(),
                c"=tpf3mp-native-mod".as_ptr(),
                c"t".as_ptr(),
            )
        };
        // SAFETY: the compiled chunk is on the stack; no arguments, no
        // results.
        let status = if status == 0 {
            unsafe { pcallk(state, 0, 0, 0, 0, std::ptr::null()) }
        } else {
            status
        };
        if status != 0 {
            // The error message is on the stack; pop it.
            unsafe { settop(state, -2) };
            note(&format!(
                "native mods' Lua chunk {at} failed (status {status}) in state {state:p}"
            ));
        }
    }
}

/// Run in a state as the patch goes in: the key of a main page already
/// loaded there, or nil.
const MAIN_PAGE_LOADED: &str = r#"
for key in pairs(_ug_loadedModules or {}) do
	if type(key) == "string" and key:find("gui/menu/main_page%.tl$") then return key end
end
return nil
"#;

/// Whether `path`, as the loader body sees it, is the main page.
fn is_main_page(path: &str) -> bool {
    path.ends_with("gui/menu/main_page.tl")
}

/// Runs `chunk` (no arguments, one result) in `state` and takes the result as
/// a string, leaving the stack as it was. `None` for a load or run error, or
/// a result that is not a string.
fn eval_string(state: *mut c_void, chunk: &'static str) -> Option<String> {
    let load = LUA_LOAD.load(Ordering::Acquire);
    let pcallk = LUA_PCALLK.load(Ordering::Acquire);
    let settop = LUA_SETTOP.load(Ordering::Acquire);
    let tolstring = LUA_TOLSTRING.load(Ordering::Acquire);
    if load == 0 || pcallk == 0 || settop == 0 || tolstring == 0 || state.is_null() {
        return None;
    }
    // SAFETY: the addresses are the profile-verified Lua 5.2 API functions.
    let (load, pcallk, settop, tolstring) = unsafe {
        (
            std::mem::transmute::<usize, LuaLoad>(load),
            std::mem::transmute::<usize, LuaPcallk>(pcallk),
            std::mem::transmute::<usize, LuaSettop>(settop),
            std::mem::transmute::<usize, LuaTolstring>(tolstring),
        )
    };
    let mut reader = Chunk {
        ptr: chunk.as_ptr(),
        len: chunk.len(),
        done: false,
    };
    // SAFETY: a live lua_State on its own thread, a reader over a static
    // buffer, NUL-terminated chunk name and mode; every branch pops what it
    // pushed.
    unsafe {
        let status = load(
            state,
            read_chunk,
            (&mut reader as *mut Chunk).cast::<c_void>(),
            c"=tpf3mp-action".as_ptr(),
            c"t".as_ptr(),
        );
        if status != 0 {
            settop(state, -2);
            note(&format!("the action chunk did not load (status {status})"));
            return None;
        }
        let status = pcallk(state, 0, 1, 0, 0, std::ptr::null());
        if status != 0 {
            settop(state, -2);
            note(&format!("the action chunk raised (status {status})"));
            return None;
        }
        let mut len = 0usize;
        let ptr = tolstring(state, -1, &mut len);
        let text = if ptr.is_null() {
            None
        } else {
            String::from_utf8(std::slice::from_raw_parts(ptr.cast::<u8>(), len).to_vec()).ok()
        };
        settop(state, -2);
        text
    }
}

/// Answers a request from the lobby window, or `None` when `path` is an
/// ordinary file the loader should handle. Each request exchanges the lobby
/// with the launcher first ([`crate::install::lobby_pump`]): at the main
/// menu nothing else reads the link.
fn answer(state: *mut c_void, path: &str) -> Option<String> {
    if path == STATE_FILE {
        crate::install::lobby_pump();
        return Some(lobby::state().to_lua());
    }
    if path != ACT_FILE {
        return None;
    }
    let Some(json) = eval_string(state, ACTION_CHUNK) else {
        note("lobby action refused: no JSON in resolveutil.__tpf3mp_action");
        return Some("error: no action".to_owned());
    };
    if let Some(done) = lobby::local_action(&json) {
        return Some(match done {
            Ok(()) => "ok".to_owned(),
            Err(error) => {
                note(&format!("lobby action refused: {error}"));
                format!("error: {error}")
            }
        });
    }
    Some(
        match lobby::parse_action(&json).and_then(|action| {
            note(&format!(
                "lobby action {} for the launcher",
                lobby::kind(&action)
            ));
            lobby::queue(action)
        }) {
            Ok(()) => {
                crate::install::lobby_pump();
                "ok".to_owned()
            }
            Err(error) => {
                note(&format!("lobby action refused: {error}"));
                format!("error: {error}")
            }
        },
    )
}

/// Pushes `reply` for Lua, twice: the loader's glue always hands Lua two
/// results, and the window reads the first.
fn push_reply(state: *mut c_void, reply: &str) -> bool {
    let push = LUA_PUSHLSTRING.load(Ordering::Acquire);
    if push == 0 {
        return false;
    }
    // SAFETY: the profile-verified lua_pushlstring; the bytes are copied.
    let push = unsafe { std::mem::transmute::<usize, LuaPushlstring>(push) };
    for _ in 0..2 {
        unsafe { push(state, reply.as_ptr().cast::<c_char>(), reply.len()) };
    }
    true
}

/// Supply the loader's two results (chunk, nil) without executing the module.
fn push_industry_loader(state: *mut c_void) -> bool {
    let load = LUA_LOAD.load(Ordering::Acquire);
    let pcallk = LUA_PCALLK.load(Ordering::Acquire);
    let settop = LUA_SETTOP.load(Ordering::Acquire);
    if load == 0 || pcallk == 0 || settop == 0 {
        return false;
    }
    // SAFETY: profile-verified Lua API; this callback owns the live state.
    let (load, pcallk, settop) = unsafe {
        (
            std::mem::transmute::<usize, LuaLoad>(load),
            std::mem::transmute::<usize, LuaPcallk>(pcallk),
            std::mem::transmute::<usize, LuaSettop>(settop),
        )
    };
    let mut chunk = Chunk {
        ptr: INDUSTRY_LOADER.as_ptr(),
        len: INDUSTRY_LOADER.len(),
        done: false,
    };
    // SAFETY: static source, valid reader context, terminated name/mode.
    let status = unsafe {
        load(
            state,
            read_chunk,
            (&mut chunk as *mut Chunk).cast(),
            c"=tpf3mp-industry".as_ptr(),
            c"t".as_ptr(),
        )
    };
    let status = if status == 0 {
        // SAFETY: the compiled outer chunk is atop the stack; two results.
        unsafe { pcallk(state, 0, 2, 0, 0, std::ptr::null()) }
    } else {
        status
    };
    if status != 0 {
        // SAFETY: discard the single load/call error, preserving caller args.
        unsafe { settop(state, -2) };
        note(&format!("industry loader failed with Lua status {status}"));
        return false;
    }
    note(&format!("industry loader supplied in Lua state {state:p}"));
    true
}

/// Records the first few paths read from the closure, and every one that
/// mentions the mod, so a wrong offset shows up in the hook's log.
fn diagnose_path(closure: *const u8) {
    static SEEN: AtomicUsize = AtomicUsize::new(0);
    let path = request_path(closure);
    let mentions = path
        .as_deref()
        .is_some_and(|p| p.contains("tpf3mp") && p != STATE_FILE && p != ACT_FILE);
    let n = SEEN.fetch_add(1, Ordering::Relaxed);
    if n < 8 || mentions {
        note(&format!("loader call #{n}: path = {path:?}"));
    }
}

/// Called by [`entry_thunk`] with the four saved argument registers; returns
/// the code to tail-jump into: the original body, or [`reply_stub`] once a
/// request has been answered.
extern "C" fn on_entry(args: *const u64) -> *const u8 {
    let mut original = ORIGINAL.load(Ordering::Acquire);
    while original.is_null() {
        std::hint::spin_loop();
        original = ORIGINAL.load(Ordering::Acquire);
    }
    // SAFETY: the thunk saved four argument words at `args`; the first is rcx,
    // the lambda's closure.
    let closure = unsafe { *args } as *const u8;
    let Some(state) = lua_state_of(closure) else {
        return original;
    };
    diagnose_path(closure);
    if let Some(reply) = request_path(closure).and_then(|path| answer(state, &path)) {
        if push_reply(state, &reply) {
            return reply_stub as *const () as *const u8;
        }
        return original;
    }
    let patched = PATCHED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(&(state as usize));
    if request_path(closure).is_some_and(|path| path.ends_with("industries/industryutil.lua")) {
        note(&format!(
            "industry loader state={state:p} patched={patched} namespace={:?}",
            request_namespace(closure)
        ));
    }
    if request_path(closure).is_some_and(|path| is_main_page(&path)) {
        // In a patched state the patch's wrap has asked for the mod's copy
        // (the body sees only the path, the same for both); in one the
        // patch is not in yet, the game's own page loads.
        note(if patched {
            "main_page.tl SERVED: the main menu's page comes from the mod, with the Multiplayer entry"
        } else {
            "main_page.tl MISSED: the main menu's page loaded before the menu patch was in its Lua state; the main menu is the game's own"
        });
    }
    patch_once(state);
    original
}

/// Where a request returns to instead of the loader body: the reply is
/// already on the Lua stack, so there is nothing left to do but return to the
/// glue, which ignores the value.
#[unsafe(naked)]
unsafe extern "C" fn reply_stub() {
    core::arch::naked_asm!("xor eax, eax", "ret");
}

/// The detour installed on the loader body. Saves the argument registers
/// (integer and vector), calls [`on_entry`], restores them and tail-jumps
/// into the original with the stack exactly as the caller left it. The frame
/// layout and alignment are those of the tracer's entry thunk
/// (`crates/tpf3mp-trace`), which its test validates.
#[unsafe(naked)]
unsafe extern "C" fn entry_thunk() {
    core::arch::naked_asm!(
        "sub rsp, 0x88",
        "mov [rsp+0x20], rcx",
        "mov [rsp+0x28], rdx",
        "mov [rsp+0x30], r8",
        "mov [rsp+0x38], r9",
        "movaps [rsp+0x40], xmm0",
        "movaps [rsp+0x50], xmm1",
        "movaps [rsp+0x60], xmm2",
        "movaps [rsp+0x70], xmm3",
        "lea rcx, [rsp+0x20]",
        "call {on_entry}",
        "mov rcx, [rsp+0x20]",
        "mov rdx, [rsp+0x28]",
        "mov r8, [rsp+0x30]",
        "mov r9, [rsp+0x38]",
        "movaps xmm0, [rsp+0x40]",
        "movaps xmm1, [rsp+0x50]",
        "movaps xmm2, [rsp+0x60]",
        "movaps xmm3, [rsp+0x70]",
        "add rsp, 0x88",
        "jmp rax",
        on_entry = sym on_entry,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_resolves_its_own_targets_only() {
        let built_in = crate::built_in_profiles();
        let profile = built_in[0].profile.as_ref().unwrap();
        assert!(
            profile
                .targets
                .iter()
                .any(|target| target.name == "GameSim::Step"),
            "the step gate's target is in the profile"
        );
        let own = own_targets(profile);
        let mut names: Vec<&str> = own.targets.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        let mut wanted = TARGETS.to_vec();
        wanted.sort_unstable();
        assert_eq!(names, wanted, "every one of the entry's, and nothing else");
    }

    #[test]
    fn the_reader_hands_the_chunk_over_whole_and_once() {
        let text = b"return 1";
        let mut chunk = Chunk {
            ptr: text.as_ptr(),
            len: text.len(),
            done: false,
        };
        let mut size = 0usize;
        // SAFETY: a valid Chunk and out-parameter.
        let first = unsafe {
            read_chunk(
                std::ptr::null_mut(),
                (&mut chunk as *mut Chunk).cast::<c_void>(),
                &mut size,
            )
        };
        assert_eq!(first.cast::<u8>(), text.as_ptr());
        assert_eq!(size, text.len());
        let second = unsafe {
            read_chunk(
                std::ptr::null_mut(),
                (&mut chunk as *mut Chunk).cast::<c_void>(),
                &mut size,
            )
        };
        assert!(second.is_null());
        assert_eq!(size, 0);
    }

    #[test]
    fn the_patch_serves_the_line_managers_window_from_the_mod() {
        assert!(PATCH.contains(r#"path:find("gui/line_vehicle_mgmt/manager_window%.tl$")"#));
        assert!(
            PATCH.contains(
                r#"pcall(orig, "tpf3mp_1::/gui/line_vehicle_mgmt/manager_window.tl", ...)"#
            )
        );
        assert!(
            PATCH.contains("the window stays the game's"),
            "a copy that does not load must fall back"
        );
    }

    #[test]
    fn the_line_managers_window_is_the_mods_and_the_games_where_the_copy_fails() {
        let lua = mlua::Lua::new();
        lua.load(
            r#"
            debugPrint = function() end
            BROKEN = false
            resolveutil = { loadfile = function(path)
                if BROKEN and path:find("^tpf3mp_1::") then return nil, "a type error" end
                return function() return path end
            end }
            "#,
        )
        .exec()
        .unwrap();
        lua.load(menu_patch(true)).exec().unwrap();
        let load = |path: &str| -> String {
            lua.load(format!("return resolveutil.loadfile({path:?})()"))
                .eval()
                .unwrap()
        };
        assert_eq!(
            load("::/gui/line_vehicle_mgmt/manager_window.tl"),
            "tpf3mp_1::/gui/line_vehicle_mgmt/manager_window.tl"
        );
        assert_eq!(
            load("::/gui/line_vehicle_mgmt/line_util.tl"),
            "::/gui/line_vehicle_mgmt/line_util.tl"
        );
        lua.load("BROKEN = true").exec().unwrap();
        assert_eq!(
            load("::/gui/line_vehicle_mgmt/manager_window.tl"),
            "::/gui/line_vehicle_mgmt/manager_window.tl",
            "the game's own when the copy does not load"
        );
    }

    #[test]
    fn the_line_manager_copy_is_not_served_for_an_other_build() {
        let lua = mlua::Lua::new();
        lua.load(
            r#"
            debugPrint = function() end
            resolveutil = { loadfile = function(path)
                return function() return path end
            end }
            "#,
        )
        .exec()
        .unwrap();
        lua.load(menu_patch(false)).exec().unwrap();
        let path: String = lua
            .load(r#"return resolveutil.loadfile("::/gui/line_vehicle_mgmt/manager_window.tl")()"#)
            .eval()
            .unwrap();
        assert_eq!(path, "::/gui/line_vehicle_mgmt/manager_window.tl");
        let menu: String = lua
            .load(r#"return resolveutil.loadfile("::/gui/menu/main_page.tl")()"#)
            .eval()
            .unwrap();
        assert_eq!(menu, "tpf3mp_1::/gui/menu/main_page.tl");
    }

    #[test]
    fn the_line_manager_copy_is_pinned_to_the_verified_40420_executable() {
        let current = Profile::from_toml(include_str!(
            "../../../profiles/tf3_build40420_steam_windows/hooks.toml"
        ))
        .unwrap();
        let previous = Profile::from_toml(include_str!(
            "../../../profiles/tf3_build40408_steam_windows/hooks.toml"
        ))
        .unwrap();
        assert!(manager_window_supported(&current));
        assert!(!manager_window_supported(&previous));
    }

    #[test]
    fn the_patch_redirects_only_the_games_main_page() {
        assert!(PATCH.contains(r#"path:find("gui/menu/main_page%.tl$")"#));
        assert!(PATCH.contains(r#"not path:find("^tpf3mp_1::")"#));
        assert!(PATCH.contains(r#"pcall(orig, "tpf3mp_1::/gui/menu/main_page.tl", ...)"#));
        assert!(
            PATCH.contains("the menu stays the game's"),
            "a missing mod copy must fall back"
        );
        assert!(
            PATCH.contains("ru.__tpf3mp_menu"),
            "the wrap must be idempotent"
        );
    }

    #[test]
    fn industry_loader_covers_first_load_and_clears_recursion_guard_on_errors() {
        let lua = mlua::Lua::new();
        lua.load(
            r#"
            debugPrint = function() end
            resolveutil = {loadfile = function(path)
                assert(resolveutil.__tpf3mp_industry_raw)
                assert(path == '::/industries/industryutil.lua')
                if fail == 'missing' then return nil, 'missing' end
                if fail == 'throw' then error('load failed') end
                return function(arg)
                    return {makeIndustryUpdateFn = function(data) return function() return data end end,
                        path = path, arg = arg}
                end, 'loader status'
            end}
        "#,
        )
        .exec()
        .unwrap();
        let (chunk, error): (mlua::Function, mlua::Value) =
            lua.load(INDUSTRY_LOADER).eval().unwrap();
        assert!(error.is_nil());
        lua.globals().set("load_industry", chunk).unwrap();
        lua.load(
            r#"
            do
                local module = load_industry(42)
                assert(module.arg == 42 and module.__tpf3mp_ordered)
                assert(not resolveutil.__tpf3mp_industry_raw)
                local data = module.makeIndustryUpdateFn({z=1, a=2})()
                local iter = getmetatable(data).__pairs(data)
                assert(iter() == 'a' and iter() == 'z')
            end
            for _, mode in ipairs({'missing', 'throw'}) do
                fail = mode
                assert(not pcall(load_industry))
                assert(not resolveutil.__tpf3mp_industry_raw)
            end
        "#,
        )
        .exec()
        .unwrap();
    }

    #[test]
    fn nothing_is_read_from_a_null_closure() {
        assert!(lua_state_of(std::ptr::null()).is_none());
        assert!(request_path(std::ptr::null()).is_none());
    }

    /// A closure laid out as the loader's: the path object at +8 holding an
    /// MSVC std::string at +0x20, both inline (short) and on the heap (long).
    #[test]
    fn the_requested_path_is_read_from_the_closure() {
        fn closure_with(path: &[u8]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
            let mut object = vec![0u8; 0x40];
            let heap = path.to_vec();
            if path.len() < 16 {
                object[0x20..0x20 + path.len()].copy_from_slice(path);
                object[0x38..0x40].copy_from_slice(&15usize.to_ne_bytes());
            } else {
                object[0x20..0x28].copy_from_slice(&(heap.as_ptr() as usize).to_ne_bytes());
                object[0x38..0x40].copy_from_slice(&heap.capacity().to_ne_bytes());
            }
            object[0x30..0x38].copy_from_slice(&path.len().to_ne_bytes());
            let mut closure = vec![0u8; 0x20];
            closure[8..16].copy_from_slice(&(object.as_ptr() as usize).to_ne_bytes());
            (closure, object, heap)
        }
        let (closure, _object, _heap) = closure_with(b"scripts/a.lua");
        assert_eq!(
            request_path(closure.as_ptr()).as_deref(),
            Some("scripts/a.lua")
        );
        let (closure, _object, _heap) = closure_with(STATE_FILE.as_bytes());
        assert_eq!(request_path(closure.as_ptr()).as_deref(), Some(STATE_FILE));
        assert_eq!(request_namespace(closure.as_ptr()).as_deref(), Some(""));
        let (closure, mut object, _heap) = closure_with(b"industries/industryutil.lua");
        object[..9].copy_from_slice(b"other_mod");
        object[0x10..0x18].copy_from_slice(&9usize.to_ne_bytes());
        object[0x18..0x20].copy_from_slice(&15usize.to_ne_bytes());
        assert_eq!(
            request_namespace(closure.as_ptr()).as_deref(),
            Some("other_mod")
        );
        assert_eq!(
            request_path(closure.as_ptr()).as_deref(),
            Some("industries/industryutil.lua")
        );
    }

    #[test]
    fn the_state_file_is_answered_and_other_files_are_not() {
        let _serial = crate::lua::tests::SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::lobby::reset();
        let none = std::ptr::null_mut();
        let state = answer(none, STATE_FILE).expect("the state file is answered");
        assert!(state.starts_with("{ connection = "), "{state}");
        assert!(state.contains(r#"connection = "disconnected""#), "{state}");
        assert!(
            state.contains("linked = false"),
            "no step driver in this test: {state}"
        );
        assert!(answer(none, "gui/main/react.lua").is_none());
        assert!(answer(none, "tpf3mp/other.lua").is_none());
        // Without a Lua state (or before lua_tolstring is resolved) an action
        // request is refused, never applied.
        assert_eq!(answer(none, ACT_FILE).as_deref(), Some("error: no action"));
    }
}
