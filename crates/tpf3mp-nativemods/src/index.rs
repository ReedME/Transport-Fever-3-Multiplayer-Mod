//! The signed index of native mods.
//!
//! `native-mods.json` lists every package the launcher may install, and
//! `native-mods.json.sig` is an Ed25519 signature of its exact bytes with
//! the project's native-mods key ([`crate::signed`]). An index that does not
//! verify, names a format this launcher does not read, or holds one entry
//! that does not check out is refused whole: the project signs it, so a
//! malformed entry is a mistake to fix there, never something to guess
//! around.
//!
//! ```json
//! {
//!   "format": 1,
//!   "serial": 12,
//!   "packages": [{
//!     "id": "bigmap", "version": "0.3.0", "name": "Big Maps",
//!     "simulation": true,
//!     "builds": ["<the game executable's SHA-256>"],
//!     "features": ["bigmap.octree", "bigmap.street_raster", "bigmap.page"],
//!     "settings": {"octree_depth": 10, "street_raster": true},
//!     "depends": [], "conflicts": [],
//!     "files": [{"path": "mod/tpf3mp_bigmap_1/mod.lua",
//!                "url": "https://…", "size": 1234, "sha256": "…"}]
//!   }]
//! }
//! ```
//!
//! `serial` only grows: the launcher remembers the highest it accepted and
//! refuses an older index, so an old signed index cannot be replayed to
//! bring back a withdrawn package.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::signed;

/// The index format this launcher reads.
pub const FORMAT: u32 = 1;
/// The index's file name.
pub const INDEX_FILE: &str = "native-mods.json";
/// Its signature's file name.
pub const SIGNATURE_FILE: &str = "native-mods.json.sig";
/// Largest index read.
pub const MAX_INDEX: u64 = 1 << 20;
/// Largest file a package may ship.
pub const MAX_FILE: u64 = 1 << 30;
/// Most files one package may ship.
pub const MAX_FILES: usize = 4096;
/// Longest package version, so that the room's terms fit a mod version
/// ([`crate::terms`]).
pub const MAX_VERSION: usize = 20;
/// Longest text setting, in bytes: a room tells its native mods' settings.
pub const MAX_TEXT_SETTING: usize = 128;
/// Most authors a package names, and the longest name of one, in bytes.
pub const MAX_AUTHORS: usize = 8;
pub const MAX_AUTHOR: usize = 48;
/// The longest address of a package's preview image.
pub const MAX_PREVIEW: usize = 300;
/// The plugin interface this TPF3-MP's hook offers (`tpf3mp-plugin-abi`).
pub const PLUGIN_ABI: u32 = tpf3mp_plugin_abi::ABI;
/// The platforms a plugin may be built for.
pub const PLATFORMS: &[&str] = &["windows-x86_64", "linux-x86_64"];
/// Most hooks a package declares.
pub const MAX_HOOKS: usize = 64;
/// Most bytes a hook's signature names, and most bytes a hook expects at its site.
pub const MAX_SIGNATURE: usize = 128;
pub const MAX_EXPECTED: usize = 64;

/// The platform this TPF3-MP can load native plugins on, as plugins name it;
/// none where the hook cannot load plugins.
pub const fn current_platform() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("windows-x86_64")
    } else {
        None
    }
}

/// The signed list of native mods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Index {
    pub format: u32,
    /// Grows with every index published; an older one is refused.
    pub serial: u64,
    pub packages: Vec<Package>,
    /// What the mods' hosts say of each mod, by id: a server's index has
    /// them, read from its registry's mods' repositories. Shown to players,
    /// never used to choose what is installed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub listings: BTreeMap<String, Listing>,
}

/// What a mod's host says of it: how often its releases were downloaded
/// and when it last changed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listing {
    /// Downloads of every release's files, all versions together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloads: Option<u64>,
    /// When it last changed, as `YYYY-MM-DD`: its newest release, or its
    /// repository's last push.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
}

/// One version of one native mod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    /// Lowercase letters, digits and `_`.
    pub id: String,
    /// Semantic version.
    pub version: String,
    /// What players are shown.
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// The mod's own project page, such as its repository, where its
    /// authors publish it and players read about it: the launcher links to
    /// it, as CKAN links to a mod's. Its files still come only as this
    /// signed index pins them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// Who made it, as players are shown them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    /// An image players are shown of it, PNG or JPEG: an `https` address.
    /// Only ever shown, never installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// Whether it changes how the world runs. If so, every game of a room
    /// must run it in the same version with the same settings
    /// ([`crate::terms`]). A feature the hook knows to change the
    /// simulation makes the package count as changing it too.
    pub simulation: bool,
    /// The game builds it runs on, by the executable's SHA-256, as hook
    /// profiles pin them. On any other build it is neither installed nor
    /// enabled.
    pub builds: Vec<String>,
    /// The hook features it enables ([`crate::features`]).
    #[serde(default)]
    pub features: Vec<String>,
    /// Its settings and their defaults. A player's settings must name
    /// these and keep their types.
    #[serde(default)]
    pub settings: BTreeMap<String, Setting>,
    #[serde(default)]
    pub depends: Vec<Dependency>,
    /// Packages that must not be installed beside it.
    #[serde(default)]
    pub conflicts: Vec<String>,
    /// Its data, Lua and settings files.
    #[serde(default)]
    pub files: Vec<FileEntry>,
    /// Native libraries the hook loads: the package's own code
    /// (proposed D29 amendment, docs/NATIVE_MODS.md, "Plugins"). A package
    /// with any changes the simulation as far as anyone can tell, and must
    /// say so.
    #[serde(default)]
    pub plugins: Vec<Plugin>,
    /// The game functions its plugins patch, each found by its signature,
    /// as the hook finds its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hooks: Vec<HookDecl>,
    /// Folders of its files that are game mods (`mod/<name>`, the name a
    /// game mod's folder has, such as `tpf3mp_bigmap_1`): the launcher puts
    /// each into the game's `mods` folder while the package is on, so the
    /// game can load its Lua and pages as `<name>::/...`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub game_mods: Vec<String>,
}

/// A place in the game's code a package patches: found by signature in the
/// running build, checked against the bytes it expects there, and patched
/// the way its `kind` says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookDecl {
    /// `<package id>::<name>`: the name the plugin attaches to.
    pub target: String,
    /// How it is patched.
    #[serde(default)]
    pub kind: HookKind,
    /// An IDA-style signature: bytes in hex, `?` for any, one match in the
    /// game's code.
    pub signature: String,
    /// From the match to the site, in bytes.
    #[serde(default)]
    pub offset: i64,
    /// The bytes the site must hold, in hex, checked before it is patched:
    /// a function's first bytes, a splice's bytes, those a rewrite replaces,
    /// a `call`, or the instruction that addresses a slot's table.
    pub expected: String,
    /// How the plugin shares it with others.
    #[serde(rename = "use", default)]
    pub access: HookUse,
    /// A splice's: how many of the expected bytes the jump takes (whole
    /// instructions, 5 to 32), which then run after the plugins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steal: Option<u8>,
    /// A rewrite's bytes, in hex, as long as `expected`, when the package
    /// gives them itself; otherwise its plugin does when it starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
    /// A slot's: where the table is, and which entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<SlotDecl>,
}

/// How a hook patches the game.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookKind {
    /// At a function's start: the plugin's function runs in its place and
    /// calls on to what was there.
    #[default]
    Detour,
    /// In the middle of a function: the plugin's callback gets every
    /// register and may change them; then the stolen instructions run and
    /// the function goes on.
    Splice,
    /// Bytes replaced one for one: an operand, a constant, a size.
    Rewrite,
    /// One `call` instruction made to call the plugin's function, which
    /// calls on to the old callee.
    Call,
    /// One entry of a table of function pointers, such as a vtable, made
    /// to point at the plugin's function, which calls on to the old one.
    Slot,
    /// Nothing written: a function or bytes the plugin only needs to find
    /// and check (one it calls, one whose code it relies on).
    Find,
}

/// Where a slot's table is: the expected instruction addresses it relative
/// to the next instruction (`lea reg, [rip + disp32]`), its 4-byte
/// displacement `disp_at` bytes in, the instruction `length` bytes long;
/// the slot is entry `index` of the table, 8 bytes each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotDecl {
    pub disp_at: u8,
    pub length: u8,
    pub index: u16,
}

/// How a plugin shares what it patches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookUse {
    /// It runs and then lets what was there run: several plugins all run,
    /// in the load order.
    #[default]
    Chain,
    /// It takes it over: no other may patch it.
    Replace,
}

impl HookDecl {
    /// The expected bytes, decoded; none if they are not hex.
    pub fn expected_bytes(&self) -> Option<Vec<u8>> {
        hex_bytes(&self.expected)
    }

    /// How many bytes from the site the patch writes: what may not overlap
    /// another's. A slot's are in its table, not at the site.
    pub fn written_len(&self) -> usize {
        match self.kind {
            HookKind::Detour => 14,
            HookKind::Splice => usize::from(self.steal.unwrap_or(5)),
            HookKind::Rewrite => self.expected.trim().len() / 2,
            HookKind::Call => 5,
            HookKind::Slot => 8,
            HookKind::Find => 0,
        }
    }
}

/// `text` as bytes, two hex digits each.
pub fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

/// A setting's value: what a package's defaults and a player's choices
/// hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Setting {
    Bool(bool),
    Int(i64),
    Text(String),
}

impl Setting {
    /// Whether `other` is a value of the same type.
    pub fn same_type(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

/// A package another needs, in versions matching `version` (a semver
/// requirement such as `^1.2`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    pub id: String,
    pub version: String,
}

/// One file of a package: where it goes in the package's folder, where it
/// comes from, and what it must be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    /// Plain names separated by `/`, inside the package's folder.
    pub path: String,
    pub url: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

/// A native library the hook loads into the game: pinned in the index by
/// size and SHA-256 like every file, built for one platform against the
/// hook's plugin interface `abi` ([`PLUGIN_ABI`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plugin {
    pub path: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub abi: u32,
    /// One of [`PLATFORMS`].
    pub platform: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IndexError {
    #[error("the native-mods index is not signed with the project's key")]
    BadSignature,
    #[error("the native-mods index is malformed: {0}")]
    Malformed(String),
    #[error("the native-mods index has format {0}; this launcher reads format {FORMAT}")]
    Format(u32),
}

impl Index {
    /// The index in `json`, if `signature` is one of `keys`' signatures of
    /// it and every entry checks out.
    pub fn verified(json: &[u8], signature: &[u8], keys: &[Vec<u8>]) -> Result<Self, IndexError> {
        if !signed::verify(json, signature, keys) {
            return Err(IndexError::BadSignature);
        }
        Self::parse(json)
    }

    /// The index in `json` if every entry checks out, whoever vouches for
    /// it: the project's signature ([`Self::verified`]) or the server that
    /// served it ([`crate::registry`]).
    pub fn parse(json: &[u8]) -> Result<Self, IndexError> {
        let index: Self = serde_json::from_slice(json)
            .map_err(|error| IndexError::Malformed(error.to_string()))?;
        if index.format != FORMAT {
            return Err(IndexError::Format(index.format));
        }
        index.validate().map_err(IndexError::Malformed)?;
        Ok(index)
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        for (id, listing) in &self.listings {
            if !is_id(id) {
                return Err(format!("the listing {id:?} is malformed"));
            }
            if let Some(updated) = &listing.updated
                && !is_date(updated)
            {
                return Err(format!("{id}: the date {updated:?} is not YYYY-MM-DD"));
            }
        }
        let mut seen = BTreeSet::new();
        for package in &self.packages {
            package
                .validate()
                .map_err(|problem| format!("{} {}: {problem}", package.id, package.version))?;
            if !seen.insert((package.id.as_str(), package.version.as_str())) {
                return Err(format!(
                    "{} {} is listed twice",
                    package.id, package.version
                ));
            }
        }
        Ok(())
    }

    /// The package `id` in `version`.
    pub fn package(&self, id: &str, version: &str) -> Option<&Package> {
        self.packages
            .iter()
            .find(|p| p.id == id && p.version == version)
    }

    /// Every version of `id`, newest first.
    pub fn versions(&self, id: &str) -> Vec<&Package> {
        let mut versions: Vec<&Package> = self.packages.iter().filter(|p| p.id == id).collect();
        versions.sort_by_key(|p| std::cmp::Reverse(p.semver()));
        versions
    }
}

impl Package {
    /// Its version, parsed; validated packages always have one.
    pub fn semver(&self) -> semver::Version {
        semver::Version::parse(&self.version).unwrap_or(semver::Version::new(0, 0, 0))
    }

    /// Whether it is pinned to the game build whose executable hashes to
    /// `build`.
    pub fn runs_on(&self, build: &str) -> bool {
        self.builds.iter().any(|pinned| pinned == build)
    }

    /// Its plugins built for `platform`.
    pub fn plugins_for(&self, platform: &str) -> Vec<&Plugin> {
        self.plugins
            .iter()
            .filter(|plugin| plugin.platform == platform)
            .collect()
    }

    /// The files installed for it here: its files, and its plugins for
    /// this platform.
    pub fn local_files(&self) -> Vec<FileEntry> {
        let plugins = current_platform()
            .map(|platform| self.plugins_for(platform))
            .unwrap_or_default();
        self.files
            .iter()
            .cloned()
            .chain(plugins.into_iter().map(|plugin| FileEntry {
                path: plugin.path.clone(),
                url: plugin.url.clone(),
                size: plugin.size,
                sha256: plugin.sha256.clone(),
            }))
            .collect()
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if !is_id(&self.id) {
            return Err("the id is not lowercase letters, digits and _".into());
        }
        if self.version.len() > MAX_VERSION || semver::Version::parse(&self.version).is_err() {
            return Err("the version is not a short semantic version".into());
        }
        if self.name.trim().is_empty() || self.name.len() > 64 {
            return Err("the name is empty or too long".into());
        }
        if self.builds.is_empty() || !self.builds.iter().all(|b| signed::is_sha256(b)) {
            return Err("it pins no game build, or one that is not a SHA-256".into());
        }
        if let Some(bad) = self.features.iter().find(|f| !is_feature_id(f)) {
            return Err(format!("the feature id {bad:?} is malformed"));
        }
        if let Some(bad) = self.settings.keys().find(|k| !is_id(k)) {
            return Err(format!("the setting {bad:?} is malformed"));
        }
        // A room tells every member its settings (protocol 20's
        // `NativeValue::Text`).
        if let Some((bad, _)) = self.settings.iter().find(
            |(_, value)| matches!(value, Setting::Text(text) if text.len() > MAX_TEXT_SETTING),
        ) {
            return Err(format!(
                "the setting {bad:?} is longer than {MAX_TEXT_SETTING} bytes"
            ));
        }
        if let Some(homepage) = &self.homepage
            && (!homepage.starts_with("https://") || homepage.len() > 200)
        {
            return Err("the homepage is not a short https address".into());
        }
        if self.authors.len() > MAX_AUTHORS
            || self.authors.iter().any(|author| {
                author.trim().is_empty()
                    || author.len() > MAX_AUTHOR
                    || author.chars().any(char::is_control)
            })
        {
            return Err(format!(
                "more than {MAX_AUTHORS} authors, or one empty or longer than {MAX_AUTHOR} bytes"
            ));
        }
        if let Some(preview) = &self.preview
            && (!preview.starts_with("https://")
                || preview.len() > MAX_PREVIEW
                || preview.chars().any(|c| c.is_whitespace() || c.is_control()))
        {
            return Err("the preview is not a short https address".into());
        }
        for dependency in &self.depends {
            if !is_id(&dependency.id) || semver::VersionReq::parse(&dependency.version).is_err() {
                return Err(format!("the dependency {:?} is malformed", dependency.id));
            }
        }
        if let Some(bad) = self.conflicts.iter().find(|c| !is_id(c)) {
            return Err(format!("the conflict {bad:?} is malformed"));
        }
        self.validate_plugins()?;
        for folder in &self.game_mods {
            let name = folder.strip_prefix("mod/").unwrap_or_default();
            let ok = is_game_mod_name(name)
                && self
                    .files
                    .iter()
                    .any(|file| file.path.starts_with(&format!("{folder}/")));
            if !ok {
                return Err(format!(
                    "the game mod {folder:?} is not mod/<name>_<number> holding its files"
                ));
            }
        }
        if self.files.len() > MAX_FILES {
            return Err("too many files".into());
        }
        let mut paths = BTreeSet::new();
        let entries = self
            .files
            .iter()
            .map(|f| (&f.path, &f.url, f.size, &f.sha256))
            .chain(
                self.plugins
                    .iter()
                    .map(|p| (&p.path, &p.url, p.size, &p.sha256)),
            );
        for (path, url, size, sha256) in entries {
            if !is_safe_path(path) {
                return Err(format!("the file path {path:?} is not a plain path"));
            }
            // Windows folds case: two names differing only in case are one
            // file there.
            if !paths.insert(path.to_ascii_lowercase()) {
                return Err(format!("the file {path:?} is listed twice"));
            }
            if !url.starts_with("https://") && !url.starts_with("http://") {
                return Err(format!("the file {path:?} has no web address"));
            }
            if size > MAX_FILE || !signed::is_sha256(sha256) {
                return Err(format!("the file {path:?} is too large or has no SHA-256"));
            }
        }
        Ok(())
    }
}

impl Package {
    fn validate_plugins(&self) -> Result<(), String> {
        if let Some(bad) = self
            .plugins
            .iter()
            .find(|p| !PLATFORMS.contains(&p.platform.as_str()) || p.abi == 0)
        {
            return Err(format!(
                "the plugin {:?} names no known platform or interface",
                bad.path
            ));
        }
        if !self.plugins.is_empty() && !self.simulation {
            return Err(
                "it has plugins, whose code may change anything: it must say it changes the \
                 simulation"
                    .into(),
            );
        }
        // A package of rewrites it gives itself needs no plugin; any other
        // hook is a plugin's.
        let needs_plugin = self
            .hooks
            .iter()
            .any(|hook| hook.kind != HookKind::Rewrite || hook.replacement.is_none());
        if needs_plugin && self.plugins.is_empty() {
            return Err("it declares hooks a plugin must run, but has no plugin".into());
        }
        if !self.hooks.is_empty() && !self.simulation {
            return Err(
                "it patches the game's code, which may change anything: it must say it changes \
                 the simulation"
                    .into(),
            );
        }
        if self.hooks.len() > MAX_HOOKS {
            return Err(format!("more than {MAX_HOOKS} hooks"));
        }
        let mut names = BTreeSet::new();
        for hook in &self.hooks {
            let name = hook
                .target
                .strip_prefix(&format!("{}::", self.id))
                .filter(|name| is_id(name))
                .ok_or_else(|| {
                    format!(
                        "the hook {:?} is not named {}::<lowercase name>",
                        hook.target, self.id
                    )
                })?;
            if !names.insert(name) {
                return Err(format!("the hook {:?} is declared twice", hook.target));
            }
            hook.validate()
                .map_err(|why| format!("the hook {:?}: {why}", hook.target))?;
        }
        Ok(())
    }
}

impl HookDecl {
    fn validate(&self) -> Result<(), String> {
        if !is_signature(&self.signature) {
            return Err(format!(
                "no usable signature: hex bytes and ?, at least 8 and at most {MAX_SIGNATURE}, \
                 4 of them fixed"
            ));
        }
        if self.offset.unsigned_abs() > 4096 {
            return Err("too far from its signature".into());
        }
        let expected = self
            .expected_bytes()
            .filter(|bytes| (1..=MAX_EXPECTED).contains(&bytes.len()))
            .ok_or(format!("no expected bytes, 1 to {MAX_EXPECTED} in hex"))?;
        let none_but = |field: &str, set: bool| {
            if set {
                Err(format!("a {:?} hook has no {field}", self.kind))
            } else {
                Ok(())
            }
        };
        match self.kind {
            HookKind::Detour | HookKind::Find => {
                none_but("steal", self.steal.is_some())?;
                none_but("replacement", self.replacement.is_some())?;
                none_but("slot", self.slot.is_some())?;
            }
            HookKind::Splice => {
                let steal = usize::from(
                    self.steal
                        .ok_or("a splice names how many bytes it steals")?,
                );
                if !(5..=32).contains(&steal) || steal > expected.len() {
                    return Err("a splice steals 5 to 32 of the bytes it expects".into());
                }
                none_but("replacement", self.replacement.is_some())?;
                none_but("slot", self.slot.is_some())?;
            }
            HookKind::Rewrite => {
                if let Some(replacement) = &self.replacement
                    && hex_bytes(replacement).map(|bytes| bytes.len()) != Some(expected.len())
                {
                    return Err("its replacement is not hex as long as what it expects".into());
                }
                none_but("steal", self.steal.is_some())?;
                none_but("slot", self.slot.is_some())?;
            }
            HookKind::Call => {
                if expected.len() != 5 || expected[0] != 0xE8 {
                    return Err("a call hook expects one `call rel32`: E8 and 4 bytes".into());
                }
                none_but("steal", self.steal.is_some())?;
                none_but("replacement", self.replacement.is_some())?;
                none_but("slot", self.slot.is_some())?;
            }
            HookKind::Slot => {
                let slot = self.slot.ok_or("a slot hook says where its table is")?;
                let length = usize::from(slot.length);
                if usize::from(slot.disp_at) + 4 > length
                    || length > 15
                    || expected.len() < length
                    || slot.index > 4096
                {
                    return Err(
                        "its slot's instruction is not one with a 4-byte displacement in what \
                         it expects"
                            .into(),
                    );
                }
                none_but("steal", self.steal.is_some())?;
                none_but("replacement", self.replacement.is_some())?;
            }
        }
        Ok(())
    }
}

/// An IDA-style signature: hex bytes and `?` or `??`, 8 to
/// [`MAX_SIGNATURE`] of them, at least 4 fixed.
pub fn is_signature(text: &str) -> bool {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let fixed = tokens
        .iter()
        .filter(|t| t.len() == 2 && t.bytes().all(|b| b.is_ascii_hexdigit()))
        .count();
    let wild = tokens.iter().filter(|t| matches!(**t, "?" | "??")).count();
    (8..=MAX_SIGNATURE).contains(&tokens.len()) && fixed + wild == tokens.len() && fixed >= 4
}

/// A game mod's folder name: lowercase letters, digits and `_`, ending in
/// `_<number>` (`tpf3mp_bigmap_1`).
pub fn is_game_mod_name(name: &str) -> bool {
    name.rsplit_once('_').is_some_and(|(stem, number)| {
        !stem.is_empty()
            && stem
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            && !number.is_empty()
            && number.bytes().all(|b| b.is_ascii_digit())
    }) && name.len() <= 64
}

/// `YYYY-MM-DD`.
pub fn is_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
}

/// A package or setting id: 1 to 40 lowercase letters, digits and `_`.
pub fn is_id(text: &str) -> bool {
    (1..=40).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// A feature id: ids joined by dots, such as `bigmap.octree`.
pub fn is_feature_id(text: &str) -> bool {
    text.len() <= 64 && text.split('.').all(is_id)
}

/// A path inside a package's folder: plain names joined by `/`, none of
/// them hidden, relative, or holding a drive or another separator.
pub fn is_safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 240
        && path.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && !part.ends_with('.')
                && !part.ends_with(' ')
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b' '))
        })
}

#[cfg(test)]
pub(crate) mod tests {
    use ring::{
        rand::SystemRandom,
        signature::{Ed25519KeyPair, KeyPair},
    };

    use super::*;

    /// A TEST key, made fresh for each test; never a real one.
    pub(crate) fn test_key() -> Ed25519KeyPair {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    }

    pub(crate) fn keys(pair: &Ed25519KeyPair) -> Vec<Vec<u8>> {
        vec![pair.public_key().as_ref().to_vec()]
    }

    pub(crate) const BUILD: &str =
        "1111111111111111111111111111111111111111111111111111111111111111";
    pub(crate) const OTHER_BUILD: &str =
        "2222222222222222222222222222222222222222222222222222222222222222";

    /// A package of `files` (path, bytes), served from `https://test/`.
    pub(crate) fn package(id: &str, version: &str, files: &[(&str, &[u8])]) -> Package {
        Package {
            id: id.into(),
            version: version.into(),
            name: id.into(),
            description: String::new(),
            homepage: None,
            simulation: false,
            builds: vec![BUILD.into()],
            features: Vec::new(),
            settings: BTreeMap::new(),
            depends: Vec::new(),
            conflicts: Vec::new(),
            files: files
                .iter()
                .map(|(path, bytes)| FileEntry {
                    path: (*path).into(),
                    url: url_of(id, version, path),
                    size: bytes.len() as u64,
                    sha256: signed::sha256_hex(bytes),
                })
                .collect(),
            plugins: Vec::new(),
            authors: Vec::new(),
            preview: None,
            hooks: Vec::new(),
            game_mods: Vec::new(),
        }
    }

    pub(crate) fn url_of(id: &str, version: &str, path: &str) -> String {
        format!("https://test/{id}/{version}/{path}")
    }

    pub(crate) fn index(packages: Vec<Package>) -> Index {
        Index {
            format: FORMAT,
            serial: 1,
            packages,
            listings: Default::default(),
        }
    }

    pub(crate) fn signed(index: &Index, pair: &Ed25519KeyPair) -> (Vec<u8>, Vec<u8>) {
        let json = serde_json::to_vec(index).unwrap();
        let signature = pair.sign(&json).as_ref().to_vec();
        (json, signature)
    }

    /// The example in docs/NATIVE_MODS.md, Big Maps as a package.
    pub(crate) fn big_maps() -> Package {
        let mut package = package(
            "bigmap",
            "0.3.0",
            &[("mod/tpf3mp_bigmap_1/mod.lua", b"-- ")],
        );
        package.name = "Big Maps".into();
        package.simulation = true;
        package.features = [
            "bigmap.octree",
            "bigmap.street_raster",
            "bigmap.placement",
            "bigmap.page",
            "bigmap.density",
            "bigmap.memory_gate",
        ]
        .map(String::from)
        .to_vec();
        package.settings = BTreeMap::from([
            ("octree_depth".into(), Setting::Int(10)),
            ("street_raster".into(), Setting::Bool(true)),
        ]);
        package
    }

    #[test]
    fn plugins_and_the_hooks_they_declare_are_checked() {
        let plugin = Plugin {
            path: "plugin/pages.dll".into(),
            url: "https://example.com/pages.dll".into(),
            size: 4096,
            sha256: "a".repeat(64),
            abi: PLUGIN_ABI,
            platform: "windows-x86_64".into(),
        };
        let hook = HookDecl {
            target: "pages::draw".into(),
            signature: "48 89 5C 24 ? 57 48 83 EC 20".into(),
            offset: 0,
            expected: "48895C2408".into(),
            access: HookUse::Chain,
            kind: HookKind::Detour,
            steal: None,
            replacement: None,
            slot: None,
        };
        let mut good = package("pages", "1.0.0", &[]);
        good.simulation = true;
        good.plugins = vec![plugin.clone()];
        good.hooks = vec![hook.clone()];
        assert_eq!(good.validate(), Ok(()));
        assert_eq!(
            good.local_files().len(),
            usize::from(current_platform() == Some("windows-x86_64"))
        );

        let broken = |change: &dyn Fn(&mut Package)| {
            let mut package = good.clone();
            change(&mut package);
            package.validate().is_err()
        };
        assert!(
            broken(&|p| p.simulation = false),
            "code may change anything"
        );
        assert!(broken(&|p| p.plugins[0].platform = "amiga".into()));
        assert!(broken(&|p| p.plugins.clear()), "hooks need a plugin");
        assert!(
            broken(&|p| p.hooks[0].target = "other::draw".into()),
            "its own names"
        );
        assert!(broken(&|p| p.hooks.push(hook.clone())), "declared twice");
        assert!(
            broken(&|p| p.hooks[0].signature = "48 89 ? ?".into()),
            "too short"
        );
        assert!(
            broken(&|p| p.hooks[0].signature = "? ? ? ? ? ? ? ? 48 89".into()),
            "too vague"
        );
        assert!(broken(
            &|p| p.hooks[0].signature = "zz 89 5C 24 ? 57 48 83".into()
        ));
        assert!(broken(&|p| p.hooks[0].expected = "489".into()));
        assert!(broken(&|p| p.hooks[0].offset = 1 << 20));
        // A raw address is no signature.
        assert!(broken(&|p| p.hooks[0].signature = "0x140123456".into()));
    }

    #[test]
    fn each_kind_of_hook_says_what_it_needs() {
        let base = HookDecl {
            target: "big::site".into(),
            kind: HookKind::Detour,
            signature: "48 89 5C 24 ? 57 48 83 EC 20".into(),
            offset: 0,
            expected: "48895C2408".into(),
            access: HookUse::Chain,
            steal: None,
            replacement: None,
            slot: None,
        };
        let check = |change: &dyn Fn(&mut HookDecl)| {
            let mut hook = base.clone();
            change(&mut hook);
            hook.validate()
        };
        assert_eq!(check(&|_| {}), Ok(()));
        // A splice steals whole instructions it expects.
        assert_eq!(
            check(&|h| {
                h.kind = HookKind::Splice;
                h.steal = Some(5);
            }),
            Ok(())
        );
        assert!(check(&|h| h.kind = HookKind::Splice).is_err(), "how many?");
        assert!(
            check(&|h| {
                h.kind = HookKind::Splice;
                h.steal = Some(6);
            })
            .is_err(),
            "more than it expects"
        );
        // A rewrite's bytes are as long as what it replaces.
        assert_eq!(
            check(&|h| {
                h.kind = HookKind::Rewrite;
                h.replacement = Some("48895C2410".into());
            }),
            Ok(())
        );
        assert!(
            check(&|h| {
                h.kind = HookKind::Rewrite;
                h.replacement = Some("90".into());
            })
            .is_err()
        );
        // A call is a call.
        assert_eq!(
            check(&|h| {
                h.kind = HookKind::Call;
                h.expected = "E812345678".into();
            }),
            Ok(())
        );
        assert!(check(&|h| h.kind = HookKind::Call).is_err());
        // A slot names its table's instruction.
        let slot = SlotDecl {
            disp_at: 3,
            length: 7,
            index: 11,
        };
        assert_eq!(
            check(&|h| {
                h.kind = HookKind::Slot;
                h.expected = "488D0512345678".into();
                h.slot = Some(slot);
            }),
            Ok(())
        );
        assert!(
            check(&|h| {
                h.kind = HookKind::Slot;
                h.expected = "488D0512345678".into();
                h.slot = Some(SlotDecl { disp_at: 5, ..slot });
            })
            .is_err(),
            "the displacement runs past the instruction"
        );
        assert!(
            check(&|h| h.steal = Some(5)).is_err(),
            "a detour steals what it needs"
        );

        // Rewrites a package gives itself need no plugin.
        let mut data = package("big", "1.0.0", &[]);
        data.simulation = true;
        data.hooks = vec![HookDecl {
            kind: HookKind::Rewrite,
            replacement: Some("48895C2410".into()),
            ..base.clone()
        }];
        assert_eq!(data.validate(), Ok(()));
        data.simulation = false;
        assert!(
            data.validate().is_err(),
            "patching code changes the simulation"
        );
    }

    #[test]
    fn authors_previews_and_listings_are_checked() {
        let mut good = package("bigmap", "0.3.0", &[]);
        good.authors = vec!["Ann".into()];
        good.preview = Some("https://example.com/bigmap.png".into());
        assert_eq!(good.validate(), Ok(()));
        let mut bad = good.clone();
        bad.preview = Some("http://example.com/bigmap.png".into());
        assert!(bad.validate().is_err(), "https only");
        let mut bad = good.clone();
        bad.authors = vec![" ".into()];
        assert!(bad.validate().is_err());
        let mut bad = good.clone();
        bad.authors = vec!["x".repeat(MAX_AUTHOR + 1)];
        assert!(bad.validate().is_err());
        let mut index = Index {
            format: FORMAT,
            serial: 1,
            packages: vec![good],
            listings: [(
                "bigmap".to_owned(),
                Listing {
                    downloads: Some(3),
                    updated: Some("2026-10-01".into()),
                },
            )]
            .into(),
        };
        assert_eq!(index.validate(), Ok(()));
        index.listings.get_mut("bigmap").unwrap().updated = Some("yesterday".into());
        assert!(index.validate().is_err());
    }

    #[test]
    fn a_signed_index_verifies_and_reads() {
        let pair = test_key();
        let (json, signature) = signed(&index(vec![big_maps()]), &pair);
        let read = Index::verified(&json, &signature, &keys(&pair)).unwrap();
        assert_eq!(read.packages[0].features.len(), 6);
        assert!(read.package("bigmap", "0.3.0").unwrap().runs_on(BUILD));
        assert!(
            !read
                .package("bigmap", "0.3.0")
                .unwrap()
                .runs_on(OTHER_BUILD)
        );
    }

    #[test]
    fn a_tampered_index_is_refused() {
        let pair = test_key();
        let (json, signature) = signed(&index(vec![big_maps()]), &pair);
        let tampered = String::from_utf8(json)
            .unwrap()
            .replace("\"simulation\":true", "\"simulation\":false")
            .into_bytes();
        assert_eq!(
            Index::verified(&tampered, &signature, &keys(&pair)),
            Err(IndexError::BadSignature)
        );
    }

    #[test]
    fn an_index_signed_with_another_key_is_refused() {
        let pair = test_key();
        let (json, signature) = signed(&index(vec![big_maps()]), &pair);
        assert_eq!(
            Index::verified(&json, &signature, &keys(&test_key())),
            Err(IndexError::BadSignature)
        );
        assert_eq!(
            Index::verified(&json, &signature, &[]),
            Err(IndexError::BadSignature)
        );
    }

    #[test]
    fn another_format_is_refused() {
        let pair = test_key();
        let mut future = index(vec![big_maps()]);
        future.format = 2;
        let (json, signature) = signed(&future, &pair);
        assert_eq!(
            Index::verified(&json, &signature, &keys(&pair)),
            Err(IndexError::Format(2))
        );
    }

    #[test]
    fn one_bad_entry_refuses_the_whole_index() {
        let pair = test_key();
        let bad: [fn(&mut Package); 9] = [
            |p| p.builds.clear(),
            |p| p.builds = vec!["not a hash".into()],
            |p| p.files[0].path = "../escape.lua".into(),
            |p| p.files[0].path = "C:/Windows/x.dll".into(),
            |p| p.files[0].path = "mod\\x.lua".into(),
            |p| p.files[0].sha256 = "ABC".into(),
            |p| p.version = "latest".into(),
            |p| p.id = "Big Maps".into(),
            |p| p.features = vec!["bigmap..octree".into()],
        ];
        for (n, spoil) in bad.iter().enumerate() {
            let mut package = big_maps();
            spoil(&mut package);
            let (json, signature) = signed(&index(vec![package]), &pair);
            assert!(
                matches!(
                    Index::verified(&json, &signature, &keys(&pair)),
                    Err(IndexError::Malformed(_))
                ),
                "case {n}"
            );
        }
        let (json, signature) = signed(&index(vec![big_maps(), big_maps()]), &pair);
        assert!(matches!(
            Index::verified(&json, &signature, &keys(&pair)),
            Err(IndexError::Malformed(_))
        ));
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let pair = test_key();
        let json = serde_json::to_string(&index(vec![big_maps()]))
            .unwrap()
            .replacen("\"serial\"", "\"run_this\":\"x\",\"serial\"", 1)
            .into_bytes();
        let signature = pair.sign(&json).as_ref().to_vec();
        assert!(matches!(
            Index::verified(&json, &signature, &keys(&pair)),
            Err(IndexError::Malformed(_))
        ));
    }

    #[test]
    fn versions_come_newest_first() {
        let index = index(vec![
            package("a", "1.2.0", &[]),
            package("a", "1.10.0", &[]),
            package("a", "1.9.1", &[]),
        ]);
        let order: Vec<&str> = index
            .versions("a")
            .iter()
            .map(|p| p.version.as_str())
            .collect();
        assert_eq!(order, ["1.10.0", "1.9.1", "1.2.0"]);
    }
}
