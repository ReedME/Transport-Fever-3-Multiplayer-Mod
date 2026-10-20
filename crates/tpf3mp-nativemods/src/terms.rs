//! Native mods as a room's terms.
//!
//! A native package that changes the simulation must run in every game of a
//! room in the same version with the same settings and package bytes, or the
//! worlds part. Its term is its id, its version, a short settings digest and
//! the full canonical package identity; the room
//! compares the terms of every member as it compares their content
//! (`ContentManifest`, docs/PROTOCOL.md), and a member missing a package,
//! or holding another version of it, installs the room's version from the
//! signed index in one click ([`missing_from_index`]), never from another
//! player. A package that changes only what its player sees is not a term:
//! players may differ in it.
//!
//! The protocol carries these terms since version 20 (D29): each goes in
//! the content manifest's own list of native mods in the shape of a mod
//! ([`NativeTerm::mod_id`], [`NativeTerm::mod_version`]), and the room
//! compares them as it compares mods
//! ([`crate::enabled::ROOM_TERMS_CARRIED`]).

use std::collections::BTreeMap;

use ring::digest::{Context, SHA256};
use serde::Serialize;

use crate::{
    enabled::Enabled,
    features::FeatureSpec,
    index::{FileEntry, Index, Package, Setting},
    signed,
};

/// Keeps settings digests apart from every other SHA-256 in the project.
const SETTINGS_DOMAIN: &[u8] = b"tpf3mp native settings 1\0";
/// Keeps package file-list digests apart from every other SHA-256.
const FILES_DOMAIN: &[u8] = b"tpf3mp native package files 1\0";
/// Names the canonical package identity whose full digest is a room term.
const PACKAGE_DOMAIN: &[u8] = b"tpf3mp native package identity 1\0";
/// What a native term's mod id starts with. No mod folder can be named so:
/// `:` is not allowed in a Windows file name.
pub const MOD_PREFIX: &str = "native:";
/// Hex digits of the settings digest kept so the server can validate the
/// settings carried beside a room term.
const SETTINGS_PREFIX: usize = 10;

/// One simulation-changing package as the room compares it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct NativeTerm {
    pub id: String,
    pub version: String,
    /// The SHA-256 of its effective settings, lowercase hex.
    pub settings: String,
    /// The full SHA-256 of its canonical package identity, lowercase hex.
    pub identity: String,
}

impl NativeTerm {
    /// Its id as a mod in a manifest: `native:<id>`.
    pub fn mod_id(&self) -> String {
        format!("{MOD_PREFIX}{}", self.id)
    }

    /// Its version as a mod in a manifest: the package's version, `+`, the
    /// settings digest's first ten hex digits, and its full package identity.
    /// The longest value is 95 bytes for a 20-byte package version.
    pub fn mod_version(&self) -> String {
        let short = &self.settings[..SETTINGS_PREFIX.min(self.settings.len())];
        format!("{}+{short}{}", self.version, self.identity)
    }
}

/// The digest of `settings`, the same for the same settings on every
/// machine: a `BTreeMap` serializes its keys in order.
pub fn settings_digest(settings: &BTreeMap<String, Setting>) -> String {
    let mut digest = Context::new(&SHA256);
    digest.update(SETTINGS_DOMAIN);
    digest.update(&serde_json::to_vec(settings).unwrap_or_default());
    signed::hex(digest.finish().as_ref())
}

/// The SHA-256 of the package's sorted file paths, sizes and content hashes.
/// The download URL is omitted because mirrors do not change the contents a
/// game runs.
pub fn files_digest(files: &[FileEntry]) -> String {
    let mut files: Vec<(&str, u64, &str)> = files
        .iter()
        .map(|file| (file.path.as_str(), file.size, file.sha256.as_str()))
        .collect();
    files.sort_unstable();
    let encoded = postcard::to_allocvec(&files).expect("file identity is serializable");
    let mut digest = Context::new(&SHA256);
    digest.update(FILES_DOMAIN);
    digest.update(&encoded);
    signed::hex(digest.finish().as_ref())
}

#[derive(Serialize)]
struct PackageIdentity<'a> {
    id: &'a str,
    version: &'a str,
    name: &'a str,
    authors: &'a [String],
    simulation: bool,
    builds: Vec<&'a str>,
    features: Vec<&'a str>,
    declared_settings: &'a BTreeMap<String, Setting>,
    effective_settings: &'a BTreeMap<String, Setting>,
    files: Vec<(&'a str, u64, &'a str)>,
    plugins: Vec<(&'a str, &'a str, u64, &'a str, u32)>,
    hooks: &'a [crate::index::HookDecl],
    game_mods: Vec<&'a str>,
    conflicts: Vec<&'a str>,
    depends: Vec<(&'a str, &'a str)>,
}

/// The full SHA-256 identity used by room terms and plugin consent. It binds
/// effective settings to every package file and every platform's plugin
/// descriptor, along with the declarations that control how the package is
/// loaded. Mirrors and display-only description/link fields do not affect it.
pub fn package_digest(package: &Package, effective_settings: &BTreeMap<String, Setting>) -> String {
    let mut files: Vec<(&str, u64, &str)> = package
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.size, file.sha256.as_str()))
        .collect();
    files.sort_unstable();
    let mut plugins: Vec<(&str, &str, u64, &str, u32)> = package
        .plugins
        .iter()
        .map(|plugin| {
            (
                plugin.platform.as_str(),
                plugin.path.as_str(),
                plugin.size,
                plugin.sha256.as_str(),
                plugin.abi,
            )
        })
        .collect();
    plugins.sort_unstable();
    let mut builds: Vec<&str> = package.builds.iter().map(String::as_str).collect();
    builds.sort_unstable();
    let features: Vec<&str> = package.features.iter().map(String::as_str).collect();
    let mut game_mods: Vec<&str> = package.game_mods.iter().map(String::as_str).collect();
    game_mods.sort_unstable();
    let mut conflicts: Vec<&str> = package.conflicts.iter().map(String::as_str).collect();
    conflicts.sort_unstable();
    let mut depends: Vec<(&str, &str)> = package
        .depends
        .iter()
        .map(|dependency| (dependency.id.as_str(), dependency.version.as_str()))
        .collect();
    depends.sort_unstable();
    let identity = PackageIdentity {
        id: &package.id,
        version: &package.version,
        name: &package.name,
        authors: &package.authors,
        simulation: package.simulation,
        builds,
        features,
        declared_settings: &package.settings,
        effective_settings,
        files,
        plugins,
        hooks: &package.hooks,
        game_mods,
        conflicts,
        depends,
    };
    let encoded = postcard::to_allocvec(&identity).expect("package identity is serializable");
    let mut digest = Context::new(&SHA256);
    digest.update(PACKAGE_DOMAIN);
    digest.update(&encoded);
    signed::hex(digest.finish().as_ref())
}

/// The room's terms for the packages in `enabled`: the simulation-changing
/// ones, ordered by id.
pub fn terms(enabled: &Enabled, registry: &[FeatureSpec]) -> Vec<NativeTerm> {
    let mut terms: Vec<NativeTerm> = enabled
        .packages
        .iter()
        .filter(|package| package.changes_simulation(registry))
        .map(|package| NativeTerm {
            id: package.id.clone(),
            version: package.version.clone(),
            settings: settings_digest(&package.settings),
            identity: package.identity_sha256.clone(),
        })
        .collect();
    terms.sort();
    terms
}

/// How a member's terms differ from the room's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TermsDiff {
    /// The room's packages the member lacks.
    pub missing: Vec<NativeTerm>,
    /// Packages both run, with another version or other settings: the
    /// room's term, then the member's.
    pub changed: Vec<(NativeTerm, NativeTerm)>,
    /// The member's packages the room does not run.
    pub extra: Vec<NativeTerm>,
}

impl TermsDiff {
    pub fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.changed.is_empty() && self.extra.is_empty()
    }
}

/// How `yours` differ from the `room`'s terms.
pub fn compare(room: &[NativeTerm], yours: &[NativeTerm]) -> TermsDiff {
    let mine: BTreeMap<&str, &NativeTerm> = yours.iter().map(|t| (t.id.as_str(), t)).collect();
    let theirs: BTreeMap<&str, &NativeTerm> = room.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut diff = TermsDiff::default();
    for term in room {
        match mine.get(term.id.as_str()) {
            None => diff.missing.push(term.clone()),
            Some(own) if *own != term => diff.changed.push((term.clone(), (*own).clone())),
            Some(_) => {}
        }
    }
    diff.extra = yours
        .iter()
        .filter(|t| !theirs.contains_key(t.id.as_str()))
        .cloned()
        .collect();
    diff
}

/// The packages of the signed `index` that give a member the room's
/// versions of what `diff` says they lack or hold in another version, for
/// the build hashing to `build`. A term the index does not list in that
/// version, or not for this build, is returned as the error: that member
/// cannot join, and nothing is fetched from anywhere else.
pub fn missing_from_index<'a>(
    diff: &TermsDiff,
    index: &'a Index,
    build: &str,
) -> Result<Vec<&'a Package>, NativeTerm> {
    diff.missing
        .iter()
        .chain(diff.changed.iter().map(|(room, _)| room))
        .map(|term| {
            index
                .package(&term.id, &term.version)
                .filter(|package| package.runs_on(build))
                .ok_or_else(|| term.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{
        enabled::{EnabledPackage, FORMAT},
        features::example::BIG_MAPS,
        index::tests::{BUILD, OTHER_BUILD, big_maps, index, package},
    };

    fn enabled(settings: &[(&str, Setting)], with_page_only: bool) -> Enabled {
        let settings: BTreeMap<String, Setting> = settings
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect();
        let bigmap_identity = package_digest(&package("bigmap", "0.3.0", &[]), &settings);
        let mut packages = vec![EnabledPackage {
            id: "bigmap".into(),
            version: "0.3.0".into(),
            simulation: true,
            features: vec!["bigmap.octree".into()],
            settings: settings.clone(),
            files_sha256: files_digest(&[]),
            identity_sha256: bigmap_identity,
            depends: Vec::new(),
            root: PathBuf::new(),
            plugins: Vec::new(),
            hooks: Vec::new(),
        }];
        if with_page_only {
            let page_settings = BTreeMap::new();
            packages.push(EnabledPackage {
                id: "pages".into(),
                version: "1.0.0".into(),
                simulation: false,
                features: vec!["bigmap.page".into()],
                settings: page_settings.clone(),
                files_sha256: files_digest(&[]),
                identity_sha256: package_digest(&package("pages", "1.0.0", &[]), &page_settings),
                depends: Vec::new(),
                root: PathBuf::new(),
                plugins: Vec::new(),
                hooks: Vec::new(),
            });
        }
        Enabled {
            format: FORMAT,
            build: BUILD.into(),
            packages,
        }
    }

    #[test]
    fn all_platform_plugin_descriptors_and_hooks_are_in_the_identity() {
        let settings = BTreeMap::new();
        let plugin = |platform: &str| crate::index::Plugin {
            path: format!("plugin/{platform}.dll"),
            url: format!("https://one.invalid/{platform}.dll"),
            size: 7,
            sha256: "a".repeat(64),
            abi: crate::index::PLUGIN_ABI,
            platform: platform.into(),
        };
        let mut package = package("plugins", "1.0.0", &[]);
        package.plugins = vec![plugin("windows-x86_64"), plugin("linux-x86_64")];
        let baseline = package_digest(&package, &settings);

        for edit in ["hash", "hook", "abi", "platform", "path", "size"] {
            let mut changed = package.clone();
            match edit {
                "hash" => changed.plugins[1].sha256 = "b".repeat(64),
                "abi" => changed.plugins[1].abi += 1,
                "platform" => changed.plugins[1].platform = "macos-aarch64".into(),
                "path" => changed.plugins[1].path.push_str(".new"),
                "size" => changed.plugins[1].size += 1,
                "hook" => changed.hooks.push(crate::index::HookDecl {
                    target: "plugins::tick".into(),
                    kind: crate::index::HookKind::Find,
                    signature: "48 89".into(),
                    offset: 0,
                    expected: "48 89".into(),
                    access: crate::index::HookUse::Chain,
                    steal: None,
                    replacement: None,
                    slot: None,
                }),
                _ => unreachable!(),
            }
            assert_ne!(package_digest(&changed, &settings), baseline, "{edit}");
        }

        let mut reordered = package.clone();
        reordered.plugins.reverse();
        assert_eq!(package_digest(&reordered, &settings), baseline);
        let mut mirror = package;
        mirror.plugins[0].url = "https://mirror.invalid/plugin.dll".into();
        assert_eq!(package_digest(&mirror, &settings), baseline);
        let mut named = mirror.clone();
        named.authors.push("A verified author".into());
        assert_ne!(package_digest(&named, &settings), baseline);
        named = mirror.clone();
        named.name.push_str(" updated");
        assert_ne!(package_digest(&named, &settings), baseline);
        let mut declared_setting = mirror.clone();
        declared_setting
            .settings
            .insert("radius".into(), Setting::Int(10));
        assert_ne!(package_digest(&declared_setting, &settings), baseline);
        assert_ne!(
            package_digest(
                &mirror,
                &BTreeMap::from([("radius".into(), Setting::Int(11))])
            ),
            baseline
        );
    }

    #[test]
    fn only_simulation_packages_are_terms_and_settings_count() {
        let a = terms(
            &enabled(&[("octree_depth", Setting::Int(10))], true),
            BIG_MAPS,
        );
        assert_eq!(a.len(), 1, "the page-only package is the player's own");
        assert_eq!(a[0].mod_id(), "native:bigmap");
        assert!(a[0].mod_version().starts_with("0.3.0+"));
        assert_eq!(a[0].mod_version().len(), "0.3.0+".len() + 74);
        assert_eq!(
            &a[0].mod_version()["0.3.0+".len()..][..10],
            &a[0].settings[..10]
        );
        assert_eq!(a[0].identity.len(), 64);

        let same = terms(
            &enabled(&[("octree_depth", Setting::Int(10))], false),
            BIG_MAPS,
        );
        assert!(compare(&a, &same).is_empty());

        let deeper = terms(
            &enabled(&[("octree_depth", Setting::Int(11))], false),
            BIG_MAPS,
        );
        let diff = compare(&a, &deeper);
        assert_eq!(diff.changed.len(), 1);
        assert_ne!(a[0].mod_version(), deeper[0].mod_version());

        let diff = compare(&a, &[]);
        assert_eq!(diff.missing, a);
        let diff = compare(&[], &a);
        assert_eq!(diff.extra, a);
    }

    #[test]
    fn a_missing_package_comes_from_the_signed_index_or_not_at_all() {
        let room = terms(&enabled(&[], false), BIG_MAPS);
        let diff = compare(&room, &[]);
        let index = index(vec![big_maps(), package("other", "1.0.0", &[])]);
        let found = missing_from_index(&diff, &index, BUILD).unwrap();
        assert_eq!(found[0].id, "bigmap");
        // Not for this build, or a version the index does not list.
        assert_eq!(
            missing_from_index(&diff, &index, OTHER_BUILD),
            Err(room[0].clone())
        );
        let mut newer = room.clone();
        newer[0].version = "9.0.0".into();
        assert!(missing_from_index(&compare(&newer, &[]), &index, BUILD).is_err());
    }

    #[test]
    fn package_files_are_sorted_and_simulation_data_changes_the_identity() {
        let first = FileEntry {
            path: "content/simulation.lua".into(),
            url: "https://one.invalid/simulation.lua".into(),
            size: 12,
            sha256: "a".repeat(64),
        };
        let second = FileEntry {
            path: "content/settings.lua".into(),
            url: "https://one.invalid/settings.lua".into(),
            size: 8,
            sha256: "b".repeat(64),
        };
        assert_eq!(
            files_digest(&[first.clone(), second.clone()]),
            files_digest(&[second.clone(), first.clone()]),
            "file order and mirrors do not change the canonical digest"
        );
        let mut another_mirror = first.clone();
        another_mirror.url = "https://mirror.invalid/simulation.lua".into();
        assert_eq!(
            files_digest(&[first.clone(), second.clone()]),
            files_digest(&[another_mirror, second.clone()]),
            "download URLs do not change package contents"
        );

        let mut package = package("bigmap", "0.3.0", &[]);
        package.simulation = true;
        package.files = vec![first.clone(), second.clone()];
        let mut unchanged = enabled(&[("octree_depth", Setting::Int(10))], false)
            .packages
            .remove(0);
        unchanged.files_sha256 = files_digest(&package.files);
        unchanged.identity_sha256 = package_digest(&package, &unchanged.settings);
        let before = unchanged.identity_sha256.clone();
        let before_term = terms(
            &Enabled {
                format: FORMAT,
                build: BUILD.into(),
                packages: vec![unchanged.clone()],
            },
            BIG_MAPS,
        )[0]
        .mod_version();

        let changed = FileEntry {
            sha256: "c".repeat(64),
            ..first.clone()
        };
        let mut updated_package = package.clone();
        updated_package.files = vec![changed, second.clone()];
        let updated_identity = package_digest(&updated_package, &unchanged.settings);
        assert_ne!(updated_identity, before);
        let mut updated = unchanged.clone();
        updated.files_sha256 = files_digest(&updated_package.files);
        updated.identity_sha256 = updated_identity;
        let mut reordered_features = package.clone();
        reordered_features.features = vec!["first.feature".into(), "second.feature".into()];
        let first_order_digest = package_digest(&reordered_features, &unchanged.settings);
        reordered_features.features.reverse();
        assert_ne!(
            package_digest(&reordered_features, &unchanged.settings),
            first_order_digest,
            "feature order changes the hook plan"
        );
        let mut with_dependency = updated_package.clone();
        with_dependency.depends.push(crate::index::Dependency {
            id: "other".into(),
            version: "^1.0".into(),
        });
        assert_ne!(
            package_digest(&with_dependency, &unchanged.settings),
            package_digest(&updated_package, &unchanged.settings)
        );
        let after_term = terms(
            &Enabled {
                format: FORMAT,
                build: BUILD.into(),
                packages: vec![updated],
            },
            BIG_MAPS,
        )[0]
        .mod_version();
        assert_ne!(
            before_term, after_term,
            "the room term changes with Lua/data bytes"
        );
    }
}
