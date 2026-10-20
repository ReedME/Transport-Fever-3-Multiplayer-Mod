//! Keeps TPF3-MP up to date from the project's GitHub releases, installing
//! only what the project signed.
//!
//! A published release carries `release.json`, its version and, for each
//! platform, the package's file name, size and SHA-256, and
//! `release.json.sig`, an Ed25519 signature of that file made with the
//! project's update key when the release is published. The launcher is
//! built with the public keys it trusts (`TPF3MP_UPDATE_PUBLIC_KEY` at build
//! time, one or more); a build without one never updates. A package is
//! installed only if:
//!
//! - the signature verifies with a trusted key, and the signed version is
//!   newer than the running one and not one this copy rolled back from;
//! - the package's size and SHA-256 match the signed manifest, checked
//!   when it is downloaded and again before it is installed;
//! - it is the archive format of this platform, and every path in it is
//!   one plain name after another, inside the package, of files and
//!   folders only.
//!
//! The files come from `releases/latest/download/` and
//! `releases/download/v<version>/`, never GitHub's rate-limited API, over
//! HTTPS only.
//!
//! Installing is journalled in `.tpf3mp-update/` in the install folder,
//! where downloads wait too (the same disk, so every step is a rename):
//! before the first file moves, the journal names each one and where its
//! old version goes (`<name>.tpf3mp-<old version>.old`). An install that
//! fails or is cut short is undone from the journal, at once or at the
//! next start. The old files stay until the new version has shown its
//! window; a new version that fails to get that far three starts running
//! is rolled back, and that version is not installed again. Only the files
//! a journal names are ever deleted. One process at a time downloads or
//! installs, holding `.tpf3mp-update/lock`.
//!
//! The player chooses when to install, since restarting ends a game; an
//! update not installed then is installed at the next start.

use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use eframe::egui;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tpf3mp_nativemods::{
    fetch::{self, FetchError},
    signed::{self, parse_keys},
};
use tracing::{info, warn};

/// This build's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The repository releases come from.
pub const REPOSITORY: &str = match option_env!("TPF3MP_REPOSITORY") {
    Some(repository) => repository,
    None => "ReedME/Transport-Fever-3-Multiplayer-Mod",
};
/// The public halves of the keys releases may be signed with, as base64 of
/// their 32 bytes, separated by commas or spaces. More than one lets the
/// project move to a new key. Without any, this build never updates.
const PUBLIC_KEYS: Option<&str> = option_env!("TPF3MP_UPDATE_PUBLIC_KEY");
/// The package this build comes in, as release file names name it.
pub const PLATFORM: Option<&str> = if cfg!(all(windows, target_arch = "x86_64")) {
    Some("windows-x64")
} else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    Some("linux-x64")
} else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    Some("macos-arm64")
} else {
    None
};

/// The file a package has at its top, naming its version and platform.
pub const PACKAGE_MARKER: &str = "tpf3mp-package.json";
/// Where downloads, the journal and the lock live in the install folder.
const STAGING: &str = ".tpf3mp-update";
const JOURNAL: &str = "journal.json";
const LOCK: &str = "lock";
/// Versions this copy rolled back from, which it does not install again.
const SKIP: &str = "skip.json";
const MANIFEST: &str = "release.json";
const SIGNATURE: &str = "release.json.sig";
/// Largest manifest read.
const MAX_MANIFEST: u64 = 64 * 1024;
/// Largest package downloaded.
const MAX_PACKAGE: u64 = 1 << 30;
/// Pause before the first check, so starting stays quick.
const FIRST_CHECK: Duration = Duration::from_secs(3);
/// How often a running launcher checks again.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// Starts a new version may take to show its window before it is rolled
/// back.
const MAX_UNCONFIRMED_STARTS: u32 = 3;
/// How long a starting launcher waits for another one's install.
const LOCK_WAIT: Duration = Duration::from_secs(60);

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("cannot reach GitHub: {0}")]
    Http(String),
    #[error("the release is not signed with the project's key")]
    BadSignature,
    #[error("the release is malformed: {0}")]
    Malformed(String),
    #[error("the release has no package for this system")]
    NoPackage,
    #[error("the downloaded package does not match the signed release")]
    Mismatch,
    #[error("the package has an unsafe entry: {0}")]
    UnsafeEntry(String),
    #[error("another TPF3-MP is installing an update")]
    Busy,
}

impl From<ureq::Error> for UpdateError {
    fn from(error: ureq::Error) -> Self {
        Self::Http(error.to_string())
    }
}

/// What the updater is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateState {
    Checking,
    UpToDate,
    /// It does not update this copy, for this reason.
    Off(String),
    Failed(String),
    Downloading {
        version: String,
        bytes: u64,
        total: u64,
    },
    /// Downloaded and checked; installs when the player chooses, or at the
    /// next start.
    Ready {
        version: String,
    },
    Installing {
        version: String,
    },
}

/// The signed description of a release.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    pub version: String,
    /// Per platform name: the package.
    pub packages: std::collections::BTreeMap<String, Package>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Package {
    pub name: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

impl Manifest {
    /// The manifest in `json`, if `signature` is one of `keys`' signature
    /// of it.
    pub fn verified(json: &[u8], signature: &[u8], keys: &[Vec<u8>]) -> Result<Self, UpdateError> {
        // The same check as the native-mods index's (D7, D29).
        if !signed::verify(json, signature, keys) {
            return Err(UpdateError::BadSignature);
        }
        let manifest: Self = serde_json::from_slice(json)
            .map_err(|error| UpdateError::Malformed(error.to_string()))?;
        semver::Version::parse(&manifest.version)
            .map_err(|error| UpdateError::Malformed(format!("version: {error}")))?;
        for package in manifest.packages.values() {
            if !safe_name(&package.name) || package.sha256.len() != 64 {
                return Err(UpdateError::Malformed(format!(
                    "package entry {}",
                    package.name
                )));
            }
        }
        Ok(manifest)
    }

    /// Whether this release is newer than `current`.
    pub fn newer_than(&self, current: &str) -> bool {
        newer(&self.version, current)
    }

    /// This platform's package, in this platform's archive format.
    fn package(&self, platform: &str) -> Result<&Package, UpdateError> {
        let package = self.packages.get(platform).ok_or(UpdateError::NoPackage)?;
        if !package.name.ends_with(archive_suffix(platform)) {
            return Err(UpdateError::Malformed(format!(
                "{} is not a {} package",
                package.name, platform
            )));
        }
        if package.size > MAX_PACKAGE {
            return Err(UpdateError::Malformed("the package is too large".into()));
        }
        Ok(package)
    }
}

fn newer(offered: &str, current: &str) -> bool {
    match (
        semver::Version::parse(offered),
        semver::Version::parse(current),
    ) {
        (Ok(offered), Ok(current)) => offered > current,
        _ => false,
    }
}

/// The archive format a platform's package comes in.
fn archive_suffix(platform: &str) -> &'static str {
    if platform.starts_with("windows") {
        ".zip"
    } else {
        ".tar.gz"
    }
}

/// A plain file name: no folders, nothing hidden or relative.
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// The keys this build trusts. An entry that is not a key is left out.
fn public_keys() -> Vec<Vec<u8>> {
    parse_keys(PUBLIC_KEYS.unwrap_or_default())
}

/// Install a signed Windows package, including the current version on a
/// first install or repair. Ordinary updates still require a newer version.
pub fn bootstrap(root: &Path, progress: impl FnMut(&str, u64, u64)) -> Result<String, UpdateError> {
    bootstrap_from(root, &public_keys(), &Source::github(), progress)
}

fn bootstrap_from(
    root: &Path,
    keys: &[Vec<u8>],
    source: &Source,
    mut progress: impl FnMut(&str, u64, u64),
) -> Result<String, UpdateError> {
    reject_linked_install(root)?;
    let agent = source.agent(Duration::from_secs(60));
    let json = get(&agent, &source.latest(MANIFEST), MAX_MANIFEST)?;
    let signature = get(&agent, &source.latest(SIGNATURE), 256)?;
    let manifest = Manifest::verified(&json, &signature, keys)?;
    if newer(VERSION, &manifest.version) {
        return Err(UpdateError::Malformed("this launcher is newer than the signed release; please try again once publication finishes".into()));
    }
    let package = manifest.package("windows-x64")?;
    let install = Install {
        root: root.to_owned(),
        exe: root.join("TPF3-MP.exe"),
        platform: "windows-x64",
    };
    if root.exists()
        && !root.join(PACKAGE_MARKER).is_file()
        && !root.join("tpf3mp-managed.json").is_file()
    {
        for entry in fs::read_dir(root)? {
            if entry?.file_name() != STAGING {
                return Err(UpdateError::Malformed(
                    "the installation folder contains unrelated files".into(),
                ));
            }
        }
    }
    let _lock = install.lock(Duration::ZERO)?;
    if Journal::path(&install).exists() && Journal::read(&install).is_none() {
        return Err(UpdateError::Malformed(
            "the interrupted installation's journal is unreadable; its files have been kept".into(),
        ));
    }
    if let Some(journal) = Journal::read(&install) {
        if !journal.roll_back(root) {
            return Err(UpdateError::Malformed(
                "the interrupted installation could not be restored".into(),
            ));
        }
        Journal::remove(&install)?;
    }
    clear_unpacked(&install);
    let archive = install.staging().join(&package.name);
    if !matches(&archive, package)? {
        download(
            &source.agent(Duration::from_secs(60 * 60)),
            &source.of(&manifest.version, &package.name),
            &archive,
            package,
            |bytes| progress(&manifest.version, bytes, package.size),
        )?;
    }
    let unpacked = install.staging().join("setup.unpacked");
    let folder = unpack(&archive, &unpacked, "windows-x64")?;
    validate_setup_package(&folder, &manifest.version)?;
    swap_in(&install, &folder, &manifest.version)?;
    let _ = fs::remove_dir_all(unpacked);
    Ok(manifest.version)
}

fn reject_linked_install(root: &Path) -> Result<(), UpdateError> {
    for path in root.ancestors() {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let linked = metadata.file_type().is_symlink();
        #[cfg(windows)]
        let linked = {
            use std::os::windows::fs::MetadataExt;
            linked || metadata.file_attributes() & 0x400 != 0
        };
        if linked {
            return Err(UpdateError::UnsafeEntry(format!(
                "{} is a filesystem link",
                path.display()
            )));
        }
    }
    Ok(())
}

fn validate_setup_package(folder: &Path, version: &str) -> Result<(), UpdateError> {
    let marker: serde_json::Value = serde_json::from_slice(&fs::read(folder.join(PACKAGE_MARKER))?)
        .map_err(|error| UpdateError::Malformed(error.to_string()))?;
    if marker["version"] != version || marker["platform"] != "windows-x64" {
        return Err(UpdateError::Malformed(
            "the package marker does not match the signed release".into(),
        ));
    }
    for file in [
        "TPF3-MP.exe",
        "tpf3mp-agent.exe",
        "tpf3mp_hook.dll",
        "tools/install.ps1",
        "tools/manage.ps1",
        "mod/tpf3mp_1/mod.json",
    ] {
        if !folder.join(file).is_file() {
            return Err(UpdateError::Malformed(format!(
                "the package is missing {file}"
            )));
        }
    }
    Ok(())
}

/// A first installation has installed its mod and is ready to open.
pub fn setup_complete(root: &Path, version: &str) {
    confirm(
        &Install {
            root: root.to_owned(),
            exe: root.join("TPF3-MP.exe"),
            platform: "windows-x64",
        },
        version,
    );
}

/// Where releases come from.
#[derive(Debug, Clone)]
pub struct Source {
    base: String,
    https_only: bool,
}

impl Source {
    /// The project's releases on GitHub.
    pub fn github() -> Self {
        Self {
            base: format!("https://github.com/{REPOSITORY}"),
            https_only: true,
        }
    }

    fn latest(&self, file: &str) -> String {
        format!("{}/releases/latest/download/{file}", self.base)
    }

    fn of(&self, version: &str, file: &str) -> String {
        format!("{}/releases/download/v{version}/{file}", self.base)
    }

    fn agent(&self, timeout: Duration) -> ureq::Agent {
        ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .https_only(self.https_only)
                .timeout_connect(Some(Duration::from_secs(30)))
                .timeout_global(Some(timeout))
                .build(),
        )
    }
}

/// Where this copy is installed, and whether the updater may change it.
#[derive(Debug, Clone)]
pub struct Install {
    /// The package's folder.
    pub root: PathBuf,
    /// The launcher's own file, as started: what restarting runs.
    pub exe: PathBuf,
    pub platform: &'static str,
}

impl Install {
    /// This copy, if it is an installed package of this platform: its
    /// folder has the package marker naming the platform.
    pub fn of_running() -> Result<Self, String> {
        let platform = PLATFORM.ok_or("this system has no packages")?;
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        let root = package_root(&exe).ok_or("cannot tell where TPF3-MP is installed")?;
        Self::at(root, exe, platform)
    }

    fn at(root: PathBuf, exe: PathBuf, platform: &'static str) -> Result<Self, String> {
        let marker = fs::read(root.join(PACKAGE_MARKER))
            .map_err(|_| "this copy is not an installed package".to_owned())?;
        #[derive(Deserialize)]
        struct Marker {
            platform: String,
        }
        let marker: Marker = serde_json::from_slice(&marker)
            .map_err(|_| "this copy's package marker is unreadable".to_owned())?;
        if marker.platform != platform {
            return Err(format!("this copy is the {} package", marker.platform));
        }
        Ok(Self {
            root,
            exe,
            platform,
        })
    }

    fn staging(&self) -> PathBuf {
        self.root.join(STAGING)
    }

    /// Holds the updater's lock, waiting at most `wait` for another
    /// process to let go of it.
    fn lock(&self, wait: Duration) -> Result<Lock, UpdateError> {
        fs::create_dir_all(self.staging())?;
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.staging().join(LOCK))?;
        let until = Instant::now() + wait;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Lock(file)),
                Err(fs::TryLockError::WouldBlock) if Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(250));
                }
                Err(fs::TryLockError::WouldBlock) => return Err(UpdateError::Busy),
                Err(fs::TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    }

    fn skipped(&self) -> Vec<String> {
        fs::read(self.staging().join(SKIP))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn skip(&self, version: &str) -> io::Result<()> {
        let mut skipped = self.skipped();
        if !skipped.iter().any(|skipped| skipped == version) {
            skipped.push(version.to_owned());
        }
        write_synced(&self.staging().join(SKIP), &serde_json::to_vec(&skipped)?)
    }
}

/// The updater's lock, held until dropped.
struct Lock(#[allow(dead_code)] File);

/// The folder a package was unpacked into, from the launcher's own path:
/// the executable's folder, or on macOS the folder holding the app bundle.
fn package_root(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    if cfg!(target_os = "macos")
        && let Some(bundle) = dir
            .ancestors()
            .find(|path| path.extension().is_some_and(|ext| ext == "app"))
    {
        return bundle.parent().map(Path::to_owned);
    }
    Some(dir.to_owned())
}

/// Checks, downloads and installs updates in the background.
#[derive(Clone)]
pub struct Updater {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<UpdateState>,
    install: Option<Install>,
    keys: Vec<Vec<u8>>,
    source: Source,
    runtime: tokio::runtime::Handle,
    repaint: Mutex<Option<egui::Context>>,
    /// One check or download at a time in this process.
    working: Mutex<()>,
}

impl Updater {
    /// An updater for this copy. It checks soon after starting and every
    /// few hours, and downloads what it finds; installing waits for the
    /// player.
    pub fn start(runtime: tokio::runtime::Handle) -> Self {
        let keys = public_keys();
        let install = Install::of_running();
        let state = match (keys.is_empty(), &install) {
            (true, _) => UpdateState::Off("this build has no update key".into()),
            (false, Err(reason)) => UpdateState::Off(reason.clone()),
            (false, Ok(_)) => UpdateState::Checking,
        };
        let updater = Self {
            inner: Arc::new(Inner {
                state: Mutex::new(state.clone()),
                install: install.ok(),
                keys,
                source: Source::github(),
                runtime,
                repaint: Mutex::default(),
                working: Mutex::default(),
            }),
        };
        if state == UpdateState::Checking {
            let every = updater.clone();
            updater.inner.runtime.spawn(async move {
                tokio::time::sleep(FIRST_CHECK).await;
                loop {
                    every.check();
                    tokio::time::sleep(CHECK_EVERY).await;
                }
            });
        } else if let UpdateState::Off(reason) = &state {
            info!(%reason, "updates are off");
        }
        updater
    }

    /// Repaints the window when the state changes.
    pub fn repaint_with(&self, ctx: egui::Context) {
        *self
            .inner
            .repaint
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(ctx);
    }

    pub fn state(&self) -> UpdateState {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set(&self, state: UpdateState) {
        *self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = state;
        if let Some(ctx) = &*self
            .inner
            .repaint
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            ctx.request_repaint();
        }
    }

    /// Checks for a newer release now, and downloads it.
    pub fn check(&self) {
        let Some(install) = self.inner.install.clone() else {
            return;
        };
        if self.inner.keys.is_empty() {
            return;
        }
        let updater = self.clone();
        self.inner.runtime.spawn_blocking(move || {
            let Ok(_working) = updater.inner.working.try_lock() else {
                return;
            };
            // A downloaded update waits for the player; nothing to check.
            if matches!(updater.state(), UpdateState::Ready { .. }) {
                return;
            }
            updater.set(UpdateState::Checking);
            let result = check_and_download(
                &install,
                &updater.inner.keys,
                &updater.inner.source,
                |version, bytes, total| {
                    updater.set(UpdateState::Downloading {
                        version: version.to_owned(),
                        bytes,
                        total,
                    });
                },
            );
            match result {
                Ok(Checked::Downloaded(version)) => {
                    info!(%version, "an update is ready to install");
                    updater.set(UpdateState::Ready { version });
                }
                Ok(Checked::UpToDate) => updater.set(UpdateState::UpToDate),
                Ok(Checked::Unsigned) => updater.set(UpdateState::Failed(
                    "the latest release is not signed for updates yet".into(),
                )),
                Err(error) => {
                    warn!(%error, "the update check failed");
                    updater.set(UpdateState::Failed(error.to_string()));
                }
            }
        });
    }

    /// Installs the downloaded update and restarts into it, closing this
    /// window. On failure everything stays as it was.
    pub fn install_and_restart(&self, ctx: &egui::Context) {
        let Some(install) = &self.inner.install else {
            return;
        };
        let UpdateState::Ready { version } = self.state() else {
            return;
        };
        self.set(UpdateState::Installing {
            version: version.clone(),
        });
        let installed = install
            .lock(Duration::ZERO)
            .and_then(|_lock| install_staged(install, &self.inner.keys));
        match installed.and_then(|installed| {
            if installed.is_some() {
                restart(install)?;
            }
            Ok(installed)
        }) {
            Ok(Some(installed)) => {
                info!(version = %installed, "installed the update; restarting");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Ok(None) => self.set(UpdateState::UpToDate),
            Err(error) => {
                warn!(%error, "cannot install the update");
                self.set(UpdateState::Failed(format!("cannot install: {error}")));
            }
        }
    }
}

/// What a check found.
#[derive(Debug, PartialEq, Eq)]
enum Checked {
    UpToDate,
    /// The latest release has no signed manifest (yet).
    Unsigned,
    /// A newer version, downloaded and checked.
    Downloaded(String),
}

/// What the journal records about an install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Journal {
    from: String,
    to: String,
    /// Each name the install puts in place, and where the file or folder
    /// it replaces was moved, if there was one.
    entries: Vec<Entry>,
    state: Stage,
    /// Starts of the new version that did not reach its window.
    starts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    name: String,
    old: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Stage {
    /// Files are being moved: undo it if found at a start.
    Swapping,
    /// Every file is in place; the old ones wait for the new version to
    /// show its window.
    Swapped,
}

impl Journal {
    fn path(install: &Install) -> PathBuf {
        install.staging().join(JOURNAL)
    }

    fn read(install: &Install) -> Option<Self> {
        let bytes = fs::read(Self::path(install)).ok()?;
        match serde_json::from_slice(&bytes) {
            Ok(journal) => Some(journal),
            Err(error) => {
                warn!(%error, "the update journal is unreadable; leaving it for a person to look at");
                None
            }
        }
    }

    fn write(&self, install: &Install) -> io::Result<()> {
        fs::create_dir_all(install.staging())?;
        write_synced(&Self::path(install), &serde_json::to_vec_pretty(self)?)
    }

    fn remove(install: &Install) -> io::Result<()> {
        match fs::remove_file(Self::path(install)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Puts back what the install replaced, and takes away what it added.
    /// Returns whether everything could be put back.
    fn roll_back(&self, root: &Path) -> bool {
        let mut complete = true;
        for entry in self.entries.iter().rev() {
            let target = root.join(&entry.name);
            match &entry.old {
                Some(old) => {
                    let old = root.join(old);
                    if !exists(&old) {
                        // Never moved aside: the original is still in place.
                        continue;
                    }
                    if exists(&target)
                        && let Err(error) = remove(&target)
                    {
                        warn!(path = %target.display(), %error, "cannot remove a new file while rolling back");
                        complete = false;
                        continue;
                    }
                    if let Err(error) = fs::rename(&old, &target) {
                        warn!(path = %target.display(), %error, "cannot put an old file back");
                        complete = false;
                    }
                }
                None => {
                    if exists(&target)
                        && let Err(error) = remove(&target)
                    {
                        warn!(path = %target.display(), %error, "cannot remove a new file while rolling back");
                        complete = false;
                    }
                }
            }
        }
        complete
    }

    /// Deletes the old files the install moved aside.
    fn remove_old(&self, root: &Path) {
        for old in self.entries.iter().filter_map(|entry| entry.old.as_ref()) {
            let path = root.join(old);
            if exists(&path)
                && let Err(error) = remove(&path)
            {
                warn!(path = %path.display(), %error, "cannot delete a file an update replaced");
            }
        }
    }
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn remove(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let partial = path.with_extension("partial");
    let mut file = File::create(&partial)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&partial, path)
}

/// At start, before anything else: finishes or undoes an install the last
/// run left, and installs an update downloaded earlier. Returns whether
/// the launcher restarted into another version and should exit now.
pub fn at_start() -> bool {
    let Ok(install) = Install::of_running() else {
        return false;
    };
    let keys = public_keys();
    match start(&install, &keys, VERSION) {
        Ok(Started::Restart) => match restart(&install) {
            Ok(()) => true,
            Err(error) => {
                warn!(%error, "cannot restart after the update");
                false
            }
        },
        Ok(Started::Continue) => false,
        Err(error) => {
            warn!(%error, "cannot finish the update");
            false
        }
    }
}

/// What a starting launcher does next.
#[derive(Debug, PartialEq, Eq)]
enum Started {
    Continue,
    /// Another version is now in place: run it.
    Restart,
}

fn start(install: &Install, keys: &[Vec<u8>], running: &str) -> Result<Started, UpdateError> {
    let _lock = install.lock(LOCK_WAIT)?;
    clear_unpacked(install);
    if let Some(mut journal) = Journal::read(install) {
        match journal.state {
            // Cut short while moving files: put everything back.
            Stage::Swapping => {
                #[cfg(windows)]
                if install.root.join("tools/install.ps1").is_file() {
                    crate::installation::check_game_closed(&install.root)
                        .map_err(|error| UpdateError::Malformed(error.to_string()))?;
                }
                info!(to = %journal.to, "undoing an update that did not finish");
                if journal.roll_back(&install.root) {
                    #[cfg(windows)]
                    crate::installation::restore_mod(&install.root)
                        .map_err(|error| UpdateError::Malformed(error.to_string()))?;
                    Journal::remove(install)?;
                }
                return Ok(if running == journal.from {
                    Started::Continue
                } else {
                    Started::Restart
                });
            }
            // The new version is starting: it has a few tries to show its
            // window before the old one comes back.
            Stage::Swapped if running == journal.to => {
                journal.starts += 1;
                if journal.starts > MAX_UNCONFIRMED_STARTS {
                    #[cfg(windows)]
                    if install.root.join("tools/install.ps1").is_file() {
                        crate::installation::check_game_closed(&install.root)
                            .map_err(|error| UpdateError::Malformed(error.to_string()))?;
                    }
                    warn!(version = %journal.to, "the new version never started; going back");
                    if journal.roll_back(&install.root) {
                        #[cfg(windows)]
                        crate::installation::restore_mod(&install.root)
                            .map_err(|error| UpdateError::Malformed(error.to_string()))?;
                        install.skip(&journal.to)?;
                        Journal::remove(install)?;
                        return Ok(Started::Restart);
                    }
                } else {
                    journal.write(install)?;
                }
                return Ok(Started::Continue);
            }
            Stage::Swapped => return Ok(Started::Continue),
        }
    }
    if keys.is_empty() {
        return Ok(Started::Continue);
    }
    match install_staged(install, keys)? {
        Some(version) => {
            info!(%version, "installed the update downloaded earlier; restarting");
            Ok(Started::Restart)
        }
        None => Ok(Started::Continue),
    }
}

/// Tells the updater this version reached its window: the files the last
/// install moved aside can go.
pub fn started() {
    let Ok(install) = Install::of_running() else {
        return;
    };
    confirm(&install, VERSION);
}

fn confirm(install: &Install, running: &str) {
    let Ok(_lock) = install.lock(Duration::from_secs(5)) else {
        return;
    };
    if let Some(journal) = Journal::read(install)
        && journal.state == Stage::Swapped
        && journal.to == running
    {
        journal.remove_old(&install.root);
        if let Err(error) = Journal::remove(install) {
            warn!(%error, "cannot remove the update journal");
        }
        info!(version = %running, "the update is complete");
    }
}

/// Deletes what earlier installs unpacked. Called holding the lock, so no
/// other process is unpacking.
fn clear_unpacked(install: &Install) {
    let Ok(entries) = fs::read_dir(install.staging()) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().contains(".unpacked") {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Runs the launcher again with the same arguments.
fn restart(install: &Install) -> Result<(), UpdateError> {
    std::process::Command::new(&install.exe)
        .args(std::env::args_os().skip(1).collect::<Vec<OsString>>())
        .spawn()?;
    Ok(())
}

fn get(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, UpdateError> {
    let mut response = agent
        .get(url)
        .header("User-Agent", format!("tpf3mp-launcher/{VERSION}"))
        .call()?;
    Ok(response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()?)
}

/// Checks the latest release and downloads its package if it is newer.
fn check_and_download(
    install: &Install,
    keys: &[Vec<u8>],
    source: &Source,
    mut progress: impl FnMut(&str, u64, u64),
) -> Result<Checked, UpdateError> {
    let agent = source.agent(Duration::from_secs(60));
    let json = match get(&agent, &source.latest(MANIFEST), MAX_MANIFEST) {
        Ok(json) => json,
        // Published but not signed yet, or no release at all.
        Err(UpdateError::Http(error)) if error.contains("404") => return Ok(Checked::Unsigned),
        Err(error) => return Err(error),
    };
    let signature = get(&agent, &source.latest(SIGNATURE), 256)?;
    let manifest = Manifest::verified(&json, &signature, keys)?;
    if !manifest.newer_than(VERSION) || install.skipped().contains(&manifest.version) {
        return Ok(Checked::UpToDate);
    }
    let package = manifest.package(install.platform)?;
    let _lock = install.lock(Duration::ZERO)?;
    let dir = install.staging().join(&manifest.version);
    fs::create_dir_all(&dir)?;
    let archive = dir.join(&package.name);
    if !matches(&archive, package)? {
        let partial = dir.join(format!("{}.part", package.name));
        let big = source.agent(Duration::from_secs(3 * 60 * 60));
        let url = source.of(&manifest.version, &package.name);
        let downloaded = download(&big, &url, &partial, package, |bytes| {
            progress(&manifest.version, bytes, package.size);
        });
        if let Err(error) = downloaded {
            let _ = fs::remove_file(&partial);
            return Err(error);
        }
        fs::rename(&partial, &archive)?;
    }
    // The manifest and signature go with the package, to check it again
    // when it is installed.
    write_synced(&dir.join(MANIFEST), &json)?;
    write_synced(&dir.join(SIGNATURE), &signature)?;
    Ok(Checked::Downloaded(manifest.version))
}

/// Downloads `package` into `path`, checking its size and hash on the way
/// (`tpf3mp_nativemods::fetch`, which native mods download with too).
fn download(
    agent: &ureq::Agent,
    url: &str,
    path: &Path,
    package: &Package,
    progress: impl FnMut(u64),
) -> Result<(), UpdateError> {
    fetch::download(
        agent,
        url,
        path,
        package.size,
        &package.sha256,
        &format!("tpf3mp-launcher/{VERSION}"),
        progress,
    )
    .map_err(|error| match error {
        FetchError::Io(error) => UpdateError::Io(error),
        FetchError::Http(error) => UpdateError::Http(error),
        FetchError::Mismatch => UpdateError::Mismatch,
    })
}

/// Whether the file at `path` is `package`.
fn matches(path: &Path, package: &Package) -> Result<bool, UpdateError> {
    Ok(fetch::file_matches(path, package.size, &package.sha256)?)
}

/// Installs the newest downloaded update that is newer than this version,
/// not skipped, and still checks out. Returns its version, or `None` if
/// there is none. Older downloads are deleted. Call it holding the lock.
fn install_staged(install: &Install, keys: &[Vec<u8>]) -> Result<Option<String>, UpdateError> {
    let Some((manifest, archive)) = newest_staged(install, keys)? else {
        return Ok(None);
    };
    // A game's loaded hook and Lua mod must stay on one version. The new
    // launcher's setup gate synchronizes the installed mod before playing.
    #[cfg(windows)]
    if install.root.join("tools/install.ps1").is_file() {
        crate::installation::check_game_closed(&install.root)
            .map_err(|error| UpdateError::Malformed(error.to_string()))?;
    }
    let unpacked = install.staging().join(format!(
        "{}.unpacked.{}",
        manifest.version,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&unpacked);
    let package = unpack(&archive, &unpacked, install.platform)?;
    swap_in(install, &package, &manifest.version)?;
    let _ = fs::remove_dir_all(&unpacked);
    let _ = fs::remove_dir_all(install.staging().join(&manifest.version));
    Ok(Some(manifest.version))
}

/// The newest staged release newer than this version whose signature and
/// package check out.
fn newest_staged(
    install: &Install,
    keys: &[Vec<u8>],
) -> Result<Option<(Manifest, PathBuf)>, UpdateError> {
    let Ok(entries) = fs::read_dir(install.staging()) else {
        return Ok(None);
    };
    let skipped = install.skipped();
    let mut best: Option<(Manifest, PathBuf)> = None;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() || entry.file_name().to_string_lossy().contains(".unpacked") {
            continue;
        }
        let (Ok(json), Ok(signature)) =
            (fs::read(dir.join(MANIFEST)), fs::read(dir.join(SIGNATURE)))
        else {
            continue;
        };
        let Ok(manifest) = Manifest::verified(&json, &signature, keys) else {
            warn!(dir = %dir.display(), "a downloaded update does not verify; deleting it");
            let _ = fs::remove_dir_all(&dir);
            continue;
        };
        if !manifest.newer_than(VERSION) || skipped.contains(&manifest.version) {
            let _ = fs::remove_dir_all(&dir);
            continue;
        }
        let Ok(package) = manifest.package(install.platform) else {
            continue;
        };
        let archive = dir.join(&package.name);
        if !matches(&archive, package)? {
            continue;
        }
        let newer = best
            .as_ref()
            .is_none_or(|(best, _)| manifest.newer_than(&best.version));
        if newer {
            best = Some((manifest, archive));
        }
    }
    Ok(best)
}

/// Unpacks `archive`, a package of `platform`, into `into` and returns the
/// package folder in it: the single folder at the archive's top. Refuses
/// another platform's archive format, any entry that would land outside
/// `into`, and anything but files and folders.
pub fn unpack(archive: &Path, into: &Path, platform: &str) -> Result<PathBuf, UpdateError> {
    fs::create_dir_all(into)?;
    let name = archive.to_string_lossy();
    if !name.ends_with(archive_suffix(platform)) {
        return Err(UpdateError::Malformed(format!(
            "{name} is not a {platform} package"
        )));
    }
    if name.ends_with(".zip") {
        unpack_zip(archive, into)?;
    } else {
        unpack_tar_gz(archive, into)?;
    }
    let tops: Vec<PathBuf> = fs::read_dir(into)?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    match tops.as_slice() {
        [top] if top.is_dir() => Ok(top.clone()),
        _ => Err(UpdateError::Malformed(
            "the package is not one folder".into(),
        )),
    }
}

/// `path` inside `into`, if every part of it is one plain name.
fn contained(into: &Path, path: &Path) -> Result<PathBuf, UpdateError> {
    let unsafe_entry = || UpdateError::UnsafeEntry(path.display().to_string());
    let mut out = into.to_owned();
    let mut any = false;
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str().ok_or_else(unsafe_entry)?;
                // A drive (C:) or another separator would change where the
                // rest lands on Windows.
                if part.contains([':', '\\', '/']) {
                    return Err(unsafe_entry());
                }
                out.push(part);
                any = true;
            }
            Component::CurDir => {}
            _ => return Err(unsafe_entry()),
        }
    }
    if any && out.starts_with(into) && out != into {
        Ok(out)
    } else {
        Err(unsafe_entry())
    }
}

fn unpack_zip(archive: &Path, into: &Path) -> Result<(), UpdateError> {
    let mut zip = zip::ZipArchive::new(File::open(archive)?)
        .map_err(|error| UpdateError::Malformed(error.to_string()))?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| UpdateError::Malformed(error.to_string()))?;
        if entry.is_symlink() {
            return Err(UpdateError::UnsafeEntry(entry.name().to_owned()));
        }
        let name = entry
            .enclosed_name()
            .ok_or_else(|| UpdateError::UnsafeEntry(entry.name().to_owned()))?;
        let path = contained(into, &name)?;
        if entry.is_dir() {
            fs::create_dir_all(&path)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = File::create(&path)?;
        io::copy(&mut entry, &mut file)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(mode & 0o755))?;
        }
    }
    Ok(())
}

fn unpack_tar_gz(archive: &Path, into: &Path) -> Result<(), UpdateError> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(File::open(archive)?));
    for entry in tar.entries()? {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        let name = entry.path()?.into_owned();
        if matches!(
            kind,
            tar::EntryType::XGlobalHeader | tar::EntryType::XHeader
        ) {
            // Metadata for the entries after it.
            continue;
        }
        let path = contained(into, &name)?;
        if kind.is_dir() {
            fs::create_dir_all(&path)?;
        } else if kind.is_file() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut file = File::create(&path)?;
            io::copy(&mut entry, &mut file)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = entry.header().mode()?;
                fs::set_permissions(&path, fs::Permissions::from_mode(mode & 0o755))?;
            }
        } else {
            return Err(UpdateError::UnsafeEntry(name.display().to_string()));
        }
    }
    Ok(())
}

/// Puts everything at the top of `package` in place of the same names in
/// the install folder, recording each step in the journal first, and
/// moving what was there aside for the new version to confirm. If a step
/// fails, puts everything back.
fn swap_in(install: &Install, package: &Path, to: &str) -> Result<(), UpdateError> {
    let root = &install.root;
    let mut names: Vec<String> = Vec::new();
    for entry in fs::read_dir(package)? {
        let name = entry?.file_name();
        let name = name
            .to_str()
            .filter(|name| !name.is_empty() && !name.contains([':', '\\', '/']))
            .ok_or_else(|| UpdateError::UnsafeEntry(name.to_string_lossy().into_owned()))?;
        names.push(name.to_owned());
    }
    names.sort();
    let suffix = format!(".tpf3mp-{VERSION}.old");
    let mut journal = Journal {
        from: VERSION.to_owned(),
        to: to.to_owned(),
        entries: names
            .iter()
            .map(|name| Entry {
                name: name.clone(),
                old: exists(&root.join(name)).then(|| format!("{name}{suffix}")),
            })
            .collect(),
        state: Stage::Swapping,
        starts: 0,
    };
    journal.write(install)?;
    let moved = (|| -> Result<(), UpdateError> {
        for entry in &journal.entries {
            let target = root.join(&entry.name);
            if let Some(old) = &entry.old {
                let old = root.join(old);
                if exists(&old) {
                    remove(&old)?;
                }
                fs::rename(&target, &old)?;
            }
            fs::rename(package.join(&entry.name), &target)?;
        }
        Ok(())
    })();
    match moved {
        Ok(()) => {
            journal.state = Stage::Swapped;
            journal.write(install)?;
            Ok(())
        }
        Err(error) => {
            if journal.roll_back(root) {
                let _ = Journal::remove(install);
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::BufRead,
        net::TcpListener,
        sync::atomic::{AtomicBool, Ordering},
    };

    use base64::Engine;
    use ring::{rand::SystemRandom, signature::Ed25519KeyPair, signature::KeyPair};

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tpf3mp-update-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn key_pair() -> Ed25519KeyPair {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    }

    fn keys(pair: &Ed25519KeyPair) -> Vec<Vec<u8>> {
        vec![pair.public_key().as_ref().to_vec()]
    }

    fn manifest_json(version: &str) -> Vec<u8> {
        format!(
            r#"{{"version":"{version}","packages":{{"windows-x64":{{"name":"tpf3mp-{version}-windows-x64.zip","size":3,"sha256":"{}"}}}}}}"#,
            "ab".repeat(32)
        )
        .into_bytes()
    }

    #[test]
    fn only_a_release_signed_with_a_trusted_key_verifies() {
        let pair = key_pair();
        let json = manifest_json("9.1.0");
        let signature = pair.sign(&json);
        let manifest = Manifest::verified(&json, signature.as_ref(), &keys(&pair)).unwrap();
        assert_eq!(manifest.version, "9.1.0");
        assert!(manifest.newer_than("0.1.0"));
        assert!(!manifest.newer_than("9.1.0"));
        assert!(!manifest.newer_than("10.0.0"));

        let mut tampered = json.clone();
        let at = tampered.len() - 10;
        tampered[at] ^= 1;
        assert!(matches!(
            Manifest::verified(&tampered, signature.as_ref(), &keys(&pair)),
            Err(UpdateError::BadSignature)
        ));
        let other = key_pair();
        assert!(matches!(
            Manifest::verified(&json, signature.as_ref(), &keys(&other)),
            Err(UpdateError::BadSignature)
        ));
        // A new key alongside the old: either signs.
        let both = [keys(&other), keys(&pair)].concat();
        assert!(Manifest::verified(&json, signature.as_ref(), &both).is_ok());
        assert!(matches!(
            Manifest::verified(&json, signature.as_ref(), &[]),
            Err(UpdateError::BadSignature)
        ));
    }

    #[test]
    fn keys_are_read_from_a_list() {
        let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        let other = base64::engine::general_purpose::STANDARD.encode([8u8; 32]);
        assert_eq!(parse_keys(&key).len(), 1);
        assert_eq!(parse_keys(&format!("{key}, {other}")).len(), 2);
        assert_eq!(parse_keys(&format!("{key} not-a-key")).len(), 1);
        assert!(parse_keys("").is_empty());
    }

    /// A manifest signed as the release workflow signs one, with OpenSSL
    /// (`openssl pkeyutl -sign -rawin`), under a key made for this test
    /// only.
    #[test]
    fn a_manifest_signed_by_the_release_workflow_verifies() {
        let key = base64::engine::general_purpose::STANDARD
            .decode("Ttl69db6GuFDYgQdtq5WrWEnEg4M7IvIJhlUt/SHdWQ=")
            .unwrap();
        let json = concat!(
            "{\n",
            "  \"version\": \"9.1.0\",\n",
            "  \"packages\": {\n",
            "    \"windows-x64\": {\n",
            "      \"name\": \"tpf3mp-9.1.0-windows-x64.zip\",\n",
            "      \"size\": 3,\n",
            "      \"sha256\": \"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\"\n",
            "    }\n",
            "  }\n",
            "}\n",
        );
        let signature: Vec<u8> = (0..64)
            .map(|at| {
                u8::from_str_radix(
                    &"a0eba348ce71fe42a0acebad26d8c61bbc95a630dfea7556a097c84f931a35a5\
                      e0c74503a3f05ea6406ef9fc4ba1157c3a12214c504b70f57842aa2a7c35070b"
                        [at * 2..at * 2 + 2],
                    16,
                )
                .unwrap()
            })
            .collect();
        let manifest = Manifest::verified(json.as_bytes(), &signature, &[key]).unwrap();
        assert_eq!(manifest.version, "9.1.0");
        assert_eq!(manifest.packages["windows-x64"].size, 3);
    }

    #[test]
    fn a_signed_manifest_still_names_only_plain_files() {
        let pair = key_pair();
        let json = br#"{"version":"9.1.0","packages":{"windows-x64":{"name":"../evil.zip","size":3,"sha256":"00"}}}"#;
        let signature = pair.sign(json);
        assert!(matches!(
            Manifest::verified(json, signature.as_ref(), &keys(&pair)),
            Err(UpdateError::Malformed(_))
        ));
    }

    #[test]
    fn a_package_must_be_its_platforms_archive() {
        let pair = key_pair();
        let json = format!(
            r#"{{"version":"9.1.0","packages":{{"windows-x64":{{"name":"tpf3mp-9.1.0-windows-x64.tar.gz","size":3,"sha256":"{}"}}}}}}"#,
            "ab".repeat(32)
        );
        let manifest = Manifest::verified(
            json.as_bytes(),
            pair.sign(json.as_bytes()).as_ref(),
            &keys(&pair),
        )
        .unwrap();
        assert!(matches!(
            manifest.package("windows-x64"),
            Err(UpdateError::Malformed(_))
        ));
        assert!(matches!(
            manifest.package("linux-x64"),
            Err(UpdateError::NoPackage)
        ));
    }

    fn zip_of(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, data) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn a_package_unpacks_into_its_folder() {
        let dir = temp("unpack");
        let archive = dir.join("tpf3mp-9.1.0-windows-x64.zip");
        zip_of(
            &archive,
            &[
                ("tpf3mp-9.1.0-windows-x64/TPF3-MP.exe", b"new launcher"),
                ("tpf3mp-9.1.0-windows-x64/docs/PLAYING.md", b"how to play"),
            ],
        );
        let package = unpack(&archive, &dir.join("out"), "windows-x64").unwrap();
        assert_eq!(
            fs::read(package.join("TPF3-MP.exe")).unwrap(),
            b"new launcher"
        );
        assert_eq!(
            fs::read(package.join("docs").join("PLAYING.md")).unwrap(),
            b"how to play"
        );
        // Another platform's archive format is refused.
        assert!(matches!(
            unpack(&archive, &dir.join("out-linux"), "linux-x64"),
            Err(UpdateError::Malformed(_))
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_package_cannot_write_outside_its_folder() {
        let dir = temp("traversal");
        let archive = dir.join("evil.zip");
        zip_of(&archive, &[("tpf3mp/../../escaped.txt", b"x")]);
        assert!(matches!(
            unpack(&archive, &dir.join("out"), "windows-x64"),
            Err(UpdateError::UnsafeEntry(_))
        ));
        assert!(!dir.join("escaped.txt").exists());

        let tarball = dir.join("evil.tar.gz");
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            File::create(&tarball).unwrap(),
            flate2::Compression::fast(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        builder
            .append_link(&mut header, "tpf3mp/link", "/etc/passwd")
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        assert!(matches!(
            unpack(&tarball, &dir.join("out-tar"), "linux-x64"),
            Err(UpdateError::UnsafeEntry(_))
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_path_with_a_drive_or_backslash_is_refused() {
        let into = Path::new("install");
        assert!(contained(into, Path::new("top/C:evil.exe")).is_err());
        assert!(contained(into, Path::new("top/D:/x")).is_err());
        // A backslash separates names on Windows, and is refused inside a
        // name elsewhere, where Windows would read it as a separator.
        let backslash = contained(into, Path::new("top/a\\b"));
        if cfg!(windows) {
            assert_eq!(backslash.unwrap(), into.join("top").join("a").join("b"));
        } else {
            assert!(backslash.is_err());
        }
        assert!(contained(into, Path::new("..")).is_err());
        assert!(contained(into, Path::new(".")).is_err());
        assert_eq!(
            contained(into, Path::new("top/TPF3-MP.exe")).unwrap(),
            into.join("top").join("TPF3-MP.exe")
        );
    }

    /// An install folder of `platform` with an old launcher, a file of the
    /// player's own that ends in `.old`, and the package marker.
    fn installed(name: &str) -> (PathBuf, Install) {
        let dir = temp(name);
        let root = dir.join("install");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join(PACKAGE_MARKER),
            br#"{"version":"0.1.0","platform":"windows-x64"}"#,
        )
        .unwrap();
        fs::write(root.join("TPF3-MP.exe"), b"old").unwrap();
        fs::write(root.join("saves.old"), b"the player's own").unwrap();
        let install = Install::at(root.clone(), root.join("TPF3-MP.exe"), "windows-x64").unwrap();
        (dir, install)
    }

    /// Stages version `version` of a package holding `files` as downloaded,
    /// signed by `pair`, and returns the archive's bytes.
    fn stage(
        install: &Install,
        pair: &Ed25519KeyPair,
        version: &str,
        files: &[(&str, &[u8])],
    ) -> Vec<u8> {
        let name = format!("tpf3mp-{version}-windows-x64.zip");
        let staged = install.staging().join(version);
        fs::create_dir_all(&staged).unwrap();
        let top = format!("tpf3mp-{version}-windows-x64");
        let entries: Vec<(String, &[u8])> = files
            .iter()
            .map(|(path, data)| (format!("{top}/{path}"), *data))
            .collect();
        let borrowed: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(path, data)| (path.as_str(), *data))
            .collect();
        zip_of(&staged.join(&name), &borrowed);
        let bytes = fs::read(staged.join(&name)).unwrap();
        let json = manifest_for(version, &name, &bytes);
        fs::write(staged.join(MANIFEST), &json).unwrap();
        fs::write(staged.join(SIGNATURE), pair.sign(json.as_bytes())).unwrap();
        bytes
    }

    fn manifest_for(version: &str, name: &str, bytes: &[u8]) -> String {
        format!(
            r#"{{"version":"{version}","packages":{{"windows-x64":{{"name":"{name}","size":{},"sha256":"{}"}}}}}}"#,
            bytes.len(),
            signed::sha256_hex(bytes)
        )
    }

    #[test]
    fn a_staged_update_is_checked_again_before_it_is_installed() {
        let (dir, install) = installed("staged");
        let pair = key_pair();
        let bytes = stage(&install, &pair, "9.1.0", &[("TPF3-MP.exe", b"new")]);
        let archive = install
            .staging()
            .join("9.1.0")
            .join("tpf3mp-9.1.0-windows-x64.zip");
        // Tampered with after the download: nothing is installed.
        let mut tampered = bytes.clone();
        let at = tampered.len() / 2;
        tampered[at] ^= 1;
        fs::write(&archive, &tampered).unwrap();
        assert_eq!(install_staged(&install, &keys(&pair)).unwrap(), None);
        assert_eq!(fs::read(install.root.join("TPF3-MP.exe")).unwrap(), b"old");
        // As downloaded: installed.
        fs::write(&archive, &bytes).unwrap();
        assert_eq!(
            install_staged(&install, &keys(&pair)).unwrap().as_deref(),
            Some("9.1.0")
        );
        assert_eq!(fs::read(install.root.join("TPF3-MP.exe")).unwrap(), b"new");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn old_files_wait_for_the_new_version_to_start_and_nothing_else_goes() {
        let (dir, install) = installed("confirm");
        let pair = key_pair();
        stage(
            &install,
            &pair,
            "9.1.0",
            &[("TPF3-MP.exe", b"new"), ("docs.md", b"docs")],
        );
        install_staged(&install, &keys(&pair)).unwrap();
        let old = format!("TPF3-MP.exe.tpf3mp-{VERSION}.old");
        assert_eq!(fs::read(install.root.join(&old)).unwrap(), b"old");
        let journal = Journal::read(&install).unwrap();
        assert_eq!(journal.state, Stage::Swapped);
        // The new version starts, and shows its window.
        assert_eq!(
            start(&install, &keys(&pair), "9.1.0").unwrap(),
            Started::Continue
        );
        assert!(
            install.root.join(&old).exists(),
            "kept until the window shows"
        );
        confirm(&install, "9.1.0");
        assert!(!install.root.join(&old).exists());
        assert!(Journal::read(&install).is_none());
        // Only what the journal named went.
        assert_eq!(
            fs::read(install.root.join("saves.old")).unwrap(),
            b"the player's own"
        );
        assert_eq!(fs::read(install.root.join("docs.md")).unwrap(), b"docs");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_install_cut_short_is_undone_at_the_next_start() {
        let (dir, install) = installed("cut-short");
        // As if the process died after moving the old launcher aside and
        // putting the new one in place, before the rest.
        let old = format!("TPF3-MP.exe.tpf3mp-{VERSION}.old");
        fs::rename(install.root.join("TPF3-MP.exe"), install.root.join(&old)).unwrap();
        fs::write(install.root.join("TPF3-MP.exe"), b"new").unwrap();
        fs::write(install.root.join("added.txt"), b"new file").unwrap();
        Journal {
            from: VERSION.into(),
            to: "9.1.0".into(),
            entries: vec![
                Entry {
                    name: "TPF3-MP.exe".into(),
                    old: Some(old.clone()),
                },
                Entry {
                    name: "added.txt".into(),
                    old: None,
                },
                Entry {
                    name: "zz-not-reached.txt".into(),
                    old: Some("zz-not-reached.txt.old-never-made".into()),
                },
            ],
            state: Stage::Swapping,
            starts: 0,
        }
        .write(&install)
        .unwrap();
        let pair = key_pair();
        assert_eq!(
            start(&install, &keys(&pair), VERSION).unwrap(),
            Started::Continue
        );
        assert_eq!(fs::read(install.root.join("TPF3-MP.exe")).unwrap(), b"old");
        assert!(!install.root.join(&old).exists());
        assert!(!install.root.join("added.txt").exists());
        assert!(Journal::read(&install).is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_version_that_never_starts_is_rolled_back_and_skipped() {
        let (dir, install) = installed("never-starts");
        let pair = key_pair();
        stage(&install, &pair, "9.1.0", &[("TPF3-MP.exe", b"broken")]);
        install_staged(&install, &keys(&pair)).unwrap();
        for _ in 0..MAX_UNCONFIRMED_STARTS {
            assert_eq!(
                start(&install, &keys(&pair), "9.1.0").unwrap(),
                Started::Continue
            );
        }
        // One start too many without a window: back to the old version.
        assert_eq!(
            start(&install, &keys(&pair), "9.1.0").unwrap(),
            Started::Restart
        );
        assert_eq!(fs::read(install.root.join("TPF3-MP.exe")).unwrap(), b"old");
        assert!(install.skipped().contains(&"9.1.0".to_owned()));
        // The same version is not installed again.
        stage(&install, &pair, "9.1.0", &[("TPF3-MP.exe", b"broken")]);
        assert_eq!(install_staged(&install, &keys(&pair)).unwrap(), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn one_process_at_a_time_installs() {
        let (dir, install) = installed("lock");
        let held = install.lock(Duration::ZERO).unwrap();
        assert!(matches!(
            install.lock(Duration::ZERO),
            Err(UpdateError::Busy)
        ));
        drop(held);
        assert!(install.lock(Duration::ZERO).is_ok());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_a_marked_package_of_this_platform_updates() {
        let dir = temp("marker");
        let exe = dir.join("TPF3-MP.exe");
        assert!(Install::at(dir.clone(), exe.clone(), "windows-x64").is_err());
        fs::write(
            dir.join(PACKAGE_MARKER),
            br#"{"version":"0.1.0","platform":"linux-x64"}"#,
        )
        .unwrap();
        assert!(Install::at(dir.clone(), exe, "windows-x64").is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A small web server that answers GitHub's release URLs from `files`
    /// (path to body) with redirects as GitHub gives them, and 404 else.
    fn serve(files: Vec<(String, Vec<u8>)>) -> (String, Arc<AtomicBool>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopping.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(mut stream) = stream else { continue };
                let mut reader = io::BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let path = line.split(' ').nth(1).unwrap_or("/").to_owned();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                }
                let answer = match files.iter().find(|(served, _)| *served == path) {
                    Some((_, body)) => {
                        let mut answer = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes();
                        answer.extend_from_slice(body);
                        answer
                    }
                    None => {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    }
                };
                let _ = stream.write_all(&answer);
            }
        });
        (base, stop)
    }

    #[test]
    fn a_release_is_found_downloaded_and_installed_end_to_end() {
        let (dir, install) = installed("end-to-end");
        let pair = key_pair();
        let top = "tpf3mp-9.1.0-windows-x64";
        let name = format!("{top}.zip");
        let archive = dir.join(&name);
        zip_of(
            &archive,
            &[
                (&format!("{top}/TPF3-MP.exe"), b"new launcher"),
                (
                    &format!("{top}/{PACKAGE_MARKER}"),
                    br#"{"version":"9.1.0","platform":"windows-x64"}"#,
                ),
            ],
        );
        let bytes = fs::read(&archive).unwrap();
        let json = manifest_for("9.1.0", &name, &bytes);
        let signature = pair.sign(json.as_bytes()).as_ref().to_vec();
        let (base, stop) = serve(vec![
            (
                "/releases/latest/download/release.json".into(),
                json.clone().into_bytes(),
            ),
            (
                "/releases/latest/download/release.json.sig".into(),
                signature,
            ),
            (format!("/releases/download/v9.1.0/{name}"), bytes),
        ]);
        let source = Source {
            base: base.clone(),
            https_only: false,
        };
        let mut seen = 0;
        let checked = check_and_download(&install, &keys(&pair), &source, |_, bytes, _| {
            seen = bytes;
        })
        .unwrap();
        assert_eq!(checked, Checked::Downloaded("9.1.0".into()));
        assert!(seen > 0, "progress is reported");
        // Installed at the next start, as if the player waited.
        assert_eq!(
            start(&install, &keys(&pair), VERSION).unwrap(),
            Started::Restart
        );
        assert_eq!(
            fs::read(install.root.join("TPF3-MP.exe")).unwrap(),
            b"new launcher"
        );
        // A release signed by another key is not downloaded.
        let other = key_pair();
        let checked = check_and_download(&install, &keys(&other), &source, |_, _, _| {});
        assert!(matches!(checked, Err(UpdateError::BadSignature)));
        // No signed manifest: nothing to do, and said so.
        let unsigned = Source {
            base: format!("{base}/nothing"),
            https_only: false,
        };
        assert_eq!(
            check_and_download(&install, &keys(&pair), &unsigned, |_, _, _| {}).unwrap(),
            Checked::Unsigned
        );
        stop.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(base.trim_start_matches("http://"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_real_source_is_https_only() {
        let source = Source::github();
        assert!(source.https_only);
        assert!(source.base.starts_with("https://github.com/"));
        assert!(
            source
                .latest(MANIFEST)
                .ends_with("/releases/latest/download/release.json")
        );
        assert!(
            source
                .of("9.1.0", "x.zip")
                .ends_with("/releases/download/v9.1.0/x.zip")
        );
    }

    #[test]
    fn a_standalone_download_installs_repairs_and_refuses_tampering() {
        let dir = tempfile::tempdir().unwrap();
        // macOS's /var is itself a symlink. Exercise an actual installation
        // directory; linked installation paths must still be refused.
        // macOS's /var is a symlink; Windows canonicalization adds the
        // extended-length prefix, which Windows PowerShell 5 cannot parse.
        #[cfg(not(windows))]
        let canonical = dir.path().canonicalize().unwrap();
        #[cfg(windows)]
        let canonical = dir.path().to_path_buf();
        let root = canonical.join("local/Programs/TPF3-MP");
        let pair = key_pair();
        let name = format!("tpf3mp-{VERSION}-windows-x64.zip");
        let archive = dir.path().join(&name);
        let marker = format!(r#"{{"version":"{VERSION}","platform":"windows-x64"}}"#);
        zip_of(
            &archive,
            &[
                ("package/TPF3-MP.exe", b"launcher"),
                ("package/tpf3mp-agent.exe", b"agent"),
                ("package/tpf3mp_hook.dll", b"hook"),
                ("package/tpf3mp-package.json", marker.as_bytes()),
                ("package/mod/tpf3mp_1/mod.json", b"{}"),
                (
                    "package/mod/tpf3mp_1/content/example.lua",
                    b"-- current mod",
                ),
                (
                    "package/tools/install.ps1",
                    include_bytes!("../../../packaging/windows/tools/install.ps1"),
                ),
                (
                    "package/tools/manage.ps1",
                    include_bytes!("../../../packaging/windows/tools/manage.ps1"),
                ),
            ],
        );
        let bytes = fs::read(&archive).unwrap();
        let json = manifest_for(VERSION, &name, &bytes);
        let (base, stop) = serve(vec![
            (
                "/releases/latest/download/release.json".into(),
                json.as_bytes().to_vec(),
            ),
            (
                "/releases/latest/download/release.json.sig".into(),
                pair.sign(json.as_bytes()).as_ref().to_vec(),
            ),
            (
                format!("/releases/download/v{VERSION}/{name}"),
                bytes.clone(),
            ),
        ]);
        let source = Source {
            base,
            https_only: false,
        };
        let mut progress = 0;
        assert_eq!(
            bootstrap_from(&root, &keys(&pair), &source, |_, bytes, _| progress = bytes).unwrap(),
            VERSION
        );
        assert!(progress > 0);
        assert_eq!(fs::read(root.join("tpf3mp_hook.dll")).unwrap(), b"hook");
        assert_eq!(
            Install::at(root.clone(), root.join("TPF3-MP.exe"), "windows-x64")
                .unwrap()
                .root,
            root
        );
        setup_complete(&root, VERSION);
        fs::write(root.join("tpf3mp_hook.dll"), b"damaged").unwrap();
        bootstrap_from(&root, &keys(&pair), &source, |_, _, _| {}).unwrap();
        assert_eq!(
            fs::read(root.join("tpf3mp_hook.dll")).unwrap(),
            b"hook",
            "repair restores the same version"
        );
        setup_complete(&root, VERSION);
        assert!(matches!(
            bootstrap_from(&root, &keys(&key_pair()), &source, |_, _, _| {}),
            Err(UpdateError::BadSignature)
        ));
        assert_eq!(fs::read(root.join("TPF3-MP.exe")).unwrap(), b"launcher");
        let foreign = canonical.join("foreign");
        fs::create_dir(&foreign).unwrap();
        fs::write(foreign.join("save.sav"), b"keep").unwrap();
        assert!(bootstrap_from(&foreign, &keys(&pair), &source, |_, _, _| {}).is_err());
        assert_eq!(fs::read(foreign.join("save.sav")).unwrap(), b"keep");

        #[cfg(windows)]
        check_windows_setup_scripts(&root, &canonical);

        stop.store(true, Ordering::SeqCst);
        // A valid signature cannot make a damaged download acceptable.
        let (base, stopped) = serve(vec![
            (
                "/releases/latest/download/release.json".into(),
                json.as_bytes().to_vec(),
            ),
            (
                "/releases/latest/download/release.json.sig".into(),
                pair.sign(json.as_bytes()).as_ref().to_vec(),
            ),
            (
                format!("/releases/download/v{VERSION}/{name}"),
                b"truncated".to_vec(),
            ),
        ]);
        let untouched = canonical.join("bad-download");
        assert!(
            bootstrap_from(
                &untouched,
                &keys(&pair),
                &Source {
                    base,
                    https_only: false
                },
                |_, _, _| {}
            )
            .is_err()
        );
        assert!(!untouched.join("TPF3-MP.exe").exists());
        stopped.store(true, Ordering::SeqCst);
    }

    #[cfg(windows)]
    fn check_windows_setup_scripts(root: &Path, temp: &Path) {
        let local = temp.join("local");
        let mods = temp.join("Steam/userdata/123/3493540/local/staging_area");
        let saves = mods.parent().unwrap().join("save/keep.sav");
        fs::create_dir_all(saves.parent().unwrap()).unwrap();
        fs::write(&saves, b"player's world").unwrap();
        let registry = format!(r"HKCU:\Software\TPF3MP-Setup-Test-{}", std::process::id());
        let run = |script: &Path, args: &[&std::ffi::OsStr]| {
            let output = std::process::Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                ])
                .arg(script)
                .args(args)
                .env("LOCALAPPDATA", &local)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        let installer = root.join("tools/install.ps1");
        run(
            &installer,
            &[
                "-ModsDir".as_ref(),
                mods.as_os_str(),
                "-SteamRoot".as_ref(),
                temp.join("Steam").as_os_str(),
            ],
        );
        assert_eq!(
            crate::installed::installed_mod(&local.join("TPF3-MP")).as_deref(),
            Some(VERSION)
        );
        // Repair replaces damaged files and remembers a custom mods folder.
        fs::write(mods.join("tpf3mp_1/content/example.lua"), b"broken").unwrap();
        run(
            &installer,
            &["-SteamRoot".as_ref(), temp.join("Steam").as_os_str()],
        );
        assert_eq!(
            fs::read(mods.join("tpf3mp_1/content/example.lua")).unwrap(),
            b"-- current mod"
        );
        let manager = root.join("tools/manage.ps1");
        let programs = temp.join("shortcuts/programs");
        let desktop = temp.join("shortcuts/desktop");
        let mut arguments: Vec<&std::ffi::OsStr> = vec![
            "-Root".as_ref(),
            root.as_os_str(),
            "-ProgramsDir".as_ref(),
            programs.as_os_str(),
            "-DesktopDir".as_ref(),
            desktop.as_os_str(),
            "-RegistryKey".as_ref(),
            registry.as_ref(),
        ];
        arguments.push("-DesktopShortcut".as_ref());
        run(&manager, &arguments);
        assert!(programs.join("TPF3-MP.lnk").is_file());
        assert!(desktop.join("TPF3-MP.lnk").is_file());
        assert!(root.join("tpf3mp-managed.json").is_file());
        // Removal runs from outside the installed directory, just as the
        // helper used by the real setup window after it closes.
        let helper = temp.join("manage.ps1");
        fs::copy(&manager, &helper).unwrap();
        run(
            &installer,
            &[
                "-Uninstall".as_ref(),
                "-SteamRoot".as_ref(),
                temp.join("Steam").as_os_str(),
            ],
        );
        arguments.push("-Uninstall".as_ref());
        run(&helper, &arguments);
        assert!(!mods.join("tpf3mp_1").exists());
        assert!(!root.exists());
        assert!(!programs.join("TPF3-MP.lnk").exists());
        assert!(!desktop.join("TPF3-MP.lnk").exists());
        assert_eq!(fs::read(&saves).unwrap(), b"player's world");
    }
}
