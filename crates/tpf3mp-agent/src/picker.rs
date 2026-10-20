//! The player's mods for rooms, found and chosen (docs/MODS.md, "Choosing
//! mods"): every mod this player has installed, scanned, and the personal
//! ones they chose to play with.
//!
//! - **Found by themselves**: Mod Hub's cache, the Steam accounts' local
//!   mods and the game's own `mods`, `mods/release` and `dlcs`
//!   (`tpf3mp_modscan::roots`),
//!   each scanned for its class and the first reason for it. The launcher's
//!   `--mods` list overrides all of this (`crate::content::split`).
//! - **Chosen**: a personal mod (a carried one too with
//!   `--personal-game-scripts`) is played with when the player chose it; a
//!   shared mod is never theirs to choose: every player needs the room's.
//! - **The room's mods** come from the owner's start save
//!   ([`Mods::own_start`]): its mods, less the owner's personal ones; or the
//!   owner picks them with a save on the game's Load Game page, with their
//!   settings and the game's own ([`Mods::choose_room`]). The owner declares them to the room with what
//!   players are told of each ([`Declaration::Room`]). The room tells every
//!   other player the list ([`Mods::adopt`], the room's `RoomMods`), who
//!   declares those they have, so that a player with every one of them
//!   matches the owner. A room that tells none is learned from what it says
//!   a game lacks ([`Mods::learn`], the room's `ContentDiff`).
//!
//! What this gives the room: what a player declares
//! ([`Mods::declaration`]) and the lists the room's world loads with in
//! their game ([`Mods::lists`]).

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use tpf3mp_bridge::{
    ModLists, ModName,
    mods::{MAX_PERSONAL_MODS, OWN_MOD},
};
use tpf3mp_modscan::{Class, roots, save::SaveMod};
use tpf3mp_proto::{
    BoundedVec, ContentDiff, ContentManifest, ModId, ModInfo, ModParams, ModRef, NATIVE_PREFIX,
    NativeMod, RoomConfig, RoomDeclaration, RoomMod, RoomMods, Text,
};

/// Longest reason kept for a mod, in bytes.
const MAX_REASON: usize = 96;

/// One installed mod, as the scan saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// Its id (`modId`), as saves and the game name it.
    pub id: String,
    /// Its name for players (`_metadata/modinfo.json`), else its id.
    pub name: String,
    /// The version the room compares: its `revision`; for a Mod Hub
    /// download also the file installed ([`hub_version`]); for TPF3-MP's
    /// own mod also its files' fingerprint.
    pub version: String,
    /// For a Mod Hub download: which mod and file it is.
    pub hub: Option<roots::HubFile>,
    pub class: Class,
    /// Why, in a line: the first reason that makes it shared, or what it is.
    pub reason: String,
    pub path: PathBuf,
}

/// Every mod in the places this player keeps them, scanned; the first of
/// each id counts.
pub fn discover(game: Option<&Path>, steam_roots: &[PathBuf]) -> Vec<Installed> {
    scanned(&roots::installed(&roots::default_roots(game, steam_roots)))
}

/// The mods `found`, scanned; the first of each id counts.
pub fn scanned(found: &[roots::Found]) -> Vec<Installed> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for mod_found in found {
        if !seen.insert(mod_found.id.clone()) {
            continue;
        }
        let report = tpf3mp_modscan::scan(&mod_found.path);
        let reason = match report.sharing().next() {
            Some(reason) => format!("{}: {}", kind_word(report.class), reason.detail),
            None => "only what this player sees".to_owned(),
        };
        let revision = report.revision.map(|r| r.to_string()).unwrap_or_default();
        out.push(Installed {
            id: mod_found.id.clone(),
            name: display_name(&mod_found.path).unwrap_or_else(|| mod_found.id.clone()),
            // TPF3-MP's own mod: its revision and the fingerprint of its
            // files, so that an old copy of the same revision differs.
            version: if mod_found.id == OWN_MOD {
                crate::own_mod::version(&revision, &mod_found.path)
            } else if let Some(hub) = mod_found.hub {
                hub_version(&revision, hub, &mod_found.id)
            } else {
                revision
            },
            hub: mod_found.hub,
            class: report.class,
            reason: shorten(&reason),
            path: mod_found.path.clone(),
        });
    }
    out
}

/// The version the room compares for a Mod Hub download of `revision`:
/// `revision+m<file>`, the file mod.io installed, so that two downloads of
/// one revision whose files differ do not match (most mods stay at revision
/// 1). A download whose file mod.io's index does not give (not finished, an
/// update pending, no index) gets a version no other game has (fail closed),
/// and a warning in the log.
pub fn hub_version(revision: &str, hub: roots::HubFile, id: &str) -> String {
    // Short enough to fit a manifest's version with the largest file id.
    let revision: String = revision.chars().take(8).collect();
    match hub.file {
        Some(file) => format!("{revision}+m{file}"),
        None => {
            tracing::warn!(
                "Mod Hub's mod {id} ({}): mod.io's index gives no installed file, so the room cannot compare it",
                hub.mod_id
            );
            let mut unique = [0u8; 4];
            let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut unique);
            let unique: String = unique.iter().map(|b| format!("{b:02x}")).collect();
            format!("{revision}+m?{unique}")
        }
    }
}

fn kind_word(class: Class) -> &'static str {
    match class {
        Class::Carried => "decides in the simulation",
        Class::Personal | Class::Shared => "every player needs it",
    }
}

fn shorten(text: &str) -> String {
    if text.len() <= MAX_REASON {
        return text.to_owned();
    }
    let mut end = MAX_REASON - 3;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// The mod's name for players, from `_metadata/modinfo.json`.
fn display_name(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join("_metadata").join("modinfo.json")).ok()?;
    let info: serde_json::Value = serde_json::from_str(&text).ok()?;
    info.get("name")?
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// Whether a mod of `class` may be chosen, with `carried_personal`.
pub fn choosable(class: Class, carried_personal: bool) -> bool {
    match class {
        Class::Personal => true,
        Class::Carried => carried_personal,
        Class::Shared => false,
    }
}

/// One of the room's mods, and whether this player has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Required {
    pub id: String,
    /// What players are told of it: its name, where it comes from, its Mod
    /// Hub number.
    pub info: ModInfo,
    /// The room's version of it (empty when the room's owner lacks it).
    pub version: String,
    /// This player's version, if installed.
    pub yours: Option<String>,
    pub have: Have,
}

/// Whether this player has one of the room's mods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Have {
    Yes,
    No,
    /// Installed in another version.
    OtherVersion,
}

/// One mod the room's owner picked in the game's mod selector, in its
/// activation order, with what the game says of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected {
    pub id: String,
    pub info: ModInfo,
}

/// What this player declares to the room: their content, or as the room's
/// owner, their content and the room's mods together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Declaration {
    Content(ContentManifest),
    Room(RoomDeclaration),
}

impl Declaration {
    /// The content declared.
    pub fn manifest(&self) -> &ContentManifest {
        match self {
            Self::Content(manifest) => manifest,
            Self::Room(room) => &room.manifest,
        }
    }

    /// The request that declares it.
    pub fn request(self) -> tpf3mp_proto::Request {
        match self {
            Self::Content(manifest) => tpf3mp_proto::Request::DeclareContent(manifest),
            Self::Room(room) => tpf3mp_proto::Request::DeclareRoom(Box::new(room)),
        }
    }
}

/// The room's mods as this player knows them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Room {
    /// In load order, with the room's versions and what players are told.
    mods: Vec<RoomMod>,
    /// The settings the room's world loads them with.
    params: Vec<ModParams>,
    /// Whether this is the room's whole list (its owner's, or as the room
    /// told it), not pieced together from what the room said this game
    /// lacks ([`Mods::learn`]).
    whole: bool,
    /// The room's native mods with their settings, as told
    /// (docs/NATIVE_MODS.md); the owner's are their game's.
    native: Vec<NativeMod>,
}

/// What players are told of TPF3-MP's own mod.
fn own_info() -> ModInfo {
    ModInfo {
        name: Text::lossy("TPF3-MP"),
        source: Text::lossy("StagingArea"),
        modio: None,
    }
}

/// This player's mods for rooms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mods {
    build: Text<64>,
    installed: Vec<Installed>,
    chosen: BTreeSet<String>,
    carried_personal: bool,
    /// The room's mods, once known.
    room: Option<Room>,
    /// Whether they are this player's own, as the room's owner.
    owner: bool,
    /// The native mods that change the simulation which this player's game
    /// runs, as the launcher started it ([`Mods::set_native`]).
    native: Vec<NativeMod>,
}

impl Mods {
    /// `installed` for a game of `build`, with the mods the player chose
    /// before (those no longer installed or choosable are dropped).
    pub fn new(
        build: Text<64>,
        installed: Vec<Installed>,
        chosen: impl IntoIterator<Item = String>,
        carried_personal: bool,
    ) -> Self {
        let mut mods = Self {
            build,
            installed,
            chosen: BTreeSet::new(),
            carried_personal,
            room: None,
            owner: false,
            native: Vec::new(),
        };
        for id in chosen {
            let _ = mods.choose(&id, true);
        }
        mods
    }

    pub fn installed(&self) -> &[Installed] {
        &self.installed
    }

    /// The native mods that change the simulation which this player's game
    /// runs now: what it declares beside its mods, and as the room's owner
    /// the room's. Returns whether they changed.
    pub fn set_native(&mut self, native: Vec<NativeMod>) -> bool {
        let changed = self.native != native;
        self.native = native;
        if self.owner
            && let Some(room) = &mut self.room
        {
            room.native = self.native.clone();
        }
        changed
    }

    /// The room's native mods with their settings, when the room told its
    /// whole list.
    pub fn room_native(&self) -> Option<&[NativeMod]> {
        self.room
            .as_ref()
            .filter(|room| room.whole)
            .map(|room| room.native.as_slice())
    }

    /// Takes in the mods found again (after a Mod Hub download, say),
    /// keeping the choices of those still choosable. Returns whether
    /// anything changed.
    pub fn rescan(&mut self, installed: Vec<Installed>) -> bool {
        if installed == self.installed {
            return false;
        }
        self.installed = installed;
        let chosen = std::mem::take(&mut self.chosen);
        for id in chosen {
            let _ = self.choose(&id, true);
        }
        true
    }

    fn find(&self, id: &str) -> Option<&Installed> {
        self.installed.iter().find(|m| m.id == id)
    }

    /// Whether the mod `id` may be chosen: an installed personal mod, or a
    /// carried one with `--personal-game-scripts`.
    pub fn is_choosable(&self, id: &str) -> bool {
        self.find(id)
            .is_some_and(|m| choosable(m.class, self.carried_personal))
    }

    pub fn is_chosen(&self, id: &str) -> bool {
        self.chosen.contains(id)
    }

    /// The mods chosen, by id, to remember.
    pub fn chosen(&self) -> Vec<String> {
        self.chosen.iter().cloned().collect()
    }

    /// Whether this player owns the room's mods.
    pub fn is_owner(&self) -> bool {
        self.owner
    }

    /// Chooses the mod `id`, or not; refused for one that is not choosable.
    pub fn choose(&mut self, id: &str, chosen: bool) -> Result<(), String> {
        if !chosen {
            self.chosen.remove(id);
            return Ok(());
        }
        match self.find(id) {
            None => Err(format!("no mod {id} is installed")),
            Some(m) if !choosable(m.class, self.carried_personal) => Err(match m.class {
                Class::Carried => format!(
                    "{} decides in the simulation: every player needs it (or start with --personal-game-scripts)",
                    m.name
                ),
                _ => format!("{} changes the world: every player needs it", m.name),
            }),
            Some(_) => {
                self.chosen.insert(id.to_owned());
                Ok(())
            }
        }
    }

    /// The room's mod `id` with this player's version, or none when this
    /// player lacks it (still the room's: fail closed).
    /// One of the room's mods as this player (its owner) has it: in their
    /// version, and, for their copy from Mod Hub, said to come from there
    /// with its number, whatever `info` (a save's memory, or the game's)
    /// says: a save remembers where its mods came from when it was made.
    fn room_mod(&self, id: &str, mut info: ModInfo) -> RoomMod {
        let here = self.find(id);
        if let Some(hub) = here.and_then(|here| here.hub) {
            info.source = Text::lossy(tpf3mp_proto::MODIO_SOURCE);
            info.modio = Some(hub.mod_id);
        }
        RoomMod {
            id: Text::lossy(id),
            version: Text::lossy(here.map_or("", |here| here.version.as_str())),
            info,
        }
    }

    /// The room this player owns starts from a save listing `save`: its
    /// mods, less this player's personal ones (whether chosen or not), are
    /// the room's, in the save's order, each in this player's version (none
    /// when not installed here: still the room's, fail closed), with what
    /// the save says of it; TPF3-MP's own last. The save's settings stay
    /// the save's.
    ///
    /// A list that cannot be the room's (more mods than a room runs) is
    /// still compared whole, but not told nor loaded as the room's: every
    /// game then loads the save's own mods, as before the room had a list,
    /// and the fingerprint still covers every one of them. `Err` says why.
    pub fn own_start(&mut self, save: &[SaveMod]) -> Result<(), String> {
        let mut mods: Vec<RoomMod> = save
            .iter()
            .filter(|m| m.id != OWN_MOD && !self.is_choosable(&m.id))
            .map(|m| {
                self.room_mod(
                    &m.id,
                    ModInfo {
                        name: Text::lossy(&m.name),
                        source: Text::lossy(&m.source),
                        modio: m.modio_id(),
                    },
                )
            })
            .collect();
        mods.push(self.room_mod(OWN_MOD, own_info()));
        self.room = Some(Room {
            mods,
            params: Vec::new(),
            whole: true,
            native: self.native.clone(),
        });
        self.owner = true;
        self.declaration().map(drop).inspect_err(|_| {
            if let Some(room) = &mut self.room {
                room.whole = false;
            }
        })
    }

    /// The room's owner picked the room's mods in the game's mod selector:
    /// `selection` in activation order, with the settings the selector
    /// holds. Their personal mods in it become the ones they play with (and
    /// those left out, not); every other is the room's, in that order, with
    /// TPF3-MP's own last whatever the selection says, and the settings of
    /// the room's mods are the room's. Refused for a mod not installed here,
    /// or a room that does not hold together ([`Mods::declaration`]);
    /// nothing changes then.
    pub fn choose_room(
        &mut self,
        selection: &[Selected],
        params: Vec<ModParams>,
    ) -> Result<(), String> {
        let mut next = self.clone();
        let mut mods = Vec::new();
        let mut personal = BTreeSet::new();
        for picked in selection {
            if picked.id == OWN_MOD {
                continue;
            }
            let Some(here) = self.find(&picked.id) else {
                return Err(format!("{} is not installed", picked.id));
            };
            if choosable(here.class, self.carried_personal) {
                personal.insert(picked.id.clone());
            } else if !mods.iter().any(|m: &RoomMod| m.id.as_str() == picked.id) {
                mods.push(self.room_mod(&picked.id, picked.info.clone()));
            }
        }
        mods.push(self.room_mod(OWN_MOD, own_info()));
        // Settings of the room's mods and the game's own; a personal mod's
        // stay this player's.
        let params = params
            .into_iter()
            .filter(|of| {
                of.id.as_str() == tpf3mp_proto::GAME_SETTINGS || mods.iter().any(|m| m.id == of.id)
            })
            .collect();
        next.chosen = next
            .chosen
            .iter()
            .filter(|id| !next.is_choosable(id))
            .cloned()
            .collect();
        next.chosen.extend(personal);
        next.room = Some(Room {
            mods,
            params,
            whole: true,
            native: self.native.clone(),
        });
        next.owner = true;
        next.declaration()?;
        *self = next;
        Ok(())
    }

    /// A room with no start save of this player's, or none at all.
    pub fn forget_room(&mut self) {
        self.room = None;
        self.owner = false;
    }

    /// Takes in the room's mods as the room tells them (`RoomMods`), or
    /// that it has none, `owns` whether this player owns the room now.
    /// Returns whether they changed, and so what this player declares. The
    /// owner's own list is never changed by it; a player who owned the room
    /// before (it passed on while they were away) takes the room's as any
    /// member does.
    pub fn adopt(&mut self, told: Option<&RoomMods>, owns: bool) -> bool {
        if self.owner && owns {
            return false;
        }
        self.owner = false;
        let room = told.map(|told| Room {
            mods: told.mods.clone(),
            params: told.params.clone(),
            whole: true,
            native: told.native.clone(),
        });
        let changed = self.room != room;
        self.room = room;
        changed
    }

    /// Takes in what the room says this player's game lacks, or has in
    /// another version, compared with the owner's: for a room that tells no
    /// list of its mods. Returns whether they changed, and so what this
    /// player declares. Never changes a list the room told, nor the
    /// owner's.
    pub fn learn(&mut self, diff: &ContentDiff) -> bool {
        if self.owner || self.room.as_ref().is_some_and(|room| room.whole) {
            return false;
        }
        let known = |id: &str| ModInfo {
            name: Text::lossy(id),
            source: Text::lossy(""),
            modio: None,
        };
        let mut room = self.room.clone().map(|room| room.mods).unwrap_or_default();
        // Native mods are not mods a world loads: the launcher gets them.
        let native = |id: &ModId| id.as_str().starts_with(NATIVE_PREFIX);
        for missing in diff.missing.iter().filter(|m| !native(&m.id)) {
            if !room.iter().any(|m| m.id == missing.id) {
                room.push(RoomMod {
                    id: missing.id.clone(),
                    version: missing.version.clone(),
                    info: known(missing.id.as_str()),
                });
            }
        }
        for change in diff.changed.iter().filter(|c| !native(&c.id)) {
            match room.iter_mut().find(|m| m.id == change.id) {
                Some(m) => m.version = change.room.clone(),
                None => room.push(RoomMod {
                    id: change.id.clone(),
                    version: change.room.clone(),
                    info: known(change.id.as_str()),
                }),
            }
        }
        // What the room does not run is not the room's.
        room.retain(|m| !diff.extra.iter().any(|extra| extra.id == m.id));
        let room = Some(Room {
            mods: room,
            params: Vec::new(),
            whole: false,
            native: Vec::new(),
        });
        let changed = self.room != room;
        self.room = room;
        changed
    }

    /// What this player declares to the room's comparison: the build, then
    /// the room's mods in the room's order, in this player's versions:
    /// every one for the room's owner (none for a mod they lack, which
    /// therefore nobody matches), those this player has for anyone else;
    /// then TPF3-MP's own mod, whatever the room's list or save says: every
    /// game of the room runs it, and its version carries the fingerprint of
    /// its files (`crate::own_mod`). It goes last in every player's manifest
    /// alike, so that where it stands in a save never makes two players
    /// differ.
    pub fn manifest(&self) -> ContentManifest {
        let mut mods: Vec<ModRef> = self
            .room
            .iter()
            .flat_map(|room| &room.mods)
            .filter(|m| m.id.as_str() != OWN_MOD)
            .filter_map(|m| {
                let version = match self.find(m.id.as_str()) {
                    Some(here) => here.version.as_str(),
                    None if self.owner => "",
                    None => return None,
                };
                Some(ModRef {
                    id: m.id.clone(),
                    version: Text::lossy(version),
                })
            })
            .collect();
        if let Some(own) = self.find(OWN_MOD) {
            mods.push(ModRef {
                id: Text::lossy(OWN_MOD),
                version: Text::lossy(&own.version),
            });
        }
        ContentManifest::new(self.build.clone(), mods)
            .with_native(self.native.iter().map(NativeMod::term).collect())
    }

    /// What this player declares: as the room's owner with the room's whole
    /// list, their content and the room's mods together, refused when that
    /// does not hold together (too many mods, TPF3-MP missing); otherwise
    /// their content.
    pub fn declaration(&self) -> Result<Declaration, String> {
        let manifest = self.manifest();
        let Some(room) = self.room.as_ref().filter(|room| self.owner && room.whole) else {
            return Ok(Declaration::Content(manifest));
        };
        let info = manifest
            .mods
            .iter()
            .map(|listed| {
                room.mods
                    .iter()
                    .find(|m| m.id == listed.id)
                    .map_or_else(own_info, |m| m.info.clone())
            })
            .collect();
        let declaration = RoomDeclaration {
            manifest,
            room: RoomConfig {
                info,
                params: room.params.clone(),
                native: self.native.clone(),
            },
        };
        declaration
            .validate()
            .map_err(|error| format!("the room's mods: {error}"))?;
        Ok(Declaration::Room(declaration))
    }

    /// The mods the room's worlds load with in this game: the room's mods in
    /// the room's order and the chosen personal ones, with the room's
    /// settings. None while the room's whole list is not known (a world then
    /// loads with its save's own, as the owner's does then).
    pub fn lists(&self) -> Option<ModLists> {
        let room = self.room.as_ref().filter(|room| room.whole)?;
        let mut shared = room
            .mods
            .iter()
            .map(|m| ModName::new(m.id.as_str()).ok())
            .collect::<Option<Vec<_>>>()?;
        if !shared.iter().any(|m| m.as_str() == OWN_MOD) {
            shared.push(ModName::new(OWN_MOD).ok()?);
        }
        let mut personal: Vec<ModName> = self
            .installed
            .iter()
            .filter(|m| self.chosen.contains(&m.id) && self.is_choosable(&m.id))
            .filter_map(|m| ModName::new(&m.id).ok())
            .collect();
        if personal.len() > MAX_PERSONAL_MODS {
            tracing::warn!(
                chosen = personal.len(),
                "more personal mods chosen than a room's world loads: the first {MAX_PERSONAL_MODS} load"
            );
            personal.truncate(MAX_PERSONAL_MODS);
        }
        Some(ModLists {
            shared: BoundedVec::new(shared).ok()?,
            personal: BoundedVec::new(personal).ok()?,
            params: room.params.clone(),
        })
    }

    /// The settings of the room's mods, as the room's owner picked them.
    pub fn room_params(&self) -> Vec<ModParams> {
        self.room
            .as_ref()
            .map(|room| room.params.clone())
            .unwrap_or_default()
    }

    /// The room's mods, and whether this player has each, TPF3-MP's own
    /// among them once the room's are known: every player declares it
    /// ([`Mods::manifest`]).
    pub fn required(&self) -> Vec<Required> {
        let Some(room) = &self.room else {
            return Vec::new();
        };
        let own = (!room.mods.iter().any(|m| m.id.as_str() == OWN_MOD))
            .then(|| self.room_mod(OWN_MOD, own_info()));
        room.mods
            .iter()
            .chain(own.as_ref())
            .map(|m| {
                let here = self.find(m.id.as_str());
                Required {
                    id: m.id.as_str().to_owned(),
                    info: m.info.clone(),
                    version: m.version.as_str().to_owned(),
                    yours: here.map(|here| here.version.clone()),
                    have: match here {
                        None => Have::No,
                        Some(here)
                            if m.version.as_str().is_empty()
                                || here.version == m.version.as_str() =>
                        {
                            Have::Yes
                        }
                        Some(_) => Have::OtherVersion,
                    },
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(id: &str, class: Class, version: &str) -> Installed {
        Installed {
            id: id.into(),
            name: id.into(),
            version: version.into(),
            class,
            reason: String::new(),
            path: PathBuf::new(),
            hub: None,
        }
    }

    /// Two downloads of one revision differ by their file; one whose file
    /// is unknown matches no other game.
    #[test]
    fn a_mod_hub_download_is_compared_by_its_file() {
        let hub = |file| roots::HubFile {
            mod_id: 6414521,
            file,
        };
        assert_eq!(hub_version("1", hub(Some(8264750)), "towns"), "1+m8264750");
        assert_ne!(
            hub_version("1", hub(Some(8264750)), "towns"),
            hub_version("1", hub(Some(9000001)), "towns")
        );
        let unknown = hub_version("1", hub(None), "towns");
        assert!(unknown.starts_with("1+m?"), "{unknown}");
        assert_ne!(unknown, hub_version("1", hub(None), "towns"));
        // The largest file id still fits a manifest's version.
        let longest = hub_version("123456789012", hub(Some(u64::MAX)), "towns");
        assert!(Text::<32>::new(&longest).is_ok(), "{longest}");
    }

    fn catalog() -> Vec<Installed> {
        vec![
            installed("tpf3mp_1", Class::Shared, "1"),
            installed("vehicles_pack", Class::Shared, "3"),
            installed("minimap", Class::Personal, "1"),
            installed("timetables", Class::Carried, "8"),
        ]
    }

    fn save(ids: &[&str]) -> Vec<SaveMod> {
        ids.iter()
            .map(|id| SaveMod {
                id: (*id).into(),
                source: "StagingArea".into(),
                hub: format!("StagingArea,{id}"),
                name: (*id).into(),
            })
            .collect()
    }

    fn names(manifest: &ContentManifest) -> Vec<String> {
        manifest
            .mods
            .iter()
            .map(|m| format!("{} {}", m.id, m.version))
            .collect()
    }

    #[test]
    fn only_personal_mods_are_chosen_and_carried_ones_on_request() {
        let mut mods = Mods::new(
            Text::lossy("40408"),
            catalog(),
            ["minimap".into(), "timetables".into(), "gone".into()],
            false,
        );
        assert_eq!(
            mods.chosen(),
            ["minimap"],
            "remembered, less what is not choosable"
        );
        assert!(mods.choose("vehicles_pack", true).is_err());
        assert!(
            mods.choose("timetables", true)
                .unwrap_err()
                .contains("--personal-game-scripts")
        );
        let mut open = Mods::new(Text::lossy("40408"), catalog(), [], true);
        open.choose("timetables", true).unwrap();
        open.choose("timetables", false).unwrap();
        assert!(open.chosen().is_empty());
        mods.choose("minimap", false).unwrap();
        assert!(mods.chosen().is_empty());
    }

    /// A save remembers where its mods came from when it was made; the room
    /// says where the owner's copy comes from now: Mod Hub, with its number.
    #[test]
    fn the_room_names_the_owners_mod_hub_copy_whatever_the_save_says() {
        let mut list = catalog();
        list[1].hub = Some(roots::HubFile {
            mod_id: 6414521,
            file: Some(8264750),
        });
        let mut owner = Mods::new(Text::lossy("40408"), list, [], false);
        owner.own_start(&save(&["vehicles_pack"])).unwrap();
        let room = told(owner.declaration().unwrap());
        assert_eq!(room.mods[0].info.source.as_str(), "mod.io");
        assert_eq!(room.mods[0].info.modio, Some(6414521));
        assert_eq!(room.mods[1].id.as_str(), OWN_MOD);
        assert_eq!(room.mods[1].info.modio, None);
    }

    /// The game's own settings (difficulty, costs, ...) travel with the
    /// room's; a personal mod's stay this player's.
    #[test]
    fn the_games_own_settings_are_the_rooms_too() {
        let mut owner = Mods::new(Text::lossy("40408"), catalog(), [], false);
        let selected = |id: &str| Selected {
            id: id.into(),
            info: ModInfo {
                name: Text::lossy(id),
                source: Text::lossy("StagingArea"),
                modio: None,
            },
        };
        let params = |id: &str| ModParams {
            id: Text::lossy(id),
            params: vec![tpf3mp_proto::ModParam {
                key: Text::lossy("advancedOptions.inflationFactor"),
                value: 3,
            }],
        };
        owner
            .choose_room(
                &[
                    selected("vehicles_pack"),
                    selected("minimap"),
                    selected(OWN_MOD),
                ],
                vec![
                    params(tpf3mp_proto::GAME_SETTINGS),
                    params("minimap"),
                    params("vehicles_pack"),
                ],
            )
            .unwrap();
        let ids: Vec<String> = owner
            .room_params()
            .iter()
            .map(|of| of.id.as_str().to_owned())
            .collect();
        assert_eq!(ids, ["", "vehicles_pack"]);
    }

    fn room_ids(mods: &Mods) -> Vec<String> {
        mods.required().into_iter().map(|r| r.id).collect()
    }

    fn told(declaration: Declaration) -> RoomMods {
        match declaration {
            Declaration::Room(room) => room.room_mods(),
            Declaration::Content(_) => panic!("the owner declares the room's mods"),
        }
    }

    #[test]
    fn the_owners_start_save_makes_the_rooms_mods_and_a_guest_adopts_them() {
        let mut owner = Mods::new(Text::lossy("40408"), catalog(), ["minimap".into()], false);
        assert_eq!(
            owner.lists(),
            None,
            "no room yet: saves load with their own"
        );
        assert_eq!(
            names(&owner.manifest()),
            ["tpf3mp_1 1"],
            "TPF3-MP itself, before the room's mods are known"
        );
        let mut start = save(&["vehicles_pack", "tpf3mp_1", "minimap", "dlc_pack"]);
        start[0].source = "mod.io".into();
        start[0].hub = "6414521".into();
        start[0].name = "Vehicles".into();
        owner.own_start(&start).unwrap();
        assert_eq!(
            names(&owner.manifest()),
            ["vehicles_pack 3", "dlc_pack ", "tpf3mp_1 1"],
            "the owner's minimap is theirs; a mod they lack is still the room's, matching nobody"
        );
        let room = told(owner.declaration().unwrap());
        assert_eq!(room.mods[0].info.name.as_str(), "Vehicles");
        assert_eq!(room.mods[0].info.modio, Some(6414521));
        assert_eq!(room.mods[2].id.as_str(), "tpf3mp_1");
        let lists = owner.lists().unwrap();
        let shared: Vec<&str> = lists.shared.iter().map(|m| m.as_str()).collect();
        assert_eq!(shared, ["vehicles_pack", "dlc_pack", "tpf3mp_1"]);
        assert_eq!(lists.personal.len(), 1);
        assert_eq!(
            owner.required().iter().map(|r| r.have).collect::<Vec<_>>(),
            [Have::Yes, Have::No, Have::Yes]
        );

        // A guest with an older vehicle pack and no minimap: told the
        // room's mods, declares those it has, in the room's order.
        let mut guest = Mods::new(
            Text::lossy("40408"),
            vec![
                installed("tpf3mp_1", Class::Shared, "1"),
                installed("vehicles_pack", Class::Shared, "2"),
            ],
            [],
            false,
        );
        assert!(guest.adopt(Some(&room), false));
        assert!(
            !guest.adopt(Some(&room), false),
            "nothing new: no second declaration"
        );
        assert_eq!(names(&guest.manifest()), ["vehicles_pack 2", "tpf3mp_1 1"]);
        assert!(matches!(guest.declaration(), Ok(Declaration::Content(_))));
        let required = guest.required();
        assert_eq!(
            required.iter().map(|r| r.have).collect::<Vec<_>>(),
            [Have::OtherVersion, Have::No, Have::Yes]
        );
        assert_eq!(required[0].yours.as_deref(), Some("2"));
        assert_eq!(required[0].info.modio, Some(6414521));
        // The guest's world loads with the room's list, as the owner's.
        let loads = guest.lists().unwrap();
        assert_eq!(loads.shared, lists.shared);
        // What a room that told its list says it lacks changes nothing.
        let diff = owner.manifest().compare(&guest.manifest()).unwrap();
        assert!(!guest.learn(&diff));
        // The owner is never told what the room is.
        assert!(!owner.adopt(Some(&room), true));
        assert!(!owner.learn(&diff));
        // A room that has no list any more: back to the save's own.
        assert!(guest.adopt(None, false));
        assert_eq!(guest.lists(), None);
        // The room passed to another while its owner was away: what it
        // tells them now is theirs to load, its settings included.
        let mut theirs = room.clone();
        theirs.params = vec![ModParams {
            id: Text::new(tpf3mp_proto::GAME_SETTINGS).unwrap(),
            params: vec![tpf3mp_proto::ModParam {
                key: Text::new("difficulty").unwrap(),
                value: 2,
            }],
        }];
        assert!(owner.adopt(Some(&theirs), false));
        assert!(!owner.is_owner());
        assert_eq!(owner.lists().unwrap().params, theirs.params);
    }

    /// The owner's game's native mods are the room's, told with their
    /// settings; a guest's are declared beside their mods, and a native mod
    /// the room says they lack is never taken for a mod a world loads.
    #[test]
    fn native_mods_go_with_the_room_but_never_into_a_worlds_mods() {
        let big_maps = |version: &str| NativeMod {
            id: Text::lossy("bigmap"),
            version: Text::lossy(&format!("{}{}", version, "a".repeat(64))),
            name: Text::lossy("Big Maps"),
            settings: vec![tpf3mp_proto::NativeSetting {
                key: Text::lossy("depth"),
                value: tpf3mp_proto::NativeValue::Int(12),
            }],
        };
        let mut owner = Mods::new(Text::lossy("40408"), catalog(), [], false);
        owner.own_start(&save(&["tpf3mp_1"])).unwrap();
        assert!(owner.set_native(vec![big_maps("0.3.0+0123456789")]));
        assert!(!owner.set_native(vec![big_maps("0.3.0+0123456789")]));
        let declared = owner.declaration().unwrap();
        assert_eq!(declared.manifest().native.len(), 1);
        let room = told(declared);
        assert_eq!(room.native, [big_maps("0.3.0+0123456789")]);
        assert_eq!(room.manifest(), owner.manifest());

        let mut guest = Mods::new(Text::lossy("40408"), catalog(), [], false);
        assert!(guest.adopt(Some(&room), false));
        assert_eq!(guest.room_native(), Some(&room.native[..]));
        // Without them, the guest differs by the native mod alone.
        let diff = room.manifest().compare(&guest.manifest()).unwrap();
        assert_eq!(diff.missing_total, 1);
        guest.set_native(room.native.clone());
        assert_eq!(room.manifest().compare(&guest.manifest()), None);

        // A room that told no list: learning leaves the native mod out of
        // the mods.
        let mut learner = Mods::new(Text::lossy("40408"), catalog(), [], false);
        let diff = room.manifest().compare(&learner.manifest()).unwrap();
        learner.learn(&diff);
        assert!(
            learner
                .manifest()
                .mods
                .iter()
                .all(|m| !m.id.as_str().starts_with(NATIVE_PREFIX))
        );
    }

    /// A list pieced together from what the room said this game lacks is
    /// never loaded: it may lack what the room said nothing of.
    #[test]
    fn a_learned_list_declares_but_never_loads() {
        let mut guest = Mods::new(Text::lossy("40408"), catalog(), [], false);
        let room = ContentManifest::new(
            Text::lossy("40408"),
            vec![
                ModRef {
                    id: Text::lossy("vehicles_pack"),
                    version: Text::lossy("3"),
                },
                ModRef {
                    id: Text::lossy("tpf3mp_1"),
                    version: Text::lossy("1"),
                },
            ],
        );
        let diff = room.compare(&guest.manifest()).unwrap();
        assert!(guest.learn(&diff));
        assert_eq!(room.compare(&guest.manifest()), None);
        assert_eq!(guest.lists(), None);
    }

    #[test]
    fn the_owner_picks_the_rooms_mods_and_settings_in_the_selector() {
        let mut owner = Mods::new(
            Text::lossy("40408"),
            vec![
                installed("tpf3mp_1", Class::Shared, "1"),
                installed("vehicles_pack", Class::Shared, "3"),
                installed("signals", Class::Shared, "2"),
                installed("minimap", Class::Personal, "1"),
                installed("colours", Class::Personal, "1"),
            ],
            ["colours".into()],
            false,
        );
        owner
            .own_start(&save(&["vehicles_pack", "tpf3mp_1"]))
            .unwrap();
        let picked = |id: &str| Selected {
            id: id.into(),
            info: ModInfo {
                name: Text::lossy(id),
                source: Text::lossy("StagingArea"),
                modio: None,
            },
        };
        let setting = |id: &str| ModParams {
            id: Text::lossy(id),
            params: vec![tpf3mp_proto::ModParam {
                key: Text::lossy("distance"),
                value: 4,
            }],
        };
        // TPF3-MP first in the selection, a personal mod among the room's.
        owner
            .choose_room(
                &[
                    picked("tpf3mp_1"),
                    picked("signals"),
                    picked("minimap"),
                    picked("vehicles_pack"),
                ],
                vec![setting("signals"), setting("minimap")],
            )
            .unwrap();
        assert_eq!(
            room_ids(&owner),
            ["signals", "vehicles_pack", "tpf3mp_1"],
            "the selection's order, TPF3-MP last, the personal mod not the room's"
        );
        assert_eq!(owner.chosen(), ["minimap"], "colours was left out");
        let lists = owner.lists().unwrap();
        assert_eq!(
            lists.params,
            [setting("signals")],
            "the room's mods' settings"
        );
        let room = told(owner.declaration().unwrap());
        assert_eq!(room.params, [setting("signals")]);

        // A mod not installed here: refused, nothing changes.
        let before = owner.clone();
        assert!(
            owner
                .choose_room(&[picked("not_here")], Vec::new())
                .unwrap_err()
                .contains("not_here")
        );
        assert_eq!(owner, before);
    }

    /// A start save with more mods than a room runs is refused as the
    /// room's list, never cut short.
    #[test]
    fn more_mods_than_a_room_runs_are_never_the_rooms() {
        let many: Vec<String> = (0..tpf3mp_proto::MAX_ROOM_MODS)
            .map(|n| format!("m{n}"))
            .collect();
        let mut installed_mods: Vec<Installed> = many
            .iter()
            .map(|id| installed(id, Class::Shared, "1"))
            .collect();
        installed_mods.push(installed("tpf3mp_1", Class::Shared, "1"));
        let mut owner = Mods::new(Text::lossy("40408"), installed_mods, [], false);
        let ids: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(
            owner
                .own_start(&save(&ids))
                .unwrap_err()
                .contains("at most"),
            "256 and TPF3-MP"
        );
        // Still compared, every one of them: the room's fingerprint covers
        // what every game loads from the save.
        let Ok(Declaration::Content(manifest)) = owner.declaration() else {
            panic!("the owner's content, without a room's list")
        };
        assert_eq!(manifest.mods.len(), tpf3mp_proto::MAX_ROOM_MODS + 1);
        assert!(owner.lists().is_none(), "the save's own mods load");
    }

    #[test]
    fn tpf3mp_itself_is_declared_last_with_its_fingerprint_whatever_the_save_lists() {
        let mut owner = Mods::new(
            Text::lossy("40408"),
            vec![
                installed("tpf3mp_1", Class::Shared, "1+0123456789abcdef"),
                installed("vehicles_pack", Class::Shared, "3"),
            ],
            [],
            false,
        );
        // A save that lists TPF3-MP first, and one that does not list it.
        owner
            .own_start(&save(&["tpf3mp_1", "vehicles_pack"]))
            .unwrap();
        assert_eq!(
            names(&owner.manifest()),
            ["vehicles_pack 3", "tpf3mp_1 1+0123456789abcdef"]
        );
        owner.own_start(&save(&["vehicles_pack"])).unwrap();
        assert_eq!(
            names(&owner.manifest()),
            ["vehicles_pack 3", "tpf3mp_1 1+0123456789abcdef"]
        );
        // A guest with an old copy of the same revision: told plainly, and
        // learning the room's mods does not make it match.
        let mut guest = Mods::new(
            Text::lossy("40408"),
            vec![
                installed("tpf3mp_1", Class::Shared, "1+fedcba9876543210"),
                installed("vehicles_pack", Class::Shared, "3"),
            ],
            [],
            false,
        );
        let room = owner.manifest();
        let told = room.compare(&guest.manifest()).unwrap();
        assert!(guest.learn(&told));
        assert_eq!(
            names(&guest.manifest()),
            ["vehicles_pack 3", "tpf3mp_1 1+fedcba9876543210"]
        );
        let told = room.compare(&guest.manifest()).unwrap();
        assert_eq!(
            told.to_string(),
            "Your TPF3-MP mod differs from the host's (yours fedcba98, host 01234567): \
             reinstall the same version"
        );
        assert!(
            guest
                .required()
                .iter()
                .any(|r| r.id == "tpf3mp_1" && r.have == Have::OtherVersion)
        );
        // With the same files, the same manifest.
        let mut same = Mods::new(
            Text::lossy("40408"),
            vec![
                installed("tpf3mp_1", Class::Shared, "1+0123456789abcdef"),
                installed("vehicles_pack", Class::Shared, "3"),
            ],
            [],
            false,
        );
        let told = room.compare(&same.manifest()).unwrap();
        same.learn(&told);
        assert_eq!(room.compare(&same.manifest()), None);
    }

    /// The owner's declaration from a start save that lists TPF3-MP (a
    /// save names no version) carries the owner's own fingerprint, and a
    /// guest with the same files matches it.
    #[test]
    fn an_owner_with_a_start_save_and_a_guest_with_the_same_files_match() {
        let catalog = || {
            vec![
                installed("tpf3mp_1", Class::Shared, "1+74554cbaf1a7d3dd"),
                installed("ug_legacy_road_1850", Class::Shared, "1"),
            ]
        };
        let mut owner = Mods::new(Text::lossy("40408"), catalog(), [], false);
        owner
            .own_start(&save(&["ug_legacy_road_1850", "tpf3mp_1"]))
            .unwrap();
        let room = owner.manifest();
        assert_eq!(
            names(&room),
            ["ug_legacy_road_1850 1", "tpf3mp_1 1+74554cbaf1a7d3dd"]
        );
        let mut guest = Mods::new(Text::lossy("40408"), catalog(), [], false);
        guest.forget_room();
        if let Some(told) = room.compare(&guest.manifest()) {
            assert!(
                told.changed.is_empty(),
                "the same TPF3-MP is never said to differ: {told}"
            );
            guest.learn(&told);
        }
        assert_eq!(room.compare(&guest.manifest()), None);
    }

    #[test]
    fn scanning_fingerprints_tpf3mp_itself() {
        let dir = tempfile::tempdir().unwrap();
        let mod_dir = dir.path().join("tpf3mp_1");
        fs::create_dir_all(mod_dir.join("content/tpf3mp")).unwrap();
        fs::write(
            mod_dir.join("mod.json"),
            r#"{"modId": "tpf3mp_1", "revision": 1}"#,
        )
        .unwrap();
        fs::write(mod_dir.join("content/tpf3mp/act.lua"), "return {}").unwrap();
        let found = roots::installed(&[dir.path().to_owned()]);
        let scanned = scanned(&found);
        let own = scanned.iter().find(|m| m.id == "tpf3mp_1").unwrap();
        assert_eq!(
            own.version,
            crate::own_mod::version("1", &mod_dir),
            "the revision and the fingerprint of the files"
        );
        assert!(own.version.starts_with("1+"));
    }

    #[test]
    fn a_long_reason_is_cut_at_a_character() {
        let long = "é".repeat(100);
        let cut = shorten(&long);
        assert!(cut.len() <= MAX_REASON && cut.ends_with("..."));
    }
}
