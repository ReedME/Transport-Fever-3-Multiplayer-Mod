//! What a player's game runs, and how two games differ.
//!
//! Every player declares a [`ContentManifest`]: the game's build and its
//! active mods in load order. Its [`ContentFingerprint`] is what rooms
//! compare, and when two differ, [`ContentManifest::compare`] says how, so
//! players learn which mods to add, remove or update instead of only that
//! something differs.
//!
//! Beside its mods, a manifest names the native mods the game runs that
//! change the simulation (docs/NATIVE_MODS.md, D29): each as a mod named
//! [`NATIVE_PREFIX`] and the package's id, whose version is the package's
//! version, `+`, the first ten hex digits of its settings digest, and the
//! full SHA-256 package identity. They are compared as the mods are, so two
//! games with different native mods, package bytes or settings never share
//! a room.

use std::{collections::HashMap, fmt};

use ring::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};

use crate::{ContentFingerprint, FixedBytes, Text};

/// The name a mod goes by, as the game lists it.
pub type ModId = Text<96>;
/// A mod's version, as the game or its author states it.
/// The extra room is for native package terms; ordinary game mods remain
/// limited to [`MAX_MOD_VERSION`] bytes by [`ContentManifest::is_valid`].
pub type ModVersion = Text<96>;
/// Longest version of an ordinary game mod, as before native terms grew.
pub const MAX_MOD_VERSION: usize = 32;
/// Longest native package version before its digest (the native index's
/// package-version limit).
pub const MAX_NATIVE_PACKAGE_VERSION: usize = 20;
/// Native term digest: ten hex digits for settings consistency, then the
/// full 64-digit package identity SHA-256.
pub const NATIVE_TERM_DIGEST_HEX: usize = 74;

/// Most mods a manifest lists by name; [`Unlisted`] summarises the rest.
pub const MAX_LISTED_MODS: usize = 2048;
/// Largest encoded manifest, so that declaring one fits a control frame.
pub const MAX_MANIFEST_BYTES: usize = 48 * 1024;
/// Most mods a [`ContentDiff`] names of each kind; its totals count all.
pub const MAX_DIFF_LISTED: usize = 32;
/// Most native mods a manifest names. More is refused, never cut short.
pub const MAX_NATIVE_TERMS: usize = 16;
/// What a native mod's id starts with in a manifest. No mod folder can be
/// named so: `:` is not allowed in a Windows file name.
pub const NATIVE_PREFIX: &str = "native:";
/// Bytes [`ContentManifest::new`] leaves for the native mods: the most of
/// them, each with the longest id and version, and their list's length.
const NATIVE_RESERVE: usize = MAX_NATIVE_TERMS * (2 + 96 + 1 + 95) + 2;

/// Keeps content fingerprints apart from every other SHA-256 in the
/// project, and names their format.
const FINGERPRINT_DOMAIN: &[u8] = b"tpf3mp content 1\0";

/// TPF3-MP's own mod, which every game of a room runs. Its version in a
/// manifest is its `revision`, a `+`, and a fingerprint of the files the
/// game loads from it (`tpf3mp-agent`'s `own_mod`), so that two copies of
/// the same revision whose files differ do not match.
pub const OWN_MOD: &str = "tpf3mp_1";

/// The fingerprint part of a version of [`OWN_MOD`], shortened for players
/// to compare: what follows the `+`, else the whole version, else "none".
pub fn own_mod_short(version: &str) -> &str {
    let fingerprint = version.rsplit_once('+').map_or(version, |(_, after)| after);
    let end = fingerprint
        .char_indices()
        .nth(8)
        .map_or(fingerprint.len(), |(at, _)| at);
    match &fingerprint[..end] {
        "" => "none",
        short => short,
    }
}

/// One mod a game runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModRef {
    pub id: ModId,
    pub version: ModVersion,
}

/// The mods after the listed ones, when a game runs more than a manifest
/// lists: how many, and a digest of their list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unlisted {
    pub count: u32,
    pub digest: FixedBytes<32>,
}

/// What a player's game runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentManifest {
    /// The game's build, as the game names it.
    pub game: Text<64>,
    /// The active mods in load order, as many as fit.
    pub mods: Vec<ModRef>,
    /// The mods after `mods`, if there are more than fit.
    pub unlisted: Option<Unlisted>,
    /// The native mods that change the simulation, each a mod named
    /// [`NATIVE_PREFIX`] and its id, ordered by id
    /// ([`ContentManifest::with_native`]).
    pub native: Vec<ModRef>,
}

impl ContentManifest {
    /// `game` running `mods` in load order. Lists as many mods as fit in a
    /// manifest and summarises the rest, so the same game and mods always
    /// give the same manifest.
    pub fn new(game: Text<64>, mods: Vec<ModRef>) -> Self {
        let mut manifest = Self {
            game,
            mods: Vec::new(),
            unlisted: None,
            native: Vec::new(),
        };
        let mut size = encoded_len(&manifest);
        let mut mods = mods.into_iter();
        for listed in mods.by_ref() {
            // Room for the summary of what does not fit, and for the list's
            // length growing by a byte or two.
            // And for the native mods.
            let grown = size + encoded_len(&listed) + 48 + NATIVE_RESERVE;
            if manifest.mods.len() == MAX_LISTED_MODS || grown > MAX_MANIFEST_BYTES {
                let rest: Vec<ModRef> = std::iter::once(listed).chain(mods).collect();
                let mut digest = Context::new(&SHA256);
                digest.update(&postcard::to_allocvec(&rest).unwrap_or_default());
                manifest.unlisted = Some(Unlisted {
                    count: u32::try_from(rest.len()).unwrap_or(u32::MAX),
                    digest: FixedBytes(digest.finish().as_ref().try_into().unwrap_or([0; 32])),
                });
                break;
            }
            size += encoded_len(&listed);
            manifest.mods.push(listed);
        }
        manifest
    }

    /// The same manifest with `native`, the game's native mods that change
    /// the simulation, ordered by id.
    pub fn with_native(mut self, mut native: Vec<ModRef>) -> Self {
        native.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        self.native = native;
        self
    }

    /// Whether the manifest is within the limits one declaration may take:
    /// at most [`MAX_NATIVE_TERMS`] native mods, each named as one, once and
    /// in order.
    pub fn is_valid(&self) -> bool {
        self.mods.len() <= MAX_LISTED_MODS
            && self
                .mods
                .iter()
                .all(|listed| listed.version.as_str().len() <= MAX_MOD_VERSION)
            && self.native.len() <= MAX_NATIVE_TERMS
            && self.native.iter().all(|term| {
                term.id
                    .as_str()
                    .strip_prefix(NATIVE_PREFIX)
                    .is_some_and(|id| !id.is_empty())
                    && native_term_version_is_full(term.version.as_str())
            })
            && self
                .native
                .windows(2)
                .all(|pair| pair[0].id.as_str() < pair[1].id.as_str())
            && encoded_len(self) <= MAX_MANIFEST_BYTES
    }

    /// What rooms compare: equal only for the same build, the same mods in
    /// the same order and the same native mods. Without native mods it is
    /// what it was before manifests had them, so that a room logged then
    /// still takes its players back.
    pub fn fingerprint(&self) -> ContentFingerprint {
        let mut digest = Context::new(&SHA256);
        digest.update(FINGERPRINT_DOMAIN);
        let listed = (&self.game, &self.mods, &self.unlisted);
        digest.update(&postcard::to_allocvec(&listed).unwrap_or_default());
        if !self.native.is_empty() {
            digest.update(&postcard::to_allocvec(&self.native).unwrap_or_default());
        }
        ContentFingerprint(FixedBytes(
            digest.finish().as_ref().try_into().unwrap_or([0; 32]),
        ))
    }

    /// How `yours` differs from this manifest, the room's, or `None` if
    /// they are the same.
    pub fn compare(&self, yours: &Self) -> Option<ContentDiff> {
        if self == yours {
            return None;
        }
        // The native mods compare as mods: their ids cannot be a mod's.
        let room: HashMap<&str, &ModRef> = first_by_id(self.mods.iter().chain(&self.native));
        let mine: HashMap<&str, &ModRef> = first_by_id(yours.mods.iter().chain(&yours.native));
        let mut diff = ContentDiff {
            game: (self.game != yours.game).then(|| GameBuilds {
                room: self.game.clone(),
                yours: yours.game.clone(),
            }),
            ..ContentDiff::default()
        };
        for listed in self.mods.iter().chain(&self.native) {
            match mine.get(listed.id.as_str()) {
                None => push(&mut diff.missing, &mut diff.missing_total, listed.clone()),
                Some(own) if own.version != listed.version => push(
                    &mut diff.changed,
                    &mut diff.changed_total,
                    ModChange {
                        id: listed.id.clone(),
                        room: listed.version.clone(),
                        yours: own.version.clone(),
                    },
                ),
                Some(_) => {}
            }
        }
        for own in yours.mods.iter().chain(&yours.native) {
            if !room.contains_key(own.id.as_str()) {
                push(&mut diff.extra, &mut diff.extra_total, own.clone());
            }
        }
        diff.unlisted = self.unlisted != yours.unlisted;
        let same_mods = diff.missing_total == 0 && diff.extra_total == 0 && diff.changed_total == 0;
        diff.reordered = same_mods && self.mods != yours.mods;
        Some(diff)
    }
}

/// Whether a native term carries the full settings prefix and package
/// identity. Old ten-digit terms still deserialize from room logs, but a
/// protocol-22 declaration cannot use one to authorize package contents.
pub fn native_term_version_is_full(version: &str) -> bool {
    let Some((package_version, digest)) = version.rsplit_once('+') else {
        return false;
    };
    !package_version.is_empty()
        && package_version.len() <= MAX_NATIVE_PACKAGE_VERSION
        && digest.len() == NATIVE_TERM_DIGEST_HEX
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn encoded_len<T: Serialize>(value: &T) -> usize {
    postcard::to_allocvec(value).map_or(usize::MAX, |bytes| bytes.len())
}

/// The first mod of each id: a list that names one twice still compares.
fn first_by_id<'a>(mods: impl Iterator<Item = &'a ModRef>) -> HashMap<&'a str, &'a ModRef> {
    let mut by_id = HashMap::new();
    for listed in mods {
        by_id.entry(listed.id.as_str()).or_insert(listed);
    }
    by_id
}

fn push<T>(list: &mut Vec<T>, total: &mut u32, item: T) {
    if list.len() < MAX_DIFF_LISTED {
        list.push(item);
    }
    *total = total.saturating_add(1);
}

/// Two builds of the game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameBuilds {
    pub room: Text<64>,
    pub yours: Text<64>,
}

/// A mod both games run, in different versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModChange {
    pub id: ModId,
    pub room: ModVersion,
    pub yours: ModVersion,
}

/// How a player's game differs from the room's: the owner's in the lobby,
/// the game's once it runs. Lists name the first [`MAX_DIFF_LISTED`] of
/// each kind, in load order; the totals count them all.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentDiff {
    /// The builds, when they differ.
    pub game: Option<GameBuilds>,
    /// Mods the room runs and this player does not.
    pub missing: Vec<ModRef>,
    pub missing_total: u32,
    /// Mods this player runs and the room does not.
    pub extra: Vec<ModRef>,
    pub extra_total: u32,
    /// Mods both run in different versions.
    pub changed: Vec<ModChange>,
    pub changed_total: u32,
    /// The same mods, loaded in another order.
    pub reordered: bool,
    /// The mods after the listed ones differ: one of the games runs more
    /// mods than a manifest names.
    pub unlisted: bool,
}

impl fmt::Display for ContentDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        // TPF3-MP's own mod first: a copy of it whose files differ runs the
        // room's actions differently, whatever its revision says.
        if let Some(own) = self
            .changed
            .iter()
            .find(|change| change.id.as_str() == OWN_MOD)
        {
            parts.push(format!(
                "Your TPF3-MP mod differs from the host's (yours {}, host {}): reinstall the same version",
                own_mod_short(own.yours.as_str()),
                own_mod_short(own.room.as_str())
            ));
        }
        if let Some(builds) = &self.game {
            parts.push(format!(
                "the room runs game build {}, you run {}",
                builds.room, builds.yours
            ));
        }
        let named = |mods: &[ModRef], total: u32| {
            let mut text = mods
                .iter()
                .map(|listed| format!("{} {}", listed.id, listed.version))
                .collect::<Vec<_>>()
                .join(", ");
            let more = total.saturating_sub(u32::try_from(mods.len()).unwrap_or(u32::MAX));
            if more > 0 {
                text.push_str(&format!(" and {more} more"));
            }
            text
        };
        if self.missing_total > 0 {
            parts.push(format!(
                "you lack {}",
                named(&self.missing, self.missing_total)
            ));
        }
        if self.extra_total > 0 {
            parts.push(format!(
                "the room lacks {}",
                named(&self.extra, self.extra_total)
            ));
        }
        // Said above.
        let own_told = self
            .changed
            .iter()
            .filter(|change| change.id.as_str() == OWN_MOD)
            .count();
        if self.changed_total > u32::try_from(own_told).unwrap_or(u32::MAX) {
            let mut text = self
                .changed
                .iter()
                .filter(|change| change.id.as_str() != OWN_MOD)
                .map(|change| {
                    format!(
                        "{} (the room has {}, you have {})",
                        change.id, change.room, change.yours
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let more = self
                .changed_total
                .saturating_sub(u32::try_from(self.changed.len()).unwrap_or(u32::MAX));
            if more > 0 && text.is_empty() {
                text = format!("{more} mods");
            } else if more > 0 {
                text.push_str(&format!(" and {more} more"));
            }
            parts.push(format!("other versions: {text}"));
        }
        if self.reordered {
            parts.push("the same mods load in another order".to_owned());
        }
        let native = |id: &ModId| id.as_str().starts_with(NATIVE_PREFIX);
        if self.missing.iter().any(|m| native(&m.id)) || self.changed.iter().any(|c| native(&c.id))
        {
            parts.push("the launcher installs the room's native mods when you join".to_owned());
        }
        if self.unlisted {
            parts.push("the mods beyond the listed ones differ".to_owned());
        }
        if parts.is_empty() {
            f.write_str("your game differs from the room's")
        } else {
            f.write_str(&parts.join("; "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(id: &str, version: &str) -> ModRef {
        ModRef {
            id: Text::new(id).unwrap(),
            version: Text::new(version).unwrap(),
        }
    }

    fn game(build: &str, mods: &[(&str, &str)]) -> ContentManifest {
        ContentManifest::new(
            Text::new(build).unwrap(),
            mods.iter()
                .map(|(id, version)| listed(id, version))
                .collect(),
        )
    }

    fn native_version(settings: &str, identity: char) -> String {
        format!("0.3.0+{settings}{}", identity.to_string().repeat(64))
    }

    #[test]
    fn the_same_game_matches() {
        let room = game("35924", &[("trains", "1.2"), ("stations", "3")]);
        let yours = game("35924", &[("trains", "1.2"), ("stations", "3")]);
        assert_eq!(room.fingerprint(), yours.fingerprint());
        assert_eq!(room.compare(&yours), None);
    }

    #[test]
    fn differences_name_the_mods_to_add_remove_or_update() {
        let room = game(
            "35924",
            &[("trains", "1.2"), ("stations", "3"), ("maps", "1")],
        );
        let yours = game("35925", &[("trains", "1.1"), ("maps", "1"), ("trees", "2")]);
        assert_ne!(room.fingerprint(), yours.fingerprint());
        let diff = room.compare(&yours).unwrap();
        assert_eq!(
            diff.game,
            Some(GameBuilds {
                room: Text::new("35924").unwrap(),
                yours: Text::new("35925").unwrap(),
            })
        );
        assert_eq!(diff.missing, [listed("stations", "3")]);
        assert_eq!(diff.extra, [listed("trees", "2")]);
        assert_eq!(
            diff.changed,
            [ModChange {
                id: Text::new("trains").unwrap(),
                room: Text::new("1.2").unwrap(),
                yours: Text::new("1.1").unwrap(),
            }]
        );
        assert!(!diff.reordered);
        assert_eq!(
            diff.to_string(),
            "the room runs game build 35924, you run 35925; you lack stations 3; \
             the room lacks trees 2; other versions: trains (the room has 1.2, you have 1.1)"
        );
    }

    #[test]
    fn another_copy_of_tpf3mp_itself_is_told_first_and_plainly() {
        let room = game(
            "40408",
            &[("trains", "1.2"), (OWN_MOD, "1+0123456789abcdef")],
        );
        let same = game(
            "40408",
            &[("trains", "1.2"), (OWN_MOD, "1+0123456789abcdef")],
        );
        assert_eq!(room.compare(&same), None, "the same files match");
        let yours = game(
            "40408",
            &[("trains", "1.1"), (OWN_MOD, "1+fedcba9876543210")],
        );
        assert_ne!(room.fingerprint(), yours.fingerprint());
        assert_eq!(
            room.compare(&yours).unwrap().to_string(),
            "Your TPF3-MP mod differs from the host's (yours fedcba98, host 01234567): \
             reinstall the same version; other versions: trains (the room has 1.2, you have 1.1)"
        );
        // Only TPF3-MP differs: that alone is said.
        let only = game(
            "40408",
            &[("trains", "1.2"), (OWN_MOD, "1+fedcba9876543210")],
        );
        assert_eq!(
            room.compare(&only).unwrap().to_string(),
            "Your TPF3-MP mod differs from the host's (yours fedcba98, host 01234567): \
             reinstall the same version"
        );
        // An agent before fingerprints declares the revision alone.
        assert_eq!(own_mod_short("1"), "1");
        assert_eq!(own_mod_short(""), "none");
    }

    #[test]
    fn load_order_counts() {
        let room = game("35924", &[("trains", "1"), ("stations", "1")]);
        let yours = game("35924", &[("stations", "1"), ("trains", "1")]);
        assert_ne!(room.fingerprint(), yours.fingerprint());
        let diff = room.compare(&yours).unwrap();
        assert!(diff.reordered);
        assert_eq!(diff.to_string(), "the same mods load in another order");
    }

    #[test]
    fn long_lists_name_the_first_and_count_the_rest() {
        let many: Vec<(String, String)> = (0..100)
            .map(|n| (format!("mod{n}"), "1".to_owned()))
            .collect();
        let many: Vec<(&str, &str)> = many.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let room = game("35924", &many);
        let yours = game("35924", &[]);
        let diff = room.compare(&yours).unwrap();
        assert_eq!(diff.missing.len(), MAX_DIFF_LISTED);
        assert_eq!(diff.missing_total, 100);
        assert!(diff.to_string().ends_with("mod31 1 and 68 more"));
    }

    #[test]
    fn a_huge_mod_list_is_summarised_and_still_told_apart() {
        let many: Vec<ModRef> = (0..3000)
            .map(|n| {
                listed(
                    &format!("workshop_{n:010}_a_rather_long_mod_folder_name"),
                    "1.0",
                )
            })
            .collect();
        let room = ContentManifest::new(Text::new("35924").unwrap(), many.clone());
        assert!(room.is_valid());
        assert!(room.mods.len() < many.len());
        let unlisted = room.unlisted.unwrap();
        assert_eq!(room.mods.len() + unlisted.count as usize, 3000);
        // The same list gives the same manifest.
        assert_eq!(
            ContentManifest::new(Text::new("35924").unwrap(), many.clone()),
            room
        );
        // A difference past the listed mods still changes the fingerprint.
        let mut other = many;
        other[2999] = listed("something_else", "1.0");
        let yours = ContentManifest::new(Text::new("35924").unwrap(), other);
        assert_ne!(room.fingerprint(), yours.fingerprint());
        let diff = room.compare(&yours).unwrap();
        assert!(diff.unlisted);
        assert_eq!(
            diff.missing_total + diff.extra_total + diff.changed_total,
            0
        );
    }

    #[test]
    fn an_oversized_manifest_is_invalid() {
        let mods: Vec<ModRef> = (0..MAX_LISTED_MODS + 1)
            .map(|n| listed(&format!("m{n}"), "1"))
            .collect();
        let manifest = ContentManifest {
            game: Text::new("35924").unwrap(),
            mods,
            unlisted: None,
            native: Vec::new(),
        };
        assert!(!manifest.is_valid());
    }

    /// A manifest without native mods has the fingerprint it had before
    /// manifests could carry them: a running room's log keeps fingerprints.
    #[test]
    fn without_native_mods_the_fingerprint_is_as_before() {
        let manifest = game("40420", &[("trains", "1.2"), (OWN_MOD, "1+00")]);
        let mut before = Context::new(&SHA256);
        before.update(FINGERPRINT_DOMAIN);
        let old = (&manifest.game, &manifest.mods, &manifest.unlisted);
        before.update(&postcard::to_allocvec(&old).unwrap());
        let before = before.finish();
        assert_eq!(manifest.fingerprint().0.0, before.as_ref());
    }

    #[test]
    fn native_mods_compare_as_mods() {
        let base = game("40420", &[("trains", "1"), (OWN_MOD, "1+00")]);
        let big = |version: &str| {
            base.clone()
                .with_native(vec![listed("native:bigmap", version)])
        };
        let room_version = native_version("0123456789", 'a');
        let room = big(&room_version);
        assert!(room.is_valid());
        assert_eq!(room.compare(&big(&room_version)), None);
        assert_eq!(room.fingerprint(), big(&room_version).fingerprint());

        // Other settings: another settings prefix and package identity.
        let other_version = native_version("abcdef0123", 'b');
        let other = big(&other_version);
        assert_ne!(room.fingerprint(), other.fingerprint());
        let diff = room.compare(&other).unwrap();
        assert_eq!(diff.changed_total, 1);
        assert!(diff.to_string().contains("installs the room's native mods"));

        // Without it: missing; the room without it: extra.
        let diff = room.compare(&base).unwrap();
        assert_eq!(diff.missing, [listed("native:bigmap", &room_version)]);
        assert!(!diff.reordered);
        let diff = base.compare(&room).unwrap();
        assert_eq!(diff.extra_total, 1);
        assert!(!diff.to_string().contains("installs"));

        // A native mod is never mistaken for a mod of the same name.
        let named = game(
            "40420",
            &[("trains", "1"), ("bigmap", "0.3.0"), (OWN_MOD, "1+00")],
        );
        assert_ne!(named.fingerprint(), room.fingerprint());
    }

    #[test]
    fn native_mods_must_be_named_as_such_ordered_and_few() {
        let base = game("40420", &[]);
        let mut wrong = base.clone();
        wrong.native = vec![listed("bigmap", "1.0.0+00")];
        assert!(!wrong.is_valid());
        let mut unordered = base.clone();
        let identity_a = native_version("0123456789", 'a');
        let identity_b = native_version("0123456789", 'b');
        unordered.native = vec![
            listed("native:b", &identity_b),
            listed("native:a", &identity_a),
        ];
        assert!(!unordered.is_valid());
        assert!(
            base.clone()
                .with_native(vec![
                    listed("native:a", &identity_a),
                    listed("native:b", &identity_b)
                ])
                .is_valid()
        );
        let many = (0..=MAX_NATIVE_TERMS)
            .map(|n| listed(&format!("native:m{n:03}"), "1"))
            .collect();
        assert!(!base.clone().with_native(many).is_valid());
        // The longest native mods still fit beside a summarised mod list.
        let huge: Vec<ModRef> = (0..3000)
            .map(|n| {
                listed(
                    &format!("workshop_{n:010}_{}", "x".repeat(76)),
                    &"v".repeat(32),
                )
            })
            .collect();
        let longest_version = format!("{}+{}", "v".repeat(20), "1".repeat(NATIVE_TERM_DIGEST_HEX));
        let longest = (0..MAX_NATIVE_TERMS)
            .map(|n| {
                listed(
                    &format!("native:{n:02}{}", "n".repeat(87)),
                    &longest_version,
                )
            })
            .collect();
        let full = ContentManifest::new(Text::new("40420").unwrap(), huge).with_native(longest);
        assert!(full.unlisted.is_some());
        assert!(full.is_valid());
    }

    #[test]
    fn old_short_native_terms_and_oversized_normal_versions_fail_closed() {
        let old = game("40420", &[]).with_native(vec![listed("native:bigmap", "0.3.0+0123456789")]);
        assert!(
            !old.is_valid(),
            "ten hex digits cannot identify package bytes"
        );
        let too_long = game(
            "40420",
            &[("ordinary_mod", &"v".repeat(MAX_MOD_VERSION + 1))],
        );
        assert!(
            !too_long.is_valid(),
            "normal mod versions keep their old limit"
        );
    }
}
