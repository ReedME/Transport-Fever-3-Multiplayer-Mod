//! Messages on the control stream. See `docs/PROTOCOL.md` for their
//! semantics. Variants are identified by position: append, never reorder.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    BoundedVec, ContentDiff, ContentManifest, Platform, RoomDeclaration, RoomMods, Text,
    bytes::{FixedBytes, Payload},
    ids::{Invite, PlayerId, RoomId, SessionId, Signature},
    snapshot::{SavedWorld, SnapshotId},
};

/// Domain separator for the identity proof in [`Hello`].
pub const AUTH_DOMAIN: &[u8] = b"tpf3mp-auth-v1";
/// TLS exporter label for the identity proof's keying material.
pub const AUTH_EXPORTER_LABEL: &[u8] = b"EXPORTER-tpf3mp-auth";

/// Largest number of members a room can have.
pub const MAX_ROOM_MEMBERS: u8 = 64;
/// Largest number of lanes in one [`GameMessage::Checkpoint`].
pub const MAX_CHECKPOINT_LANES: usize = 32;
/// Largest payload of a [`GameMessage::Preview`]: a long road's or a
/// station's build fits; anything larger is not shown.
pub const MAX_PREVIEW: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage {
    Hello(Hello),
    Request { id: u32, request: Request },
    Game(GameMessage),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMessage {
    Welcome(Welcome),
    Reject(Reject),
    Response {
        id: u32,
        result: Result<Response, RequestError>,
    },
    RoomUpdate(RoomView),
    IntentRejected {
        client_seq: u64,
        reason: IntentRejection,
    },
    /// This client's world differs from the room's verdict at a checkpoint,
    /// in these lanes.
    Diverged {
        step: u64,
        lanes: Vec<u16>,
    },
    /// The room's owner removed this player, who cannot come back to it.
    Kicked,
    /// Upload the world this client saved at the save event `event`: open a
    /// bulk stream and serve `snapshot` on it. Event 0 is the world the
    /// owner named for the room to start from ([`Request::StartWorld`]).
    Upload {
        event: u64,
        snapshot: SnapshotId,
    },
    /// A member of the room said something.
    Chat {
        from: PlayerId,
        text: ChatText,
    },
    /// How this player's game differs from the room's, sent whenever that
    /// changes, and before a refused join. `None`: it no longer differs.
    ContentDiff(Option<ContentDiff>),
    /// A message from the server's operator to everyone connected, such as
    /// a restart coming.
    Notice(ChatText),
    // Last, so the earlier variants keep their tags on the wire.
    /// What another member's build tool shows now ([`GameMessage::Preview`]),
    /// relayed as it came; `None` once it shows nothing. Advisory: never
    /// part of the room's world or log.
    Preview {
        from: PlayerId,
        preview: Option<Payload>,
    },
    /// The room's mods, as its owner declared them ([`Request::DeclareRoom`]):
    /// sent to every member when they change, to a member joining the lobby,
    /// and before a refused join to a running game, so that a player learns
    /// which mods to have before declaring theirs. `None`: the room's owner
    /// declared none, and the room only compares content.
    RoomMods(Option<Box<RoomMods>>),
}

/// One chat message: a line of text, no longer than a short paragraph.
pub type ChatText = Text<280>;

/// The client's first message after the preamble.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub client_version: Text<64>,
    pub platform: Platform,
    pub name: Text<32>,
    pub identity: PlayerId,
    /// Ed25519 signature over [`AUTH_DOMAIN`] followed by 32 bytes of TLS
    /// keying material exported under [`AUTH_EXPORTER_LABEL`].
    pub proof: Signature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    pub server_version: Text<64>,
    pub session_id: SessionId,
    /// The rules this server offers rooms, the default first.
    pub rules: Vec<RulesOffer>,
}

/// The name of a set of rules a room is played by, such as `native`: the
/// game's own economy.
pub type RulesName = Text<32>;

/// Rules a server offers: what a host picks from when creating a room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesOffer {
    pub name: RulesName,
    /// What playing by them means, for the host choosing.
    pub description: Text<200>,
}

/// The server's answer to a [`Hello`] it will not serve. The server closes
/// the connection after sending it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reject {
    pub reason: RejectReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectReason {
    ServerFull,
    /// The identity proof did not verify.
    BadProof,
    /// The client's network address already holds its share of sessions.
    TooManyConnections,
}

impl fmt::Display for RejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ServerFull => "the server is full; try again later",
            Self::BadProof => "the server could not verify this client's identity",
            Self::TooManyConnections => {
                "too many players are connected from this network; close another game first"
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    CreateRoom(CreateRoom),
    JoinRoom(JoinRoom),
    LeaveRoom,
    SetReady(bool),
    /// What this player's game runs. Declared once per connection, before
    /// joining a running game; a room's lobby takes it on joining, and
    /// again whenever the player declares anew.
    DeclareContent(ContentManifest),
    StartGame,
    SetSpeed(Speed),
    /// The owner removes a player from the room for good, for example one
    /// whose game froze.
    Kick(PlayerId),
    /// Says something to everyone in the room.
    Chat(ChatText),
    /// Lines of this client's log, redacted, for the server's operator to
    /// read by this session's ID (see "Diagnostics" in PROTOCOL.md).
    Diagnostics(crate::DiagnosticBatch),
    /// The owner, in the lobby: the room's game starts from this world, a
    /// save the owner's client holds, and not from one the owner's game
    /// saves once the game began. The room asks for it at once
    /// ([`ServerMessage::Upload`] with event 0), and when the game starts
    /// every member loads it, the owner too. Declaring another replaces it,
    /// for as long as the room is in its lobby, and marks every member not
    /// ready again: they agreed to the world before. The first one named
    /// leaves readiness as it is: it is the world the room was waiting for.
    /// The same world again only updates what the room shows of it
    /// (`save`). Only on a server that keeps snapshots.
    StartWorld {
        world: SavedWorld,
        save: StartSave,
    },
    /// The owner, in the lobby: the room's game starts from no handed-over
    /// world after all, but from the owner's game, as without
    /// [`Request::StartWorld`]. Marks every member not ready again if the
    /// room had one. Done when it had none.
    ClearStartWorld,
    /// The server's list of public rooms (those created with a
    /// [`CreateRoom::listing`]), [`ROOMS_PER_PAGE`] a page from `page` 0.
    /// Answered with [`Response::Rooms`]. A private room is never listed.
    ListRooms {
        page: u16,
    },
    /// The owner of a public room says what the list shows of it now, such
    /// as the game's year and its companies once it runs. A private room
    /// stays private (`NotListed`).
    DescribeRoom(RoomListing),
    /// The picture this player shows in rooms, one of [`BANNERS`] or
    /// [`PORTRAITS`] by id; `None` for their default. Kept for the connection, and shown to the
    /// room this player is in at once. Unknown ids are refused
    /// (`UnknownBanner`).
    SetBanner(Option<BannerId>),
    /// Lines of this player's logs, redacted, each with its source (the
    /// launcher, the agent, the hook, the game, its error reports), all
    /// under the launcher's run (see "Diagnostics" in PROTOCOL.md). Takes
    /// the place of [`Request::Diagnostics`] from version 16 on.
    Telemetry(crate::Telemetry),
    /// The owner, in the lobby: what their game runs and the room's mods,
    /// together ([`RoomDeclaration`], validated whole). Takes the place of
    /// the owner's [`Request::DeclareContent`]; the room tells every member
    /// ([`ServerMessage::RoomMods`]) and marks them not ready when the mods
    /// change. Refused for anyone else (`NotOwner`), once the game runs
    /// (`GameRunning`), and when it does not hold together
    /// (`InvalidContent`). An owner's plain `DeclareContent` leaves the room
    /// without a list of mods.
    DeclareRoom(Box<RoomDeclaration>),
    /// Checks whether this server can admit the invite and password, without
    /// joining the room. The response intentionally carries no room details.
    ResolveInvite {
        invite: Invite,
        password: Option<Text<64>>,
    },
}

/// A player's picture: one of [`BANNERS`] or [`PORTRAITS`], by id. Long
/// enough for the longest portrait id.
pub type BannerId = Text<32>;

/// The banners players pick from: short ids, each standing for one of the
/// game's own pictures (the window maps them; the server only checks the
/// id). A player who picks none shows one chosen from their key.
pub const BANNERS: &[&str] = &[
    "m01",
    "m02",
    "m03",
    "m04",
    "m05",
    "m06",
    "m07",
    "m08",
    "temperate",
    "subarctic",
    "tropical",
    "dry",
    "mapeditor",
    "mapeditor2",
    "mod01",
    "mod02",
    "main",
    "loadgame",
    "loading1",
    "loading2",
    "loading3",
    "loading4",
];

/// The campaign's characters a player may show instead of a banner, by
/// the name their portrait has in the game's campaign missions
/// (`mission/dialogue/<id>_neutral.tga`). The pictures are the game's: each
/// player's launcher takes them from their own install, and a game without
/// one shows the player's banner instead (docs/LOBBY.md, "Portraits").
pub const PORTRAITS: &[&str] = &[
    "andrew",
    "katie_baker",
    "major",
    "anton_zurbriggen",
    "dr_karl_brandt",
    "lorenzo_bianchi",
    "freiherr_von_schlitzwiesen",
    "nasra_ramahi",
    "salim_al_zalabia",
    "bart_korner",
    "richard_o_sullivan",
    "sun_flowers",
    "andrea",
    "astrid_larsson",
    "lasse",
    "nils_eriksen",
    "mateo_cruz",
    "richard_cleese",
    "salita_ananda_cruz",
    "holly_travers",
    "monaro_namatjira",
    "tom_mclaren",
    "chisato_murai",
    "sayoko_tanizaki",
    "takumi_arakawa",
];

/// Whether `id` names one of [`PORTRAITS`].
pub fn is_portrait(id: &str) -> bool {
    PORTRAITS.contains(&id)
}

/// Whether `id` names a picture a player may show: one of [`BANNERS`] or
/// [`PORTRAITS`].
pub fn is_banner(id: &str) -> bool {
    BANNERS.contains(&id) || is_portrait(id)
}

/// Most rooms a page of the room list holds.
pub const ROOMS_PER_PAGE: usize = 20;

/// What the room list shows of a public room besides what the server knows
/// itself: what its owner declares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomListing {
    /// The world's map type: its climate, as the game names it, such as
    /// `temperate` (`::/climates/temperate/temperate.clima`); empty when
    /// the owner's game did not say.
    pub map: Text<32>,
    /// The game's year: the start year until the owner says another; 0
    /// when unknown.
    pub year: u16,
    /// The companies playing in the room's game.
    pub companies: u8,
}

/// The save a room's game starts from, as its owner names it
/// ([`Request::StartWorld`]) and every member sees it ([`RoomView::start`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartSave {
    /// The save's name in the owner's save folder, without `.sav`.
    pub name: Text<64>,
    /// Its map type, its climate as the game names it (`temperate`); empty
    /// when the owner's game did not say.
    pub map: Text<32>,
    /// Its year; 0 when unknown.
    pub year: u16,
}

/// The world a room in its lobby starts from, as its members see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartView {
    pub save: StartSave,
    /// Whether the room has received it: until then the game cannot start
    /// (`StartWorldPending`).
    pub arrived: bool,
}

/// One public room, as the room list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedRoom {
    /// The room's invite: a public room's is for anyone to join with.
    pub invite: Invite,
    pub name: Text<48>,
    pub rules: RulesName,
    pub players: u8,
    pub max_players: u8,
    pub has_password: bool,
    pub phase: RoomPhase,
    pub listing: RoomListing,
    /// The room's play style, as its owner chose it ([`CreateRoom::competitive`]).
    pub competitive: bool,
}

/// A page of the room list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomPage {
    pub page: u16,
    pub rooms: BoundedVec<ListedRoom, ROOMS_PER_PAGE>,
    /// Whether a later page has more.
    pub more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRoom {
    pub name: Text<48>,
    pub max_players: u8,
    pub password: Option<Text<64>>,
    pub settings: RoomSettings,
    /// One of the rules the server offers (see [`Welcome::rules`]), or its
    /// default.
    pub rules: Option<RulesName>,
    /// `Some` lists the room in the server's room list, where anyone sees
    /// it and its invite; `None`, the default, keeps it private: joined
    /// only by an invite its members pass on.
    pub listing: Option<RoomListing>,
    /// The play style the owner means the room for: `false` co-op (every
    /// player for the room's one company, as a room starts, D21), `true`
    /// competitive (each player for a company of their own). The server
    /// only carries it: players see it and found their companies in the
    /// game as D21 lets them.
    pub competitive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinRoom {
    pub invite: Invite,
    pub password: Option<Text<64>>,
    /// For a running game: where this client continues, to receive only
    /// later turns. `None` means the client has no world of this game: it
    /// receives one to load (see `TurnStart::world`), or, from a server that
    /// keeps no snapshots, the game from its first turn.
    pub resume: Option<Resume>,
}

/// Where a returning client continues a running game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resume {
    /// The last turn this client applied.
    pub after_turn: u64,
    /// The history those turns belong to, from the `TurnStart` of the stream
    /// they came on. A room restored after a crash that lost turns starts a
    /// new history, and refuses to resume anyone past the point where the
    /// two differ.
    pub history: u64,
    /// The producing-world epoch from its turn stream; preserved on resume.
    pub world_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Response {
    RoomCreated {
        invite: Invite,
        room: RoomView,
    },
    RoomJoined(RoomView),
    Done,
    Rooms(RoomPage),
    /// The invite and optional password match an open room on this server.
    InviteMatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestError {
    AlreadyInRoom,
    NotInRoom,
    /// The invite, its room or the password is wrong. The three are
    /// deliberately indistinguishable.
    BadInvite,
    RoomFull,
    NotOwner,
    NotAllReady,
    ContentMismatch,
    GameRunning,
    GameNotRunning,
    InvalidSettings,
    TooManyRooms,
    /// The requested resume point is no longer held by the server.
    ResumeUnavailable,
    /// The connection sent requests faster than the server allows.
    RateLimited,
    /// No such player is in the room.
    NoSuchPlayer,
    /// The owner cannot kick themselves; they can leave.
    CannotKickSelf,
    /// The server does not offer the rules asked for.
    UnknownRules,
    /// The declared content exceeds a manifest's limits.
    InvalidContent,
    /// The server keeps no more diagnostics from this session: it keeps
    /// none, or this session sent all it may.
    DiagnosticsNotKept,
    /// The server keeps no worlds, so a room cannot be handed one.
    WorldsNotKept,
    /// The world the room starts from is still on its way to the server.
    StartWorldPending,
    /// The room is private: it is in no list to describe.
    NotListed,
    /// No such banner or portrait (see [`BANNERS`], [`PORTRAITS`]).
    UnknownBanner,
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AlreadyInRoom => "already in a room",
            Self::NotInRoom => "not in a room",
            Self::BadInvite => "the invite or password is not valid",
            Self::RoomFull => "the room is full",
            Self::NotOwner => "only the room owner can do that",
            Self::NotAllReady => "not every player is ready",
            Self::ContentMismatch => "players have different game versions or mods",
            Self::GameRunning => "the game is already running",
            Self::GameNotRunning => "the game is not running",
            Self::InvalidSettings => "the room settings are out of range",
            Self::TooManyRooms => "the server cannot host more rooms",
            Self::ResumeUnavailable => "the game can no longer be resumed from that point",
            Self::RateLimited => "too many requests; try again in a moment",
            Self::NoSuchPlayer => "no such player is in the room",
            Self::CannotKickSelf => "the owner cannot kick themselves; leave the room instead",
            Self::UnknownRules => "this server does not offer those rules",
            Self::InvalidContent => "the game's list of mods is too long to declare",
            Self::DiagnosticsNotKept => "the server keeps no more diagnostics from this session",
            Self::WorldsNotKept => {
                "this server keeps no worlds, so a room cannot start from a save"
            }
            Self::StartWorldPending => {
                "the save the room starts from is still being uploaded; start once it is there"
            }
            Self::NotListed => "the room is private, so it is in no list",
            Self::UnknownBanner => "there is no such banner",
        })
    }
}

/// Settings fixed when a room is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomSettings {
    /// Simulation steps per second at 1x speed. TPF2 runs 5; TPF3 is
    /// measured on release day.
    pub steps_per_second: u16,
    /// How far ahead of the room clock the server seals steps. Clients play
    /// behind a jitter buffer of their own, so this does not set the delay
    /// players feel (see "Playout" in `docs/PROTOCOL.md`).
    pub input_delay_ms: u16,
    /// Members report checkpoint digests at every step divisible by this.
    pub checkpoint_interval: u32,
}

impl RoomSettings {
    pub const DEFAULT: Self = Self {
        steps_per_second: 5,
        input_delay_ms: 250,
        checkpoint_interval: 50,
    };

    /// Whether every setting is within the range the server accepts.
    pub fn is_valid(&self) -> bool {
        (1..=240).contains(&self.steps_per_second)
            && (20..=2000).contains(&self.input_delay_ms)
            && (1..=1_000_000).contains(&self.checkpoint_interval)
    }
}

/// Digest of a [`ContentManifest`]: the game build and the mods in load
/// order. Players must match exactly to play together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContentFingerprint(pub FixedBytes<32>);

/// Session speed in percent of normal: `100` is 1x, `0` pauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Speed(pub u16);

impl Speed {
    pub const PAUSED: Self = Self(0);
    pub const NORMAL: Self = Self(100);
    pub const MAX: Self = Self(1600);

    pub fn is_paused(self) -> bool {
        self.0 == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomView {
    pub id: RoomId,
    pub name: Text<48>,
    /// The rules the room is played by.
    pub rules: RulesName,
    pub owner: PlayerId,
    pub max_players: u8,
    pub has_password: bool,
    pub phase: RoomPhase,
    pub settings: RoomSettings,
    pub members: Vec<MemberView>,
    /// The play style ([`CreateRoom::competitive`]).
    pub competitive: bool,
    /// In the lobby: the save the owner handed over for the game to start
    /// from ([`Request::StartWorld`]); `None` when the owner's game provides
    /// the world, and once the game runs.
    pub start: Option<StartView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoomPhase {
    Lobby,
    Running,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberView {
    pub player: PlayerId,
    pub name: Text<32>,
    pub platform: Platform,
    pub ready: bool,
    pub content: Option<ContentFingerprint>,
    pub connected: bool,
    /// The banner this player picked, if any.
    pub banner: Option<BannerId>,
    /// How far this player's game is with the room's world while it comes
    /// in ([`GameMessage::Loading`]); `None` otherwise.
    pub loading: Option<LoadingStage>,
    /// How this player's game differs from the room's (the owner's in the
    /// lobby, the game's once it runs); `None` while it does not, or while
    /// either has not said ([`MemberView::content`]).
    pub differs: Option<ContentStatus>,
}

/// How a member's game differs from the room's, in counts: what the room's
/// owner sees of each member.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentStatus {
    /// The room's mods this game lacks.
    pub missing: u16,
    /// The room's mods it has in another version.
    pub changed: u16,
    /// Mods it runs that the room does not.
    pub extra: u16,
    /// It runs another game build.
    pub game: bool,
    /// The same mods, in another order.
    pub reordered: bool,
    /// The mods beyond a manifest's listed ones differ.
    pub unlisted: bool,
}

impl ContentDiff {
    /// The difference in counts.
    pub fn status(&self) -> ContentStatus {
        let count = |total: u32| u16::try_from(total).unwrap_or(u16::MAX);
        ContentStatus {
            missing: count(self.missing_total),
            changed: count(self.changed_total),
            extra: count(self.extra_total),
            game: self.game.is_some(),
            reordered: self.reordered,
            unlisted: self.unlisted,
        }
    }
}

/// Where a player's game is with the room's world while it comes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoadingStage {
    /// Receiving it: this many percent so far (0 to 100).
    Fetching { percent: u8 },
    /// The game loads it.
    Loading,
}

/// A client's game traffic, carried on the control stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameMessage {
    Intent {
        client_seq: u64,
        payload: Payload,
        /// A password the intent needs, such as a company's: the server
        /// seals it ([`crate::Seal`]) into the event it orders, and neither
        /// logs nor relays the password itself.
        secret: Option<Secret>,
    },
    /// The last step this client has executed.
    Progress { world_epoch: u64, step: u64 },
    Checkpoint {
        world_epoch: u64,
        step: u64,
        lanes: Vec<LaneDigest>,
    },
    /// This client saved its world at the save event `event`, where its
    /// lanes were these. `world` is `None` if the save failed.
    Saved {
        world_epoch: u64,
        event: u64,
        lanes: Vec<LaneDigest>,
        world: Option<SavedWorld>,
    },
    // Last, so the earlier variants keep their tags on the wire.
    /// Where this player's game is with the room's world while it comes in,
    /// for the other members to see (`MemberView::loading`); `None` once it
    /// plays or has none coming. At most about two a second; the room keeps
    /// no more of them, and logs none.
    Loading(Option<LoadingStage>),
    /// What this player's build tool shows now, for the other members to
    /// see in their games: the action it would build, encoded as an
    /// intent's payload, at most [`MAX_PREVIEW`] bytes; `None` once it shows
    /// nothing. Advisory: the room relays it to the other members of a
    /// running game and keeps, orders and logs none. Sent again every few
    /// seconds while it shows, so a receiver forgets one that stops coming.
    Preview(Option<Payload>),
    /// Acknowledges the world ordered on the turn stream, before reports.
    Loaded { world_epoch: u64, step: u64 },
}

/// A password a player typed for an intent: a company's, to join it or to
/// set it (docs/PROTOCOL.md, "Secrets"). It goes to the server beside the
/// intent and no further: the server orders the intent with a
/// [`crate::Seal`] of it, an HMAC under the server's key, which every game
/// compares with the seal it keeps. `Debug` never shows the password, so a
/// log line of the message gives nothing away.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Secret {
    /// What the password is for, as the intent names it (a company's id).
    /// The seal binds it, so a password sealed for one company fits no
    /// other.
    pub scope: u64,
    pub password: Text<64>,
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Secret")
            .field("scope", &self.scope)
            .field("password", &"<hidden>")
            .finish()
    }
}

/// The digest of one lane of world state at a checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneDigest {
    pub lane: u16,
    pub digest: FixedBytes<32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntentRejection {
    GameNotRunning,
    RateLimited,
    /// The room's rules refused the intent; the code is ruleset-defined.
    Refused {
        code: u16,
    },
}
