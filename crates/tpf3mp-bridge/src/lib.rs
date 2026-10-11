//! What the agent and the in-game hook say to each other over the
//! shared-memory link (`tpf3mp-ipc`), and the step gate that keeps the
//! game's simulation in step with the room.
//!
//! The agent runs the network side: it follows the room's turn stream and
//! decides when each step may run (`tpf3mp_agent::Playout`). The hook runs
//! inside the game and does as little as it can. It applies events and runs
//! steps when the [`Gate`] allows, and reports what the player does and how
//! far the game has got.
//!
//! # Ordering
//!
//! Each direction is an ordered stream of messages. The agent sends every
//! event for step `s` after releasing step `s - 1` and before releasing step
//! `s`, and never releases several steps in one message across an event.
//! The hook reads messages only while its game waits before a step, and
//! stops reading once that step is released. Each event is therefore
//! applied exactly between the two steps the room ordered it for, and the
//! [`Gate`] refuses anything that breaks this.
//!
//! This crate has no async runtime and no network code, because the hook
//! links it into the game. [`Session`] is the hook's whole side of the
//! link; the game-specific part of the hook only implements [`Game`].

mod gate;
pub mod mods;
mod session;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use tpf3mp_proto::{
    BoundedVec, ChatText, Event, IntentRejection, LaneDigest, MAX_ROOM_MEMBERS, Payload, PlayerId,
    RulesName, Secret, Speed, Text,
};

pub use gate::{Gate, GateError, Gated};
pub use mods::{ModLists, ModName, Plan};
pub use session::{Begin, Game, Load, Notice, SaveOrder, Session, SessionError, StepGate};

/// Version of these messages. Both sides send it first and refuse a peer
/// that speaks another. 7 added [`ToAgent::WorldUp`]; 8 added
/// [`ToAgent::MenuUp`]; 9 added the main menu's Multiplayer window's
/// [`ToHook::Lobby`] and [`ToAgent::Lobby`]; 10 added the rules, saves,
/// world and differences to [`LobbyView`], and the rules and start save to
/// [`LobbyAction::Create`]; 11 carries a password beside a command
/// ([`ToAgent::Command`]'s `secret`) and a seal in each ordered command
/// (protocol 8); 12 added the mods the room's world loads with to
/// [`ToHook::Begin`]. (The lobby's, the passwords' and the mods' changes
/// were each 10 on their own branches.) 13 added the player's mods and the
/// room's shared mods to [`LobbyView`], and [`LobbyAction::ChooseMod`]; 14
/// the server's public rooms to [`LobbyView`] ([`LobbyView::rooms`]),
/// [`LobbyAction::ListRooms`] and a room's listing to
/// [`LobbyAction::Create`]; 15 the server setting: the server's address
/// and the launcher's default to [`LobbyView`], and
/// [`LobbyAction::SetServer`]; 16 the players' banners: each
/// [`LobbyMember::banner`], the player's own ([`LobbyView::banner`]) and
/// [`LobbyAction::SetBanner`]; 17 a room's play style, co-op or
/// competitive ([`LobbyRoom::competitive`], in [`LobbyAction::Create`] and
/// the room list); 18 each member's loading progress
/// ([`LobbyMember::loading`]), and in the game's Multiplayer window each
/// member's banner and loading progress ([`RoomMember`]), and its Leave
/// as [`LobbyAction::Leave`]; 19 the campaign portraits this player's game
/// can show ([`LobbyView::portraits`]), and banner ids of up to 32 bytes
/// that may name one (protocol 13's `tpf3mp_proto::PORTRAITS`); 20 the save
/// a room starts from on its page ([`LobbyRoom::start`]), the owner's
/// upload of it ([`LobbyRoom::upload`]) and the owner's choice of another
/// in the lobby ([`LobbyAction::ChooseStart`]; protocol 14); 22 lists up to
/// 100 saves ([`MAX_LOBBY_SAVES`], 40 before), the player's own only; 23
/// the launcher's run, for the window to show ([`LobbyView::log_session`];
/// protocol 16); 24 carries what the players' build tools show: the
/// player's own ([`ToAgent::Preview`]) and the other members'
/// ([`ToHook::Preview`]; protocol 17); 25 the room's mods as its owner
/// declared them, with their names, sources, Mod Hub numbers and this
/// player's versions ([`LobbyRoomMod`], up to [`MAX_LOBBY_ROOM_MODS`]),
/// how each member's game differs ([`LobbyMember::differs`]), the owner's
/// choice of the room's mods and their settings
/// ([`LobbyAction::ChooseRoomMods`]), finding the installed mods again
/// ([`LobbyAction::RescanMods`]), and the room's settings of its mods in
/// [`ModLists`] (protocol 18); 26 the release's servers in the room list
/// ([`LobbyRoomList::servers`]) and each public room's server and ping
/// ([`LobbyPublicRoom::server`], [`LobbyPublicRoom::ping_ms`]); 27 the
/// listed server in [`LobbyAction::Join`]; 28 the release's trusted servers
/// in [`LobbyView::servers`] before a room list
/// is fetched, so the game's invite form can ask for an explicit region.
pub const BRIDGE_VERSION: u32 = 29;
/// Most servers the room list names ([`LobbyRoomList::servers`]).
pub const MAX_LOBBY_SERVERS: usize = 8;
/// The link name the agent creates and the hook opens, unless told
/// otherwise.
pub const DEFAULT_LINK: &str = "tpf3mp.default";
/// Largest encoded message, within the link's default frame limit
/// (`tpf3mp_ipc::DEFAULT_MAX_MESSAGE`). An event with the largest intent
/// payload fits.
pub const MAX_MESSAGE: usize = 60 * 1024;
/// Longest file path the link carries, in UTF-8 bytes.
pub const MAX_PATH: usize = 1024;

/// From the agent to the hook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToHook {
    /// The first message after the link opens.
    Hello { version: u32 },
    /// A game begins. A [`ToHook::Load`] follows. The hook writes the saves
    /// the room asks for into `saves`, a directory the agent made for this
    /// game. `rules` names the rules the room is played by: with `native`,
    /// the game's own economy runs as in single player. `player` is the
    /// local player, as the room's events name the actor: the hook knows its
    /// own commands by it when the room orders them.
    Begin {
        room: tpf3mp_proto::RoomId,
        rules: RulesName,
        steps_per_second: u16,
        checkpoint_interval: u32,
        saves: Text<MAX_PATH>,
        player: PlayerId,
        /// The mods the room's worlds load with in this game: the room's
        /// shared ones and this player's personal ones ([`mods::plan`]).
        /// `None` when the agent does not know this player's mods: a world
        /// then loads with the mods its save lists.
        mods: Option<ModLists>,
    },
    /// Apply this event before running step `event.step`.
    Apply(Event),
    /// Steps up to and including `through` may run.
    Release { through: u64 },
    /// The room's speed, for the game's display. Pacing comes from
    /// [`ToHook::Release`] alone.
    Speed(Speed),
    /// At the checkpoint at `step`, this replica's `lanes` differed from the
    /// room's verdict.
    Diverged { step: u64, lanes: Vec<u16> },
    /// The room refused one of the player's commands, which then never
    /// happens. `command` counts the player's [`ToAgent::Command`]s from 0.
    Refused {
        command: u64,
        reason: IntentRejection,
    },
    /// The game session is over; the game stops waiting at the gate.
    End { reason: Text<128> },
    /// Load a world, then answer [`ToAgent::Loaded`] with `next_step`, the
    /// first step that world runs. Everything sent before this is void.
    ///
    /// The first load of a game may name no file: the game then loads the
    /// world every player starts from. Otherwise `file` is a save the room
    /// agreed on, for a player joining a running game, one who could no
    /// longer resume, or one whose world diverged.
    Load {
        file: Option<Text<MAX_PATH>>,
        next_step: u64,
        world_epoch: u64,
    },
    /// A member of the room said something; `from` is their name.
    Chat { from: Text<32>, text: ChatText },
    /// The room as it stands, for the game's Multiplayer window: sent when
    /// the game begins and whenever the room changes.
    Room(RoomInfo),
    /// The launcher's lobby as it stands, for the main menu's Multiplayer
    /// window (D17): sent whenever it changes, before, during and after a
    /// room's game. Only the latest counts. Boxed: it is far larger than
    /// the other messages.
    Lobby(Box<LobbyView>),
    /// What another member's build tool shows now: an action as an intent
    /// carries one, or `None` once it shows nothing (protocol 17's
    /// `ServerMessage::Preview`). Advisory: never applied to the world, and
    /// only the latest of each member counts.
    Preview {
        from: PlayerId,
        preview: Option<Payload>,
    },
}

/// Most chat lines a [`LobbyView`] carries: the newest.
pub const MAX_LOBBY_CHAT: usize = 40;
/// Most rules a [`LobbyView`] offers.
pub const MAX_LOBBY_RULES: usize = 8;
/// Most saves a [`LobbyView`] lists: the newest. The game's autosaves and
/// the mod's own room copies are left out before the cut
/// (`tpf3mp_agent::steam::list_saves`).
pub const MAX_LOBBY_SAVES: usize = 100;
/// Longest save name a [`LobbyView`] lists or a [`LobbyAction::Create`]
/// names, in UTF-8 bytes.
pub const MAX_SAVE_NAME: usize = 64;
/// A save in the game's save folder, by its name without `.sav`.
pub type SaveName = Text<MAX_SAVE_NAME>;

/// What the main menu's Multiplayer window shows: the launcher's connection,
/// room and chat, as the launcher window shows them (D17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyView {
    pub connection: LobbyConnection,
    /// The server the launcher plays on, as players see it.
    pub server: Text<128>,
    /// That server's address, `host:port`, as the server setting shows it;
    /// empty without one.
    pub server_address: Text<128>,
    /// The launcher's default server, `host:port`, which the setting's
    /// "Reset to default" goes back to; empty without one.
    pub server_default: Text<128>,
    /// The release's trusted regions, available before rooms are listed so
    /// an invite can name one explicitly when several regions match.
    pub servers: BoundedVec<LobbyServer, MAX_LOBBY_SERVERS>,
    /// The banner this player picked, if any: one of
    /// `tpf3mp_proto::BANNERS` or of [`LobbyView::portraits`].
    pub banner: Option<tpf3mp_proto::BannerId>,
    /// The campaign portraits this player's game can show
    /// (`tpf3mp_proto::PORTRAITS`), in that order: those the launcher took
    /// from this player's install. Empty without the campaign.
    pub portraits: BoundedVec<tpf3mp_proto::BannerId, MAX_LOBBY_PORTRAITS>,
    /// The player's name.
    pub name: Text<32>,
    /// What went wrong last, until something succeeds.
    pub error: Option<Text<256>>,
    /// The newest thing the player should know.
    pub notice: Option<Text<256>>,
    pub room: Option<LobbyRoom>,
    /// The room's chat, oldest first.
    pub chat: BoundedVec<LobbyLine, MAX_LOBBY_CHAT>,
    /// The rules the server offers new rooms, its default first.
    pub rules: BoundedVec<LobbyRules, MAX_LOBBY_RULES>,
    /// The player's saves, newest first: what a room they create can start
    /// from.
    pub saves: BoundedVec<SaveName, MAX_LOBBY_SAVES>,
    /// The save rooms this player creates start from unless they pick
    /// another (the launcher's `--start-save`).
    pub start_save: Option<SaveName>,
    /// The room's world in this player's game.
    pub world: LobbyWorld,
    /// How this player's game differs from the room's, while it does.
    pub differences: Option<Text<256>>,
    /// The page of the server's public rooms last asked for
    /// ([`LobbyAction::ListRooms`]), while connected.
    pub rooms: Option<LobbyRoomList>,
    /// The mods this player has installed, those they may choose first
    /// (docs/MODS.md, "Choosing mods"), as many as fit.
    pub mods: BoundedVec<LobbyMod, MAX_LOBBY_MODS>,
    /// The room's mods, as its owner declared them, and whether this player
    /// has each; empty while they are not known. As many as fit the
    /// message: [`LobbyView::room_mods_more`] counts the rest.
    pub room_mods: BoundedVec<LobbyRoomMod, MAX_LOBBY_ROOM_MODS>,
    /// The room's mods beyond those listed.
    pub room_mods_more: u32,
    /// Of all the room's mods, how many this player lacks, and how many it
    /// has in another version: what the window's mods line says, listed or
    /// not.
    pub room_mods_missing: u16,
    pub room_mods_other: u16,
    /// The settings of the room's mods, as its owner picked them: what the
    /// owner's mod selector starts from again.
    pub room_params: BoundedVec<LobbySetting, { tpf3mp_proto::MAX_ROOM_PARAMS }>,
    /// The launcher's run, the code every line of its diagnostics carries,
    /// for the window to show with a Copy (proposed D10 amendment); empty
    /// while diagnostics are off.
    pub log_session: Text<8>,
}

/// Most portraits a [`LobbyView`] offers: room for all of
/// `tpf3mp_proto::PORTRAITS`.
pub const MAX_LOBBY_PORTRAITS: usize = 32;

/// Most installed mods a [`LobbyView`] lists.
pub const MAX_LOBBY_MODS: usize = 64;
/// Most of the room's mods a [`LobbyView`] lists: as many as a room runs.
/// The launcher lists fewer when the whole view would not fit a message.
pub const MAX_LOBBY_ROOM_MODS: usize = tpf3mp_proto::MAX_ROOM_MODS;

/// One mod this player has installed, as the window lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyMod {
    /// Its id, as [`LobbyAction::ChooseMod`] names it.
    pub id: ModName,
    /// Its name for players.
    pub name: Text<48>,
    pub class: LobbyModClass,
    /// Why it is of its class, in a line ("every player needs it: ...").
    pub reason: Text<96>,
    /// Whether the player plays with it.
    pub chosen: bool,
    /// Whether the player may choose it: a personal mod, or a carried one
    /// with `--personal-game-scripts`. A shared mod never: every player
    /// needs the room's.
    pub choosable: bool,
}

/// What the scan made of a mod (`tpf3mp_modscan::Class`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LobbyModClass {
    /// Only what this player sees.
    Personal,
    /// Decides in the simulation through what the room carries.
    Carried,
    /// Every player needs it.
    Shared,
}

/// One of the room's mods, and whether this player has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyRoomMod {
    pub id: ModName,
    /// Its name for players, as the room's owner's game has it.
    pub name: Text<48>,
    /// The room's version of it (empty when unknown).
    pub version: Text<32>,
    /// This player's version, when it has one.
    pub yours: Option<Text<32>>,
    pub have: LobbyHave,
    /// Where the owner's game has it from (`mod.io`, `StagingArea`,
    /// `UserMods`, `DLC`, `BuiltInMods`); empty unknown.
    pub source: Text<16>,
    /// Its Mod Hub number: the owner's claim of where to get it, which the
    /// window has this player's game resolve before offering to install it.
    pub modio: Option<u64>,
}

/// Whether this player has one of the room's shared mods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LobbyHave {
    Yes,
    No,
    /// Installed, in another version.
    OtherVersion,
}

/// A page of the server's public rooms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyRoomList {
    pub page: u16,
    pub rooms: BoundedVec<LobbyPublicRoom, { tpf3mp_proto::ROOMS_PER_PAGE }>,
    /// A later page has more.
    pub more: bool,
    /// The servers the rooms come from, when the launcher plays on its
    /// release's several servers; empty with one.
    pub servers: BoundedVec<LobbyServer, MAX_LOBBY_SERVERS>,
}

/// One of the release's servers, as the room list names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyServer {
    /// Its name, such as `EU`.
    pub name: Text<24>,
    /// Its round trip, in milliseconds; 0 unknown.
    pub ping_ms: u16,
    /// The launcher plays on it: rooms this player creates go there.
    pub here: bool,
    /// It answers.
    pub reachable: bool,
}

/// One public room, as the room browser shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyPublicRoom {
    pub invite: Text<128>,
    pub name: Text<48>,
    pub rules: RulesName,
    pub players: u8,
    pub max_players: u8,
    pub has_password: bool,
    pub running: bool,
    /// The climate, such as `temperate`; empty unknown.
    pub map: Text<32>,
    /// The game's year; 0 unknown.
    pub year: u16,
    pub companies: u8,
    pub competitive: bool,
    /// The name of the room's server, such as `EU`, when the list has
    /// several servers'; empty with one.
    pub server: Text<24>,
    /// That server's round trip, in milliseconds; 0 unknown.
    pub ping_ms: u16,
}

/// What a public room's list entry says of its world.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyListing {
    /// The climate of the start save, such as `temperate`.
    pub map: Text<32>,
    /// The start save's year; 0 unknown.
    pub year: u16,
}

/// Rules a room can be played by, as the server offers them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyRules {
    pub name: RulesName,
    pub description: Text<200>,
}

/// Where the room's world is in this player's game.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LobbyWorld {
    /// No world of the room's yet.
    #[default]
    None,
    /// Coming from the room: `bytes` of `total` so far.
    Fetching { bytes: u64, total: u64 },
    /// The game loads it.
    Loading,
    /// The game plays it.
    Playing,
}

impl Default for LobbyView {
    /// Not connected, no room, nothing said.
    fn default() -> Self {
        Self {
            connection: LobbyConnection::Disconnected,
            server: Text::lossy(""),
            server_address: Text::lossy(""),
            server_default: Text::lossy(""),
            servers: BoundedVec::empty(),
            banner: None,
            portraits: BoundedVec::empty(),
            name: Text::lossy(""),
            error: None,
            notice: None,
            room: None,
            chat: BoundedVec::empty(),
            rules: BoundedVec::empty(),
            saves: BoundedVec::empty(),
            start_save: None,
            world: LobbyWorld::None,
            differences: None,
            mods: BoundedVec::empty(),
            room_mods: BoundedVec::empty(),
            room_mods_more: 0,
            room_mods_missing: 0,
            room_mods_other: 0,
            room_params: BoundedVec::empty(),
            rooms: None,
            log_session: Text::lossy(""),
        }
    }
}

/// Whether the launcher is connected to its server.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LobbyConnection {
    #[default]
    Disconnected,
    Connecting,
    Connected,
}

/// The room the player is in, as its lobby shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyRoom {
    pub name: Text<48>,
    pub rules: RulesName,
    /// What to send friends.
    pub invite: Option<Text<128>>,
    /// The room's game has begun.
    pub running: bool,
    pub you_own: bool,
    pub max_players: u8,
    pub has_password: bool,
    pub members: BoundedVec<LobbyMember, { MAX_ROOM_MEMBERS as usize }>,
    /// Co-op (`false`) or competitive (`true`).
    pub competitive: bool,
    /// In the lobby: the save the room's game starts from, as the room
    /// names it to everyone; `None` when the owner's game provides the
    /// world.
    pub start: Option<LobbyStart>,
    /// For the owner: the save they picked on its way to the room. Start
    /// waits for it.
    pub upload: Option<LobbyUpload>,
}

/// The save a room starts from, as its page shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyStart {
    pub name: SaveName,
    /// Its climate, such as `temperate`; empty unknown.
    pub map: Text<32>,
    /// Its year; 0 unknown.
    pub year: u16,
    /// Whether the room has it: until then the game cannot start.
    pub arrived: bool,
}

/// The owner's save on its way to the room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyUpload {
    pub save: SaveName,
    /// How much of it went up, 0 to 100; 0 while it is being read.
    pub percent: u8,
}

/// One member of the room, as its lobby shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyMember {
    pub player: PlayerId,
    pub name: Text<32>,
    pub ready: bool,
    pub connected: bool,
    pub owner: bool,
    pub you: bool,
    /// Whether this member's game matches the owner's: `None` while either
    /// has not said.
    pub same_content: Option<bool>,
    /// How this member's game differs from the room's, while it does.
    pub differs: Option<tpf3mp_proto::ContentStatus>,
    /// The banner this member picked (`tpf3mp_proto::BANNERS`), or the
    /// portrait (`tpf3mp_proto::PORTRAITS`) this player's game can show, if
    /// any: a portrait it cannot show is left out, for the default banner.
    pub banner: Option<tpf3mp_proto::BannerId>,
    /// Where this member's game is with the room's world while it comes in.
    pub loading: Option<tpf3mp_proto::LoadingStage>,
}

/// One line of the room's chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyLine {
    pub from: Text<32>,
    pub text: ChatText,
    pub you: bool,
}

/// What the player asks for in the main menu's Multiplayer window: the
/// launcher's own actions (D17). The launcher carries them out as if its
/// window had asked, under the same one-server or compiled-region policy
/// (D12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LobbyAction {
    Connect {
        name: Text<32>,
    },
    Disconnect,
    Create {
        room: Text<48>,
        max_players: u8,
        password: Option<Text<64>>,
        /// One of the server's rules; its default without.
        rules: Option<RulesName>,
        /// The save the room starts from, which every game loads from its
        /// menu; without, the launcher's own (`--start-save`), if any.
        start_save: Option<SaveName>,
        /// `Some` lists the room in the server's room list; `None` keeps it
        /// private.
        listing: Option<LobbyListing>,
        /// Competitive rather than co-op.
        competitive: bool,
    },
    /// Asks for page `page` of the server's public rooms.
    ListRooms {
        page: u16,
    },
    Join {
        invite: Text<128>,
        /// The trusted listed region shown on a public room card. Absent
        /// for a typed invite, which the launcher resolves across every
        /// listed region before joining.
        #[serde(default)]
        server: Option<Text<24>>,
        password: Option<Text<64>>,
    },
    Ready {
        ready: bool,
    },
    Start,
    Kick {
        player: PlayerId,
    },
    Chat {
        text: ChatText,
    },
    Leave,
    /// Play with the installed mod `id`, or not: a personal one, or a
    /// carried one with `--personal-game-scripts` (docs/MODS.md). The
    /// launcher remembers it for next time.
    ChooseMod {
        id: ModName,
        chosen: bool,
    },
    /// The player's server setting: play on `server`, a `host:port`, from
    /// now on; empty goes back to the launcher's default. The launcher
    /// checks it, remembers it, and reconnects there if connected. Refused
    /// in a room. Regional invite routing is limited to the release's
    /// compiled server list (D12).
    SetServer {
        server: Text<128>,
    },
    /// Show this banner or portrait in rooms; `None` for the default.
    SetBanner {
        banner: Option<tpf3mp_proto::BannerId>,
    },
    /// The room's owner, in its lobby: the room starts from the save `save`
    /// now, one of [`LobbyView::saves`], which the launcher hands over in
    /// place of the one before; empty for none, the owner's game then
    /// providing the world. `map` and `year` are what the owner's game read
    /// of the save, for the room to show (empty and 0 unknown). Every player
    /// is asked to get ready again.
    ChooseStart {
        save: SaveName,
        map: Text<32>,
        year: u16,
    },
    /// The room's owner, in its lobby: the room's mods are these, as the
    /// game's Load Game page has them, in its activation order, with the
    /// settings it holds, the game's own among them (docs/MODS.md, "The
    /// room's mods"); and, with `save`, the room starts from that save, as
    /// [`LobbyAction::ChooseStart`] says, both at once. Every player is asked
    /// to get ready again.
    ChooseRoomMods {
        save: Option<SaveName>,
        map: Text<32>,
        year: u16,
        mods: BoundedVec<LobbySelected, MAX_LOBBY_ROOM_MODS>,
        params: BoundedVec<LobbySetting, { tpf3mp_proto::MAX_ROOM_PARAMS }>,
    },
    /// Find the installed mods again, as after installing one from Mod Hub.
    RescanMods,
}

/// One mod the room's owner picked in the game's mod selector, with what
/// the game says of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbySelected {
    pub id: ModName,
    pub name: Text<48>,
    pub source: Text<16>,
    pub modio: Option<u64>,
}

/// One setting of a mod, as the game's mod selector holds it; with the id
/// [`tpf3mp_proto::GAME_SETTINGS`] one of the game's own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbySetting {
    pub id: ModName,
    pub key: Text<64>,
    pub value: i64,
}

/// The room as the game's Multiplayer window shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomInfo {
    pub name: Text<48>,
    pub owner: PlayerId,
    pub members: BoundedVec<RoomMember, { MAX_ROOM_MEMBERS as usize }>,
}

/// One member of the room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomMember {
    pub player: PlayerId,
    pub name: Text<32>,
    pub connected: bool,
    /// The banner this member picked (`tpf3mp_proto::BANNERS`), or the
    /// portrait (`tpf3mp_proto::PORTRAITS`) this player's game can show, if
    /// any: a portrait it cannot show is left out, for the default banner.
    pub banner: Option<tpf3mp_proto::BannerId>,
    /// Where this member's game is with the room's world while it comes in.
    pub loading: Option<tpf3mp_proto::LoadingStage>,
}

/// From the hook to the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToAgent {
    /// The first message after the link opens.
    Hello { version: u32, build: Text<64> },
    /// The world is loaded, and `next_step` is the first step it will run.
    Loaded { next_step: u64, world_epoch: u64 },
    /// The local player did something: have the room order it, with the
    /// password it needs, if any (a company's), which the room seals.
    Command {
        payload: Payload,
        secret: Option<Secret>,
    },
    /// The game ran this step.
    Ran { step: u64, world_epoch: u64 },
    /// The world's digests at a checkpoint step, taken after running it.
    Checkpoint {
        step: u64,
        lanes: Vec<LaneDigest>,
        world_epoch: u64,
    },
    /// A line for the agent's log.
    Log { message: Text<256> },
    /// The world was saved at the save event `event`, into `file`, or
    /// `None` if saving failed. `lanes` are its digests there.
    Saved {
        event: u64,
        lanes: Vec<LaneDigest>,
        file: Option<Text<MAX_PATH>>,
        world_epoch: u64,
    },
    /// The player says something to the room.
    Chat { text: ChatText },
    /// The player picked this speed in the game's speed row: ask the room
    /// for it. Only the room's owner may change the room's speed; the server
    /// refuses anyone else, and the agent shows the refusal.
    Speed { speed: Speed },
    /// Before the room begins a game: a world is up in the game, with the
    /// mod linked to the hook, and the game steps it. `world` counts the
    /// worlds whose GUI started since the hook began, from 1, so a world is
    /// told once and a new one has a higher number. The agent marks the
    /// player ready in the room's lobby, once per world.
    WorldUp { world: u64 },
    /// Before the room begins a game: the game is at its main menu, with no
    /// world up, and can load the room's world from there when the room
    /// sends one ([`ToHook::Load`] with a file). `menu` counts the times the
    /// game came to its menu since the hook began, from 1. The agent marks
    /// a player other than the room's owner ready, once per `menu`, if it
    /// keeps worlds; the owner's game needs a world up, to save it for the
    /// room.
    MenuUp { menu: u64 },
    /// The player asked for this in the main menu's Multiplayer window.
    Lobby(LobbyAction),
    /// What the player's build tool shows now, for the other members: an
    /// action as [`ToAgent::Command`] carries one, at most
    /// `tpf3mp_proto::MAX_PREVIEW` bytes, or `None` once it shows nothing.
    Preview { preview: Option<Payload> },
}

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("the message is {0} bytes, over the {MAX_MESSAGE}-byte limit")]
    TooLarge(usize),
    #[error("the message does not decode: {0}")]
    Malformed(#[from] postcard::Error),
    #[error("the peer speaks bridge version {0}, not {BRIDGE_VERSION}")]
    Version(u32),
}

/// Encodes a message for the link.
pub fn encode<T: Serialize>(message: &T) -> Result<Vec<u8>, BridgeError> {
    let bytes = postcard::to_stdvec(message)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(BridgeError::TooLarge(bytes.len()));
    }
    Ok(bytes)
}

/// Decodes a message from the link. Trailing bytes are refused.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, BridgeError> {
    if bytes.len() > MAX_MESSAGE {
        return Err(BridgeError::TooLarge(bytes.len()));
    }
    let (message, rest) = postcard::take_from_bytes(bytes)?;
    if !rest.is_empty() {
        return Err(BridgeError::Malformed(
            postcard::Error::DeserializeBadEncoding,
        ));
    }
    Ok(message)
}

/// Checks a peer's hello.
pub fn check_version(version: u32) -> Result<(), BridgeError> {
    if version == BRIDGE_VERSION {
        Ok(())
    } else {
        Err(BridgeError::Version(version))
    }
}

#[cfg(test)]
mod tests {
    use tpf3mp_proto::{EventBody, FixedBytes, MAX_PAYLOAD, PlayerId};

    use super::*;

    #[test]
    fn messages_round_trip() {
        let to_hook = ToHook::Apply(Event {
            seq: 7,
            step: 3,
            body: EventBody::Command {
                player: PlayerId(FixedBytes([1; 32])),
                client_seq: 9,
                payload: Payload::new(vec![4, 5, 6]).unwrap(),
                seal: None,
            },
        });
        assert_eq!(
            decode::<ToHook>(&encode(&to_hook).unwrap()).unwrap(),
            to_hook
        );
        let to_agent = ToAgent::Checkpoint {
            world_epoch: 1,
            step: 50,
            lanes: vec![LaneDigest {
                lane: 2,
                digest: FixedBytes([8; 32]),
            }],
        };
        assert_eq!(
            decode::<ToAgent>(&encode(&to_agent).unwrap()).unwrap(),
            to_agent
        );
        for to_agent in [ToAgent::WorldUp { world: 2 }, ToAgent::MenuUp { menu: 3 }] {
            assert_eq!(
                decode::<ToAgent>(&encode(&to_agent).unwrap()).unwrap(),
                to_agent
            );
        }
    }

    #[test]
    fn the_largest_intent_fits_in_one_message() {
        let apply = ToHook::Apply(Event {
            seq: u64::MAX,
            step: u64::MAX,
            body: EventBody::Command {
                player: PlayerId(FixedBytes([0xff; 32])),
                client_seq: u64::MAX,
                payload: Payload::new(vec![0xab; MAX_PAYLOAD]).unwrap(),
                seal: Some(tpf3mp_proto::Seal {
                    scope: u64::MAX,
                    tag: FixedBytes([0xff; 32]),
                }),
            },
        });
        assert!(encode(&apply).is_ok());
    }

    #[test]
    fn malformed_and_oversized_messages_are_refused() {
        let mut bytes = encode(&ToAgent::Ran {
            world_epoch: 1,
            step: 5,
        })
        .unwrap();
        bytes.push(0);
        assert!(matches!(
            decode::<ToAgent>(&bytes),
            Err(BridgeError::Malformed(_))
        ));
        assert!(matches!(
            decode::<ToAgent>(&[0xff; 3]),
            Err(BridgeError::Malformed(_))
        ));
        assert!(matches!(
            decode::<ToAgent>(&vec![0; MAX_MESSAGE + 1]),
            Err(BridgeError::TooLarge(_))
        ));
    }

    #[test]
    fn the_fullest_lobby_fits_in_one_message() {
        let member = |n: u8| LobbyMember {
            player: tpf3mp_proto::PlayerId(FixedBytes([n; 32])),
            name: Text::new("x".repeat(32)).unwrap(),
            ready: true,
            connected: true,
            owner: n == 0,
            you: n == 1,
            same_content: Some(true),
            differs: None,
            banner: Some(Text::new("x".repeat(32)).unwrap()),
            loading: Some(tpf3mp_proto::LoadingStage::Fetching { percent: 100 }),
        };
        let line = LobbyLine {
            from: Text::new("y".repeat(32)).unwrap(),
            text: Text::new("z".repeat(280)).unwrap(),
            you: false,
        };
        let view = ToHook::Lobby(Box::new(LobbyView {
            connection: LobbyConnection::Connected,
            server: Text::new("s".repeat(128)).unwrap(),
            server_address: Text::new("a".repeat(128)).unwrap(),
            server_default: Text::new("d".repeat(128)).unwrap(),
            servers: BoundedVec::empty(),
            banner: Some(Text::new("b".repeat(32)).unwrap()),
            portraits: BoundedVec::new(vec![
                Text::new("p".repeat(32)).unwrap();
                MAX_LOBBY_PORTRAITS
            ])
            .unwrap(),
            name: Text::new("n".repeat(32)).unwrap(),
            error: Some(Text::new("e".repeat(256)).unwrap()),
            notice: Some(Text::new("o".repeat(256)).unwrap()),
            room: Some(LobbyRoom {
                name: Text::new("r".repeat(48)).unwrap(),
                rules: Text::new("native").unwrap(),
                invite: Some(Text::new("i".repeat(128)).unwrap()),
                running: false,
                you_own: true,
                max_players: 64,
                has_password: true,
                members: BoundedVec::new((0..MAX_ROOM_MEMBERS).map(member).collect()).unwrap(),
                competitive: false,
                start: Some(LobbyStart {
                    name: Text::new("s".repeat(MAX_SAVE_NAME)).unwrap(),
                    map: Text::new("m".repeat(32)).unwrap(),
                    year: u16::MAX,
                    arrived: false,
                }),
                upload: Some(LobbyUpload {
                    save: Text::new("u".repeat(MAX_SAVE_NAME)).unwrap(),
                    percent: 100,
                }),
            }),
            chat: BoundedVec::new(vec![line; MAX_LOBBY_CHAT]).unwrap(),
            rules: BoundedVec::new(vec![
                LobbyRules {
                    name: Text::new("r".repeat(32)).unwrap(),
                    description: Text::new("d".repeat(200)).unwrap(),
                };
                MAX_LOBBY_RULES
            ])
            .unwrap(),
            saves: BoundedVec::new(vec![
                Text::new("s".repeat(MAX_SAVE_NAME)).unwrap();
                MAX_LOBBY_SAVES
            ])
            .unwrap(),
            start_save: Some(Text::new("s".repeat(MAX_SAVE_NAME)).unwrap()),
            world: LobbyWorld::Fetching {
                bytes: u64::MAX,
                total: u64::MAX,
            },
            differences: Some(Text::new("d".repeat(256)).unwrap()),
            mods: BoundedVec::new(vec![
                LobbyMod {
                    id: Text::new("m".repeat(96)).unwrap(),
                    name: Text::new("n".repeat(48)).unwrap(),
                    class: LobbyModClass::Carried,
                    reason: Text::new("r".repeat(96)).unwrap(),
                    chosen: true,
                    choosable: true,
                };
                MAX_LOBBY_MODS
            ])
            .unwrap(),
            room_mods: BoundedVec::new(vec![
                LobbyRoomMod {
                    id: Text::new("m".repeat(96)).unwrap(),
                    version: Text::new("v".repeat(32)).unwrap(),
                    have: LobbyHave::OtherVersion,
                    name: Text::new("n".repeat(48)).unwrap(),
                    yours: Some(Text::new("y".repeat(32)).unwrap()),
                    source: Text::new("s".repeat(16)).unwrap(),
                    modio: Some(u64::MAX),
                };
                // The launcher lists as many of the room's mods as fit the
                // rest of the view, and counts the rest
                // (`tpf3mp_agent::launcher::lobby::view`): with everything
                // else at its longest, still this many.
                24
            ])
            .unwrap(),
            room_mods_more: u32::MAX,
            room_mods_missing: 0,
            room_mods_other: 0,
            room_params: BoundedVec::empty(),
            log_session: Text::new("l".repeat(8)).unwrap(),
            rooms: Some(LobbyRoomList {
                page: u16::MAX,
                rooms: BoundedVec::new(vec![
                    LobbyPublicRoom {
                        invite: Text::new("i".repeat(128)).unwrap(),
                        name: Text::new("n".repeat(48)).unwrap(),
                        rules: Text::new("r".repeat(32)).unwrap(),
                        players: u8::MAX,
                        max_players: u8::MAX,
                        has_password: true,
                        running: true,
                        map: Text::new("m".repeat(32)).unwrap(),
                        year: u16::MAX,
                        companies: u8::MAX,
                        competitive: true,
                        server: Text::new("s".repeat(24)).unwrap(),
                        ping_ms: u16::MAX,
                    };
                    tpf3mp_proto::ROOMS_PER_PAGE
                ])
                .unwrap(),
                more: true,
                servers: BoundedVec::new(vec![
                    LobbyServer {
                        name: Text::new("s".repeat(24)).unwrap(),
                        ping_ms: u16::MAX,
                        here: true,
                        reachable: true,
                    };
                    MAX_LOBBY_SERVERS
                ])
                .unwrap(),
            }),
        }));
        let bytes = encode(&view).unwrap();
        assert_eq!(decode::<ToHook>(&bytes).unwrap(), view);
        let action = ToAgent::Lobby(LobbyAction::Create {
            room: Text::new("Alps").unwrap(),
            max_players: 4,
            password: None,
            rules: Some(Text::new("native").unwrap()),
            start_save: Some(Text::new("mptest").unwrap()),
            listing: Some(LobbyListing {
                map: Text::new("temperate").unwrap(),
                year: 1850,
            }),
            competitive: true,
        });
        assert_eq!(
            decode::<ToAgent>(&encode(&action).unwrap()).unwrap(),
            action
        );
        let choose = ToAgent::Lobby(LobbyAction::ChooseMod {
            id: Text::new("schbrongx_minimap").unwrap(),
            chosen: true,
        });
        assert_eq!(
            decode::<ToAgent>(&encode(&choose).unwrap()).unwrap(),
            choose
        );
        let set = ToAgent::Lobby(LobbyAction::SetServer {
            server: Text::new("s".repeat(128)).unwrap(),
        });
        assert_eq!(decode::<ToAgent>(&encode(&set).unwrap()).unwrap(), set);
        let pick = ToAgent::Lobby(LobbyAction::ChooseStart {
            save: Text::new("s".repeat(MAX_SAVE_NAME)).unwrap(),
            map: Text::new("temperate").unwrap(),
            year: 1900,
        });
        assert_eq!(decode::<ToAgent>(&encode(&pick).unwrap()).unwrap(), pick);
    }

    #[test]
    fn only_the_same_version_is_accepted() {
        assert!(check_version(BRIDGE_VERSION).is_ok());
        assert!(check_version(BRIDGE_VERSION + 1).is_err());
    }
}
