//! The room's mods as its owner chooses them and every member sees them
//! (docs/MODS.md, "The room's mods"; proposed D28).
//!
//! The owner declares their game's content and the room's mods in one
//! request ([`RoomDeclaration`]): the manifest the room compares, as every
//! player declares one, and beside each of its mods what players are told
//! of it (its name, where it comes from, its Mod Hub number), with the
//! settings the room's world loads it with. The room sends every member the
//! list ([`RoomMods`]), so each knows which mods to have before declaring
//! theirs. The fingerprint stays the gate: what a member is told never
//! decides whether they may play.
//!
//! What players are told of a mod is the owner's claim. A Mod Hub number in
//! it is a hint for where to get the mod, which a player's own game resolves
//! and the player confirms; it proves nothing.
//!
//! The room's native mods (docs/NATIVE_MODS.md, D29) travel the same way:
//! the manifest names each by its id and a version holding the settings
//! digest prefix and full package identity ([`crate::NATIVE_PREFIX`]), and
//! beside it the declaration gives the settings themselves
//! ([`NativeMod`]), so that a member's launcher can install that version
//! from the project's signed index and run it with the room's settings. The
//! fingerprint still decides: a member whose package or settings differ
//! from the room's does not match.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{
    ContentManifest, MAX_NATIVE_TERMS, ModId, ModRef, ModVersion, NATIVE_PREFIX, OWN_MOD, Text,
};

/// Most ordinary mods a room runs, TPF3-MP's own counted. Native mods have
/// their own [`MAX_NATIVE_TERMS`] limit. A selection of more ordinary mods
/// is refused, never cut short: every game loads the whole list.
pub const MAX_ROOM_MODS: usize = 256;
/// Most mod settings a room carries, over all its mods.
pub const MAX_ROOM_PARAMS: usize = 512;
/// Largest encoded [`RoomDeclaration`], so that it fits a control frame
/// with its request's envelope, and the [`RoomMods`] made of it too.
pub const MAX_ROOM_DECLARATION_BYTES: usize = 60 * 1024;
/// Where Mod Hub's mods come from, as the game names the source.
pub const MODIO_SOURCE: &str = "mod.io";
/// The id under which the game keeps its own settings beside the mods'
/// (`modParams[""]`: difficulty, costs, the economy's, the towns', ...):
/// the room carries them like a mod's, so every game loads its world with
/// the owner's.
pub const GAME_SETTINGS: &str = "";

/// What players are told of one of the room's mods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModInfo {
    /// Its name for players.
    pub name: Text<48>,
    /// Where the owner's game has it from, as the game names it: `mod.io`,
    /// `StagingArea`, `UserMods`, `DLC`, `BuiltInMods`.
    pub source: Text<16>,
    /// Its Mod Hub (mod.io) number, for a mod from Mod Hub.
    pub modio: Option<u64>,
}

/// One setting of a mod, as the game keeps it (`modParams`, a mod's
/// settings by name, each a whole number). The game's own names run to 46
/// bytes (`economy.industryDevelopment.closureProbability`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModParam {
    pub key: Text<64>,
    pub value: i64,
}

/// The settings of one of the room's mods, or with the id
/// [`GAME_SETTINGS`] the game's own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModParams {
    pub id: ModId,
    pub params: Vec<ModParam>,
}

/// A native mod's setting value, as the signed index types them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeValue {
    Bool(bool),
    Int(i64),
    Text(Text<128>),
}

/// One setting of a native mod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSetting {
    /// The setting's id in the index: lowercase letters, digits and `_`.
    pub key: Text<40>,
    pub value: NativeValue,
}

/// One of the room's native mods: what a member's launcher installs and
/// runs to match it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeMod {
    /// The package's id in the signed index.
    pub id: Text<40>,
    /// The version the manifest names: package version, `+`, ten settings
    /// digest hex digits, and the full package identity SHA-256.
    pub version: ModVersion,
    /// Its name for players.
    pub name: Text<48>,
    /// Every setting it runs with, defaults included, ordered by key.
    pub settings: Vec<NativeSetting>,
}

impl NativeMod {
    /// It as the manifest names it: `native:<id>` in its version.
    pub fn term(&self) -> ModRef {
        ModRef {
            id: Text::lossy(&format!("{NATIVE_PREFIX}{}", self.id)),
            version: self.version.clone(),
        }
    }

    /// The package's version in the index: before the final `+` digest.
    pub fn package_version(&self) -> &str {
        self.version
            .as_str()
            .rsplit_once('+')
            .map_or(self.version.as_str(), |(version, _)| version)
    }
}

/// The room's mods beside the owner's manifest: one [`ModInfo`] for each of
/// its mods, in its order, the settings the room's world loads them with,
/// and one [`NativeMod`] for each of its native mods, in the manifest's
/// order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomConfig {
    pub info: Vec<ModInfo>,
    pub params: Vec<ModParams>,
    pub native: Vec<NativeMod>,
}

/// The owner's declaration of their game's content and the room's mods,
/// taken together or not at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomDeclaration {
    pub manifest: ContentManifest,
    pub room: RoomConfig,
}

/// One of the room's mods, as every member is told it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomMod {
    pub id: ModId,
    /// The owner's version, which every member's must equal.
    pub version: ModVersion,
    pub info: ModInfo,
}

/// The room's mods, in load order, TPF3-MP's own last, with their settings:
/// what every member's game needs and loads the room's world with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomMods {
    /// The game's build the room runs.
    pub game: Text<64>,
    pub mods: Vec<RoomMod>,
    pub params: Vec<ModParams>,
    /// The native mods every member's game runs, with their settings.
    pub native: Vec<NativeMod>,
}

impl RoomMods {
    /// The manifest a game with exactly these mods, in these versions,
    /// declares.
    pub fn manifest(&self) -> ContentManifest {
        ContentManifest::new(
            self.game.clone(),
            self.mods
                .iter()
                .map(|m| ModRef {
                    id: m.id.clone(),
                    version: m.version.clone(),
                })
                .collect(),
        )
        .with_native(self.native.iter().map(NativeMod::term).collect())
    }
}

/// Why a [`RoomDeclaration`] is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RoomModsError {
    #[error("a room runs at most {MAX_ROOM_MODS} ordinary mods, TPF3-MP counted; this one has {0}")]
    TooMany(usize),
    #[error("the room's mods do not match the declared ones one for one")]
    NotAligned,
    #[error("the mod id {0:?} is not one the game could name")]
    BadId(String),
    #[error("the mod {0} is listed twice")]
    Twice(String),
    #[error("TPF3-MP's own mod must be the room's last mod, once")]
    OwnMod,
    #[error("the mod {0} has a Mod Hub number but does not come from Mod Hub")]
    NotModHub(String),
    #[error("settings for {0:?}, which is not one of the room's mods, or given twice")]
    Settings(String),
    #[error("the setting {1} of {0} is given twice")]
    SettingTwice(String, String),
    #[error("a setting of {0} has no name, or one with a line break or tab")]
    BadSetting(String),
    #[error("a room carries at most {MAX_ROOM_PARAMS} mod settings; this one has {0}")]
    TooManySettings(usize),
    #[error("the room's mods take {0} bytes, over the {MAX_ROOM_DECLARATION_BYTES} a room may")]
    TooLarge(usize),
    #[error("the room's native mods do not match the declared ones one for one, or are too many")]
    Native,
    #[error("a setting of the native mod {0} is malformed or given twice")]
    NativeSetting(String),
}

/// Whether `id` is a mod id as the game writes them: letters, digits, `_`,
/// `-` and `.`.
pub fn is_mod_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

impl RoomDeclaration {
    /// Whether the declaration holds together: every one of the manifest's
    /// mods listed (no summarised tail), at most [`MAX_ROOM_MODS`], each id
    /// valid and once, TPF3-MP's own exactly once and last, a Mod Hub number
    /// only for a mod from Mod Hub, settings only for the room's mods and the
    /// game's own ([`GAME_SETTINGS`]), each once, and the whole within
    /// [`MAX_ROOM_DECLARATION_BYTES`].
    pub fn validate(&self) -> Result<(), RoomModsError> {
        let mods = &self.manifest.mods;
        let total = mods.len()
            + self
                .manifest
                .unlisted
                .map_or(0, |unlisted| unlisted.count as usize);
        if total > MAX_ROOM_MODS {
            return Err(RoomModsError::TooMany(total));
        }
        if self.manifest.unlisted.is_some() || mods.len() != self.room.info.len() {
            return Err(RoomModsError::NotAligned);
        }
        let mut ids = HashSet::new();
        for listed in mods {
            let id = listed.id.as_str();
            if !is_mod_id(id) {
                return Err(RoomModsError::BadId(id.to_owned()));
            }
            if !ids.insert(id) {
                return Err(RoomModsError::Twice(id.to_owned()));
            }
        }
        if mods.last().map(|m| m.id.as_str()) != Some(OWN_MOD) {
            return Err(RoomModsError::OwnMod);
        }
        for (listed, info) in mods.iter().zip(&self.room.info) {
            if info.modio.is_some() && info.source.as_str() != MODIO_SOURCE {
                return Err(RoomModsError::NotModHub(listed.id.as_str().to_owned()));
            }
        }
        let mut with_settings = HashSet::new();
        let mut settings = 0;
        for of in &self.room.params {
            let id = of.id.as_str();
            if !(id == GAME_SETTINGS || ids.contains(id)) || !with_settings.insert(id) {
                return Err(RoomModsError::Settings(id.to_owned()));
            }
            let mut keys = HashSet::new();
            for param in &of.params {
                let key = param.key.as_str();
                if key.is_empty() || key.chars().any(char::is_control) {
                    return Err(RoomModsError::BadSetting(id.to_owned()));
                }
                if !keys.insert(key) {
                    return Err(RoomModsError::SettingTwice(
                        id.to_owned(),
                        param.key.as_str().to_owned(),
                    ));
                }
            }
            settings += of.params.len();
        }
        if settings > MAX_ROOM_PARAMS {
            return Err(RoomModsError::TooManySettings(settings));
        }
        self.validate_native()?;
        let size = postcard::to_allocvec(self).map_or(usize::MAX, |bytes| bytes.len());
        if size > MAX_ROOM_DECLARATION_BYTES {
            return Err(RoomModsError::TooLarge(size));
        }
        Ok(())
    }

    /// The native mods: as many as the manifest names and the same, in its
    /// order, each with a package id, a version and settings keyed once in
    /// order by an index id.
    fn validate_native(&self) -> Result<(), RoomModsError> {
        let named = &self.manifest.native;
        let told = &self.room.native;
        if named.len() != told.len() || told.len() > MAX_NATIVE_TERMS || !self.manifest.is_valid() {
            return Err(RoomModsError::Native);
        }
        let is_id = |text: &str| {
            !text.is_empty()
                && text
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        };
        for (named, native) in named.iter().zip(told) {
            if named.id.as_str().strip_prefix(NATIVE_PREFIX) != Some(native.id.as_str())
                || named.version != native.version
                || !is_id(native.id.as_str())
                || native.package_version().is_empty()
            {
                return Err(RoomModsError::Native);
            }
            let ordered = native
                .settings
                .windows(2)
                .all(|pair| pair[0].key.as_str() < pair[1].key.as_str());
            if !ordered || !native.settings.iter().all(|s| is_id(s.key.as_str())) {
                return Err(RoomModsError::NativeSetting(native.id.as_str().to_owned()));
            }
        }
        Ok(())
    }

    /// What every member is told: the manifest's mods with their info, the
    /// settings, and the native mods.
    pub fn room_mods(&self) -> RoomMods {
        RoomMods {
            game: self.manifest.game.clone(),
            mods: self
                .manifest
                .mods
                .iter()
                .zip(&self.room.info)
                .map(|(listed, info)| RoomMod {
                    id: listed.id.clone(),
                    version: listed.version.clone(),
                    info: info.clone(),
                })
                .collect(),
            params: self.room.params.clone(),
            native: self.room.native.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CONTROL_MAX_FRAME, Request, ServerMessage, Unlisted, encode_frame};

    fn info(name: &str) -> ModInfo {
        ModInfo {
            name: Text::lossy(name),
            source: Text::lossy("StagingArea"),
            modio: None,
        }
    }

    fn declaration(ids: &[&str]) -> RoomDeclaration {
        RoomDeclaration {
            manifest: ContentManifest::new(
                Text::lossy("40408"),
                ids.iter()
                    .map(|id| ModRef {
                        id: Text::lossy(id),
                        version: Text::lossy("1"),
                    })
                    .collect(),
            ),
            room: RoomConfig {
                info: ids.iter().map(|id| info(id)).collect(),
                params: Vec::new(),
                native: Vec::new(),
            },
        }
    }

    fn big_maps(version: &str) -> NativeMod {
        NativeMod {
            id: Text::lossy("bigmap"),
            version: Text::lossy(version),
            name: Text::lossy("Big Maps"),
            settings: vec![
                NativeSetting {
                    key: Text::lossy("octree_depth"),
                    value: NativeValue::Int(10),
                },
                NativeSetting {
                    key: Text::lossy("street_raster"),
                    value: NativeValue::Bool(true),
                },
            ],
        }
    }

    fn full_version(package: &str, settings: &str, identity: char) -> String {
        format!("{package}+{settings}{}", identity.to_string().repeat(64))
    }

    fn with_native(mut room: RoomDeclaration, native: Vec<NativeMod>) -> RoomDeclaration {
        room.manifest = room.manifest.with_native(
            native
                .iter()
                .map(|n| ModRef {
                    id: Text::lossy(&format!("{NATIVE_PREFIX}{}", n.id)),
                    version: n.version.clone(),
                })
                .collect(),
        );
        room.room.native = native;
        room
    }

    #[test]
    fn a_room_tells_its_native_mods_with_their_settings() {
        let version = full_version("0.3.0", "0123456789", 'a');
        let room = with_native(declaration(&["a", OWN_MOD]), vec![big_maps(&version)]);
        assert_eq!(room.validate(), Ok(()));
        let told = room.room_mods();
        assert_eq!(told.native[0].package_version(), "0.3.0");
        assert_eq!(told.native[0].version.as_str().len(), 80);
        assert_eq!(told.manifest(), room.manifest);
        assert_eq!(told.manifest().fingerprint(), room.manifest.fingerprint());
    }

    #[test]
    fn native_mods_must_match_the_manifest_one_for_one() {
        let version = full_version("0.3.0", "0123456789", 'a');
        let other_version = full_version("0.3.1", "0123456789", 'b');
        // Told but not named.
        let mut room = declaration(&["a", OWN_MOD]);
        room.room.native = vec![big_maps(&version)];
        assert_eq!(room.validate(), Err(RoomModsError::Native));
        // Named in another version.
        let mut room = with_native(declaration(&["a", OWN_MOD]), vec![big_maps(&version)]);
        room.room.native[0].version = Text::lossy(&other_version);
        assert_eq!(room.validate(), Err(RoomModsError::Native));
        // An id the index could not have.
        let bad_version = full_version("1.0.0", "0000000000", 'a');
        let mut odd = big_maps(&bad_version);
        odd.id = Text::lossy("Big-Maps");
        assert_eq!(
            with_native(declaration(&[OWN_MOD]), vec![odd]).validate(),
            Err(RoomModsError::Native)
        );
        // Settings out of order or twice.
        let mut twice = big_maps(&bad_version);
        twice.settings.push(twice.settings[0].clone());
        assert_eq!(
            with_native(declaration(&[OWN_MOD]), vec![twice]).validate(),
            Err(RoomModsError::NativeSetting("bigmap".into()))
        );
    }

    #[test]
    fn a_room_with_tpf3mp_last_holds_together() {
        let room = declaration(&["auto_signals_1", "revyn112_towns_de", OWN_MOD]);
        assert_eq!(room.validate(), Ok(()));
        let told = room.room_mods();
        assert_eq!(told.mods.len(), 3);
        assert_eq!(told.mods[2].id.as_str(), OWN_MOD);
        assert_eq!(told.manifest(), room.manifest);
    }

    #[test]
    fn tpf3mp_must_be_last_and_once() {
        assert_eq!(
            declaration(&[OWN_MOD, "a"]).validate(),
            Err(RoomModsError::OwnMod)
        );
        assert_eq!(declaration(&["a"]).validate(), Err(RoomModsError::OwnMod));
        assert_eq!(
            declaration(&[OWN_MOD, OWN_MOD]).validate(),
            Err(RoomModsError::Twice(OWN_MOD.into()))
        );
    }

    #[test]
    fn ids_must_be_the_games_and_once() {
        assert_eq!(
            declaration(&["a b", OWN_MOD]).validate(),
            Err(RoomModsError::BadId("a b".into()))
        );
        assert_eq!(
            declaration(&["a", "a", OWN_MOD]).validate(),
            Err(RoomModsError::Twice("a".into()))
        );
    }

    #[test]
    fn info_must_match_the_manifest_one_for_one() {
        let mut room = declaration(&["a", OWN_MOD]);
        room.room.info.pop();
        assert_eq!(room.validate(), Err(RoomModsError::NotAligned));
    }

    #[test]
    fn a_mod_hub_number_only_for_a_mod_hub_mod() {
        let mut room = declaration(&["towns", OWN_MOD]);
        room.room.info[0].modio = Some(6414521);
        assert_eq!(
            room.validate(),
            Err(RoomModsError::NotModHub("towns".into()))
        );
        room.room.info[0].source = Text::lossy(MODIO_SOURCE);
        assert_eq!(room.validate(), Ok(()));
    }

    #[test]
    fn settings_only_for_the_rooms_mods_each_once() {
        let param = |key: &str| ModParam {
            key: Text::lossy(key),
            value: 2,
        };
        let mut room = declaration(&["signals", OWN_MOD]);
        room.room.params = vec![ModParams {
            id: Text::lossy("signals"),
            params: vec![param("distance"), param("mode")],
        }];
        assert_eq!(room.validate(), Ok(()));
        room.room.params[0].params.push(param("mode"));
        assert_eq!(
            room.validate(),
            Err(RoomModsError::SettingTwice("signals".into(), "mode".into()))
        );
        room.room.params = vec![ModParams {
            id: Text::lossy("other"),
            params: vec![],
        }];
        assert_eq!(
            room.validate(),
            Err(RoomModsError::Settings("other".into()))
        );
        room.room.params = vec![
            ModParams {
                id: Text::lossy("signals"),
                params: vec![],
            };
            2
        ];
        assert_eq!(
            room.validate(),
            Err(RoomModsError::Settings("signals".into()))
        );
    }

    /// The game's own settings travel under the id `""`, once, with the
    /// game's longest names.
    #[test]
    fn the_games_own_settings_travel_too() {
        let mut room = declaration(&["signals", OWN_MOD]);
        let game = ModParams {
            id: Text::lossy(GAME_SETTINGS),
            params: vec![ModParam {
                key: Text::new("economy.industryDevelopment.closureProbability").unwrap(),
                value: 2,
            }],
        };
        room.room.params = vec![game.clone()];
        assert_eq!(room.validate(), Ok(()));
        room.room.params.push(game);
        assert_eq!(room.validate(), Err(RoomModsError::Settings(String::new())));
    }

    /// 257 mods are refused, never cut to 256: every game loads them all.
    #[test]
    fn more_mods_than_a_room_runs_are_refused() {
        let ids: Vec<String> = (0..MAX_ROOM_MODS)
            .map(|n| format!("m{n}"))
            .chain([OWN_MOD.to_owned()])
            .collect();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        assert_eq!(
            declaration(&ids).validate(),
            Err(RoomModsError::TooMany(MAX_ROOM_MODS + 1))
        );
        assert_eq!(declaration(&ids[1..]).validate(), Ok(()));
        let mut summarised = declaration(&["a", OWN_MOD]);
        summarised.manifest.unlisted = Some(Unlisted {
            count: 1,
            digest: crate::FixedBytes([0; 32]),
        });
        assert_eq!(summarised.validate(), Err(RoomModsError::NotAligned));
    }

    /// The largest declaration a room takes fits a control frame, as a
    /// request and as the list every member is told.
    #[test]
    fn the_largest_room_fits_a_frame_both_ways() {
        let id = |n: usize| format!("{n:03}_{}", "m".repeat(91));
        let room = RoomDeclaration {
            manifest: ContentManifest {
                game: Text::lossy(&"b".repeat(64)),
                mods: (0..MAX_ROOM_MODS - 1)
                    .map(|n| ModRef {
                        id: Text::lossy(&id(n)),
                        version: Text::lossy(&"v".repeat(32)),
                    })
                    .chain([ModRef {
                        id: Text::lossy(OWN_MOD),
                        version: Text::lossy(&"v".repeat(32)),
                    }])
                    .collect(),
                unlisted: None,
                native: Vec::new(),
            },
            room: RoomConfig {
                info: (0..MAX_ROOM_MODS)
                    .map(|_| ModInfo {
                        name: Text::lossy(&"n".repeat(48)),
                        source: Text::lossy(MODIO_SOURCE),
                        modio: Some(u64::MAX),
                    })
                    .collect(),
                params: Vec::new(),
                native: Vec::new(),
            },
        };
        // The most mods with the longest of everything fit.
        assert_eq!(room.validate(), Ok(()));
        // With every setting a room carries, the longest of keys too, they
        // do not: refused, never sent.
        let mut crowded = room.clone();
        crowded.room.params = (0..MAX_ROOM_MODS)
            .map(|n| ModParams {
                id: room.manifest.mods[n].id.clone(),
                params: (0..MAX_ROOM_PARAMS / MAX_ROOM_MODS)
                    .map(|k| ModParam {
                        key: Text::lossy(&format!("{k}{}", "k".repeat(47))),
                        value: i64::MIN,
                    })
                    .collect(),
            })
            .collect();
        assert!(matches!(
            crowded.validate(),
            Err(RoomModsError::TooLarge(_))
        ));
        let request = crate::ClientMessage::Request {
            id: u32::MAX,
            request: Request::DeclareRoom(Box::new(room.clone())),
        };
        assert!(encode_frame(&request, CONTROL_MAX_FRAME).is_ok());
        let told = ServerMessage::RoomMods(Some(Box::new(room.room_mods())));
        assert!(encode_frame(&told, CONTROL_MAX_FRAME).is_ok());
    }

    /// The 16-native cap still fits beside the full 256 ordinary mods when
    /// every native term uses the maximum package version and full identity.
    #[test]
    fn max_native_identities_fit_room_declaration_and_control_frames() {
        let native: Vec<NativeMod> = (0..MAX_NATIVE_TERMS)
            .map(|n| NativeMod {
                id: Text::new(format!("n{n:02}")).unwrap(),
                version: Text::new(format!(
                    "{}+{}{}",
                    "v".repeat(crate::MAX_NATIVE_PACKAGE_VERSION),
                    "a".repeat(10),
                    "b".repeat(64)
                ))
                .unwrap(),
                name: Text::new("n").unwrap(),
                settings: Vec::new(),
            })
            .collect();
        let ordinary_count = MAX_ROOM_MODS;
        let mods: Vec<ModRef> = (0..ordinary_count - 1)
            .map(|n| ModRef {
                id: Text::new(format!("m{n}")).unwrap(),
                version: Text::new("1").unwrap(),
            })
            .chain([ModRef {
                id: Text::new(OWN_MOD).unwrap(),
                version: Text::new("1").unwrap(),
            }])
            .collect();
        let room = RoomDeclaration {
            manifest: ContentManifest::new(Text::new("1").unwrap(), mods)
                .with_native(native.iter().map(NativeMod::term).collect()),
            room: RoomConfig {
                info: (0..ordinary_count)
                    .map(|_| ModInfo {
                        name: Text::new("n").unwrap(),
                        source: Text::new("").unwrap(),
                        modio: None,
                    })
                    .collect(),
                params: Vec::new(),
                native,
            },
        };

        assert_eq!(
            room.manifest.mods.len() + room.manifest.native.len(),
            MAX_ROOM_MODS + MAX_NATIVE_TERMS
        );
        assert_eq!(room.manifest.mods.len(), MAX_ROOM_MODS);
        assert_eq!(room.manifest.native.len(), MAX_NATIVE_TERMS);
        assert_eq!(room.validate(), Ok(()));
        assert!(postcard::to_allocvec(&room).unwrap().len() <= MAX_ROOM_DECLARATION_BYTES);
        let request = crate::ClientMessage::Request {
            id: u32::MAX,
            request: Request::DeclareRoom(Box::new(room.clone())),
        };
        assert!(encode_frame(&request, CONTROL_MAX_FRAME).is_ok());
        let told = ServerMessage::RoomMods(Some(Box::new(room.room_mods())));
        assert!(encode_frame(&told, CONTROL_MAX_FRAME).is_ok());

        let mut too_many = room;
        let extra = NativeMod {
            id: Text::new("n16").unwrap(),
            ..too_many.room.native[0].clone()
        };
        too_many.manifest.native.push(extra.term());
        too_many.room.native.push(extra);
        assert_eq!(too_many.validate(), Err(RoomModsError::Native));
    }
}
