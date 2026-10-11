//! Connects a game, through its hook, to a room.
//!
//! Turns from the server become the messages the hook's
//! [`Gate`](tpf3mp_bridge::Gate) expects, released at the pace
//! [`Playout`] sets. What the hook reports becomes intents, progress,
//! checkpoints and saves for the room.
//!
//! A turn stream may start from a world the room agreed on (a player
//! joining a running game, one who could no longer resume, one rebased
//! after diverging). The bridge then fetches that world, has the game load
//! it, and only then lets the stream's turns through.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use thiserror::Error;
use tokio::{
    sync::{mpsc, watch},
    task::AbortHandle,
};
use tpf3mp_bridge::{
    BridgeError, LobbyAction, LobbyView, MAX_MESSAGE, MAX_PATH, ModLists, RoomInfo, RoomMember,
    ToAgent, ToHook, check_version, decode, encode,
};
use tpf3mp_net::close;
use tpf3mp_proto::{
    BoundedVec, ChatText, ContentDiff, Event, EventBody, Invite, JoinRoom, LaneDigest,
    LoadingStage, MAX_PREVIEW, MAX_ROOM_MEMBERS, Payload, PlayerId, Request, RequestError, Resume,
    RoomPhase, RoomView, SavedWorld, SessionId, SnapshotId, Speed, StartSave, Text, TurnStart,
    WorldOffer,
};
use tpf3mp_snapshot::ManifestId;
use tracing::{debug, info, warn};

use crate::{
    Action, Client, ClientError, ClientEvent, ConnectOptions, Events, FollowError, Playout,
    TurnFollower, Worlds, connect, picker::Declaration, transfer,
};

/// The agent's end of the link to the hook.
pub trait HookLink: Send {
    /// Sends one encoded message. `Ok(false)` means the hook is not reading
    /// fast enough; the bridge tries again later.
    fn send(&mut self, message: &[u8]) -> Result<bool, BridgeFault>;
    /// Receives one encoded message into `buf`, if one is waiting.
    fn recv(&mut self, buf: &mut Vec<u8>) -> Result<bool, BridgeFault>;
    /// Signals that the agent is alive.
    fn heartbeat(&mut self);
    /// The hook's heartbeat counter, which advances while it is alive.
    fn peer_heartbeat(&self) -> u64;
}

impl HookLink for tpf3mp_ipc::Link {
    fn send(&mut self, message: &[u8]) -> Result<bool, BridgeFault> {
        match tpf3mp_ipc::Link::send(self, message) {
            Ok(()) => Ok(true),
            Err(tpf3mp_ipc::SendError::Full) => Ok(false),
            Err(error) => Err(BridgeFault::Link(error.to_string())),
        }
    }

    fn recv(&mut self, buf: &mut Vec<u8>) -> Result<bool, BridgeFault> {
        buf.resize(MAX_MESSAGE, 0);
        match self.recv_into(buf) {
            Ok(Some(len)) => {
                buf.truncate(len);
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(error) => Err(BridgeFault::Link(error.to_string())),
        }
    }

    fn heartbeat(&mut self) {
        tpf3mp_ipc::Link::heartbeat(self);
    }

    fn peer_heartbeat(&self) -> u64 {
        tpf3mp_ipc::Link::peer_heartbeat(self)
    }
}

#[derive(Debug, Clone)]
pub struct BridgeOptions {
    /// Playout margin: each step plays this long after its seal arrives.
    pub playout_margin: Duration,
    /// How long a late arrival keeps the playout buffer grown.
    pub playout_memory: Duration,
    /// How often the hook's messages are read.
    pub poll: Duration,
    /// A hook whose heartbeat stands still this long while the game runs is
    /// gone.
    pub hook_timeout: Duration,
    /// The same, while the game loads its world, which can take minutes.
    pub load_timeout: Duration,
    /// The least time between two progress reports to the server.
    pub progress_every: Duration,
    /// Where worlds are kept. Without it, saves are reported as failed and
    /// a world the room offers cannot be loaded.
    pub worlds: Option<Worlds>,
    /// What a front end shows of the session, kept up to date by the
    /// bridge.
    pub status: Option<SharedStatus>,
    /// The launcher's lobby, for the game's main-menu window (D17).
    pub lobby: Option<LobbyLink>,
    /// A save of the player's own that the room's game starts from, when
    /// this player owns the room: the bridge hands it to the room in the
    /// lobby (`Request::StartWorld`), and every game, this one too, loads
    /// it from its main menu when the game starts. Without it, the owner's
    /// game has the world up and saves it for the room once the game began.
    /// The owner may name another while the room is in its lobby
    /// ([`Control::StartWorld`]).
    pub start_world: Option<PathBuf>,
    /// A newly generated world starts once its owner has loaded it and
    /// every member is ready. Existing-save rooms keep their Start button.
    pub start_generated_world: bool,
    /// What the room shows every member of [`Self::start_world`]: its name,
    /// map and year. Without, its file's name, the map and year unknown.
    pub start_save: Option<StartSave>,
    /// This player's mods for the room's worlds: the shared ones it
    /// declared and its personal ones (`crate::content::split`), handed to
    /// the hook when the game begins. Without them a world loads with the
    /// mods its save lists.
    pub mods: Option<ModLists>,
    /// The player's mods as they choose them in the lobby (`crate::picker`):
    /// the lists at the moment the game begins, in place of `mods`, and what
    /// to declare anew when the room says how this game differs.
    pub picker: Option<PickerLink>,
}

/// The mods the room's worlds load with, now.
pub type ListsNow = Arc<dyn Fn() -> Option<ModLists> + Send + Sync>;
/// Takes in how the room says this game differs; returns what to declare
/// anew, if that changed.
pub type Learn = Arc<dyn Fn(&ContentDiff) -> Option<Declaration> + Send + Sync>;
/// Takes in the room's mods as the room tells them, and whether this player
/// owns the room now; returns what to declare anew, if that changed.
pub type Adopt =
    Arc<dyn Fn(Option<&tpf3mp_proto::RoomMods>, bool) -> Option<Declaration> + Send + Sync>;

/// The launcher's mod picker, as a bridge asks it (`crate::picker::Mods`).
#[derive(Clone)]
pub struct PickerLink {
    pub lists: ListsNow,
    pub learn: Learn,
    pub adopt: Adopt,
}

impl std::fmt::Debug for PickerLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PickerLink")
    }
}

/// The launcher's lobby as a bridge passes it on: the lobby to show the
/// game's main-menu window, and where the window's actions go (D17).
#[derive(Debug, Clone)]
pub struct LobbyLink {
    pub views: watch::Receiver<LobbyView>,
    pub actions: mpsc::UnboundedSender<LobbyAction>,
}

impl Default for BridgeOptions {
    fn default() -> Self {
        Self {
            playout_margin: Duration::from_millis(20),
            playout_memory: Duration::from_secs(10),
            poll: Duration::from_millis(2),
            hook_timeout: Duration::from_secs(60),
            load_timeout: Duration::from_secs(600),
            progress_every: Duration::from_millis(20),
            worlds: None,
            status: None,
            lobby: None,
            start_world: None,
            start_generated_world: false,
            start_save: None,
            mods: None,
            picker: None,
        }
    }
}

/// What a front end asks of the room session a bridge runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    Ready(bool),
    Start,
    Speed(Speed),
    Kick(PlayerId),
    Chat(ChatText),
    /// Show this banner in the room from now on.
    Banner(Option<tpf3mp_proto::BannerId>),
    /// The room's owner, in its lobby: the room starts from this save now,
    /// in place of the one before; `None` for none, the owner's game then
    /// providing the world. `declare` is what this game declares first,
    /// when the room's shared mods change with the save (`crate::picker`).
    /// Everyone is asked to get ready again.
    StartWorld {
        start: Option<(PathBuf, StartSave)>,
        declare: Option<Declaration>,
    },
    /// Declare this anew: the room's owner's new list of the room's mods,
    /// or this player's content after the mods installed changed.
    Declare(Declaration),
    /// Leave the room, which ends the session.
    Leave,
    /// The game the front end started has exited. Once its hook attached,
    /// this ends the session as a hook that stopped responding does
    /// ([`BridgeFault::GameClosed`]), without waiting out the heartbeat
    /// limits, so the player can start the game again at once. Before
    /// then the game never joined, and the session waits for the next one.
    GameClosed,
}

/// How often the room hears this game's loading progress again while it
/// stays fetching: about two a second (`GameMessage::Loading`).
const LOADING_EVERY: Duration = Duration::from_millis(500);

/// Chat lines and notices a status keeps.
const STATUS_HISTORY: usize = 100;

/// What a front end shows of the room session a bridge runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The room as last announced.
    pub room: Option<RoomView>,
    /// The game's build, once its hook attached.
    pub game: Option<String>,
    pub world: WorldStatus,
    /// The last step the game ran.
    pub step: Option<u64>,
    pub speed: Speed,
    /// The latest chat, oldest first.
    pub chat: VecDeque<(PlayerId, ChatText)>,
    /// What the player should know, oldest first: divergences, refusals,
    /// rejoins.
    pub notices: VecDeque<String>,
    /// How this player's game differs from the room's, while it does.
    pub content_diff: Option<ContentDiff>,
    /// The room's mods, as its owner declared them and the room told them.
    pub room_mods: Option<tpf3mp_proto::RoomMods>,
    /// The server's name for the current connection, which its log uses:
    /// what a player quotes to the server's operator.
    pub session: Option<SessionId>,
    /// The server speaks a newer protocol: this client must update to
    /// play there.
    pub outdated: bool,
    /// The operator's latest notice, such as a restart coming.
    pub announcement: Option<String>,
    /// The save this player, the room's owner, is handing the room to start
    /// from, while it is on its way.
    pub start_upload: Option<StartUpload>,
}

/// The owner's save on its way to the room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartUpload {
    /// Its name, as the room will show it.
    pub save: String,
    /// How much of it went up, 0 to 100; 0 while it is read.
    pub percent: u8,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            room: None,
            game: None,
            world: WorldStatus::None,
            step: None,
            speed: Speed::NORMAL,
            chat: VecDeque::new(),
            notices: VecDeque::new(),
            content_diff: None,
            room_mods: None,
            session: None,
            outdated: false,
            announcement: None,
            start_upload: None,
        }
    }
}

impl Status {
    pub fn notice(&mut self, notice: impl Into<String>) {
        push_bounded(&mut self.notices, notice.into());
    }

    /// Takes the operator's notice `text`: the latest shown on its own, and
    /// every one among the notices.
    pub fn announce(&mut self, text: &str) {
        self.announcement = Some(text.to_owned());
        self.notice(format!("from the server: {text}"));
    }
}

/// Where the game's world stands, for display.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorldStatus {
    /// No game has begun.
    #[default]
    None,
    /// Fetching the world to load: bytes present of the whole.
    Fetching { bytes: u64, total: u64 },
    /// The game is loading its world.
    Loading,
    /// The game plays the room's world.
    Playing,
}

/// A status shared between a bridge and a front end.
pub type SharedStatus = Arc<Mutex<Status>>;

/// `part` of `whole` in whole percent, 0 to 100.
fn percent_of(part: u64, whole: u64) -> u8 {
    if whole == 0 {
        return 0;
    }
    let percent = u128::from(part.min(whole)) * 100 / u128::from(whole);
    u8::try_from(percent).unwrap_or(100)
}

/// Whether `stage` should be told the room, which last heard `reported`.
fn loading_due(
    reported: Option<(Option<LoadingStage>, Instant)>,
    stage: Option<LoadingStage>,
    now: Instant,
) -> bool {
    let Some((last, at)) = reported else {
        // Nothing told on this connection: nothing to clear either.
        return stage.is_some();
    };
    if last == stage {
        return false;
    }
    let same_fetch = matches!(
        (last, stage),
        (
            Some(LoadingStage::Fetching { .. }),
            Some(LoadingStage::Fetching { .. })
        )
    );
    !same_fetch || now.saturating_duration_since(at) >= LOADING_EVERY
}

fn push_bounded<T>(list: &mut VecDeque<T>, item: T) {
    if list.len() == STATUS_HISTORY {
        list.pop_front();
    }
    list.push_back(item);
}

/// What the player is told when rejoining finds the room gone, in the
/// launcher's window and the game's Multiplayer window alike.
pub const ROOM_GONE: &str = "The room is gone (closed or the server restarted)";

#[derive(Debug, Error)]
pub enum BridgeFault {
    #[error("the link to the hook failed: {0}")]
    Link(String),
    #[error(transparent)]
    Message(#[from] BridgeError),
    #[error("the hook sent {0} out of place")]
    Unexpected(&'static str),
    #[error("the hook stopped responding")]
    HookGone,
    #[error("Transport Fever 3 closed")]
    GameClosed,
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("the server broke a turn invariant: {0}")]
    Follow(#[from] FollowError),
    #[error(
        "Lost the room and could not rejoin it ({0}); it may be gone (closed or the server restarted)"
    )]
    Rejoin(String),
    /// Rejoining found no room: it closed, or the server restarted without
    /// it. Rejoining stops; the player is back on the server, in no room.
    #[error("{ROOM_GONE}")]
    RoomGone,
    /// The room's world does not run TPF3-MP's mod: loaded, it would hold
    /// paused for good, without a word.
    #[error("{}", crate::save_check::WORLD_WITHOUT_OWN_MOD)]
    WorldWithoutOwnMod,
    #[error("the game loaded its world to run step {got} next, but step {expected} was ordered")]
    LoadedElsewhere { expected: u64, got: u64 },
    #[error("the room sent a world to load, but this agent keeps no worlds")]
    NoWorlds,
    #[error("the path {0} is too long for the link to the game")]
    PathTooLong(PathBuf),
}

/// How a bridged session ended.
#[derive(Debug)]
pub enum BridgeEnd {
    /// The connection to the server closed.
    Closed(quinn::ConnectionError),
    /// The room's owner removed this player.
    Kicked,
    /// The client's events ended.
    EventsEnded,
    /// The world to load could not be fetched. Rejoining gets a new offer.
    WorldUnavailable,
    /// The player left the room.
    Left,
}

/// Where the game's world stands with respect to the turn stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum World {
    /// The game runs the stream's world.
    Ready,
    /// The stream starts from this world, which is being fetched. Nothing
    /// reaches the game until it has loaded it.
    Fetching {
        offer: WorldOffer,
        next_step: u64,
        attempt: u64,
    },
    /// The game is loading a world, after which it runs `next_step`.
    Loading { next_step: u64 },
}

/// Work the bridge handed off, finished.
#[derive(Debug)]
enum Done {
    Ingested {
        generation: u64,
        world_epoch: u64,
        event: u64,
        lanes: Vec<LaneDigest>,
        result: Result<(ManifestId, SavedWorld), String>,
        hold: Option<transfer::PendingWorld>,
    },
    Fetched {
        attempt: u64,
        result: Result<(PathBuf, ManifestId), String>,
        hold: Option<transfer::PendingWorld>,
    },
    Uploaded {
        snapshot: SnapshotId,
        result: Result<u64, String>,
    },
    /// The save the room starts from is cut into the store. `attempt`
    /// counts the saves named ([`Bridge::start_attempt`]).
    StartCut {
        attempt: u64,
        result: Result<(ManifestId, SavedWorld, Option<FileStamp>), String>,
        hold: Option<transfer::PendingWorld>,
    },
    /// The room would not take the save it was to start from.
    StartRefused { attempt: u64, error: String },
    /// The room took back the save it was to start from.
    StartCleared { attempt: u64 },
}

/// Where the save the room starts from stands, when this player hands one
/// over (see [`BridgeOptions::start_world`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartWorld {
    /// None to hand over, or no longer: the room's game began, or handing
    /// it over failed and the room starts as without one.
    None,
    /// Named, waiting for the room's lobby with this player as its owner.
    Named,
    /// Being cut into the store.
    Cutting,
    /// Told to the room, which asks for it and receives it.
    Told(SnapshotId),
    /// The room has it: every game loads it when the game starts.
    Uploaded(SnapshotId),
    /// The owner took it back; the room is being told.
    Clearing,
}

/// Saves of this game the store keeps, newest last: the room asks for the
/// newest, and the one before may still be in flight.
const SAVES_KEPT: usize = 2;

/// A file's size and last write, to tell whether it changed since.
type FileStamp = (u64, std::time::SystemTime);

fn file_stamp(file: &Path) -> Option<FileStamp> {
    let metadata = std::fs::metadata(file).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}

/// The lists a game without the picker (`--mods`) loads the room's world
/// with: the room's mods and settings as its owner declared them, when it
/// did, then the player's listed personal mods; the listed ones alone
/// otherwise. The content check made the listed shared mods the room's,
/// but not the settings: without the room's, this game would load the
/// save's while every other game loads the owner's.
fn told_lists(
    told: Option<&tpf3mp_proto::RoomMods>,
    listed: Option<&ModLists>,
) -> Option<ModLists> {
    let Some(told) = told else {
        return listed.cloned();
    };
    let shared = told
        .mods
        .iter()
        .map(|m| tpf3mp_bridge::ModName::new(m.id.as_str()).ok())
        .collect::<Option<Vec<_>>>()?;
    Some(ModLists {
        shared: tpf3mp_proto::BoundedVec::new(shared).ok()?,
        personal: listed
            .map(|lists| lists.personal.clone())
            .unwrap_or_default(),
        params: told.params.clone(),
    })
}

/// Most events held after a game's turn stream until the room said its mods
/// (which it does right after the join): far more than that moment brings.
const MAX_HELD_EVENTS: usize = 4096;

/// Most bytes held so, far below what the client's turn budget
/// and the hook's outbox allow a hostile server to make this game hold.
const MAX_HELD_BYTES: usize = 16 << 20;

/// What any other held event is taken to cost.
const HELD_EVENT_OVERHEAD: usize = 256;

/// What a held event costs to hold: a turn as the follower weighs one.
fn held_weight(event: &ClientEvent) -> usize {
    match event {
        // As the follower weighs a turn it holds: every event costs.
        ClientEvent::Turn(turn) => crate::follower::turn_weight(turn),
        _ => HELD_EVENT_OVERHEAD,
    }
}

/// Couples one game's hook to one client.
pub struct Bridge<L> {
    link: L,
    options: BridgeOptions,
    follower: Option<TurnFollower>,
    playout: Option<Playout>,
    /// Messages for the hook, in order, sent as the hook takes them.
    outbox: Outbox,
    hook_ready: bool,
    begun: bool,
    world: World,
    /// Every load sent to the hook still awaiting its answer, in send order.
    /// A server restart can replace a load before its answer arrives.
    sent_loads: VecDeque<(u64, u64, u64)>,
    /// Identifies the latest load order even if it starts at the same step.
    load_generation: u64,
    world_epoch: u64,
    /// Retried on exact reconnect: enqueueing locally is not a server ack.
    last_loaded: Option<(u64, u64)>,
    /// Whether the game has loaded a world since the last load was ordered.
    loaded: bool,
    /// The room's speed as the hook was last told it; none before the
    /// first turn.
    speed: Option<Speed>,
    /// Commands the hook has sent; numbers each one's intent.
    commands: u64,
    /// The last step the game ran, or before any, the step before its
    /// first: the progress to report.
    progress: Option<u64>,
    /// The progress last reported on the current connection.
    reported: Option<u64>,
    /// How much of the world being fetched is here, in percent.
    fetch_percent: Arc<AtomicU8>,
    /// The loading stage last told the room on the current connection, and
    /// when; `None` before any.
    loading_reported: Option<(Option<LoadingStage>, Instant)>,
    last_report: Instant,
    wait_until: Option<Instant>,
    hook_beat: (u64, Instant),
    buf: Vec<u8>,
    done_tx: mpsc::UnboundedSender<Done>,
    done_rx: mpsc::UnboundedReceiver<Done>,
    /// Counts fetches, so a result that arrives after a newer offer is
    /// ignored.
    attempts: u64,
    fetch: Option<AbortHandle>,
    /// Snapshots of the game's own saves, newest last.
    saved: VecDeque<ManifestId>,
    /// The world last received.
    received: Option<ManifestId>,
    /// The save the room starts from, when this player hands one over.
    start: StartWorld,
    /// Its snapshot, kept in the store for the upload and for this game's
    /// own load of it.
    start_kept: Option<ManifestId>,
    /// Counts the saves named to start from, so the work for one replaced
    /// since is ignored.
    start_attempt: u64,
    /// The save the room holds to start from, as far as this agent handed
    /// it over: named again, it needs no upload.
    start_held: Option<SnapshotId>,
    /// What to declare to the room before naming the save on its way: the
    /// room's shared mods follow it.
    start_declare: Option<Declaration>,
    /// The save told to the room to start from, and its file's size and
    /// time as it was read.
    start_told: Option<(SavedWorld, FileStamp)>,
    /// What a front end asks of the session.
    controls: Option<mpsc::Receiver<Control>>,
    /// The room as the server last showed it: the game's Multiplayer window
    /// shows it, and chat names its members.
    room: Option<RoomView>,
    /// The room's phase as last announced, if it was.
    room_phase: Option<RoomPhase>,
    /// The latest world the game said is up ([`ToAgent::WorldUp`]), or 0.
    world_up: u64,
    /// The latest world the player's readiness was decided for.
    readied: u64,
    /// The room's owner as last announced, if it was.
    room_owner: Option<PlayerId>,
    /// The latest menu arrival the game told ([`ToAgent::MenuUp`]), or 0.
    menu_up: u64,
    /// The latest menu arrival the player's readiness was decided for.
    menu_readied: u64,
    /// The game's build, once its hook said hello.
    build: Option<String>,
    /// What this game last declared to the room in the session, when the
    /// picker changed it: declared again on a new connection.
    declared: Option<Declaration>,
    /// The room's mods and settings as its owner last declared them
    /// (`RoomMods`), for a game without the picker (`--mods`).
    told: Option<Box<tpf3mp_proto::RoomMods>>,
    /// Whether the room said its mods on this session (`RoomMods`, which it
    /// sends every member once a connection, `None` included), and the
    /// turn stream that would begin the game held until it has: the lists
    /// the game begins with carry the room's mods and settings.
    room_heard: bool,
    held_stream: Option<TurnStart>,
    /// What came after that stream until then, taken up in order after it.
    held_events: VecDeque<ClientEvent>,
    /// The commands' bytes among them.
    held_bytes: usize,
    /// What the hook said while the bridge had no connection, rejoining:
    /// read so the game's window still reaches the launcher (its Leave
    /// above all), and taken up first once the room is back.
    held: VecDeque<Result<ToAgent, BridgeError>>,
}

/// Most hook messages held while rejoining; past it, the rest wait in the
/// link, as they did before.
const MAX_HELD: usize = 4096;

impl<L: HookLink> Bridge<L> {
    /// A new connection to the room: it is told the room's mods anew, which
    /// may have changed while this game was away, and what was held for the
    /// one before goes with it.
    fn on_new_connection(&mut self) {
        self.room_heard = false;
        self.held_stream = None;
        self.held_events.clear();
        self.held_bytes = 0;
    }

    /// The lists this game loads the room's worlds with: the picker's, or
    /// without it the room's as told (`--mods`); none, and a world loads
    /// with its save's own mods.
    fn load_lists(&self) -> Option<ModLists> {
        match &self.options.picker {
            Some(picker) => (picker.lists)(),
            None => told_lists(self.told.as_deref(), self.options.mods.as_ref()),
        }
    }

    pub fn new(link: L, options: BridgeOptions) -> Self {
        let now = Instant::now();
        let (done_tx, done_rx) = mpsc::unbounded_channel();
        Self {
            hook_beat: (link.peer_heartbeat(), now),
            link,
            follower: None,
            playout: None,
            outbox: Outbox::default(),
            hook_ready: false,
            begun: false,
            world: World::Ready,
            sent_loads: VecDeque::new(),
            load_generation: 0,
            world_epoch: 1,
            last_loaded: None,
            loaded: false,
            speed: None,
            commands: 0,
            progress: None,
            reported: None,
            fetch_percent: Arc::new(AtomicU8::new(0)),
            loading_reported: None,
            last_report: now,
            wait_until: None,
            buf: Vec::new(),
            done_tx,
            done_rx,
            attempts: 0,
            fetch: None,
            saved: VecDeque::new(),
            received: None,
            start: if options.start_world.is_some() {
                StartWorld::Named
            } else {
                StartWorld::None
            },
            start_kept: None,
            start_attempt: 0,
            start_held: None,
            start_declare: None,
            start_told: None,
            controls: None,
            room: None,
            room_phase: options.status.as_ref().and_then(|status| {
                let status = status.lock().unwrap_or_else(PoisonError::into_inner);
                status.room.as_ref().map(|room| room.phase)
            }),
            world_up: 0,
            readied: 0,
            room_owner: options.status.as_ref().and_then(|status| {
                let status = status.lock().unwrap_or_else(PoisonError::into_inner);
                status.room.as_ref().map(|room| room.owner)
            }),
            menu_up: 0,
            menu_readied: 0,
            options,
            build: None,
            declared: None,
            told: None,
            room_heard: false,
            held_stream: None,
            held_events: VecDeque::new(),
            held_bytes: 0,
            held: VecDeque::new(),
        }
    }

    /// Takes over a link whose hook has already said hello, as `build`, and
    /// was answered (the launcher's lobby link, `launcher::lobby`).
    pub fn greeted(mut self, build: &str) -> Self {
        self.hook_ready = true;
        self.build = Some(build.to_owned());
        self.status(|status| status.game = Some(build.to_owned()));
        self
    }

    /// Gives the link back, with the game's build if its hook said hello:
    /// the launcher keeps it for the game's next room.
    pub fn into_link(self) -> (L, Option<String>) {
        let build = self.build.filter(|_| self.hook_ready);
        (self.link, build)
    }

    /// Queues the launcher's lobby for the hook when it changed: only the
    /// newest waits to go.
    fn lobby_news(&mut self) {
        let Some(lobby) = &mut self.options.lobby else {
            return;
        };
        if !lobby.views.has_changed().unwrap_or(false) {
            return;
        }
        let view = lobby.views.borrow_and_update().clone();
        self.outbox
            .retain(|message| !matches!(message, ToHook::Lobby(_)));
        self.outbox.push_back(ToHook::Lobby(Box::new(view)));
    }

    /// The player acted in the game's main-menu window: the launcher does
    /// it.
    fn lobby_action(&self, action: LobbyAction) {
        match &self.options.lobby {
            Some(lobby) => {
                let _ = lobby.actions.send(action);
            }
            None => debug!(?action, "a lobby action with no launcher to take it"),
        }
    }

    /// Takes a front end's requests (ready, start, speed, chat, leave) from
    /// `controls` while the session runs.
    pub fn with_controls(mut self, controls: mpsc::Receiver<Control>) -> Self {
        self.controls = Some(controls);
        self
    }

    /// Updates the front end's view of the session, if it has one.
    fn status(&self, update: impl FnOnce(&mut Status)) {
        if let Some(status) = &self.options.status {
            update(&mut status.lock().unwrap_or_else(PoisonError::into_inner));
        }
    }

    /// Runs the session on one connection until it closes or something
    /// breaks. Waits for the hook to attach first; the room may start before
    /// it. The bridge keeps its state, so after a lost connection it can run
    /// again on a new one that resumes the room (see [`play`]). The hook
    /// is not told the session ended; [`Bridge::end`] does that.
    pub async fn run(
        &mut self,
        client: &Client,
        events: &mut Events,
    ) -> Result<BridgeEnd, BridgeFault> {
        loop {
            let now = Instant::now();
            self.link.heartbeat();
            self.check_hook(now)?;
            self.read_hook(client).await?;
            self.hand_over_start_world(client);
            self.start_generated_world(client);
            self.report_progress(client, now).await?;
            self.report_loading(client, now).await?;
            self.lobby_news();
            // Turns wait while the world they continue is being fetched.
            if !matches!(self.world, World::Fetching { .. })
                && let (Some(follower), Some(playout)) = (&mut self.follower, &mut self.playout)
            {
                self.wait_until = pump(follower, playout, now, &mut self.outbox);
            }
            self.flush()?;
            let poll_at = now + self.options.poll;
            let wake = self.wait_until.map_or(poll_at, |at| at.min(poll_at));
            // While the hook falls behind, the room's turns wait in the
            // client, and past its bound on the server's stream.
            let taking = !self.outbox.is_full();
            tokio::select! {
                event = events.recv(), if taking => {
                    let Some(event) = event else {
                        return Ok(BridgeEnd::EventsEnded);
                    };
                    if let Some(end) = self.on_event(event, client)? {
                        return Ok(end);
                    }
                }
                Some(done) = self.done_rx.recv() => {
                    if let Some(end) = self.on_done(done, client).await? {
                        return Ok(end);
                    }
                }
                Some(control) = next_control(&mut self.controls) => {
                    if let Some(end) = self.on_control(control, client).await? {
                        return Ok(end);
                    }
                }
                () = tokio::time::sleep_until(wake.into()) => {}
            }
        }
    }

    /// Tells the hook the session is over, as far as the link still takes
    /// messages. A room left before its game began has nothing for the game
    /// to end: it stays as it was, the launcher's lobby link keeps going,
    /// and the game can follow the player into the next room.
    pub fn end(&mut self, reason: &str) {
        if let Some(fetch) = self.fetch.take() {
            fetch.abort();
        }
        if !self.begun {
            self.outbox
                .retain(|message| matches!(message, ToHook::Lobby(_)));
            let _ = self.flush();
            return;
        }
        self.outbox.push_back(ToHook::End {
            reason: Text::lossy(reason),
        });
        let _ = self.flush();
    }

    /// Keeps the hook waiting while the agent has no connection: without a
    /// beat it would give up on the agent.
    pub fn keep_alive(&mut self) {
        self.link.heartbeat();
    }

    /// Where to resume the room after reconnecting. `None` before the first
    /// turn stream, and while the world a stream starts from is still being
    /// fetched: the room then offers a world again.
    pub fn resume_point(&self) -> Option<Resume> {
        if matches!(self.world, World::Fetching { .. }) {
            return None;
        }
        self.follower.as_ref().map(TurnFollower::resume_point)
    }

    fn check_hook(&mut self, now: Instant) -> Result<(), BridgeFault> {
        let beat = self.link.peer_heartbeat();
        let limit = if self.loaded {
            self.options.hook_timeout
        } else {
            self.options.load_timeout
        };
        if beat != self.hook_beat.0 {
            self.hook_beat = (beat, now);
        } else if self.hook_ready && now.saturating_duration_since(self.hook_beat.1) > limit {
            return Err(BridgeFault::HookGone);
        }
        Ok(())
    }

    /// The hook's next message: those held while rejoining first.
    fn next_from_hook(&mut self) -> Result<Option<ToAgent>, BridgeFault> {
        if let Some(held) = self.held.pop_front() {
            return Ok(Some(held?));
        }
        if !self.link.recv(&mut self.buf)? {
            return Ok(None);
        }
        Ok(Some(decode(&self.buf)?))
    }

    /// While rejoining: keeps the hook waiting, shows it the launcher's
    /// lobby, passes the game window's actions to the launcher (a Leave
    /// among them, which ends the rejoining), and holds everything else
    /// the hook says for when the room is back.
    fn away(&mut self) {
        self.link.heartbeat();
        while self.held.len() < MAX_HELD {
            match self.link.recv(&mut self.buf) {
                Ok(true) => {}
                Ok(false) => break,
                Err(error) => {
                    debug!(%error, "reading the hook while rejoining failed");
                    break;
                }
            }
            match decode::<ToAgent>(&self.buf) {
                Ok(ToAgent::Lobby(action)) if self.hook_ready => self.lobby_action(action),
                other => self.held.push_back(other),
            }
        }
        self.lobby_news();
        if let Err(error) = self.flush() {
            debug!(%error, "writing to the hook while rejoining failed");
        }
    }

    async fn read_hook(&mut self, client: &Client) -> Result<(), BridgeFault> {
        while let Some(message) = self.next_from_hook()? {
            if !self.hook_ready && !matches!(message, ToAgent::Hello { .. }) {
                return Err(BridgeFault::Unexpected("a message before its hello"));
            }
            // What the game reports of a world that is being replaced
            // describes nothing the room plays.
            let current = self.world == World::Ready;
            match message {
                ToAgent::Hello { version, build } => {
                    if self.hook_ready {
                        return Err(BridgeFault::Unexpected("a second hello"));
                    }
                    check_version(version)?;
                    info!(%build, "the game's hook attached");
                    self.hook_ready = true;
                    self.build = Some(build.as_str().to_owned());
                    self.status(|status| status.game = Some(build.as_str().to_owned()));
                    // The hello goes first, ahead of a game that began
                    // before the hook attached.
                    self.outbox.push_front(ToHook::Hello {
                        version: tpf3mp_bridge::BRIDGE_VERSION,
                    });
                }
                ToAgent::Loaded {
                    next_step,
                    world_epoch,
                } => {
                    // The hook finishes loads in order. A previous world's
                    // answer may arrive after rejoining ordered a replacement.
                    if !self.current_load_ack(next_step, world_epoch)? {
                        continue;
                    }
                    self.world = World::Ready;
                    self.status(|status| status.world = WorldStatus::Playing);
                    // Loaded counts as progress: the server holds the room
                    // until every member has loaded.
                    let progress = next_step.saturating_sub(1);
                    self.last_loaded = Some((world_epoch, progress));
                    self.progress = Some(progress);
                    self.reported = None;
                    self.loaded = true;
                    client.report_loaded(world_epoch, progress).await?;
                    self.reported = Some(progress);
                }
                ToAgent::Command { payload, secret } => {
                    client
                        .send_intent_with(self.commands, payload, secret)
                        .await?;
                    self.commands += 1;
                }
                ToAgent::Ran { step, world_epoch }
                    if current && world_epoch == self.world_epoch =>
                {
                    self.progress = Some(step);
                    self.status(|status| status.step = Some(step));
                }
                ToAgent::Checkpoint {
                    step,
                    lanes,
                    world_epoch,
                } if current && world_epoch == self.world_epoch => {
                    client.report_checkpoint(world_epoch, step, lanes).await?;
                }
                ToAgent::Saved {
                    event,
                    lanes,
                    file,
                    world_epoch,
                } if current && world_epoch == self.world_epoch => {
                    self.saved_world(event, lanes, file, client).await?;
                }
                ToAgent::Ran { .. } | ToAgent::Checkpoint { .. } | ToAgent::Saved { .. } => {}
                ToAgent::Chat { text } => self.request(client, Request::Chat(text)),
                // The game's speed row: the room's owner sets the room's
                // speed from it; anyone else's is refused, as a notice.
                ToAgent::Speed { speed } => self.request(client, Request::SetSpeed(speed)),
                ToAgent::WorldUp { world } => self.world_up(world, client),
                ToAgent::MenuUp { menu } => self.menu_up(menu, client),
                ToAgent::Log { message } => info!(hook = %message),
                ToAgent::Lobby(action) => self.lobby_action(action),
                // Advisory and over the size the room relays: not shown.
                ToAgent::Preview { preview }
                    if current && preview.as_ref().is_none_or(|p| p.len() <= MAX_PREVIEW) =>
                {
                    client.send_preview(preview).await?;
                }
                ToAgent::Preview { .. } => {}
            }
        }
        Ok(())
    }

    /// The game has its world number `world` up, with the mod linked: in
    /// the room's lobby, the player is marked ready, as by the Ready button.
    /// Once a world: a player who then says Not ready stays so until another
    /// world is up. Never once the room's game began, nor while its world
    /// is being replaced; while the room's phase is not known yet, it waits
    /// for the room's announcement (fail closed).
    fn world_up(&mut self, world: u64, client: &Client) {
        if world <= self.world_up {
            debug!(world, "a world already told up");
            return;
        }
        self.world_up = world;
        self.ready_for_world(client);
    }

    /// Marks the player ready for the latest world up, if not done for it
    /// and the room is in its lobby. See [`Bridge::world_up`].
    fn ready_for_world(&mut self, client: &Client) {
        let world = self.world_up;
        if world <= self.readied || self.start_world_on_its_way(client) {
            return;
        }
        let lobby = match self.room_phase {
            // Not known yet: the room's announcement decides.
            None if !self.begun => return,
            Some(RoomPhase::Lobby) => !self.begun && self.world == World::Ready,
            _ => false,
        };
        // Decided for this world, either way.
        self.readied = world;
        if !lobby {
            debug!(world, "the game's world is up outside the room's lobby");
            return;
        }
        info!(world, "the game's world is up with the mod linked: ready");
        self.status(|status| {
            status.notice("your game has its world up: you are marked ready");
        });
        self.request(client, Request::SetReady(true));
    }

    fn start_generated_world(&mut self, client: &Client) {
        if self.options.start_generated_world
            && generated_world_can_start(
                self.room.as_ref(),
                client.player(),
                self.world_up,
                self.begun,
            )
        {
            self.options.start_generated_world = false;
            self.status(|status| status.notice("your new world is ready: starting multiplayer"));
            self.request(client, Request::StartGame);
        }
    }

    /// The game is at its main menu, arrived there for the `menu`th time,
    /// and can load the room's world from there: in the room's lobby, a
    /// player other than the room's owner is marked ready, as by the Ready
    /// button, once per arrival. So is the owner once the room has the save
    /// the owner handed over to start from ([`BridgeOptions::start_world`]):
    /// the owner's game then loads it from the menu as every other does.
    /// Without one the owner is not: the room's first world is the owner's,
    /// which their game saves for the room from a world it has up. Nor is
    /// anyone whose agent keeps no worlds, as it could not fetch the room's.
    /// Otherwise as [`Bridge::world_up`].
    fn menu_up(&mut self, menu: u64, client: &Client) {
        if menu <= self.menu_up {
            debug!(menu, "a menu arrival already told");
            return;
        }
        self.menu_up = menu;
        self.ready_at_menu(client);
    }

    /// Marks the player ready for the latest menu arrival, if not decided
    /// for it and the room is in its lobby. See [`Bridge::menu_up`].
    fn ready_at_menu(&mut self, client: &Client) {
        let menu = self.menu_up;
        if menu <= self.menu_readied || self.start_world_on_its_way(client) {
            return;
        }
        let lobby = match self.room_phase {
            None if !self.begun => return,
            Some(RoomPhase::Lobby) => !self.begun && self.world == World::Ready,
            _ => false,
        };
        // Decided for this arrival, either way.
        self.menu_readied = menu;
        if !lobby {
            debug!(menu, "the game is at its menu outside the room's lobby");
            return;
        }
        // The room's announcement names its owner along with its phase;
        // without one, nobody is marked (fail closed).
        let Some(owner) = self.room_owner else {
            return;
        };
        if owner == client.player() && !matches!(self.start, StartWorld::Uploaded(_)) {
            info!(
                menu,
                "the game is at its main menu, but the room plays its owner's world: load it to be ready"
            );
            self.status(|status| {
                status.notice("load the world the room will play: your game saves it for the room");
            });
            return;
        }
        if self.options.worlds.is_none() {
            debug!(
                menu,
                "the game is at its main menu, but this agent keeps no worlds to hand it the room's"
            );
            return;
        }
        info!(
            menu,
            "the game is at its main menu and loads the room's world when the game starts: ready"
        );
        self.status(|status| {
            status.notice(
                "your game waits at its main menu for the room's world: you are marked ready",
            );
        });
        self.request(client, Request::SetReady(true));
    }

    /// Whether this player owns the room and the save it starts from is
    /// still on its way there: readiness waits for it, undecided, since the
    /// room cannot start before it arrives.
    fn start_world_on_its_way(&self, client: &Client) -> bool {
        matches!(
            self.start,
            StartWorld::Named | StartWorld::Cutting | StartWorld::Told(_) | StartWorld::Clearing
        ) && self.room_owner == Some(client.player())
            && !self.begun
    }

    /// Hands the room the save it starts from, once in the room's lobby
    /// with this player as its owner: cut into the store off this task,
    /// then told to the room (see [`Bridge::on_done`]), which asks for it.
    /// Once the room's game began, the room plays what it has.
    fn hand_over_start_world(&mut self, client: &Client) {
        if self.start != StartWorld::Named {
            return;
        }
        if self.begun {
            self.start = StartWorld::None;
            return;
        }
        if self.room_phase != Some(RoomPhase::Lobby) || self.room_owner != Some(client.player()) {
            return;
        }
        let (Some(worlds), Some(file)) = (
            self.options.worlds.clone(),
            self.options.start_world.clone(),
        ) else {
            warn!("a save to start the room from, but this agent keeps no worlds to hand it over");
            self.start = StartWorld::None;
            return;
        };
        info!(file = %file.display(), "handing the room the save it starts from");
        let save = self.start_save_named();
        self.status(|status| {
            status.notice(format!(
                "the room starts from your save {}: handing it over",
                save.name
            ));
            status.start_upload = Some(StartUpload {
                save: save.name.as_str().to_owned(),
                percent: 0,
            });
        });
        self.start = StartWorld::Cutting;
        let attempt = self.start_attempt;
        let done = self.done_tx.clone();
        tokio::task::spawn_blocking(move || {
            // Read before the save is: one written over meanwhile is newer.
            let stamp = file_stamp(&file);
            let result = worlds
                .ingest_copy(&file)
                .map(|(manifest, world)| (manifest.id(), world, stamp))
                .map_err(|error| format!("cannot read the save {}: {error}", file.display()));
            let hold = result
                .as_ref()
                .ok()
                .map(|(id, _, _)| worlds.pending_result(*id));
            let _ = done.send(Done::StartCut {
                attempt,
                result,
                hold,
            });
        });
    }

    /// What the room shows of the save named to start from.
    fn start_save_named(&self) -> StartSave {
        self.options
            .start_save
            .clone()
            .unwrap_or_else(|| StartSave {
                name: Text::lossy(
                    self.options
                        .start_world
                        .as_deref()
                        .and_then(Path::file_stem)
                        .and_then(|stem| stem.to_str())
                        .unwrap_or_default(),
                ),
                map: Text::lossy(""),
                year: 0,
            })
    }

    /// The owner names another save for the room to start from, or none
    /// ([`Control::StartWorld`]): it replaces the one before, and the room
    /// asks everyone to get ready again. Only in the room's lobby, as its
    /// owner; the room refuses it otherwise too.
    fn change_start_world(
        &mut self,
        start: Option<(PathBuf, StartSave)>,
        declare: Option<Declaration>,
        client: &Client,
    ) {
        if self.begun || self.room_phase != Some(RoomPhase::Lobby) {
            self.status(|status| {
                status.notice("the room's game has begun: it plays the world it has");
            });
            return;
        }
        if self.room_owner != Some(client.player()) {
            self.status(|status| {
                status.notice("only the room's owner chooses the save it starts from");
            });
            return;
        }
        // The save the room has or receives, unchanged on disk, named again:
        // only what the room shows of it changes, such as the map and year
        // the owner's game read once the room was made.
        if let Some((file, save)) = &start
            && let Some((told, stamp)) = &self.start_told
            && matches!(self.start, StartWorld::Told(_) | StartWorld::Uploaded(_))
            && self.options.start_world.as_deref() == Some(file.as_path())
            && file_stamp(file).as_ref() == Some(stamp)
        {
            info!(file = %file.display(), "the owner describes the save the room starts from");
            self.options.start_save = Some(save.clone());
            // Its mods or their settings picked anew: the room takes them
            // before what it shows of the save, in that order.
            let mut requests = Vec::new();
            if let Some(declaration) = declare {
                self.declared = Some(declaration.clone());
                requests.push(declaration.request());
            }
            requests.push(Request::StartWorld {
                world: *told,
                save: save.clone(),
            });
            self.requests_in_order(client, requests);
            return;
        }
        self.options.start_generated_world = start.is_none();
        self.start_attempt += 1;
        self.start_told = None;
        // Readiness is decided afresh once the room has the new world.
        self.readied = self.world_up.saturating_sub(1);
        self.menu_readied = self.menu_up.saturating_sub(1);
        match start {
            Some((file, save)) => {
                info!(file = %file.display(), "the owner names another save for the room to start from");
                self.options.start_world = Some(file);
                self.options.start_save = Some(save);
                self.start_declare = declare;
                self.start = StartWorld::Named;
                self.hand_over_start_world(client);
            }
            None => {
                info!("the owner takes back the save the room starts from");
                self.options.start_world = None;
                self.options.start_save = None;
                // Readiness waits until the room took it back.
                self.start = StartWorld::Clearing;
                self.start_kept = None;
                self.start_held = None;
                self.status(|status| {
                    status.start_upload = None;
                    status.notice("the room starts from the world your game has: load it");
                });
                let requests = client.requests();
                let done = self.done_tx.clone();
                let status = self.options.status.clone();
                let attempt = self.start_attempt;
                tokio::spawn(async move {
                    let declared = match declare {
                        Some(declaration) => requests.done(declaration.request()).await,
                        None => Ok(()),
                    };
                    match declared.and(requests.done(Request::ClearStartWorld).await) {
                        Ok(()) => {
                            let _ = done.send(Done::StartCleared { attempt });
                        }
                        Err(error) => {
                            let _ = done.send(Done::StartCleared { attempt });
                            if let Some(status) = status {
                                status
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner)
                                    .notice(error.to_string());
                            }
                        }
                    }
                });
            }
        }
    }

    /// The room's start world as the room shows it changed: a player other
    /// than the owner is told, and asked to get ready again when it replaced
    /// another, as the room asks.
    fn start_news(&self, before: Option<&RoomView>, now: &RoomView, client: &Client) {
        let Some(before) = before else {
            return;
        };
        if now.owner == client.player() || now.phase != RoomPhase::Lobby {
            return;
        }
        let name = |room: &RoomView| {
            room.start
                .as_ref()
                .map(|start| start.save.name.as_str().to_owned())
        };
        let (was, is) = (name(before), name(now));
        if was == is {
            return;
        }
        // A save in place of another asks everyone to agree again; the first
        // is the world the room was waiting for.
        let notice = match (was, is) {
            (Some(_), Some(save)) => format!(
                "the owner changed the save the room starts from to {save}: press Ready once you agree"
            ),
            (Some(_), None) => {
                "the room now starts from the world the owner's game has: press Ready once you agree"
                    .to_owned()
            }
            (None, Some(save)) => format!("the room starts from the owner's save {save}"),
            (None, None) => return,
        };
        info!(save = ?name(now), "the room's start save changed");
        self.status(|status| status.notice(notice));
    }

    /// The game saved its world at a save event: cut the save into the
    /// store off this task, then report it (see [`Bridge::on_done`]).
    async fn saved_world(
        &mut self,
        event: u64,
        lanes: Vec<LaneDigest>,
        file: Option<Text<MAX_PATH>>,
        client: &Client,
    ) -> Result<(), BridgeFault> {
        let file = file.map(|file| PathBuf::from(file.as_str()));
        let (Some(worlds), Some(file)) = (self.options.worlds.clone(), file) else {
            client
                .report_saved(self.world_epoch, event, lanes, None)
                .await?;
            return Ok(());
        };
        let done = self.done_tx.clone();
        let generation = self.load_generation;
        let world_epoch = self.world_epoch;
        tokio::task::spawn_blocking(move || {
            // The hook saves where it was told. Any other file it names is
            // the player's, not the agent's to take in and delete.
            let result = if within(&file, worlds.saves()) {
                worlds
                    .ingest(&file)
                    .map(|(manifest, world)| (manifest.id(), world))
                    .map_err(|error| format!("cannot keep the save {}: {error}", file.display()))
            } else {
                Err(format!(
                    "the game reported a save outside {}: not taken",
                    worlds.saves().display()
                ))
            };
            let hold = result
                .as_ref()
                .ok()
                .map(|(id, _)| worlds.pending_result(*id));
            let _ = done.send(Done::Ingested {
                generation,
                world_epoch,
                event,
                lanes,
                result,
                hold,
            });
        });
        Ok(())
    }

    /// Reports how far the game has run, at most every `progress_every`,
    /// and at once on a new connection, which knows nothing yet.
    async fn report_progress(&mut self, client: &Client, now: Instant) -> Result<(), BridgeFault> {
        let Some(progress) = self.progress else {
            return Ok(());
        };
        let Some(reported) = self.reported else {
            if let Some((epoch, step)) = self.last_loaded {
                client.report_loaded(epoch, step).await?;
            }
            client.report_progress(self.world_epoch, progress).await?;
            self.reported = Some(progress);
            self.last_report = now;
            return Ok(());
        };
        let due = now.saturating_duration_since(self.last_report) >= self.options.progress_every;
        if due && progress > reported {
            client.report_progress(self.world_epoch, progress).await?;
            self.reported = Some(progress);
            self.last_report = now;
        }
        Ok(())
    }

    /// Where this game is with the room's world while it comes in.
    fn loading_stage(&self) -> Option<LoadingStage> {
        match self.world {
            World::Ready => None,
            World::Fetching { .. } => Some(LoadingStage::Fetching {
                percent: self.fetch_percent.load(Ordering::Relaxed),
            }),
            World::Loading { .. } => Some(LoadingStage::Loading),
        }
    }

    /// Tells the room where this game is with its world when that changed:
    /// a new stage at once, a new percent of the same fetch at most every
    /// [`LOADING_EVERY`].
    async fn report_loading(&mut self, client: &Client, now: Instant) -> Result<(), BridgeFault> {
        let stage = self.loading_stage();
        if !loading_due(self.loading_reported, stage, now) {
            return Ok(());
        }
        client.report_loading(stage).await?;
        self.loading_reported = Some((stage, now));
        Ok(())
    }

    /// Handles one event from the server. Returns how the session ended, if
    /// it did.
    fn on_event(
        &mut self,
        event: ClientEvent,
        client: &Client,
    ) -> Result<Option<BridgeEnd>, BridgeFault> {
        // A held stream's game follows it, never before it: its turns and
        // what the game is asked. The room's view, its mods, chat, previews
        // and losing the connection are taken up at once (the room's owner
        // among them, before its mods are adopted).
        if self.held_stream.is_some()
            && matches!(
                event,
                ClientEvent::TurnStream(_)
                    | ClientEvent::Turn(_)
                    | ClientEvent::Diverged { .. }
                    | ClientEvent::Upload { .. }
                    | ClientEvent::IntentRejected { .. }
            )
        {
            self.held_bytes = self.held_bytes.saturating_add(held_weight(&event));
            if self.held_events.len() >= MAX_HELD_EVENTS || self.held_bytes > MAX_HELD_BYTES {
                return Err(BridgeFault::Unexpected(
                    "the room did not say its mods before its game's turns",
                ));
            }
            self.held_events.push_back(event);
            return Ok(None);
        }
        match event {
            ClientEvent::TurnStream(start) if !self.begun && !self.room_heard => {
                // A game joined while it runs: its stream and the room's
                // mods come on different streams; the game begins once both
                // are here.
                debug!("the room's mods first, then the game begins");
                self.held_stream = Some(start);
            }
            ClientEvent::TurnStream(start) => {
                // A new stream, perhaps on a new connection: tell it where
                // the game stands.
                self.reported = None;
                self.loading_reported = None;
                self.playout = Some(Playout::new(
                    start.steps_per_second,
                    self.options.playout_margin,
                    self.options.playout_memory,
                ));
                if !self.begun {
                    self.begun = true;
                    let saves = self.saves_dir();
                    self.outbox.push_back(ToHook::Begin {
                        room: start.room,
                        rules: start.rules.clone(),
                        steps_per_second: start.steps_per_second,
                        checkpoint_interval: start.checkpoint_interval,
                        saves: path_text(&saves)?,
                        player: client.player(),
                        mods: self.load_lists(),
                    });
                    if let Some(room) = &self.room {
                        self.outbox.push_back(ToHook::Room(room_info(room)));
                    }
                }
                let next_step = start.sealed_through.saturating_add(1);
                match (start.world, &mut self.follower) {
                    (Some(offer), _) => {
                        self.world_epoch = start.world_epoch;
                        self.follower = Some(TurnFollower::new(&start));
                        self.fetch_world(offer, next_step, client)?;
                    }
                    (None, None) => {
                        self.world_epoch = start.world_epoch;
                        // A new game: every player loads the world it starts
                        // from.
                        self.follower = Some(TurnFollower::new(&start));
                        self.order_load(None, next_step)?;
                    }
                    (None, Some(follower)) => match follower.restart(&start) {
                        Ok(()) => {}
                        // The game from its first turn, from a server that
                        // keeps no worlds: start over from the first world.
                        Err(_) if start.next_event == 1 && start.sealed_through == 0 => {
                            self.world_epoch = start.world_epoch;
                            self.follower = Some(TurnFollower::new(&start));
                            self.order_load(None, next_step)?;
                        }
                        Err(error) => return Err(error.into()),
                    },
                }
            }
            ClientEvent::Turn(turn) => {
                let follower = self
                    .follower
                    .as_mut()
                    .ok_or(BridgeFault::Unexpected("a turn before its stream"))?;
                follower.accept(turn)?;
                if let Some(playout) = &mut self.playout {
                    playout.on_turn(follower.sealed_through(), follower.speed(), Instant::now());
                }
                if let Some(speed) = speed_news(&mut self.speed, follower.speed()) {
                    self.outbox.push_back(ToHook::Speed(speed));
                    self.status(|status| status.speed = speed);
                }
            }
            ClientEvent::IntentRejected { client_seq, reason } => {
                self.outbox.push_back(ToHook::Refused {
                    command: client_seq,
                    reason,
                });
                self.status(|status| {
                    status.notice(format!("the room refused an action: {reason:?}"))
                });
            }
            ClientEvent::Diverged { step, lanes } => {
                self.status(|status| {
                    status.notice(format!(
                        "your world differed from the room's at step {step}; the room's replaces it"
                    ));
                });
                self.outbox.push_back(ToHook::Diverged { step, lanes });
            }
            ClientEvent::Upload { event, snapshot } => self.upload(event, snapshot, client),
            ClientEvent::Preview { from, preview } => {
                // Only to a game that plays the room's world, and only the
                // latest of each member: one waiting is replaced.
                if self.begun && self.world == World::Ready {
                    self.outbox.preview(from, preview);
                }
            }
            ClientEvent::Chat { from, text } => {
                // The game hears chat once its session began; the front end
                // hears all of it.
                if self.begun {
                    let name = self.name_of(&from);
                    self.outbox.push_back(ToHook::Chat {
                        from: name,
                        text: text.clone(),
                    });
                }
                self.status(|status| push_bounded(&mut status.chat, (from, text)));
            }
            ClientEvent::RoomUpdate(room) => {
                // The game's Multiplayer window shows the room once the game
                // began.
                if self.begun {
                    self.outbox.push_back(ToHook::Room(room_info(&room)));
                }
                self.start_news(self.room.as_ref(), &room, client);
                // The room gave up on the save on its way (it did not start
                // arriving in time): the owner may pick it again.
                if matches!(self.start, StartWorld::Told(_))
                    && room.owner == client.player()
                    && room.phase == RoomPhase::Lobby
                    && room.start.is_none()
                    && self.room.as_ref().is_some_and(|before| {
                        before
                            .start
                            .as_ref()
                            .is_some_and(|start| start.save.name == self.start_save_named().name)
                    })
                {
                    self.start_failed("the room gave up waiting for it", client);
                }
                self.room = Some(room.clone());
                self.room_phase = Some(room.phase);
                self.room_owner = Some(room.owner);
                self.ready_for_world(client);
                self.ready_at_menu(client);
                self.status(|status| status.room = Some(room));
            }
            ClientEvent::Notice(text) => self.status(|status| status.announce(text.as_str())),
            ClientEvent::ContentDiff(diff) => {
                // What the room says this game lacks names the room's shared
                // mods: the picker declares those this player has.
                let again = diff
                    .as_ref()
                    .zip(self.options.picker.as_ref())
                    .and_then(|(diff, picker)| (picker.learn)(diff));
                if let Some(declaration) = again {
                    info!(
                        mods = declaration.manifest().mods.len(),
                        "declaring the room's shared mods this game has"
                    );
                    self.declared = Some(declaration.clone());
                    self.request(client, declaration.request());
                }
                self.status(|status| {
                    if let Some(diff) = &diff {
                        status.notice(format!("your game differs from the room's: {diff}"));
                    }
                    status.content_diff = diff;
                });
            }
            ClientEvent::RoomMods(room) => {
                // The room's mods as its owner declared them: the picker
                // declares those this player has, in the room's order.
                let again = self.options.picker.as_ref().and_then(|picker| {
                    // An owner not known yet takes no one's room away.
                    let owns = self.room_owner.is_none_or(|owner| owner == client.player());
                    (picker.adopt)(room.as_deref(), owns)
                });
                if let Some(declaration) = again {
                    info!(
                        mods = declaration.manifest().mods.len(),
                        "declaring the room's mods this game has"
                    );
                    self.declared = Some(declaration.clone());
                    self.request(client, declaration.request());
                }
                self.told.clone_from(&room);
                self.room_heard = true;
                self.status(|status| status.room_mods = room.map(|room| *room));
                if let Some(start) = self.held_stream.take() {
                    if let Some(end) = self.on_event(ClientEvent::TurnStream(start), client)? {
                        return Ok(Some(end));
                    }
                    self.held_bytes = 0;
                    while let Some(event) = self.held_events.pop_front() {
                        if let Some(end) = self.on_event(event, client)? {
                            return Ok(Some(end));
                        }
                    }
                }
            }
            ClientEvent::Kicked => return Ok(Some(BridgeEnd::Kicked)),
            ClientEvent::Closed(reason) => return Ok(Some(BridgeEnd::Closed(reason))),
        }
        Ok(None)
    }

    /// Handles work that finished off the bridge's task.
    async fn on_done(
        &mut self,
        done: Done,
        client: &Client,
    ) -> Result<Option<BridgeEnd>, BridgeFault> {
        match done {
            Done::Ingested {
                generation,
                world_epoch,
                event,
                lanes,
                result,
                hold,
            } => {
                if generation != self.load_generation
                    || world_epoch != self.world_epoch
                    || self.world != World::Ready
                {
                    drop(hold);
                    self.tidy();
                    return Ok(None);
                }
                let world = match result {
                    Ok((id, world)) => {
                        self.saved.push_back(id);
                        while self.saved.len() > SAVES_KEPT {
                            self.saved.pop_front();
                        }
                        self.tidy();
                        Some(world)
                    }
                    Err(error) => {
                        warn!(%error, "a save of the game could not be kept");
                        None
                    }
                };
                drop(hold);
                client
                    .report_saved(world_epoch, event, lanes, world)
                    .await?;
            }
            Done::Fetched {
                attempt,
                result,
                hold,
            } => {
                let World::Fetching {
                    next_step,
                    attempt: current,
                    ..
                } = self.world
                else {
                    drop(hold);
                    self.tidy();
                    return Ok(None);
                };
                if attempt != current {
                    drop(hold);
                    self.tidy();
                    return Ok(None);
                }
                self.fetch = None;
                match result {
                    Ok((file, id)) => {
                        info!(file = %file.display(), "fetched the world to load");
                        self.received = Some(id);
                        self.tidy();
                        // A world loaded with the room's list runs TPF3-MP's
                        // mod whatever its save lists (`mods::plan`); one
                        // loaded with its own mods must list it.
                        if self.load_lists().is_none() {
                            check_world(&file)?;
                        }
                        self.order_load(Some(&file), next_step)?;
                    }
                    Err(error) => {
                        warn!(%error, "fetching the world to load failed");
                        return Ok(Some(BridgeEnd::WorldUnavailable));
                    }
                }
                drop(hold);
            }
            Done::Uploaded { snapshot, result } => {
                let starting = self.start == StartWorld::Told(snapshot);
                match result {
                    Ok(bytes) if starting => {
                        info!(%snapshot, bytes, "uploaded the save the room starts from; the game waits at its menu to load it with everyone");
                        self.start_arrived(snapshot, client);
                    }
                    Ok(bytes) => info!(%snapshot, bytes, "uploaded a save the room asked for"),
                    Err(error) if starting => {
                        self.start_failed(&format!("uploading it failed: {error}"), client);
                    }
                    Err(error) => warn!(%snapshot, %error, "uploading a save failed"),
                }
            }
            Done::StartCut {
                attempt,
                result,
                hold,
            } => {
                if self.start != StartWorld::Cutting || attempt != self.start_attempt {
                    drop(hold);
                    self.tidy();
                    return Ok(None);
                }
                match result {
                    Ok((id, world, stamp)) => {
                        info!(snapshot = %world.snapshot, bytes = world.size, "told the room the save it starts from");
                        self.start_kept = Some(id);
                        self.start_told = stamp.map(|stamp| (world, stamp));
                        self.tidy();
                        self.start = StartWorld::Told(world.snapshot);
                        self.tell_start_world(client, world);
                        // The save the room has already, named again: the
                        // room only updates what it shows of it, and asks
                        // for nothing.
                        if self.start_held == Some(world.snapshot) {
                            info!(snapshot = %world.snapshot, "the room has this save already");
                            self.start_arrived(world.snapshot, client);
                        } else {
                            // The room lets go of the one it held.
                            self.start_held = None;
                        }
                    }
                    Err(error) => self.start_failed(&error, client),
                }
                drop(hold);
            }
            Done::StartRefused { attempt, error } => {
                if matches!(self.start, StartWorld::Told(_)) && attempt == self.start_attempt {
                    self.start_failed(&format!("the room refused it: {error}"), client);
                }
            }
            Done::StartCleared { attempt } => {
                if attempt == self.start_attempt && self.start == StartWorld::Clearing {
                    self.start = StartWorld::None;
                    // Readiness waited for the room to take it back.
                    self.ready_for_world(client);
                    self.ready_at_menu(client);
                }
            }
        }
        Ok(None)
    }

    /// The room has the save it starts from: every game loads it from its
    /// menu when the game starts, the owner's too, so the owner is ready
    /// as a guest is.
    fn start_arrived(&mut self, snapshot: SnapshotId, client: &Client) {
        self.start = StartWorld::Uploaded(snapshot);
        self.start_held = Some(snapshot);
        self.status(|status| {
            status.start_upload = None;
            status.notice(
                "the room has your save: every game loads it from its main menu when the game starts",
            );
        });
        self.ready_for_world(client);
        self.ready_at_menu(client);
    }

    /// Tells the room the save it starts from, on a task of its own: the
    /// room asks for it with [`ClientEvent::Upload`]. What this game
    /// declares for it goes first, on the same task, so the room compares
    /// everyone's mods with the new save's.
    fn tell_start_world(&mut self, client: &Client, world: SavedWorld) {
        let requests = client.requests();
        let done = self.done_tx.clone();
        let save = self.start_save_named();
        let declare = self.start_declare.take();
        if let Some(declaration) = &declare {
            self.declared = Some(declaration.clone());
        }
        let attempt = self.start_attempt;
        tokio::spawn(async move {
            let declared = match declare {
                Some(declaration) => requests.done(declaration.request()).await,
                None => Ok(()),
            };
            let told = match declared {
                Ok(()) => requests.done(Request::StartWorld { world, save }).await,
                Err(error) => Err(error),
            };
            if let Err(error) = told {
                let _ = done.send(Done::StartRefused {
                    attempt,
                    error: error.to_string(),
                });
            }
        });
    }

    /// Handing over the save the room starts from failed: the room starts
    /// as without one, from the owner's world up.
    fn start_failed(&mut self, why: &str, client: &Client) {
        warn!(reason = why, "the room cannot start from the save named");
        self.start = StartWorld::None;
        self.start_held = None;
        self.start_told = None;
        self.status(|status| {
            status.start_upload = None;
            status.notice(format!(
                "the room cannot start from your save ({why}); load the world the room will play instead"
            ));
        });
        // Readiness waited for the save; it is decided as without one now.
        self.ready_for_world(client);
        self.ready_at_menu(client);
    }

    /// Starts fetching the world a stream starts from. Whatever the game
    /// was sent for the world it replaces is void.
    fn fetch_world(
        &mut self,
        offer: WorldOffer,
        next_step: u64,
        client: &Client,
    ) -> Result<(), BridgeFault> {
        let worlds = self.options.worlds.clone().ok_or(BridgeFault::NoWorlds)?;
        self.void_world();
        self.attempts += 1;
        let attempt = self.attempts;
        self.world = World::Fetching {
            offer,
            next_step,
            attempt,
        };
        info!(snapshot = %offer.snapshot, bytes = offer.size, "fetching the world to load");
        let total = offer.size;
        self.status(|status| status.world = WorldStatus::Fetching { bytes: 0, total });
        self.fetch_percent.store(0, Ordering::Relaxed);
        let percent = Arc::clone(&self.fetch_percent);
        let opener = client.bulk();
        let done = self.done_tx.clone();
        let status = self.options.status.clone();
        let task = tokio::spawn(async move {
            let progress = |progress: tpf3mp_snapshot::Progress| {
                percent.store(
                    percent_of(progress.bytes_present, progress.bytes_total),
                    Ordering::Relaxed,
                );
                if let Some(status) = &status {
                    let mut status = status.lock().unwrap_or_else(PoisonError::into_inner);
                    status.world = WorldStatus::Fetching {
                        bytes: progress.bytes_present,
                        total: progress.bytes_total,
                    };
                }
            };
            let result = transfer::fetch_world(&opener, &worlds, offer, progress)
                .await
                .map(|(file, manifest)| (file, manifest.id()))
                .map_err(|error| error.to_string());
            let hold = result
                .as_ref()
                .ok()
                .map(|(_, id)| worlds.pending_result(*id));
            let _ = done.send(Done::Fetched {
                attempt,
                result,
                hold,
            });
        });
        self.fetch = Some(task.abort_handle());
        Ok(())
    }

    /// Has the game load a world, then run `next_step`.
    fn order_load(&mut self, file: Option<&Path>, next_step: u64) -> Result<(), BridgeFault> {
        let file = file.map(path_text).transpose()?;
        self.void_world();
        self.load_generation += 1;
        self.outbox.push_back(ToHook::Load {
            file,
            next_step,
            world_epoch: self.world_epoch,
        });
        self.world = World::Loading { next_step };
        self.status(|status| status.world = WorldStatus::Loading);
        Ok(())
    }

    /// Carries out a front end's request. Requests go out on a task of their
    /// own, so a round trip never holds up the game; leaving ends the
    /// session once the room has let the player go, and the game closing
    /// ends it as a fault.
    async fn on_control(
        &mut self,
        control: Control,
        client: &Client,
    ) -> Result<Option<BridgeEnd>, BridgeFault> {
        let request = match control {
            Control::Ready(ready) => Request::SetReady(ready),
            Control::Start => Request::StartGame,
            Control::Speed(speed) => Request::SetSpeed(speed),
            Control::Kick(player) => Request::Kick(player),
            Control::Chat(text) => Request::Chat(text),
            Control::Banner(banner) => Request::SetBanner(banner),
            Control::Declare(declaration) => {
                self.declared = Some(declaration.clone());
                declaration.request()
            }
            Control::StartWorld { start, declare } => {
                self.change_start_world(start, declare, client);
                return Ok(None);
            }
            Control::Leave => {
                // Never held up by a server that does not answer: the
                // player leaves either way, and a seat the server could not
                // be told about is let go after the room's grace period.
                match tokio::time::timeout(LEAVE_WAIT, client.leave_room()).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        debug!(%error, "leaving the room failed; ending the session anyway");
                    }
                    Err(_) => debug!("the server did not answer the leave; ending the session"),
                }
                return Ok(Some(BridgeEnd::Left));
            }
            // A game whose hook attached is the one this session plays
            // through: without it the session cannot go on, and a new game's
            // hook could not attach to it.
            Control::GameClosed if self.hook_ready => return Err(BridgeFault::GameClosed),
            Control::GameClosed => {
                info!("the game closed before its hook attached; waiting for the next one");
                return Ok(None);
            }
        };
        self.request(client, request);
        Ok(None)
    }

    /// Sends a request on a task of its own; a refusal becomes a notice.
    /// Sends `requests` one after the other, each once the one before was
    /// answered; the first refused stops the rest.
    fn requests_in_order(&self, client: &Client, requests: Vec<Request>) {
        let sender = client.requests();
        let status = self.options.status.clone();
        tokio::spawn(async move {
            for request in requests {
                if let Err(error) = sender.done(request).await {
                    if let Some(status) = status {
                        status
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .notice(error.to_string());
                    }
                    return;
                }
            }
        });
    }

    fn request(&self, client: &Client, request: Request) {
        let requests = client.requests();
        let status = self.options.status.clone();
        tokio::spawn(async move {
            if let Err(error) = requests.done(request).await
                && let Some(status) = status
            {
                status
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .notice(error.to_string());
            }
        });
    }

    /// A member's name, as the room last announced it.
    fn name_of(&self, player: &PlayerId) -> Text<32> {
        let own = self.room.as_ref().and_then(|room| {
            room.members
                .iter()
                .find(|member| member.player == *player)
                .map(|member| member.name.clone())
        });
        if let Some(name) = own {
            return name;
        }
        let known = self.options.status.as_ref().and_then(|status| {
            let status = status.lock().unwrap_or_else(PoisonError::into_inner);
            let room = status.room.as_ref()?;
            let member = room
                .members
                .iter()
                .find(|member| member.player == *player)?;
            Some(member.name.clone())
        });
        known.unwrap_or_else(|| Text::lossy(&player.to_string()))
    }

    /// Forgets what the game was sent for its current world and how far it
    /// got: a new one replaces it.
    fn void_world(&mut self) {
        if let Some(fetch) = self.fetch.take() {
            fetch.abort();
        }
        self.outbox.retain(|message| {
            !matches!(
                message,
                ToHook::Apply(_) | ToHook::Release { .. } | ToHook::Load { .. }
            )
        });
        self.progress = None;
        self.loaded = false;
        self.last_loaded = None;
    }

    /// Consumes the next sent load's answer. Only the latest ordered world
    /// can become playable; an older one may finish while its replacement is
    /// being fetched or loaded, even when both begin at the same step.
    fn current_load_ack(&mut self, next_step: u64, world_epoch: u64) -> Result<bool, BridgeFault> {
        let (expected, generation, expected_epoch) = self
            .sent_loads
            .pop_front()
            .ok_or(BridgeFault::Unexpected("a world nobody ordered"))?;
        if next_step != expected || world_epoch != expected_epoch {
            return Err(BridgeFault::LoadedElsewhere {
                expected,
                got: next_step,
            });
        }
        Ok(generation == self.load_generation
            && matches!(self.world, World::Loading { next_step: step } if step == next_step))
    }

    /// Uploads a save the room asked for.
    fn upload(&mut self, event: u64, snapshot: SnapshotId, client: &Client) {
        let Some(worlds) = self.options.worlds.clone() else {
            warn!(
                event,
                "the room asked for a save, but this agent keeps no worlds"
            );
            return;
        };
        let opener = client.bulk();
        let done = self.done_tx.clone();
        // The save the room starts from: how far it went up shows on the
        // owner's room page, where Start waits for it.
        let status = self
            .options
            .status
            .clone()
            .filter(|_| event == 0 && self.start == StartWorld::Told(snapshot));
        let name = self.start_save_named().name;
        tokio::spawn(async move {
            // Only while this save is the one on its way: one the owner
            // replaced may still be going up.
            let progress = |percent: u8| {
                if let Some(status) = &status
                    && let Some(upload) = &mut status
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .start_upload
                    && upload.save == name.as_str()
                {
                    upload.percent = percent;
                }
            };
            let result = transfer::upload_world_with_progress(&opener, &worlds, snapshot, progress)
                .await
                .map(|served| served.bytes)
                .map_err(|error| error.to_string());
            let _ = done.send(Done::Uploaded { snapshot, result });
        });
    }

    /// Keeps only the snapshots still useful: the game's newest saves and
    /// the world last received. Off the bridge's task.
    fn tidy(&self) {
        let Some(worlds) = self.options.worlds.clone() else {
            return;
        };
        let keep: Vec<ManifestId> = self
            .saved
            .iter()
            .copied()
            .chain(self.received)
            .chain(self.start_kept)
            .collect();
        worlds.set_keep(&keep);
        tokio::task::spawn_blocking(move || {
            if let Err(error) = worlds.collect() {
                debug!(%error, "cannot tidy the world store");
            }
        });
    }

    /// Where the game writes its saves.
    fn saves_dir(&self) -> PathBuf {
        match &self.options.worlds {
            Some(worlds) => worlds.saves().to_owned(),
            None => std::env::temp_dir().join("tpf3mp-saves"),
        }
    }

    /// Sends what the hook will take now, in order. Nothing goes out before
    /// the hook's hello.
    fn flush(&mut self) -> Result<(), BridgeFault> {
        if !self.hook_ready {
            return Ok(());
        }
        while let Some(message) = self.outbox.front() {
            let bytes = encode(message)?;
            if !self.link.send(&bytes)? {
                break;
            }
            if let ToHook::Load {
                next_step,
                world_epoch,
                ..
            } = message
            {
                self.sent_loads
                    .push_back((*next_step, self.load_generation, *world_epoch));
            }
            self.outbox.pop_front();
        }
        Ok(())
    }
}

/// Whether `file` is a file inside `dir`, links followed.
fn within(file: &Path, dir: &Path) -> bool {
    match (file.canonicalize(), dir.canonicalize()) {
        (Ok(file), Ok(dir)) => file != dir && file.starts_with(dir),
        _ => false,
    }
}

/// Messages waiting for the hook, and about how many bytes they hold.
///
/// A hook that does not read, as before it attaches or while the game loads
/// a world, must not make the agent hold whatever the server sends. Past
/// [`OUTBOX_BYTES`] the bridge takes nothing more from the room until the
/// hook catches up: the turns wait in the client, whose own bound then holds
/// back the server's stream.
#[derive(Debug, Default)]
pub(crate) struct Outbox {
    messages: VecDeque<ToHook>,
    bytes: usize,
}

/// About how much the outbox holds before the bridge stops taking turns.
const OUTBOX_BYTES: usize = 16 << 20;

/// The speed to tell the hook after a turn at `now`: the turn's, unless
/// the hook was last told it. The first turn always tells it, so the game's
/// Multiplayer window shows a room at normal speed too.
fn speed_news(told: &mut Option<Speed>, now: Speed) -> Option<Speed> {
    if *told == Some(now) {
        return None;
    }
    *told = Some(now);
    Some(now)
}

/// The room as the game's Multiplayer window shows it: its name, owner and
/// members, at most as many as a room holds.
fn generated_world_can_start(
    room: Option<&RoomView>,
    player: PlayerId,
    world_up: u64,
    begun: bool,
) -> bool {
    !begun
        && world_up > 0
        && room.is_some_and(|room| {
            room.owner == player
                && room.phase == RoomPhase::Lobby
                && !room.members.is_empty()
                && room
                    .members
                    .iter()
                    .all(|member| member.connected && member.ready)
        })
}

fn room_info(room: &RoomView) -> RoomInfo {
    let members = room
        .members
        .iter()
        .take(usize::from(MAX_ROOM_MEMBERS))
        .map(|member| RoomMember {
            player: member.player,
            name: member.name.clone(),
            connected: member.connected,
            banner: member
                .banner
                .clone()
                .filter(|id| crate::portraits::shown(id.as_str())),
            loading: member.loading,
        })
        .collect();
    RoomInfo {
        name: room.name.clone(),
        owner: room.owner,
        members: BoundedVec::new(members).unwrap_or_default(),
    }
}

impl Outbox {
    /// About how many bytes `message` takes.
    fn weight(message: &ToHook) -> usize {
        let carried = match message {
            ToHook::Apply(Event {
                body: EventBody::Command { payload, .. },
                ..
            }) => payload.len(),
            ToHook::Load { file, .. } => file.as_ref().map_or(0, |file| file.as_str().len()),
            ToHook::Begin {
                rules, saves, mods, ..
            } => {
                rules.as_str().len()
                    + saves.as_str().len()
                    + mods
                        .as_ref()
                        .map_or(0, |m| (m.shared.len() + m.personal.len()) * 97)
            }
            ToHook::Diverged { lanes, .. } => lanes.len() * 2,
            ToHook::Chat { from, text } => from.as_str().len() + text.as_str().len(),
            ToHook::Preview { preview, .. } => preview.as_ref().map_or(0, Payload::len),
            ToHook::Room(room) => room.members.len() * 64,
            ToHook::Lobby(view) => {
                view.chat.len() * 320
                    + view.room.as_ref().map_or(0, |room| room.members.len() * 80)
                    + view.rules.len() * 240
                    + view.saves.len() * 72
            }
            ToHook::End { reason } => reason.as_str().len(),
            _ => 0,
        };
        carried + 64
    }

    fn push_back(&mut self, message: ToHook) {
        self.bytes += Self::weight(&message);
        self.messages.push_back(message);
    }

    fn push_front(&mut self, message: ToHook) {
        self.bytes += Self::weight(&message);
        self.messages.push_front(message);
    }

    fn front(&self) -> Option<&ToHook> {
        self.messages.front()
    }

    fn pop_front(&mut self) -> Option<ToHook> {
        let message = self.messages.pop_front()?;
        self.bytes = self.bytes.saturating_sub(Self::weight(&message));
        Some(message)
    }

    fn retain(&mut self, keep: impl FnMut(&ToHook) -> bool) {
        self.messages.retain(keep);
        self.bytes = self.messages.iter().map(Self::weight).sum();
    }

    fn is_full(&self) -> bool {
        self.bytes >= OUTBOX_BYTES
    }

    /// Queues `from`'s preview in place of one of theirs still waiting:
    /// only the latest counts, so the queue holds one a member at most.
    fn preview(&mut self, from: PlayerId, preview: Option<Payload>) {
        if self
            .messages
            .iter()
            .any(|queued| matches!(queued, ToHook::Preview { from: other, .. } if *other == from))
        {
            self.retain(
                |queued| !matches!(queued, ToHook::Preview { from: other, .. } if *other == from),
            );
        }
        self.push_back(ToHook::Preview { from, preview });
    }
}

/// Refuses a world to load that does not run TPF3-MP's mod (fail closed: it
/// would hold paused for good). A world whose mods do not read is loaded:
/// the hook's own plan of its mods says more (`tpf3mp-hook`, `plan_mods`).
fn check_world(file: &Path) -> Result<(), BridgeFault> {
    match crate::save_check::runs_own_mod(file) {
        Ok(true) => Ok(()),
        Ok(false) => {
            warn!(file = %file.display(), "the room's world does not run TPF3-MP's mod; not loading it");
            Err(BridgeFault::WorldWithoutOwnMod)
        }
        Err(why) => {
            debug!(%why, "cannot tell whether the room's world runs TPF3-MP's mod");
            Ok(())
        }
    }
}

fn path_text(path: &Path) -> Result<Text<MAX_PATH>, BridgeFault> {
    Text::new(path.to_string_lossy().into_owned())
        .map_err(|_| BridgeFault::PathTooLong(path.to_owned()))
}

/// Where to find the room again after the connection drops.
#[derive(Debug, Clone)]
pub struct Rejoin {
    pub options: ConnectOptions,
    pub invite: Invite,
    pub password: Option<Text<64>>,
    /// This player's game build and mods, declared on every new
    /// connection: a running game can only be joined afresh with them.
    pub content: Option<Declaration>,
    /// Stop trying after this long without a connection.
    pub give_up_after: Duration,
}

/// Plays the room through the hook until it ends. After a lost connection
/// (a network drop, or the server restarting) it reconnects and resumes
/// the room where the game stands, so the game sees only a pause. Tells
/// the hook when the session is over.
///
/// Rejoining gives up, and the session ends with the player in no room:
/// at once when the server no longer has the room
/// ([`BridgeFault::RoomGone`]); after [`Rejoin::give_up_after`] without a
/// connection that held; after [`MAX_QUICK_LOSSES`] connections in a row
/// lost again right after rejoining. A Leave while rejoining ends the
/// session at once, as left.
pub async fn play<L: HookLink>(
    bridge: &mut Bridge<L>,
    mut client: Client,
    mut events: Events,
    rejoin: &Rejoin,
) -> Result<BridgeEnd, BridgeFault> {
    let mut losses = Losses::new(rejoin.give_up_after, Instant::now());
    loop {
        let session = client.welcome().session_id;
        bridge.status(|status| status.session = Some(session));
        let ended = match bridge.run(&client, &mut events).await {
            // A request can find the connection gone before its closing
            // reaches the events: then it is the same loss.
            Err(BridgeFault::Client(
                error @ (ClientError::Disconnected | ClientError::Timeout),
            )) => match tokio::time::timeout(CLOSE_NOTICE, client.closed()).await {
                Ok(reason) => Ok(BridgeEnd::Closed(reason)),
                Err(_) => Err(BridgeFault::Client(error)),
            },
            other => other,
        };
        match ended {
            Ok(BridgeEnd::Closed(reason)) if worth_rejoining(&reason) => {
                warn!(%reason, "lost the server; rejoining the room");
                bridge.status(|status| status.notice("lost the server; rejoining the room"));
            }
            Ok(BridgeEnd::WorldUnavailable) => {
                warn!("the world to load was not available; rejoining the room for another");
            }
            Ok(end) => {
                bridge.end(&format!("{end:?}"));
                return Ok(end);
            }
            Err(fault) => {
                bridge.end(&fault.to_string());
                return Err(fault);
            }
        }
        let options = rejoin.options.again_after(&client);
        drop(client);
        let outcome = match losses.lost(Instant::now()) {
            Ok(deadline) => rejoin_room(bridge, rejoin, &options, deadline).await,
            Err(why) => Err(GaveUp::Failed(why)),
        };
        match outcome {
            Ok((new_client, new_events)) => {
                info!("rejoined the room");
                bridge.on_new_connection();
                bridge.status(|status| status.notice("rejoined the room"));
                losses.connected(Instant::now());
                client = new_client;
                events = new_events;
            }
            Err(GaveUp::Left) => {
                info!("left the room while rejoining it");
                bridge.end("left the room");
                return Ok(BridgeEnd::Left);
            }
            Err(GaveUp::GameClosed) => {
                let fault = BridgeFault::GameClosed;
                bridge.end(&fault.to_string());
                return Err(fault);
            }
            Err(GaveUp::RoomGone) => {
                warn!("the room is gone; no longer rejoining it");
                let fault = BridgeFault::RoomGone;
                bridge.status(|status| status.notice(ROOM_GONE));
                bridge.end(ROOM_GONE);
                return Err(fault);
            }
            Err(GaveUp::Failed(error)) => {
                warn!(%error, "no longer rejoining the room");
                let fault = BridgeFault::Rejoin(error);
                bridge.status(|status| status.notice(fault.to_string()));
                bridge.end(&fault.to_string());
                return Err(fault);
            }
        }
    }
}

/// How long a request that failed for a lost connection waits for the
/// connection to say it closed, before the failure counts as a fault.
const CLOSE_NOTICE: Duration = Duration::from_secs(5);

/// How long leaving waits for the server to let the player go.
const LEAVE_WAIT: Duration = Duration::from_secs(3);

/// A connection rejoined this long ago held: losing it starts afresh.
const STABLE: Duration = Duration::from_secs(120);

/// Connections in a row lost within [`STABLE`] of rejoining, after which
/// rejoining stops: a room that keeps dropping the player is not one to
/// keep them in, unable to leave (seen live: "lost the server; rejoining
/// the room reason=timed out" over and over after a restart).
pub const MAX_QUICK_LOSSES: u32 = 5;

/// The lost connections of one session, which say when rejoining stops.
#[derive(Debug)]
struct Losses {
    patience: Duration,
    /// When the current connection was made.
    connected: Instant,
    /// The first loss since a connection last held: the patience runs from
    /// there, across rejoins that did not hold.
    first: Option<Instant>,
    /// Connections in a row lost within [`STABLE`].
    quick: u32,
}

impl Losses {
    fn new(patience: Duration, connected: Instant) -> Self {
        Self {
            patience,
            connected,
            first: None,
            quick: 0,
        }
    }

    /// A connection is lost at `now`: when rejoining must have succeeded
    /// by, or why it is not worth trying.
    fn lost(&mut self, now: Instant) -> Result<Instant, String> {
        if now.saturating_duration_since(self.connected) >= STABLE {
            self.first = None;
            self.quick = 0;
        } else {
            self.quick += 1;
        }
        if self.quick > MAX_QUICK_LOSSES {
            return Err(format!(
                "the connection dropped {} times in a row right after rejoining",
                self.quick
            ));
        }
        let first = *self.first.get_or_insert(now);
        let deadline = first + self.patience;
        if deadline <= now {
            return Err(format!(
                "no connection held for {} s",
                self.patience.as_secs()
            ));
        }
        Ok(deadline)
    }

    /// A rejoin succeeded at `now`.
    fn connected(&mut self, now: Instant) {
        self.connected = now;
    }
}

/// Whether a lost connection is worth rejoining after: not when this side
/// closed it, another connection replaced it, or the protocol broke.
fn worth_rejoining(reason: &quinn::ConnectionError) -> bool {
    match reason {
        quinn::ConnectionError::LocallyClosed | quinn::ConnectionError::VersionMismatch => false,
        quinn::ConnectionError::ApplicationClosed(closed) => ![
            close::REPLACED,
            close::PROTOCOL_VIOLATION,
            close::VERSION_MISMATCH,
            close::IDLE,
        ]
        .contains(&closed.error_code),
        _ => true,
    }
}

/// Why rejoining stopped.
#[derive(Debug)]
enum GaveUp {
    /// The server has no such room any more.
    RoomGone,
    /// The player left the room meanwhile.
    Left,
    /// The game closed meanwhile.
    GameClosed,
    /// Out of patience, or the server needs a newer client.
    Failed(String),
}

/// Reconnects and rejoins, backing off between attempts and keeping the
/// hook waiting all the while, until `deadline`. A room that can no longer
/// resume the game where it stands is joined afresh, and sends a world to
/// load. The front end's requests are taken meanwhile: Leave (from the
/// launcher's window or, through the hook, the game's) stops it at once.
async fn rejoin_room<L: HookLink>(
    bridge: &mut Bridge<L>,
    rejoin: &Rejoin,
    options: &ConnectOptions,
    deadline: Instant,
) -> Result<(Client, Events), GaveUp> {
    let mut controls = bridge.controls.take();
    let outcome = rejoin_attempts(bridge, &mut controls, rejoin, options, deadline).await;
    bridge.controls = controls;
    outcome
}

async fn rejoin_attempts<L: HookLink>(
    bridge: &mut Bridge<L>,
    controls: &mut Option<mpsc::Receiver<Control>>,
    rejoin: &Rejoin,
    options: &ConnectOptions,
    deadline: Instant,
) -> Result<(Client, Events), GaveUp> {
    let mut backoff = Duration::from_millis(250);
    let mut resume = bridge.resume_point();
    let declared = bridge.declared.clone();
    loop {
        let attempt = async {
            let (client, events) = connect(options.clone()).await.map_err(|error| {
                if error.client_is_older() {
                    Failed::Outdated(error.for_player())
                } else {
                    Failed::Retry(error.to_string())
                }
            })?;
            if let Some(content) = declared.as_ref().or(rejoin.content.as_ref()) {
                client
                    .declare(content.clone())
                    .await
                    .map_err(|error| Failed::Retry(error.to_string()))?;
            }
            client
                .join_room(JoinRoom {
                    invite: rejoin.invite,
                    password: rejoin.password.clone(),
                    resume,
                })
                .await
                .map_err(|error| match error {
                    ClientError::Refused(RequestError::ResumeUnavailable) => {
                        Failed::ResumeGone(error.to_string())
                    }
                    // The invite found no room: unknown rooms answer as
                    // bad invites do (D13), and a seated player's own
                    // invite and password are never bad. A room that closed
                    // while being joined answers that it has no such member.
                    ClientError::Refused(RequestError::BadInvite | RequestError::NotInRoom) => {
                        Failed::Gone
                    }
                    other => Failed::Retry(other.to_string()),
                })?;
            Ok::<_, Failed>((client, events))
        };
        let outcome =
            match tokio::time::timeout_at(deadline.into(), away(bridge, controls, attempt)).await {
                Ok(Ok(outcome)) => outcome,
                Ok(Err(gave_up)) => return Err(gave_up),
                Err(_) => return Err(GaveUp::Failed("rejoin deadline expired".into())),
            };
        match outcome {
            Ok(rejoined) => return Ok(rejoined),
            Err(Failed::Gone) => return Err(GaveUp::RoomGone),
            // The server was updated past this client: no attempt can
            // succeed until the player updates too.
            Err(Failed::Outdated(error)) => {
                bridge.status(|status| status.outdated = true);
                return Err(GaveUp::Failed(error));
            }
            // The room no longer has these turns: join without them, for a
            // world to load. Joining without them cannot be refused so.
            Err(Failed::ResumeGone(error)) if resume.is_some() => {
                debug!(%error, "the room cannot resume here; joining afresh");
                resume = None;
            }
            Err(Failed::ResumeGone(error) | Failed::Retry(error))
                if Instant::now() + backoff >= deadline =>
            {
                return Err(GaveUp::Failed(error));
            }
            Err(Failed::ResumeGone(error) | Failed::Retry(error)) => {
                debug!(%error, "rejoining failed; trying again");
                away(bridge, controls, tokio::time::sleep(backoff)).await?;
                backoff = (backoff * 2).min(Duration::from_secs(5));
            }
        }
    }
}

/// Why one attempt to rejoin failed.
enum Failed {
    /// Worth trying again.
    Retry(String),
    /// The room no longer has the turns asked for.
    ResumeGone(String),
    /// The server has no such room.
    Gone,
    /// The server speaks a newer protocol.
    Outdated(String),
}

/// Runs `work` while the bridge has no connection ([`Bridge::away`]),
/// taking the front end's requests: a Leave, or the game closing, stops it.
async fn away<L: HookLink, T>(
    bridge: &mut Bridge<L>,
    controls: &mut Option<mpsc::Receiver<Control>>,
    work: impl std::future::Future<Output = T>,
) -> Result<T, GaveUp> {
    let mut work = std::pin::pin!(work);
    let mut beat = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {
            done = &mut work => return Ok(done),
            Some(control) = next_control(controls) => match control {
                Control::Leave => return Err(GaveUp::Left),
                Control::GameClosed if bridge.hook_ready => return Err(GaveUp::GameClosed),
                Control::GameClosed => {
                    info!("the game closed before its hook attached; waiting for the next one");
                }
                other => {
                    debug!(?other, "a request while rejoining the room; not sent");
                    bridge.status(|status| {
                        status.notice("not connected to the room: rejoining it, try again then");
                    });
                }
            },
            _ = beat.tick() => bridge.away(),
        }
    }
}

/// The next request of a front end, or never without one.
async fn next_control(controls: &mut Option<mpsc::Receiver<Control>>) -> Option<Control> {
    match controls {
        Some(controls) => controls.recv().await,
        None => std::future::pending().await,
    }
}

/// Moves what the follower allows, at the pace the playout sets, into
/// messages for the hook, in the order its gate requires: every event for
/// step `s` after the release of step `s - 1` and before the release of step
/// `s`. Steps with no events between them go out as one release. Returns
/// when the next step falls due, if one is sealed but not yet due.
pub(crate) fn pump(
    follower: &mut TurnFollower,
    playout: &mut Playout,
    now: Instant,
    out: &mut Outbox,
) -> Option<Instant> {
    let mut released = None;
    let wait = loop {
        if out.is_full() {
            // The hook is behind; the follower keeps the rest.
            break None;
        }
        if let Some(step) = follower.next_step() {
            let due = playout.due(step, now).unwrap_or(now);
            if due > now {
                break Some(due);
            }
            playout.played(step, due);
            follower.next_action();
            released = Some(step);
            continue;
        }
        match follower.next_action() {
            Some(Action::Apply(event)) => {
                if let Some(through) = released.take() {
                    out.push_back(ToHook::Release { through });
                }
                out.push_back(ToHook::Apply(event));
            }
            Some(Action::Execute(step)) => released = Some(step),
            None => break None,
        }
    };
    if let Some(through) = released {
        out.push_back(ToHook::Release { through });
    }
    wait
}

#[cfg(test)]
mod tests {
    use tpf3mp_proto::{FixedBytes, MAX_PAYLOAD, Payload, RoomId, Turn, TurnStart};

    use super::*;

    /// A game without the picker (`--mods`) loads the room's world with the
    /// room's mods and settings as its owner declared them, its own listed
    /// personal mods after them: the content check does not cover the
    /// settings, and the save's would differ from every other game's.
    #[test]
    fn without_the_picker_the_rooms_settings_still_load() {
        let name = |id: &str| tpf3mp_bridge::ModName::new(id).unwrap();
        let listed = ModLists {
            shared: tpf3mp_proto::BoundedVec::new(vec![name("signals"), name("tpf3mp_1")]).unwrap(),
            personal: tpf3mp_proto::BoundedVec::new(vec![name("minimap")]).unwrap(),
            params: Vec::new(),
        };
        let room_mod = |id: &str| tpf3mp_proto::RoomMod {
            id: Text::new(id).unwrap(),
            version: Text::new("1").unwrap(),
            info: tpf3mp_proto::ModInfo {
                name: Text::lossy(id),
                source: Text::lossy("mod.io"),
                modio: None,
            },
        };
        let settings = vec![tpf3mp_proto::ModParams {
            id: Text::new(tpf3mp_proto::GAME_SETTINGS).unwrap(),
            params: vec![tpf3mp_proto::ModParam {
                key: Text::new("difficulty").unwrap(),
                value: 2,
            }],
        }];
        let told = tpf3mp_proto::RoomMods {
            game: Text::lossy("40408"),
            mods: vec![room_mod("signals"), room_mod("tpf3mp_1")],
            params: settings.clone(),
        };
        let lists = told_lists(Some(&told), Some(&listed)).unwrap();
        assert_eq!(lists.shared, listed.shared);
        assert_eq!(lists.personal, listed.personal);
        assert_eq!(lists.params, settings);
        // A room that told none: the listed ones, as before.
        assert_eq!(told_lists(None, Some(&listed)), Some(listed));
    }

    /// Held before the room said its mods, a turn of many events without
    /// commands weighs as the follower weighs it: a hostile server cannot
    /// make this game hold gigabytes of empty events under the bound.
    #[test]
    fn held_turns_weigh_every_event() {
        let left = |seq| Event {
            seq,
            step: 1,
            body: EventBody::PlayerLeft {
                player: PlayerId(FixedBytes([7; 32])),
                kicked: false,
            },
        };
        let turn = Turn {
            number: 1,
            sealed_through: 0,
            speed: Speed::NORMAL,
            events: (0..10_000).map(left).collect(),
        };
        assert!(held_weight(&ClientEvent::Turn(turn)) >= 10_000 * 64);
        assert!(held_weight(&ClientEvent::Kicked) > 0);
    }

    async fn control_peer(
        count: usize,
    ) -> (
        Client,
        tokio::sync::oneshot::Receiver<Vec<tpf3mp_proto::ClientMessage>>,
        tokio::task::JoinHandle<()>,
    ) {
        use tpf3mp_net::{
            Identity, ServerIdentity, ServerTrust, read_message, read_preamble, server_config,
            write_message, write_preamble,
        };
        use tpf3mp_proto::{
            CONTROL_MAX_FRAME, ClientMessage, PROTOCOL_VERSION, ServerMessage, SessionId, Welcome,
        };
        let identity = ServerIdentity::self_signed(&["localhost"]).unwrap();
        let leaf = identity.leaf().clone();
        let endpoint = quinn::Endpoint::server(
            server_config(identity).unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        let address = endpoint.local_addr().unwrap();
        let (reported_tx, reported) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let connection = endpoint.accept().await.unwrap().await.unwrap();
            let (mut send, mut recv) = connection.accept_bi().await.unwrap();
            read_preamble(&mut recv).await.unwrap();
            write_preamble(&mut send, PROTOCOL_VERSION).await.unwrap();
            let _: ClientMessage = read_message(&mut recv, CONTROL_MAX_FRAME).await.unwrap();
            write_message(
                &mut send,
                &ServerMessage::Welcome(Welcome {
                    server_version: Text::new("test").unwrap(),
                    session_id: SessionId("AB2CD3".parse().unwrap()),
                    rules: Vec::new(),
                }),
                CONTROL_MAX_FRAME,
            )
            .await
            .unwrap();
            let mut messages = Vec::new();
            for _ in 0..count {
                messages.push(
                    read_message::<ClientMessage>(&mut recv, CONTROL_MAX_FRAME)
                        .await
                        .unwrap(),
                );
            }
            reported_tx.send(messages).unwrap();
        });
        let (client, _) = crate::connect(crate::ConnectOptions::new(
            address,
            "localhost",
            ServerTrust::Pinned(leaf),
            Arc::new(Identity::generate().unwrap().0),
            Text::new("test").unwrap(),
        ))
        .await
        .unwrap();
        (client, reported, server)
    }

    #[tokio::test]
    async fn delayed_ingest_from_a_replaced_world_never_reaches_the_server() {
        use tpf3mp_proto::{ClientMessage, GameMessage};
        let (client, reported, server) = control_peer(1).await;
        let mut bridge = Bridge::new(AcceptingLink, BridgeOptions::default()).greeted("test");
        let old_generation = bridge.load_generation;
        let old_epoch = bridge.world_epoch;
        let (release, wait) = std::sync::mpsc::channel();
        let done_tx = bridge.done_tx.clone();
        let ingest = tokio::task::spawn_blocking(move || {
            wait.recv().unwrap();
            done_tx
                .send(Done::Ingested {
                    generation: old_generation,
                    world_epoch: old_epoch,
                    event: 12,
                    lanes: Vec::new(),
                    result: Err("save failed".into()),
                    hold: None,
                })
                .unwrap();
        });
        bridge.world_epoch = 2;
        bridge.order_load(None, 1).unwrap();
        bridge.flush().unwrap();
        assert!(bridge.current_load_ack(1, 2).unwrap());
        bridge.world = World::Ready;
        release.send(()).unwrap();
        ingest.await.unwrap();
        let done = bridge.done_rx.recv().await.unwrap();
        bridge.on_done(done, &client).await.unwrap();
        // An ordered marker makes absence observable without a timing sleep.
        client.report_loading(None).await.unwrap();
        let message = tokio::time::timeout(Duration::from_secs(10), reported)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                message.as_slice(),
                [ClientMessage::Game(GameMessage::Loading(None))]
            ),
            "stale ingest was sent before the marker: {message:?}"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn an_exact_resume_replays_loaded_before_its_current_progress() {
        use tpf3mp_proto::{ClientMessage, GameMessage};
        let (client, reported, server) = control_peer(2).await;
        let mut bridge = Bridge::new(AcceptingLink, BridgeOptions::default()).greeted("test");
        let mut stream = start();
        stream.sealed_through = 40;
        bridge.begun = true;
        bridge.follower = Some(TurnFollower::new(&stream));
        bridge.world = World::Ready;
        bridge.last_loaded = Some((1, 0));
        bridge.progress = Some(40);
        bridge.reported = Some(40);
        // The previous control stream could have lost Loaded after the hook
        // completed its first load. An exact resume must retry its original
        // frontier, not the forty steps the game has since run.
        bridge
            .on_event(ClientEvent::TurnStream(stream), &client)
            .unwrap();
        bridge
            .report_progress(&client, Instant::now())
            .await
            .unwrap();
        let messages = tokio::time::timeout(Duration::from_secs(10), reported)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                messages.as_slice(),
                [
                    ClientMessage::Game(GameMessage::Loaded {
                        world_epoch: 1,
                        step: 0
                    }),
                    ClientMessage::Game(GameMessage::Progress {
                        world_epoch: 1,
                        step: 40
                    }),
                ]
            ),
            "load acknowledgement must precede progress: {messages:?}"
        );
        bridge.order_load(None, 41).unwrap();
        assert!(
            bridge.last_loaded.is_none(),
            "a replacement forgets the old world's ACK"
        );
        server.await.unwrap();
    }

    #[test]
    fn ending_a_bridge_releases_queued_and_unsent_world_results() {
        let dir = tempfile::tempdir().unwrap();
        let worlds = Worlds::open(dir.path(), 1 << 20).unwrap();
        let file = worlds.saves().join("world.sav");
        std::fs::write(&file, [5u8; 1000]).unwrap();
        let (first, world) = worlds.ingest_copy(&file).unwrap();
        let (second, _) = worlds.ingest_copy(&file).unwrap();
        assert_eq!(
            first.id(),
            second.id(),
            "two independent results of one snapshot"
        );
        let received = worlds.received_file(&world.snapshot);
        std::fs::write(&received, [5u8; 1000]).unwrap();
        let bridge = Bridge::new(
            AcceptingLink,
            BridgeOptions {
                worlds: Some(worlds.clone()),
                ..BridgeOptions::default()
            },
        )
        .greeted("test");
        let sender = bridge.done_tx.clone();
        sender
            .send(Done::Ingested {
                generation: bridge.load_generation,
                world_epoch: bridge.world_epoch,
                event: 12,
                lanes: Vec::new(),
                result: Ok((first.id(), world)),
                hold: Some(worlds.pending_result(first.id())),
            })
            .unwrap();
        // Receiver destruction drops its queued completion, but only one hold.
        drop(bridge);
        worlds.collect().unwrap();
        assert!(worlds.store().manifest(&second.id()).is_ok());
        assert!(received.exists());
        // Disk work finishes after that receiver closed. Discarding the send
        // error drops its completion guard without needing bridge handling.
        assert!(sender.is_closed());
        let _ = sender.send(Done::StartCut {
            attempt: 1,
            result: Ok((second.id(), world, None)),
            hold: Some(worlds.pending_result(second.id())),
        });
        worlds.collect().unwrap();
        assert!(worlds.store().manifest(&second.id()).is_err());
        assert!(!received.exists());
    }

    struct AcceptingLink;

    impl HookLink for AcceptingLink {
        fn send(&mut self, _: &[u8]) -> Result<bool, BridgeFault> {
            Ok(true)
        }

        fn recv(&mut self, _: &mut Vec<u8>) -> Result<bool, BridgeFault> {
            Ok(false)
        }

        fn heartbeat(&mut self) {}

        fn peer_heartbeat(&self) -> u64 {
            0
        }
    }

    #[test]
    fn an_old_load_answer_cannot_complete_its_replacement_at_the_same_step() {
        let mut bridge = Bridge::new(AcceptingLink, BridgeOptions::default()).greeted("test");
        bridge.order_load(None, 1).unwrap();
        bridge.flush().unwrap();
        bridge.order_load(None, 1).unwrap();
        bridge.flush().unwrap();

        assert!(!bridge.current_load_ack(1, 1).unwrap());
        assert_eq!(bridge.world, World::Loading { next_step: 1 });
        assert!(bridge.current_load_ack(1, 1).unwrap());
    }

    #[test]
    fn a_replaced_load_answer_during_rejoin_is_ignored() {
        let mut bridge = Bridge::new(AcceptingLink, BridgeOptions::default()).greeted("test");
        bridge.order_load(None, 1).unwrap();
        bridge.flush().unwrap();
        bridge.void_world();
        bridge.world = World::Ready;

        assert!(!bridge.current_load_ack(1, 1).unwrap());
        assert!(matches!(
            bridge.current_load_ack(1, 1),
            Err(BridgeFault::Unexpected("a world nobody ordered"))
        ));
    }

    #[test]
    fn an_unsent_replaced_load_needs_no_answer() {
        let mut bridge = Bridge::new(AcceptingLink, BridgeOptions::default()).greeted("test");
        bridge.order_load(None, 1).unwrap();
        bridge.order_load(None, 1).unwrap();
        bridge.flush().unwrap();

        assert!(bridge.current_load_ack(1, 1).unwrap());
        assert!(matches!(
            bridge.current_load_ack(1, 1),
            Err(BridgeFault::Unexpected("a world nobody ordered"))
        ));
    }

    #[test]
    fn a_load_answer_for_the_wrong_step_is_still_a_protocol_error() {
        let mut bridge = Bridge::new(AcceptingLink, BridgeOptions::default()).greeted("test");
        bridge.order_load(None, 5).unwrap();
        bridge.flush().unwrap();

        assert!(matches!(
            bridge.current_load_ack(4, 1),
            Err(BridgeFault::LoadedElsewhere {
                expected: 5,
                got: 4
            })
        ));
    }

    #[test]
    fn a_world_without_tpf3mps_mod_is_not_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let without = dir.path().join("w-1.sav");
        crate::save_check::saves::write(&without, &["urbangames_preorder_pack"]);
        let refused = check_world(&without).unwrap_err();
        assert!(matches!(refused, BridgeFault::WorldWithoutOwnMod));
        assert!(
            refused
                .to_string()
                .starts_with("The room's world doesn't have the TPF3-MP mod enabled")
        );
        let with = dir.path().join("w-2.sav");
        crate::save_check::saves::write(&with, &["urbangames_preorder_pack", "tpf3mp_1"]);
        assert!(check_world(&with).is_ok());
        // One whose mods do not read loads as before.
        let junk = dir.path().join("w-3.sav");
        std::fs::write(&junk, b"not a save").unwrap();
        assert!(check_world(&junk).is_ok());
    }

    #[test]
    fn rejoining_stops_after_repeated_quick_losses_or_its_patience() {
        let patience = Duration::from_secs(300);
        let start = Instant::now();
        let mut losses = Losses::new(patience, start);
        // Each rejoin lost again moments later, as in the live loop.
        let mut now = start;
        for _ in 0..MAX_QUICK_LOSSES {
            now += Duration::from_secs(5);
            let deadline = losses.lost(now).unwrap();
            assert_eq!(deadline, start + Duration::from_secs(5) + patience);
            losses.connected(now);
        }
        now += Duration::from_secs(5);
        assert!(losses.lost(now).unwrap_err().contains("in a row"));

        // A connection that held starts afresh, patience and count alike.
        let mut losses = Losses::new(patience, start);
        losses.lost(start + Duration::from_secs(1)).unwrap();
        losses.connected(start + Duration::from_secs(2));
        let later = start + Duration::from_secs(2) + STABLE;
        assert_eq!(losses.lost(later).unwrap(), later + patience);

        // Rejoins that never hold run out of patience across them.
        let mut losses = Losses::new(patience, start);
        losses.lost(start).unwrap();
        losses.connected(start + Duration::from_secs(250));
        assert!(
            losses
                .lost(start + Duration::from_secs(301))
                .unwrap_err()
                .contains("no connection held")
        );
    }

    #[test]
    fn fetch_percent_is_whole_and_bounded() {
        assert_eq!(percent_of(0, 0), 0);
        assert_eq!(percent_of(0, 1000), 0);
        assert_eq!(percent_of(421, 1000), 42);
        assert_eq!(percent_of(1000, 1000), 100);
        assert_eq!(percent_of(5000, 1000), 100);
        assert_eq!(percent_of(u64::MAX, u64::MAX), 100);
    }

    /// The room hears a new stage at once and a new percent of the same
    /// fetch at most about twice a second; nothing twice.
    #[test]
    fn loading_is_told_on_change_and_throttled_while_fetching() {
        let now = Instant::now();
        let fetching = |percent| Some(LoadingStage::Fetching { percent });
        // Nothing told yet on this connection.
        assert!(!loading_due(None, None, now));
        assert!(loading_due(None, fetching(0), now));
        let told = Some((fetching(10), now));
        assert!(!loading_due(told, fetching(10), now + LOADING_EVERY));
        assert!(!loading_due(told, fetching(11), now + LOADING_EVERY / 2));
        assert!(loading_due(told, fetching(11), now + LOADING_EVERY));
        // A new stage, or its end, at once.
        assert!(loading_due(told, Some(LoadingStage::Loading), now));
        assert!(loading_due(told, None, now));
        let loading = Some((Some(LoadingStage::Loading), now));
        assert!(!loading_due(loading, Some(LoadingStage::Loading), now));
        assert!(loading_due(loading, None, now));
    }

    fn start() -> TurnStart {
        TurnStart {
            world_epoch: 1,
            room: RoomId(FixedBytes([0; 16])),
            rules: Text::new("native").unwrap(),
            next_turn: 1,
            next_event: 1,
            sealed_through: 0,
            steps_per_second: 10,
            checkpoint_interval: 10,
            history: 1,
            world: None,
        }
    }

    fn event(seq: u64, step: u64) -> Event {
        Event {
            seq,
            step,
            body: EventBody::PlayerLeft {
                player: PlayerId(FixedBytes([1; 32])),
                kicked: false,
            },
        }
    }

    fn turn(number: u64, sealed_through: u64, events: Vec<Event>) -> Turn {
        Turn {
            number,
            sealed_through,
            speed: Speed::NORMAL,
            events,
        }
    }

    /// A follower and a playout that has seen `turns` arrive at `now`.
    fn fed(turns: Vec<Turn>, now: Instant) -> (TurnFollower, Playout) {
        let mut follower = TurnFollower::new(&start());
        let mut playout = Playout::new(10, Duration::ZERO, Duration::from_secs(10));
        for turn in turns {
            follower.accept(turn).unwrap();
            playout.on_turn(follower.sealed_through(), follower.speed(), now);
        }
        (follower, playout)
    }

    #[test]
    fn the_game_hears_the_rooms_speed_from_its_first_turn() {
        let mut told = None;
        assert_eq!(
            speed_news(&mut told, Speed::NORMAL),
            Some(Speed::NORMAL),
            "even at normal speed"
        );
        assert_eq!(speed_news(&mut told, Speed::NORMAL), None, "once");
        assert_eq!(speed_news(&mut told, Speed(400)), Some(Speed(400)));
        assert_eq!(speed_news(&mut told, Speed::PAUSED), Some(Speed::PAUSED));
    }

    #[test]
    fn the_game_sees_the_room_by_its_members_names() {
        use tpf3mp_proto::{MemberView, Platform, RoomPhase, RoomSettings};
        let member = |n: u8, name: &str, connected: bool| MemberView {
            player: PlayerId(FixedBytes([n; 32])),
            name: Text::new(name).unwrap(),
            platform: Platform::current(),
            ready: true,
            content: None,
            connected,
            banner: None,
            loading: None,
            differs: None,
        };
        let room = RoomView {
            id: RoomId(FixedBytes([7; 16])),
            name: Text::new("Sunday line").unwrap(),
            rules: Text::new("native").unwrap(),
            owner: PlayerId(FixedBytes([1; 32])),
            max_players: 4,
            has_password: false,
            phase: RoomPhase::Running,
            settings: RoomSettings::DEFAULT,
            members: vec![member(1, "Ann", true), member(2, "Bo", false)],
            competitive: false,
            start: None,
        };
        let info = room_info(&room);
        assert_eq!(info.name.as_str(), "Sunday line");
        assert_eq!(info.owner, room.owner);
        let members: Vec<(&str, bool)> = info
            .members
            .iter()
            .map(|m| (m.name.as_str(), m.connected))
            .collect();
        assert_eq!(members, [("Ann", true), ("Bo", false)]);
        // A banner reaches the game; a portrait it lacks does not, so the
        // game shows that member's default banner.
        let mut pictured = room.clone();
        pictured.members[0].banner = Some(Text::new("dry").unwrap());
        pictured.members[1].banner = Some(Text::new("lasse").unwrap());
        let info = room_info(&pictured);
        assert_eq!(
            info.members[0].banner.as_ref().map(Text::as_str),
            Some("dry")
        );
        assert_eq!(info.members[1].banner, None);
    }

    #[test]
    fn generated_world_waits_for_the_owner_world_and_every_player() {
        use tpf3mp_proto::{MemberView, Platform, RoomSettings};
        let owner = PlayerId(FixedBytes([1; 32]));
        let guest = PlayerId(FixedBytes([2; 32]));
        let mut room = RoomView {
            start: None,

            id: RoomId(FixedBytes([7; 16])),
            name: Text::new("New world").unwrap(),
            rules: Text::new("native").unwrap(),
            owner,
            max_players: 2,
            has_password: false,
            phase: RoomPhase::Lobby,
            settings: RoomSettings::DEFAULT,
            competitive: false,
            members: [owner, guest]
                .map(|player| MemberView {
                    loading: None,
                    differs: None,

                    player,
                    name: Text::new("Player").unwrap(),
                    platform: Platform::current(),
                    ready: true,
                    content: None,
                    connected: true,
                    banner: None,
                })
                .to_vec(),
        };
        assert!(!generated_world_can_start(Some(&room), owner, 0, false));
        assert!(!generated_world_can_start(Some(&room), guest, 1, false));
        room.members[1].ready = false;
        assert!(!generated_world_can_start(Some(&room), owner, 1, false));
        room.members[1].ready = true;
        room.members[1].connected = false;
        assert!(!generated_world_can_start(Some(&room), owner, 1, false));
        room.members[1].connected = true;
        assert!(generated_world_can_start(Some(&room), owner, 1, false));
        assert!(!generated_world_can_start(Some(&room), owner, 1, true));
        room.phase = RoomPhase::Running;
        assert!(!generated_world_can_start(Some(&room), owner, 1, false));
    }

    #[test]
    fn only_each_members_latest_preview_waits_for_the_game() {
        let mut out = Outbox::default();
        let ann = PlayerId(tpf3mp_proto::FixedBytes([1; 32]));
        let bob = PlayerId(tpf3mp_proto::FixedBytes([2; 32]));
        let shown = |n: u8| Some(Payload::new(vec![n; 8]).unwrap());
        out.preview(ann, shown(1));
        out.push_back(ToHook::Release { through: 5 });
        out.preview(bob, shown(2));
        out.preview(ann, shown(3));
        out.preview(ann, None);
        assert_eq!(
            out.messages,
            [
                ToHook::Release { through: 5 },
                ToHook::Preview {
                    from: bob,
                    preview: shown(2)
                },
                ToHook::Preview {
                    from: ann,
                    preview: None
                },
            ]
        );
    }

    #[test]
    fn steps_without_events_between_them_go_out_as_one_release() {
        let now = Instant::now();
        let (mut follower, mut playout) = fed(vec![turn(1, 5, vec![])], now);
        let mut out = Outbox::default();
        // Steps 1 to 5 were sealed at once; reckoned at pace, all are due.
        let wait = pump(&mut follower, &mut playout, now, &mut out);
        assert_eq!(out.messages, [ToHook::Release { through: 5 }]);
        assert_eq!(wait, None, "nothing further is sealed");
    }

    #[test]
    fn an_event_splits_the_releases_around_it() {
        let now = Instant::now();
        let (mut follower, mut playout) = fed(
            vec![turn(1, 2, vec![event(1, 1)]), turn(2, 4, vec![event(2, 3)])],
            now,
        );
        let mut out = Outbox::default();
        // Both turns arrived together, so steps 3 and 4 play at pace after
        // step 2: a second later, all are due.
        pump(
            &mut follower,
            &mut playout,
            now + Duration::from_secs(1),
            &mut out,
        );
        assert_eq!(
            out.messages,
            [
                ToHook::Apply(event(1, 1)),
                ToHook::Release { through: 2 },
                ToHook::Apply(event(2, 3)),
                ToHook::Release { through: 4 },
            ]
        );
    }

    #[test]
    fn a_step_not_yet_due_waits_and_says_when() {
        let now = Instant::now();
        let (mut follower, mut playout) = fed(vec![turn(1, 1, vec![])], now);
        let mut out = Outbox::default();
        pump(&mut follower, &mut playout, now, &mut out);
        assert_eq!(out.messages, [ToHook::Release { through: 1 }]);
        // Step 2 arrives now; with steps 100 ms apart, it plays 100 ms after
        // step 1.
        follower.accept(turn(2, 2, vec![])).unwrap();
        playout.on_turn(2, Speed::NORMAL, now);
        out = Outbox::default();
        let wait = pump(&mut follower, &mut playout, now, &mut out);
        assert!(out.messages.is_empty());
        assert!(wait.is_some_and(|at| at > now), "{wait:?}");
        let wait = pump(
            &mut follower,
            &mut playout,
            now + Duration::from_millis(100),
            &mut out,
        );
        assert_eq!(out.messages, [ToHook::Release { through: 2 }]);
        assert_eq!(wait, None);
    }

    #[test]
    fn a_full_outbox_takes_nothing_more_from_the_room() {
        let now = Instant::now();
        let big = |seq| Event {
            seq,
            step: 1,
            body: EventBody::Command {
                player: PlayerId(FixedBytes([7; 32])),
                client_seq: seq,
                payload: Payload::new(vec![0; MAX_PAYLOAD]).unwrap(),
                seal: None,
            },
        };
        // Far more than the outbox takes, all for the next step.
        let events: Vec<Event> = (1..=1000).map(big).collect();
        let (mut follower, mut playout) = fed(vec![turn(1, 0, events)], now);
        let mut out = Outbox::default();
        pump(&mut follower, &mut playout, now, &mut out);
        assert!(out.is_full());
        let taken = out.messages.len();
        assert!(taken < 1000 && out.bytes < OUTBOX_BYTES + MAX_PAYLOAD + 64);
        // Nothing more while it is full; the rest once the hook read some.
        pump(&mut follower, &mut playout, now, &mut out);
        assert_eq!(out.messages.len(), taken);
        while out.pop_front().is_some() && out.messages.len() > taken / 2 {}
        pump(&mut follower, &mut playout, now, &mut out);
        assert!(out.messages.len() > taken / 2);
    }

    fn lobby(name: &str) -> LobbyView {
        LobbyView {
            name: Text::lossy(name),
            ..LobbyView::default()
        }
    }

    #[test]
    fn a_greeted_bridge_passes_the_launchers_lobby_both_ways() {
        use crate::launcher::lobby::tests::FakeLink;

        let fake = FakeLink::default();
        let (views_tx, views) = watch::channel(LobbyView::default());
        let (actions, mut heard) = mpsc::unbounded_channel();
        let mut bridge = Bridge::new(
            fake.clone(),
            BridgeOptions {
                lobby: Some(LobbyLink { views, actions }),
                ..BridgeOptions::default()
            },
        )
        .greeted("40408");
        views_tx.send(lobby("A")).unwrap();
        views_tx.send(lobby("B")).unwrap();
        bridge.lobby_news();
        bridge.flush().unwrap();
        assert_eq!(
            fake.hook_hears(),
            vec![ToHook::Lobby(Box::new(lobby("B")))],
            "the newest only, and no hello again"
        );
        bridge.lobby_news();
        bridge.flush().unwrap();
        assert!(fake.hook_hears().is_empty(), "unchanged");

        bridge.lobby_action(LobbyAction::Start);
        assert_eq!(heard.try_recv().unwrap(), LobbyAction::Start);

        // A room left before its game began ends nothing in the game, and
        // the launcher gets the link back, greeted.
        bridge.end("Left");
        assert!(fake.hook_hears().is_empty());
        let (_link, build) = bridge.into_link();
        assert_eq!(build.as_deref(), Some("40408"));
    }

    #[test]
    fn only_files_inside_the_saves_directory_are_within_it() {
        let dir = std::env::temp_dir().join(format!("tpf3mp-within-{}", std::process::id()));
        let saves = dir.join("saves");
        std::fs::create_dir_all(&saves).unwrap();
        let inside = saves.join("12.sav");
        let outside = dir.join("notes.txt");
        std::fs::write(&inside, b"save").unwrap();
        std::fs::write(&outside, b"notes").unwrap();
        assert!(within(&inside, &saves));
        assert!(!within(&outside, &saves));
        assert!(!within(&saves.join("..").join("notes.txt"), &saves));
        assert!(!within(&saves, &saves));
        assert!(!within(&saves.join("missing.sav"), &saves));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
