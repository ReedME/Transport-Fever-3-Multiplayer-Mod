//! The in-game native hook.
//!
//! This is the library the TPF3-MP launcher loads into the game it starts,
//! and into no other (D11 in `docs/DECISIONS.md`): on Windows by
//! `LoadLibraryW` in the game while it is suspended, on Linux by
//! `LD_PRELOAD` in the game's own environment (`tpf3mp-launch`). Its job at
//! milestone M0 is only the skeleton around the real work:
//!
//! - do nothing at all unless the launcher named its link in the game's
//!   environment ([`LINK_ENV`]);
//! - run early, off the loader lock (a thread from `DllMain` on Windows, a
//!   [`ctor`](https://docs.rs/ctor) constructor on Unix);
//! - identify the running executable and find a matching build [`profile`];
//! - fail closed - install nothing - when no profile matches the build;
//! - connect to the agent's shared-memory link if it is present;
//! - log every step to a file under the per-user data directory.
//!
//! With a matched profile it installs the step gate (`install`, [`step`]):
//! `GameSim::Step` is detoured so the game runs its simulation one step for
//! each step the room releases (`docs/HOOKS.md`, "The step gate in the
//! game"), and Lua's `print`, which gives each of the game's Lua states the
//! mod's link to the hook ([`lua`]): the player's actions go to the room
//! from there, and the room's are applied by the mod's game script in the
//! update they were ordered for ("Actions in the game"). Lanes, saving and
//! loading come next.

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use tpf3mp_hookcore::profile::{BuildIdentity, Profile, ProfileError};

pub mod at_menu;
pub mod autoload;
pub mod build_data;
pub mod builds;
pub mod cadence;
pub mod clipboard;
pub mod crossings;
pub mod drawing;
pub mod edgewatch;
pub mod emission;
pub mod fastindex;
pub mod guiplayer;
pub mod image;
pub mod industries;
mod install;
pub mod interval;
pub mod junctions;
pub mod lanedump;
pub mod lanehash;
pub mod log;
pub mod lua;
pub mod menu;
pub mod modules;
pub mod native_mods;
pub mod netread;
pub mod network;
pub mod order;
/// Tests' harness: the game's own functions, relocated from the
/// executable and run in the test process.
#[cfg(all(test, windows, target_arch = "x86_64"))]
pub(crate) mod original;
pub mod perf;
pub mod persons;
mod platform;
pub mod plugins;
pub mod previewcancel;
pub mod previews;
pub mod probe;
pub mod roadtrace;
pub mod savefast;
pub mod seeds;
pub mod simperf;
pub mod step;
pub mod steptrace;
pub mod stoptool;
pub mod streettrace;
pub mod terrain;
pub mod ticks;
pub mod toolchain;
pub mod toolplayer;
pub mod townfield;
pub mod towntrace;
pub mod worlds;

/// The lobby as the main menu's Multiplayer window sees it (docs/LOBBY.md).
pub mod lobby;
/// The main-menu Multiplayer entry (docs/LOBBY.md): Windows x86-64 only.
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod menu_entry;

/// Names the link to the launcher that started this game, and that
/// launcher's process. The launcher always sets both; without them, the hook
/// does nothing (D11).
pub use tpf3mp_ipc::{LAUNCHER_PID_ENV, LINK_ENV};

/// Puts the hook's data directory (its log and profiles) here instead of
/// the per-user one, for several games on one PC.
pub const DATA_DIR_ENV: &str = tpf3mp_ipc::DATA_DIR_ENV;

/// Application name used for the per-user data directory.
const APP_DIR: &str = "TPF3-MP";

/// Runs the whole bootstrap sequence. Called once, on a thread that is not
/// holding the loader lock. Never panics across the FFI boundary: every step
/// logs its outcome and returns.
pub fn bootstrap() {
    // Only a game TPF3-MP's launcher started runs the hook: the launcher
    // names its link in the game's environment. Loaded any other way, the
    // hook writes, hashes and opens nothing (D11).
    // The launcher keeps the game suspended until the hook says it is
    // ready: on every way out of here, at the latest.
    let ready = platform::Ready::new();
    let Some(link_name) = launched_link() else {
        return;
    };
    let data_dir = data_dir();
    if let Some(dir) = &data_dir {
        let _ = fs::create_dir_all(dir);
    }
    let mut log = Logger::open(data_dir.as_deref());
    log.line("hook bootstrap starting");

    match resolve_build(&mut log, data_dir.as_deref()) {
        BuildOutcome::Matched { profile, profiles } => {
            log.line(&format!(
                "matched profile {:?} ({} targets; {} matching in all)",
                profile.name,
                profile.targets.len(),
                profiles.len()
            ));
            // Arm the menu entry and every native hook while the game's main
            // thread is suspended. RegisterAppUsertypes must not run before
            // the world-loading hook can capture the menu's Lua state.
            install_menu(&profiles, &mut log, data_dir.as_deref());
            // Native mods the launcher enabled (proposed D29): one that
            // changes the simulation and cannot run keeps the game out of
            // rooms.
            if let Some(reason) = native_mods::decide(&profiles, &profile.build.sha256, &mut log) {
                log.line(&format!("multiplayer disabled (fail-closed): {reason}"));
            } else if let Err(reason) = native_mods::load_plugins(&profiles, &mut log) {
                log.line(&format!(
                    "multiplayer disabled (fail-closed): native mods' plugins: {reason}"
                ));
            } else {
                match install::install(&profile, &link_name, Logger::open(data_dir.as_deref())) {
                    install::Installed::Yes { step_rva } => log.line(&format!(
                        "step gate installed on {} at {step_rva:#x}; the session is attached to {link_name:?}",
                        install::STEP_TARGET
                    )),
                    install::Installed::No(reason) => {
                        log.line(&format!("multiplayer disabled (fail-closed): {reason}"));
                    }
                }
            }
        }
        BuildOutcome::FailedClosed(reason) => {
            log.line(&format!("multiplayer disabled (fail-closed): {reason}"));
        }
    }

    log.line("hook bootstrap complete");
    ready.signal(&mut log);
}

/// Arms the main-menu Multiplayer entry (docs/LOBBY.md) from the matched
/// profile. When a target is missing the menu stays the game's, and the
/// reason is logged (fail-closed).
#[cfg(all(windows, target_arch = "x86_64"))]
#[allow(unsafe_code)]
fn install_menu(profiles: &[Profile], log: &mut Logger, data_dir: Option<&Path>) {
    // The menu's targets may live in any profile for this build: several can
    // be installed side by side (one per feature), so each is tried in turn.
    let mut reasons = Vec::new();
    for profile in profiles {
        match menu_entry::resolve_targets(profile) {
            Ok(targets) => {
                let log_path = data_dir.map(|dir| dir.join("hook.log"));
                // SAFETY: the launcher loaded the hook into the suspended game,
                // so no game code runs yet, and the targets are profile-verified.
                match unsafe {
                    menu_entry::install(
                        &targets,
                        log_path.as_deref(),
                        menu_entry::manager_window_supported(profile),
                    )
                } {
                    Ok(()) => log.line(&format!(
                        "main-menu Multiplayer entry armed from profile {:?} (loader at {:#x})",
                        profile.name, targets.loadfile
                    )),
                    Err(error) => log.line(&format!("main-menu entry not armed: {error}")),
                }
                return;
            }
            Err(reason) => reasons.push(format!("{:?}: {reason}", profile.name)),
        }
    }
    log.line(&format!(
        "main-menu entry not armed (fail-closed): {}",
        reasons.join("; ")
    ));
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn install_menu(_profiles: &[Profile], log: &mut Logger, _data_dir: Option<&Path>) {
    log.line("main-menu entry: Windows x86-64 only for now");
}

/// The result of trying to match the running build to a profile.
enum BuildOutcome {
    Matched {
        /// The profile the step gate is installed from.
        profile: Profile,
        /// Every profile that matches the build, `profile` first.
        profiles: Vec<Profile>,
    },
    FailedClosed(String),
}

fn resolve_build(log: &mut Logger, data_dir: Option<&Path>) -> BuildOutcome {
    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            return BuildOutcome::FailedClosed(format!("cannot find this executable: {error}"));
        }
    };
    let identity = match BuildIdentity::of_file(&exe) {
        Ok(identity) => identity,
        Err(error) => {
            return BuildOutcome::FailedClosed(format!("cannot hash {}: {error}", exe.display()));
        }
    };
    log.line(&format!(
        "executable {} sha256={} size={:?}",
        exe.display(),
        identity.sha256,
        identity.size
    ));

    let profiles_dir = data_dir.map(|dir| dir.join("profiles"));
    // The data folder's profiles first, so one placed there can stand in for
    // a built-in one without a release; then the release's own.
    let mut profiles = profiles_dir
        .as_deref()
        .map(load_profiles)
        .unwrap_or_default();
    profiles.extend(built_in_profiles());
    for loaded in &profiles {
        if let Err(error) = &loaded.profile {
            log.line(&format!(
                "ignoring unreadable profile {}: {error:?}",
                loaded.path.display()
            ));
        }
    }

    match select_native_profile(&profiles, &identity) {
        Ok(profile) => BuildOutcome::Matched {
            profile: profile.clone(),
            profiles: matching_profiles(&profiles, &identity),
        },
        Err(reason) => BuildOutcome::FailedClosed(reason),
    }
}

/// Why a profile file could not be turned into a [`Profile`].
#[derive(Debug)]
pub enum LoadError {
    /// The file could not be read.
    Read(String),
    /// The file was read but is not a valid profile.
    Parse(ProfileError),
}

/// A profile file that was found, parsed or not.
pub struct LoadedProfile {
    pub path: PathBuf,
    pub profile: Result<Profile, LoadError>,
}

/// The profiles this release was built with, from the repository's
/// `profiles/` folder: the game builds it can hook without a profile file.
/// A new game build needs a reviewed profile and native data bundle, and so
/// a new release (DAY_ONE.md, patch duty).
pub use build_data::BUILT_IN_PROFILES;

/// [`BUILT_IN_PROFILES`], parsed, each under the path `built-in/<file>`.
pub fn built_in_profiles() -> Vec<LoadedProfile> {
    BUILT_IN_PROFILES
        .iter()
        .map(|(file, text)| LoadedProfile {
            path: PathBuf::from("built-in").join(file),
            profile: Profile::from_toml(text).map_err(LoadError::Parse),
        })
        .collect()
}

/// Reads flat `*.toml` profiles and immediate build directories' `hooks.toml`.
/// A missing directory yields an empty list; unreadable or malformed profiles
/// are kept as `Err` so they can be logged.
pub fn load_profiles(dir: &Path) -> Vec<LoadedProfile> {
    let mut out = Vec::new();
    let paths = match tpf3mp_hookcore::profile::profile_files(dir) {
        Ok(paths) => paths,
        Err(_) => return out,
    };
    for path in paths {
        let profile = match fs::read_to_string(&path) {
            Ok(text) => Profile::from_toml(&text).map_err(LoadError::Parse),
            Err(error) => Err(LoadError::Read(error.to_string())),
        };
        out.push(LoadedProfile { path, profile });
    }
    out
}

/// The first profile whose declared build identity matches the running build.
/// Returns `None` when none match, which is the fail-closed signal.
pub fn select_profile<'a>(
    profiles: &'a [LoadedProfile],
    identity: &BuildIdentity,
) -> Option<&'a Profile> {
    profiles
        .iter()
        .filter_map(|loaded| loaded.profile.as_ref().ok())
        .find(|profile| profile.verify_identity(identity).is_ok())
}

/// Native installation requires the compiled layouts as well as a profile.
fn select_native_profile<'a>(
    profiles: &'a [LoadedProfile],
    identity: &BuildIdentity,
) -> Result<&'a Profile, String> {
    build_data::verify_identity(identity)?;
    select_profile(profiles, identity).ok_or_else(|| {
        format!(
            "no profile matches supported native build {}",
            identity.sha256
        )
    })
}

/// Every profile whose declared build identity matches the running build, in
/// directory order (so the one [`select_profile`] picks comes first).
pub fn matching_profiles(profiles: &[LoadedProfile], identity: &BuildIdentity) -> Vec<Profile> {
    profiles
        .iter()
        .filter_map(|loaded| loaded.profile.as_ref().ok())
        .filter(|profile| profile.verify_identity(identity).is_ok())
        .cloned()
        .collect()
}

/// The link to the launcher that started this game: [`LINK_ENV`]. `None`
/// when no launcher started it, and the hook then does nothing.
pub fn launched_link() -> Option<String> {
    launched_link_from(|key| std::env::var(key).ok(), parent_process())
}

/// On Linux and macOS, also `None` in a program the game started, which
/// inherits the variables and, through `LD_PRELOAD`, the hook: there the
/// parent is the game, not the launcher [`LAUNCHER_PID_ENV`] names. On
/// Windows, nothing the game starts loads the hook (`parent` is `None`).
fn launched_link_from(get: impl Fn(&str) -> Option<String>, parent: Option<u32>) -> Option<String> {
    let link = get(LINK_ENV).filter(|name| !name.is_empty())?;
    if let Some(parent) = parent {
        let launcher: u32 = get(LAUNCHER_PID_ENV)?.parse().ok()?;
        if launcher != parent {
            return None;
        }
    }
    Some(link)
}

#[cfg(unix)]
fn parent_process() -> Option<u32> {
    Some(std::os::unix::process::parent_id())
}

#[cfg(not(unix))]
fn parent_process() -> Option<u32> {
    None
}

/// The data directory for logs and profiles: [`DATA_DIR_ENV`] when set,
/// otherwise the per-user one.
pub fn data_dir() -> Option<PathBuf> {
    data_dir_from(|key| std::env::var(key).ok())
}

/// Resolves the data directory from an environment getter, so the mapping can
/// be tested without touching the real environment.
fn data_dir_from(get: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(dir) = get(DATA_DIR_ENV).filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    #[cfg(windows)]
    {
        get("LOCALAPPDATA").map(|base| PathBuf::from(base).join(APP_DIR))
    }
    #[cfg(target_os = "macos")]
    {
        get("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join(APP_DIR)
        })
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        get("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| get("HOME").map(|home| PathBuf::from(home).join(".local").join("share")))
            .map(|base| base.join(APP_DIR))
    }
}

/// A tiny append-only line logger. When no data directory is available it drops
/// messages rather than failing the hook.
pub(crate) struct Logger {
    file: Option<File>,
}

impl Logger {
    pub(crate) fn open(dir: Option<&Path>) -> Self {
        let file = dir.and_then(|dir| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("hook.log"))
                .ok()
        });
        Self { file }
    }

    pub(crate) fn line(&mut self, message: &str) {
        if let Some(file) = &mut self.file {
            let seconds = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let _ = writeln!(file, "[{seconds}] {message}");
        }
    }

    /// Writes `messages` as [`Logger::line`] would, in one write.
    pub(crate) fn lines(&mut self, messages: &[String]) {
        if let Some(file) = &mut self.file {
            let seconds = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let mut text = String::new();
            for message in messages {
                text.push_str(&format!("[{seconds}] {message}\n"));
            }
            let _ = file.write_all(text.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn profile_toml(name: &str, sha: &str) -> String {
        format!(
            r#"
name = "{name}"
[build]
sha256 = "{sha}"
[[target]]
name = "t"
signature = "40 53"
prologue = "40 53"
"#
        )
    }

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir()
                .join(format!("tpf3mp-hook-{tag}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn data_dir_maps_the_expected_environment_variable() {
        let mut env = HashMap::new();
        #[cfg(windows)]
        {
            env.insert("LOCALAPPDATA", r"C:\Users\x\AppData\Local".to_string());
        }
        #[cfg(target_os = "macos")]
        {
            env.insert("HOME", "/Users/x".to_string());
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            env.insert("HOME", "/home/x".to_string());
        }
        let dir = data_dir_from(|key| env.get(key).cloned()).expect("a base dir");
        assert!(dir.ends_with(APP_DIR), "{dir:?} should end with {APP_DIR}");
    }

    #[test]
    fn data_dir_is_none_without_the_environment() {
        assert!(data_dir_from(|_| None).is_none());
    }

    #[test]
    fn data_dir_and_link_can_be_set_per_game() {
        let env = HashMap::from([
            (DATA_DIR_ENV, "/rig/p2".to_string()),
            (LINK_ENV, "rig-p2".to_string()),
            ("LOCALAPPDATA", "C:/Users/x/AppData/Local".to_string()),
            ("HOME", "/home/x".to_string()),
        ]);
        let get = |key: &str| env.get(key).cloned();
        assert_eq!(data_dir_from(get), Some(PathBuf::from("/rig/p2")));
        assert_eq!(launched_link_from(get, None).as_deref(), Some("rig-p2"));

        // Unset or empty: no launcher started this game, and the hook stays
        // out of it.
        let empty = HashMap::from([(DATA_DIR_ENV, String::new()), (LINK_ENV, String::new())]);
        let get = |key: &str| empty.get(key).cloned();
        assert_eq!(launched_link_from(get, None), None);
        assert_eq!(launched_link_from(|_| None, None), None);
        assert!(data_dir_from(get).is_none());
    }

    #[test]
    fn only_the_process_the_launcher_started_runs_the_hook() {
        let env = HashMap::from([
            (LINK_ENV, "tpf3mp.default".to_string()),
            (LAUNCHER_PID_ENV, "4242".to_string()),
        ]);
        let get = |key: &str| env.get(key).cloned();
        // The game, whose parent is the launcher.
        assert_eq!(
            launched_link_from(get, Some(4242)).as_deref(),
            Some("tpf3mp.default")
        );
        // A program the game started: it inherited the variables.
        assert_eq!(launched_link_from(get, Some(5151)), None);
        // Without the launcher's process, nothing on Linux and macOS.
        let no_pid = HashMap::from([(LINK_ENV, "tpf3mp.default".to_string())]);
        assert_eq!(
            launched_link_from(|key| no_pid.get(key).cloned(), Some(4242)),
            None
        );
        let garbled = HashMap::from([
            (LINK_ENV, "tpf3mp.default".to_string()),
            (LAUNCHER_PID_ENV, "42x".to_string()),
        ]);
        assert_eq!(
            launched_link_from(|key| garbled.get(key).cloned(), Some(42)),
            None
        );
    }

    #[test]
    fn select_profile_matches_by_identity_and_fails_closed_otherwise() {
        let dir = TempDir::new("select");
        fs::write(dir.0.join("a.toml"), profile_toml("Build A", "aaaa")).unwrap();
        fs::write(dir.0.join("b.toml"), profile_toml("Build B", "bbbb")).unwrap();
        fs::write(dir.0.join("notes.txt"), "ignored").unwrap();
        let profiles = load_profiles(&dir.0);
        assert_eq!(profiles.len(), 2, "only .toml files are loaded");

        let matches_b = BuildIdentity {
            sha256: "bbbb".into(),
            size: None,
            pe_timestamp: None,
        };
        assert_eq!(
            select_profile(&profiles, &matches_b).map(|p| p.name.as_str()),
            Some("Build B")
        );

        let matches_none = BuildIdentity {
            sha256: "cccc".into(),
            size: None,
            pe_timestamp: None,
        };
        assert!(
            select_profile(&profiles, &matches_none).is_none(),
            "an unknown build matches no profile (fail-closed)"
        );
    }

    #[test]
    fn the_steam_profiles_select_by_identity_and_native_bundle_fails_closed() {
        const RELEASE_40408: &str =
            include_str!("../../../profiles/tf3_build40408_steam_windows/hooks.toml");
        const RELEASE_40420: &str =
            include_str!("../../../profiles/tf3_build40420_steam_windows/hooks.toml");
        let known = [
            (
                "Transport Fever 3 Build 40408 (Steam, Windows x64)",
                RELEASE_40408,
                BuildIdentity {
                    sha256: "de1daad3a13f3b7e9f79903361bb43769cf4f15e59271a263aefe1f075f23ef2"
                        .into(),
                    size: Some(69_711_288),
                    pe_timestamp: Some(0x6AB6_9FE5),
                },
            ),
            (
                "Transport Fever 3 Build 40420 (Steam, Windows x64)",
                RELEASE_40420,
                BuildIdentity {
                    sha256: "74861ac43b041aebc5179154345b3cf1ec83154c8e6cc58e0d9e02ff5fa602e4"
                        .into(),
                    size: Some(69_755_832),
                    pe_timestamp: Some(0x6AC5_0427),
                },
            ),
        ];
        let profiles: Vec<_> = known
            .iter()
            .map(|(name, text, _)| LoadedProfile {
                path: PathBuf::from(name),
                profile: Profile::from_toml(text).map_err(LoadError::Parse),
            })
            .collect();
        for (expected_name, _, identity) in &known {
            assert_eq!(
                select_profile(&profiles, identity).map(|profile| profile.name.as_str()),
                Some(*expected_name),
                "profile selection for {}",
                identity.sha256
            );
            assert_eq!(matching_profiles(&profiles, identity).len(), 1);
        }

        let built_in = built_in_profiles();
        assert_eq!(built_in.len(), BUILT_IN_PROFILES.len());
        assert!(built_in.iter().all(|loaded| loaded.profile.is_ok()));
        let compiled = Profile::from_toml(build_data::native::PROFILE_TOML).unwrap();
        assert_eq!(
            select_profile(&built_in, &compiled.build).map(|profile| profile.name.as_str()),
            Some(compiled.name.as_str())
        );
        assert!(select_native_profile(&built_in, &compiled.build).is_ok());
        let selected = known
            .iter()
            .find(|(_, _, identity)| identity.sha256 == compiled.build.sha256)
            .expect("the compiled native bundle names one of the verified Steam builds");
        assert_eq!(selected.0, compiled.name);
        for (_, _, identity) in known
            .iter()
            .filter(|(_, _, identity)| identity.sha256 != compiled.build.sha256)
        {
            assert!(
                select_native_profile(&built_in, identity).is_err(),
                "the compiled native bundle must refuse a different known build"
            );
        }
    }

    #[test]
    fn bootstrap_selection_refuses_external_profiles_for_another_native_build() {
        let dir = TempDir::new("unknown-native");
        let known = Profile::from_toml(build_data::native::PROFILE_TOML)
            .unwrap()
            .build;
        let mut unknown = known.clone();
        unknown.sha256 = "01".repeat(32);
        fs::write(
            dir.0.join("patched.toml"),
            profile_toml("Patched", &unknown.sha256),
        )
        .unwrap();
        let profiles = load_profiles(&dir.0);
        assert!(
            select_profile(&profiles, &unknown).is_some(),
            "generic signature matching alone would accept this"
        );
        assert!(
            select_native_profile(&profiles, &unknown).is_err(),
            "bootstrap must refuse the old compiled layouts"
        );
        assert!(
            select_native_profile(&[], &known).is_err(),
            "a native layout alone is not a profile"
        );
    }

    #[test]
    fn load_profiles_finds_bundles_and_flat_custom_profiles_in_stable_order() {
        let dir = TempDir::new("bundles");
        let bundle = dir.0.join("z-build");
        fs::create_dir(&bundle).unwrap();
        fs::write(bundle.join("hooks.toml"), profile_toml("Bundled", "bbbb")).unwrap();
        fs::write(bundle.join("metadata.toml"), "not a profile").unwrap();
        fs::write(dir.0.join("a-custom.toml"), profile_toml("Custom", "aaaa")).unwrap();
        let profiles = load_profiles(&dir.0);
        assert_eq!(
            profiles
                .iter()
                .map(|p| p.profile.as_ref().unwrap().name.as_str())
                .collect::<Vec<_>>(),
            ["Custom", "Bundled"]
        );
    }

    #[test]
    fn a_data_folder_profile_comes_before_a_built_in_one() {
        let dir = TempDir::new("override");
        let sha = "de1daad3a13f3b7e9f79903361bb43769cf4f15e59271a263aefe1f075f23ef2";
        fs::write(dir.0.join("mine.toml"), profile_toml("Mine", sha)).unwrap();
        let mut profiles = load_profiles(&dir.0);
        profiles.extend(built_in_profiles());
        let identity = BuildIdentity {
            sha256: sha.into(),
            size: None,
            pe_timestamp: None,
        };
        assert_eq!(
            select_profile(&profiles, &identity).map(|p| p.name.as_str()),
            Some("Mine")
        );
    }

    #[test]
    fn load_profiles_on_missing_directory_is_empty() {
        assert!(load_profiles(Path::new("/no/such/dir/tpf3mp")).is_empty());
    }
}
