//! Native mods on disk: the launcher's own folder for them, and the
//! registry of every file it put there.
//!
//! ```text
//! <launcher data>/native-mods/
//!   native-mods.json, native-mods.json.sig   the last index accepted
//!   registry.json                            every installed package and file
//!   package-sources.json                     trusted source per installed version
//!   consents.json                            exact plugin-identity approvals
//!   enabled.json                             what the next game runs (crate::enabled)
//!   <id>/<version>/…                         a package's files
//!   <id>/.partial-<version>/                 a download in progress
//! ```
//!
//! Nothing goes into the game's folder. An install downloads every file
//! into the package's `.partial-` folder, checking each against the signed
//! index's size and SHA-256, and only when all of them check out renames
//! the folder into place and records it: a download cut short or tampered
//! with leaves the installed version as it was. An upgrade keeps the
//! version before it, for [`Store::rollback`], and deletes the one before
//! that. Uninstalling deletes exactly the files the registry names, then
//! the folders they leave empty; a file someone else put there stays.
//!
//! One launcher at a time uses the folder (the launcher's instance lock,
//! `tpf3mp-agent`'s `launcher::instance`).

#[cfg(feature = "install")]
use std::time::Duration;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    enabled::{self, Enabled, EnabledPackage},
    features::{self, Effect},
    fetch::{self, FetchError},
    index::{self, FileEntry, Index, IndexError, Package, Setting},
    resolve::{self, ResolveError},
    signed,
};

/// The registry's file name.
pub const REGISTRY: &str = "registry.json";
const REGISTRY_FORMAT: u32 = 2;
const PREVIOUS_REGISTRY_FORMAT: u32 = 1;
const PARTIAL: &str = ".partial-";
const PACKAGE_SOURCES: &str = "package-sources.json";
const PACKAGE_SOURCES_FORMAT: u32 = 1;
const CONSENTS: &str = "consents.json";
const CONSENTS_FORMAT: u32 = 1;

/// Per-package effective settings supplied to a source-aware install. Each
/// plugin-bearing package in the resolved closure must have an entry.
pub type EffectiveSettingsByPackage = BTreeMap<String, BTreeMap<String, Setting>>;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageSources {
    format: u32,
    packages: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsentRecords {
    format: u32,
    by_source: BTreeMap<String, BTreeSet<String>>,
}

/// Where packages come from.
pub trait Source {
    /// The index and its signature.
    fn index(&self) -> Result<(Vec<u8>, Vec<u8>), FetchError>;
    /// Downloads `file` into `path`, checked against its size and SHA-256
    /// on the way ([`fetch::copy_verified`]).
    fn download(&self, file: &FileEntry, path: &Path) -> Result<(), FetchError>;
}

/// The index and packages over HTTPS (the `install` feature): `<base>/native-mods.json` and its
/// signature, and each file at the address the signed index gives.
#[cfg(feature = "install")]
pub struct Http {
    base: String,
    https_only: bool,
    user_agent: String,
}

#[cfg(feature = "install")]
impl Http {
    pub fn new(base: impl Into<String>, user_agent: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_owned(),
            https_only: true,
            user_agent: user_agent.into(),
        }
    }

    /// The project's index on GitHub: assets of the `native-mods` release
    /// of `repository` (`owner/name`).
    pub fn github(repository: &str, user_agent: impl Into<String>) -> Self {
        Self::new(
            format!("https://github.com/{repository}/releases/download/native-mods"),
            user_agent,
        )
    }

    /// Allows plain HTTP, for tests on loopback.
    pub fn allow_http(mut self) -> Self {
        self.https_only = false;
        self
    }
}

#[cfg(feature = "install")]
impl Source for Http {
    fn index(&self) -> Result<(Vec<u8>, Vec<u8>), FetchError> {
        let agent = fetch::agent(self.https_only, Duration::from_secs(60));
        let json = fetch::get(
            &agent,
            &format!("{}/{}", self.base, index::INDEX_FILE),
            index::MAX_INDEX,
            &self.user_agent,
        )?;
        let signature = fetch::get(
            &agent,
            &format!("{}/{}", self.base, index::SIGNATURE_FILE),
            256,
            &self.user_agent,
        )?;
        Ok((json, signature))
    }

    fn download(&self, file: &FileEntry, path: &Path) -> Result<(), FetchError> {
        let agent = fetch::agent(self.https_only, Duration::from_secs(3 * 60 * 60));
        fetch::download(
            &agent,
            &file.url,
            path,
            file.size,
            &file.sha256,
            &self.user_agent,
            |_| {},
        )
    }
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Fetch(#[from] FetchError),
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    #[error(
        "the native-mods index offered ({offered}) is older than one already accepted ({seen})"
    )]
    OlderIndex { offered: u64, seen: u64 },
    #[error("the native-mods index reuses serial {serial} for different contents")]
    SerialConflict { serial: u64 },
    #[error("the accepted native-mods index at serial {serial} cannot be bound to its contents")]
    UnboundSerial { serial: u64 },
    #[error("no native-mods index has been accepted yet")]
    NoIndex,
    #[error("the native-mods registry cannot be read: {0}")]
    Registry(String),
    #[error("the native-mod consent or source records cannot be read: {0}")]
    ConsentStore(String),
    #[error("the native package source is not canonical: {0}")]
    InvalidSource(String),
    #[error("plugin package {id} {version} has no trusted source recorded")]
    MissingSource { id: String, version: String },
    #[error("plugin package {id} {version} needs approval from {source_id} for {digest}")]
    ConsentRequired {
        id: String,
        version: String,
        source_id: String,
        digest: String,
    },
    #[error("effective settings for plugin package {0} were not supplied")]
    MissingEffectiveSettings(String),
    #[error("plugin package {id} cannot be loaded on this target ({platform})")]
    UnsupportedPluginPlatform { id: String, platform: String },
    #[error("the native mod {0} is not installed")]
    NotInstalled(String),
    #[error("the native mod {0} has no earlier version to go back to")]
    NoPrevious(String),
    #[error("files of {id} {version} are missing or changed: {}", .files.join(", "))]
    Damaged {
        id: String,
        version: String,
        files: Vec<String>,
    },
    #[error("{0} holds files the registry does not name; move them away first")]
    Occupied(PathBuf),
    #[error("{by} needs {id}")]
    Needed { id: String, by: String },
    #[error("{0}")]
    Setting(String),
    #[error("native-mod path is unsafe or crosses a symbolic link: {0}")]
    UnsafePath(PathBuf),
    #[error("{id} cannot be switched on: {why}")]
    Clash { id: String, why: String },
    #[error("simulation-changing native mod {id} is not pinned to this game build ({build})")]
    SimulationPackageNotForBuild { id: String, build: String },
}

/// Whether a failed current-version change already made it to registry.json.
/// Install replacement must keep the new folder when only old-version
/// cleanup failed after that durable write.
struct MakeCurrentError {
    source: StoreError,
    saved: bool,
}

impl From<StoreError> for MakeCurrentError {
    fn from(source: StoreError) -> Self {
        Self {
            source,
            saved: false,
        }
    }
}

impl From<MakeCurrentError> for StoreError {
    fn from(error: MakeCurrentError) -> Self {
        error.source
    }
}

fn ensure_plugin_platform_supported(
    package: &Package,
    platform: Option<&str>,
) -> Result<(), StoreError> {
    if package.plugins.is_empty() {
        return Ok(());
    }
    let plugins = platform
        .map(|platform| package.plugins_for(platform))
        .unwrap_or_default();
    if plugins.is_empty() || plugins.iter().any(|plugin| plugin.abi != index::PLUGIN_ABI) {
        return Err(StoreError::UnsupportedPluginPlatform {
            id: package.id.clone(),
            platform: platform
                .unwrap_or("unsupported operating system or architecture")
                .to_owned(),
        });
    }
    Ok(())
}

/// Every installed package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub format: u32,
    /// The highest index serial accepted.
    pub serial: u64,
    /// SHA-256 of the exact accepted index bytes, binding each serial to one
    /// immutable index even if the cache files are lost or a write is cut short.
    #[serde(default)]
    pub index_sha256: Option<String>,
    pub packages: BTreeMap<String, Installed>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            format: REGISTRY_FORMAT,
            serial: 0,
            index_sha256: None,
            packages: BTreeMap::new(),
        }
    }
}

/// One installed package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Installed {
    /// The version in use.
    pub current: String,
    /// The version before it, kept for a rollback.
    pub previous: Option<String>,
    /// Whether the player switched it on; installing does not.
    pub enabled: bool,
    /// The player's settings, over the package's defaults.
    pub settings: BTreeMap<String, Setting>,
    /// Each version on disk, as the signed index described it: its files
    /// are exactly these.
    pub versions: BTreeMap<String, Package>,
}

impl Installed {
    /// The package in use.
    pub fn package(&self) -> Option<&Package> {
        self.versions.get(&self.current)
    }
}

/// The native-mods folder and its registry.
pub struct Store {
    root: PathBuf,
    registry: Registry,
    sources: PackageSources,
    consents: ConsentRecords,
}

impl Store {
    /// The store in `root` (`<launcher data>/native-mods`). A registry that
    /// cannot be read is an error, never replaced by an empty one.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        ensure_no_symlink_path(&root, &root.join(REGISTRY))?;
        let registry = match fs::read(root.join(REGISTRY)) {
            Ok(bytes) => {
                let mut registry: Registry = serde_json::from_slice(&bytes)
                    .map_err(|error| StoreError::Registry(error.to_string()))?;
                if registry.format == PREVIOUS_REGISTRY_FORMAT {
                    registry.format = REGISTRY_FORMAT;
                } else if registry.format != REGISTRY_FORMAT {
                    return Err(StoreError::Registry(format!("format {}", registry.format)));
                }
                validate_registry(&registry)?;
                registry
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Registry::default(),
            Err(error) => return Err(error.into()),
        };
        let sources = read_package_sources(&root)?;
        let consents = read_consents(&root)?;
        Ok(Self {
            root,
            registry,
            sources,
            consents,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Trusted source recorded for one installed package version. Source
    /// IDs are `project` or a canonical lowercase `host:port` from the
    /// authenticated room connection.
    pub fn source_for(&self, id: &str, version: &str) -> Option<&str> {
        if !self
            .registry
            .packages
            .get(id)
            .is_some_and(|installed| installed.versions.contains_key(version))
        {
            return None;
        }
        self.sources
            .packages
            .get(id)?
            .get(version)
            .map(String::as_str)
    }

    /// Whether this trusted source approved the exact canonical package
    /// identity. The digest includes declared metadata, all platform plugin
    /// descriptors, hooks and effective settings.
    pub fn has_consent(&self, source_id: &str, digest: &str) -> Result<bool, StoreError> {
        validate_source_id(source_id)?;
        validate_digest(digest)?;
        Ok(self
            .consents
            .by_source
            .get(source_id)
            .is_some_and(|digests| digests.contains(digest)))
    }

    /// Records approval for one exact source and canonical package
    /// identity. Callers must invoke this only after the player approves.
    pub fn record_consent(&mut self, source_id: &str, digest: &str) -> Result<(), StoreError> {
        validate_source_id(source_id)?;
        validate_digest(digest)?;
        let mut next = self.consents.clone();
        next.by_source
            .entry(source_id.to_owned())
            .or_default()
            .insert(digest.to_owned());
        self.save_consents(&next)?;
        self.consents = next;
        Ok(())
    }

    /// The folder of `id` in `version`.
    pub fn folder(&self, id: &str, version: &str) -> PathBuf {
        self.root.join(id).join(version)
    }

    /// Fetches the index from `source` and accepts it ([`Self::accept_index`]).
    pub fn refresh(&mut self, source: &dyn Source, keys: &[Vec<u8>]) -> Result<Index, StoreError> {
        let (json, signature) = source.index()?;
        self.accept_index(&json, &signature, keys)
    }

    /// Accepts `json` as the index if `signature` is one of `keys`' and its
    /// serial is not older than one accepted before, and keeps it.
    pub fn accept_index(
        &mut self,
        json: &[u8],
        signature: &[u8],
        keys: &[Vec<u8>],
    ) -> Result<Index, StoreError> {
        let index = Index::verified(json, signature, keys)?;
        let digest = signed::sha256_hex(json);

        // A cache pair can be one rename ahead of the registry after power
        // loss. A valid higher serial is accepted history too: persist its
        // floor before considering the newly fetched bytes.
        let cached = match self.verified_cached_index(keys) {
            Ok(cached) => cached,
            Err(StoreError::Index(_) | StoreError::NoIndex) => None,
            Err(error) => return Err(error),
        };
        if let Some((cached, cached_json)) = cached {
            let cached_digest = signed::sha256_hex(&cached_json);
            if cached.serial > self.registry.serial {
                self.registry.serial = cached.serial;
                self.registry.index_sha256 = Some(cached_digest);
                self.save()?;
            } else if cached.serial == self.registry.serial {
                match &self.registry.index_sha256 {
                    Some(accepted)
                        if accepted != &cached_digest && index.serial <= self.registry.serial =>
                    {
                        return Err(StoreError::SerialConflict {
                            serial: index.serial,
                        });
                    }
                    None => {
                        self.registry.index_sha256 = Some(cached_digest);
                        self.save()?;
                    }
                    _ => {}
                }
            }
        }

        if index.serial < self.registry.serial {
            return Err(StoreError::OlderIndex {
                offered: index.serial,
                seen: self.registry.serial,
            });
        }
        if index.serial == self.registry.serial {
            match &self.registry.index_sha256 {
                Some(accepted) if accepted == &digest => {
                    // Rewriting the same accepted bytes repairs a missing or
                    // mixed cache pair without changing the serial binding.
                    write_atomic(&self.root, &self.root.join(index::INDEX_FILE), json)?;
                    write_atomic(
                        &self.root,
                        &self.root.join(index::SIGNATURE_FILE),
                        signature,
                    )?;
                    return Ok(index);
                }
                Some(_) => {
                    return Err(StoreError::SerialConflict {
                        serial: index.serial,
                    });
                }
                None if self.registry.serial == 0 => {}
                None => {
                    return Err(StoreError::UnboundSerial {
                        serial: index.serial,
                    });
                }
            }
        }
        ensure_no_symlink_path(&self.root, &self.root)?;
        fs::create_dir_all(&self.root)?;
        write_atomic(&self.root, &self.root.join(index::INDEX_FILE), json)?;
        write_atomic(
            &self.root,
            &self.root.join(index::SIGNATURE_FILE),
            signature,
        )?;
        self.registry.serial = index.serial;
        self.registry.index_sha256 = Some(digest);
        self.save()?;
        Ok(index)
    }

    /// The last accepted index, verified again. A complete cache written
    /// before a crash can advance and repair the registry's serial floor.
    pub fn cached_index(&mut self, keys: &[Vec<u8>]) -> Result<Index, StoreError> {
        let (index, json) = self
            .verified_cached_index(keys)?
            .ok_or(StoreError::NoIndex)?;
        if index.serial < self.registry.serial {
            return Err(StoreError::OlderIndex {
                offered: index.serial,
                seen: self.registry.serial,
            });
        }
        let digest = signed::sha256_hex(&json);
        if index.serial == self.registry.serial
            && self
                .registry
                .index_sha256
                .as_ref()
                .is_some_and(|accepted| accepted != &digest)
        {
            return Err(StoreError::SerialConflict {
                serial: index.serial,
            });
        }
        if index.serial > self.registry.serial || self.registry.index_sha256.is_none() {
            self.registry.serial = index.serial;
            self.registry.index_sha256 = Some(digest);
            self.save()?;
        }
        Ok(index)
    }

    fn verified_cached_index(
        &self,
        keys: &[Vec<u8>],
    ) -> Result<Option<(Index, Vec<u8>)>, StoreError> {
        let json_path = self.root.join(index::INDEX_FILE);
        let signature_path = self.root.join(index::SIGNATURE_FILE);
        ensure_no_symlink_path(&self.root, &json_path)?;
        ensure_no_symlink_path(&self.root, &signature_path)?;
        let json = read_optional(&json_path)?;
        let signature = read_optional(&signature_path)?;
        match (json, signature) {
            (Some(json), Some(signature)) => {
                let index = Index::verified(&json, &signature, keys)?;
                Ok(Some((index, json)))
            }
            (None, None) => Ok(None),
            _ => Err(StoreError::NoIndex),
        }
    }

    /// Installs `id` (the newest version for the build) and what it needs.
    /// Upgrades it when it is installed. Returns what was installed.
    pub fn install(
        &mut self,
        context: resolve::Context<'_>,
        id: &str,
        source: &dyn Source,
    ) -> Result<Vec<String>, StoreError> {
        self.install_resolved(context, id, None, None, source)
    }

    /// Installs `id` in exactly `version` (a room's) and what it needs, or
    /// makes that version current again when it is on disk.
    pub fn install_exact(
        &mut self,
        context: resolve::Context<'_>,
        id: &str,
        version: &str,
        source: &dyn Source,
    ) -> Result<Vec<String>, StoreError> {
        self.install_resolved(context, id, Some(version), None, source)
    }

    /// Source-aware install. Plugin-bearing packages in the entire resolved
    /// closure must have an approval for their full content identity and
    /// effective settings before any package is installed or made current.
    pub fn install_from(
        &mut self,
        context: resolve::Context<'_>,
        id: &str,
        source_id: &str,
        effective_settings_by_package: &EffectiveSettingsByPackage,
        source: &dyn Source,
    ) -> Result<Vec<String>, StoreError> {
        self.install_resolved(
            context,
            id,
            None,
            Some((source_id, effective_settings_by_package)),
            source,
        )
    }

    /// Source-aware exact-version install; see [`Self::install_from`].
    pub fn install_exact_from(
        &mut self,
        context: resolve::Context<'_>,
        id: &str,
        version: &str,
        source_id: &str,
        effective_settings_by_package: &EffectiveSettingsByPackage,
        source: &dyn Source,
    ) -> Result<Vec<String>, StoreError> {
        self.install_resolved(
            context,
            id,
            Some(version),
            Some((source_id, effective_settings_by_package)),
            source,
        )
    }

    fn install_resolved(
        &mut self,
        context: resolve::Context<'_>,
        id: &str,
        version: Option<&str>,
        source_context: Option<(&str, &EffectiveSettingsByPackage)>,
        source: &dyn Source,
    ) -> Result<Vec<String>, StoreError> {
        if let Some((source_id, _)) = source_context {
            validate_source_id(source_id)?;
        }
        let installed: Vec<&Package> = self
            .registry
            .packages
            .values()
            .filter_map(Installed::package)
            .collect();
        let resolution = match version {
            Some(version) => resolve::resolve_exact_plan(context, id, version, &installed),
            None => resolve::resolve_plan(context, &[id], &installed),
        };
        let resolution = resolution?;
        let mut selected_sources = BTreeMap::new();
        for package in &resolution.closure {
            let current_index_package = context.index.package(&package.id, &package.version);
            match source_context {
                Some((source_id, settings_by_package)) => {
                    let source = if current_index_package.is_some() {
                        Some(source_id)
                    } else {
                        self.source_for(&package.id, &package.version)
                    };
                    let settings = settings_by_package.get(&package.id);
                    self.require_plugin_consent(package, source, settings)?;
                    if current_index_package.is_some() {
                        selected_sources.insert(
                            (package.id.clone(), package.version.clone()),
                            source_id.to_owned(),
                        );
                    }
                }
                None => {
                    if !package.plugins.is_empty() {
                        return Err(StoreError::MissingSource {
                            id: package.id.clone(),
                            version: package.version.clone(),
                        });
                    }
                }
            }
        }

        let order: Vec<Package> = resolution.install.into_iter().cloned().collect();
        let mut done = Vec::new();
        for package in &order {
            self.install_one(package, source)?;
            done.push(format!("{} {}", package.id, package.version));
        }
        let mut next_sources = self.sources.clone();
        for ((id, version), source_id) in selected_sources {
            next_sources
                .packages
                .entry(id)
                .or_default()
                .insert(version, source_id);
        }
        if source_context.is_some() && !next_sources.packages.is_empty() {
            self.save_sources(&next_sources)?;
            self.sources = next_sources;
        }
        Ok(done)
    }

    fn install_one(&mut self, package: &Package, source: &dyn Source) -> Result<(), StoreError> {
        let folder = self.folder(&package.id, &package.version);
        check_package_paths(&self.root, &folder, package)?;
        let registered = self
            .registry
            .packages
            .get(&package.id)
            .and_then(|installed| installed.versions.get(&package.version))
            .cloned();
        let expected = package_file_set(package);
        let present = package_file_set_on_disk(&self.root, &folder)?;
        if let Some(old) = &registered {
            // Every file already in this folder must have belonged to the
            // registered package. In particular, a user file whose path was
            // only added by the new manifest remains an extra and is never
            // silently overwritten.
            let old_files = package_file_set(old);
            if !present.is_subset(&old_files) {
                return Err(StoreError::Occupied(folder));
            }
            if present == expected && self.damaged(&folder, package)?.is_empty() {
                return self.make_current(package).map_err(StoreError::from);
            }
        } else if folder.exists() {
            // An unregistered folder may be left by an interrupted install.
            // Take it only when its complete file set and bytes are exactly
            // those of the signed package.
            if present != expected || !self.damaged(&folder, package)?.is_empty() {
                return Err(StoreError::Occupied(folder));
            }
            return self.make_current(package).map_err(StoreError::from);
        }
        let partial = self
            .root
            .join(&package.id)
            .join(format!("{PARTIAL}{}", package.version));
        ensure_no_symlink_path(&self.root, &partial)?;
        if partial.exists() {
            ensure_no_symlink_tree(&self.root, &partial)?;
            fs::remove_dir_all(&partial)?;
        }
        let backup = self
            .root
            .join(&package.id)
            .join(format!("{PARTIAL}{}-previous", package.version));
        ensure_no_symlink_path(&self.root, &backup)?;
        if backup.exists() {
            return Err(StoreError::Occupied(backup));
        }
        fs::create_dir_all(&partial)?;
        let downloaded = package.local_files().iter().try_for_each(|file| {
            let path = partial.join(&file.path);
            ensure_no_symlink_path(&self.root, &path)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            ensure_no_symlink_path(&self.root, &path)?;
            source.download(file, &path).map_err(StoreError::from)
        });
        if let Err(error) = downloaded {
            let _ = remove_partial_tree(&self.root, &partial);
            return Err(error);
        }
        ensure_no_symlink_tree(&self.root, &partial)?;
        if package_file_set_on_disk(&self.root, &partial)? != expected
            || !self.damaged(&partial, package)?.is_empty()
        {
            let _ = remove_partial_tree(&self.root, &partial);
            return Err(StoreError::Fetch(FetchError::Mismatch));
        }
        ensure_no_symlink_path(&self.root, &folder)?;
        let moved_old = registered.is_some() && folder.exists();
        if moved_old && let Err(error) = fs::rename(&folder, &backup) {
            let _ = remove_partial_tree(&self.root, &partial);
            return Err(error.into());
        }
        if let Err(error) = fs::rename(&partial, &folder) {
            if moved_old {
                let _ = fs::rename(&backup, &folder);
            }
            let _ = remove_partial_tree(&self.root, &partial);
            return Err(error.into());
        }
        let registry_before = self.registry.clone();
        if let Err(error) = self.make_current(package) {
            if error.saved {
                // registry.json now describes the new bytes. Keep the new
                // folder in place; restoring the backup would split the
                // registry identity from the files it authenticates.
                if moved_old {
                    remove_partial_tree(&self.root, &backup)?;
                }
                return Err(error.source);
            }
            self.registry = registry_before;
            let _ = fs::rename(&folder, &partial);
            if moved_old {
                let _ = fs::rename(&backup, &folder);
            }
            let _ = remove_partial_tree(&self.root, &partial);
            return Err(error.source);
        }
        if moved_old {
            remove_partial_tree(&self.root, &backup)?;
        }
        Ok(())
    }

    /// Records `package`, on disk, as its id's version in use, keeping the
    /// one before and deleting older ones.
    fn make_current(&mut self, package: &Package) -> Result<(), MakeCurrentError> {
        let prior_current = self
            .registry
            .packages
            .get(&package.id)
            .map(|installed| installed.current.clone());
        if let Some(installed) = self.registry.packages.get(&package.id) {
            for old in installed.versions.values().filter(|old| {
                old.version != package.version
                    && prior_current.as_deref() != Some(old.version.as_str())
            }) {
                check_package_paths(&self.root, &self.folder(&old.id, &old.version), old)?;
            }
        }
        let entry = self
            .registry
            .packages
            .entry(package.id.clone())
            .or_insert_with(|| Installed {
                current: package.version.clone(),
                previous: None,
                enabled: false,
                settings: BTreeMap::new(),
                versions: BTreeMap::new(),
            });
        entry
            .versions
            .insert(package.version.clone(), package.clone());
        if entry.current != package.version {
            entry.previous = Some(std::mem::replace(
                &mut entry.current,
                package.version.clone(),
            ));
        }
        // The player's settings that the new version still has, with the
        // same type.
        entry.settings.retain(|key, value| {
            package
                .settings
                .get(key)
                .is_some_and(|default| default.same_type(value))
        });
        let keep = [Some(entry.current.clone()), entry.previous.clone()];
        let old: Vec<Package> = entry
            .versions
            .values()
            .filter(|p| !keep.contains(&Some(p.version.clone())))
            .cloned()
            .collect();
        for package in &old {
            entry.versions.remove(&package.version);
        }
        self.save()?;
        if let Err(source) = self.prune_sources() {
            return Err(MakeCurrentError {
                source,
                saved: true,
            });
        }
        for package in old {
            if let Err(source) = remove_registered(
                &self.root,
                &self.folder(&package.id, &package.version),
                &package,
            ) {
                return Err(MakeCurrentError {
                    source,
                    saved: true,
                });
            }
        }
        Ok(())
    }

    /// Deletes every file of `id` the registry names, and the folders that
    /// leaves empty. Refused while another installed package needs it.
    pub fn uninstall(&mut self, id: &str) -> Result<(), StoreError> {
        let installed = self
            .registry
            .packages
            .get(id)
            .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?;
        if let Some(by) = self
            .registry
            .packages
            .values()
            .filter_map(Installed::package)
            .find(|p| p.id != id && p.depends.iter().any(|d| d.id == id))
        {
            return Err(StoreError::Needed {
                id: id.to_owned(),
                by: by.id.clone(),
            });
        }
        let versions: Vec<Package> = installed.versions.values().cloned().collect();
        for package in &versions {
            check_package_paths(&self.root, &self.folder(id, &package.version), package)?;
        }
        let package_folder = self.root.join(id);
        ensure_no_symlink_path(&self.root, &package_folder)?;
        let partials = match fs::read_dir(&package_folder) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with(PARTIAL))
                })
                .collect::<Vec<_>>(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        for partial in &partials {
            ensure_no_symlink_tree(&self.root, partial)?;
        }
        self.registry.packages.remove(id);
        self.save()?;
        let mut next_sources = self.sources.clone();
        next_sources.packages.remove(id);
        if !next_sources.packages.is_empty() || self.root.join(PACKAGE_SOURCES).exists() {
            self.save_sources(&next_sources)?;
            self.sources = next_sources;
        }
        for package in &versions {
            remove_registered(&self.root, &self.folder(id, &package.version), package)?;
        }
        for partial in partials {
            let _ = remove_partial_tree(&self.root, &partial);
        }
        let _ = fs::remove_dir(&package_folder);
        Ok(())
    }

    /// Goes back to the version in use before the last upgrade, if its
    /// files are still whole.
    pub fn rollback(&mut self, id: &str) -> Result<(), StoreError> {
        let installed = self
            .registry
            .packages
            .get(id)
            .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?;
        let previous = installed
            .previous
            .as_ref()
            .and_then(|v| installed.versions.get(v))
            .ok_or_else(|| StoreError::NoPrevious(id.to_owned()))?
            .clone();
        let settings = effective_settings(&previous, &installed.settings);
        ensure_plugin_platform_supported(&previous, index::current_platform())?;
        self.require_plugin_consent(
            &previous,
            self.source_for(id, &previous.version),
            Some(&settings),
        )?;
        self.verify_package(&self.folder(id, &previous.version), &previous)?;
        self.make_current(&previous).map_err(StoreError::from)
    }

    /// Checks the files of the version of `id` in use.
    pub fn verify(&self, id: &str) -> Result<(), StoreError> {
        let package = self
            .registry
            .packages
            .get(id)
            .and_then(Installed::package)
            .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?;
        self.verify_package(&self.folder(id, &package.version), package)
    }

    /// Switches `id` on or off for the next game.
    pub fn set_enabled(&mut self, id: &str, on: bool) -> Result<(), StoreError> {
        self.set_enabled_in(features::BUILT_IN, id, on)
    }

    /// [`Store::set_enabled`] with the features in `registry`: switching a
    /// package on whose features clash with those of one already on is
    /// refused ([`enabled::first_clash`]).
    pub fn set_enabled_in(
        &mut self,
        registry: &[features::FeatureSpec],
        id: &str,
        on: bool,
    ) -> Result<(), StoreError> {
        if on {
            let installed = self
                .registry
                .packages
                .get(id)
                .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?;
            let package = installed
                .package()
                .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?;
            let settings = effective_settings(package, &installed.settings);
            ensure_plugin_platform_supported(package, index::current_platform())?;
            self.require_plugin_consent(
                package,
                self.source_for(id, &package.version),
                Some(&settings),
            )?;
            if let Some(why) = self.clash(registry, id) {
                return Err(StoreError::Clash {
                    id: id.to_owned(),
                    why,
                });
            }
        }
        self.registry
            .packages
            .get_mut(id)
            .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?
            .enabled = on;
        self.save()
    }

    /// Why the installed package `id` cannot be on beside those that are,
    /// with the features in `registry`, if it cannot.
    pub fn clash(&self, registry: &[features::FeatureSpec], id: &str) -> Option<String> {
        let mine = self.registry.packages.get(id)?.package()?;
        let others: Vec<(&str, &[String])> = self
            .registry
            .packages
            .iter()
            .filter(|(other, installed)| other.as_str() != id && installed.enabled)
            .filter_map(|(other, installed)| {
                installed
                    .package()
                    .map(|package| (other.as_str(), package.features.as_slice()))
            })
            .collect();
        enabled::first_clash(registry, &mine.features, &others).or_else(|| {
            self.registry
                .packages
                .iter()
                .filter(|(other, installed)| other.as_str() != id && installed.enabled)
                .find_map(|(other, installed)| {
                    let theirs = installed.package()?;
                    enabled::hooks_clash(&mine.hooks, &theirs.hooks)
                        .map(|why| format!("it clashes with {other}: {why}"))
                })
        })
    }

    /// Puts the game mods of the packages on for `build` into the game's
    /// `mods` folder (`mods_dir`), each a copy of its folder in the
    /// package, and takes away those it put there before for packages no
    /// longer on. A folder it put there holds [`GAME_MOD_MARK`]; one
    /// without it is a player's own and is never touched: a package whose
    /// game mod would take its name is refused. Returns what it did, for
    /// the log.
    pub fn sync_game_mods(&self, build: &str, mods_dir: &Path) -> Result<Vec<String>, StoreError> {
        let mut wanted: BTreeMap<String, (PathBuf, &Package)> = BTreeMap::new();
        for installed in self.registry.packages.values().filter(|i| i.enabled) {
            let Some(package) = installed.package().filter(|p| p.runs_on(build)) else {
                continue;
            };
            for folder in &package.game_mods {
                let name = folder.trim_start_matches("mod/").to_owned();
                let from = self.folder(&package.id, &package.version).join(folder);
                wanted.insert(name, (from, package));
            }
        }
        let mut done = Vec::new();
        // Ours that are no longer wanted go first.
        if let Ok(entries) = fs::read_dir(mods_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = entry.path();
                if path.join(GAME_MOD_MARK).is_file() && !wanted.contains_key(&name) {
                    fs::remove_dir_all(&path)?;
                    done.push(format!("game mod {name} taken away"));
                }
            }
        }
        for (name, (from, package)) in wanted {
            let to = mods_dir.join(&name);
            let mark = format!("{} {}\n", package.id, package.version);
            if to.exists() {
                match fs::read_to_string(to.join(GAME_MOD_MARK)) {
                    Ok(theirs) if theirs == mark => continue,
                    Ok(_) => fs::remove_dir_all(&to)?,
                    Err(_) => {
                        return Err(StoreError::Occupied(to));
                    }
                }
            }
            copy_tree(&from, &to)?;
            fs::write(to.join(GAME_MOD_MARK), mark)?;
            done.push(format!(
                "game mod {name} from {} {}",
                package.id, package.version
            ));
        }
        Ok(done)
    }

    /// The enabled packages, in the order the game switches them on
    /// ([`enabled::load_order`]).
    pub fn load_order(&self) -> Vec<String> {
        let packages: Vec<(&str, Vec<&str>)> = self
            .registry
            .packages
            .iter()
            .filter(|(_, installed)| installed.enabled)
            .filter_map(|(id, installed)| {
                installed.package().map(|package| {
                    (
                        id.as_str(),
                        package.depends.iter().map(|d| d.id.as_str()).collect(),
                    )
                })
            })
            .collect();
        enabled::load_order(&packages)
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    /// Puts `id`'s settings back to its package's defaults.
    pub fn reset_settings(&mut self, id: &str) -> Result<(), StoreError> {
        let installed = self
            .registry
            .packages
            .get(id)
            .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?;
        installed
            .package()
            .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?;
        self.registry
            .packages
            .get_mut(id)
            .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?
            .settings
            .clear();
        self.save()
    }

    /// Sets one of `id`'s settings: one its package has, with the same type.
    pub fn set_setting(&mut self, id: &str, key: &str, value: Setting) -> Result<(), StoreError> {
        let installed = self
            .registry
            .packages
            .get(id)
            .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?;
        let package = installed
            .versions
            .get(&installed.current)
            .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?
            .clone();
        match package.settings.get(key) {
            Some(default) if default.same_type(&value) => {
                self.registry
                    .packages
                    .get_mut(id)
                    .ok_or_else(|| StoreError::NotInstalled(id.to_owned()))?
                    .settings
                    .insert(key.to_owned(), value);
                self.save()
            }
            Some(_) => Err(StoreError::Setting(format!(
                "{id}'s setting {key} has another type"
            ))),
            None => Err(StoreError::Setting(format!("{id} has no setting {key}"))),
        }
    }

    /// The enabled packages for the game build hashing to `build`, with
    /// their settings. Every enabled package's listed bytes and complete
    /// file set are verified before its identity is declared. An enabled
    /// package not for this build is left out, with its id in the second
    /// list.
    pub fn enabled_for(&self, build: &str) -> Result<(Enabled, Vec<String>), StoreError> {
        let mut packages = Vec::new();
        let mut left_out = Vec::new();
        for id in self.load_order() {
            let Some(installed) = self.registry.packages.get(&id) else {
                continue;
            };
            let id = &id;
            let Some(package) = installed.package().filter(|_| installed.enabled) else {
                continue;
            };
            if !package.runs_on(build) {
                if package.simulation
                    || package.features.iter().any(|id| {
                        features::find(features::BUILT_IN, id)
                            .is_none_or(|feature| feature.effect == Effect::Simulation)
                    })
                {
                    return Err(StoreError::SimulationPackageNotForBuild {
                        id: id.clone(),
                        build: build.to_owned(),
                    });
                }
                left_out.push(id.clone());
                continue;
            }
            let settings = effective_settings(package, &installed.settings);
            ensure_plugin_platform_supported(package, index::current_platform())?;
            self.require_plugin_consent(
                package,
                self.source_for(id, &package.version),
                Some(&settings),
            )?;
            self.verify_package(&self.folder(id, &package.version), package)?;
            let identity_sha256 = crate::terms::package_digest(package, &settings);
            packages.push(EnabledPackage {
                id: id.clone(),
                version: package.version.clone(),
                simulation: package.simulation,
                features: package.features.clone(),
                settings,
                files_sha256: crate::terms::files_digest(&package.files),
                identity_sha256,
                depends: package.depends.clone(),
                root: self.folder(id, &package.version),
                plugins: crate::index::current_platform()
                    .map(|platform| package.plugins_for(platform))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|plugin| enabled::EnabledPlugin {
                        path: plugin.path.clone(),
                        sha256: plugin.sha256.clone(),
                        abi: plugin.abi,
                    })
                    .collect(),
                hooks: package.hooks.clone(),
            });
        }
        Ok((
            Enabled {
                format: enabled::FORMAT,
                build: build.to_owned(),
                packages,
            },
            left_out,
        ))
    }

    /// Writes what the next game on `build` runs to `enabled.json`, for
    /// the launcher to name in the game's environment ([`enabled::ENV`]).
    pub fn write_enabled(&self, build: &str) -> Result<(PathBuf, Vec<String>), StoreError> {
        let (enabled, left_out) = self.enabled_for(build)?;
        ensure_no_symlink_path(&self.root, &self.root)?;
        fs::create_dir_all(&self.root)?;
        let path = self.root.join(enabled::FILE);
        let json =
            serde_json::to_vec_pretty(&enabled).map_err(|e| StoreError::Registry(e.to_string()))?;
        write_atomic(&self.root, &path, &json)?;
        Ok((path, left_out))
    }

    /// The files of `package` in `folder` that are missing or changed.
    fn damaged(&self, folder: &Path, package: &Package) -> Result<Vec<String>, StoreError> {
        check_package_paths(&self.root, folder, package)?;
        let mut damaged = Vec::new();
        for file in &package.local_files() {
            if !fetch::file_matches(&folder.join(&file.path), file.size, &file.sha256)? {
                damaged.push(file.path.clone());
            }
        }
        Ok(damaged)
    }

    fn verify_package(&self, folder: &Path, package: &Package) -> Result<(), StoreError> {
        let damaged = self.damaged(folder, package)?;
        if !damaged.is_empty() {
            return Err(StoreError::Damaged {
                id: package.id.clone(),
                version: package.version.clone(),
                files: damaged,
            });
        }
        if package_file_set_on_disk(&self.root, folder)? != package_file_set(package) {
            return Err(StoreError::Occupied(folder.to_owned()));
        }
        Ok(())
    }

    fn save_sources(&self, sources: &PackageSources) -> Result<(), StoreError> {
        ensure_no_symlink_path(&self.root, &self.root)?;
        fs::create_dir_all(&self.root)?;
        let json = serde_json::to_vec_pretty(sources)
            .map_err(|error| StoreError::ConsentStore(error.to_string()))?;
        write_atomic(&self.root, &self.root.join(PACKAGE_SOURCES), &json)
    }

    fn save_consents(&self, consents: &ConsentRecords) -> Result<(), StoreError> {
        ensure_no_symlink_path(&self.root, &self.root)?;
        fs::create_dir_all(&self.root)?;
        let json = serde_json::to_vec_pretty(consents)
            .map_err(|error| StoreError::ConsentStore(error.to_string()))?;
        write_atomic(&self.root, &self.root.join(CONSENTS), &json)
    }

    fn prune_sources(&mut self) -> Result<(), StoreError> {
        let mut next = self.sources.clone();
        next.packages.retain(|id, versions| {
            if let Some(installed) = self.registry.packages.get(id) {
                versions.retain(|version, _| installed.versions.contains_key(version));
            } else {
                versions.clear();
            }
            !versions.is_empty()
        });
        if next.packages.is_empty() && !self.root.join(PACKAGE_SOURCES).exists() {
            return Ok(());
        }
        self.save_sources(&next)?;
        self.sources = next;
        Ok(())
    }

    fn require_plugin_consent(
        &self,
        package: &Package,
        source_id: Option<&str>,
        settings: Option<&BTreeMap<String, Setting>>,
    ) -> Result<(), StoreError> {
        if package.plugins.is_empty() {
            return Ok(());
        }
        let source_id = source_id.ok_or_else(|| StoreError::MissingSource {
            id: package.id.clone(),
            version: package.version.clone(),
        })?;
        let settings =
            settings.ok_or_else(|| StoreError::MissingEffectiveSettings(package.id.clone()))?;
        if package.settings.len() != settings.len()
            || package.settings.iter().any(|(key, default)| {
                settings
                    .get(key)
                    .is_none_or(|value| !default.same_type(value))
            })
        {
            return Err(StoreError::Setting(format!(
                "effective settings for {} do not match its signed schema",
                package.id
            )));
        }
        let digest = crate::terms::package_digest(package, settings);
        if self.has_consent(source_id, &digest)? {
            Ok(())
        } else {
            Err(StoreError::ConsentRequired {
                id: package.id.clone(),
                version: package.version.clone(),
                source_id: source_id.to_owned(),
                digest,
            })
        }
    }

    fn save(&self) -> Result<(), StoreError> {
        ensure_no_symlink_path(&self.root, &self.root)?;
        fs::create_dir_all(&self.root)?;
        let json = serde_json::to_vec_pretty(&self.registry)
            .map_err(|error| StoreError::Registry(error.to_string()))?;
        write_atomic(&self.root, &self.root.join(REGISTRY), &json)?;
        Ok(())
    }
}

/// Deletes the files `package` names in `folder`, then the folders that
/// leaves empty, `folder` last. Nothing else.
fn remove_registered(root: &Path, folder: &Path, package: &Package) -> Result<(), StoreError> {
    check_package_paths(root, folder, package)?;
    let mut dirs = Vec::new();
    for file in &package.local_files() {
        let path = folder.join(&file.path);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let mut parent = path.parent();
        while let Some(dir) = parent.filter(|dir| dir.starts_with(folder) && *dir != folder) {
            dirs.push(dir.to_owned());
            parent = dir.parent();
        }
    }
    // Deepest first; a folder that is not empty stays.
    dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    dirs.dedup();
    for dir in dirs {
        let _ = fs::remove_dir(dir);
    }
    let _ = fs::remove_dir(folder);
    Ok(())
}

/// Writes `bytes` to `path` through a file beside it, so that a reader
/// sees the old contents or the new, never part of them.
fn write_atomic(root: &Path, path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    ensure_no_symlink_path(root, path)?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".new");
    let temporary = PathBuf::from(temporary);
    ensure_no_symlink_path(root, &temporary)?;
    {
        use io::Write;
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&temporary, path)?;
    Ok(())
}

fn validate_registry(registry: &Registry) -> Result<(), StoreError> {
    if registry
        .index_sha256
        .as_ref()
        .is_some_and(|digest| !signed::is_sha256(digest))
    {
        return Err(StoreError::Registry("invalid accepted-index digest".into()));
    }
    for (id, installed) in &registry.packages {
        if !index::is_id(id) {
            return Err(StoreError::Registry(format!("invalid package id {id:?}")));
        }
        let Some(current) = installed.versions.get(&installed.current) else {
            return Err(StoreError::Registry(format!(
                "{id} has no registered current version {}",
                installed.current
            )));
        };
        if let Some(previous) = &installed.previous
            && (previous == &installed.current || !installed.versions.contains_key(previous))
        {
            return Err(StoreError::Registry(format!(
                "{id} has an invalid previous version {previous}"
            )));
        }
        for (version, package) in &installed.versions {
            package
                .validate()
                .map_err(|problem| StoreError::Registry(format!("{id} {version}: {problem}")))?;
            if package.id != *id || package.version != *version {
                return Err(StoreError::Registry(format!(
                    "{id} {version} does not match its package identity"
                )));
            }
        }
        for (key, value) in &installed.settings {
            match current.settings.get(key) {
                Some(default) if default.same_type(value) => {}
                _ => {
                    return Err(StoreError::Registry(format!(
                        "{id} has an invalid setting {key:?}"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Rejects a symlink in an internal path before a file operation. `root` is
/// the store boundary; every remaining component must be a normal relative
/// name and every existing component must be an ordinary filesystem entry.
fn ensure_no_symlink_path(root: &Path, path: &Path) -> Result<(), StoreError> {
    let absolute = |path: &Path| -> Result<PathBuf, StoreError> {
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()?.join(path)
        };
        if path.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        }) {
            return Err(StoreError::UnsafePath(path));
        }
        Ok(path)
    };
    let root = absolute(root)?;
    let path = absolute(path)?;
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| StoreError::UnsafePath(path.clone()))?;
    if relative
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(StoreError::UnsafePath(path));
    }

    // The caller owns the store root, not its parents. System data paths can
    // contain symlinked ancestors (macOS /var -> /private/var); reject links
    // at the root and below, where registry-controlled paths are managed.
    let mut ancestors = vec![root.clone()];
    let mut ancestor = root.clone();
    for component in relative.components() {
        ancestor.push(component);
        ancestors.push(ancestor.clone());
    }
    for ancestor in ancestors {
        match fs::symlink_metadata(&ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(StoreError::UnsafePath(ancestor));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// The file in a game mod's folder that says the launcher put it there,
/// and from which package.
pub const GAME_MOD_MARK: &str = ".tpf3mp-native-mod";

/// Copies the folder `from` to `to`, files and folders, no links.
fn copy_tree(from: &Path, to: &Path) -> Result<(), StoreError> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)?;
        } else {
            return Err(StoreError::UnsafePath(entry.path()));
        }
    }
    Ok(())
}

fn check_package_paths(root: &Path, folder: &Path, package: &Package) -> Result<(), StoreError> {
    ensure_no_symlink_path(root, folder)?;
    for file in &package.local_files() {
        if !index::is_safe_path(&file.path) {
            return Err(StoreError::UnsafePath(folder.join(&file.path)));
        }
        ensure_no_symlink_path(root, &folder.join(&file.path))?;
    }
    Ok(())
}

/// The paths a package's signed manifest names, case-folded as Windows does.
fn package_file_set(package: &Package) -> BTreeSet<String> {
    package
        .local_files()
        .iter()
        .map(|file| file.path.to_ascii_lowercase())
        .collect()
}

fn effective_settings(
    package: &Package,
    overrides: &BTreeMap<String, Setting>,
) -> BTreeMap<String, Setting> {
    let mut settings = package.settings.clone();
    for (key, value) in overrides {
        if package
            .settings
            .get(key)
            .is_some_and(|default| default.same_type(value))
        {
            settings.insert(key.clone(), value.clone());
        }
    }
    settings
}

/// Regular files physically present in a package folder, case-folded as
/// Windows does. Links and special files are refused before being followed.
fn package_file_set_on_disk(root: &Path, folder: &Path) -> Result<BTreeSet<String>, StoreError> {
    ensure_no_symlink_tree(root, folder)?;
    let metadata = match fs::symlink_metadata(folder) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() {
        return Err(StoreError::Occupied(folder.to_owned()));
    }

    let mut files = BTreeSet::new();
    let mut directories = vec![folder.to_owned()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                directories.push(path);
            } else if kind.is_file() {
                let relative = path
                    .strip_prefix(folder)
                    .map_err(|_| StoreError::UnsafePath(path.clone()))?;
                files.insert(
                    relative
                        .to_string_lossy()
                        .replace('\\', "/")
                        .to_ascii_lowercase(),
                );
            } else {
                return Err(StoreError::Occupied(path));
            }
        }
    }
    Ok(files)
}

fn ensure_no_symlink_tree(root: &Path, folder: &Path) -> Result<(), StoreError> {
    ensure_no_symlink_path(root, folder)?;
    let metadata = match fs::symlink_metadata(folder) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(folder)? {
        ensure_no_symlink_tree(root, &entry?.path())?;
    }
    Ok(())
}

fn remove_partial_tree(root: &Path, folder: &Path) -> Result<(), StoreError> {
    ensure_no_symlink_tree(root, folder)?;
    match fs::remove_dir_all(folder) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, StoreError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn read_package_sources(root: &Path) -> Result<PackageSources, StoreError> {
    let path = root.join(PACKAGE_SOURCES);
    ensure_no_symlink_path(root, &path).map_err(consent_store_error)?;
    let Some(bytes) = read_optional(&path).map_err(consent_store_error)? else {
        return Ok(PackageSources {
            format: PACKAGE_SOURCES_FORMAT,
            packages: BTreeMap::new(),
        });
    };
    let state: PackageSources = serde_json::from_slice(&bytes)
        .map_err(|error| StoreError::ConsentStore(error.to_string()))?;
    if state.format != PACKAGE_SOURCES_FORMAT {
        return Err(StoreError::ConsentStore(format!(
            "package source format {}",
            state.format
        )));
    }
    for (id, versions) in &state.packages {
        if !index::is_id(id) {
            return Err(StoreError::ConsentStore(format!(
                "invalid package id {id:?}"
            )));
        }
        for (version, source) in versions {
            if semver::Version::parse(version).is_err() {
                return Err(StoreError::ConsentStore(format!(
                    "invalid version {id} {version:?}"
                )));
            }
            validate_source_id(source).map_err(consent_store_error)?;
        }
    }
    Ok(state)
}

fn read_consents(root: &Path) -> Result<ConsentRecords, StoreError> {
    let path = root.join(CONSENTS);
    ensure_no_symlink_path(root, &path).map_err(consent_store_error)?;
    let Some(bytes) = read_optional(&path).map_err(consent_store_error)? else {
        return Ok(ConsentRecords {
            format: CONSENTS_FORMAT,
            by_source: BTreeMap::new(),
        });
    };
    let state: ConsentRecords = serde_json::from_slice(&bytes)
        .map_err(|error| StoreError::ConsentStore(error.to_string()))?;
    if state.format != CONSENTS_FORMAT {
        return Err(StoreError::ConsentStore(format!(
            "consent format {}",
            state.format
        )));
    }
    for (source, digests) in &state.by_source {
        validate_source_id(source).map_err(consent_store_error)?;
        for digest in digests {
            validate_digest(digest).map_err(consent_store_error)?;
        }
    }
    Ok(state)
}

fn validate_source_id(source: &str) -> Result<(), StoreError> {
    let invalid = || StoreError::InvalidSource(source.to_owned());
    if source == "project" {
        return Ok(());
    }
    if source.is_empty() || source.len() > 255 || source != source.to_ascii_lowercase() {
        return Err(invalid());
    }
    let (host, port) = if let Some(bracketed) = source.strip_prefix('[') {
        let (address, suffix) = bracketed.split_once(']').ok_or_else(invalid)?;
        let port = suffix.strip_prefix(':').ok_or_else(invalid)?;
        let parsed: std::net::Ipv6Addr = address.parse().map_err(|_| invalid())?;
        if parsed.to_string() != address {
            return Err(invalid());
        }
        (None, port)
    } else {
        let (host, port) = source.split_once(':').ok_or_else(invalid)?;
        if host.contains(':') || host.is_empty() {
            return Err(invalid());
        }
        if !host
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(&byte))
        {
            return Err(invalid());
        }
        (Some(host), port)
    };
    let port_number: u16 = port.parse().map_err(|_| invalid())?;
    if port_number == 0 || port_number.to_string() != port {
        return Err(invalid());
    }
    if let Some(host) = host
        && let Ok(address) = host.parse::<std::net::Ipv4Addr>()
        && address.to_string() != host
    {
        return Err(invalid());
    }
    Ok(())
}

fn validate_digest(digest: &str) -> Result<(), StoreError> {
    if !signed::is_sha256(digest) || digest.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(StoreError::ConsentStore(
            "invalid package identity digest".into(),
        ));
    }
    Ok(())
}

fn consent_store_error(error: StoreError) -> StoreError {
    StoreError::ConsentStore(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::HashMap};

    use super::*;
    use crate::{
        enabled,
        features::example::BIG_MAPS,
        index::{
            Dependency,
            tests::{BUILD, OTHER_BUILD, big_maps, index, keys, package, signed, test_key, url_of},
        },
        resolve::Context,
    };

    /// Serves files from memory, as a server would: bytes by address, some
    /// cut short.
    #[derive(Default)]
    struct Served {
        files: RefCell<HashMap<String, Vec<u8>>>,
        index: RefCell<Option<(Vec<u8>, Vec<u8>)>>,
    }

    impl Served {
        fn serve(&self, id: &str, version: &str, files: &[(&str, &[u8])]) {
            for (path, bytes) in files {
                self.files
                    .borrow_mut()
                    .insert(url_of(id, version, path), bytes.to_vec());
            }
        }
    }

    impl Source for Served {
        fn index(&self) -> Result<(Vec<u8>, Vec<u8>), FetchError> {
            self.index
                .borrow()
                .clone()
                .ok_or_else(|| FetchError::Http("404".into()))
        }

        fn download(&self, file: &FileEntry, path: &Path) -> Result<(), FetchError> {
            let files = self.files.borrow();
            let bytes = files
                .get(&file.url)
                .ok_or_else(|| FetchError::Http("404".into()))?;
            fetch::copy_verified(&bytes[..], path, file.size, &file.sha256, |_| {})
        }
    }

    const V1: &[(&str, &[u8])] = &[
        ("mod/tpf3mp_bigmap_1/mod.lua", b"-- version 1"),
        ("data/sizes.toml", b"sizes = [128]"),
    ];
    const V2: &[(&str, &[u8])] = &[
        ("mod/tpf3mp_bigmap_1/mod.lua", b"-- version 2"),
        ("data/sizes.toml", b"sizes = [128, 256]"),
        ("data/new.toml", b"new = true"),
    ];

    fn context(index: &Index) -> Context<'_> {
        Context {
            index,
            build: BUILD,
            registry: BIG_MAPS,
        }
    }

    #[cfg(all(windows, target_arch = "x86_64"))]
    fn plugin_package(version: &str, path: &str, bytes: &[u8]) -> Package {
        let mut package = package("plugged", version, &[]);
        package.simulation = true;
        package.plugins = vec![crate::index::Plugin {
            path: path.into(),
            url: url_of("plugged", version, path),
            size: bytes.len() as u64,
            sha256: signed::sha256_hex(bytes),
            abi: crate::index::PLUGIN_ABI,
            platform: "windows-x86_64".into(),
        }];
        package
    }

    #[cfg(all(windows, target_arch = "x86_64"))]
    fn approval_settings(package: &Package) -> EffectiveSettingsByPackage {
        BTreeMap::from([(package.id.clone(), package.settings.clone())])
    }

    #[test]
    fn plugin_enable_rejects_targets_without_a_loadable_local_plugin() {
        let mut package = package("plugged", "1.0.0", &[]);
        package.plugins = vec![crate::index::Plugin {
            path: "plugin/main.dll".into(),
            url: "https://example.invalid/plugin/main.dll".into(),
            size: 1,
            sha256: "0".repeat(64),
            abi: crate::index::PLUGIN_ABI,
            platform: "windows-x86_64".into(),
        }];

        assert!(ensure_plugin_platform_supported(&package, None).is_err());
        assert!(ensure_plugin_platform_supported(&package, Some("linux-x86_64")).is_err());
        assert!(ensure_plugin_platform_supported(&package, Some("macos-aarch64")).is_err());
        assert!(ensure_plugin_platform_supported(&package, Some("windows-x86_64")).is_ok());
    }

    /// Every file under `dir`, relative, sorted.
    fn tree(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_owned()];
        while let Some(at) = stack.pop() {
            let Ok(entries) = fs::read_dir(&at) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path.clone());
                }
                out.push(
                    path.strip_prefix(dir)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
        out.sort();
        out
    }

    #[test]
    fn an_install_puts_verified_files_in_the_launchers_folder_and_registers_them() {
        let dir = tempfile::tempdir().unwrap();
        let served = Served::default();
        served.serve("bigmap", "1.0.0", V1);
        let index = index(vec![package("bigmap", "1.0.0", V1)]);
        let mut store = Store::open(dir.path().join("native-mods")).unwrap();
        let done = store.install(context(&index), "bigmap", &served).unwrap();
        assert_eq!(done, ["bigmap 1.0.0"]);
        let folder = store.folder("bigmap", "1.0.0");
        assert_eq!(
            fs::read(folder.join("mod/tpf3mp_bigmap_1/mod.lua")).unwrap(),
            b"-- version 1"
        );
        store.verify("bigmap").unwrap();
        // The registry survives a restart; installing is not enabling.
        let again = Store::open(dir.path().join("native-mods")).unwrap();
        let installed = &again.registry().packages["bigmap"];
        assert_eq!(installed.current, "1.0.0");
        assert!(!installed.enabled);
        assert_eq!(installed.package().unwrap().files.len(), 2);
    }

    #[test]
    #[cfg(all(windows, target_arch = "x86_64"))]
    fn plugin_install_needs_exact_source_approval_and_persists_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("native-mods");
        let bytes = b"plugin v1";
        let package = plugin_package("1.0.0", "plugin/main.dll", bytes);
        let index = index(vec![package.clone()]);
        let served = Served::default();
        served.serve("plugged", "1.0.0", &[("plugin/main.dll", bytes)]);
        let settings = approval_settings(&package);
        let digest = crate::terms::package_digest(&package, &package.settings);
        let mut store = Store::open(&root).unwrap();

        assert!(matches!(
            store.install_exact_from(
                context(&index),
                "plugged",
                "1.0.0",
                "project",
                &settings,
                &served,
            ),
            Err(StoreError::ConsentRequired { .. })
        ));
        assert!(!store.folder("plugged", "1.0.0").exists());

        store.record_consent("project", &digest).unwrap();
        store
            .install_exact_from(
                context(&index),
                "plugged",
                "1.0.0",
                "project",
                &settings,
                &served,
            )
            .unwrap();
        assert_eq!(store.source_for("plugged", "1.0.0"), Some("project"));
        store.set_enabled("plugged", true).unwrap();
        let (enabled, _) = store.enabled_for(BUILD).unwrap();
        assert_eq!(enabled.packages[0].identity_sha256, digest);

        let reopened = Store::open(&root).unwrap();
        assert_eq!(reopened.source_for("plugged", "1.0.0"), Some("project"));
        assert!(reopened.has_consent("project", &digest).unwrap());
        fs::write(root.join(CONSENTS), br#"{"format":1,"by_source":{}}"#).unwrap();
        let mut without_approval = Store::open(&root).unwrap();
        assert!(!without_approval.has_consent("project", &digest).unwrap());
        assert!(matches!(
            without_approval.set_enabled("plugged", true),
            Err(StoreError::ConsentRequired { .. })
        ));
        without_approval.record_consent("project", &digest).unwrap();
        without_approval.set_enabled("plugged", true).unwrap();
    }

    #[test]
    fn malformed_consent_records_are_not_replaced_with_an_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("native-mods");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(CONSENTS), b"{ not json").unwrap();
        assert!(matches!(
            Store::open(&root),
            Err(StoreError::ConsentStore(_))
        ));
    }

    #[test]
    #[cfg(all(windows, target_arch = "x86_64"))]
    fn source_aware_install_gates_plugin_dependencies_before_any_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("native-mods");
        let plugin_bytes = b"plugin";
        let plugin = plugin_package("1.0.0", "plugin/main.dll", plugin_bytes);
        let addon_bytes = b"addon";
        let mut addon = package("addon", "1.0.0", &[("addon.lua", addon_bytes)]);
        addon.depends.push(Dependency {
            id: "plugged".into(),
            version: "^1".into(),
        });
        let index = index(vec![plugin.clone(), addon.clone()]);
        let served = Served::default();
        served.serve("plugged", "1.0.0", &[("plugin/main.dll", plugin_bytes)]);
        served.serve("addon", "1.0.0", &[("addon.lua", addon_bytes)]);
        let mut settings = approval_settings(&plugin);
        settings.extend(approval_settings(&addon));
        let plugin_digest = crate::terms::package_digest(&plugin, &plugin.settings);
        let mut store = Store::open(&root).unwrap();
        assert!(matches!(
            store.install_from(context(&index), "addon", "project", &settings, &served),
            Err(StoreError::ConsentRequired { id, .. }) if id == "plugged"
        ));
        assert!(store.registry().packages.is_empty());
        assert!(!root.join("addon").exists());

        store.record_consent("project", &plugin_digest).unwrap();
        store
            .install_from(context(&index), "addon", "project", &settings, &served)
            .unwrap();
        assert_eq!(store.source_for("plugged", "1.0.0"), Some("project"));
        assert_eq!(store.source_for("addon", "1.0.0"), Some("project"));
    }

    #[test]
    #[cfg(all(windows, target_arch = "x86_64"))]
    fn installed_plugin_bytes_are_checked_before_enabled_identity_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("native-mods");
        let bytes = b"plugin v1";
        let package = plugin_package("1.0.0", "plugin/main.dll", bytes);
        let index = index(vec![package.clone()]);
        let served = Served::default();
        served.serve("plugged", "1.0.0", &[("plugin/main.dll", bytes)]);
        let settings = approval_settings(&package);
        let digest = crate::terms::package_digest(&package, &package.settings);
        let mut store = Store::open(&root).unwrap();
        store.record_consent("project", &digest).unwrap();
        store
            .install_exact_from(
                context(&index),
                "plugged",
                "1.0.0",
                "project",
                &settings,
                &served,
            )
            .unwrap();
        store.set_enabled("plugged", true).unwrap();
        fs::write(
            store.folder("plugged", "1.0.0").join("plugin/main.dll"),
            b"tampered",
        )
        .unwrap();

        assert!(matches!(
            store.write_enabled(BUILD),
            Err(StoreError::Damaged { id, .. }) if id == "plugged"
        ));
        assert!(!root.join(enabled::FILE).exists());
    }

    #[test]
    #[cfg(all(windows, target_arch = "x86_64"))]
    fn same_version_plugin_rename_replaces_the_exact_installed_file_set() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("native-mods");
        let old_bytes = b"old plugin";
        let old = plugin_package("1.0.0", "plugin/old.dll", old_bytes);
        let old_index = index(vec![old.clone()]);
        let served = Served::default();
        served.serve("plugged", "1.0.0", &[("plugin/old.dll", old_bytes)]);
        let settings = approval_settings(&old);
        let old_digest = crate::terms::package_digest(&old, &old.settings);
        let mut store = Store::open(&root).unwrap();
        store.record_consent("project", &old_digest).unwrap();
        store
            .install_exact_from(
                context(&old_index),
                "plugged",
                "1.0.0",
                "project",
                &settings,
                &served,
            )
            .unwrap();

        let new_bytes = b"new plugin";
        let new = plugin_package("1.0.0", "plugin/new.dll", new_bytes);
        let new_index = index(vec![new.clone()]);
        served.serve("plugged", "1.0.0", &[("plugin/new.dll", new_bytes)]);
        let new_settings = approval_settings(&new);
        let new_digest = crate::terms::package_digest(&new, &new.settings);
        assert_ne!(old_digest, new_digest);
        assert!(matches!(
            store.install_exact_from(
                context(&new_index),
                "plugged",
                "1.0.0",
                "project",
                &new_settings,
                &served,
            ),
            Err(StoreError::ConsentRequired { .. })
        ));
        let folder = store.folder("plugged", "1.0.0");
        assert!(folder.join("plugin/old.dll").exists());
        assert!(!folder.join("plugin/new.dll").exists());

        store.record_consent("project", &new_digest).unwrap();
        store
            .install_exact_from(
                context(&new_index),
                "plugged",
                "1.0.0",
                "project",
                &new_settings,
                &served,
            )
            .unwrap();
        assert!(!folder.join("plugin/old.dll").exists());
        assert_eq!(fs::read(folder.join("plugin/new.dll")).unwrap(), new_bytes);
        store.verify("plugged").unwrap();
    }

    #[test]
    fn data_only_packages_keep_source_less_install_and_current_terms() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"simulation data";
        let mut package = package("plain", "1.0.0", &[("data/world.lua", bytes)]);
        package.simulation = true;
        let index = index(vec![package]);
        let served = Served::default();
        served.serve("plain", "1.0.0", &[("data/world.lua", bytes)]);
        let mut store = Store::open(dir.path().join("native-mods")).unwrap();
        store.install(context(&index), "plain", &served).unwrap();
        store.set_enabled("plain", true).unwrap();
        let (enabled, _) = store.enabled_for(BUILD).unwrap();
        assert_eq!(enabled.packages.len(), 1);
        assert_eq!(enabled.packages[0].identity_sha256.len(), 64);
        assert_eq!(crate::terms::terms(&enabled, BIG_MAPS).len(), 1);
    }

    #[test]
    fn a_wrong_hash_or_a_partial_download_installs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("native-mods");
        let index = index(vec![package("bigmap", "1.0.0", V1)]);
        for body in [&b"-- version X"[..], &b"-- ver"[..]] {
            let served = Served::default();
            served.serve("bigmap", "1.0.0", V1);
            served
                .files
                .borrow_mut()
                .insert(url_of("bigmap", "1.0.0", "data/sizes.toml"), body.to_vec());
            let mut store = Store::open(&root).unwrap();
            assert!(matches!(
                store.install(context(&index), "bigmap", &served),
                Err(StoreError::Fetch(FetchError::Mismatch))
            ));
            assert!(store.registry().packages.is_empty());
            // No package folder, no partial download left.
            assert!(tree(&root.join("bigmap")).is_empty(), "{:?}", tree(&root));
        }
    }

    #[test]
    fn a_package_not_pinned_to_this_build_is_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        let served = Served::default();
        served.serve("bigmap", "1.0.0", V1);
        let mut other = package("bigmap", "1.0.0", V1);
        other.builds = vec![OTHER_BUILD.into()];
        let index = index(vec![other]);
        let mut store = Store::open(dir.path()).unwrap();
        assert!(matches!(
            store.install(context(&index), "bigmap", &served),
            Err(StoreError::Resolve(ResolveError::NotForBuild(_)))
        ));
        assert!(tree(dir.path()).is_empty());
    }

    #[test]
    fn only_a_signed_index_not_older_than_the_last_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let pair = test_key();
        let mut store = Store::open(dir.path()).unwrap();
        let mut newer = index(vec![package("bigmap", "1.0.0", V1)]);
        newer.serial = 5;
        let (json, signature) = signed(&newer, &pair);
        let served = Served::default();
        *served.index.borrow_mut() = Some((json.clone(), signature.clone()));
        assert_eq!(store.refresh(&served, &keys(&pair)).unwrap(), newer);
        assert_eq!(store.cached_index(&keys(&pair)).unwrap(), newer);
        // Unsigned, tampered, another key.
        assert!(matches!(
            store.accept_index(&json, b"", &keys(&pair)),
            Err(StoreError::Index(IndexError::BadSignature))
        ));
        assert!(matches!(
            store.accept_index(&json, &signature, &keys(&test_key())),
            Err(StoreError::Index(IndexError::BadSignature))
        ));
        assert!(matches!(
            store.cached_index(&keys(&test_key())),
            Err(StoreError::Index(IndexError::BadSignature))
        ));
        // An older index, signed, is a replay.
        let mut older = newer.clone();
        older.serial = 4;
        let (json, signature) = signed(&older, &pair);
        assert!(matches!(
            store.accept_index(&json, &signature, &keys(&pair)),
            Err(StoreError::OlderIndex {
                offered: 4,
                seen: 5
            })
        ));
        assert_eq!(store.cached_index(&keys(&pair)).unwrap().serial, 5);
    }

    #[test]
    fn a_different_index_cannot_reuse_an_accepted_serial() {
        let dir = tempfile::tempdir().unwrap();
        let pair = test_key();
        let mut store = Store::open(dir.path()).unwrap();
        let mut first = index(vec![package("bigmap", "1.0.0", V1)]);
        first.serial = 5;
        let (json, signature) = signed(&first, &pair);
        store.accept_index(&json, &signature, &keys(&pair)).unwrap();

        let mut changed = first;
        changed.packages[0].description = "changed without a serial bump".into();
        let (json, signature) = signed(&changed, &pair);
        assert!(store.accept_index(&json, &signature, &keys(&pair)).is_err());
        assert_eq!(
            store.cached_index(&keys(&pair)).unwrap().packages[0].description,
            ""
        );
    }

    #[test]
    fn a_cached_index_written_before_a_crash_recovers_the_serial_floor() {
        let dir = tempfile::tempdir().unwrap();
        let pair = test_key();
        let mut store = Store::open(dir.path()).unwrap();
        let mut first = index(vec![package("bigmap", "1.0.0", V1)]);
        first.serial = 5;
        let (old_json, old_signature) = signed(&first, &pair);
        store
            .accept_index(&old_json, &old_signature, &keys(&pair))
            .unwrap();

        // Simulate power loss after the new index/signature were renamed but
        // before the registry's serial floor was saved.
        let mut newer = index(vec![package("bigmap", "2.0.0", V2)]);
        newer.serial = 6;
        let (new_json, new_signature) = signed(&newer, &pair);
        fs::write(dir.path().join(index::INDEX_FILE), new_json).unwrap();
        fs::write(dir.path().join(index::SIGNATURE_FILE), new_signature).unwrap();
        drop(store);

        let mut recovered = Store::open(dir.path()).unwrap();
        assert_eq!(recovered.cached_index(&keys(&pair)).unwrap().serial, 6);
        assert_eq!(recovered.registry().serial, 6);
        assert!(matches!(
            recovered.accept_index(&old_json, &old_signature, &keys(&pair)),
            Err(StoreError::OlderIndex {
                offered: 5,
                seen: 6
            })
        ));
    }

    #[test]
    fn uninstalling_leaves_nothing_of_the_package() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("native-mods");
        let served = Served::default();
        served.serve("bigmap", "1.0.0", V1);
        served.serve("bigmap", "2.0.0", V2);
        let mut store = Store::open(&root).unwrap();
        let first = index(vec![package("bigmap", "1.0.0", V1)]);
        store.install(context(&first), "bigmap", &served).unwrap();
        let second = index(vec![
            package("bigmap", "1.0.0", V1),
            package("bigmap", "2.0.0", V2),
        ]);
        store.install(context(&second), "bigmap", &served).unwrap();
        store.set_enabled("bigmap", true).unwrap();
        store.write_enabled(BUILD).unwrap();
        // A file someone else put there.
        fs::write(root.join("bigmap/2.0.0/data/mine.txt"), b"mine").unwrap();
        store.uninstall("bigmap").unwrap();
        assert!(store.registry().packages.is_empty());
        assert_eq!(
            tree(&root.join("bigmap")),
            ["2.0.0", "2.0.0/data", "2.0.0/data/mine.txt"]
        );
        fs::remove_dir_all(root.join("bigmap")).unwrap();

        // Without it, nothing of the package is left at all.
        store.install(context(&second), "bigmap", &served).unwrap();
        store.uninstall("bigmap").unwrap();
        assert!(!root.join("bigmap").exists(), "{:?}", tree(&root));
        assert!(matches!(
            store.uninstall("bigmap"),
            Err(StoreError::NotInstalled(_))
        ));
    }

    #[test]
    fn an_upgrade_keeps_the_old_version_until_the_new_one_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let served = Served::default();
        served.serve("bigmap", "1.0.0", V1);
        let mut store = Store::open(dir.path()).unwrap();
        let first = index(vec![package("bigmap", "1.0.0", V1)]);
        store.install(context(&first), "bigmap", &served).unwrap();
        store
            .set_setting("bigmap", "octree_depth", Setting::Int(9))
            .unwrap_err();

        // Version 2's server hands out a damaged file.
        let second = index(vec![
            package("bigmap", "1.0.0", V1),
            package("bigmap", "2.0.0", V2),
        ]);
        served.serve("bigmap", "2.0.0", V2);
        served.files.borrow_mut().insert(
            url_of("bigmap", "2.0.0", "data/new.toml"),
            b"new = 1!!!".to_vec(),
        );
        assert!(store.install(context(&second), "bigmap", &served).is_err());
        assert_eq!(store.registry().packages["bigmap"].current, "1.0.0");
        store.verify("bigmap").unwrap();
        assert!(!store.folder("bigmap", "2.0.0").exists());

        // Fixed: 2 in use, 1 kept for a rollback.
        served.serve("bigmap", "2.0.0", V2);
        store.install(context(&second), "bigmap", &served).unwrap();
        let installed = &store.registry().packages["bigmap"];
        assert_eq!(
            (installed.current.as_str(), installed.previous.as_deref()),
            ("2.0.0", Some("1.0.0"))
        );
        assert!(store.folder("bigmap", "1.0.0").exists());

        store.rollback("bigmap").unwrap();
        let installed = &store.registry().packages["bigmap"];
        assert_eq!(
            (installed.current.as_str(), installed.previous.as_deref()),
            ("1.0.0", Some("2.0.0"))
        );
        store.verify("bigmap").unwrap();

        // A third version: the one before the previous goes.
        const V3: &[(&str, &[u8])] = &[("mod/tpf3mp_bigmap_1/mod.lua", b"-- version 3")];
        served.serve("bigmap", "3.0.0", V3);
        let third = index(vec![
            package("bigmap", "1.0.0", V1),
            package("bigmap", "2.0.0", V2),
            package("bigmap", "3.0.0", V3),
        ]);
        store.install(context(&third), "bigmap", &served).unwrap();
        let installed = &store.registry().packages["bigmap"];
        assert_eq!(
            (installed.current.as_str(), installed.previous.as_deref()),
            ("3.0.0", Some("1.0.0"))
        );
        assert!(!store.folder("bigmap", "2.0.0").exists());
    }

    #[test]
    fn a_rollback_to_damaged_files_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let served = Served::default();
        served.serve("bigmap", "1.0.0", V1);
        served.serve("bigmap", "2.0.0", V2);
        let mut store = Store::open(dir.path()).unwrap();
        let both = index(vec![
            package("bigmap", "1.0.0", V1),
            package("bigmap", "2.0.0", V2),
        ]);
        let first = index(vec![package("bigmap", "1.0.0", V1)]);
        assert!(matches!(
            store.rollback("bigmap"),
            Err(StoreError::NotInstalled(_))
        ));
        store.install(context(&first), "bigmap", &served).unwrap();
        assert!(matches!(
            store.rollback("bigmap"),
            Err(StoreError::NoPrevious(_))
        ));
        store.install(context(&both), "bigmap", &served).unwrap();
        fs::write(
            store.folder("bigmap", "1.0.0").join("data/sizes.toml"),
            b"x",
        )
        .unwrap();
        assert!(matches!(
            store.rollback("bigmap"),
            Err(StoreError::Damaged { .. })
        ));
        assert_eq!(store.registry().packages["bigmap"].current, "2.0.0");
    }

    #[test]
    fn a_needed_package_stays_and_dependencies_install_first() {
        let dir = tempfile::tempdir().unwrap();
        let served = Served::default();
        served.serve("base", "1.0.0", V1);
        served.serve("addon", "1.0.0", &[("addon.lua", b"--")]);
        let mut addon = package("addon", "1.0.0", &[("addon.lua", b"--")]);
        addon.depends.push(Dependency {
            id: "base".into(),
            version: "^1".into(),
        });
        let index = index(vec![package("base", "1.0.0", V1), addon]);
        let mut store = Store::open(dir.path()).unwrap();
        let done = store.install(context(&index), "addon", &served).unwrap();
        assert_eq!(done, ["base 1.0.0", "addon 1.0.0"]);
        assert!(matches!(
            store.uninstall("base"),
            Err(StoreError::Needed { .. })
        ));
        store.uninstall("addon").unwrap();
        store.uninstall("base").unwrap();
        assert!(tree(dir.path()).iter().all(|f| f.starts_with(REGISTRY)));
    }

    #[test]
    fn a_folder_left_by_a_cut_install_is_taken_only_if_it_is_the_package() {
        let dir = tempfile::tempdir().unwrap();
        let served = Served::default();
        let index = index(vec![package("bigmap", "1.0.0", V1)]);
        let mut store = Store::open(dir.path()).unwrap();
        let folder = store.folder("bigmap", "1.0.0");
        for (path, bytes) in V1 {
            let file = folder.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, bytes).unwrap();
        }
        // Nothing is served: the folder is taken as it is.
        store.install(context(&index), "bigmap", &served).unwrap();
        assert_eq!(store.registry().packages["bigmap"].current, "1.0.0");

        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let folder = store.folder("bigmap", "1.0.0");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("other.txt"), b"?").unwrap();
        assert!(matches!(
            store.install(context(&index), "bigmap", &served),
            Err(StoreError::Occupied(_))
        ));
        assert!(folder.join("other.txt").exists());
    }

    #[test]
    fn a_same_version_install_replaces_removed_files_and_preserves_untracked_extras() {
        const RENAMED: &[(&str, &[u8])] = &[
            ("mod/tpf3mp_bigmap_1/mod.lua", b"-- version 1"),
            ("data/renamed.toml", b"renamed = true"),
        ];

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("native-mods");
        let folder = root.join("bigmap/1.0.0");
        let served = Served::default();
        served.serve("bigmap", "1.0.0", V1);
        served.serve("bigmap", "1.0.0", RENAMED);
        let old_index = index(vec![package("bigmap", "1.0.0", V1)]);
        let new_index = index(vec![package("bigmap", "1.0.0", RENAMED)]);
        let mut store = Store::open(&root).unwrap();
        store
            .install(context(&old_index), "bigmap", &served)
            .unwrap();

        let user_file = folder.join("data/renamed.toml");
        fs::write(&user_file, b"keep this").unwrap();
        assert!(matches!(
            store.install_exact(context(&new_index), "bigmap", "1.0.0", &served),
            Err(StoreError::Occupied(_))
        ));
        assert_eq!(fs::read(&user_file).unwrap(), b"keep this");
        assert!(folder.join("data/sizes.toml").exists());

        fs::remove_file(user_file).unwrap();
        store
            .install_exact(context(&new_index), "bigmap", "1.0.0", &served)
            .unwrap();

        assert_eq!(
            tree(&folder),
            [
                "data",
                "data/renamed.toml",
                "mod",
                "mod/tpf3mp_bigmap_1",
                "mod/tpf3mp_bigmap_1/mod.lua"
            ]
        );
        assert!(!folder.join("data/sizes.toml").exists());
        assert_eq!(
            store.registry().packages["bigmap"].package().unwrap().files,
            package("bigmap", "1.0.0", RENAMED).files
        );
        store.verify("bigmap").unwrap();
    }

    #[test]
    fn enabled_files_must_still_match_their_signed_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let served = Served::default();
        served.serve("bigmap", "1.0.0", V1);
        let index = index(vec![package("bigmap", "1.0.0", V1)]);
        let mut store = Store::open(dir.path()).unwrap();
        store.install(context(&index), "bigmap", &served).unwrap();
        store.set_enabled("bigmap", true).unwrap();
        fs::write(
            store.folder("bigmap", "1.0.0").join("data/sizes.toml"),
            b"tampered simulation data",
        )
        .unwrap();

        assert!(matches!(
            store.write_enabled(BUILD),
            Err(StoreError::Damaged { id, files, .. })
                if id == "bigmap" && files == ["data/sizes.toml"]
        ));
        assert!(!dir.path().join(enabled::FILE).exists());
    }

    #[test]
    fn the_hook_gets_the_enabled_packages_for_its_build_with_settings() {
        let dir = tempfile::tempdir().unwrap();
        let served = Served::default();
        let files: &[(&str, &[u8])] = &[("mod/tpf3mp_bigmap_1/mod.lua", b"-- ")];
        served.serve("bigmap", "0.3.0", files);
        let index = index(vec![big_maps()]);
        let mut store = Store::open(dir.path()).unwrap();
        store.install(context(&index), "bigmap", &served).unwrap();

        let (path, _) = store.write_enabled(BUILD).unwrap();
        assert!(
            enabled::read(&path).unwrap().packages.is_empty(),
            "not enabled yet"
        );

        store.set_enabled("bigmap", true).unwrap();
        store
            .set_setting("bigmap", "octree_depth", Setting::Int(11))
            .unwrap();
        assert!(matches!(
            store.set_setting("bigmap", "octree_depth", Setting::Bool(true)),
            Err(StoreError::Setting(_))
        ));
        assert!(matches!(
            store.set_setting("bigmap", "nonsense", Setting::Int(1)),
            Err(StoreError::Setting(_))
        ));
        let (path, left_out) = store.write_enabled(BUILD).unwrap();
        assert!(left_out.is_empty());
        let list = enabled::read(&path).unwrap();
        let package = &list.packages[0];
        assert_eq!(package.settings["octree_depth"], Setting::Int(11));
        assert_eq!(package.settings["street_raster"], Setting::Bool(true));
        assert_eq!(package.root, store.folder("bigmap", "0.3.0"));
        let plan = enabled::plan(&list, BUILD, BIG_MAPS, |_| true, true);
        assert!(plan.feature("bigmap.octree").is_some());

        // Big Maps changes the simulation: this build mismatch prevents the
        // launcher from starting instead of producing an empty enabled list.
        assert!(matches!(
            store.write_enabled(OTHER_BUILD),
            Err(StoreError::SimulationPackageNotForBuild { .. })
        ));
    }

    #[test]
    fn packages_load_after_their_dependencies_and_never_clash() {
        use crate::features::{FeatureSpec, replace};
        const REGISTRY: &[FeatureSpec] = &[
            FeatureSpec {
                id: "base.own",
                effect: Effect::Ui,
                targets: &[replace("ui::page")],
            },
            FeatureSpec {
                id: "other.own",
                effect: Effect::Ui,
                targets: &[replace("ui::page")],
            },
        ];
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let mut add = |id: &str, depends: &[&str], features: &[&str]| {
            let mut package = package(id, "1.0.0", &[]);
            package.simulation = false;
            package.features = features.iter().map(|f| (*f).to_owned()).collect();
            package.depends = depends
                .iter()
                .map(|d| Dependency {
                    id: (*d).to_owned(),
                    version: "*".into(),
                })
                .collect();
            store.registry.packages.insert(
                id.to_owned(),
                Installed {
                    current: "1.0.0".into(),
                    previous: None,
                    enabled: false,
                    settings: BTreeMap::new(),
                    versions: BTreeMap::from([("1.0.0".to_owned(), package)]),
                },
            );
        };
        add("aaa_needs", &["zzz_base"], &[]);
        add("zzz_base", &[], &["base.own"]);
        add("mmm_other", &[], &["other.own"]);
        store.set_enabled_in(REGISTRY, "aaa_needs", true).unwrap();
        store.set_enabled_in(REGISTRY, "zzz_base", true).unwrap();
        assert_eq!(store.load_order(), ["zzz_base", "aaa_needs"]);
        let (list, _) = store.enabled_for(BUILD).unwrap();
        let ids: Vec<&str> = list.packages.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            ["zzz_base", "aaa_needs"],
            "the game gets them in that order"
        );

        // Both take the page over: the second is refused, and says why.
        assert!(store.clash(REGISTRY, "mmm_other").is_some());
        let refused = store.set_enabled_in(REGISTRY, "mmm_other", true);
        assert!(
            matches!(&refused, Err(StoreError::Clash { id, why }) if id == "mmm_other" && why.contains("zzz_base")),
            "{refused:?}"
        );
        assert!(!store.registry.packages["mmm_other"].enabled);
        // Switching off is never refused; then the other may go on.
        store.set_enabled_in(REGISTRY, "zzz_base", false).unwrap();
        store.set_enabled_in(REGISTRY, "mmm_other", true).unwrap();
    }

    #[test]
    fn game_mods_go_into_the_games_mods_while_their_package_is_on() {
        let dir = tempfile::tempdir().unwrap();
        let served = Served::default();
        let files: &[(&str, &[u8])] = &[
            ("mod/tpf3mp_bigmap_1/mod.lua", b"-- big"),
            ("mod/tpf3mp_bigmap_1/content/gui/page.tl", b"-- page"),
        ];
        served.serve("bigmap", "0.3.0", files);
        let mut package = big_maps();
        package.files = files
            .iter()
            .map(|(path, bytes)| FileEntry {
                path: (*path).to_owned(),
                url: url_of("bigmap", "0.3.0", path),
                size: bytes.len() as u64,
                sha256: crate::signed::sha256_hex(bytes),
            })
            .collect();
        package.game_mods = vec!["mod/tpf3mp_bigmap_1".into()];
        assert_eq!(package.validate(), Ok(()));
        let index = index(vec![package]);
        let mut store = Store::open(dir.path().join("native")).unwrap();
        store.install(context(&index), "bigmap", &served).unwrap();
        let mods = dir.path().join("game").join("mods");
        fs::create_dir_all(mods.join("players_own_1")).unwrap();

        assert!(
            store.sync_game_mods(BUILD, &mods).unwrap().is_empty(),
            "not on yet"
        );
        store.set_enabled("bigmap", true).unwrap();
        let done = store.sync_game_mods(BUILD, &mods).unwrap();
        assert_eq!(done, ["game mod tpf3mp_bigmap_1 from bigmap 0.3.0"]);
        assert_eq!(
            fs::read(mods.join("tpf3mp_bigmap_1/content/gui/page.tl")).unwrap(),
            b"-- page"
        );
        assert!(
            store.sync_game_mods(BUILD, &mods).unwrap().is_empty(),
            "already there"
        );

        store.set_enabled("bigmap", false).unwrap();
        let done = store.sync_game_mods(BUILD, &mods).unwrap();
        assert_eq!(done, ["game mod tpf3mp_bigmap_1 taken away"]);
        assert!(!mods.join("tpf3mp_bigmap_1").exists());
        assert!(
            mods.join("players_own_1").exists(),
            "a player's own is never touched"
        );

        // A player's own of the same name: refused, and left alone.
        fs::create_dir_all(mods.join("tpf3mp_bigmap_1")).unwrap();
        store.set_enabled("bigmap", true).unwrap();
        assert!(matches!(
            store.sync_game_mods(BUILD, &mods),
            Err(StoreError::Occupied(_))
        ));
        assert!(mods.join("tpf3mp_bigmap_1").exists());
    }

    #[test]
    fn an_unreadable_registry_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(REGISTRY), b"{ broken").unwrap();
        assert!(matches!(
            Store::open(dir.path()),
            Err(StoreError::Registry(_))
        ));
        assert_eq!(fs::read(dir.path().join(REGISTRY)).unwrap(), b"{ broken");
    }

    #[test]
    fn a_registry_with_a_traversal_path_is_refused_before_uninstall() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("native-mods");
        fs::create_dir_all(&root).unwrap();
        let outside = dir.path().join("keep.txt");
        fs::write(&outside, b"keep me").unwrap();

        let package = package("bad", "1.0.0", &[("../../keep.txt", b"keep me")]);
        let installed = Installed {
            current: "1.0.0".into(),
            previous: None,
            enabled: false,
            settings: Default::default(),
            versions: [("1.0.0".into(), package)].into(),
        };
        let registry = Registry {
            format: REGISTRY_FORMAT,
            serial: 0,
            index_sha256: None,
            packages: [("bad".into(), installed)].into(),
        };
        fs::write(root.join(REGISTRY), serde_json::to_vec(&registry).unwrap()).unwrap();

        assert!(matches!(Store::open(&root), Err(StoreError::Registry(_))));
        assert_eq!(fs::read(outside).unwrap(), b"keep me");
    }

    #[cfg(unix)]
    #[test]
    fn package_operations_refuse_symlinked_parent_directories() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("native-mods");
        let served = Served::default();
        served.serve("bigmap", "1.0.0", V1);
        let package_index = index(vec![package("bigmap", "1.0.0", V1)]);
        let mut store = Store::open(&root).unwrap();
        store
            .install(context(&package_index), "bigmap", &served)
            .unwrap();

        let outside = dir.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("sizes.toml"), b"sizes = [128]").unwrap();
        let data = store.folder("bigmap", "1.0.0").join("data");
        fs::remove_dir_all(&data).unwrap();
        symlink(&outside, &data).unwrap();

        assert!(store.verify("bigmap").is_err());
        assert!(store.uninstall("bigmap").is_err());
        assert_eq!(
            fs::read(outside.join("sizes.toml")).unwrap(),
            b"sizes = [128]"
        );
        assert!(
            store
                .folder("bigmap", "1.0.0")
                .join("mod/tpf3mp_bigmap_1/mod.lua")
                .exists()
        );
        assert!(store.registry().packages.contains_key("bigmap"));
    }

    #[test]
    fn a_simulation_package_for_another_build_is_not_written_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("native-mods");
        let mut store = Store::open(&root).unwrap();
        let mut package = package("bigmap", "1.0.0", &[]);
        package.simulation = true;
        package.builds = vec![OTHER_BUILD.into()];
        store.registry.packages.insert(
            "bigmap".into(),
            Installed {
                current: "1.0.0".into(),
                previous: None,
                enabled: true,
                settings: Default::default(),
                versions: [("1.0.0".into(), package)].into(),
            },
        );

        assert!(store.write_enabled(BUILD).is_err());
        assert!(!root.join(enabled::FILE).exists());
    }

    #[test]
    fn a_ui_only_package_for_another_build_is_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("native-mods");
        let mut store = Store::open(&root).unwrap();
        let mut package = package("pages", "1.0.0", &[]);
        package.simulation = false;
        package.builds = vec![OTHER_BUILD.into()];
        store.registry.packages.insert(
            "pages".into(),
            Installed {
                current: "1.0.0".into(),
                previous: None,
                enabled: true,
                settings: Default::default(),
                versions: [("1.0.0".into(), package)].into(),
            },
        );

        let (enabled, left_out) = store.enabled_for(BUILD).unwrap();
        assert_eq!(left_out, ["pages"]);
        assert!(enabled.packages.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn store_open_allows_a_symlinked_parent_but_refuses_a_linked_root() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir_all(outside.join("native-mods")).unwrap();
        fs::write(
            outside.join("native-mods").join(REGISTRY),
            br#"{"format":1,"serial":0,"packages":{}}"#,
        )
        .unwrap();
        let link = dir.path().join("store-link");
        symlink(&outside, &link).unwrap();

        // macOS legitimately reaches temporary data through /var, a link to
        // /private/var. The managed root remains confined under that parent.
        assert!(Store::open(link.join("native-mods")).is_ok());

        let root_link = dir.path().join("linked-native-mods");
        symlink(outside.join("native-mods"), &root_link).unwrap();
        assert!(matches!(
            Store::open(root_link),
            Err(StoreError::UnsafePath(_))
        ));
    }
}
