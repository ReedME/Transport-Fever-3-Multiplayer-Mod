//! Saving and loading whole worlds in the game (docs/HOOKS.md, "The room's
//! world"): the files, and what the hook asks of the mod's GUI for them.
//!
//! The game saves and loads through its own script API, which only the
//! GUI's Lua can call (`app.saveGame`, `app.loadGame`), and only into and
//! from its own save folder: `<Steam>/userdata/<account>/3493540/local/save`.
//! So a save the room orders is made under a name of this game's own
//! (`tpf3mp_<pid>_<event>`, as two games on one PC share the folder), found
//! there once the GUI says it is written, and moved to the room's file; the
//! picture the game writes beside it is removed. A world the room hands
//! over is copied into the folder as `tpf3mp_room_<pid>` and loaded from
//! there, by the GUI of the world the game has up, or by the game's main menu
//! when it has none (`crate::menu`); it stays until the next one replaces
//! it.
//!
//! Each game has its own process id, so each game played leaves its own
//! copy, a whole world each. [`sweep`] removes those of games no longer
//! running: when the hook starts, and each time a new copy is written
//! (docs/HOOKS.md, "The room's world").

use std::{
    fs,
    path::{Path, PathBuf},
};

use tpf3mp_bridge::Notice;
use tpf3mp_proto::PlayerId;

use crate::{
    lua,
    step::{GameControl, LoadFrom, WorldMark},
};

/// Transport Fever 3's Steam app.
pub const STEAM_APP: &str = "3493540";

/// The game's side of saves and loads, through the mod's GUI.
pub struct GuiWorlds {
    folder: Folder,
    tag: String,
}

/// Where the game saves.
enum Folder {
    /// Given: the folder, or why there is none.
    Given(Result<PathBuf, String>),
    /// The Steam account's, found when first needed (Steam's API answers
    /// only once the game started it), then kept.
    Steam(Option<PathBuf>),
}

impl GuiWorlds {
    /// For the game's own save folder, as Steam names it.
    pub fn in_steam_folder() -> Self {
        Self {
            folder: Folder::Steam(None),
            tag: std::process::id().to_string(),
        }
    }

    /// For the save folder `folder`, or why there is none. Removes the
    /// copies games no longer running left there ([`sweep`]).
    pub fn in_folder(folder: Result<PathBuf, String>) -> Self {
        let worlds = Self {
            folder: Folder::Given(folder),
            tag: std::process::id().to_string(),
        };
        worlds.sweep();
        worlds
    }

    /// Removes the copies of games no longer running from the save folder,
    /// and logs what it removed.
    fn sweep(&self) {
        let folder = match &self.folder {
            Folder::Given(Ok(folder)) | Folder::Steam(Some(folder)) => folder,
            _ => return,
        };
        let swept = sweep(folder, std::process::id(), running);
        for line in swept.lines() {
            crate::install::log_line(&line);
        }
    }

    fn folder(&mut self) -> Result<PathBuf, String> {
        match &mut self.folder {
            Folder::Given(folder) => folder.clone(),
            Folder::Steam(Some(folder)) => Ok(folder.clone()),
            Folder::Steam(kept) => {
                let (folder, from) = save_folder()?;
                crate::install::log_line(&format!(
                    "the game's save folder is {} ({from})",
                    folder.display()
                ));
                *kept = Some(folder.clone());
                Ok(folder)
            }
        }
    }
}

/// What [`sweep`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Swept {
    /// The files removed, by name.
    pub removed: Vec<String>,
    /// Their bytes.
    pub bytes: u64,
    /// The files that could not be removed, with why.
    pub failed: Vec<(String, String)>,
}

impl Swept {
    /// For the hook's log.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if !self.removed.is_empty() {
            lines.push(format!(
                "removed {} room save copies of games no longer running from the save folder ({} MB): {}",
                self.removed.len(),
                self.bytes / 1_000_000,
                self.removed.join(", ")
            ));
        }
        for (name, why) in &self.failed {
            lines.push(format!(
                "could not remove the old room save copy {name}: {why}"
            ));
        }
        lines
    }
}

/// The game whose copy the save folder's file `name` is, if it is one the
/// hook writes, by its exact name: `tpf3mp_room_<pid>.sav` (a room's world,
/// copied in to load) or `tpf3mp_<pid>_<event>.sav` (a save the room
/// ordered, before it moves out), or the `.jpg` picture of either. Any
/// other name, the player's saves', is not.
pub fn copy_of(name: &str) -> Option<u32> {
    let stem = name
        .strip_suffix(".sav")
        .or_else(|| name.strip_suffix(".jpg"))?;
    let rest = stem.strip_prefix("tpf3mp_")?;
    if let Some(pid) = rest.strip_prefix("room_") {
        return pid_of(pid);
    }
    let (pid, event) = rest.split_once('_')?;
    number(event, 20)?;
    pid_of(pid)
}

/// A process id as `format!` writes one: decimal digits, no leading zero.
fn pid_of(text: &str) -> Option<u32> {
    number(text, 10)?
        .try_into()
        .ok()
        .filter(|pid: &u32| *pid != 0)
}

/// A number of at most `digits` decimal digits, as `format!` writes it.
fn number(text: &str, digits: usize) -> Option<u64> {
    let plain = !text.is_empty()
        && text.len() <= digits
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    if plain { text.parse().ok() } else { None }
}

/// Removes from the save folder `folder` the copies ([`copy_of`]) of games
/// no longer running: files only, by their exact names, never those of
/// this game (`own`), whose copy is the world it loads or plays, nor those
/// of a process `running` says may run, another game sharing the folder.
/// Everything else, the player's saves among them, stays. A file that
/// cannot be removed stays too, and is said so.
pub fn sweep(folder: &Path, own: u32, running: impl Fn(u32) -> bool) -> Swept {
    let mut swept = Swept::default();
    let Ok(entries) = fs::read_dir(folder) else {
        return swept;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(pid) = copy_of(&name) else {
            continue;
        };
        if pid == own || running(pid) {
            continue;
        }
        // Not through a link, and not a folder.
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !meta.file_type().is_file() {
            continue;
        }
        match fs::remove_file(entry.path()) {
            Ok(()) => {
                swept.bytes += meta.len();
                swept.removed.push(name);
            }
            Err(error) => swept.failed.push((name, error.to_string())),
        }
    }
    swept
}

/// Whether process `pid` may be running. Fails closed: a process that
/// cannot be asked about is taken as running.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn running(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, GetLastError},
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    /// The exit code of a process still running.
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: a handle opened for querying only, closed once.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            // No such process; anything else (access denied) may be one.
            return GetLastError() != ERROR_INVALID_PARAMETER;
        }
        let mut code = 0u32;
        let asked = GetExitCodeProcess(handle, &raw mut code);
        CloseHandle(handle);
        asked == 0 || code == STILL_ACTIVE
    }
}

/// Query existence without sending a signal, on macOS as well as Linux.
/// Only ESRCH proves absence; permission errors keep the copy untouched.
#[cfg(unix)]
#[allow(unsafe_code)]
pub fn running(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return true;
    }
    // SAFETY: signal zero only queries a positive process ID.
    let exists = unsafe { libc::kill(pid, 0) == 0 };
    exists || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// Whether process `pid` may be running: here, always.
#[cfg(not(any(windows, unix)))]
pub fn running(_pid: u32) -> bool {
    true
}

impl GameControl for GuiWorlds {
    fn request_replay(
        &mut self,
        step: u64,
        actions: &[crate::step::Ordered],
    ) -> Result<(), String> {
        lua::request_replay(step, actions)
    }

    fn replay_result(&mut self) -> Option<Result<(), String>> {
        lua::take_replay_result()
    }

    fn cancel_replay(&mut self) {
        lua::cancel_replay();
    }

    fn request_save(&mut self, name: &str) {
        lua::request_save(name);
    }

    fn save_result(&mut self) -> Option<Result<PathBuf, String>> {
        let answer = lua::take_save_answer()?;
        let result = answer.and_then(|name| {
            let folder = self.folder()?;
            let file = folder.join(format!("{name}.sav"));
            // The picture beside it is the player's save menu's, not the
            // room's.
            let _ = fs::remove_file(folder.join(format!("{name}.jpg")));
            if file.is_file() {
                Ok(file)
            } else {
                Err(format!(
                    "the game says it saved {name}, but {} is not there",
                    file.display()
                ))
            }
        });
        if result.is_ok() {
            self.sweep();
        }
        Some(result)
    }

    fn room_notice(&mut self, notice: &Notice) {
        lua::notice(notice);
    }

    fn set_me(&mut self, player: PlayerId) {
        lua::set_me(player);
    }

    fn set_room(&mut self, room: tpf3mp_proto::RoomId) {
        lua::set_room(room);
    }

    fn set_mods(&mut self, mods: Option<tpf3mp_bridge::ModLists>) {
        lua::set_mods(mods);
    }

    fn request_load(&mut self, file: &Path, from: LoadFrom) -> Result<(), String> {
        let folder = self.folder()?;
        let name = format!("tpf3mp_room_{}", self.tag);
        let target = folder.join(format!("{name}.sav"));
        fs::copy(file, &target).map_err(|error| {
            format!(
                "copying the room's save {} to {}: {error}",
                file.display(),
                target.display()
            )
        })?;
        self.sweep();
        match from {
            LoadFrom::Gui => lua::request_load(&name),
            LoadFrom::Menu => lua::request_menu_load(&name),
        }
        Ok(())
    }

    fn load_done(&mut self) -> bool {
        lua::load_done()
    }

    fn load_failed(&mut self) -> Option<String> {
        lua::take_load_failure()
    }

    fn world_up(&mut self) -> Option<u64> {
        lua::take_world_up()
    }

    fn world_mark(&mut self) -> WorldMark {
        WorldMark {
            closed: crate::menu::world_closes(),
            started: lua::worlds_started(),
        }
    }
}

/// The save folder of the Steam account playing, and where it was found:
/// first as Steam's API names the account's folder for the game, which
/// works under Proton too; else under Steam's folder from the registry, or,
/// under Proton, under the Steam client's folder Proton names, of the
/// account the registry names or the one account with a save folder for
/// the game. Neither: why, for both (fail closed).
fn save_folder() -> Result<(PathBuf, &'static str), String> {
    let from_api = steam_api::user_data_folder().and_then(|local| {
        let folder = local.join("save");
        if folder.is_dir() {
            Ok(folder)
        } else {
            Err(format!("{} is no folder", folder.display()))
        }
    });
    choose(from_api, || {
        let roots = steam_roots();
        if roots.is_empty() {
            return Err("cannot find Steam's folder".to_owned());
        }
        let account = active_account().filter(|account| *account != 0);
        let mut why = Vec::new();
        for root in &roots {
            match save_folder_in(root, account) {
                Ok(folder) => return Ok(folder),
                Err(error) => why.push(error),
            }
        }
        Err(why.join("; "))
    })
}

/// Steam's API's answer, else the folders' (`from_folders`), with both
/// reasons when neither has a save folder.
fn choose(
    from_api: Result<PathBuf, String>,
    from_folders: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<(PathBuf, &'static str), String> {
    let api_error = match from_api {
        Ok(folder) => return Ok((folder, "from Steam's API")),
        Err(error) => error,
    };
    from_folders()
        .map(|folder| (folder, "from Steam's folder"))
        .map_err(|folders_error| {
            format!(
                "cannot find the game's save folder: Steam's API: {api_error}; Steam's folder: {folders_error}"
            )
        })
}

/// The save folder under the Steam installation `root`: of `account` when
/// it has one, else of the one account that has one.
fn save_folder_in(root: &Path, account: Option<u32>) -> Result<PathBuf, String> {
    let userdata = root.join("userdata");
    if let Some(account) = account {
        let folder = save_folder_of(&userdata.join(account.to_string()));
        if folder.is_dir() {
            return Ok(folder);
        }
    }
    let mut found: Vec<PathBuf> = fs::read_dir(&userdata)
        .map_err(|error| format!("reading {}: {error}", userdata.display()))?
        .flatten()
        .map(|account| save_folder_of(&account.path()))
        .filter(|folder| folder.is_dir())
        .collect();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(format!(
            "no Steam account under {} has a save folder for the game",
            userdata.display()
        )),
        _ => Err(format!(
            "several Steam accounts under {} have a save folder for the game, and Steam names none as playing",
            userdata.display()
        )),
    }
}

fn save_folder_of(account: &Path) -> PathBuf {
    account.join(STEAM_APP).join("local").join("save")
}

/// Wine's name for the Unix path `unix`, through the drive Z: that Wine
/// and Proton map to `/`; none for a path that is not a Unix one.
#[cfg_attr(not(windows), allow(dead_code))]
fn wine_path(unix: &str) -> Option<PathBuf> {
    unix.starts_with('/')
        .then(|| PathBuf::from(format!("Z:{}", unix.replace('/', "\\"))))
}

/// Steam's folders, the likeliest first: the registry's, Program Files',
/// and under Proton the Steam client's that Proton names
/// (`STEAM_COMPAT_CLIENT_INSTALL_PATH`), since there the registry names
/// Proton's stand-in for Steam, which has no accounts' folders.
#[cfg(windows)]
fn steam_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut add = |root: PathBuf| {
        if root.is_dir() && !roots.contains(&root) {
            roots.push(root);
        }
    };
    if let Some(path) = registry::string(r"Software\Valve\Steam", "SteamPath") {
        add(PathBuf::from(path));
    }
    if let Some(base) = std::env::var_os("ProgramFiles(x86)") {
        add(PathBuf::from(base).join("Steam"));
    }
    if let Some(path) = std::env::var("STEAM_COMPAT_CLIENT_INSTALL_PATH")
        .ok()
        .and_then(|path| wine_path(&path))
    {
        add(path);
    }
    roots
}

#[cfg(windows)]
fn active_account() -> Option<u32> {
    registry::dword(r"Software\Valve\Steam\ActiveProcess", "ActiveUser")
}

#[cfg(not(windows))]
fn steam_roots() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(not(windows))]
fn active_account() -> Option<u32> {
    None
}

/// Steam's API in the game: the account's folder for the game, as
/// `ISteamUser::GetUserDataFolder` names it
/// (`<Steam>/userdata/<account>/3493540/local`), through the game's own
/// `steam_api64.dll`, once the game started it. Under Proton, Steam's
/// bridge names a Windows path, or a Unix one, taken through drive Z:.
#[cfg(windows)]
#[allow(unsafe_code)]
mod steam_api {
    use std::{
        ffi::{CStr, c_char, c_int, c_void},
        path::PathBuf,
    };

    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

    /// `SteamAPI_SteamUser_v023`: the game's ISteamUser, or null before
    /// the game started Steam's API.
    type SteamUser = unsafe extern "C" fn() -> *mut c_void;
    /// `SteamAPI_ISteamUser_GetUserDataFolder`.
    type GetUserDataFolder = unsafe extern "C" fn(*mut c_void, *mut c_char, c_int) -> bool;

    pub fn user_data_folder() -> Result<PathBuf, String> {
        let module: Vec<u16> = "steam_api64.dll".encode_utf16().chain(Some(0)).collect();
        // SAFETY: a NUL-terminated wide string; the handle of a loaded
        // module is not ours to free.
        let handle = unsafe { GetModuleHandleW(module.as_ptr()) };
        if handle.is_null() {
            return Err("the game has no steam_api64.dll loaded".to_owned());
        }
        // SAFETY: a valid module handle and NUL-terminated names.
        let (user, folder) = unsafe {
            (
                GetProcAddress(handle, c"SteamAPI_SteamUser_v023".as_ptr().cast()),
                GetProcAddress(
                    handle,
                    c"SteamAPI_ISteamUser_GetUserDataFolder".as_ptr().cast(),
                ),
            )
        };
        let (Some(user), Some(folder)) = (user, folder) else {
            return Err("the game's steam_api64.dll has no SteamAPI_SteamUser_v023 \
                 or SteamAPI_ISteamUser_GetUserDataFolder"
                .to_owned());
        };
        // SAFETY: Steam's flat API exports these names with these
        // signatures (steam_api_flat.h of the SDK whose ISteamUser023 the
        // game asks for).
        let (user, folder) = unsafe {
            (
                std::mem::transmute::<unsafe extern "system" fn() -> isize, SteamUser>(user),
                std::mem::transmute::<unsafe extern "system" fn() -> isize, GetUserDataFolder>(
                    folder,
                ),
            )
        };
        // SAFETY: as above; null before Steam's API is up.
        let steam_user = unsafe { user() };
        if steam_user.is_null() {
            return Err("Steam's API is not started in the game yet".to_owned());
        }
        let mut buffer = vec![0 as c_char; 1024];
        let size = c_int::try_from(buffer.len()).unwrap_or(c_int::MAX);
        // SAFETY: the ISteamUser Steam gave, and a buffer of `size` bytes.
        let found = unsafe { folder(steam_user, buffer.as_mut_ptr(), size) };
        if let Some(last) = buffer.last_mut() {
            *last = 0;
        }
        // SAFETY: NUL-terminated within the buffer, as just made sure.
        let text = unsafe { CStr::from_ptr(buffer.as_ptr()) };
        let text = text
            .to_str()
            .map_err(|_| "Steam names the game's folder in no UTF-8".to_owned())?;
        if !found || text.is_empty() {
            return Err("Steam names no folder for the game".to_owned());
        }
        Ok(super::wine_path(text).unwrap_or_else(|| PathBuf::from(text)))
    }
}

#[cfg(not(windows))]
mod steam_api {
    pub fn user_data_folder() -> Result<std::path::PathBuf, String> {
        Err("Steam's API is read on Windows only".to_owned())
    }
}

/// The current user's registry values Steam writes.
#[cfg(windows)]
#[allow(unsafe_code)]
mod registry {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    pub fn string(key: &str, value: &str) -> Option<String> {
        let (key, value) = (wide(key), wide(value));
        let mut buffer = vec![0u16; 1024];
        let mut size = u32::try_from(buffer.len() * 2).ok()?;
        // SAFETY: both names end in a NUL, and the buffer holds `size`
        // bytes, which RegGetValueW writes back.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &raw mut size,
            )
        };
        if status != 0 {
            return None;
        }
        let units = (size as usize / 2).min(buffer.len());
        let text = String::from_utf16_lossy(&buffer[..units]);
        let text = text.trim_end_matches('\0');
        (!text.is_empty()).then(|| text.to_owned())
    }

    pub fn dword(key: &str, value: &str) -> Option<u32> {
        let (key, value) = (wide(key), wide(value));
        let mut data = 0u32;
        let mut size = 4u32;
        // SAFETY: as above, with room for one DWORD.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&raw mut data).cast(),
                &raw mut size,
            )
        };
        (status == 0).then_some(data)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::PoisonError;

    use super::*;
    use crate::lua::tests::{Lua, SERIAL};

    fn folder(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tpf3mp-worlds-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A process id no process has: not a multiple of 4 on Windows, above
    /// any `pid_max` on Linux.
    const GONE: u32 = u32::MAX - 1;

    #[test]
    fn only_the_hooks_own_names_are_room_copies() {
        assert_eq!(copy_of("tpf3mp_room_41856.sav"), Some(41856));
        assert_eq!(copy_of("tpf3mp_41856_21.sav"), Some(41856));
        assert_eq!(copy_of("tpf3mp_41856_21.jpg"), Some(41856));
        assert_eq!(copy_of("tpf3mp_room_7.jpg"), Some(7));
        assert_eq!(copy_of("tpf3mp_7_0.sav"), Some(7));
        assert_eq!(copy_of("tpf3mp_4294967295_1.sav"), Some(u32::MAX));
        for not_ours in [
            "two.sav",
            "MPGAME.sav",
            "autosave_two_1929-01-10.sav",
            "autosave_tpf3mp_room_7_1900-01-06.sav",
            "tpf3mp_room_7",
            "tpf3mp_room_7.sav.bak",
            "tpf3mp_room_7.lua",
            "tpf3mp_room_.sav",
            "tpf3mp_room_x.sav",
            "tpf3mp_room_07.sav",
            "tpf3mp_room_0.sav",
            "tpf3mp_room_-7.sav",
            "tpf3mp_room_+7.sav",
            "tpf3mp_room_4294967296.sav",
            "tpf3mp_room_7_1.sav",
            "TPF3MP_room_7.sav",
            "tpf3mp_room_7.SAV",
            " tpf3mp_room_7.sav",
            "tpf3mp_7.sav",
            "tpf3mp_7_.sav",
            "tpf3mp__1.sav",
            "tpf3mp_7_1_2.sav",
            "tpf3mp_7_01.sav",
            "tpf3mp_7_x.sav",
            "tpf3mp_mine_1.sav",
            "my tpf3mp_7_1.sav",
            "tpf3mp_7_1 copy.sav",
        ] {
            assert_eq!(copy_of(not_ours), None, "{not_ours}");
        }
    }

    #[test]
    fn the_sweep_removes_only_copies_of_games_no_longer_running() {
        let dir = folder("sweep");
        let own = 400;
        let live = 500;
        let files = [
            // Games gone: removed.
            "tpf3mp_room_41856.sav",
            "tpf3mp_41856_21.sav",
            "tpf3mp_41856_21.jpg",
            "tpf3mp_room_3760.sav",
            // This game's: the world it loads or plays, and its own save.
            "tpf3mp_room_400.sav",
            "tpf3mp_400_3.sav",
            // Another game's, still running.
            "tpf3mp_room_500.sav",
            "tpf3mp_500_9.sav",
            // The player's.
            "two.sav",
            "two.jpg",
            "autosave_two_1929-01-10.sav",
            "tpf3mp_room_07.sav",
            "tpf3mp_room_9.sav.bak",
            "TPF3MP_room_9.sav",
            "tpf3mp_mine_1.sav",
        ];
        for name in files {
            fs::write(dir.join(name), b"world").unwrap();
        }
        // A folder with a copy's name is not a copy.
        fs::create_dir(dir.join("tpf3mp_room_600.sav")).unwrap();
        let swept = sweep(&dir, own, |pid| pid == live);
        assert_eq!(
            swept.removed,
            [
                "tpf3mp_41856_21.jpg",
                "tpf3mp_41856_21.sav",
                "tpf3mp_room_3760.sav",
                "tpf3mp_room_41856.sav",
            ]
        );
        assert_eq!(swept.bytes, 20);
        assert!(swept.failed.is_empty());
        for kept in &files[4..] {
            assert!(dir.join(kept).is_file(), "{kept} is kept");
        }
        assert!(dir.join("tpf3mp_room_600.sav").is_dir());
        assert!(swept.lines()[0].contains("removed 4 room save copies"));
        // Nothing left to remove: nothing logged.
        let again = sweep(&dir, own, |pid| pid == live);
        assert_eq!(again, Swept::default());
        assert!(again.lines().is_empty());
        // No folder: nothing.
        assert_eq!(
            sweep(&dir.join("missing"), own, |_| false),
            Swept::default()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn this_process_runs_and_no_process_has_an_impossible_id() {
        assert!(running(std::process::id()));
        assert!(!running(GONE));
    }

    #[test]
    fn the_hook_sweeps_when_it_starts_and_when_it_copies_a_rooms_world_in() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = folder("sweep-start");
        let gone = format!("tpf3mp_room_{GONE}.sav");
        let gone_save = format!("tpf3mp_{GONE}_4.sav");
        fs::write(dir.join(&gone), b"old world").unwrap();
        fs::write(dir.join(&gone_save), b"old save").unwrap();
        fs::write(dir.join("mine.sav"), b"the player's").unwrap();
        let mut worlds = GuiWorlds::in_folder(Ok(dir.clone()));
        assert!(!dir.join(&gone).exists(), "removed at start");
        assert!(!dir.join(&gone_save).exists(), "removed at start");
        assert!(dir.join("mine.sav").is_file());
        // A copy of a game that ended while this one ran.
        fs::write(dir.join(&gone), b"old world").unwrap();
        let room = dir.join("from-the-room.sav");
        fs::write(&room, b"the room's world").unwrap();
        let lua = Lua::new();
        lua.register();
        worlds.request_load(&room, LoadFrom::Gui).unwrap();
        assert!(!dir.join(&gone).exists(), "removed once a new copy is in");
        let own = dir.join(format!("tpf3mp_room_{}.sav", std::process::id()));
        assert_eq!(fs::read(&own).unwrap(), b"the room's world");
        assert!(room.is_file(), "the room's file is the agent's");
        assert!(dir.join("mine.sav").is_file());
        let _ = lua.run("return tpf3mp_native.poll()");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn steams_api_names_the_folder_first_and_the_folders_are_the_fallback() {
        let api = PathBuf::from("api");
        let folders = PathBuf::from("folders");
        assert_eq!(
            choose(Ok(api.clone()), || unreachable!("not needed")),
            Ok((api, "from Steam's API"))
        );
        // Under Proton without Steam's API: Steam's folder.
        assert_eq!(
            choose(Err("not started".into()), || Ok(folders.clone())),
            Ok((folders, "from Steam's folder"))
        );
        // Neither: both reasons, fail closed.
        let error = choose(Err("not started".into()), || Err("no userdata".into())).unwrap_err();
        assert!(error.contains("Steam's API: not started"), "{error}");
        assert!(error.contains("Steam's folder: no userdata"), "{error}");
    }

    #[test]
    fn a_steam_folder_gives_the_accounts_save_folder() {
        let root = folder("steam-root");
        // No accounts at all: why.
        assert!(save_folder_in(&root, None).unwrap_err().contains("reading"));
        let one = root.join("userdata/111/3493540/local/save");
        fs::create_dir_all(&one).unwrap();
        fs::create_dir_all(root.join("userdata/222/other_game")).unwrap();
        assert_eq!(save_folder_in(&root, None), Ok(one.clone()));
        let two = root.join("userdata/333/3493540/local/save");
        fs::create_dir_all(&two).unwrap();
        assert!(save_folder_in(&root, None).unwrap_err().contains("several"));
        assert_eq!(save_folder_in(&root, Some(333)), Ok(two));
        // An account playing without a save folder: the one that has one
        // decides only when it is the only one.
        assert!(save_folder_in(&root, Some(222)).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unix_paths_are_taken_through_wines_drive_z() {
        assert_eq!(
            wine_path("/home/me/.local/share/Steam"),
            Some(PathBuf::from(r"Z:\home\me\.local\share\Steam"))
        );
        assert_eq!(wine_path(r"C:\Program Files (x86)\Steam"), None);
    }

    #[test]
    fn a_save_is_found_in_the_folder_once_the_gui_says_it_is_written() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = folder("save");
        let lua = Lua::new();
        lua.register();
        let _ = lua::take_save_answer();
        let mut worlds = GuiWorlds::in_folder(Ok(dir.clone()));
        worlds.request_save("tpf3mp_1_5");
        assert_eq!(worlds.save_result(), None, "not answered yet");
        fs::write(dir.join("tpf3mp_1_5.sav"), b"world").unwrap();
        fs::write(dir.join("tpf3mp_1_5.jpg"), b"picture").unwrap();
        lua.run("tpf3mp_native.poll() tpf3mp_native.saved('tpf3mp_1_5', true)")
            .unwrap();
        assert_eq!(worlds.save_result(), Some(Ok(dir.join("tpf3mp_1_5.sav"))));
        assert!(
            !dir.join("tpf3mp_1_5.jpg").exists(),
            "the picture is not kept"
        );
        // A save the GUI claims but that is not there is a failure.
        worlds.request_save("tpf3mp_1_6");
        lua.run("tpf3mp_native.saved('tpf3mp_1_6', true)").unwrap();
        assert!(
            worlds
                .save_result()
                .unwrap()
                .unwrap_err()
                .contains("is not there")
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_rooms_save_is_copied_in_and_the_gui_asked_to_load_it() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = folder("load");
        let room = dir.join("from-the-room.sav");
        fs::write(&room, b"the room's world").unwrap();
        let lua = Lua::new();
        lua.register();
        let mut worlds = GuiWorlds::in_folder(Ok(dir.clone()));
        worlds.request_load(&room, LoadFrom::Gui).unwrap();
        let name = format!("tpf3mp_room_{}", std::process::id());
        assert_eq!(
            fs::read(dir.join(format!("{name}.sav"))).unwrap(),
            b"the room's world"
        );
        assert_eq!(lua.run("return tpf3mp_native.poll().load"), Ok(name));
        // Without a save folder nothing is asked.
        let mut nowhere = GuiWorlds::in_folder(Err("no Steam".into()));
        assert_eq!(
            nowhere.request_load(&room, LoadFrom::Menu),
            Err("no Steam".into())
        );
        assert_eq!(lua.run("return tpf3mp_native.poll()"), Ok("nil".into()));
        assert_eq!(lua::take_menu_load(), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn with_no_world_up_the_main_menu_is_asked_to_load_the_rooms_save() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = folder("menu-load");
        let room = dir.join("from-the-room.sav");
        fs::write(&room, b"the room's world").unwrap();
        let lua = Lua::new();
        lua.register();
        let mut worlds = GuiWorlds::in_folder(Ok(dir.clone()));
        worlds.request_load(&room, LoadFrom::Menu).unwrap();
        let name = format!("tpf3mp_room_{}", std::process::id());
        assert!(dir.join(format!("{name}.sav")).is_file());
        assert_eq!(
            lua.run("return tpf3mp_native.poll()"),
            Ok("nil".into()),
            "not the GUI's"
        );
        assert_eq!(lua::take_menu_load(), Some(name));
        lua::menu_load_failed("no menu".into());
        assert_eq!(worlds.load_failed().as_deref(), Some("no menu"));
        assert_eq!(worlds.load_failed(), None);
        fs::remove_dir_all(&dir).unwrap();
    }
}
