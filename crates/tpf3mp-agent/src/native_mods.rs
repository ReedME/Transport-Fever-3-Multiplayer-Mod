//! Native mods (D29, docs/NATIVE_MODS.md): what the launcher hands the game
//! it starts, what it declares to a room, and the player's panel of them.
//!
//! Before the game starts, the launcher writes the enabled packages for the
//! game's build (the executable's SHA-256) to `native-mods/enabled.json` in
//! its data folder and names that file in the game's environment
//! (`TPF3MP_NATIVE_MODS`); the hook enables nothing else. With nothing
//! enabled, or no native mod installed, the game gets no variable and the
//! executable is not even hashed.
//!
//! The simulation-changing ones among them are the game's native terms
//! ([`tpf3mp_nativemods::terms`]): the launcher declares them in the content
//! manifest, with their settings for a room's owner ([`NativeMod`]), and a
//! member who joins a room whose terms differ gets the room's versions from
//! the signed index and the room's settings ([`match_room`]), never from
//! another player. Mods from Mod Hub are not native: players get those
//! through their own game (D28).
//!
//! Downloads need the launcher's HTTPS source ([`NativeSetup`]); this crate
//! builds without one, and a launcher built without the native-mods key
//! offers no native mod at all.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::SystemTime,
};

use serde::Serialize;
use tpf3mp_nativemods::{
    FOLDER, enabled,
    enabled::Enabled,
    features::{self, BUILT_IN, Effect},
    fetch::sha256_of_file,
    index::{Index, Package, Setting},
    registry::{Proposal, Proposals},
    resolve,
    store::{EffectiveSettingsByPackage, Source, Store, StoreError},
    terms,
};
use tpf3mp_proto::{NativeMod, NativeSetting, NativeValue, Text};
use tracing::{info, warn};

/// Where the launcher gets native mods: the project's signed index, as the
/// launcher binary fetches it, and the keys it is signed with.
#[derive(Clone)]
pub struct NativeSetup {
    pub source: Arc<dyn Source + Send + Sync>,
    pub keys: Vec<Vec<u8>>,
}

impl std::fmt::Debug for NativeSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeSetup")
            .field("keys", &self.keys.len())
            .finish_non_exhaustive()
    }
}

/// What the game the launcher starts gets of the native mods.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameNative {
    /// The variable naming the enabled list, if any is enabled.
    pub env: Option<(String, String)>,
    /// The simulation-changing ones, as the room compares them.
    pub terms: Vec<NativeMod>,
}

fn store(data_dir: &Path) -> Result<Store, String> {
    Store::open(data_dir.join(FOLDER)).map_err(|error| format!("native mods: {error}"))
}

/// The SHA-256 of the game's executable `exe`, computed once for each
/// version of the file.
pub fn build_of(exe: &Path) -> Result<String, String> {
    type Seen = Option<(PathBuf, u64, Option<SystemTime>, String)>;
    static SEEN: Mutex<Seen> = Mutex::new(None);
    let meta = std::fs::metadata(exe)
        .map_err(|error| format!("native mods: cannot read {}: {error}", exe.display()))?;
    let (len, modified) = (meta.len(), meta.modified().ok());
    let mut seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((path, l, m, build)) = seen.as_ref()
        && path == exe
        && *l == len
        && *m == modified
    {
        return Ok(build.clone());
    }
    let build = sha256_of_file(exe)
        .map_err(|error| format!("native mods: cannot hash {}: {error}", exe.display()))?;
    *seen = Some((exe.to_owned(), len, modified, build.clone()));
    Ok(build)
}

/// The variable for the game's environment naming the native mods enabled
/// for `exe`, with the launcher's data folder `data_dir`, and their terms.
/// A registry that cannot be read stops the start: what it enables cannot
/// be told (fail closed).
pub fn game_env(data_dir: &Path, exe: &Path) -> Result<GameNative, String> {
    let store = store(data_dir)?;
    // Game mods of packages on go into the game's `mods`, and those of
    // packages switched off come out, before anything else.
    let ours_there = |mods: &Path| {
        std::fs::read_dir(mods).is_ok_and(|entries| {
            entries.flatten().any(|e| {
                e.path()
                    .join(tpf3mp_nativemods::store::GAME_MOD_MARK)
                    .is_file()
            })
        })
    };
    let wanted = store
        .registry()
        .packages
        .values()
        .any(|i| i.package().is_some_and(|p| !p.game_mods.is_empty()));
    if let Some(game) = exe
        .parent()
        .filter(|game| wanted || ours_there(&game.join("mods")))
    {
        let build = build_of(exe)?;
        for line in store
            .sync_game_mods(&build, &game.join("mods"))
            .map_err(|error| format!("native mods' game mods: {error}"))?
        {
            info!("{line}");
        }
    }
    if !store.registry().packages.values().any(|i| i.enabled) {
        return Ok(GameNative::default());
    }
    let build = build_of(exe)?;
    let (path, left_out) = store
        .write_enabled(&build)
        .map_err(|error| format!("native mods: {error}"))?;
    for id in left_out {
        warn!(%id, "the native mod is enabled but not for this build of the game; left out");
    }
    info!(file = %path.display(), "the game gets the enabled native mods");
    let list = enabled::read(&path)?;
    Ok(GameNative {
        env: Some((enabled::ENV.to_owned(), path.to_string_lossy().into_owned())),
        terms: room_terms(&store, &list)?,
    })
}

/// The terms the next game on `exe` runs with, as [`game_env`] would hand
/// them to it, without writing anything.
pub fn next_terms(data_dir: &Path, exe: &Path) -> Result<Vec<NativeMod>, String> {
    let store = store(data_dir)?;
    if !store.registry().packages.values().any(|i| i.enabled) {
        return Ok(Vec::new());
    }
    let (list, _) = store
        .enabled_for(&build_of(exe)?)
        .map_err(|error| format!("native mods: {error}"))?;
    room_terms(&store, &list)
}

/// The simulation-changing packages of `list` as a room's native mods.
fn room_terms(store: &Store, list: &Enabled) -> Result<Vec<NativeMod>, String> {
    terms::terms(list, BUILT_IN)
        .into_iter()
        .map(|term| {
            let package = list
                .packages
                .iter()
                .find(|p| p.id == term.id)
                .ok_or_else(|| format!("native mods: {} is not enabled", term.id))?;
            let name = store
                .registry()
                .packages
                .get(&term.id)
                .and_then(|i| i.package())
                .map_or(term.id.as_str(), |p| p.name.as_str());
            let settings = package
                .settings
                .iter()
                .map(|(key, value)| {
                    Ok(NativeSetting {
                        key: Text::new(key)
                            .map_err(|_| format!("the setting {key} is too long"))?,
                        value: to_value(value)?,
                    })
                })
                .collect::<Result<_, String>>()?;
            Ok(NativeMod {
                id: Text::new(&term.id).map_err(|_| format!("the id {} is too long", term.id))?,
                version: Text::new(term.mod_version())
                    .map_err(|_| format!("the version of {} is too long", term.id))?,
                name: Text::lossy(name),
                settings,
            })
        })
        .collect()
}

/// A setting as a room tells it.
pub fn to_value(setting: &Setting) -> Result<NativeValue, String> {
    Ok(match setting {
        Setting::Bool(on) => NativeValue::Bool(*on),
        Setting::Int(n) => NativeValue::Int(*n),
        Setting::Text(text) => NativeValue::Text(
            Text::new(text).map_err(|_| "a native mod's text setting is too long".to_owned())?,
        ),
    })
}

/// A setting a room told, as the store keeps it.
pub fn from_value(value: &NativeValue) -> Setting {
    match value {
        NativeValue::Bool(on) => Setting::Bool(*on),
        NativeValue::Int(n) => Setting::Int(*n),
        NativeValue::Text(text) => Setting::Text(text.as_str().to_owned()),
    }
}

/// The native mods panel: every package of the server's index and of the
/// last signed index for this game's build, every one installed, and when
/// the player asks, the mods proposed for the server's registry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct NativeView {
    /// Whether there is an index to offer mods from: the server played on
    /// offers one, or this launcher was built with the native-mods key.
    pub available: bool,
    /// The serial of the index shown, once one was accepted.
    pub serial: Option<u64>,
    /// The server whose index is shown, if any.
    pub server: Option<String>,
    /// Whether the mods proposed for the server's registry are shown.
    pub proposed_shown: bool,
    pub packages: Vec<NativeRow>,
    /// Why the panel could not be read, if it could not.
    pub problem: Option<String>,
}

/// One native mod in the panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeRow {
    pub id: String,
    pub name: String,
    pub description: String,
    /// The mod's own project page, to open in the browser.
    pub homepage: Option<String>,
    /// Who made it.
    pub authors: Vec<String>,
    /// The address of an image of it, to show.
    pub preview: Option<String>,
    /// Whether every game of a room must run it alike.
    pub simulation: bool,
    /// The newest version the index has for this game's build.
    pub available: Option<String>,
    /// Its download, all its files together, in bytes.
    pub available_size: Option<u64>,
    pub installed: Option<String>,
    /// The installed version's files together, in bytes.
    pub installed_size: Option<u64>,
    /// Where it comes in the order the game loads the native mods enabled,
    /// from 1: after those it depends on, the rest by id.
    pub load_order: Option<usize>,
    /// Why it cannot be switched on beside those that are, if it is off
    /// and cannot: its features clash with theirs.
    pub clash: Option<String>,
    /// Whether it carries code of its own, plugins the hook loads into the
    /// game (proposed D29 amendment).
    pub code: bool,
    /// Why this TPF3-MP cannot install it, if it cannot: a hook feature it
    /// lacks, or no plugin it can load.
    pub unsupported: Option<String>,
    /// How often its releases were downloaded, as its host says.
    pub downloads: Option<u64>,
    /// When it last changed, `YYYY-MM-DD`, as its host says.
    pub updated: Option<String>,
    /// The version a rollback goes back to.
    pub previous: Option<String>,
    pub enabled: bool,
    /// Whether the index still lists it.
    pub listed: bool,
    /// For a mod only proposed for the server's registry: its pull request.
    /// Nobody has reviewed it.
    pub proposed: Option<Proposal>,
}

/// Why this TPF3-MP cannot install `package`, if it cannot.
fn unsupported(package: &Package) -> Option<String> {
    if let Some(feature) = package
        .features
        .iter()
        .find(|id| features::find(BUILT_IN, id).is_none())
    {
        return Some(format!(
            "it needs the hook feature {feature}, which this TPF3-MP does not have"
        ));
    }
    if package.plugins.is_empty() {
        return None;
    }
    let here = tpf3mp_nativemods::index::current_platform()
        .map(|platform| package.plugins_for(platform))
        .unwrap_or_default();
    if here.is_empty() {
        Some("it has no plugin for this platform".into())
    } else if here
        .iter()
        .any(|p| p.abi != tpf3mp_nativemods::index::PLUGIN_ABI)
    {
        Some("its plugin is built for another version of TPF3-MP".into())
    } else {
        None
    }
}

/// Whether `package` changes the simulation: it says so, or a feature it
/// names does (an unknown one counts, fail closed).
fn changes_simulation(package: &Package) -> bool {
    package.simulation
        || !package.plugins.is_empty()
        || !package.hooks.is_empty()
        || package
            .features
            .iter()
            .any(|id| features::find(BUILT_IN, id).is_none_or(|f| f.effect == Effect::Simulation))
}

/// Where the launcher keeps a server's index: `native-mods/servers/<the
/// start of the SHA-256 of its address>/`.
fn server_dir(data_dir: &Path, server: &str) -> PathBuf {
    let key = tpf3mp_nativemods::signed::sha256_hex(server.trim().to_ascii_lowercase().as_bytes());
    data_dir.join(FOLDER).join("servers").join(&key[..16])
}

const SERVER_INDEX: &str = "index.json";
const SERVER_PROPOSED: &str = "proposed.json";

/// Takes in `json`, the index the server `server` serves with `serial` (or
/// with `proposed`, the mods proposed for it), and keeps it: the server
/// vouches for it as it vouches for its rooms (proposed D29 amendment). An
/// index whose serial is older than the one kept is refused, and one whose
/// serial matches must have byte-identical JSON, so a serial cannot name
/// different contents.
pub fn accept_server_index(
    data_dir: &Path,
    server: &str,
    serial: u64,
    json: &[u8],
    proposed: bool,
) -> Result<(), String> {
    let found = if proposed {
        Proposals::parse(json).map(|p| p.serial)
    } else {
        Index::parse(json)
            .map(|index| index.serial)
            .map_err(|error| error.to_string())
    }
    .map_err(|error| format!("{server}'s native mods: {error}"))?;
    if found != serial {
        return Err(format!("{server}'s native mods name another serial"));
    }
    let dir = server_dir(data_dir, server);
    let file = dir.join(if proposed {
        SERVER_PROPOSED
    } else {
        SERVER_INDEX
    });
    let kept_bytes = match std::fs::read(&file) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!(
                "cannot read the saved index from {server}: {error}"
            ));
        }
    };
    if let Some(kept_bytes) = kept_bytes {
        let kept = if proposed {
            Proposals::parse(&kept_bytes).map(|p| p.serial)
        } else {
            Index::parse(&kept_bytes)
                .map(|index| index.serial)
                .map_err(|error| error.to_string())
        }
        .map_err(|error| format!("cannot verify the saved index from {server}: {error}"))?;
        if kept > serial {
            return Err(format!(
                "{server} served an older index of native mods than before"
            ));
        }
        if kept == serial && kept_bytes != json {
            return Err(format!(
                "{server} changed its native-mod index bytes without increasing its serial"
            ));
        }
    }
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let partial = dir.join(".partial");
    std::fs::write(&partial, json).map_err(|error| error.to_string())?;
    std::fs::rename(&partial, &file).map_err(|error| error.to_string())
}

/// The index the server `server` served last, kept.
pub fn server_index(data_dir: &Path, server: &str) -> Option<Index> {
    let bytes = std::fs::read(server_dir(data_dir, server).join(SERVER_INDEX)).ok()?;
    Index::parse(&bytes).ok()
}

/// The mods proposed for the server's registry, as it served them last.
pub fn server_proposals(data_dir: &Path, server: &str) -> Option<Proposals> {
    let bytes = std::fs::read(server_dir(data_dir, server).join(SERVER_PROPOSED)).ok()?;
    Proposals::parse(&bytes).ok()
}

/// The indexes to install from, the server's first.
fn indexes(
    store: &mut Store,
    data_dir: &Path,
    setup: Option<&NativeSetup>,
    server: Option<&str>,
) -> (Vec<Index>, Option<String>) {
    let mut found = Vec::new();
    let mut problem = None;
    if let Some(index) = server.and_then(|server| server_index(data_dir, server)) {
        found.push(index);
    }
    if let Some(setup) = setup.filter(|setup| !setup.keys.is_empty()) {
        match store.cached_index(&setup.keys) {
            Ok(index) => found.push(index),
            Err(StoreError::NoIndex) => {}
            Err(error) => problem = Some(format!("native mods: {error}")),
        }
    }
    (found, problem)
}

/// The panel for a game whose executable hashes to `build`, if known,
/// played on `server`, with the proposed mods when `proposed`.
pub fn view(
    data_dir: &Path,
    setup: Option<&NativeSetup>,
    build: Option<&str>,
    server: Option<&str>,
    proposed: bool,
) -> NativeView {
    let server_has = server.and_then(|server| server_index(data_dir, server));
    let keyed = setup.is_some_and(|setup| !setup.keys.is_empty());
    if server_has.is_none() && !keyed {
        return NativeView::default();
    }
    let mut view = NativeView {
        available: true,
        server: server_has.as_ref().and(server).map(str::to_owned),
        proposed_shown: proposed,
        ..NativeView::default()
    };
    let mut store = match store(data_dir) {
        Ok(store) => store,
        Err(error) => {
            view.problem = Some(error);
            return view;
        }
    };
    let (indexes, problem) = indexes(&mut store, data_dir, setup, server);
    view.problem = problem;
    view.serial = indexes.first().map(|index| index.serial);
    let proposals = proposed
        .then(|| server.and_then(|server| server_proposals(data_dir, server)))
        .flatten();
    let mut ids: Vec<&str> = store
        .registry()
        .packages
        .keys()
        .map(String::as_str)
        .collect();
    for index in &indexes {
        ids.extend(index.packages.iter().map(|p| p.id.as_str()));
    }
    if let Some(proposals) = &proposals {
        ids.extend(proposals.packages.iter().map(|p| p.id.as_str()));
    }
    ids.sort_unstable();
    ids.dedup();
    view.packages = ids
        .into_iter()
        .filter_map(|id| row(&store, &indexes, proposals.as_ref(), id, build))
        .collect();
    view
}

fn row(
    store: &Store,
    indexes: &[Index],
    proposals: Option<&Proposals>,
    id: &str,
    build: Option<&str>,
) -> Option<NativeRow> {
    let installed = store.registry().packages.get(id);
    let mut versions = indexes
        .iter()
        .map(|index| index.versions(id))
        .find(|versions| !versions.is_empty())
        .unwrap_or_default();
    let proposal = match proposals {
        Some(proposals) if versions.is_empty() => {
            versions = proposals.packages.iter().filter(|p| p.id == id).collect();
            versions.sort_by_key(|p| std::cmp::Reverse(p.semver()));
            proposals.pulls.get(id).filter(|_| !versions.is_empty())
        }
        _ => None,
    };
    let available = versions
        .iter()
        .find(|p| build.is_none_or(|build| p.runs_on(build)))
        .copied();
    let shown = installed.and_then(|i| i.package()).or(available)?;
    // The newest description of it: what the index offers, else what is
    // installed.
    let newest = available.unwrap_or(shown);
    let size = |package: &Package| package.files.iter().map(|f| f.size).sum::<u64>();
    let listing = indexes.iter().find_map(|index| index.listings.get(id));
    let load_order = installed
        .filter(|i| i.enabled)
        .and_then(|_| store.load_order().iter().position(|other| other == id))
        .map(|at| at + 1);
    let clash = installed
        .filter(|i| !i.enabled)
        .and_then(|_| store.clash(BUILT_IN, id));
    Some(NativeRow {
        id: id.to_owned(),
        name: shown.name.clone(),
        description: shown.description.clone(),
        homepage: shown.homepage.clone(),
        authors: newest.authors.clone(),
        preview: newest.preview.clone(),
        simulation: changes_simulation(shown),
        available: available.map(|p| p.version.clone()),
        available_size: available.map(size),
        installed: installed.map(|i| i.current.clone()),
        installed_size: installed.and_then(|i| i.package()).map(size),
        load_order,
        clash,
        code: !newest.plugins.is_empty() || !newest.hooks.is_empty(),
        unsupported: available.and_then(unsupported),
        downloads: listing.and_then(|l| l.downloads),
        updated: listing.and_then(|l| l.updated.clone()),
        previous: installed.and_then(|i| i.previous.clone()),
        enabled: installed.is_some_and(|i| i.enabled),
        listed: !versions.is_empty(),
        proposed: proposal.cloned(),
    })
}

/// What the player does with a native mod in the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Fetch the signed index again.
    Refresh,
    /// Install the newest version for this build, or upgrade to it.
    Install(String),
    /// Install a mod only proposed for the server's registry, which the
    /// player chose knowing nobody reviewed it.
    InstallProposed(String),
    Uninstall(String),
    Rollback(String),
    Enable(String, bool),
}

/// One validated executable package in a review shown to the player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PluginConsentPackage {
    pub id: String,
    pub name: String,
    pub version: String,
    pub authors: Vec<String>,
    /// `project` or the canonical authenticated `host:port` that offered it.
    pub source: String,
    /// Full SHA-256 package identity, including effective settings.
    pub digest: String,
    /// Whether this exact source and identity already had approval.
    pub approved: bool,
}

/// The exact executable packages a player must approve before an action can
/// fetch, install, enable, or roll back native plugin code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PluginConsentPrompt {
    pub request_id: String,
    pub action: String,
    pub packages: Vec<PluginConsentPackage>,
}

/// The operation held while the UI waits for a player decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PendingPluginOperation {
    Change(Change),
    MatchRoom {
        room: Vec<NativeMod>,
        resume_join: bool,
    },
}

/// An untrusted UI response can only resolve the request currently held here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingPluginConsent {
    pub prompt: PluginConsentPrompt,
    pub operation: PendingPluginOperation,
}

/// Carries out `change`, for a game whose executable hashes to `build`.
/// Blocking: downloads run here.
pub fn apply(
    data_dir: &Path,
    setup: &NativeSetup,
    build: Option<&str>,
    server: Option<&str>,
    change: Change,
) -> Result<(), String> {
    let error = |error: StoreError| format!("native mods: {error}");
    require_plugin_consent(&plugin_consent_review(
        data_dir, setup, build, server, &change, false,
    )?)?;
    let mut store = store(data_dir)?;
    let proposed = matches!(&change, Change::InstallProposed(_));
    match change {
        // The server's index is fetched over its connection, by the caller.
        Change::Refresh if setup.keys.is_empty() => Ok(()),
        Change::Refresh => store
            .refresh(setup.source.as_ref(), &setup.keys)
            .map(drop)
            .map_err(error),
        Change::Install(id) | Change::InstallProposed(id) => {
            let build =
                build.ok_or("start the game once, or name it, so the launcher knows its build")?;
            let (index, source) = if proposed {
                let server = server.ok_or("connect to the server that proposed this native mod")?;
                let index = server_proposals(data_dir, server)
                    .map(|proposals| proposals.index())
                    .ok_or("the server proposes no such mod now")?;
                (index, server_source_id(server)?)
            } else {
                index_listing_with_source(
                    &mut store,
                    data_dir,
                    setup,
                    server,
                    &[(&id, None)],
                    false,
                )?
            };
            let packages = resolved_packages_for_install(&store, &index, build, &id, None)?;
            let settings = effective_settings_by_package(&store, &packages, &BTreeMap::new());
            let done = store
                .install_from(
                    resolve::Context {
                        index: &index,
                        build,
                        registry: BUILT_IN,
                    },
                    &id,
                    &source,
                    &settings,
                    setup.source.as_ref(),
                )
                .map_err(error)?;
            info!(installed = ?done, "installed native mods");
            Ok(())
        }
        Change::Uninstall(id) => store.uninstall(&id).map_err(error),
        Change::Rollback(id) => store.rollback(&id).map_err(error),
        Change::Enable(id, on) => store.set_enabled(&id, on).map_err(error),
    }
}

/// The index with every one of `wanted` in it and the source identity that
/// authorized it: the authenticated server's, else the signed project index.
fn index_listing_with_source(
    store: &mut Store,
    data_dir: &Path,
    setup: &NativeSetup,
    server: Option<&str>,
    wanted: &[(&str, Option<&str>)],
    refresh_project: bool,
) -> Result<(Index, String), String> {
    let lists = |index: &Index| {
        wanted.iter().all(|(id, version)| match version {
            Some(version) => index.package(id, version).is_some(),
            None => !index.versions(id).is_empty(),
        })
    };
    if let Some((origin, index)) = server
        .and_then(|server| server_index(data_dir, server).map(|index| (server, index)))
        .filter(|(_, index)| lists(index))
    {
        return Ok((index, server_source_id(origin)?));
    }
    if setup.keys.is_empty() {
        return Err("no index of native mods lists the room's".into());
    }
    let index = if refresh_project {
        store
            .refresh(setup.source.as_ref(), &setup.keys)
            .map_err(|error| format!("native mods: {error}"))?
    } else {
        match store.cached_index(&setup.keys) {
            Ok(index) => index,
            Err(StoreError::NoIndex) => store
                .refresh(setup.source.as_ref(), &setup.keys)
                .map_err(|error| format!("native mods: {error}"))?,
            Err(error) => return Err(format!("native mods: {error}")),
        }
    };
    if !lists(&index) {
        return Err("no current signed native-mod index lists the requested package".into());
    }
    Ok((index, "project".to_owned()))
}

/// Canonical source identity accepted by the persistent plugin-consent
/// store. The address itself came from the launcher's server connection.
fn server_source_id(server: &str) -> Result<String, String> {
    let server = crate::launcher::server_address(server)?;
    let (host, port) = server
        .rsplit_once(':')
        .ok_or_else(|| "the native-mod source has no server port".to_owned())?;
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| "the native-mod source has an invalid server port".to_owned())?;
    let host = if let Some(ip) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        format!(
            "[{}]",
            ip.parse::<std::net::Ipv6Addr>().map_err(|_| {
                "the native-mod source has an invalid IPv6 server address".to_owned()
            })?
        )
    } else if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        ip.to_string()
    } else if !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        host.to_ascii_lowercase()
    } else {
        return Err("the native-mod source has an invalid server address".into());
    };
    Ok(format!("{host}:{port}"))
}

fn effective_settings(
    store: &Store,
    package: &Package,
    override_settings: Option<&BTreeMap<String, Setting>>,
) -> BTreeMap<String, Setting> {
    if let Some(settings) = override_settings {
        return settings.clone();
    }
    let mut settings = package.settings.clone();
    if let Some(installed) = store.registry().packages.get(&package.id) {
        for (key, value) in &installed.settings {
            if package.settings.contains_key(key) {
                settings.insert(key.clone(), value.clone());
            }
        }
    }
    settings
}

fn effective_settings_from_room(
    package: &Package,
    native: Option<&NativeMod>,
) -> Result<BTreeMap<String, Setting>, String> {
    let mut settings = package.settings.clone();
    if let Some(native) = native {
        if native.settings.len() != package.settings.len() {
            return Err(format!(
                "the room does not give every setting of {}",
                package.id
            ));
        }
        let mut seen = BTreeSet::new();
        for setting in &native.settings {
            let key = setting.key.as_str();
            if !seen.insert(key) {
                return Err(format!(
                    "the room gives setting {key} of {} more than once",
                    package.id
                ));
            }
            let value = from_value(&setting.value);
            let default = package
                .settings
                .get(key)
                .ok_or_else(|| format!("the room sets unknown setting {key} on {}", package.id))?;
            if !default.same_type(&value) {
                return Err(format!(
                    "the room gives setting {key} of {} another type",
                    package.id
                ));
            }
            settings.insert(key.to_owned(), value);
        }
    }
    Ok(settings)
}

fn plugin_item(
    store: &Store,
    package: &Package,
    source: &str,
    settings: &BTreeMap<String, Setting>,
) -> Result<PluginConsentPackage, String> {
    let digest = terms::package_digest(package, settings);
    let approved = store
        .has_consent(source, &digest)
        .map_err(|error| format!("native plugin consent: {error}"))?;
    Ok(PluginConsentPackage {
        id: package.id.clone(),
        name: package.name.clone(),
        version: package.version.clone(),
        authors: package.authors.clone(),
        source: source.to_owned(),
        digest,
        approved,
    })
}

fn resolved_packages_for_install(
    store: &Store,
    index: &Index,
    build: &str,
    id: &str,
    version: Option<&str>,
) -> Result<Vec<Package>, String> {
    let installed: Vec<&Package> = store
        .registry()
        .packages
        .values()
        .filter_map(|installed| installed.package())
        .collect();
    let context = resolve::Context {
        index,
        build,
        registry: BUILT_IN,
    };
    let resolution = match version {
        Some(version) => resolve::resolve_exact_plan(context, id, version, &installed),
        None => resolve::resolve_plan(context, &[id], &installed),
    }
    .map_err(|error| format!("native mods: {error}"))?;
    Ok(resolution.closure.into_iter().cloned().collect())
}

fn effective_settings_by_package(
    store: &Store,
    packages: &[Package],
    overrides: &BTreeMap<String, BTreeMap<String, Setting>>,
) -> EffectiveSettingsByPackage {
    packages
        .iter()
        .map(|package| {
            let settings = overrides
                .get(&package.id)
                .cloned()
                .unwrap_or_else(|| effective_settings(store, package, None));
            (package.id.clone(), settings)
        })
        .collect()
}

/// Resolves an install without fetching package files or changing the
/// registry, and computes the full plugin closure the action would use.
fn plugin_items_for_install(
    store: &Store,
    index: &Index,
    index_source: &str,
    build: &str,
    id: &str,
    version: Option<&str>,
    room_settings: &BTreeMap<String, BTreeMap<String, Setting>>,
) -> Result<Vec<PluginConsentPackage>, String> {
    let packages = resolved_packages_for_install(store, index, build, id, version)?;
    let mut found = BTreeMap::new();
    for package in &packages {
        if package.plugins.is_empty() {
            continue;
        }
        let source = if index.package(&package.id, &package.version).is_some() {
            index_source.to_owned()
        } else {
            store
                .source_for(&package.id, &package.version)
                .map(str::to_owned)
                .ok_or_else(|| {
                    format!(
                        "cannot verify where installed plugin {} {} came from; reinstall it from a trusted native-mod index before using it",
                        package.id, package.version
                    )
                })?
        };
        let settings = room_settings
            .get(&package.id)
            .cloned()
            .unwrap_or_else(|| effective_settings(store, package, None));
        let item = plugin_item(store, package, &source, &settings)?;
        found.insert(
            (item.id.clone(), item.version.clone(), item.source.clone()),
            item,
        );
    }
    Ok(found.into_values().collect())
}

fn plugin_items_for_installed(
    store: &Store,
    id: &str,
    version: Option<&str>,
) -> Result<Vec<PluginConsentPackage>, String> {
    let installed = store
        .registry()
        .packages
        .get(id)
        .ok_or_else(|| format!("native mod {id} is not installed"))?;
    let version = version.unwrap_or(&installed.current);
    let package = installed
        .versions
        .get(version)
        .ok_or_else(|| format!("native mod {id} {version} is not installed"))?;
    let mut closure = BTreeMap::new();
    let mut visiting = BTreeSet::new();
    installed_package_closure(store, package, &mut closure, &mut visiting)?;
    closure
        .into_values()
        .filter(|package| !package.plugins.is_empty())
        .map(|package| {
            let source = store
                .source_for(&package.id, &package.version)
                .map(str::to_owned)
                .ok_or_else(|| {
                    format!(
                        "cannot verify where installed plugin {} {} came from; reinstall it from a trusted native-mod index before using it",
                        package.id, package.version
                    )
                })?;
            let settings = effective_settings(store, &package, None);
            plugin_item(store, &package, &source, &settings)
        })
        .collect()
}

fn installed_package_closure(
    store: &Store,
    package: &Package,
    closure: &mut BTreeMap<String, Package>,
    visiting: &mut BTreeSet<String>,
) -> Result<(), String> {
    if closure.contains_key(&package.id) {
        return Ok(());
    }
    if !visiting.insert(package.id.clone()) {
        return Err(format!(
            "the installed native mods depend on each other in a circle at {}",
            package.id
        ));
    }
    for dependency in &package.depends {
        let installed = store
            .registry()
            .packages
            .get(&dependency.id)
            .ok_or_else(|| {
                format!(
                    "{} needs installed native mod {} {}, which is missing",
                    package.id, dependency.id, dependency.version
                )
            })?;
        let dependency_package = installed.package().ok_or_else(|| {
            format!(
                "{} needs installed native mod {} {}, which has no current version",
                package.id, dependency.id, dependency.version
            )
        })?;
        let requirement = semver::VersionReq::parse(&dependency.version).map_err(|_| {
            format!(
                "{} has an invalid version requirement for {}",
                package.id, dependency.id
            )
        })?;
        if !requirement.matches(&dependency_package.semver()) {
            return Err(format!(
                "{} needs native mod {} {}, but the installed version is {}",
                package.id, dependency.id, dependency.version, dependency_package.version
            ));
        }
        installed_package_closure(store, dependency_package, closure, visiting)?;
    }
    visiting.remove(&package.id);
    closure.insert(package.id.clone(), package.clone());
    Ok(())
}

/// Reviews a manual native-mod action without fetching package files or
/// mutating the package registry. `refresh_project` refreshes the signed
/// project index before resolving it.
pub fn plugin_consent_review(
    data_dir: &Path,
    setup: &NativeSetup,
    build: Option<&str>,
    server: Option<&str>,
    change: &Change,
    refresh_project: bool,
) -> Result<Vec<PluginConsentPackage>, String> {
    let mut store = store(data_dir)?;
    match change {
        Change::Install(id) => {
            let build =
                build.ok_or("start the game once, or name it, so the launcher knows its build")?;
            let wanted = [(id.as_str(), None)];
            let (index, source) = index_listing_with_source(
                &mut store,
                data_dir,
                setup,
                server,
                &wanted,
                refresh_project,
            )?;
            plugin_items_for_install(&store, &index, &source, build, id, None, &BTreeMap::new())
        }
        Change::InstallProposed(id) => {
            let build =
                build.ok_or("start the game once, or name it, so the launcher knows its build")?;
            let server = server.ok_or("connect to the server that proposed this native mod")?;
            let proposals =
                server_proposals(data_dir, server).ok_or("the server proposes no such mod now")?;
            let index = proposals.index();
            plugin_items_for_install(
                &store,
                &index,
                &server_source_id(server)?,
                build,
                id,
                None,
                &BTreeMap::new(),
            )
        }
        Change::Enable(id, true) => plugin_items_for_installed(&store, id, None),
        Change::Rollback(id) => {
            let previous = store
                .registry()
                .packages
                .get(id)
                .and_then(|installed| installed.previous.as_deref())
                .ok_or_else(|| format!("native mod {id} has no earlier version to go back to"))?;
            plugin_items_for_installed(&store, id, Some(previous))
        }
        Change::Refresh | Change::Uninstall(_) | Change::Enable(_, false) => Ok(Vec::new()),
    }
}

/// Reviews the exact package versions and settings the room requires,
/// including executable dependencies, without changing the package registry.
pub fn room_plugin_consent_review(
    data_dir: &Path,
    setup: &NativeSetup,
    build: &str,
    server: Option<&str>,
    room: &[NativeMod],
    refresh_project: bool,
) -> Result<Vec<PluginConsentPackage>, String> {
    if room.is_empty() {
        return Ok(Vec::new());
    }
    let mut store = store(data_dir)?;
    let wanted: Vec<(&str, Option<&str>)> = room
        .iter()
        .map(|native| (native.id.as_str(), Some(native.package_version())))
        .collect();
    let (index, source) = index_listing_with_source(
        &mut store,
        data_dir,
        setup,
        server,
        &wanted,
        refresh_project,
    )?;
    let mut room_settings = BTreeMap::new();
    for native in room {
        let package = index
            .package(native.id.as_str(), native.package_version())
            .filter(|package| package.runs_on(build))
            .ok_or_else(|| {
                format!(
                    "the signed index has no {} {} for your game's build",
                    native.name,
                    native.package_version()
                )
            })?;
        if !changes_simulation(package) {
            return Err(format!(
                "the room lists {} as simulation-changing, but its signed package does not change the simulation",
                native.name
            ));
        }
        let settings = effective_settings_from_room(package, Some(native))?;
        let expected = terms::NativeTerm {
            id: package.id.clone(),
            version: package.version.clone(),
            settings: terms::settings_digest(&settings),
            identity: terms::package_digest(package, &settings),
        }
        .mod_version();
        if native.version.as_str() != expected {
            return Err(format!(
                "the room's {} package or settings no longer match its signed identity; the room must update its native-mod terms",
                native.name
            ));
        }
        room_settings.insert(package.id.clone(), settings);
    }
    let mut found = BTreeMap::new();
    for native in room {
        for item in plugin_items_for_install(
            &store,
            &index,
            &source,
            build,
            native.id.as_str(),
            Some(native.package_version()),
            &room_settings,
        )? {
            found.insert(
                (item.id.clone(), item.version.clone(), item.source.clone()),
                item,
            );
        }
    }
    Ok(found.into_values().collect())
}

fn same_plugin_identities(a: &[PluginConsentPackage], b: &[PluginConsentPackage]) -> bool {
    let identity = |items: &[PluginConsentPackage]| {
        let mut identities: Vec<_> = items
            .iter()
            .map(|item| {
                (
                    item.id.clone(),
                    item.name.clone(),
                    item.version.clone(),
                    item.authors.clone(),
                    item.source.clone(),
                    item.digest.clone(),
                )
            })
            .collect();
        identities.sort_unstable();
        identities
    };
    identity(a) == identity(b)
}

/// Re-resolves an approved action against the current validated index and
/// records consent only when every reviewed source, package, and effective
/// setting is unchanged. A changed identity returns a replacement review.
pub(crate) fn approve_plugin_consent(
    data_dir: &Path,
    setup: &NativeSetup,
    build: Option<&str>,
    server: Option<&str>,
    operation: &PendingPluginOperation,
    expected: &[PluginConsentPackage],
) -> Result<Option<Vec<PluginConsentPackage>>, String> {
    let current = match operation {
        PendingPluginOperation::Change(change) => {
            plugin_consent_review(data_dir, setup, build, server, change, true)?
        }
        PendingPluginOperation::MatchRoom { room, .. } => room_plugin_consent_review(
            data_dir,
            setup,
            build.ok_or("start the game first so the launcher can verify its build")?,
            server,
            room,
            true,
        )?,
    };
    if !same_plugin_identities(expected, &current) {
        return Ok(Some(current));
    }
    let mut store = store(data_dir)?;
    for item in &current {
        store
            .record_consent(&item.source, &item.digest)
            .map_err(|error| format!("native plugin consent: {error}"))?;
    }
    Ok(None)
}

fn require_plugin_consent(items: &[PluginConsentPackage]) -> Result<(), String> {
    if items.iter().any(|item| !item.approved) {
        return Err(
            "native plugin code needs your explicit approval before this action can continue"
                .into(),
        );
    }
    Ok(())
}

/// Makes the next game on the build `build` run exactly the room's native
/// mods `room`: each checked against the signed index (including when the
/// id and version already match), installed if its files or metadata are
/// stale, given the room's settings, and enabled; every other
/// simulation-changing one is switched off. Ones that change only what a
/// player sees stay the player's. Returns whether anything changed, and so
/// whether a running game must start again to run them. Refused when the
/// index does not list the room's version for this build, or the result
/// does not match the room: then that player cannot join.
pub fn match_room(
    data_dir: &Path,
    setup: &NativeSetup,
    build: &str,
    server: Option<&str>,
    room: &[NativeMod],
) -> Result<bool, String> {
    let error = |error: StoreError| format!("native mods: {error}");
    require_plugin_consent(&room_plugin_consent_review(
        data_dir, setup, build, server, room, false,
    )?)?;
    let mut store = store(data_dir)?;
    let before = store.registry().clone();
    let wanted: Vec<(&str, Option<&str>)> = room
        .iter()
        .map(|m| (m.id.as_str(), Some(m.package_version())))
        .collect();
    if !room.is_empty() {
        let (index, source) =
            index_listing_with_source(&mut store, data_dir, setup, server, &wanted, false)?;
        let mut room_settings = BTreeMap::new();
        for native in room {
            let package = index
                .package(native.id.as_str(), native.package_version())
                .filter(|package| package.runs_on(build))
                .ok_or_else(|| {
                    format!(
                        "the signed index has no {} {} for your game's build",
                        native.name,
                        native.package_version()
                    )
                })?;
            room_settings.insert(
                package.id.clone(),
                effective_settings_from_room(package, Some(native))?,
            );
        }
        for native in room {
            let (id, version) = (native.id.as_str(), native.package_version());
            let package = index
                .package(id, version)
                .filter(|p| p.runs_on(build))
                .ok_or_else(|| {
                    format!(
                        "the signed index has no {} {version} for your game's build",
                        native.name
                    )
                })?;
            info!(%id, %version, "checking the room's native mod against its signed package");
            let packages = resolved_packages_for_install(&store, &index, build, id, Some(version))?;
            let settings = effective_settings_by_package(&store, &packages, &room_settings);
            store
                .install_exact_from(
                    resolve::Context {
                        index: &index,
                        build,
                        registry: BUILT_IN,
                    },
                    &package.id,
                    version,
                    &source,
                    &settings,
                    setup.source.as_ref(),
                )
                .map_err(error)?;
            // The room's settings, all of them: a told setting overrides the
            // default, and none of this player's own stays.
            store.reset_settings(id).map_err(error)?;
            for setting in &native.settings {
                store
                    .set_setting(id, setting.key.as_str(), from_value(&setting.value))
                    .map_err(error)?;
            }
            store.set_enabled(id, true).map_err(error)?;
        }
    }
    // Another simulation-changing mod would part this game from the room.
    let others: Vec<String> = store
        .registry()
        .packages
        .iter()
        .filter(|(id, installed)| {
            installed.enabled
                && !room.iter().any(|m| m.id.as_str() == id.as_str())
                && installed.package().is_some_and(changes_simulation)
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in others {
        info!(%id, "switching off a native mod the room does not run");
        store.set_enabled(&id, false).map_err(error)?;
    }
    let (list, _) = store.enabled_for(build).map_err(error)?;
    let ours = room_terms(&store, &list)?;
    let theirs: BTreeMap<&str, &str> = room
        .iter()
        .map(|m| (m.id.as_str(), m.version.as_str()))
        .collect();
    let mine: BTreeMap<&str, &str> = ours
        .iter()
        .map(|m| (m.id.as_str(), m.version.as_str()))
        .collect();
    if theirs != mine {
        return Err(
            "your native mods still differ from the room's after installing them: the room's settings could not be matched"
                .into(),
        );
    }
    Ok(store.registry() != &before)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use tpf3mp_nativemods::store::REGISTRY;

    #[test]
    fn nothing_installed_or_enabled_passes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("game.exe");
        fs::write(&exe, b"not really a game").unwrap();
        assert_eq!(game_env(dir.path(), &exe).unwrap(), GameNative::default());

        // Installed, not enabled.
        let root = dir.path().join(FOLDER);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join(REGISTRY),
            br#"{"format":1,"serial":3,"packages":{}}"#,
        )
        .unwrap();
        assert_eq!(game_env(dir.path(), &exe).unwrap(), GameNative::default());

        // Enabled for this build: the game is told where the list is.
        let package = tpf3mp_nativemods::index::Package {
            id: "pages".into(),
            version: "1.0.0".into(),
            name: "Pages".into(),
            description: String::new(),
            homepage: None,
            simulation: false,
            builds: vec![sha256_of_file(&exe).unwrap()],
            features: Vec::new(),
            settings: Default::default(),
            depends: Vec::new(),
            conflicts: Vec::new(),
            files: Vec::new(),
            plugins: Vec::new(),
            authors: Vec::new(),
            preview: None,
            hooks: Vec::new(),
            game_mods: Vec::new(),
        };
        let registry = tpf3mp_nativemods::store::Registry {
            format: 1,
            serial: 3,
            index_sha256: None,
            packages: [(
                "pages".to_owned(),
                tpf3mp_nativemods::store::Installed {
                    current: "1.0.0".into(),
                    previous: None,
                    enabled: true,
                    settings: Default::default(),
                    versions: [("1.0.0".to_owned(), package)].into(),
                },
            )]
            .into(),
        };
        fs::write(root.join(REGISTRY), serde_json::to_vec(&registry).unwrap()).unwrap();
        let (name, path) = game_env(dir.path(), &exe).unwrap().env.unwrap();
        assert_eq!(name, enabled::ENV);
        let list = enabled::read(Path::new(&path)).unwrap();
        assert_eq!(list.packages[0].id, "pages");

        // Another build of the game: the list is empty.
        fs::write(&exe, b"another build").unwrap();
        let (_, path) = game_env(dir.path(), &exe).unwrap().env.unwrap();
        assert!(enabled::read(Path::new(&path)).unwrap().packages.is_empty());

        // A registry that cannot be read stops the start.
        fs::write(root.join(REGISTRY), b"{ broken").unwrap();
        assert!(game_env(dir.path(), &exe).is_err());
    }

    #[test]
    fn an_enabled_simulation_mod_for_another_build_stops_the_game_start() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("game.exe");
        fs::write(&exe, b"the new build").unwrap();
        let root = dir.path().join(FOLDER);
        fs::create_dir_all(&root).unwrap();
        let package = tpf3mp_nativemods::index::Package {
            id: "bigmap".into(),
            version: "1.0.0".into(),
            name: "Big Maps".into(),
            description: String::new(),
            homepage: None,
            simulation: true,
            builds: vec!["0".repeat(64)],
            features: Vec::new(),
            settings: Default::default(),
            depends: Vec::new(),
            conflicts: Vec::new(),
            files: Vec::new(),
            plugins: Vec::new(),
            authors: Vec::new(),
            preview: None,
            hooks: Vec::new(),
            game_mods: Vec::new(),
        };
        let registry = tpf3mp_nativemods::store::Registry {
            format: 1,
            serial: 3,
            index_sha256: None,
            packages: [(
                "bigmap".to_owned(),
                tpf3mp_nativemods::store::Installed {
                    current: "1.0.0".into(),
                    previous: None,
                    enabled: true,
                    settings: Default::default(),
                    versions: [("1.0.0".to_owned(), package)].into(),
                },
            )]
            .into(),
        };
        fs::write(root.join(REGISTRY), serde_json::to_vec(&registry).unwrap()).unwrap();

        assert!(game_env(dir.path(), &exe).is_err());
        assert!(!root.join(enabled::FILE).exists());
    }

    #[test]
    fn startup_refuses_to_declare_a_signed_identity_after_a_package_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("game.exe");
        fs::write(&exe, b"the game").unwrap();
        let build = build_of(&exe).unwrap();
        let (mut package, (url, bytes)) = native_package("bigmap", "0.3.0", &build);
        package.files[0].path = "content/simulation.lua".into();
        let setup = setup(vec![(package, (url, bytes))]);

        apply(
            &dir.path().join("player"),
            &setup,
            Some(&build),
            None,
            Change::Refresh,
        )
        .unwrap();
        apply(
            &dir.path().join("player"),
            &setup,
            Some(&build),
            None,
            Change::Install("bigmap".into()),
        )
        .unwrap();
        apply(
            &dir.path().join("player"),
            &setup,
            Some(&build),
            None,
            Change::Enable("bigmap".into(), true),
        )
        .unwrap();

        let player = dir.path().join("player");
        assert_eq!(game_env(&player, &exe).unwrap().terms.len(), 1);
        fs::write(
            player
                .join(FOLDER)
                .join("bigmap/0.3.0/content/simulation.lua"),
            b"changed after installation",
        )
        .unwrap();

        let error = game_env(&player, &exe).unwrap_err();
        assert!(error.contains("content/simulation.lua"), "{error}");
        assert!(next_terms(&player, &exe).is_err());
    }

    /// A signed index and its files, held in memory, signed with a TEST
    /// key made for the test.
    struct Fake {
        json: Vec<u8>,
        signature: Vec<u8>,
        files: BTreeMap<String, Vec<u8>>,
    }

    const TEST_PLUGIN_BYTES: &[u8] = b"test plugin";

    impl Source for Fake {
        fn index(&self) -> Result<(Vec<u8>, Vec<u8>), tpf3mp_nativemods::fetch::FetchError> {
            Ok((self.json.clone(), self.signature.clone()))
        }

        fn download(
            &self,
            file: &tpf3mp_nativemods::index::FileEntry,
            path: &Path,
        ) -> Result<(), tpf3mp_nativemods::fetch::FetchError> {
            let bytes = self.files.get(&file.url).cloned().unwrap_or_default();
            tpf3mp_nativemods::fetch::copy_verified(
                &bytes[..],
                path,
                file.size,
                &file.sha256,
                |_| {},
            )
        }
    }

    fn native_package(id: &str, version: &str, build: &str) -> (Package, (String, Vec<u8>)) {
        let bytes = format!("{id} {version}").into_bytes();
        let url = format!("https://example.invalid/{id}/{version}/data.txt");
        let package = Package {
            id: id.into(),
            version: version.into(),
            name: format!("{id} mod"),
            description: String::new(),
            homepage: Some(format!("https://example.invalid/{id}")),
            simulation: true,
            builds: vec![build.into()],
            features: Vec::new(),
            settings: BTreeMap::from([
                ("depth".to_owned(), Setting::Int(10)),
                ("raster".to_owned(), Setting::Bool(true)),
            ]),
            depends: Vec::new(),
            conflicts: Vec::new(),
            files: vec![tpf3mp_nativemods::index::FileEntry {
                path: "data.txt".into(),
                url: url.clone(),
                size: bytes.len() as u64,
                sha256: tpf3mp_nativemods::signed::sha256_hex(&bytes),
            }],
            plugins: Vec::new(),
            authors: Vec::new(),
            preview: None,
            hooks: Vec::new(),
            game_mods: Vec::new(),
        };
        (package, (url, bytes))
    }

    fn setup(packages: Vec<(Package, (String, Vec<u8>))>) -> NativeSetup {
        setup_with_serial(packages, 1)
    }

    fn setup_with_serial(packages: Vec<(Package, (String, Vec<u8>))>, serial: u64) -> NativeSetup {
        use ring::signature::{Ed25519KeyPair, KeyPair};
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let index = Index {
            format: 1,
            serial,
            packages: packages.iter().map(|(p, _)| p.clone()).collect(),
            listings: Default::default(),
        };
        let json = serde_json::to_vec(&index).unwrap();
        let signature = pair.sign(&json).as_ref().to_vec();
        let mut files = BTreeMap::new();
        for (package, (url, bytes)) in &packages {
            files.insert(url.clone(), bytes.clone());
            for plugin in &package.plugins {
                files.insert(plugin.url.clone(), TEST_PLUGIN_BYTES.to_vec());
            }
        }
        NativeSetup {
            source: Arc::new(Fake {
                json,
                signature,
                files,
            }),
            keys: vec![pair.public_key().as_ref().to_vec()],
        }
    }

    #[cfg(all(windows, target_arch = "x86_64"))]
    fn with_plugin(mut package: Package) -> Package {
        package.authors = vec!["Native Mod Authors".into()];
        package.plugins.push(tpf3mp_nativemods::index::Plugin {
            path: format!("plugin/{}.dll", package.id),
            url: format!("https://example.invalid/{}/plugin.dll", package.id),
            size: TEST_PLUGIN_BYTES.len() as u64,
            sha256: tpf3mp_nativemods::signed::sha256_hex(TEST_PLUGIN_BYTES),
            abi: tpf3mp_nativemods::index::PLUGIN_ABI,
            platform: "windows-x86_64".into(),
        });
        package
    }

    #[test]
    fn server_consent_source_is_a_valid_canonical_server_address() {
        assert_eq!(
            server_source_id(" EXAMPLE.org:29470 ").unwrap(),
            "example.org:29470"
        );
        assert_eq!(
            server_source_id("[2001:0db8::1]:29470").unwrap(),
            "[2001:db8::1]:29470"
        );
        assert!(server_source_id("bad..host:29470").is_err());
        assert!(server_source_id("example.org:+29470").is_err());
    }

    #[test]
    fn offline_data_only_install_can_resolve_a_cached_server_index() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("game.exe");
        fs::write(&exe, b"the game").unwrap();
        let build = build_of(&exe).unwrap();
        let (package, _) = native_package("cached", "1.0.0", &build);
        let json = serde_json::to_vec(&Index {
            format: 1,
            serial: 1,
            packages: vec![package],
            listings: Default::default(),
        })
        .unwrap();
        let server = "eu.example.org:29470";
        accept_server_index(dir.path(), server, 1, &json, false).unwrap();
        let setup = setup(Vec::new());

        assert!(
            plugin_consent_review(
                dir.path(),
                &setup,
                Some(&build),
                Some(server),
                &Change::Install("cached".into()),
                false,
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            plugin_consent_review(
                dir.path(),
                &setup,
                Some(&build),
                None,
                &Change::Install("cached".into()),
                false,
            )
            .is_err()
        );
    }

    /// A member joining a room gets the room's version from the signed
    /// index, with the room's settings, and leaves off a simulation mod the
    /// room does not run; then their terms are the room's.
    #[test]
    fn a_member_gets_the_rooms_native_mods_from_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("game.exe");
        fs::write(&exe, b"the game").unwrap();
        let build = build_of(&exe).unwrap();
        let setup = setup(vec![
            native_package("bigmap", "0.3.0", &build),
            native_package("bigmap", "0.4.0", &build),
            native_package("other", "1.0.0", &build),
        ]);

        // The owner: Big Maps 0.3.0, deeper than the default.
        let owner = dir.path().join("owner");
        apply(&owner, &setup, Some(&build), None, Change::Refresh).unwrap();
        let mut store = Store::open(owner.join(FOLDER)).unwrap();
        let index = store.cached_index(&setup.keys).unwrap();
        let context = resolve::Context {
            index: &index,
            build: &build,
            registry: BUILT_IN,
        };
        store
            .install_exact(context, "bigmap", "0.3.0", setup.source.as_ref())
            .unwrap();
        store.set_enabled("bigmap", true).unwrap();
        store
            .set_setting("bigmap", "depth", Setting::Int(12))
            .unwrap();
        let room = next_terms(&owner, &exe).unwrap();
        assert_eq!(room.len(), 1);
        assert_eq!(room[0].package_version(), "0.3.0");
        assert_eq!(room[0].name.as_str(), "bigmap mod");

        // A member with the newest version and another simulation mod.
        let member = dir.path().join("member");
        apply(
            &member,
            &setup,
            Some(&build),
            None,
            Change::Install("bigmap".into()),
        )
        .unwrap();
        apply(
            &member,
            &setup,
            Some(&build),
            None,
            Change::Install("other".into()),
        )
        .unwrap();
        apply(
            &member,
            &setup,
            Some(&build),
            None,
            Change::Enable("other".into(), true),
        )
        .unwrap();
        assert_ne!(next_terms(&member, &exe).unwrap(), room);

        assert!(match_room(&member, &setup, &build, None, &room).unwrap());
        let theirs = next_terms(&member, &exe).unwrap();
        assert_eq!(theirs, room, "the same version, settings and nothing else");
        let row = view(&member, Some(&setup), Some(&build), None, false)
            .packages
            .into_iter()
            .find(|row| row.id == "other")
            .unwrap();
        assert!(!row.enabled, "the room does not run it");
        // Matched already: nothing changes, and the game need not restart.
        assert!(!match_room(&member, &setup, &build, None, &room).unwrap());

        // A version the index does not have for this build: refused.
        let mut gone = room.clone();
        gone[0].version = Text::lossy("9.9.9+0000000000");
        assert!(match_room(&member, &setup, &build, None, &gone).is_err());

        // A room without native mods: the member's are switched off.
        assert!(match_room(&member, &setup, &build, None, &[]).unwrap());
        assert!(next_terms(&member, &exe).unwrap().is_empty());
    }

    #[test]
    fn a_room_with_a_changed_full_identity_is_refused_before_matching_mutates_packages() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("game.exe");
        fs::write(&exe, b"the game").unwrap();
        let build = build_of(&exe).unwrap();
        let setup = setup(vec![
            native_package("bigmap", "0.3.0", &build),
            native_package("bigmap", "0.4.0", &build),
        ]);
        let owner = dir.path().join("owner");
        apply(&owner, &setup, Some(&build), None, Change::Refresh).unwrap();
        // Install the older version as the owner's intended room package.
        let mut owner_store = Store::open(owner.join(FOLDER)).unwrap();
        let index = owner_store.cached_index(&setup.keys).unwrap();
        owner_store
            .install_exact(
                resolve::Context {
                    index: &index,
                    build: &build,
                    registry: BUILT_IN,
                },
                "bigmap",
                "0.3.0",
                setup.source.as_ref(),
            )
            .unwrap();
        owner_store
            .set_setting("bigmap", "depth", Setting::Int(12))
            .unwrap();
        owner_store.set_enabled("bigmap", true).unwrap();
        drop(owner_store);
        let mut room = next_terms(&owner, &exe).unwrap();
        assert_eq!(room[0].package_version(), "0.3.0");

        let member = dir.path().join("member");
        apply(
            &member,
            &setup,
            Some(&build),
            None,
            Change::Install("bigmap".into()),
        )
        .unwrap();
        // The member keeps its current 0.4.0 package and customized setting.
        let mut member_store = Store::open(member.join(FOLDER)).unwrap();
        member_store
            .set_setting("bigmap", "depth", Setting::Int(18))
            .unwrap();
        member_store.set_enabled("bigmap", true).unwrap();
        drop(member_store);

        let before = next_terms(&member, &exe).unwrap();
        let mut changed_identity = room[0].version.as_str().to_owned();
        let last = changed_identity.pop().unwrap();
        changed_identity.push(if last == '0' { '1' } else { '0' });
        room[0].version = Text::lossy(&changed_identity);
        let error = match_room(&member, &setup, &build, None, &room).unwrap_err();
        assert!(error.contains("signed identity"), "{error}");
        assert_eq!(next_terms(&member, &exe).unwrap(), before);
        let member_store = Store::open(member.join(FOLDER)).unwrap();
        let installed = &member_store.registry().packages["bigmap"];
        assert_eq!(installed.current, "0.4.0");
        assert!(installed.enabled);
        assert_eq!(installed.settings["depth"], Setting::Int(18));
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[test]
    fn an_unapproved_transitive_plugin_is_reviewed_and_blocks_install_before_payloads() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("game.exe");
        fs::write(&exe, b"the game").unwrap();
        let build = build_of(&exe).unwrap();
        let (mut root, root_file) = native_package("modroot", "1.0.0", &build);
        root.depends.push(tpf3mp_nativemods::index::Dependency {
            id: "engine".into(),
            version: "^1.0".into(),
        });
        let (engine, engine_file) = native_package("engine", "1.2.0", &build);
        let setup = setup(vec![(with_plugin(engine), engine_file), (root, root_file)]);
        let player = dir.path().join("player");
        let review = plugin_consent_review(
            &player,
            &setup,
            Some(&build),
            None,
            &Change::Install("modroot".into()),
            false,
        )
        .unwrap();
        assert_eq!(review.len(), 1);
        assert_eq!(review[0].id, "engine");
        assert_eq!(review[0].source, "project");
        assert_eq!(review[0].digest.len(), 64);
        assert!(!review[0].approved);

        let error = apply(
            &player,
            &setup,
            Some(&build),
            None,
            Change::Install("modroot".into()),
        )
        .unwrap_err();
        assert!(error.contains("explicit approval"), "{error}");
        assert!(!player.join(FOLDER).join("modroot").exists());
        assert!(!player.join(FOLDER).join("engine").exists());
        let store = Store::open(player.join(FOLDER)).unwrap();
        assert!(store.registry().packages.is_empty());
        assert!(!store.has_consent("project", &review[0].digest).unwrap());

        let changed = approve_plugin_consent(
            &player,
            &setup,
            Some(&build),
            None,
            &PendingPluginOperation::Change(Change::Install("modroot".into())),
            &review,
        )
        .unwrap();
        assert!(changed.is_none());
        assert!(
            Store::open(player.join(FOLDER))
                .unwrap()
                .has_consent("project", &review[0].digest)
                .unwrap()
        );
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[test]
    fn room_plugin_dependency_uses_its_room_settings_before_earlier_root_install() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("game.exe");
        fs::write(&exe, b"the game").unwrap();
        let build = build_of(&exe).unwrap();
        let (mut root, root_file) = native_package("a_root", "1.0.0", &build);
        root.depends.push(tpf3mp_nativemods::index::Dependency {
            id: "z_engine".into(),
            version: "^1.0".into(),
        });
        let (engine, engine_file) = native_package("z_engine", "1.0.0", &build);
        let engine = with_plugin(engine);
        let setup = setup(vec![(root, root_file), (engine.clone(), engine_file)]);

        let owner = dir.path().join("owner");
        apply(&owner, &setup, Some(&build), None, Change::Refresh).unwrap();
        let mut owner_store = Store::open(owner.join(FOLDER)).unwrap();
        let index = owner_store.cached_index(&setup.keys).unwrap();
        let packages =
            resolved_packages_for_install(&owner_store, &index, &build, "a_root", Some("1.0.0"))
                .unwrap();
        let defaults = effective_settings_by_package(&owner_store, &packages, &BTreeMap::new());
        let default_digest = terms::package_digest(&engine, &defaults["z_engine"]);
        owner_store
            .record_consent("project", &default_digest)
            .unwrap();
        owner_store
            .install_exact_from(
                resolve::Context {
                    index: &index,
                    build: &build,
                    registry: BUILT_IN,
                },
                "a_root",
                "1.0.0",
                "project",
                &defaults,
                setup.source.as_ref(),
            )
            .unwrap();
        owner_store
            .set_setting("z_engine", "depth", Setting::Int(12))
            .unwrap();
        let room_engine_settings = BTreeMap::from([
            ("depth".to_owned(), Setting::Int(12)),
            ("raster".to_owned(), Setting::Bool(true)),
        ]);
        let room_engine_digest = terms::package_digest(&engine, &room_engine_settings);
        owner_store
            .record_consent("project", &room_engine_digest)
            .unwrap();
        owner_store.set_enabled("a_root", true).unwrap();
        owner_store.set_enabled("z_engine", true).unwrap();
        drop(owner_store);
        let room = next_terms(&owner, &exe).unwrap();
        assert_eq!(
            room.iter()
                .map(|native| native.id.as_str())
                .collect::<Vec<_>>(),
            ["a_root", "z_engine"],
            "the room root is matched before its plugin dependency"
        );

        let member = dir.path().join("member");
        apply(&member, &setup, Some(&build), None, Change::Refresh).unwrap();
        let mut member_store = Store::open(member.join(FOLDER)).unwrap();
        let index = member_store.cached_index(&setup.keys).unwrap();
        let packages =
            resolved_packages_for_install(&member_store, &index, &build, "a_root", Some("1.0.0"))
                .unwrap();
        let defaults = effective_settings_by_package(&member_store, &packages, &BTreeMap::new());
        let default_digest = terms::package_digest(&engine, &defaults["z_engine"]);
        member_store
            .record_consent("project", &default_digest)
            .unwrap();
        member_store
            .install_exact_from(
                resolve::Context {
                    index: &index,
                    build: &build,
                    registry: BUILT_IN,
                },
                "a_root",
                "1.0.0",
                "project",
                &defaults,
                setup.source.as_ref(),
            )
            .unwrap();
        drop(member_store);

        // Model an existing installation from before explicit consent was
        // required: it has trusted provenance but no saved approval yet.
        fs::write(
            member.join(FOLDER).join("consents.json"),
            br#"{"format":1,"by_source":{}}"#,
        )
        .unwrap();
        let review =
            room_plugin_consent_review(&member, &setup, &build, None, &room, false).unwrap();
        assert_eq!(review.len(), 1);
        assert_eq!(review[0].id, "z_engine");
        assert_eq!(review[0].digest, room_engine_digest);
        assert!(!review[0].approved);
        assert!(
            approve_plugin_consent(
                &member,
                &setup,
                Some(&build),
                None,
                &PendingPluginOperation::MatchRoom {
                    room: room.clone(),
                    resume_join: false,
                },
                &review,
            )
            .unwrap()
            .is_none()
        );

        assert!(match_room(&member, &setup, &build, None, &room).unwrap());
        assert_eq!(next_terms(&member, &exe).unwrap(), room);
        let member_store = Store::open(member.join(FOLDER)).unwrap();
        assert_eq!(
            member_store.registry().packages["z_engine"].settings["depth"],
            Setting::Int(12)
        );
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[test]
    fn changed_plugin_identity_requires_a_new_review_before_consent_is_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("game.exe");
        fs::write(&exe, b"the game").unwrap();
        let build = build_of(&exe).unwrap();
        let (package, file) = native_package("pluginmod", "1.0.0", &build);
        let original = setup(vec![(with_plugin(package.clone()), file.clone())]);
        let player = dir.path().join("player");
        let review = plugin_consent_review(
            &player,
            &original,
            Some(&build),
            None,
            &Change::Install("pluginmod".into()),
            false,
        )
        .unwrap();
        let mut changed_package = with_plugin(package);
        changed_package.plugins[0].sha256 = "f".repeat(64);
        let changed = setup_with_serial(vec![(changed_package, file)], 2);
        let replacement = approve_plugin_consent(
            &player,
            &changed,
            Some(&build),
            None,
            &PendingPluginOperation::Change(Change::Install("pluginmod".into())),
            &review,
        )
        .unwrap()
        .unwrap();
        assert_eq!(replacement.len(), 1);
        assert_ne!(replacement[0].digest, review[0].digest);
        assert!(!replacement[0].approved);
        let store = Store::open(player.join(FOLDER)).unwrap();
        assert!(!store.has_consent("project", &review[0].digest).unwrap());
        assert!(
            !store
                .has_consent("project", &replacement[0].digest)
                .unwrap()
        );
        assert!(store.registry().packages.is_empty());
    }

    #[test]
    fn a_same_version_package_with_changed_simulation_lua_is_replaced_before_join() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("game.exe");
        fs::write(&exe, b"the game").unwrap();
        let build = build_of(&exe).unwrap();
        let server = "eu.example:29470";
        let mut old = native_package("bigmap", "0.3.0", &build).0;
        old.files[0].path = "content/simulation.lua".into();
        let old_url = old.files[0].url.clone();
        let old_bytes = b"return 1".to_vec();
        old.files[0].size = old_bytes.len() as u64;
        old.files[0].sha256 = tpf3mp_nativemods::signed::sha256_hex(&old_bytes);
        let mut old_setup = setup(vec![(old.clone(), (old_url.clone(), old_bytes.clone()))]);
        old_setup.keys.clear();

        let old_index = Index {
            format: 1,
            serial: 1,
            packages: vec![old.clone()],
            listings: Default::default(),
        };
        let old_json = serde_json::to_vec(&old_index).unwrap();
        let owner = dir.path().join("owner");
        let member = dir.path().join("member");
        accept_server_index(&owner, server, 1, &old_json, false).unwrap();
        accept_server_index(&member, server, 1, &old_json, false).unwrap();
        for player in [&owner, &member] {
            apply(
                player,
                &old_setup,
                Some(&build),
                Some(server),
                Change::Install("bigmap".into()),
            )
            .unwrap();
            apply(
                player,
                &old_setup,
                Some(&build),
                Some(server),
                Change::Enable("bigmap".into(), true),
            )
            .unwrap();
        }
        let old_terms = next_terms(&owner, &exe).unwrap();
        assert_eq!(next_terms(&member, &exe).unwrap(), old_terms);

        let new_bytes = b"return 2".to_vec();
        let mut updated = old.clone();
        updated.files[0].size = new_bytes.len() as u64;
        updated.files[0].sha256 = tpf3mp_nativemods::signed::sha256_hex(&new_bytes);
        let mut updated_setup = setup(vec![(
            updated.clone(),
            (old_url.clone(), new_bytes.clone()),
        )]);
        updated_setup.keys.clear();
        let updated_index = Index {
            format: 1,
            serial: 2,
            packages: vec![updated],
            listings: Default::default(),
        };
        let updated_json = serde_json::to_vec(&updated_index).unwrap();
        accept_server_index(&owner, server, 2, &updated_json, false).unwrap();
        accept_server_index(&member, server, 2, &updated_json, false).unwrap();

        // The owner installs the refreshed same-version package. Settings
        // and the version stay fixed; only the simulation Lua bytes change.
        apply(
            &owner,
            &updated_setup,
            Some(&build),
            Some(server),
            Change::Install("bigmap".into()),
        )
        .unwrap();
        let room_terms = next_terms(&owner, &exe).unwrap();
        assert_eq!(room_terms[0].package_version(), "0.3.0");
        assert_ne!(room_terms[0].version, old_terms[0].version);
        assert_eq!(
            &room_terms[0].version.as_str().rsplit_once('+').unwrap().1[..10],
            &old_terms[0].version.as_str().rsplit_once('+').unwrap().1[..10],
            "the settings digest remains the same"
        );

        assert!(match_room(&member, &updated_setup, &build, Some(server), &room_terms,).unwrap());
        assert_eq!(next_terms(&member, &exe).unwrap(), room_terms);
        assert_eq!(
            fs::read(
                member
                    .join(FOLDER)
                    .join("bigmap")
                    .join("0.3.0")
                    .join("content/simulation.lua")
            )
            .unwrap(),
            new_bytes
        );
    }

    #[test]
    fn settings_travel_as_the_room_tells_them() {
        for setting in [
            Setting::Bool(true),
            Setting::Int(-7),
            Setting::Text("north".into()),
        ] {
            assert_eq!(from_value(&to_value(&setting).unwrap()), setting);
        }
        assert!(to_value(&Setting::Text("x".repeat(200))).is_err());
    }

    #[test]
    fn without_the_key_the_panel_offers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            view(dir.path(), None, None, None, false),
            NativeView::default()
        );
        let mut keyless = setup(Vec::new());
        keyless.keys.clear();
        assert!(!view(dir.path(), Some(&keyless), None, None, false).available);
        let shown = view(dir.path(), Some(&setup(Vec::new())), None, None, false);
        assert!(shown.available);
        assert_eq!(shown.serial, None, "no index fetched yet");
    }

    /// A server's own index needs no key: the launcher takes it from the
    /// server, installs the room's mods from it, and shows the mods its
    /// registry's open pull requests propose only when asked.
    #[test]
    fn a_servers_own_index_and_its_proposals() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("game.exe");
        fs::write(&exe, b"the game build two").unwrap();
        let build = build_of(&exe).unwrap();
        let big = native_package("bigmap", "0.3.0", &build);
        let trees = native_package("trees", "1.0.0", &build);
        let mut keyless = setup(vec![big.clone(), trees.clone()]);
        keyless.keys.clear();
        let server = "eu.example:29470";
        let mut shown_big = big.0.clone();
        shown_big.authors = vec!["Ann".into()];
        shown_big.preview = Some("https://example.invalid/bigmap.png".into());
        let index = Index {
            format: 1,
            serial: 5,
            packages: vec![shown_big],
            listings: [(
                "bigmap".to_owned(),
                tpf3mp_nativemods::index::Listing {
                    downloads: Some(1234),
                    updated: Some("2026-10-01".into()),
                },
            )]
            .into(),
        };
        let json = serde_json::to_vec(&index).unwrap();
        let player = dir.path().join("player");
        assert!(!view(&player, Some(&keyless), Some(&build), Some(server), false).available);
        accept_server_index(&player, server, 5, &json, false).unwrap();
        accept_server_index(&player, server, 5, &json, false).unwrap();
        let mut same_serial_changed = index.clone();
        same_serial_changed.packages[0].description = "different bytes".into();
        assert!(
            accept_server_index(
                &player,
                server,
                5,
                &serde_json::to_vec(&same_serial_changed).unwrap(),
                false,
            )
            .is_err()
        );
        // Another serial than it says, or an older index: refused.
        assert!(accept_server_index(&player, server, 6, &json, false).is_err());
        let older = Index {
            serial: 4,
            ..index.clone()
        };
        assert!(
            accept_server_index(
                &player,
                server,
                4,
                &serde_json::to_vec(&older).unwrap(),
                false
            )
            .is_err()
        );

        let shown = view(&player, Some(&keyless), Some(&build), Some(server), false);
        assert!(shown.available);
        assert_eq!(shown.server.as_deref(), Some(server));
        assert_eq!(shown.packages.len(), 1);
        // What the browser shows of it: who made it, an image, its size,
        // and what its host says.
        let row = &shown.packages[0];
        assert_eq!(row.authors, ["Ann"]);
        assert_eq!(
            row.preview.as_deref(),
            Some("https://example.invalid/bigmap.png")
        );
        assert_eq!(row.available_size, Some(b"bigmap 0.3.0".len() as u64));
        assert_eq!(
            (row.downloads, row.updated.as_deref()),
            (Some(1234), Some("2026-10-01"))
        );
        assert_eq!((row.installed_size, row.load_order), (None, None));
        // Another server's index is not this one's.
        assert!(
            !view(
                &player,
                Some(&keyless),
                Some(&build),
                Some("other:1"),
                false
            )
            .available
        );

        // A room's mod, from the server's index.
        // The digest is the defaults': match it as the owner would.
        let owner = dir.path().join("owner");
        accept_server_index(&owner, server, 5, &json, false).unwrap();
        apply(
            &owner,
            &keyless,
            Some(&build),
            Some(server),
            Change::Install("bigmap".into()),
        )
        .unwrap();
        apply(
            &owner,
            &keyless,
            Some(&build),
            Some(server),
            Change::Enable("bigmap".into(), true),
        )
        .unwrap();
        let owners = next_terms(&owner, &exe).unwrap();
        assert_eq!(owners[0].package_version(), "0.3.0");
        let installed = view(&owner, Some(&keyless), Some(&build), Some(server), false);
        assert_eq!(
            (
                installed.packages[0].installed_size,
                installed.packages[0].load_order
            ),
            (Some(b"bigmap 0.3.0".len() as u64), Some(1)),
            "the first, and only, native mod the game loads"
        );
        assert!(match_room(&player, &keyless, &build, Some(server), &owners).unwrap());
        assert_eq!(next_terms(&player, &exe).unwrap(), owners);

        // Proposals: shown only when asked, marked with their pull request.
        let proposals = Proposals {
            format: 1,
            serial: 5,
            packages: vec![trees.0.clone()],
            pulls: [(
                "trees".to_owned(),
                Proposal {
                    number: 12,
                    title: "Add Trees".into(),
                    url: "https://github.com/r/reg/pull/12".into(),
                },
            )]
            .into(),
        };
        accept_server_index(
            &player,
            server,
            5,
            &serde_json::to_vec(&proposals).unwrap(),
            true,
        )
        .unwrap();
        let mut same_serial_proposals_changed = proposals.clone();
        same_serial_proposals_changed
            .pulls
            .get_mut("trees")
            .unwrap()
            .title = "Different title".into();
        assert!(
            accept_server_index(
                &player,
                server,
                5,
                &serde_json::to_vec(&same_serial_proposals_changed).unwrap(),
                true,
            )
            .is_err()
        );
        let hidden = view(&player, Some(&keyless), Some(&build), Some(server), false);
        assert!(hidden.packages.iter().all(|row| row.id != "trees"));
        let shown = view(&player, Some(&keyless), Some(&build), Some(server), true);
        let row = shown.packages.iter().find(|row| row.id == "trees").unwrap();
        assert_eq!(row.proposed.as_ref().unwrap().number, 12);
        // Never from the index proper; only when the player chooses it.
        assert!(
            apply(
                &player,
                &keyless,
                Some(&build),
                Some(server),
                Change::Install("trees".into())
            )
            .is_err()
        );
        apply(
            &player,
            &keyless,
            Some(&build),
            Some(server),
            Change::InstallProposed("trees".into()),
        )
        .unwrap();
    }
}
