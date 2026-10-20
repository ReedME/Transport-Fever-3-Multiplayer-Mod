//! A room: its lobby, its members and, once running, its sequencer. Each room
//! is one task that owns all of its state; connections talk to it through
//! [`RoomHandle`]. The turn invariants it upholds are in `docs/PROTOCOL.md`.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use ring::hmac;
use thiserror::Error;
use tokio::{
    sync::{mpsc, oneshot},
    time::MissedTickBehavior,
};
use tpf3mp_nativemods::{index::Setting, terms::settings_digest};
use tpf3mp_net::{bulk, close};
use tpf3mp_proto::{
    BannerId, ChatText, ContentFingerprint, ContentManifest, Event, EventBody, FRAME_HEADER_LEN,
    FixedBytes, IntentRejection, Invite, LaneDigest, LoadingStage, MemberView, NativeMod, Payload,
    Platform, PlayerId, RequestError, Resume, RoomDeclaration, RoomId, RoomListing, RoomMods,
    RoomPhase, RoomSettings, RoomView, RulesName, SavedWorld, Seal, Secret, ServerMessage,
    SnapshotId, Speed, StartSave, StartView, TURN_MAX_FRAME, Text, Turn, TurnMessage, TurnStart,
    WorldOffer, decode_frame, encode_frame, native_term_version_is_full,
};
use tpf3mp_snapshot::{Manifest, ManifestId};
use tracing::{debug, error, info, warn};

use crate::{
    admission::RoomShare,
    directory::Directory,
    limit::TokenBucket,
    metrics::{self, Metrics},
    pacing::Pacer,
    persist::{self, Base, LogError, RoomLog, StartMember, StartRecord},
    ruleset::{RulesMenu, Ruleset},
    snapshots::{
        self, Agreed, Candidates, Pointer, PointerWrites, SAVE_DEADLINE, SavePoint, SaveReport,
        SaveRound, Saves, Snapshots, UPLOAD_START, Upload,
    },
    verdict::{self, Report, Verdict},
};

/// Commands a room queues before senders wait.
pub(crate) const ROOM_QUEUE: usize = 1024;
/// Payload bytes one turn may carry. Far below the frame cap, so a turn with
/// the per-event overhead always encodes.
const TURN_PAYLOAD_BUDGET: usize = TURN_MAX_FRAME / 2;
/// How far past the slowest member the frontier may run, beyond the lead.
const MAX_AHEAD: Duration = Duration::from_secs(2);
/// Intents per second a player may send, and the burst allowed on top.
const INTENTS_PER_SECOND: u32 = 20;
const INTENT_BURST: u32 = 40;
/// Chat messages a player may send per second, and the burst on top. Every
/// message goes to every member.
const CHATS_PER_SECOND: u32 = 1;
const CHAT_BURST: u32 = 5;
/// Intent payload bytes per second a player may send, and the burst allowed
/// on top. A build command is a few hundred bytes; this leaves room for
/// large ones without letting one player grow a room's log quickly.
const PAYLOAD_BYTES_PER_SECOND: u32 = 32 * 1024;
const PAYLOAD_BURST: u32 = 256 * 1024;
/// A checkpoint round waits this long for every pacing member before
/// deciding with the reports it has.
const CHECKPOINT_DEADLINE: Duration = Duration::from_secs(30);
/// Decided rounds kept to judge members who report late.
const DECIDED_ROUNDS_KEPT: usize = 64;
/// Reports a round needs for a verdict. One report compares against
/// nothing, and would only judge later reporters by an unchecked claim.
const MIN_VERDICT_REPORTS: usize = 2;
/// Undecided rounds at once; no client can make the server hold more.
const MAX_OPEN_ROUNDS: usize = 64;
/// Turns kept in memory for resuming: an hour at the default tick, and at
/// most this many bytes. A player away longer needs a world snapshot
/// instead.
const RESUME_WINDOW: usize = 36_000;
const RESUME_WINDOW_BYTES: usize = 64 << 20;
/// No honest log seals further than this: centuries of play at the fastest
/// settings. Recovery refuses logs that do, so arithmetic on steps never
/// overflows.
const MAX_FRONTIER: u64 = 1 << 48;
/// No honest game gets this many turns or events; a compacted log's base
/// claiming more is refused, so counting on never overflows.
const MAX_COUNT: u64 = 1 << 56;
/// The least time between two rebases of one member. A replica that keeps
/// diverging is told so every time, but reloading it more often would only
/// keep its player out of the game.
const REBASE_GAP: Duration = Duration::from_secs(300);
/// Players a room keeps barred. The owner kicking throwaway identities in a
/// loop cannot grow it further.
const MAX_BANNED: usize = 1024;

/// The channels to one connection of a member.
#[derive(Clone)]
pub(crate) struct MemberLink {
    /// Distinguishes this connection from an earlier or later one of the
    /// same player.
    pub(crate) id: u64,
    pub(crate) control: mpsc::Sender<ServerMessage>,
    /// Other members' build previews ([`ServerMessage::Preview`]): dropped
    /// when full, never a reason to disconnect.
    pub(crate) advisory: mpsc::Sender<ServerMessage>,
    pub(crate) turns: mpsc::Sender<TurnFeed>,
    pub(crate) connection: quinn::Connection,
}

/// What a connection's turn-stream writer receives.
pub(crate) enum TurnFeed {
    /// Open a new turn stream: the start message, then turns already sealed.
    Open {
        start: TurnStart,
        backlog: Vec<Arc<[u8]>>,
    },
    /// One encoded turn frame.
    Frame(Arc<[u8]>),
    /// Finish the turn stream.
    Close,
}

pub(crate) struct NewMember {
    pub(crate) player: PlayerId,
    pub(crate) name: Text<32>,
    pub(crate) platform: Platform,
    pub(crate) link: MemberLink,
    /// What the player's game runs, if the player declared it.
    pub(crate) content: Option<Arc<Declared>>,
    /// The banner the player picked (`Request::SetBanner`), if any.
    pub(crate) banner: Option<BannerId>,
}

/// What a player's game runs, as the player declared it, and for a room's
/// owner who declared them ([`tpf3mp_proto::Request::DeclareRoom`]), the
/// room's mods.
#[derive(Debug)]
pub(crate) struct Declared {
    pub(crate) fingerprint: ContentFingerprint,
    pub(crate) manifest: ContentManifest,
    /// The room's mods with a digest of them, to tell members once each.
    pub(crate) room: Option<(RoomMods, [u8; 32])>,
}

impl Declared {
    pub(crate) fn new(manifest: ContentManifest) -> Self {
        Self {
            fingerprint: manifest.fingerprint(),
            manifest,
            room: None,
        }
    }

    /// Restores a current-format declaration with the exact native settings its
    /// manifest names. Native mismatch is an inconsistent start record and
    /// must stop recovery rather than becoming invented defaults.
    pub(crate) fn restored_with_native(
        manifest: ContentManifest,
        native: Vec<NativeMod>,
    ) -> Result<Self, tpf3mp_proto::RoomModsError> {
        let info = manifest
            .mods
            .iter()
            .map(|listed| tpf3mp_proto::ModInfo {
                name: Text::lossy(listed.id.as_str()),
                source: Text::lossy(""),
                modio: None,
            })
            .collect();
        let declaration = RoomDeclaration {
            manifest,
            room: tpf3mp_proto::RoomConfig {
                info,
                params: Vec::new(),
                native,
            },
        };
        match declaration.validate() {
            Ok(()) => {
                if let Some(native) = declaration
                    .room
                    .native
                    .iter()
                    .find(|native| !native_settings_match_version(native))
                {
                    return Err(tpf3mp_proto::RoomModsError::NativeSetting(
                        native.id.as_str().to_owned(),
                    ));
                }
                Ok(Self::with_room(declaration))
            }
            Err(_)
                if declaration.manifest.native.is_empty() && declaration.room.native.is_empty() =>
            {
                // Preserve the old behavior for non-native manifests that
                // did not describe a complete room mod list.
                Ok(Self::new(declaration.manifest))
            }
            Err(error) => Err(error),
        }
    }

    /// The owner's declaration of their content and the room's mods, which
    /// the connection validated.
    pub(crate) fn with_room(declaration: RoomDeclaration) -> Self {
        let room = declaration.room_mods();
        let digest = room_digest(&room);
        Self {
            fingerprint: declaration.manifest.fingerprint(),
            manifest: declaration.manifest,
            room: Some((room, digest)),
        }
    }

    /// What members are told of the room's mods when this is the room's
    /// content: the owner's list, when they declared one. Never one made up
    /// from a manifest: a member told a list loads the room's world with
    /// it, and the owner's game, without one, with its save's own.
    fn told(&self) -> Option<(RoomMods, [u8; 32])> {
        self.room.clone()
    }
}

fn native_settings_match_version(native: &NativeMod) -> bool {
    let settings: BTreeMap<String, Setting> = native
        .settings
        .iter()
        .map(|setting| {
            let value = match &setting.value {
                tpf3mp_proto::NativeValue::Bool(value) => Setting::Bool(*value),
                tpf3mp_proto::NativeValue::Int(value) => Setting::Int(*value),
                tpf3mp_proto::NativeValue::Text(value) => Setting::Text(value.as_str().to_owned()),
            };
            (setting.key.as_str().to_owned(), value)
        })
        .collect();
    let digest = settings_digest(&settings);
    let expected = &digest[..10];
    native
        .version
        .as_str()
        .rsplit_once('+')
        .is_some_and(|(_, suffix)| {
            native_term_version_is_full(native.version.as_str()) && suffix.starts_with(expected)
        })
}

fn native_settings_are_persistable(declared: &Declared) -> bool {
    let Some((room, _)) = &declared.room else {
        return declared.manifest.native.is_empty();
    };
    room.native.len() == declared.manifest.native.len()
        && room.native.iter().all(native_settings_match_version)
}

/// A digest of what members are told of the room's mods.
fn room_digest(room: &RoomMods) -> [u8; 32] {
    let digest = ring::digest::digest(
        &ring::digest::SHA256,
        &postcard::to_allocvec(room).unwrap_or_default(),
    );
    digest.as_ref().try_into().unwrap_or([0; 32])
}

pub(crate) type Reply<T = ()> = oneshot::Sender<Result<T, RequestError>>;

pub(crate) enum RoomCommand {
    ResolveInvite {
        player: PlayerId,
        invite: Invite,
        password: Option<Text<64>>,
        reply: Reply,
    },
    Join {
        member: NewMember,
        invite: Invite,
        password: Option<Text<64>>,
        resume: Option<Resume>,
        scan_plan: tpf3mp_proto::ScanPlan,
        reply: Reply<RoomView>,
    },
    Leave {
        player: PlayerId,
        reply: Reply,
    },
    Disconnected {
        player: PlayerId,
        link: u64,
    },
    SetReady {
        player: PlayerId,
        ready: bool,
        reply: Reply,
    },
    DeclareContent {
        player: PlayerId,
        content: Arc<Declared>,
        reply: Reply,
    },
    Start {
        player: PlayerId,
        reply: Reply,
    },
    /// The owner names the world the game starts from, a save its client
    /// holds; the room asks for it at once.
    StartWorld {
        player: PlayerId,
        world: SavedWorld,
        save: StartSave,
        reply: Reply,
    },
    /// The owner takes back the world it handed over: the owner's game
    /// provides the room's world again.
    ClearStartWorld {
        player: PlayerId,
        reply: Reply,
    },
    SetSpeed {
        player: PlayerId,
        speed: Speed,
        reply: Reply,
    },
    Kick {
        player: PlayerId,
        target: PlayerId,
        reply: Reply,
    },
    /// Whether the player is still a member; `NotInRoom` if not, for
    /// example after a kick.
    IsMember {
        player: PlayerId,
        reply: Reply,
    },
    Chat {
        player: PlayerId,
        text: ChatText,
        reply: Reply,
    },
    /// A member picked another banner.
    SetBanner {
        player: PlayerId,
        banner: Option<BannerId>,
        reply: Reply,
    },
    /// The owner of a public room says what the room list shows of it.
    Describe {
        player: PlayerId,
        listing: RoomListing,
        reply: Reply,
    },
    Intent {
        player: PlayerId,
        client_seq: u64,
        payload: Payload,
        secret: Option<Secret>,
    },
    Progress {
        player: PlayerId,
        link: u64,
        step: u64,
        world_epoch: u64,
    },
    Loaded {
        player: PlayerId,
        link: u64,
        world_epoch: u64,
        step: u64,
    },
    /// Where a member's game is with the room's world while it comes in.
    Loading {
        player: PlayerId,
        stage: Option<LoadingStage>,
    },
    /// What a member's build tool shows now, for the others.
    Preview {
        player: PlayerId,
        preview: Option<Payload>,
    },
    Checkpoint {
        player: PlayerId,
        link: u64,
        world_epoch: u64,
        step: u64,
        lanes: Vec<LaneDigest>,
    },
    Saved {
        player: PlayerId,
        link: u64,
        world_epoch: u64,
        event: u64,
        lanes: Vec<LaneDigest>,
        world: Option<SavedWorld>,
    },
    /// A member's connection wants to fetch a snapshot. The room answers
    /// with its manifest only if it offered that snapshot to that
    /// connection.
    Fetch {
        player: PlayerId,
        link: u64,
        snapshot: SnapshotId,
        reply: oneshot::Sender<Option<Arc<Manifest>>>,
    },
    /// A member's connection starts to upload a save. The room answers
    /// whether it asked that member for that save.
    Upload {
        player: PlayerId,
        link: u64,
        snapshot: SnapshotId,
        reply: oneshot::Sender<bool>,
    },
    /// An upload ended: the save is in the store, verified and retained, or
    /// it failed.
    Uploaded {
        player: PlayerId,
        snapshot: SnapshotId,
        result: Result<Arc<Manifest>, String>,
    },
}

/// A connection's way to reach a room.
#[derive(Clone)]
pub(crate) struct RoomHandle {
    commands: mpsc::Sender<RoomCommand>,
}

impl RoomHandle {
    pub(crate) fn new(commands: mpsc::Sender<RoomCommand>) -> Self {
        Self { commands }
    }

    /// Sends a request and waits for the room's answer. A room that has
    /// closed answers `NotInRoom`.
    pub(crate) async fn request<T>(
        &self,
        make: impl FnOnce(Reply<T>) -> RoomCommand,
    ) -> Result<T, RequestError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(make(reply))
            .await
            .map_err(|_| RequestError::NotInRoom)?;
        answer.await.map_err(|_| RequestError::NotInRoom)?
    }

    /// Queues a command without waiting. Returns false when the room is gone
    /// or its queue is full.
    pub(crate) fn notify(&self, command: RoomCommand) -> bool {
        self.commands.try_send(command).is_ok()
    }

    /// Sends a command that carries its own reply channel and waits for the
    /// answer. `None` when the room is gone.
    pub(crate) async fn ask<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<T>) -> RoomCommand,
    ) -> Option<T> {
        let (reply, answer) = oneshot::channel();
        self.commands.send(make(reply)).await.ok()?;
        answer.await.ok()
    }

    /// Queues a command, waiting while the queue is full: for news the room
    /// must not miss. Returns false when the room is gone.
    pub(crate) async fn tell(&self, command: RoomCommand) -> bool {
        self.commands.send(command).await.is_ok()
    }
}

/// Secret material that authorizes joining a room.
pub(crate) struct RoomSecrets {
    pub(crate) key: hmac::Key,
    /// Kept as bytes: they are persisted, and ring verifies against slices.
    pub(crate) invite_tag: Vec<u8>,
    pub(crate) password_tag: Option<Vec<u8>>,
}

impl RoomSecrets {
    /// What an invite's tag signs: its code alone, since the server finds
    /// the room by the tag (`Directory::find`).
    pub(crate) fn invite_input(invite: &Invite) -> Vec<u8> {
        [b"invite code".as_slice(), invite.0.as_str().as_bytes()].concat()
    }

    pub(crate) fn password_input(room: &RoomId, password: &Text<64>) -> Vec<u8> {
        [
            b"password".as_slice(),
            &room.0.0,
            password.as_str().as_bytes(),
        ]
        .concat()
    }

    /// What a company password's seal signs: the room, the scope the player
    /// named (the company) and the password, so a seal fits one company of
    /// one room.
    pub(crate) fn seal_input(room: &RoomId, scope: u64, password: &Text<64>) -> Vec<u8> {
        [
            b"company password".as_slice(),
            &room.0.0,
            &scope.to_le_bytes(),
            password.as_str().as_bytes(),
        ]
        .concat()
    }

    /// The seal the room orders an intent with in place of its password
    /// (`Secret`). Every game compares seals; only this server's key makes
    /// or checks one.
    fn seal(&self, room: &RoomId, secret: &Secret) -> Seal {
        let tag = hmac::sign(
            &self.key,
            &Self::seal_input(room, secret.scope, &secret.password),
        );
        let mut bytes = [0; 32];
        bytes.copy_from_slice(tag.as_ref());
        Seal {
            scope: secret.scope,
            tag: FixedBytes(bytes),
        }
    }

    /// Checks the invite and password in constant time. Both are always
    /// checked; which one failed stays inside the server, and the client
    /// learns only `BadInvite`.
    fn check(&self, room: &RoomId, invite: &Invite, password: Option<&Text<64>>) -> Admittance {
        let invite_ok = hmac::verify(
            &self.key,
            &Self::invite_input(invite),
            self.invite_tag.as_ref(),
        )
        .is_ok();
        let password_ok = match (&self.password_tag, password) {
            (None, _) => true,
            (Some(tag), Some(password)) => hmac::verify(
                &self.key,
                &Self::password_input(room, password),
                tag.as_ref(),
            )
            .is_ok(),
            (Some(_), None) => false,
        };
        match (invite_ok, password_ok) {
            (true, true) => Admittance::Admitted,
            (true, false) => Admittance::WrongPassword,
            (false, _) => Admittance::WrongInvite,
        }
    }
}

enum Admittance {
    Admitted,
    /// A valid invite with a wrong or missing password: someone holding the
    /// invite, possibly guessing.
    WrongPassword,
    WrongInvite,
}

/// Wrong passwords a room takes from newcomers per minute. Past this, it
/// refuses every newcomer's password, right or wrong, for the rest of the
/// minute, so a leaked invite does not let anyone guess the password at
/// line rate. Members already seated are never held up.
const PASSWORD_FAILURES_PER_MINUTE: u32 = 10;

struct PasswordGuard {
    window_start: Instant,
    failures: u32,
}

impl PasswordGuard {
    fn new() -> Self {
        Self {
            window_start: Instant::now(),
            failures: 0,
        }
    }

    fn roll(&mut self, now: Instant) {
        if now.saturating_duration_since(self.window_start) >= Duration::from_secs(60) {
            self.window_start = now;
            self.failures = 0;
        }
    }

    fn open(&mut self, now: Instant) -> bool {
        self.roll(now);
        self.failures < PASSWORD_FAILURES_PER_MINUTE
    }

    fn failed(&mut self, now: Instant) {
        self.roll(now);
        self.failures = self.failures.saturating_add(1);
    }
}

/// Passwords one player may send with intents in [`SECRET_WINDOW`]. The room
/// cannot tell a right company password from a wrong one (every game
/// compares the seals), so it counts them all: enough to set a password
/// and join a few companies, and at most about 2,900 guesses a day, as
/// D13 holds a room's own password to.
const SECRETS_PER_WINDOW: u32 = 20;
/// Passwords the whole room takes in [`SECRET_WINDOW`]: a new player key
/// costs nothing, so a player's own count alone does not bound guesses.
const ROOM_SECRETS_PER_WINDOW: u32 = 60;
const SECRET_WINDOW: Duration = Duration::from_secs(10 * 60);

/// Counts the room's passwords in the current window, by player and in all.
/// It is the room's, not a membership's: leaving and joining again gives no
/// password back. It holds at most one entry for each password taken.
struct SecretBudget {
    window_start: Instant,
    room_used: u32,
    used: BTreeMap<PlayerId, u32>,
}

impl SecretBudget {
    fn new() -> Self {
        Self {
            window_start: Instant::now(),
            room_used: 0,
            used: BTreeMap::new(),
        }
    }

    /// Takes one for `player`, if the window has one left for them and for
    /// the room.
    fn take(&mut self, player: &PlayerId, now: Instant) -> bool {
        if now.saturating_duration_since(self.window_start) >= SECRET_WINDOW {
            self.window_start = now;
            self.room_used = 0;
            self.used.clear();
        }
        if self.room_used >= ROOM_SECRETS_PER_WINDOW {
            return false;
        }
        let used = self.used.entry(*player).or_insert(0);
        if *used >= SECRETS_PER_WINDOW {
            return false;
        }
        *used += 1;
        self.room_used += 1;
        true
    }
}

struct Member {
    player: PlayerId,
    name: Text<32>,
    platform: Platform,
    /// Where its game is with the room's world while it comes in, as it
    /// last said, and when the room last showed a change; not logged.
    loading: Option<LoadingStage>,
    loading_at: Option<Instant>,
    /// The banner the player picked; not logged, so a restored room shows
    /// the default until the player says again.
    banner: Option<BannerId>,
    ready: bool,
    content: Option<ContentFingerprint>,
    /// The manifest behind `content`, when this connection declared it.
    declared: Option<Arc<Declared>>,
    /// The room's content and this member's when this member was last
    /// told they differ; `None` when it has not been told of a difference.
    told_diff: Option<(ContentFingerprint, ContentFingerprint)>,
    /// The digest of the room's mods this member was last told, `Some(None)`
    /// for none; `None` until told at all.
    told_room: Option<Option<[u8; 32]>>,
    link: Option<MemberLink>,
    /// Whether this member's current link has an open turn stream.
    streaming: bool,
    pace: Pace,
    /// When this member's progress last moved forward.
    advanced: Instant,
    intents: TokenBucket,
    payload_bytes: TokenBucket,
    chats: TokenBucket,
    /// Passwords sent with intents.
    /// What this member must receive before it can follow the game.
    needs: Needs,
    /// The snapshot this member's connection may fetch.
    offered: Option<SnapshotId>,
    /// The world this member's game last loaded, as far as the room knows:
    /// set once its load is acknowledged, kept when the player reconnects and
    /// resumes (its game keeps playing that world), unknown after a restart.
    loaded: Option<SnapshotId>,
    /// Epoch the current stream reports; zero until a stream is opened.
    world_epoch: u64,
    /// Frontier of an ordered world, until explicitly acknowledged.
    load_step: Option<u64>,
    /// When the room last rebased this member.
    rebased: Option<Instant>,
    /// The first event of this member's turn stream: it can report only
    /// saves from there on.
    stream_from: u64,
}

/// What a member of a running game must receive before it can follow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Needs {
    Nothing,
    /// A world: it joined the running game, or could no longer resume.
    World,
    /// A world agreed on at or after this step, where its own diverged.
    Rebase {
        after: u64,
    },
}

/// How long the room waits for a member before it stops holding everyone
/// else for them. A demoted member rejoins the pacing set by catching up.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timeouts {
    /// A member with sealed steps to run that has not advanced for this long.
    pub(crate) stall: Duration,
    /// A member still loading the world this long after the start.
    pub(crate) load: Duration,
    /// A running game with nobody connected for this long closes.
    pub(crate) abandoned: Duration,
}

/// How a member relates to the room clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pace {
    /// Loading the world after the start; holds the clock.
    Loading,
    /// Executing close to the frontier; the slowest of these paces the room.
    Following(u64),
    /// Too far behind to pace the room (after reconnecting, for example).
    CatchingUp(Option<u64>),
}

enum Phase {
    Lobby,
    Running(Box<Game>),
}

/// The world a room's game starts from, when its owner handed one over in
/// the lobby (`Request::StartWorld`) instead of having its game save one
/// once the game began.
enum StartWorld {
    /// Asked of the owner, not received yet.
    Asked {
        world: SavedWorld,
        save: StartSave,
        from: PlayerId,
        asked: Instant,
        /// Whether the owner opened its stream.
        receiving: bool,
    },
    /// In the store, held by the room.
    Held {
        manifest: Arc<Manifest>,
        save: StartSave,
    },
}

impl StartWorld {
    fn snapshot(&self) -> SnapshotId {
        match self {
            Self::Asked { world, .. } => world.snapshot,
            Self::Held { manifest, .. } => bulk::snapshot_id(&manifest.id()),
        }
    }

    /// The save as the owner named it.
    fn save(&self) -> &StartSave {
        match self {
            Self::Asked { save, .. } | Self::Held { save, .. } => save,
        }
    }

    /// What the room's members see of it.
    fn view(&self) -> StartView {
        StartView {
            save: self.save().clone(),
            arrived: matches!(self, Self::Held { .. }),
        }
    }
}

struct Game {
    pacer: Pacer,
    speed: Speed,
    /// The speed in the last turn sent. A change must reach clients even
    /// when nothing else happens, or a pause would go unannounced.
    announced_speed: Speed,
    sealed_through: u64,
    next_turn: u64,
    next_event: u64,
    pending: Vec<Event>,
    /// The most recent turns, encoded, for members who resume: at most
    /// [`RESUME_WINDOW`] of them, starting with turn `log_first_turn`.
    log: VecDeque<LoggedTurn>,
    log_first_turn: u64,
    /// The frontier of the turn before `log_first_turn`.
    sealed_before_log: u64,
    /// Bytes of the turns in `log`.
    log_bytes: usize,
    resume_window: usize,
    last_tick: Instant,
    /// When the game started (or was restored), for the load timeout.
    started: Instant,
    /// Checkpoint rounds by step.
    rounds: BTreeMap<u64, Round>,
    /// Rounds at or below this step are closed: pruned or expired. A report
    /// for such a step without a kept round is ignored, so nobody can reopen
    /// old rounds, fill the open-round limit and switch verdicts off.
    rounds_closed_through: u64,
    /// Every history of the game, oldest first; the last is current. A new
    /// one begins at each recovery, after the last turn logged.
    histories: Vec<History>,
    saves: Saves,
    /// Who sits at the table as the events say, in join order.
    seated: Vec<Seat>,
}

/// A player at a game's table: who, under which name, on which platform.
type Seat = (PlayerId, Text<32>, Platform);

/// A stretch of a game's turns that clients can resume on.
struct History {
    id: u64,
    /// The last turn this history shares with the one before it.
    after_turn: u64,
}

struct Round {
    opened: Instant,
    reports: Vec<Report>,
    verdict: Option<Verdict>,
}

struct LoggedTurn {
    first_event: u64,
    sealed_through: u64,
    frame: Arc<[u8]>,
}

/// Where a turn stream starts, and the sealed turns it starts with.
struct Stream {
    next_turn: u64,
    next_event: u64,
    sealed_through: u64,
    backlog: Vec<Arc<[u8]>>,
}

/// How a seated member comes back.
enum Rejoin {
    Lobby,
    Stream(TurnFeed),
    World,
}

pub(crate) struct Room {
    id: RoomId,
    name: Text<48>,
    /// The name of the rules `ruleset` plays by.
    rules: RulesName,
    owner: PlayerId,
    max_players: u8,
    settings: RoomSettings,
    secrets: RoomSecrets,
    members: Vec<Member>,
    phase: Phase,
    ruleset: Box<dyn Ruleset>,
    tick: Duration,
    metrics: Arc<Metrics>,
    /// Where running games are logged; `None` keeps rooms in memory only.
    data_dir: Option<PathBuf>,
    timeouts: Timeouts,
    log: Option<RoomLog>,
    /// A compaction of the log in progress.
    compaction: Option<Compaction>,
    snapshot_writes: Arc<std::sync::Mutex<PointerWrites>>,
    /// The log's size at which it is compacted next.
    compact_at: u64,
    /// See [`RoomEnv::compact_log_at`].
    compact_log_at: u64,
    /// Since when no member has been connected, while the game runs.
    unattended_since: Option<Instant>,
    password_guard: PasswordGuard,
    /// Company passwords sent with intents ([`SecretBudget`]).
    secret_budget: SecretBudget,
    /// Players the owner removed, who cannot join again.
    banned: BTreeSet<PlayerId>,
    /// Counts this room against the address that created it until it
    /// closes. Restored rooms have none.
    _share: Option<RoomShare>,
    /// The game build and mods of a running game, which players who join
    /// it must match.
    content: Option<ContentFingerprint>,
    /// The manifest behind `content`, to tell players who do not match
    /// how they differ.
    game_content: Option<Arc<Declared>>,
    /// The server's snapshots; `None` if it keeps none, and then nobody can
    /// join a running game.
    snapshots: Option<Arc<Snapshots>>,
    /// In the lobby: the world the owner handed over for the game to start
    /// from, if any.
    start_world: Option<StartWorld>,
    closed: bool,
    /// What the room list shows of the room.
    summary: SharedSummary,
    /// The play style its owner chose; not logged, so a restored room is
    /// co-op.
    competitive: bool,
}

/// What every room of a server shares.
#[derive(Clone)]
pub(crate) struct RoomEnv {
    pub(crate) tick: Duration,
    pub(crate) metrics: Arc<Metrics>,
    pub(crate) data_dir: Option<PathBuf>,
    pub(crate) timeouts: Timeouts,
    pub(crate) snapshots: Option<Arc<Snapshots>>,
    /// A game's log is compacted once it grows past this many bytes.
    pub(crate) compact_log_at: u64,
}

/// Why a room log could not be turned back into a room.
#[derive(Debug, Error)]
pub(crate) enum RecoverError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Log(#[from] LogError),
    #[error("the log has no start record")]
    Empty,
    #[error("the start record is unreadable: {0}")]
    Start(postcard::Error),
    #[error("the log's format version {0} is not supported")]
    Version(u16),
    #[error("the start record's room settings are out of range")]
    Settings,
    #[error("the start record's native settings are missing or do not match its manifest")]
    NativeSettings,
    #[error("the log's native room terms lack a full package identity")]
    NativeIdentity,
    #[error("the room is played by rules \"{0}\", which this server no longer offers")]
    UnknownRules(String),
    #[error("the log's file name does not match its room")]
    Misnamed,
    #[error("turn record {0} is unreadable")]
    Turn(usize),
    #[error("turn record {0} breaks the log's continuity")]
    Continuity(usize),
    #[error("turn record {0} seals implausibly far")]
    Frontier(usize),
    #[error("the log's base contradicts itself")]
    Base,
    #[error("the rules cannot take on the log's base: {0}")]
    Rules(String),
}

/// How often a member's loading progress is shown again while it stays
/// fetching: about two a second.
const LOADING_EVERY: Duration = Duration::from_millis(400);

/// What the room list shows of a room, kept current by the room (on every
/// change its members see) for the directory to read without asking it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Summary {
    pub(crate) name: Text<48>,
    pub(crate) rules: RulesName,
    pub(crate) owner: PlayerId,
    pub(crate) players: u8,
    /// Members connected now. A room nobody is connected to is waiting
    /// out its grace period ([`Timeouts::abandoned`]) for its players, and
    /// the list leaves it out: nobody else has a game to join there.
    pub(crate) connected: u8,
    pub(crate) max_players: u8,
    pub(crate) has_password: bool,
    pub(crate) phase: RoomPhase,
    pub(crate) competitive: bool,
    /// `Some` for a public room: what its owner declared. `None` keeps the
    /// room out of every list. A room restored after a restart is private
    /// until created again: its log keeps no listing and no invite.
    pub(crate) listing: Option<RoomListing>,
    /// Who is in the room, for the operator's admin endpoint (`/rooms`);
    /// never in the room list.
    pub(crate) members: Vec<SummaryMember>,
}

/// A member as the admin endpoint shows it: who, under which name, and
/// whether connected now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SummaryMember {
    pub(crate) player: PlayerId,
    pub(crate) name: Text<32>,
    pub(crate) connected: bool,
}

/// A room's [`Summary`], shared with the directory.
pub(crate) type SharedSummary = Arc<std::sync::Mutex<Summary>>;

pub(crate) struct RoomSpec {
    pub(crate) id: RoomId,
    pub(crate) name: Text<48>,
    pub(crate) max_players: u8,
    pub(crate) settings: RoomSettings,
    pub(crate) secrets: RoomSecrets,
    pub(crate) rules: RulesName,
    pub(crate) ruleset: Box<dyn Ruleset>,
    pub(crate) env: RoomEnv,
    pub(crate) share: RoomShare,
    /// `Some` lists the room publicly.
    pub(crate) listing: Option<RoomListing>,
    /// The play style its owner chose (`CreateRoom::competitive`).
    pub(crate) competitive: bool,
}

impl Room {
    pub(crate) fn new(spec: RoomSpec, owner: NewMember) -> Self {
        let mut room = Self {
            id: spec.id,
            name: spec.name,
            rules: spec.rules,
            owner: owner.player,
            max_players: spec.max_players,
            settings: spec.settings,
            secrets: spec.secrets,
            members: Vec::new(),
            phase: Phase::Lobby,
            ruleset: spec.ruleset,
            tick: spec.env.tick,
            metrics: spec.env.metrics,
            data_dir: spec.env.data_dir,
            timeouts: spec.env.timeouts,
            log: None,
            compaction: None,
            snapshot_writes: Arc::default(),
            compact_at: spec.env.compact_log_at,
            compact_log_at: spec.env.compact_log_at,
            unattended_since: None,
            password_guard: PasswordGuard::new(),
            secret_budget: SecretBudget::new(),
            banned: BTreeSet::new(),
            _share: Some(spec.share),
            content: None,
            game_content: None,
            snapshots: spec.env.snapshots,
            start_world: None,
            closed: false,
            competitive: spec.competitive,
            summary: Arc::new(std::sync::Mutex::new(Summary {
                name: Text::lossy(""),
                rules: Text::lossy(""),
                owner: owner.player,
                players: 0,
                connected: 0,
                max_players: 0,
                has_password: false,
                phase: RoomPhase::Lobby,
                competitive: false,
                listing: spec.listing,
                members: Vec::new(),
            })),
        };
        room.members.push(Member::new(owner));
        room.refresh_summary();
        room
    }

    /// Rebuilds a running room from its log: replays every turn through the
    /// ruleset and seats everyone who had not left, disconnected and ready to
    /// resume. A compacted log starts from its base instead, and replays
    /// only the turns after it. A log whose players had all left is deleted
    /// and gives `None`.
    pub(crate) fn recover(
        path: &Path,
        key: hmac::Key,
        menu: &RulesMenu,
        env: RoomEnv,
    ) -> Result<Option<Self>, RecoverError> {
        let mut reader = RoomLog::read(path)?;
        let first = reader.next_record()?.ok_or(RecoverError::Empty)?;
        // The version leads the start record. Read it alone first: a log of
        // another format has another layout, and is named as such rather
        // than reported unreadable.
        let (version, _) = postcard::take_from_bytes::<u16>(&first).map_err(RecoverError::Start)?;
        if version != persist::FORMAT_VERSION {
            return Err(RecoverError::Version(version));
        }
        let start: StartRecord = postcard::from_bytes(&first).map_err(RecoverError::Start)?;
        if !start.settings.is_valid() {
            return Err(RecoverError::Settings);
        }
        if start.manifest.is_none() && !start.native_mods.is_empty() {
            return Err(RecoverError::NativeSettings);
        }
        if start.manifest.as_ref().is_some_and(|stored| {
            stored
                .clone()
                .manifest()
                .native
                .iter()
                .any(|term| !native_term_version_is_full(term.version.as_str()))
        }) {
            return Err(RecoverError::NativeIdentity);
        }
        // A room keeps its rules for good; the server must still have them.
        let mut ruleset = match menu.find(Some(start.rules.as_str())) {
            Some(choice) => (choice.factory)(),
            None => return Err(RecoverError::UnknownRules(start.rules.as_str().to_owned())),
        };
        let expected = start.id.to_string();
        if path.file_stem() != Some(std::ffi::OsStr::new(&expected)) {
            return Err(RecoverError::Misnamed);
        }
        let restored_content = start
            .manifest
            .clone()
            .map(|stored| {
                let manifest = stored.manifest();
                Declared::restored_with_native(manifest, start.native_mods.clone())
                    .map_err(|_| RecoverError::NativeSettings)
            })
            .transpose()?;
        let mut game = Game::new(start.settings, start.history);
        // Players the owner removed.
        let mut banned = BTreeSet::new();
        let base = start.base.as_ref();
        if let Some(base) = base {
            let mut seats: Vec<PlayerId> = base.seated.iter().map(|(player, ..)| *player).collect();
            seats.sort_unstable();
            seats.dedup();
            if !game.rebase(base)
                || base.banned.len() > MAX_BANNED
                || seats.len() != base.seated.len()
                || seats.len() > usize::from(start.max_players)
            {
                return Err(RecoverError::Base);
            }
            ruleset.restore(&base.rules).map_err(RecoverError::Rules)?;
            banned.extend(base.banned.iter().copied());
        }
        // A compacted log's turns up to its base are only kept for players
        // who resume: the base holds what they did.
        let base_turn = base.map_or(0, |base| base.after_turn);
        let mut based = base.is_none();
        // Ownership passes as the live room passed it: to the earliest
        // remaining player whenever the owner leaves.
        let mut owner = start.owner;
        let mut index = 0;
        loop {
            if !based && game.next_turn > base_turn {
                // The kept turns must end exactly where the base stands.
                if !base.is_some_and(|base| game.meets(base)) {
                    return Err(RecoverError::Continuity(index));
                }
                based = true;
            }
            let Some(frame) = reader.next_record()? else {
                break;
            };
            let turn = match frame
                .get(FRAME_HEADER_LEN..)
                .map(decode_frame::<TurnMessage>)
            {
                Some(Ok(TurnMessage::Turn(turn))) => turn,
                Some(Ok(TurnMessage::Start(marker))) => {
                    // An earlier recovery began a new history here. The
                    // histories of the kept turns are in the base.
                    if !based
                        || marker.next_turn != game.next_turn
                        || marker.next_event != game.next_event
                    {
                        return Err(RecoverError::Continuity(index));
                    }
                    game.begin_history(marker.history);
                    index += 1;
                    continue;
                }
                _ => return Err(RecoverError::Turn(index)),
            };
            if turn.number != game.next_turn || turn.sealed_through < game.sealed_through {
                return Err(RecoverError::Continuity(index));
            }
            if turn.sealed_through > MAX_FRONTIER {
                return Err(RecoverError::Frontier(index));
            }
            let first_event = turn
                .events
                .first()
                .map_or(game.next_event, |event| event.seq);
            for event in &turn.events {
                if event.seq != game.next_event {
                    return Err(RecoverError::Continuity(index));
                }
                game.next_event += 1;
                if !based {
                    continue;
                }
                ruleset.apply(event);
                seat(&mut game.seated, event);
                if let EventBody::PlayerLeft { player, kicked } = &event.body {
                    if *player == owner
                        && let Some((first, ..)) = game.seated.first()
                    {
                        owner = *first;
                    }
                    if *kicked && banned.len() < MAX_BANNED {
                        banned.insert(*player);
                    }
                }
            }
            game.remember(LoggedTurn {
                first_event,
                sealed_through: turn.sealed_through,
                frame: Arc::from(frame),
            });
            game.next_turn += 1;
            game.sealed_through = turn.sealed_through;
            game.speed = turn.speed;
            game.announced_speed = turn.speed;
            index += 1;
        }
        if !based {
            // The log ends among its kept turns.
            return Err(RecoverError::Continuity(index));
        }
        game.pacer.resume_at(game.sealed_through);
        // Every player started with the same content, and players who joined
        // later had to match it.
        let content = match base {
            Some(base) => base.content,
            None => start.members.first().and_then(|member| member.content),
        };
        // The manifest only counts if it is the game's.
        let game_content = restored_content
            .filter(|declared| Some(declared.fingerprint) == content)
            .map(Arc::new);
        let members: Vec<Member> = game
            .seated
            .iter()
            .cloned()
            .map(|(player, name, platform)| Member {
                player,
                name,
                platform,
                banner: None,
                loading: None,
                loading_at: None,
                ready: true,
                content,
                declared: None,
                told_diff: None,
                told_room: None,
                link: None,
                streaming: false,
                pace: Pace::CatchingUp(None),
                advanced: Instant::now(),
                intents: TokenBucket::new(INTENTS_PER_SECOND, INTENT_BURST),
                payload_bytes: TokenBucket::new(PAYLOAD_BYTES_PER_SECOND, PAYLOAD_BURST),
                chats: TokenBucket::new(CHATS_PER_SECOND, CHAT_BURST),
                needs: Needs::Nothing,
                offered: None,
                loaded: None,
                world_epoch: 0,
                load_step: None,
                rebased: None,
                stream_from: 0,
            })
            .collect();
        if members.is_empty() {
            reader.delete()?;
            if let Some(dir) = &env.data_dir {
                Pointer::remove(dir, &start.id);
            }
            return Ok(None);
        }
        if let (Some(dir), Some(snapshots)) = (&env.data_dir, &env.snapshots) {
            game.saves.current = recover_snapshot(dir, &start.id, snapshots, &game);
            game.saves.last_point = game.saves.current.as_ref().map(|agreed| agreed.point);
        }
        // Only now, with the room rebuilt, may a torn final record be cut.
        let mut log = reader.into_log()?;
        // Turns after the last one logged may have reached clients before
        // the crash, and the room will now number different turns the same
        // way. It begins a new history, so nobody is resumed onto turns that
        // differ from the ones they saw.
        game.begin_history(new_history());
        let head = Stream {
            next_turn: game.next_turn,
            next_event: game.next_event,
            sealed_through: game.sealed_through,
            backlog: Vec::new(),
        };
        let marker = TurnMessage::Start(game.turn_start(
            start.id,
            start.settings,
            &start.rules,
            &head,
            None,
        ));
        log.append(&encode_frame(&marker, TURN_MAX_FRAME).map_err(io::Error::other)?)?;
        let owner = if members.iter().any(|member| member.player == owner) {
            owner
        } else {
            members[0].player
        };
        // A log compacted just before the restart is not compacted again at
        // once.
        let compact_at = next_compaction(log.written(), env.compact_log_at);
        let room = Self {
            id: start.id,
            name: start.name,
            rules: start.rules,
            owner,
            max_players: start.max_players,
            settings: start.settings,
            secrets: RoomSecrets {
                key,
                invite_tag: start.invite_tag,
                password_tag: start.password_tag,
            },
            members,
            phase: Phase::Running(Box::new(game)),
            ruleset,
            tick: env.tick,
            metrics: env.metrics,
            data_dir: env.data_dir,
            timeouts: env.timeouts,
            log: Some(log),
            compaction: None,
            snapshot_writes: Arc::default(),
            compact_at,
            compact_log_at: env.compact_log_at,
            // Nobody is connected after a restart; the abandon timeout runs
            // from here.
            unattended_since: None,
            password_guard: PasswordGuard::new(),
            secret_budget: SecretBudget::new(),
            banned,
            _share: None,
            content,
            game_content,
            snapshots: env.snapshots,
            start_world: None,
            closed: false,
            competitive: false,
            // Private after a restart: the log keeps no listing.
            summary: Arc::new(std::sync::Mutex::new(Summary {
                name: Text::lossy(""),
                rules: Text::lossy(""),
                owner,
                players: 0,
                connected: 0,
                max_players: 0,
                has_password: false,
                phase: RoomPhase::Running,
                competitive: false,
                listing: None,
                members: Vec::new(),
            })),
        };
        room.refresh_summary();
        Ok(Some(room))
    }

    pub(crate) fn id(&self) -> RoomId {
        self.id
    }

    /// What the room list shows of this room, kept current.
    pub(crate) fn summary(&self) -> SharedSummary {
        Arc::clone(&self.summary)
    }

    /// Brings [`Self::summary`] up to date with the room.
    fn refresh_summary(&self) {
        let mut summary = self
            .summary
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        summary.name.clone_from(&self.name);
        summary.rules.clone_from(&self.rules);
        summary.owner = self.owner;
        summary.players = u8::try_from(self.members.len()).unwrap_or(u8::MAX);
        summary.connected = self.connected();
        summary.max_players = self.max_players;
        summary.has_password = self.secrets.password_tag.is_some();
        summary.competitive = self.competitive;
        summary.phase = match self.phase {
            Phase::Lobby => RoomPhase::Lobby,
            Phase::Running(_) => RoomPhase::Running,
        };
        summary.members = self
            .members
            .iter()
            .map(|member| SummaryMember {
                player: member.player,
                name: member.name.clone(),
                connected: member.link.is_some(),
            })
            .collect();
    }

    /// The owner of a public room updates what the list shows of it.
    fn describe(&mut self, player: PlayerId, listing: RoomListing) -> Result<(), RequestError> {
        if !self.members.iter().any(|member| member.player == player) {
            return Err(RequestError::NotInRoom);
        }
        if player != self.owner {
            return Err(RequestError::NotOwner);
        }
        let mut summary = self
            .summary
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let shown = summary.listing.as_mut().ok_or(RequestError::NotListed)?;
        *shown = listing;
        Ok(())
    }

    /// The tag of the room's invite, which the directory finds it by.
    pub(crate) fn invite_tag(&self) -> Vec<u8> {
        self.secrets.invite_tag.clone()
    }

    /// The snapshots this room holds in the server's store.
    pub(crate) fn held_snapshots(&self) -> Vec<ManifestId> {
        match &self.phase {
            Phase::Running(game) => game.saves.held(),
            Phase::Lobby => Vec::new(),
        }
    }

    pub(crate) fn view(&self) -> RoomView {
        let reference = self.reference_content();
        RoomView {
            id: self.id,
            name: self.name.clone(),
            rules: self.rules.clone(),
            owner: self.owner,
            max_players: self.max_players,
            has_password: self.secrets.password_tag.is_some(),
            phase: match self.phase {
                Phase::Lobby => RoomPhase::Lobby,
                Phase::Running(_) => RoomPhase::Running,
            },
            settings: self.settings,
            members: self
                .members
                .iter()
                .map(|member| MemberView {
                    player: member.player,
                    name: member.name.clone(),
                    platform: member.platform,
                    ready: member.ready,
                    content: member.content,
                    connected: member.link.is_some(),
                    banner: member.banner.clone(),
                    loading: member.loading,
                    differs: match (&reference, &member.declared) {
                        (Some(room), Some(own)) if room.fingerprint != own.fingerprint => room
                            .manifest
                            .compare(&own.manifest)
                            .map(|diff| diff.status()),
                        _ => None,
                    },
                })
                .collect(),
            competitive: self.competitive,
            start: match self.phase {
                Phase::Lobby => self.start_world.as_ref().map(StartWorld::view),
                Phase::Running(_) => None,
            },
        }
    }

    pub(crate) async fn run(
        mut self,
        mut commands: mpsc::Receiver<RoomCommand>,
        directory: Arc<Directory>,
    ) {
        info!(room = %self.id, "room opened");
        // The owner hears the room's mods they declared, as every member
        // who joins does.
        self.tell_content();
        let mut ticker = tokio::time::interval(self.tick);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        while !self.closed {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.handle(command),
                    None => break,
                },
                _ = ticker.tick() => self.on_tick(Instant::now()),
                done = compaction_done(&mut self.compaction) => self.finish_compaction(done),
            }
        }
        directory.remove(&self.id);
        info!(room = %self.id, "room closed");
    }

    fn handle(&mut self, command: RoomCommand) {
        match command {
            RoomCommand::ResolveInvite {
                player,
                invite,
                password,
                reply,
            } => {
                let result = self.resolve_invite(player, &invite, password.as_ref());
                let _ = reply.send(result);
            }
            RoomCommand::Join {
                member,
                invite,
                password,
                resume,
                scan_plan,
                reply,
            } => {
                let result = self.join(member, &invite, password.as_ref(), resume, scan_plan);
                let joined = result.is_ok();
                let _ = reply.send(result);
                if joined {
                    self.broadcast_view();
                }
            }
            RoomCommand::Leave { player, reply } => {
                let result = self.leave(player, false);
                let _ = reply.send(result);
            }
            RoomCommand::Disconnected { player, link } => self.disconnected(player, link),
            RoomCommand::SetReady {
                player,
                ready,
                reply,
            } => {
                let result = self
                    .in_lobby(player)
                    .map(|member| std::mem::replace(&mut member.ready, ready) != ready);
                self.answer_and_broadcast_if_changed(reply, result);
            }
            RoomCommand::DeclareContent {
                player,
                content,
                reply,
            } => {
                let result = self.declare_content(player, content);
                self.answer_and_broadcast_if_changed(reply, result);
            }
            RoomCommand::Start { player, reply } => {
                let result = self.start(player);
                self.answer_and_broadcast(reply, result);
            }
            RoomCommand::StartWorld {
                player,
                world,
                save,
                reply,
            } => {
                let result = self.ask_for_start_world(player, world, save);
                self.answer_and_broadcast_if_changed(reply, result);
            }
            RoomCommand::ClearStartWorld { player, reply } => {
                let result = self.clear_start_world(player);
                self.answer_and_broadcast_if_changed(reply, result);
            }
            RoomCommand::SetSpeed {
                player,
                speed,
                reply,
            } => {
                let _ = reply.send(self.set_speed(player, speed));
            }
            RoomCommand::Kick {
                player,
                target,
                reply,
            } => {
                let _ = reply.send(self.kick(player, target));
            }
            RoomCommand::Chat {
                player,
                text,
                reply,
            } => {
                let _ = reply.send(self.chat(player, text));
            }
            RoomCommand::SetBanner {
                player,
                banner,
                reply,
            } => {
                let result = self
                    .members
                    .iter_mut()
                    .find(|member| member.player == player)
                    .map(|member| member.banner = banner)
                    .ok_or(RequestError::NotInRoom);
                self.answer_and_broadcast(reply, result);
            }
            RoomCommand::Describe {
                player,
                listing,
                reply,
            } => {
                let _ = reply.send(self.describe(player, listing));
            }
            RoomCommand::IsMember { player, reply } => {
                let member = self.members.iter().any(|m| m.player == player);
                let _ = reply.send(if member {
                    Ok(())
                } else {
                    Err(RequestError::NotInRoom)
                });
            }
            RoomCommand::Intent {
                player,
                client_seq,
                payload,
                secret,
            } => self.intent(player, client_seq, payload, secret.as_ref()),
            RoomCommand::Progress {
                player,
                link,
                step,
                world_epoch,
            } => self.progress(player, link, world_epoch, step),
            RoomCommand::Loaded {
                player,
                link,
                world_epoch,
                step,
            } => self.loaded(player, link, world_epoch, step),
            RoomCommand::Loading { player, stage } => self.loading(player, stage, Instant::now()),
            RoomCommand::Preview { player, preview } => self.preview(player, preview),
            RoomCommand::Checkpoint {
                player,
                link,
                world_epoch,
                step,
                lanes,
            } => self.checkpoint(player, link, world_epoch, step, lanes, Instant::now()),
            RoomCommand::Saved {
                player,
                link,
                world_epoch,
                event,
                lanes,
                world,
            } => self.saved(
                player,
                link,
                (world_epoch, event),
                lanes,
                world,
                Instant::now(),
            ),
            RoomCommand::Fetch {
                player,
                link,
                snapshot,
                reply,
            } => {
                let _ = reply.send(self.fetchable(player, link, &snapshot));
            }
            RoomCommand::Upload {
                player,
                link,
                snapshot,
                reply,
            } => {
                let _ = reply.send(self.upload_starts(player, link, &snapshot));
            }
            RoomCommand::Uploaded {
                player,
                snapshot,
                result,
            } => self.uploaded(player, snapshot, result, Instant::now()),
        }
    }

    fn answer_and_broadcast(&mut self, reply: Reply, result: Result<(), RequestError>) {
        let changed = result.is_ok();
        let _ = reply.send(result);
        if changed {
            self.broadcast_view();
        }
    }

    /// Answers a request that may have changed nothing. Repeating a request
    /// must not make the room broadcast to everyone again.
    fn answer_and_broadcast_if_changed(
        &mut self,
        reply: Reply,
        result: Result<bool, RequestError>,
    ) {
        let changed = result == Ok(true);
        let _ = reply.send(result.map(|_| ()));
        if changed {
            self.broadcast_view();
        }
    }

    fn member_mut(&mut self, player: PlayerId) -> Option<&mut Member> {
        self.members
            .iter_mut()
            .find(|member| member.player == player)
    }

    fn in_lobby(&mut self, player: PlayerId) -> Result<&mut Member, RequestError> {
        if !matches!(self.phase, Phase::Lobby) {
            return Err(RequestError::GameRunning);
        }
        self.member_mut(player).ok_or(RequestError::NotInRoom)
    }

    /// Checks an invite and password without changing room state. Banned
    /// players get the same answer as an invalid invite, as on join.
    fn resolve_invite(
        &self,
        player: PlayerId,
        invite: &Invite,
        password: Option<&Text<64>>,
    ) -> Result<(), RequestError> {
        if self.banned.contains(&player) {
            return Err(RequestError::BadInvite);
        }
        match self.secrets.check(&self.id, invite, password) {
            Admittance::Admitted => Ok(()),
            Admittance::WrongPassword | Admittance::WrongInvite => Err(RequestError::BadInvite),
        }
    }

    fn join(
        &mut self,
        new: NewMember,
        invite: &Invite,
        password: Option<&Text<64>>,
        resume: Option<Resume>,
        scan_plan: tpf3mp_proto::ScanPlan,
    ) -> Result<RoomView, RequestError> {
        let now = Instant::now();
        let content = new.content.as_ref().map(|declared| declared.fingerprint);
        if self.banned.contains(&new.player) {
            return Err(RequestError::BadInvite);
        }
        let seated = self.members.iter().any(|m| m.player == new.player);
        match self.secrets.check(&self.id, invite, password) {
            Admittance::Admitted if seated || self.password_guard.open(now) => {}
            Admittance::WrongPassword if !seated => {
                self.password_guard.failed(now);
                return Err(RequestError::BadInvite);
            }
            _ => return Err(RequestError::BadInvite),
        }
        if !scan_plan.is_valid() || scan_plan != self.settings.scan_plan {
            return Err(RequestError::ScanPlanMismatch);
        }
        if let Some(index) = self.members.iter().position(|m| m.player == new.player) {
            // The same player again, e.g. after reconnecting: the new
            // connection takes over the seat. Validate the resume point
            // before touching the seat, so a failed resume changes nothing.
            let rejoin = match &self.phase {
                Phase::Lobby => Rejoin::Lobby,
                Phase::Running(_) if content.is_some() && content != self.content => {
                    self.tell_refused(&new);
                    return Err(RequestError::ContentMismatch);
                }
                // Without a world of this game, a player receives one; so
                // does one that owes a world the others loaded while it could
                // not take it (it would otherwise resume its own).
                Phase::Running(_)
                    if rejoins_with_world(
                        self.snapshots.is_some(),
                        resume.is_some(),
                        self.members[index].needs,
                    ) =>
                {
                    Rejoin::World
                }
                Phase::Running(game) => {
                    if resume.is_some_and(|resume| {
                        resume.world_epoch == 0
                            || (self.members[index].world_epoch != 0
                                && resume.world_epoch != self.members[index].world_epoch)
                    }) {
                        return Err(RequestError::ResumeUnavailable);
                    }
                    Rejoin::Stream(game.resume_feed(self.id, self.settings, &self.rules, resume)?)
                }
            };
            let member = &mut self.members[index];
            if let Some(old) = member.link.take()
                && old.id != new.link.id
            {
                old.connection
                    .close(close::REPLACED, b"signed in on another connection");
            }
            member.name = new.name;
            member.platform = new.platform;
            member.streaming = false;
            // A new connection has not been told how its content differs.
            member.told_diff = None;
            member.told_room = None;
            if let Some(declared) = new.content {
                member.content = Some(declared.fingerprint);
                member.declared = Some(declared);
            }
            match rejoin {
                Rejoin::Lobby => {}
                Rejoin::Stream(feed) => {
                    member.pace = Pace::CatchingUp(None);
                    member.needs = Needs::Nothing;
                    if let TurnFeed::Open { start, .. } = &feed {
                        member.stream_from = start.next_event;
                        member.world_epoch = start.world_epoch;
                    }
                    member.streaming = new.link.turns.try_send(feed).is_ok();
                }
                Rejoin::World => {
                    member.pace = Pace::CatchingUp(None);
                    member.needs = Needs::World;
                }
            }
            member.link = Some(new.link);
            self.offer_worlds(now);
            return Ok(self.view());
        }
        let full = self.members.len() >= usize::from(self.max_players);
        if let Phase::Running(game) = &mut self.phase {
            // A newcomer to a running game starts from a snapshot, which
            // only a server that keeps them can give, of the same content.
            if self.snapshots.is_none() {
                return Err(RequestError::GameRunning);
            }
            if full {
                return Err(RequestError::RoomFull);
            }
            if content.is_none() || content != self.content {
                self.tell_refused(&new);
                return Err(RequestError::ContentMismatch);
            }
            game.append(
                EventBody::PlayerJoined {
                    player: new.player,
                    name: new.name.clone(),
                    platform: new.platform,
                },
                self.ruleset.as_mut(),
            );
            info!(room = %self.id, player = %new.player, "a player joins the running game");
            metrics::increment(&self.metrics.late_joins);
            let mut member = Member::new(new);
            member.ready = true;
            member.needs = Needs::World;
            self.members.push(member);
            self.offer_worlds(now);
            return Ok(self.view());
        }
        if full {
            return Err(RequestError::RoomFull);
        }
        self.members.push(Member::new(new));
        Ok(self.view())
    }

    fn leave(&mut self, player: PlayerId, kicked: bool) -> Result<(), RequestError> {
        let index = self
            .members
            .iter()
            .position(|member| member.player == player)
            .ok_or(RequestError::NotInRoom)?;
        let member = self.members.remove(index);
        if let Some(link) = &member.link
            && member.streaming
        {
            let _ = link.turns.try_send(TurnFeed::Close);
        }
        if let Phase::Running(game) = &mut self.phase {
            game.append(
                EventBody::PlayerLeft { player, kicked },
                self.ruleset.as_mut(),
            );
        }
        self.after_departure(player);
        Ok(())
    }

    /// A member's build preview, to every other member of the running game
    /// still connected: advisory, so the room keeps, orders and logs none of
    /// it, sends it in a queue of its own and drops it for a member whose
    /// queue is full. Outside a running game, or from a non-member, ignored.
    fn preview(&mut self, player: PlayerId, preview: Option<Payload>) {
        if !matches!(self.phase, Phase::Running(_))
            || !self.members.iter().any(|m| m.player == player)
        {
            return;
        }
        for member in &self.members {
            if member.player == player {
                continue;
            }
            if let Some(link) = &member.link {
                let _ = link.advisory.try_send(ServerMessage::Preview {
                    from: player,
                    preview: preview.clone(),
                });
            }
        }
    }

    /// Passes a member's message to everyone in the room, the sender too, so
    /// every member sees the same conversation.
    /// A member says how far its game is with the room's world. The room
    /// shows it to every member, at most every [`LOADING_EVERY`] while the
    /// stage stays the same (a stage change, and the end, show at once), and
    /// logs none of it.
    fn loading(&mut self, player: PlayerId, stage: Option<LoadingStage>, now: Instant) {
        let Some(member) = self.members.iter_mut().find(|m| m.player == player) else {
            return;
        };
        if member.loading == stage {
            return;
        }
        let same_kind = matches!(
            (member.loading, stage),
            (
                Some(LoadingStage::Fetching { .. }),
                Some(LoadingStage::Fetching { .. })
            )
        );
        if same_kind
            && member
                .loading_at
                .is_some_and(|at| now.saturating_duration_since(at) < LOADING_EVERY)
        {
            return;
        }
        member.loading = stage.map(|stage| match stage {
            LoadingStage::Fetching { percent } => LoadingStage::Fetching {
                percent: percent.min(100),
            },
            other => other,
        });
        member.loading_at = Some(now);
        self.broadcast_view();
    }

    fn chat(&mut self, player: PlayerId, text: ChatText) -> Result<(), RequestError> {
        let now = Instant::now();
        let member = self
            .members
            .iter_mut()
            .find(|member| member.player == player)
            .ok_or(RequestError::NotInRoom)?;
        if !member.chats.take(now, 1) {
            return Err(RequestError::RateLimited);
        }
        for index in 0..self.members.len() {
            self.push(
                index,
                ServerMessage::Chat {
                    from: player,
                    text: text.clone(),
                },
            );
        }
        Ok(())
    }

    /// The owner removes a player for good: the player is told, leaves as if
    /// by choice, and cannot join this room again.
    fn kick(&mut self, by: PlayerId, target: PlayerId) -> Result<(), RequestError> {
        if by != self.owner {
            return Err(RequestError::NotOwner);
        }
        if target == by {
            return Err(RequestError::CannotKickSelf);
        }
        let index = self
            .members
            .iter()
            .position(|member| member.player == target)
            .ok_or(RequestError::NoSuchPlayer)?;
        info!(room = %self.id, player = %target, "the owner removed a player");
        self.push(index, ServerMessage::Kicked);
        if self.banned.len() < MAX_BANNED {
            self.banned.insert(target);
        }
        self.leave(target, true)
    }

    fn disconnected(&mut self, player: PlayerId, link: u64) {
        // The member's link may already be gone, dropped by the room for a
        // full or closed queue.
        let Some(index) = self.members.iter().position(|member| {
            member.player == player && member.link.as_ref().is_none_or(|l| l.id == link)
        }) else {
            // An older connection of a player who has reconnected since.
            return;
        };
        match self.phase {
            Phase::Lobby => {
                // A lobby seat is not held for anyone.
                self.members.remove(index);
                self.after_departure(player);
            }
            Phase::Running(_) => {
                // Running games hold the seat: the player can resume.
                let member = &mut self.members[index];
                member.link = None;
                member.streaming = false;
                member.pace = Pace::CatchingUp(None);
                self.broadcast_view();
            }
        }
    }

    /// Hands ownership on and closes the room when nobody is left.
    fn after_departure(&mut self, player: PlayerId) {
        if self.members.is_empty() {
            // Everyone left: the game is over, and so is its log.
            self.discard_game();
            self.closed = true;
            return;
        }
        if self.owner == player {
            self.owner = self.members[0].player;
            // The world the old owner handed over goes with them: the new
            // owner's is the room's now.
            self.drop_start_world();
        }
        self.broadcast_view();
    }

    /// The owner names the world the game starts from (`Request::StartWorld`):
    /// the room asks the owner to upload it, unless it has it already. It
    /// replaces any world named before, for as long as the room is in its
    /// lobby, and then every member is not ready again: they agreed to the
    /// world before. The same world again only updates what the room shows
    /// of it. Returns whether anything changed.
    fn ask_for_start_world(
        &mut self,
        player: PlayerId,
        world: SavedWorld,
        save: StartSave,
    ) -> Result<bool, RequestError> {
        if player != self.owner {
            return Err(RequestError::NotOwner);
        }
        if matches!(self.phase, Phase::Running(_)) {
            return Err(RequestError::GameRunning);
        }
        if self.snapshots.is_none() {
            return Err(RequestError::WorldsNotKept);
        }
        let Some(index) = self.members.iter().position(|m| m.player == player) else {
            return Err(RequestError::NotInRoom);
        };
        if let Some(named) = &mut self.start_world
            && named.snapshot() == world.snapshot
        {
            let (StartWorld::Asked { save: shown, .. } | StartWorld::Held { save: shown, .. }) =
                named;
            if *shown == save {
                return Ok(false);
            }
            *shown = save;
            self.list_start_save();
            return Ok(true);
        }
        let replacing = self.start_world.is_some();
        self.drop_start_world();
        info!(room = %self.id, snapshot = %world.snapshot, bytes = world.size, replacing, "the owner hands over the world the game starts from");
        self.start_world = Some(StartWorld::Asked {
            world,
            save,
            from: player,
            asked: Instant::now(),
            receiving: false,
        });
        // The first named is the world the room was waiting for; another
        // in its place is not the one the members agreed to.
        if replacing {
            self.unready_all();
        }
        self.list_start_save();
        self.push(
            index,
            ServerMessage::Upload {
                event: 0,
                snapshot: world.snapshot,
            },
        );
        Ok(true)
    }

    /// The owner takes back the world it handed over
    /// (`Request::ClearStartWorld`): the room's world is the owner's game's
    /// again, and every member is not ready again. Returns whether the room
    /// had one.
    fn clear_start_world(&mut self, player: PlayerId) -> Result<bool, RequestError> {
        if !self.members.iter().any(|member| member.player == player) {
            return Err(RequestError::NotInRoom);
        }
        if player != self.owner {
            return Err(RequestError::NotOwner);
        }
        if matches!(self.phase, Phase::Running(_)) {
            return Err(RequestError::GameRunning);
        }
        if self.start_world.is_none() {
            return Ok(false);
        }
        info!(room = %self.id, "the owner takes back the world the game starts from");
        self.drop_start_world();
        self.unready_all();
        self.list_start_save();
        Ok(true)
    }

    /// Every member is not ready: the world they agreed to changed.
    fn unready_all(&mut self) {
        for member in &mut self.members {
            member.ready = false;
        }
    }

    /// A public room's list entry follows the save it starts from: its map
    /// and year, unknown when the save does not say or there is none.
    fn list_start_save(&self) {
        let (map, year) = self
            .start_world
            .as_ref()
            .map_or((Text::lossy(""), 0), |named| {
                (named.save().map.clone(), named.save().year)
            });
        let mut summary = self
            .summary
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(listing) = summary.listing.as_mut() {
            listing.map = map;
            listing.year = year;
        }
    }

    /// Forgets the world the owner handed over, giving back the room's hold
    /// on it.
    fn drop_start_world(&mut self) {
        if let (Some(StartWorld::Held { manifest, .. }), Some(snapshots)) =
            (self.start_world.take(), &self.snapshots)
        {
            release_in_background(Arc::clone(snapshots), vec![manifest.id()]);
        }
    }

    /// Gives up on a world the owner was asked for that did not start
    /// arriving in time. An upload under way has the bulk stream's own
    /// limits.
    fn expire_start_world(&mut self, now: Instant) {
        if let Some(StartWorld::Asked {
            asked,
            receiving: false,
            world,
            ..
        }) = &self.start_world
            && now.saturating_duration_since(*asked) >= UPLOAD_START
        {
            warn!(room = %self.id, snapshot = %world.snapshot, "the owner did not upload the world the game starts from");
            metrics::increment(&self.metrics.uploads_failed);
            self.start_world = None;
            self.list_start_save();
            self.broadcast_view();
        }
    }

    fn start(&mut self, player: PlayerId) -> Result<(), RequestError> {
        if player != self.owner {
            return Err(RequestError::NotOwner);
        }
        if matches!(self.phase, Phase::Running(_)) {
            return Err(RequestError::GameRunning);
        }
        if !self.members.iter().all(|member| member.ready) {
            return Err(RequestError::NotAllReady);
        }
        let first_content = self.members[0].content;
        if first_content.is_none()
            || self
                .members
                .iter()
                .any(|member| member.content != first_content)
        {
            return Err(RequestError::ContentMismatch);
        }
        if self.members[0]
            .declared
            .as_ref()
            .is_some_and(|declared| !native_settings_are_persistable(declared))
        {
            // A native manifest without a matching RoomDeclaration, or with
            // settings that disagree with its term, cannot be told faithfully
            // to members after recovery.
            return Err(RequestError::InvalidContent);
        }
        if matches!(self.start_world, Some(StartWorld::Asked { .. })) {
            return Err(RequestError::StartWorldPending);
        }
        let start_world = match self.start_world.take() {
            Some(StartWorld::Held { manifest, .. }) => Some(manifest),
            _ => None,
        };
        let mut game = Game::new(self.settings, new_history());
        // The log starts by naming everyone at the table, in join order, so a
        // replay of the log alone reproduces membership.
        for member in &self.members {
            game.append(
                EventBody::PlayerJoined {
                    player: member.player,
                    name: member.name.clone(),
                    platform: member.platform,
                },
                self.ruleset.as_mut(),
            );
        }
        let first = Stream {
            next_turn: game.next_turn,
            next_event: 1,
            sealed_through: 0,
            backlog: Vec::new(),
        };
        let open = game.turn_start(self.id, self.settings, &self.rules, &first, None);
        self.content = first_content;
        self.game_content = self.members[0].declared.clone();
        // With snapshots, the owner's world is everyone's. Handed over in
        // the lobby, every player loads it at once, the owner too. Otherwise
        // the owner's game loads it, the room saves it before the first
        // step, and every player loads that save. Worlds generated on each
        // machine could differ between platforms. Everyone still holds the
        // clock until loaded.
        let shared_world = self.snapshots.is_some();
        let from_start_world = start_world.is_some();
        let owner = self.owner;
        for member in &mut self.members {
            member.pace = Pace::Loading;
            member.stream_from = 1;
            member.streaming = false;
            if shared_world && (from_start_world || member.player != owner) {
                member.needs = Needs::World;
                continue;
            }
            member.needs = Needs::Nothing;
            member.world_epoch = open.world_epoch;
            member.load_step = Some(open.sealed_through);
            if let Some(link) = &member.link {
                member.streaming = link
                    .turns
                    .try_send(TurnFeed::Open {
                        start: open.clone(),
                        backlog: Vec::new(),
                    })
                    .is_ok();
            }
        }
        let history = game.history();
        self.phase = Phase::Running(Box::new(game));
        self.open_log(history);
        if let Some(manifest) = start_world {
            // The world stands before the first turn: a stream from it is
            // the game's first. The room's hold on it passes to its slot.
            let point = SavePoint {
                event: 0,
                after_turn: 0,
                history,
                sealed_through: 0,
            };
            if let Phase::Running(game) = &mut self.phase {
                game.saves.last_point = Some(point);
            }
            self.promote(Agreed { manifest, point });
        } else if shared_world && self.members.len() > 1 {
            // Save as soon as the owner has loaded.
            self.offer_worlds(Instant::now());
        }
        info!(room = %self.id, players = self.members.len(), shared_world, from_start_world, "game started");
        metrics::increment(&self.metrics.games_started);
        Ok(())
    }

    /// Starts the log of a game that has just started. A game whose log
    /// cannot be written keeps running, in memory only.
    fn open_log(&mut self, history: u64) {
        let Some(dir) = &self.data_dir else {
            return;
        };
        let start = self.start_record(history, None);
        match RoomLog::create(dir, &start) {
            Ok(log) => self.log = Some(log),
            Err(error) => {
                error!(room = %self.id, %error, "cannot create the room log; the game will not survive a restart");
            }
        }
    }

    /// What the room's log starts with: the room, and for a compacted log
    /// the game's state the log continues from.
    fn start_record(&self, history: u64, base: Option<Base>) -> StartRecord {
        StartRecord {
            version: persist::FORMAT_VERSION,
            history,
            id: self.id,
            name: self.name.clone(),
            rules: self.rules.clone(),
            manifest: self
                .game_content
                .as_ref()
                .map(|declared| persist::StoredManifest::from(&declared.manifest)),
            owner: self.owner,
            max_players: self.max_players,
            settings: self.settings,
            invite_tag: self.secrets.invite_tag.clone(),
            password_tag: self.secrets.password_tag.clone(),
            members: self
                .members
                .iter()
                .map(|member| StartMember {
                    player: member.player,
                    name: member.name.clone(),
                    platform: member.platform,
                    content: member.content,
                })
                .collect(),
            base,
            native_mods: self
                .game_content
                .as_ref()
                .and_then(|declared| declared.room.as_ref())
                .map_or_else(Vec::new, |(room, _)| room.native.clone()),
        }
    }

    /// Starts rewriting a long game's log to begin from where the game
    /// stands now, keeping only the turns players may still resume on.
    /// Recovery then replays only what comes after, and the log never
    /// reaches its size limit. Runs right after sealing, when the rules have
    /// applied exactly the logged events. The rewrite runs on a blocking
    /// thread while the game goes on; see [`Room::finish_compaction`]. Rules
    /// that cannot save their state keep the whole log.
    fn start_compaction(&mut self) {
        if self.compaction.is_some() {
            return;
        }
        let (Some(dir), Some(written)) = (&self.data_dir, self.log.as_ref().map(RoomLog::written))
        else {
            return;
        };
        let Phase::Running(game) = &self.phase else {
            return;
        };
        if written < self.compact_at || !game.pending.is_empty() {
            return;
        }
        let Some(rules) = self.ruleset.save() else {
            self.compact_at = u64::MAX;
            return;
        };
        let base = game.base(rules, self.content, &self.banned);
        let first_history = game.histories.first().map_or(0, |history| history.id);
        let start = self.start_record(first_history, Some(base));
        let frames: Vec<Arc<[u8]>> = game
            .log
            .iter()
            .map(|turn| Arc::clone(&turn.frame))
            .collect();
        let dir = dir.clone();
        let task = tokio::task::spawn_blocking(move || RoomLog::compacted(&dir, &start, &frames));
        self.compaction = Some(Compaction {
            task,
            tail: Vec::new(),
        });
    }

    /// Puts a finished compaction's log in place of the room's, with the
    /// turns sealed since it began. Until then the old log stays the room's,
    /// and stays so if anything fails.
    fn finish_compaction(&mut self, done: Result<io::Result<RoomLog>, tokio::task::JoinError>) {
        let Some(compaction) = self.compaction.take() else {
            return;
        };
        let before = self.log.as_ref().map_or(0, RoomLog::written);
        let installed = match done {
            Ok(Ok(mut log)) => {
                let result = compaction
                    .tail
                    .iter()
                    .try_for_each(|frame| log.append(frame))
                    .and_then(|()| log.install());
                match result {
                    Ok(()) => Ok(log),
                    Err(error) => {
                        log.discard();
                        Err(error)
                    }
                }
            }
            Ok(Err(error)) => Err(error),
            Err(error) => Err(io::Error::other(error)),
        };
        match installed {
            Ok(log) => {
                info!(room = %self.id, before, after = log.written(), "compacted the room log");
                metrics::increment(&self.metrics.logs_compacted);
                self.compact_at = next_compaction(log.written(), self.compact_log_at);
                // The old log closes here, already replaced.
                self.log = Some(log);
            }
            Err(error) => {
                warn!(room = %self.id, %error, "cannot compact the room log; it keeps growing");
                self.compact_at = next_compaction(before, self.compact_log_at);
            }
        }
    }

    /// Drops a running compaction, for a game that is over: its log, once
    /// written, is deleted rather than put in place.
    fn cancel_compaction(&mut self) {
        let Some(compaction) = self.compaction.take() else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Ok(Ok(log)) = compaction.task.await {
                    tokio::task::spawn_blocking(move || log.discard());
                }
            });
        }
    }

    fn set_speed(&mut self, player: PlayerId, speed: Speed) -> Result<(), RequestError> {
        if player != self.owner {
            return Err(RequestError::NotOwner);
        }
        let Phase::Running(game) = &mut self.phase else {
            return Err(RequestError::GameNotRunning);
        };
        if speed > Speed::MAX {
            return Err(RequestError::InvalidSettings);
        }
        game.speed = speed;
        Ok(())
    }

    /// Orders a member's intent. A password sent with it is sealed here
    /// (`RoomSecrets::seal`) and goes no further: the event carries the seal,
    /// never the password, and nothing logs either.
    fn intent(
        &mut self,
        player: PlayerId,
        client_seq: u64,
        payload: Payload,
        secret: Option<&Secret>,
    ) {
        let now = Instant::now();
        let Some(index) = self.members.iter().position(|m| m.player == player) else {
            return;
        };
        let rejection = match &mut self.phase {
            Phase::Lobby => Some(IntentRejection::GameNotRunning),
            Phase::Running(game) => {
                let member = &mut self.members[index];
                if !member.intents.take(now, 1)
                    || !member.payload_bytes.take(now, payload.len() as u64)
                    || (secret.is_some() && !self.secret_budget.take(&player, now))
                {
                    Some(IntentRejection::RateLimited)
                } else if let Err(code) = self.ruleset.validate(&player, &payload) {
                    Some(IntentRejection::Refused { code })
                } else {
                    let seal = secret.map(|secret| self.secrets.seal(&self.id, secret));
                    game.append(
                        EventBody::Command {
                            player,
                            client_seq,
                            payload,
                            seal,
                        },
                        self.ruleset.as_mut(),
                    );
                    None
                }
            }
        };
        if let Some(reason) = rejection {
            metrics::increment(&self.metrics.intents_refused);
            self.push(index, ServerMessage::IntentRejected { client_seq, reason });
        }
    }

    fn loaded(&mut self, player: PlayerId, link: u64, world_epoch: u64, step: u64) {
        let Some(member) = self
            .members
            .iter_mut()
            .find(|m| m.player == player && m.link.as_ref().is_some_and(|l| l.id == link))
        else {
            return;
        };
        if member.world_epoch != world_epoch || member.load_step != Some(step) {
            return;
        }
        member.load_step = None;
        member.loaded = member.offered;
        self.progress(player, link, world_epoch, step);
    }

    fn progress(&mut self, player: PlayerId, link: u64, world_epoch: u64, step: u64) {
        let Phase::Running(game) = &self.phase else {
            return;
        };
        let sealed = game.sealed_through;
        let window = game.pacer.window(game.speed);
        let Some(member) = self
            .members
            .iter_mut()
            .find(|m| m.player == player && m.link.as_ref().is_some_and(|l| l.id == link))
        else {
            return;
        };
        if !member.accepts_report(world_epoch) {
            return;
        }
        if step > sealed {
            // Executing an unsealed step breaks the protocol's central rule.
            warn!(room = %self.id, %player, step, sealed, "progress beyond the frontier");
            if let Some(link) = member.link.take() {
                link.connection
                    .close(close::PROTOCOL_VIOLATION, b"progress beyond the frontier");
            }
            member.streaming = false;
            member.pace = Pace::CatchingUp(None);
            return;
        }
        let pace = match member.pace {
            Pace::Loading => Pace::Following(step),
            Pace::Following(previous) => Pace::Following(previous.max(step)),
            Pace::CatchingUp(_) if step.saturating_add(window) >= sealed => Pace::Following(step),
            Pace::CatchingUp(_) => Pace::CatchingUp(Some(step)),
        };
        let moved = match (member.pace, pace) {
            (Pace::Following(before), Pace::Following(after)) => after > before,
            (_, Pace::Following(_)) => true,
            _ => false,
        };
        if moved {
            member.advanced = Instant::now();
        }
        member.pace = pace;
    }

    /// Stops members who stopped advancing from holding the room: one that
    /// has sealed steps to run but has not moved for the stall timeout, or
    /// one still loading after the load timeout. While the room is paused
    /// nobody is expected to move, so every clock restarts.
    fn demote_stalled(&mut self, now: Instant) {
        let Phase::Running(game) = &self.phase else {
            return;
        };
        let (sealed, paused, started) = (game.sealed_through, game.speed.is_paused(), game.started);
        for member in self.members.iter_mut().filter(|m| holds_clock(m)) {
            if paused {
                member.advanced = now;
                continue;
            }
            let stalled = match member.pace {
                Pace::Loading => now.saturating_duration_since(started) >= self.timeouts.load,
                Pace::Following(step) => {
                    step < sealed
                        && now.saturating_duration_since(member.advanced) >= self.timeouts.stall
                }
                Pace::CatchingUp(_) => false,
            };
            if stalled {
                info!(room = %self.id, player = %member.player, pace = ?member.pace, "a member stopped advancing; the room no longer waits for it");
                metrics::increment(&self.metrics.stalls);
                member.pace = match member.pace {
                    Pace::Following(step) => Pace::CatchingUp(Some(step)),
                    _ => Pace::CatchingUp(None),
                };
            }
        }
    }

    fn checkpoint(
        &mut self,
        player: PlayerId,
        link: u64,
        world_epoch: u64,
        step: u64,
        mut lanes: Vec<LaneDigest>,
        now: Instant,
    ) {
        let interval = u64::from(self.settings.checkpoint_interval);
        let Phase::Running(game) = &mut self.phase else {
            return;
        };
        let Some(order) = self
            .members
            .iter()
            .position(|m| m.player == player && m.link.as_ref().is_some_and(|l| l.id == link))
        else {
            return;
        };
        if !self.members[order].accepts_report(world_epoch) {
            return;
        }
        if step == 0 || !step.is_multiple_of(interval) || step > game.sealed_through {
            debug!(room = %self.id, %player, step, "ignoring a checkpoint that is not due");
            return;
        }
        if step <= game.rounds_closed_through && !game.rounds.contains_key(&step) {
            debug!(room = %self.id, %player, step, "ignoring a checkpoint for a closed round");
            return;
        }
        let open = game
            .rounds
            .values()
            .filter(|round| round.verdict.is_none())
            .count();
        if !game.rounds.contains_key(&step) && open >= MAX_OPEN_ROUNDS {
            return;
        }
        lanes.sort_by_key(|lane| lane.lane);
        lanes.dedup_by_key(|lane| lane.lane);
        let round = game.rounds.entry(step).or_insert_with(|| Round {
            opened: now,
            reports: Vec::new(),
            verdict: None,
        });
        if round.reports.iter().any(|report| report.player == player) {
            return;
        }
        let report = Report {
            player,
            platform: self.members[order].platform,
            order,
            lanes,
            loaded: self.members[order].loaded,
        };
        let notices = match &round.verdict {
            Some(verdict) => {
                // A late report is judged against the decided verdict.
                let diverged = verdict::diverging_lanes(&report.lanes, verdict);
                round.reports.push(report);
                if diverged.is_empty() {
                    Vec::new()
                } else {
                    vec![(player, diverged)]
                }
            }
            None => {
                round.reports.push(report);
                if round.reports.len() >= MIN_VERDICT_REPORTS
                    && round_complete(&self.members, round)
                {
                    decide_round(round)
                } else {
                    Vec::new()
                }
            }
        };
        game.prune_rounds();
        self.announce_divergence(step, notices);
    }

    /// Decides rounds that everyone pacing the room has reported (members may
    /// have left since the last report) or whose deadline has passed. A
    /// round too few members reported by its deadline closes without a
    /// verdict.
    fn decide_waiting_rounds(&mut self, now: Instant) {
        let Phase::Running(game) = &mut self.phase else {
            return;
        };
        let mut decided = Vec::new();
        let mut expired = Vec::new();
        for (step, round) in &mut game.rounds {
            if round.verdict.is_some() {
                continue;
            }
            let overdue = now.saturating_duration_since(round.opened) >= CHECKPOINT_DEADLINE;
            let enough = round.reports.len() >= MIN_VERDICT_REPORTS;
            if enough && (overdue || round_complete(&self.members, round)) {
                decided.push((*step, decide_round(round)));
            } else if overdue {
                expired.push(*step);
            }
        }
        for step in expired {
            game.close_round(step);
        }
        if decided.is_empty() {
            return;
        }
        game.prune_rounds();
        for (step, notices) in decided {
            self.announce_divergence(step, notices);
        }
    }

    /// Tells members their world diverged at `step`, and schedules a rebase
    /// for each: the first world the room agrees on from there on replaces
    /// theirs.
    fn announce_divergence(&mut self, step: u64, notices: Vec<(PlayerId, Vec<u16>)>) {
        if notices.is_empty() {
            return;
        }
        let now = Instant::now();
        let rebasing = self.snapshots.is_some();
        for (player, lanes) in notices {
            warn!(room = %self.id, %player, step, ?lanes, "replica diverged from the verdict");
            metrics::increment(&self.metrics.divergences);
            if let Some(index) = self.members.iter().position(|m| m.player == player) {
                self.push(index, ServerMessage::Diverged { step, lanes });
                let member = &mut self.members[index];
                let rested = member
                    .rebased
                    .is_none_or(|at| now.saturating_duration_since(at) >= REBASE_GAP);
                if rebasing && member.needs == Needs::Nothing && rested {
                    member.needs = Needs::Rebase { after: step };
                }
            }
        }
        self.offer_worlds(now);
    }

    /// A member's report of a save.
    fn saved(
        &mut self,
        player: PlayerId,
        link: u64,
        (world_epoch, event): (u64, u64),
        mut lanes: Vec<LaneDigest>,
        world: Option<SavedWorld>,
        now: Instant,
    ) {
        let Some(order) = self
            .members
            .iter()
            .position(|m| m.player == player && m.link.as_ref().is_some_and(|l| l.id == link))
        else {
            return;
        };
        // Only a member whose stream carried the save can have made it.
        let member = &self.members[order];
        if !member.accepts_report(world_epoch) || member.stream_from > event {
            debug!(room = %self.id, %player, event, "ignoring a report of a save this member never saw");
            return;
        }
        let (platform, loaded) = (member.platform, member.loaded);
        let Phase::Running(game) = &mut self.phase else {
            return;
        };
        // A save decided already, or never made: nothing to add to.
        let Some(round) = game.saves.rounds.get_mut(&event) else {
            debug!(room = %self.id, %player, event, "ignoring a report of no open save");
            return;
        };
        if round
            .reports
            .iter()
            .any(|save| save.report.player == player)
        {
            return;
        }
        lanes.sort_by_key(|lane| lane.lane);
        lanes.dedup_by_key(|lane| lane.lane);
        round.reports.push(SaveReport {
            report: Report {
                player,
                platform,
                order,
                lanes,
                loaded,
            },
            world,
        });
        if save_complete(&self.members, round) {
            self.decide_save(event, now);
        }
    }

    /// The manifest a member's connection may fetch: the snapshot the room
    /// offered that connection, and nothing else, so nobody can probe the
    /// server's store for other rooms' worlds.
    fn fetchable(
        &self,
        player: PlayerId,
        link: u64,
        snapshot: &SnapshotId,
    ) -> Option<Arc<Manifest>> {
        let member = self
            .members
            .iter()
            .find(|m| m.player == player && m.link.as_ref().is_some_and(|l| l.id == link))?;
        if member.offered != Some(*snapshot) {
            return None;
        }
        let Phase::Running(game) = &self.phase else {
            return None;
        };
        game.saves
            .find(snapshot)
            .map(|agreed| Arc::clone(&agreed.manifest))
    }

    /// Whether the room asked this member's connection for this save, and
    /// has not seen it start uploading yet.
    fn upload_starts(&mut self, player: PlayerId, link: u64, snapshot: &SnapshotId) -> bool {
        let linked = self
            .members
            .iter()
            .any(|m| m.player == player && m.link.as_ref().is_some_and(|l| l.id == link));
        let game = match &mut self.phase {
            Phase::Running(game) => game,
            Phase::Lobby => {
                return match &mut self.start_world {
                    Some(StartWorld::Asked {
                        world,
                        from,
                        receiving,
                        ..
                    }) if linked
                        && *from == player
                        && world.snapshot == *snapshot
                        && !*receiving =>
                    {
                        *receiving = true;
                        true
                    }
                    _ => false,
                };
            }
        };
        match &mut game.saves.upload {
            Some(upload)
                if linked
                    && upload.from == player
                    && upload.world.snapshot == *snapshot
                    && !upload.receiving =>
            {
                upload.receiving = true;
                true
            }
            _ => false,
        }
    }

    fn uploaded(
        &mut self,
        player: PlayerId,
        snapshot: SnapshotId,
        result: Result<Arc<Manifest>, String>,
        now: Instant,
    ) {
        if matches!(self.phase, Phase::Lobby) {
            self.start_world_uploaded(player, snapshot, result);
            return;
        }
        let Phase::Running(game) = &mut self.phase else {
            return;
        };
        let Some(upload) = game
            .saves
            .upload
            .take_if(|upload| upload.from == player && upload.world.snapshot == snapshot)
        else {
            // An upload the room gave up on: its hold goes back.
            if let (Ok(manifest), Some(snapshots)) = (&result, &self.snapshots) {
                release_in_background(Arc::clone(snapshots), vec![manifest.id()]);
            }
            return;
        };
        match result {
            Ok(manifest) => {
                info!(room = %self.id, %player, %snapshot, "received a save");
                self.promote(Agreed {
                    manifest,
                    point: upload.point,
                });
            }
            Err(error) => {
                warn!(room = %self.id, %player, %snapshot, %error, "a save's upload failed");
                metrics::increment(&self.metrics.uploads_failed);
                game.saves.failed_uploader(player);
                self.ask_for_upload(upload.point, upload.rest, now);
            }
        }
    }

    /// An upload in the lobby ended: the world the game starts from, if it
    /// is the one the room asked for. The room takes over the hold the
    /// upload took; one it no longer wants goes back.
    fn start_world_uploaded(
        &mut self,
        player: PlayerId,
        snapshot: SnapshotId,
        result: Result<Arc<Manifest>, String>,
    ) {
        let asked = matches!(
            &self.start_world,
            Some(StartWorld::Asked { world, from, .. })
                if *from == player && world.snapshot == snapshot
        );
        match (asked, result) {
            (true, Ok(manifest)) => {
                info!(room = %self.id, %player, %snapshot, bytes = manifest.total_size(), "received the world the game starts from");
                if let Some(StartWorld::Asked { save, .. }) = self.start_world.take() {
                    self.start_world = Some(StartWorld::Held { manifest, save });
                }
                // Everyone sees it arrived: the owner may start.
                self.broadcast_view();
            }
            (true, Err(error)) => {
                warn!(room = %self.id, %player, %snapshot, %error, "the upload of the world the game starts from failed");
                metrics::increment(&self.metrics.uploads_failed);
                self.start_world = None;
                self.list_start_save();
                self.broadcast_view();
            }
            (false, Ok(manifest)) => {
                if let Some(snapshots) = &self.snapshots {
                    release_in_background(Arc::clone(snapshots), vec![manifest.id()]);
                }
            }
            (false, Err(_)) => {}
        }
    }

    /// Asks the first reachable player of `candidates` to upload its save.
    /// A save the room already holds needs no upload.
    fn ask_for_upload(&mut self, point: SavePoint, mut candidates: Candidates, now: Instant) {
        while let Some((player, world)) = candidates.pop_front() {
            let held = match &self.phase {
                Phase::Running(game) => game
                    .saves
                    .find(&world.snapshot)
                    .map(|agreed| Arc::clone(&agreed.manifest)),
                Phase::Lobby => return,
            };
            if let Some(manifest) = held {
                if let Some(snapshots) = &self.snapshots {
                    snapshots.hold(manifest.id());
                }
                self.promote(Agreed { manifest, point });
                return;
            }
            let Some(index) = self
                .members
                .iter()
                .position(|m| m.player == player && m.link.is_some())
            else {
                continue;
            };
            if let Phase::Running(game) = &mut self.phase {
                game.saves.upload = Some(Upload {
                    point,
                    from: player,
                    world,
                    rest: candidates,
                    asked: now,
                    receiving: false,
                });
            }
            self.push(
                index,
                ServerMessage::Upload {
                    event: point.event,
                    snapshot: world.snapshot,
                },
            );
            return;
        }
        debug!(room = %self.id, event = point.event, "nobody could hand this save on");
    }

    /// Makes `agreed` the snapshot players who need a world receive. The one
    /// before stays for downloads that may still run; the one before that
    /// goes. The caller took a hold on `agreed` for its slot; the slot that
    /// goes gives its hold back.
    fn promote(&mut self, agreed: Agreed) {
        let Phase::Running(game) = &mut self.phase else {
            return;
        };
        let id = agreed.id();
        let pointer = Pointer::new(id, agreed.point);
        info!(
            room = %self.id,
            snapshot = %id,
            step = agreed.point.sealed_through,
            bytes = agreed.manifest.total_size(),
            "the room agreed on a snapshot"
        );
        metrics::increment(&self.metrics.snapshots_agreed);
        let dropped = game.saves.previous.take();
        game.saves.previous = game.saves.current.replace(agreed);
        let released: Vec<ManifestId> = dropped
            .map(|agreed| agreed.manifest.id())
            .into_iter()
            .collect();
        if let Some(snapshots) = &self.snapshots {
            let writes = Arc::clone(&self.snapshot_writes);
            writes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .request(pointer, released);
            let snapshots = Arc::clone(snapshots);
            let dir = self.data_dir.clone();
            let room = self.id;
            tokio::task::spawn_blocking(move || {
                let result = writes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .commit(pointer, dir.as_deref(), &room);
                match result {
                    Ok(released) => snapshots.release(&released),
                    Err(error) => {
                        warn!(%room, %error, "cannot record the room's snapshot; preserving the previous restart world")
                    }
                }
            });
        }
        self.offer_worlds(Instant::now());
    }

    /// Sends the current snapshot, with the turns since it, to every member
    /// waiting for a world it serves. Wants a save if someone still waits.
    fn offer_worlds(&mut self, now: Instant) {
        let Phase::Running(game) = &mut self.phase else {
            return;
        };
        let mut waiting = false;
        let mut feeds = Vec::new();
        for (index, member) in self.members.iter().enumerate() {
            let serves = |agreed: &Agreed| match member.needs {
                Needs::Nothing => false,
                Needs::World => true,
                Needs::Rebase { after } => agreed.point.sealed_through >= after,
            };
            if member.needs == Needs::Nothing || member.link.is_none() {
                continue;
            }
            let feed = game
                .saves
                .current
                .as_ref()
                .filter(|agreed| serves(agreed))
                .and_then(|agreed| {
                    game.feed_from(self.id, self.settings, &self.rules, agreed)
                        .ok()
                });
            match feed {
                Some(feed) => feeds.push((index, feed)),
                None => waiting = true,
            }
        }
        // Every game plays from the same loaded world. A world handed to one
        // member goes to every member playing, so all of them load the same
        // save at the same step: a game that kept its own world numbers its
        // entities differently from one that loaded a save, and Transport
        // Fever 3's simulation depends on entity ids (a vehicle leaving a
        // depot starts at an offset made from its id).
        // A member whose game already plays this very world needs nothing:
        // it loaded it and followed the same turns since. One that cannot
        // take it now (away, or not taking turns) owes it, and is handed it
        // when it next can, even when it comes back resuming.
        if !feeds.is_empty() {
            let served: Vec<usize> = feeds.iter().map(|(index, _)| *index).collect();
            let current = game.saves.current.as_ref().map(Agreed::id);
            let frontier = game
                .saves
                .current
                .as_ref()
                .map(|agreed| agreed.point.sealed_through);
            for (index, member) in self.members.iter_mut().enumerate() {
                if served.contains(&index) {
                    continue;
                }
                // Another member can join while this link is still loading
                // the world already sent to it. Keep that offer and its epoch
                // until Loaded; it has not loaded yet, but needs no duplicate.
                // Explicitly requested feeds above still resend on reconnect.
                if member.link.is_some()
                    && member.streaming
                    && member.offered == current
                    && member.load_step.is_some()
                    && member.load_step == frontier
                {
                    continue;
                }
                match refeed(
                    member.loaded,
                    current,
                    member.link.is_some(),
                    member.streaming,
                ) {
                    Refeed::AlreadyOnIt => continue,
                    Refeed::Owes => {
                        member.needs = Needs::World;
                        continue;
                    }
                    Refeed::Feed => {}
                }
                if let Some(feed) = game.saves.current.as_ref().and_then(|agreed| {
                    game.feed_from(self.id, self.settings, &self.rules, agreed)
                        .ok()
                }) {
                    info!(room = %self.id, player = %member.player, "rebasing a replica with the others");
                    feeds.push((index, feed));
                }
            }
        }
        game.saves.wanted = waiting;
        let offered = game.saves.current.as_ref().map(Agreed::id);
        // Every game handed the world, and every one already on it, plays
        // from it now: what the others reported past its step came from
        // worlds nobody plays any more.
        let handed = game
            .saves
            .current
            .as_ref()
            .map(|agreed| (agreed.point.sealed_through, agreed.point.event, agreed.id()));
        if !feeds.is_empty()
            && let Some((step, event, world)) = handed
        {
            let forgotten = game.forget_reports_after(step, event, world, now);
            if forgotten > 0 {
                info!(room = %self.id, step, forgotten, "forgot reports past the world handed out from games that did not play it");
            }
        }
        let mut slow = Vec::new();
        for (index, (feed, stream_from)) in feeds {
            let member = &mut self.members[index];
            let Some(link) = &member.link else {
                continue;
            };
            let was_streaming = member.streaming;
            let TurnFeed::Open { start, .. } = &feed else {
                continue;
            };
            let world_epoch = start.world_epoch;
            let load_step = start.sealed_through;
            member.streaming = link.turns.try_send(feed).is_ok();
            if !member.streaming {
                // A member already playing whose queue is full would get
                // neither its old stream nor the world: as any slow
                // consumer, it is disconnected, and it owes the world, so
                // it is handed it when it reconnects.
                member.needs = Needs::World;
                if was_streaming {
                    slow.push(index);
                }
                continue;
            }
            if matches!(member.needs, Needs::Rebase { .. }) {
                info!(room = %self.id, player = %member.player, "rebasing a replica that diverged");
                metrics::increment(&self.metrics.rebases);
                member.rebased = Some(now);
            }
            member.needs = Needs::Nothing;
            member.offered = offered;
            member.world_epoch = world_epoch;
            member.load_step = Some(load_step);
            // A player still loading the first world keeps holding the
            // clock; anyone else catches up.
            if member.pace != Pace::Loading {
                member.pace = Pace::CatchingUp(None);
            }
            member.stream_from = stream_from;
        }
        for index in slow {
            self.drop_link(index, true);
        }
    }

    /// Advances the game's saves: decides rounds that are complete or out of
    /// time, moves past an upload that never started, and seals a new save
    /// when one is due.
    fn run_saves(&mut self, now: Instant) {
        let Some(snapshots) = self.snapshots.clone() else {
            return;
        };
        let Phase::Running(game) = &mut self.phase else {
            return;
        };
        let ready: Vec<u64> = game
            .saves
            .rounds
            .iter()
            .filter(|(_, round)| {
                now.saturating_duration_since(round.opened) >= SAVE_DEADLINE
                    || save_complete(&self.members, round)
            })
            .map(|(event, _)| *event)
            .collect();
        let stalled = game.saves.upload.take_if(|upload| {
            let waited = now.saturating_duration_since(upload.asked);
            if upload.receiving {
                waited >= snapshots::upload_deadline(upload.world.size)
            } else {
                waited >= UPLOAD_START
            }
        });
        if let Some(upload) = &stalled {
            game.saves.failed_uploader(upload.from);
        }
        for event in ready {
            self.decide_save(event, now);
        }
        if let Some(upload) = stalled {
            warn!(room = %self.id, player = %upload.from, receiving = upload.receiving, "a player asked for its save did not deliver it in time");
            metrics::increment(&self.metrics.uploads_failed);
            if upload.receiving
                && let Some(member) = self.members.iter_mut().find(|m| m.player == upload.from)
                && let Some(link) = member.link.take()
            {
                // Its transfer is still running: end it, which frees the
                // slot it holds. The player reconnects and resumes.
                link.connection
                    .close(close::SLOW_CONSUMER, b"the upload was too slow");
                member.streaming = false;
                member.pace = Pace::CatchingUp(None);
            }
            self.ask_for_upload(upload.point, upload.rest, now);
        }
        self.save_if_due(&snapshots, now);
    }

    /// Decides the save round of the save event `event`, tells members who
    /// diverged, and asks a member whose save agreed to upload it.
    fn decide_save(&mut self, event: u64, now: Instant) {
        let Phase::Running(game) = &mut self.phase else {
            return;
        };
        let Some(round) = game.saves.rounds.remove(&event) else {
            return;
        };
        let (mut candidates, diverged) = snapshots::decide(&round.reports);
        if !round.reports.is_empty() && diverged.len() == round.reports.len() {
            warn!(
                room = %self.id,
                event,
                reports = round.reports.len(),
                "no complete replica matches the save's lane verdict; promoting no snapshot and retaining the last agreed world; owner intervention may be required"
            );
        }
        // Players whose uploads failed go last.
        candidates
            .make_contiguous()
            .sort_by_key(|(player, _)| game.saves.failed.contains(player));
        debug!(
            room = %self.id,
            event,
            reports = round.reports.len(),
            candidates = candidates.len(),
            "decided a save"
        );
        self.announce_divergence(round.point.sealed_through, diverged);
        self.ask_for_upload(round.point, candidates, now);
    }

    /// Seals a save when one is due: the save event alone at the end of a
    /// turn that runs no new steps, so a stream from it starts at a turn
    /// boundary.
    fn save_if_due(&mut self, snapshots: &Snapshots, now: Instant) {
        let playing = self
            .members
            .iter()
            .any(|m| m.streaming && matches!(m.pace, Pace::Following(_)));
        let Phase::Running(game) = &mut self.phase else {
            return;
        };
        let log = (game.sealed_through, game.next_event);
        if !playing
            || !game
                .saves
                .due(now, snapshots.every, snapshots.min_gap, game.started, log)
        {
            return;
        }
        game.append(EventBody::Save, self.ruleset.as_mut());
        let event = game.next_event - 1;
        let frontier = game.sealed_through;
        let frames = match game.seal(frontier) {
            Ok(frames) => frames,
            Err(error) => {
                error!(room = %self.id, %error, "cannot encode a turn; closing the room");
                self.close_all(close::SHUTTING_DOWN, b"internal error");
                return;
            }
        };
        let point = SavePoint {
            event,
            after_turn: game.next_turn - 1,
            history: game.history(),
            sealed_through: frontier,
        };
        game.saves.rounds.insert(
            event,
            SaveRound {
                point,
                opened: now,
                reports: Vec::new(),
            },
        );
        game.saves.last_save = Some(now);
        game.saves.last_point = Some(point);
        info!(room = %self.id, event, step = frontier, "the room saves its world");
        metrics::increment(&self.metrics.saves);
        self.publish(frames);
    }

    /// Closes a running game nobody has been connected to for the abandon
    /// timeout, and deletes its log. Without this, games whose players all
    /// disconnected would hold server resources forever, even across
    /// restarts.
    fn expire_if_abandoned(&mut self, now: Instant) {
        if !matches!(self.phase, Phase::Running(_)) {
            return;
        }
        if self.members.iter().any(|member| member.link.is_some()) {
            self.unattended_since = None;
            return;
        }
        let since = match self.unattended_since {
            Some(since) => since,
            None => {
                info!(
                    room = %self.id,
                    grace_secs = self.timeouts.abandoned.as_secs(),
                    "nobody is connected to the game; it closes unless a player returns in time"
                );
                *self.unattended_since.insert(now)
            }
        };
        if now.saturating_duration_since(since) < self.timeouts.abandoned {
            return;
        }
        info!(room = %self.id, "closing a game nobody returned to");
        metrics::increment(&self.metrics.rooms_abandoned);
        self.discard_game();
        self.closed = true;
    }

    /// Deletes what a finished game kept: its log, its snapshot pointer and
    /// its snapshots.
    fn discard_game(&mut self) {
        self.cancel_compaction();
        let mut writes = self
            .snapshot_writes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pending_releases = writes.cancel();
        if let Some(log) = self.log.take()
            && let Err(error) = log.delete()
        {
            warn!(room = %self.id, %error, "cannot delete the log of a closed room");
        }
        if let Some(dir) = &self.data_dir {
            Pointer::remove(dir, &self.id);
        }
        if let (Some(snapshots), Phase::Running(game)) = (&self.snapshots, &self.phase) {
            release_in_background(
                Arc::clone(snapshots),
                game.saves
                    .held()
                    .into_iter()
                    .chain(pending_releases)
                    .collect(),
            );
        }
        drop(writes);
        self.drop_start_world();
    }

    /// Frees lobby seats whose connection is gone. A lobby seat is not held
    /// for anyone, and a notice of the disconnect can be lost when the
    /// room's queue is full, so this does not wait for one.
    fn sweep_lobby(&mut self) {
        if !matches!(self.phase, Phase::Lobby) {
            return;
        }
        while let Some(index) = self.members.iter().position(|m| m.link.is_none()) {
            let player = self.members.remove(index).player;
            self.after_departure(player);
            if self.closed {
                return;
            }
        }
    }

    /// Members connected now.
    fn connected(&self) -> u8 {
        let connected = self.members.iter().filter(|m| m.link.is_some()).count();
        u8::try_from(connected).unwrap_or(u8::MAX)
    }

    /// Lets go of links whose connection has closed, as the notice of the
    /// disconnect would have ([`Self::disconnected`]). That notice is lost
    /// when the room's queue is full, and a link left behind would count as
    /// a player connected: the room would wait for it forever, and never
    /// close. Keeps the room list's count of connected players current.
    fn sweep_closed_links(&mut self) {
        let mut changed = false;
        for member in &mut self.members {
            let closed = member
                .link
                .as_ref()
                .is_some_and(|link| link.connection.close_reason().is_some());
            if closed {
                debug!(room = %self.id, player = %member.player, "letting go of a closed connection");
                member.link = None;
                member.streaming = false;
                member.pace = Pace::CatchingUp(None);
                changed = true;
            }
        }
        if changed && matches!(self.phase, Phase::Running(_)) {
            self.broadcast_view();
        }
        let connected = self.connected();
        let stale = self
            .summary
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .connected
            != connected;
        if stale {
            self.refresh_summary();
        }
    }

    fn on_tick(&mut self, now: Instant) {
        self.sweep_closed_links();
        self.sweep_lobby();
        self.expire_if_abandoned(now);
        if self.closed {
            return;
        }
        self.expire_start_world(now);
        self.decide_waiting_rounds(now);
        self.demote_stalled(now);
        self.run_saves(now);
        if self.closed {
            return;
        }
        let Phase::Running(game) = &mut self.phase else {
            return;
        };
        let elapsed = now.saturating_duration_since(game.last_tick);
        game.last_tick = now;
        let slowest = slowest_pacer(&self.members);
        let frontier = game
            .pacer
            .advance(elapsed, game.speed, slowest, game.sealed_through);
        if frontier == game.sealed_through
            && game.pending.is_empty()
            && game.speed == game.announced_speed
        {
            return;
        }
        let ordered = game.pending.len() as u64;
        let frames = match game.seal(frontier) {
            Ok(frames) => {
                metrics::add(&self.metrics.events_ordered, ordered);
                metrics::add(&self.metrics.turns_sealed, frames.len() as u64);
                frames
            }
            Err(error) => {
                // Unreachable with the payload budget; fail closed if it
                // happens rather than send a partial log.
                error!(room = %self.id, %error, "cannot encode a turn; closing the room");
                self.close_all(close::SHUTTING_DOWN, b"internal error");
                return;
            }
        };
        self.publish(frames);
    }

    /// Logs sealed turns and sends them to every member with a stream.
    fn publish(&mut self, frames: Vec<Arc<[u8]>>) {
        if let Some(log) = &mut self.log
            && let Err(error) = frames.iter().try_for_each(|frame| log.append(frame))
        {
            error!(room = %self.id, %error, "cannot append to the room log; it stops here");
            self.log = None;
        }
        // A compaction under way gets them too, logged or not: its log may
        // yet be put in place.
        if let Some(compaction) = &mut self.compaction {
            compaction.tail.extend(frames.iter().cloned());
        }
        for frame in frames {
            for index in 0..self.members.len() {
                if self.members[index].streaming {
                    self.send_turn(index, TurnFeed::Frame(Arc::clone(&frame)));
                }
            }
        }
        self.start_compaction();
    }

    /// Sends a control message to a member, disconnecting members whose
    /// queue is full instead of buffering without bound.
    fn push(&mut self, index: usize, message: ServerMessage) {
        let member = &mut self.members[index];
        let Some(link) = &member.link else {
            return;
        };
        if let Err(error) = link.control.try_send(message) {
            self.drop_link(index, matches!(error, mpsc::error::TrySendError::Full(_)));
        }
    }

    fn send_turn(&mut self, index: usize, feed: TurnFeed) {
        let member = &mut self.members[index];
        let Some(link) = &member.link else {
            return;
        };
        if let Err(error) = link.turns.try_send(feed) {
            self.drop_link(index, matches!(error, mpsc::error::TrySendError::Full(_)));
        }
    }

    fn drop_link(&mut self, index: usize, slow: bool) {
        let member = &mut self.members[index];
        if let Some(link) = member.link.take() {
            if slow {
                debug!(room = %self.id, player = %member.player, "disconnecting a slow consumer");
                metrics::increment(&self.metrics.slow_consumers);
                link.connection
                    .close(close::SLOW_CONSUMER, b"not reading fast enough");
            }
            member.streaming = false;
            member.pace = Pace::CatchingUp(None);
        }
    }

    fn broadcast_view(&mut self) {
        self.refresh_summary();
        // The room's mods before the view: on the same ordered stream, so a
        // member asked by this view to get ready again has the mods and
        // settings it agrees to before it can, and before any game starts.
        let reference = self.reference_content();
        self.tell_room_mods(reference.as_deref());
        let view = self.view();
        for index in 0..self.members.len() {
            self.push(index, ServerMessage::RoomUpdate(view.clone()));
        }
        self.tell_content();
    }

    /// The content every member must match: the owner's in the lobby, the
    /// game's once it runs.
    fn reference_content(&self) -> Option<Arc<Declared>> {
        match self.phase {
            Phase::Lobby => self
                .members
                .iter()
                .find(|member| member.player == self.owner)
                .and_then(|owner| owner.declared.clone()),
            Phase::Running(_) => self.game_content.clone(),
        }
    }

    /// Tells each member whose content differs from the room's how, once
    /// for each difference, and tells a member once it no longer differs.
    fn tell_content(&mut self) {
        let reference = self.reference_content();
        self.tell_room_mods(reference.as_deref());
        for index in 0..self.members.len() {
            let member = &self.members[index];
            let differs = match (&reference, &member.declared) {
                (Some(room), Some(own)) if room.fingerprint != own.fingerprint => {
                    Some((room.fingerprint, own.fingerprint))
                }
                _ => None,
            };
            if differs == member.told_diff || member.link.is_none() {
                continue;
            }
            let message = match (&reference, &member.declared, differs) {
                (Some(room), Some(own), Some(_)) => room.manifest.compare(&own.manifest),
                _ => None,
            };
            self.members[index].told_diff = differs;
            self.push(index, ServerMessage::ContentDiff(message));
        }
    }

    /// Tells each connected member the room's mods, `reference`'s, when they
    /// changed since the member was last told: so that a member knows which
    /// mods to have before declaring theirs.
    fn tell_room_mods(&mut self, reference: Option<&Declared>) {
        let told = reference.and_then(Declared::told);
        let digest = told.as_ref().map(|(_, digest)| *digest);
        for index in 0..self.members.len() {
            let member = &self.members[index];
            if member.told_room == Some(digest) || member.link.is_none() {
                continue;
            }
            self.members[index].told_room = Some(digest);
            let message = told.as_ref().map(|(room, _)| Box::new(room.clone()));
            self.push(index, ServerMessage::RoomMods(message));
        }
    }

    /// Tells a player refused for their content the game's mods, so that
    /// they can declare those they have and try again, and how theirs
    /// differ, when both are known.
    fn tell_refused(&self, new: &NewMember) {
        if let Some((room, _)) = self.game_content.as_deref().and_then(Declared::told) {
            // A full queue loses only this list; the refusal follows.
            let _ = new
                .link
                .control
                .try_send(ServerMessage::RoomMods(Some(Box::new(room))));
        }
        if let (Some(room), Some(own)) = (&self.game_content, &new.content) {
            let diff = room.manifest.compare(&own.manifest);
            // A full queue loses only this explanation; the refusal follows.
            let _ = new.link.control.try_send(ServerMessage::ContentDiff(diff));
        }
    }

    /// A member declares what their game runs. In a running game only the
    /// content the game started with is accepted, and it changes nothing.
    fn declare_content(
        &mut self,
        player: PlayerId,
        content: Arc<Declared>,
    ) -> Result<bool, RequestError> {
        let running = matches!(self.phase, Phase::Running(_));
        let owner = self.owner == player;
        if content.room.is_some() && !owner {
            return Err(RequestError::NotOwner);
        }
        let member = self.member_mut(player).ok_or(RequestError::NotInRoom)?;
        if running {
            if content.room.is_some() {
                return Err(RequestError::GameRunning);
            }
            return if member.content == Some(content.fingerprint) {
                member.declared = Some(content);
                Ok(false)
            } else {
                Err(RequestError::GameRunning)
            };
        }
        let digest = |declared: Option<&Arc<Declared>>| {
            declared.and_then(|declared| declared.room.as_ref().map(|(_, digest)| *digest))
        };
        let room_changed = owner && digest(member.declared.as_ref()) != digest(Some(&content));
        let changed = member.content.replace(content.fingerprint) != Some(content.fingerprint);
        member.declared = Some(content);
        if room_changed {
            // The members agreed to the room's mods before: they changed.
            self.unready_all();
        }
        Ok(changed || room_changed)
    }

    fn close_all(&mut self, code: quinn::VarInt, reason: &[u8]) {
        for member in &mut self.members {
            if let Some(link) = member.link.take() {
                link.connection.close(code, reason);
            }
            member.streaming = false;
        }
        self.closed = true;
    }
}

impl Member {
    fn accepts_report(&self, world_epoch: u64) -> bool {
        self.streaming && self.world_epoch == world_epoch && self.load_step.is_none()
    }
    fn new(new: NewMember) -> Self {
        Self {
            player: new.player,
            name: new.name,
            platform: new.platform,
            banner: new.banner,
            loading: None,
            loading_at: None,
            ready: false,
            content: new.content.as_ref().map(|declared| declared.fingerprint),
            declared: new.content,
            told_diff: None,
            told_room: None,
            link: Some(new.link),
            streaming: false,
            pace: Pace::CatchingUp(None),
            advanced: Instant::now(),
            intents: TokenBucket::new(INTENTS_PER_SECOND, INTENT_BURST),
            payload_bytes: TokenBucket::new(PAYLOAD_BYTES_PER_SECOND, PAYLOAD_BURST),
            chats: TokenBucket::new(CHATS_PER_SECOND, CHAT_BURST),
            needs: Needs::Nothing,
            offered: None,
            loaded: None,
            world_epoch: 0,
            load_step: None,
            rebased: None,
            stream_from: 0,
        }
    }
}

/// Whether a member can hold the room's clock: one with a turn stream, or
/// one still waiting for the world the game starts from.
fn holds_clock(member: &Member) -> bool {
    member.streaming || (member.pace == Pace::Loading && member.link.is_some())
}

/// Whether every member pacing the room has reported this round. Members
/// catching up report later and are judged against the verdict then.
fn round_complete(members: &[Member], round: &Round) -> bool {
    members
        .iter()
        .filter(|m| m.streaming && matches!(m.pace, Pace::Loading | Pace::Following(_)))
        .all(|m| round.reports.iter().any(|report| report.player == m.player))
}

/// Whether every member pacing the room whose stream carries this save has
/// reported it. A member whose stream starts after the save never sees it.
fn save_complete(members: &[Member], round: &SaveRound) -> bool {
    members
        .iter()
        .filter(|m| {
            m.streaming
                && m.stream_from <= round.point.event
                && matches!(m.pace, Pace::Loading | Pace::Following(_))
        })
        .all(|m| {
            round
                .reports
                .iter()
                .any(|save| save.report.player == m.player)
        })
}

/// Stops keeping `ids` in the store, off the room's task. The server's
/// next collection deletes their chunks.
fn release_in_background(snapshots: Arc<Snapshots>, ids: Vec<ManifestId>) {
    if ids.is_empty() {
        return;
    }
    tokio::task::spawn_blocking(move || snapshots.release(&ids));
}

/// The snapshot a restored room last agreed on, if its pointer is readable,
/// the store still holds it, and the recovered log can be followed from it.
fn recover_snapshot(
    dir: &Path,
    room: &RoomId,
    snapshots: &Snapshots,
    game: &Game,
) -> Option<Agreed> {
    let pointer = Pointer::read(dir, room)?;
    let manifest = match snapshots
        .store
        .manifest(&bulk::manifest_id(&pointer.snapshot))
    {
        Ok(manifest) => manifest,
        Err(error) => {
            warn!(%room, %error, "a restored room's snapshot is gone");
            return None;
        }
    };
    let agreed = Agreed {
        manifest: Arc::new(manifest),
        point: pointer.point,
    };
    if game.stream_from_save(&agreed.point).is_err() {
        warn!(%room, "a restored room's snapshot does not fit its log");
        return None;
    }
    Some(agreed)
}

fn decide_round(round: &mut Round) -> Vec<(PlayerId, Vec<u16>)> {
    let (verdict, diverged) = verdict::decide(&round.reports);
    round.verdict = Some(verdict);
    diverged
}

/// The lowest progress among members who pace the room, or `None` to hold
/// the clock: while anyone is still loading, or when nobody is following.
/// Only members that can hold the clock count (see [`holds_clock`]).
/// What the room does, as a world goes out, for a member it was not handed
/// to: every game plays from the same loaded world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refeed {
    /// Its game already plays this very world: it loaded it and has run the
    /// same turns since.
    AlreadyOnIt,
    /// It plays another world, or one the room no longer knows (after a
    /// restart): it loads this one with the others.
    Feed,
    /// It cannot take the world now (away, or not taking turns): it owes
    /// it, and is handed it when it next can.
    Owes,
}

fn refeed(
    loaded: Option<SnapshotId>,
    current: Option<SnapshotId>,
    linked: bool,
    streaming: bool,
) -> Refeed {
    if loaded == current {
        Refeed::AlreadyOnIt
    } else if !linked || !streaming {
        Refeed::Owes
    } else {
        Refeed::Feed
    }
}

/// Whether a returning player is handed a world rather than resuming its
/// stream: it brings no turns of this game, or it owes a world the others
/// loaded while it could not take it (resuming, it would play its own).
/// Only a room that keeps worlds hands one.
fn rejoins_with_world(keeps_worlds: bool, resuming: bool, needs: Needs) -> bool {
    keeps_worlds && (!resuming || needs == Needs::World)
}

fn slowest_pacer(members: &[Member]) -> Option<u64> {
    let mut slowest: Option<u64> = None;
    for member in members.iter().filter(|m| holds_clock(m)) {
        match member.pace {
            Pace::Loading => return None,
            Pace::Following(step) => {
                slowest = Some(slowest.map_or(step, |current| current.min(step)));
            }
            Pace::CatchingUp(_) => {}
        }
    }
    slowest
}

/// Keeps `seated` as a game's events say: who sits at the table, in join
/// order.
fn seat(seated: &mut Vec<Seat>, event: &Event) {
    match &event.body {
        EventBody::PlayerLeft { player, .. } => seated.retain(|(seat, ..)| seat != player),
        EventBody::PlayerJoined {
            player,
            name,
            platform,
        } => {
            seated.retain(|(seat, ..)| seat != player);
            seated.push((*player, name.clone(), *platform));
        }
        EventBody::Command { .. } | EventBody::Save => {}
    }
}

/// A room's log being compacted on a blocking thread.
struct Compaction {
    task: tokio::task::JoinHandle<io::Result<RoomLog>>,
    /// Turns sealed since it began, which the new log needs too.
    tail: Vec<Arc<[u8]>>,
}

/// The result of the running compaction, or never without one.
async fn compaction_done(
    compaction: &mut Option<Compaction>,
) -> Result<io::Result<RoomLog>, tokio::task::JoinError> {
    match compaction {
        Some(compaction) => (&mut compaction.task).await,
        None => std::future::pending().await,
    }
}

/// Where a log compacted to `size` bytes is compacted next: once it has
/// grown by `every`, or by its own size if that is more, so a rewrite never
/// writes more than was appended since the last.
fn next_compaction(size: u64, every: u64) -> u64 {
    size.saturating_add(every.max(size))
}

/// A fresh history ID. It is random, so a client can never mistake a later
/// history of the room for one it saw.
fn new_history() -> u64 {
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes).expect("the operating system's random source is available");
    u64::from_le_bytes(bytes).max(1)
}

impl Game {
    fn new(settings: RoomSettings, history: u64) -> Self {
        Self {
            pacer: Pacer::new(
                settings.steps_per_second,
                Duration::from_millis(u64::from(settings.input_delay_ms)),
                MAX_AHEAD,
            ),
            speed: Speed::NORMAL,
            announced_speed: Speed::NORMAL,
            sealed_through: 0,
            next_turn: 1,
            next_event: 1,
            pending: Vec::new(),
            log: VecDeque::new(),
            log_first_turn: 1,
            sealed_before_log: 0,
            log_bytes: 0,
            resume_window: RESUME_WINDOW,
            last_tick: Instant::now(),
            started: Instant::now(),
            rounds: BTreeMap::new(),
            rounds_closed_through: 0,
            histories: vec![History {
                id: history,
                after_turn: 0,
            }],
            saves: Saves::default(),
            seated: Vec::new(),
        }
    }

    /// Stands the game where a compacted log's kept turns begin, with the
    /// base's histories and table. `false` if the base contradicts itself.
    fn rebase(&mut self, base: &Base) -> bool {
        // Counters that could not have grown this far in a game's life: a
        // crafted base must not bring them near overflowing.
        let plausible = base.after_turn < MAX_COUNT && base.next_event < MAX_COUNT;
        let consistent = plausible
            && base.first_turn >= 1
            && base.first_turn <= base.after_turn.saturating_add(1)
            && base.first_event >= 1
            && base.first_event <= base.next_event
            && base.sealed_before <= base.sealed_through
            && base.sealed_through <= MAX_FRONTIER
            && base.histories.first().is_some_and(|&(_, after)| after == 0)
            && base.histories.windows(2).all(|pair| pair[0].1 <= pair[1].1)
            && base
                .histories
                .last()
                .is_some_and(|&(_, after)| after <= base.after_turn);
        if !consistent {
            return false;
        }
        self.next_turn = base.first_turn;
        self.log_first_turn = base.first_turn;
        self.next_event = base.first_event;
        self.sealed_through = base.sealed_before;
        self.sealed_before_log = base.sealed_before;
        self.speed = base.speed;
        self.announced_speed = base.speed;
        self.histories = base
            .histories
            .iter()
            .map(|&(id, after_turn)| History { id, after_turn })
            .collect();
        self.seated.clone_from(&base.seated);
        true
    }

    /// Whether the game stands where `base` says its kept turns end.
    fn meets(&self, base: &Base) -> bool {
        self.next_turn == base.after_turn.saturating_add(1)
            && self.next_event == base.next_event
            && self.sealed_through == base.sealed_through
    }

    /// Where the game stands now, for a compacted log that keeps the turns
    /// of the resume window. Only between turns: nothing may be pending.
    fn base(
        &self,
        rules: Vec<u8>,
        content: Option<ContentFingerprint>,
        banned: &BTreeSet<PlayerId>,
    ) -> Base {
        Base {
            first_turn: self.log_first_turn,
            first_event: self
                .log
                .front()
                .map_or(self.next_event, |turn| turn.first_event),
            sealed_before: self.sealed_before_log,
            after_turn: self.next_turn - 1,
            next_event: self.next_event,
            sealed_through: self.sealed_through,
            speed: self.speed,
            histories: self
                .histories
                .iter()
                .map(|history| (history.id, history.after_turn))
                .collect(),
            content,
            seated: self.seated.clone(),
            banned: banned.iter().copied().collect(),
            rules,
        }
    }

    /// The current history.
    fn history(&self) -> u64 {
        self.histories.last().map_or(0, |history| history.id)
    }

    /// Begins a new history after the last turn so far.
    fn begin_history(&mut self, id: u64) {
        let after_turn = self.next_turn.saturating_sub(1);
        self.histories.push(History { id, after_turn });
    }

    /// The start message of `stream`, which starts from `world` if given.
    fn turn_start(
        &self,
        room: RoomId,
        settings: RoomSettings,
        rules: &RulesName,
        stream: &Stream,
        world: Option<WorldOffer>,
    ) -> TurnStart {
        TurnStart {
            room,
            rules: rules.clone(),
            next_turn: stream.next_turn,
            next_event: stream.next_event,
            sealed_through: stream.sealed_through,
            steps_per_second: settings.steps_per_second,
            checkpoint_interval: settings.checkpoint_interval,
            history: self.history(),
            world,
            world_epoch: self.history(),
        }
    }

    /// Keeps the newest decided rounds for late reports and drops the rest.
    fn prune_rounds(&mut self) {
        let decided: Vec<u64> = self
            .rounds
            .iter()
            .filter(|(_, round)| round.verdict.is_some())
            .map(|(step, _)| *step)
            .collect();
        let excess = decided.len().saturating_sub(DECIDED_ROUNDS_KEPT);
        for step in &decided[..excess] {
            self.close_round(*step);
        }
    }

    /// Forgets every checkpoint and save report made past a snapshot by a
    /// game that did not play from it. The room has just handed the snapshot
    /// `world`, saved after `step` (its event `event`), so every game of the
    /// room plays from it, or owes it (see `offer_worlds`). A game that keeps
    /// its own world numbers its entities otherwise than one that loaded the
    /// save, and the simulation depends on the ids (a vehicle bought after
    /// the save starts at an offset made from its id), so the replayed steps
    /// may rightly differ from what was reported before the load. Kept, those
    /// reports made the room ignore a reloaded game's report of the same step
    /// as a repeat, and judge a game that had not reported it yet against a
    /// verdict the old worlds decided: a false divergence, and a rebase.
    /// A round that loses reports is decided again from the ones it keeps,
    /// with a fresh deadline; one that loses all of them is dropped, and a
    /// report opens it again unless a later round closed since (then that
    /// step goes unchecked, as any closed round's). Returns how many
    /// reports it forgot.
    fn forget_reports_after(
        &mut self,
        step: u64,
        event: u64,
        world: SnapshotId,
        now: Instant,
    ) -> usize {
        let current = Some(world);
        let mut forgotten = 0;
        let mut emptied = Vec::new();
        for (&at, round) in self.rounds.range_mut(step.saturating_add(1)..) {
            let before = round.reports.len();
            round.reports.retain(|report| report.loaded == current);
            if round.reports.len() == before {
                continue;
            }
            forgotten += before - round.reports.len();
            if round.reports.is_empty() {
                emptied.push(at);
            } else {
                round.verdict = None;
                round.opened = now;
            }
        }
        for at in emptied {
            self.rounds.remove(&at);
        }
        for (_, round) in self.saves.rounds.range_mut(event.saturating_add(1)..) {
            let before = round.reports.len();
            round.reports.retain(|save| save.report.loaded == current);
            forgotten += before - round.reports.len();
        }
        forgotten
    }

    fn close_round(&mut self, step: u64) {
        self.rounds.remove(&step);
        self.rounds_closed_through = self.rounds_closed_through.max(step);
    }

    /// Orders an event: the next sequence number, and the first step no
    /// member can have executed yet.
    fn append(&mut self, body: EventBody, ruleset: &mut dyn Ruleset) {
        let event = Event {
            seq: self.next_event,
            step: self.sealed_through + 1,
            body,
        };
        self.next_event += 1;
        ruleset.apply(&event);
        seat(&mut self.seated, &event);
        self.pending.push(event);
    }

    /// Seals up to `frontier`, returning the encoded turns. Pending events
    /// are split over several turns if needed; only the last one moves the
    /// frontier, so every turn is a valid prefix of the log.
    fn seal(&mut self, frontier: u64) -> Result<Vec<Arc<[u8]>>, tpf3mp_proto::FrameError> {
        let mut batches: Vec<Vec<Event>> = vec![Vec::new()];
        let mut budget = 0;
        for event in std::mem::take(&mut self.pending) {
            let size = match &event.body {
                EventBody::Command { payload, .. } => payload.len(),
                _ => 0,
            };
            if budget + size > TURN_PAYLOAD_BUDGET && !batches.last().is_some_and(Vec::is_empty) {
                batches.push(Vec::new());
                budget = 0;
            }
            budget += size;
            if let Some(batch) = batches.last_mut() {
                batch.push(event);
            }
        }
        let last = batches.len() - 1;
        let mut frames = Vec::with_capacity(batches.len());
        for (index, events) in batches.into_iter().enumerate() {
            let first_event = events.first().map_or(self.next_event, |event| event.seq);
            let sealed_through = if index == last {
                frontier
            } else {
                self.sealed_through
            };
            let turn = Turn {
                number: self.next_turn,
                sealed_through,
                speed: self.speed,
                events,
            };
            let frame: Arc<[u8]> = encode_frame(&TurnMessage::Turn(turn), TURN_MAX_FRAME)?.into();
            self.remember(LoggedTurn {
                first_event,
                sealed_through,
                frame: Arc::clone(&frame),
            });
            self.next_turn += 1;
            frames.push(frame);
        }
        self.sealed_through = frontier;
        self.announced_speed = self.speed;
        Ok(frames)
    }

    /// Keeps a sealed turn for resuming.
    fn remember(&mut self, turn: LoggedTurn) {
        self.log_bytes += turn.frame.len();
        self.log.push_back(turn);
        self.trim();
    }

    /// Drops the oldest turns beyond the window's turns or bytes. The newest
    /// turn always stays.
    fn trim(&mut self) {
        while self.log.len() > self.resume_window
            || (self.log_bytes > RESUME_WINDOW_BYTES && self.log.len() > 1)
        {
            let Some(oldest) = self.log.pop_front() else {
                break;
            };
            self.log_bytes -= oldest.frame.len();
            self.log_first_turn += 1;
            self.sealed_before_log = oldest.sealed_through;
        }
    }

    /// The turn feed for a member resuming at `resume` (or from the first
    /// turn). Refused: resuming before the window, after a turn that does
    /// not exist yet, on a history this game never had, or past the point
    /// where the client's history and the current one part.
    fn resume_feed(
        &self,
        room: RoomId,
        settings: RoomSettings,
        rules: &RulesName,
        resume: Option<Resume>,
    ) -> Result<TurnFeed, RequestError> {
        let stream = self.stream_after(resume)?;
        let mut start = self.turn_start(room, settings, rules, &stream, None);
        start.world_epoch = resume.map_or(self.history(), |resume| resume.world_epoch);
        Ok(TurnFeed::Open {
            start,
            backlog: stream.backlog,
        })
    }

    /// The turn feed that starts from an agreed snapshot, and the first
    /// event it carries.
    fn feed_from(
        &self,
        room: RoomId,
        settings: RoomSettings,
        rules: &RulesName,
        agreed: &Agreed,
    ) -> Result<(TurnFeed, u64), RequestError> {
        let stream = self.stream_from_save(&agreed.point)?;
        let next_event = stream.next_event;
        let mut start = self.turn_start(room, settings, rules, &stream, Some(agreed.offer()));
        start.world_epoch = new_history();
        let feed = TurnFeed::Open {
            start,
            backlog: stream.backlog,
        };
        Ok((feed, next_event))
    }

    /// The stream after a save: it must be in the resume window, and the
    /// log must still say what the save says about where it stands.
    fn stream_from_save(&self, point: &SavePoint) -> Result<Stream, RequestError> {
        let stream = self.stream_after(Some(Resume {
            world_epoch: 1,
            after_turn: point.after_turn,
            history: point.history,
        }))?;
        if stream.next_event != point.event.saturating_add(1)
            || stream.sealed_through != point.sealed_through
        {
            return Err(RequestError::ResumeUnavailable);
        }
        Ok(stream)
    }

    /// Where a stream after `resume` starts, with the turns it starts with.
    fn stream_after(&self, resume: Option<Resume>) -> Result<Stream, RequestError> {
        let from = match resume {
            None => 1,
            Some(resume) => {
                let index = self
                    .histories
                    .iter()
                    .position(|history| history.id == resume.history)
                    .ok_or(RequestError::ResumeUnavailable)?;
                if self
                    .histories
                    .get(index + 1)
                    .is_some_and(|next| resume.after_turn > next.after_turn)
                {
                    return Err(RequestError::ResumeUnavailable);
                }
                resume.after_turn.saturating_add(1)
            }
        };
        if from > self.next_turn || from < self.log_first_turn {
            return Err(RequestError::ResumeUnavailable);
        }
        let skip = usize::try_from(from - self.log_first_turn)
            .map_err(|_| RequestError::ResumeUnavailable)?;
        let backlog: Vec<&LoggedTurn> = self.log.range(skip..).collect();
        // The first event the resumed stream will carry: from the backlog,
        // else from the events waiting for the next turn.
        let next_event = backlog.first().map_or_else(
            || {
                self.pending
                    .first()
                    .map_or(self.next_event, |event| event.seq)
            },
            |turn| turn.first_event,
        );
        // The frontier of the turn before the stream's first.
        let sealed_through = match skip.checked_sub(1) {
            None => self.sealed_before_log,
            Some(before) => self
                .log
                .get(before)
                .map_or(self.sealed_through, |turn| turn.sealed_through),
        };
        Ok(Stream {
            next_turn: from,
            next_event,
            sealed_through,
            backlog: backlog.iter().map(|turn| Arc::clone(&turn.frame)).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::ruleset::{NATIVE, RulesChoice};
    use tpf3mp_proto::{FixedBytes, ModRef, NativeSetting, NativeValue};

    fn native() -> RulesName {
        RulesName::new(NATIVE).unwrap()
    }

    fn native_terms() -> (ContentManifest, Vec<NativeMod>) {
        let settings = vec![NativeSetting {
            key: Text::new("radius").unwrap(),
            value: NativeValue::Int(37),
        }];
        let settings_map = BTreeMap::from([("radius".to_owned(), Setting::Int(37))]);
        let version = Text::new(format!(
            "1.2.3+{}{}",
            &settings_digest(&settings_map)[..10],
            "a".repeat(64)
        ))
        .unwrap();
        let term = ModRef {
            id: Text::new("native:bigmap").unwrap(),
            version: version.clone(),
        };
        let manifest = ContentManifest::new(
            Text::new("40420").unwrap(),
            vec![ModRef {
                id: Text::new("tpf3mp_1").unwrap(),
                version: Text::new("1.0.0").unwrap(),
            }],
        )
        .with_native(vec![term]);
        let native = NativeMod {
            id: Text::new("bigmap").unwrap(),
            version,
            name: Text::new("Big Maps").unwrap(),
            settings,
        };
        (manifest, vec![native])
    }

    fn assert_native_terms(room: &Room, expected: &[NativeMod]) {
        let declared = room
            .game_content
            .as_deref()
            .expect("the original manifest matches the game content");
        let (told, _) = declared.told().expect("native room terms are retained");
        assert_eq!(told.native, expected);
        assert_eq!(told.manifest(), declared.manifest);
    }

    fn write_record(path: &Path, payload: &[u8], records: &[Vec<u8>]) {
        let mut bytes = Vec::new();
        for payload in std::iter::once(payload).chain(records.iter().map(Vec::as_slice)) {
            bytes.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
            bytes.extend_from_slice(&crc32fast::hash(payload).to_le_bytes());
            bytes.extend_from_slice(payload);
        }
        std::fs::write(path, bytes).unwrap();
    }

    /// After a restart, a running game's room tells its mods again from the
    /// manifest its log kept, each by its id, so a newcomer learns the
    /// whole list; one that is no room's list tells none.
    #[test]
    fn a_restored_game_tells_its_mods_from_its_manifest() {
        let listed = |id: &str| tpf3mp_proto::ModRef {
            id: Text::new(id).unwrap(),
            version: Text::new("1").unwrap(),
        };
        let manifest = ContentManifest::new(
            Text::new("40408").unwrap(),
            vec![listed("signals"), listed("trees"), listed("tpf3mp_1")],
        );
        let restored = Declared::restored_with_native(manifest.clone(), Vec::new()).unwrap();
        assert_eq!(restored.fingerprint, manifest.fingerprint());
        let (room, _) = restored.told().expect("the room's mods told again");
        let ids: Vec<&str> = room.mods.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["signals", "trees", "tpf3mp_1"]);
        assert_eq!(room.manifest(), manifest);
        // Without TPF3-MP last it is no room's list: none told, as before.
        let other = ContentManifest::new(Text::new("40408").unwrap(), vec![listed("signals")]);
        assert!(
            Declared::restored_with_native(other, Vec::new())
                .unwrap()
                .told()
                .is_none()
        );
    }

    #[test]
    fn restored_native_room_terms_keep_exact_settings_and_reject_missing_settings() {
        let (manifest, native) = native_terms();
        let restored = Declared::restored_with_native(manifest.clone(), native.clone()).unwrap();
        assert_eq!(restored.fingerprint, manifest.fingerprint());
        let (told, _) = restored.told().expect("new members are told native terms");
        assert_eq!(told.native, native);
        assert_eq!(told.manifest(), manifest);

        let (manifest, _) = native_terms();
        assert!(Declared::restored_with_native(manifest, Vec::new()).is_err());
    }

    #[test]
    fn a_native_game_needs_room_settings_before_it_can_start_and_be_recovered() {
        let ordinary = ContentManifest::new(
            Text::new("40420").unwrap(),
            vec![ModRef {
                id: Text::new("tpf3mp_1").unwrap(),
                version: Text::new("1.0.0").unwrap(),
            }],
        );
        assert!(native_settings_are_persistable(&Declared::new(ordinary)));

        let (manifest, native) = native_terms();
        assert!(!native_settings_are_persistable(&Declared::new(
            manifest.clone()
        )));

        let declaration = RoomDeclaration {
            manifest,
            room: tpf3mp_proto::RoomConfig {
                info: vec![tpf3mp_proto::ModInfo {
                    name: Text::new("TPF3-MP").unwrap(),
                    source: Text::new("").unwrap(),
                    modio: None,
                }],
                params: Vec::new(),
                native,
            },
        };
        declaration.validate().unwrap();
        assert!(native_settings_are_persistable(&Declared::with_room(
            declaration
        )));
    }

    #[test]
    fn a_native_game_with_a_stale_settings_digest_cannot_open_a_log() {
        let dir =
            std::env::temp_dir().join(format!("tpf3mp-start-native-digest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let id = RoomId(FixedBytes([0x86; 16]));
        let owner = player(1);
        let (manifest, mut native_mods) = native_terms();
        native_mods[0].settings[0].value = NativeValue::Int(38);
        let declaration = RoomDeclaration {
            manifest,
            room: tpf3mp_proto::RoomConfig {
                info: vec![tpf3mp_proto::ModInfo {
                    name: Text::new("Big Maps").unwrap(),
                    source: Text::new("").unwrap(),
                    modio: None,
                }],
                params: Vec::new(),
                native: native_mods,
            },
        };
        declaration.validate().unwrap();
        let declared = Arc::new(Declared::with_room(declaration));
        let mut member = test_member();
        member.player = owner;
        member.ready = true;
        member.content = Some(declared.fingerprint);
        member.declared = Some(declared);
        let mut room = Room {
            id,
            name: Text::new("digest guard").unwrap(),
            rules: native(),
            owner,
            max_players: 4,
            settings: RoomSettings::DEFAULT,
            secrets: RoomSecrets {
                key: hmac::Key::new(hmac::HMAC_SHA256, &[0; 32]),
                invite_tag: vec![0; 32],
                password_tag: None,
            },
            members: vec![member],
            phase: Phase::Lobby,
            ruleset: Box::new(Recorder::default()),
            tick: Duration::from_millis(100),
            metrics: Arc::new(Metrics::default()),
            data_dir: Some(dir.clone()),
            timeouts: Timeouts {
                stall: Duration::from_secs(20),
                load: Duration::from_secs(300),
                abandoned: Duration::from_secs(600),
            },
            log: None,
            compaction: None,
            snapshot_writes: Arc::default(),
            compact_at: u64::MAX,
            compact_log_at: u64::MAX,
            unattended_since: None,
            password_guard: PasswordGuard::new(),
            secret_budget: SecretBudget::new(),
            banned: BTreeSet::new(),
            _share: None,
            content: None,
            game_content: None,
            snapshots: None,
            start_world: None,
            closed: false,
            summary: Arc::new(std::sync::Mutex::new(Summary {
                name: Text::new("digest guard").unwrap(),
                rules: native(),
                owner,
                players: 1,
                connected: 0,
                max_players: 4,
                has_password: false,
                phase: RoomPhase::Lobby,
                competitive: false,
                listing: None,
                members: Vec::new(),
            })),
            competitive: false,
        };
        let path = RoomLog::path_for(&dir, &id);

        let refused = matches!(room.start(owner), Err(RequestError::InvalidContent));
        let no_log_file = !path.exists();
        let no_open_log = room.log.is_none();
        let still_in_lobby = matches!(room.phase, Phase::Lobby);
        drop(room);
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(refused, "stale native settings must refuse game start");
        assert!(no_log_file, "refused start must not create a room log");
        assert!(no_open_log, "refused start must not open a room log");
        assert!(still_in_lobby, "refused start leaves the room in its lobby");
    }

    /// The native rules, recording what they apply.
    fn recorder_menu() -> RulesMenu {
        RulesMenu::single(RulesChoice {
            name: native(),
            description: Text::new("records").unwrap(),
            factory: Arc::new(|| Box::new(Recorder::default())),
        })
    }

    #[test]
    fn resuming_is_limited_to_the_window() {
        let room = RoomId(FixedBytes([0; 16]));
        let settings = RoomSettings::DEFAULT;
        let mut game = Game::new(settings, 1);
        game.resume_window = 3;
        for frontier in 1..=5 {
            // Five empty turns, numbered 1 to 5; the window keeps 3 to 5.
            game.seal(frontier).unwrap();
        }
        let feed = |after_turn: Option<u64>| {
            let resume = after_turn.map(|after_turn| Resume {
                world_epoch: 1,
                after_turn,
                history: 1,
            });
            game.resume_feed(room, settings, &native(), resume)
        };
        assert!(matches!(feed(None), Err(RequestError::ResumeUnavailable)));
        assert!(matches!(
            feed(Some(1)),
            Err(RequestError::ResumeUnavailable)
        ));
        let Ok(TurnFeed::Open { start, backlog }) = feed(Some(2)) else {
            panic!("the window starts at turn 3");
        };
        assert_eq!((start.next_turn, backlog.len()), (3, 3));
        let Ok(TurnFeed::Open { start, backlog }) = feed(Some(5)) else {
            panic!("resuming at the head needs no backlog");
        };
        assert_eq!((start.next_turn, backlog.len()), (6, 0));
        assert!(matches!(
            feed(Some(6)),
            Err(RequestError::ResumeUnavailable)
        ));
    }

    #[test]
    fn resuming_never_crosses_into_turns_the_client_did_not_see() {
        let room = RoomId(FixedBytes([0; 16]));
        let settings = RoomSettings::DEFAULT;
        let mut game = Game::new(settings, 1);
        for frontier in 1..=4 {
            game.seal(frontier).unwrap();
        }
        // A crash lost the turns after 4. The restored room numbers its new
        // turns 5 and on too, in history 2.
        game.begin_history(2);
        for frontier in 5..=8 {
            game.seal(frontier).unwrap();
        }
        let feed = |after_turn, history| {
            game.resume_feed(
                room,
                settings,
                &native(),
                Some(Resume {
                    after_turn,
                    history,
                    world_epoch: 1,
                }),
            )
        };
        let Ok(TurnFeed::Open { start, .. }) = feed(4, 1) else {
            panic!("turns 1 to 4 are the same in both histories");
        };
        assert_eq!(start.history, 2, "the stream names the current history");
        assert!(
            matches!(feed(6, 1), Err(RequestError::ResumeUnavailable)),
            "turn 6 of history 1 was lost"
        );
        assert!(feed(6, 2).is_ok());
        assert!(
            matches!(feed(2, 9), Err(RequestError::ResumeUnavailable)),
            "a history the game never had"
        );
    }

    #[test]
    fn the_resume_window_is_bounded_in_bytes_too() {
        let mut game = Game::new(RoomSettings::DEFAULT, 1);
        let big = RESUME_WINDOW_BYTES / 4 + 1;
        for _ in 0..8 {
            game.remember(LoggedTurn {
                first_event: 1,
                sealed_through: 0,
                frame: vec![0; big].into(),
            });
        }
        assert_eq!(game.log.len(), 3, "four would exceed the bytes");
        assert_eq!(game.log_first_turn, 6);
        assert_eq!(game.log_bytes, 3 * big);
    }

    #[test]
    fn slowest_pacer_holds_for_loading_members_and_ignores_catching_up() {
        let pace = |pace| Member {
            pace,
            streaming: true,
            ..test_member()
        };
        assert_eq!(slowest_pacer(&[]), None);
        assert_eq!(
            slowest_pacer(&[pace(Pace::Following(9)), pace(Pace::Following(4))]),
            Some(4)
        );
        assert_eq!(
            slowest_pacer(&[pace(Pace::Following(9)), pace(Pace::Loading)]),
            None
        );
        assert_eq!(
            slowest_pacer(&[pace(Pace::Following(9)), pace(Pace::CatchingUp(Some(1)))]),
            Some(9)
        );
    }

    /// Rules whose state is every command's payload, in order, so a replay
    /// that skips or repeats one shows.
    #[derive(Default)]
    struct Recorder(Vec<u8>);

    impl Ruleset for Recorder {
        fn validate(&self, _player: &PlayerId, _payload: &Payload) -> Result<(), u16> {
            Ok(())
        }

        fn apply(&mut self, event: &Event) {
            if let EventBody::Command { payload, .. } = &event.body {
                self.0.extend_from_slice(payload.as_bytes());
            }
        }

        fn save(&self) -> Option<Vec<u8>> {
            Some(self.0.clone())
        }

        fn restore(&mut self, state: &[u8]) -> Result<(), String> {
            self.0 = state.to_vec();
            Ok(())
        }
    }

    fn player(n: u8) -> PlayerId {
        PlayerId(FixedBytes([n; 32]))
    }

    fn joined(n: u8) -> EventBody {
        EventBody::PlayerJoined {
            player: player(n),
            name: Text::new(format!("p{n}")).unwrap(),
            platform: Platform::current(),
        }
    }

    fn command(n: u8, byte: u8) -> EventBody {
        EventBody::Command {
            player: player(n),
            client_seq: u64::from(byte),
            payload: Payload::new(vec![byte]).unwrap(),
            seal: None,
        }
    }

    fn recover_room(path: &Path, dir: &Path) -> Room {
        recover_by(path, dir, &recorder_menu()).unwrap().unwrap()
    }

    fn recover_by(path: &Path, dir: &Path, menu: &RulesMenu) -> Result<Option<Room>, RecoverError> {
        let env = RoomEnv {
            tick: Duration::from_millis(100),
            metrics: Arc::new(Metrics::default()),
            data_dir: Some(dir.to_owned()),
            timeouts: Timeouts {
                stall: Duration::from_secs(20),
                load: Duration::from_secs(300),
                abandoned: Duration::from_secs(600),
            },
            snapshots: None,
            compact_log_at: u64::MAX,
        };
        let key = hmac::Key::new(hmac::HMAC_SHA256, &[0; 32]);
        Room::recover(path, key, menu, env)
    }

    fn recovery_start(id: RoomId, version: u16) -> StartRecord {
        StartRecord {
            version,
            history: 1,
            id,
            name: Text::new("recovery fixture").unwrap(),
            rules: native(),
            manifest: None,
            owner: player(1),
            max_players: 4,
            settings: RoomSettings::DEFAULT,
            invite_tag: vec![0; 32],
            password_tag: None,
            members: Vec::new(),
            base: None,
            native_mods: Vec::new(),
        }
    }

    #[test]
    fn old_formats_are_rejected_before_decoding_or_changing_any_bytes() {
        let dir = tempfile::tempdir().unwrap();
        for version in [8_u16, 9, 10] {
            let id = RoomId(FixedBytes([version as u8; 16]));
            // Opaque, deliberately incompatible remainder: a format prefix is
            // enough to refuse both upstream and ReedME's ambiguous format 9.
            let payload = postcard::to_stdvec(&(version, "an incompatible layout")).unwrap();
            let path = RoomLog::path_for(dir.path(), &id);
            write_record(&path, &payload, &[]);
            let before = std::fs::read(&path).unwrap();
            assert!(matches!(
                recover_by(&path, dir.path(), &recorder_menu()),
                Err(RecoverError::Version(found)) if found == version
            ));
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
    }

    #[test]
    fn current_format_missing_inconsistent_or_tampered_native_settings_fail_closed() {
        let dir = std::env::temp_dir().join(format!("tpf3mp-v9-native-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (manifest, valid_native) = native_terms();
        for (ordinal, native_mods) in [
            Vec::new(),
            vec![NativeMod {
                version: Text::new(format!("1.2.4+{}{}", "0".repeat(10), "a".repeat(64))).unwrap(),
                ..valid_native[0].clone()
            }],
            vec![NativeMod {
                settings: vec![NativeSetting {
                    key: Text::new("radius").unwrap(),
                    value: NativeValue::Int(38),
                }],
                ..valid_native[0].clone()
            }],
        ]
        .into_iter()
        .enumerate()
        {
            let id = RoomId(FixedBytes([0x83 + ordinal as u8; 16]));
            let mut start = recovery_start(id, persist::FORMAT_VERSION);
            start.manifest = Some(persist::StoredManifest::from(&manifest));
            start.native_mods = native_mods;
            let payload = postcard::to_stdvec(&start).unwrap();
            let path = RoomLog::path_for(&dir, &id);
            write_record(&path, &payload, &[]);
            let before = std::fs::read(&path).unwrap();

            assert!(matches!(
                recover_by(&path, &dir, &recorder_menu()),
                Err(RecoverError::NativeSettings)
            ));
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn current_format_short_native_terms_decode_but_recovery_fails_closed() {
        let dir =
            std::env::temp_dir().join(format!("tpf3mp-v9-native-short-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let id = RoomId(FixedBytes([0x8a; 16]));
        let (manifest, mut native_mods) = native_terms();
        let mut legacy_manifest = manifest.clone();
        let legacy_version = Text::new(format!(
            "1.2.3+{}",
            &settings_digest(&BTreeMap::from([("radius".to_owned(), Setting::Int(37))]))[..10]
        ))
        .unwrap();
        legacy_manifest.native[0].version = legacy_version.clone();
        native_mods[0].version = legacy_version;

        let mut start = recovery_start(id, persist::FORMAT_VERSION);
        start.manifest = Some(persist::StoredManifest::from(&legacy_manifest));
        start.native_mods = native_mods;
        let payload = postcard::to_stdvec(&start).unwrap();
        let decoded: StartRecord = postcard::from_bytes(&payload).unwrap();
        assert_eq!(decoded.manifest.unwrap().manifest(), legacy_manifest);
        let path = RoomLog::path_for(&dir, &id);
        write_record(&path, &payload, &[]);
        let before = std::fs::read(&path).unwrap();

        assert!(matches!(
            recover_by(&path, &dir, &recorder_menu()),
            Err(RecoverError::NativeIdentity)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_log_of_an_older_format_is_named_as_such() {
        let dir = std::env::temp_dir().join(format!("tpf3mp-old-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let id = RoomId(FixedBytes([7; 16]));
        // A version 5 start record: the version, then a layout this server
        // no longer reads.
        let payload = postcard::to_stdvec(&(5u16, 1u64, "an older layout")).unwrap();
        let mut record = Vec::new();
        record.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
        record.extend_from_slice(&crc32fast::hash(&payload).to_le_bytes());
        record.extend_from_slice(&payload);
        let path = RoomLog::path_for(&dir, &id);
        std::fs::write(&path, record).unwrap();
        assert!(matches!(
            recover_by(&path, &dir, &recorder_menu()),
            Err(RecoverError::Version(5))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_room_keeps_its_rules_or_is_not_restored() {
        let dir = std::env::temp_dir().join(format!("tpf3mp-rules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let id = RoomId(FixedBytes([9; 16]));
        let settings = RoomSettings::DEFAULT;
        let mut game = Game::new(settings, 1);
        let mut rules = Recorder::default();
        game.append(joined(1), &mut rules);
        let frames = game.seal(10).unwrap();
        let start = StartRecord {
            version: persist::FORMAT_VERSION,
            history: 1,
            id,
            name: Text::new("strict room").unwrap(),
            rules: RulesName::new("strict").unwrap(),
            manifest: None,
            owner: player(1),
            max_players: 8,
            settings,
            invite_tag: vec![0; 32],
            password_tag: None,
            members: Vec::new(),
            base: None,
            native_mods: Vec::new(),
        };
        let mut log = RoomLog::create(&dir, &start).unwrap();
        for frame in &frames {
            log.append(frame).unwrap();
        }
        drop(log);
        let path = RoomLog::path_for(&dir, &id);

        // A server that dropped the rules does not restore the room with
        // other rules.
        assert!(matches!(
            recover_by(&path, &dir, &recorder_menu()),
            Err(RecoverError::UnknownRules(name)) if name == "strict"
        ));
        // One that has them restores it with them, even if they are not its
        // default.
        let menu = recorder_menu().with(RulesChoice {
            name: RulesName::new("strict").unwrap(),
            description: Text::new("strict").unwrap(),
            factory: Arc::new(|| Box::new(Recorder::default())),
        });
        let room = recover_by(&path, &dir, &menu).unwrap().unwrap();
        assert_eq!(room.view().rules.as_str(), "strict");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn running(room: &mut Room) -> &mut Game {
        let Phase::Running(game) = &mut room.phase else {
            panic!("the room runs a game");
        };
        game
    }

    #[tokio::test]
    async fn a_compacted_log_restores_the_same_game() {
        let dir = std::env::temp_dir().join(format!("tpf3mp-compact-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let id = RoomId(FixedBytes([5; 16]));
        let settings = RoomSettings::DEFAULT;
        let (manifest, native_mods) = native_terms();
        let content = manifest.fingerprint();

        // Twenty turns with a command each, then one where a third player
        // joins, then two empty ones.
        let mut game = Game::new(settings, 1);
        let mut rules = Recorder::default();
        let mut frames = Vec::new();
        game.append(joined(1), &mut rules);
        game.append(joined(2), &mut rules);
        for turn in 1..=20 {
            game.append(command(1, turn), &mut rules);
            frames.extend(game.seal(u64::from(turn) * 10).unwrap());
        }
        game.append(joined(3), &mut rules);
        game.append(command(3, 21), &mut rules);
        frames.extend(game.seal(210).unwrap());
        frames.extend(game.seal(220).unwrap());
        frames.extend(game.seal(230).unwrap());
        let start = StartRecord {
            version: persist::FORMAT_VERSION,
            history: 1,
            id,
            name: Text::new("compacted").unwrap(),
            rules: native(),
            manifest: Some(persist::StoredManifest::from(&manifest)),
            owner: player(1),
            max_players: 8,
            settings,
            invite_tag: vec![0; 32],
            password_tag: None,
            members: vec![StartMember {
                player: player(1),
                name: Text::new("p1").unwrap(),
                platform: Platform::current(),
                content: Some(content),
            }],
            base: None,
            native_mods: native_mods.clone(),
        };
        let path = RoomLog::path_for(&dir, &id);
        let logged_frames: Vec<Vec<u8>> = frames.iter().map(|frame| frame.to_vec()).collect();
        std::fs::create_dir_all(&dir).unwrap();
        write_record(&path, &postcard::to_stdvec(&start).unwrap(), &logged_frames);
        // Recovery appends a history marker. A second recovery proves the
        // format-11 start and appended epoch-bearing marker remain readable.
        let restored = recover_room(&path, &dir);
        assert_eq!(restored.settings.scan_plan, tpf3mp_proto::ScanPlan::DEFAULT);
        drop(restored);

        // A current-format room compacts, keeping its last three turns.
        let mut first = recover_room(&path, &dir);
        assert_native_terms(&first, &native_mods);
        assert_eq!(first.start_record(2, None).native_mods, native_mods);
        let before = std::fs::metadata(&path).unwrap().len();
        let game = running(&mut first);
        game.resume_window = 3;
        game.trim();
        first.compact_at = 0;
        first.start_compaction();
        // A turn sealed while the rewrite runs joins the new log.
        let during = {
            let Phase::Running(game) = &mut first.phase else {
                panic!("the room runs a game");
            };
            game.append(command(1, 77), first.ruleset.as_mut());
            game.seal(235).unwrap()
        };
        first.publish(during.clone());
        let done = compaction_done(&mut first.compaction).await;
        first.finish_compaction(done);
        assert_eq!(
            first
                .metrics
                .logs_compacted
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        let after = std::fs::metadata(&path).unwrap().len();
        assert!(after < before, "the log shrank: {after} < {before}");

        // Play goes on after the compaction: the second player is kicked
        // and a fourth joins.
        let Phase::Running(game) = &mut first.phase else {
            panic!("the room runs a game");
        };
        let kicked = EventBody::PlayerLeft {
            player: player(2),
            kicked: true,
        };
        game.append(kicked, first.ruleset.as_mut());
        game.append(joined(4), first.ruleset.as_mut());
        game.append(command(4, 99), first.ruleset.as_mut());
        let frames_after = game.seal(240).unwrap();
        first.publish(frames_after.clone());
        let game = running(&mut first);
        let expected = (game.next_turn, game.next_event, game.sealed_through);
        let seated = game.seated.clone();
        let histories: Vec<(u64, u64)> = game
            .histories
            .iter()
            .map(|history| (history.id, history.after_turn))
            .collect();
        let rules = first.ruleset.save().unwrap();
        drop(first);

        // A second restart reads the compacted log: the base, the kept
        // turns for resuming only, and the turn after, replayed.
        let mut second = recover_room(&path, &dir);
        assert_native_terms(&second, &native_mods);
        assert_eq!(
            second.ruleset.save().unwrap(),
            rules,
            "no command lost or repeated"
        );
        let seats: Vec<PlayerId> = second.members.iter().map(|m| m.player).collect();
        assert_eq!(seats, [player(1), player(3), player(4)]);
        assert!(second.banned.contains(&player(2)));
        let game = running(&mut second);
        assert_eq!(
            (game.next_turn, game.next_event, game.sealed_through),
            expected
        );
        assert_eq!(game.seated, seated);
        assert_eq!((game.log_first_turn, game.sealed_before_log), (21, 200));
        let restored: Vec<(u64, u64)> = game
            .histories
            .iter()
            .map(|history| (history.id, history.after_turn))
            .collect();
        assert_eq!(restored[..histories.len()], histories[..], "and one more");
        assert_eq!(restored.len(), histories.len() + 1);

        // A player who saw turn 22 resumes on the kept turns; one who saw
        // only turn 19 cannot.
        let history = histories.last().unwrap().0;
        let stream = game
            .stream_after(Some(Resume {
                world_epoch: 1,
                after_turn: 22,
                history,
            }))
            .unwrap();
        // Turn 23, kept; the turn sealed during the rewrite; the one after.
        assert_eq!(stream.backlog.len(), 3);
        assert_eq!(stream.backlog[0], frames[22]);
        assert_eq!(stream.backlog[1], during[0]);
        assert_eq!(stream.backlog[2], frames_after[0]);
        assert!(
            game.stream_after(Some(Resume {
                world_epoch: 1,
                after_turn: 19,
                history,
            }))
            .is_err()
        );
        drop(second);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_base_that_contradicts_itself_is_refused() {
        let good = Base {
            first_turn: 5,
            first_event: 3,
            sealed_before: 40,
            after_turn: 7,
            next_event: 4,
            sealed_through: 70,
            speed: Speed::NORMAL,
            histories: vec![(1, 0), (2, 6)],
            content: None,
            seated: Vec::new(),
            banned: Vec::new(),
            rules: Vec::new(),
        };
        let fresh = || Game::new(RoomSettings::DEFAULT, 1);
        assert!(fresh().rebase(&good));
        let broken = [
            Base {
                first_turn: 9,
                ..good.clone()
            },
            Base {
                first_event: 5,
                ..good.clone()
            },
            Base {
                sealed_before: 71,
                ..good.clone()
            },
            Base {
                histories: Vec::new(),
                ..good.clone()
            },
            Base {
                histories: vec![(1, 0), (2, 8)],
                ..good.clone()
            },
            Base {
                histories: vec![(1, 3)],
                ..good.clone()
            },
        ];
        for base in &broken {
            assert!(!fresh().rebase(base), "{base:?}");
        }
    }

    /// A base's counters stand where the game stood. One standing near the
    /// end of their range is refused, rather than overflowing on the next
    /// turn read.
    #[test]
    fn an_implausible_base_is_refused() {
        let dir = std::env::temp_dir().join(format!("tpf3mp-poc-base-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let id = RoomId(FixedBytes([6; 16]));
        let base = Base {
            first_turn: u64::MAX,
            first_event: 1,
            sealed_before: 0,
            after_turn: u64::MAX - 1,
            next_event: 1,
            sealed_through: 0,
            speed: Speed::NORMAL,
            histories: vec![(1, 0)],
            content: None,
            seated: vec![(player(1), Text::new("p1").unwrap(), Platform::current())],
            banned: Vec::new(),
            rules: Vec::new(),
        };
        let start = StartRecord {
            version: persist::FORMAT_VERSION,
            history: 1,
            id,
            name: Text::new("crafted").unwrap(),
            rules: native(),
            manifest: None,
            owner: player(1),
            max_players: 8,
            settings: RoomSettings::DEFAULT,
            invite_tag: vec![0; 32],
            password_tag: None,
            members: Vec::new(),
            base: Some(base),
            native_mods: Vec::new(),
        };
        let mut log = RoomLog::create(&dir, &start).unwrap();
        let turn = TurnMessage::Turn(Turn {
            number: u64::MAX,
            sealed_through: 0,
            speed: Speed::NORMAL,
            events: Vec::new(),
        });
        log.append(&encode_frame(&turn, TURN_MAX_FRAME).unwrap())
            .unwrap();
        drop(log);
        let path = RoomLog::path_for(&dir, &id);
        let env = RoomEnv {
            tick: Duration::from_millis(100),
            metrics: Arc::new(Metrics::default()),
            data_dir: Some(dir.clone()),
            timeouts: Timeouts {
                stall: Duration::from_secs(20),
                load: Duration::from_secs(300),
                abandoned: Duration::from_secs(600),
            },
            snapshots: None,
            compact_log_at: u64::MAX,
        };
        let key = hmac::Key::new(hmac::HMAC_SHA256, &[0; 32]);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            matches!(
                Room::recover(&path, key, &recorder_menu(), env),
                Err(RecoverError::Base | RecoverError::Continuity(_))
            )
        }));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            matches!(outcome, Ok(true)),
            "an implausible base must be refused; recovery panicked: {}",
            outcome.is_err()
        );
    }

    /// The live room hands ownership to the earliest remaining player when
    /// the owner leaves; an owner who comes back is a player like any other.
    #[test]
    fn a_restored_room_has_the_owner_the_live_room_had() {
        let dir = std::env::temp_dir().join(format!("tpf3mp-owner-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let id = RoomId(FixedBytes([7; 16]));
        let mut game = Game::new(RoomSettings::DEFAULT, 1);
        let mut rules = Recorder::default();
        let mut frames = Vec::new();
        for n in 1..=3 {
            game.append(joined(n), &mut rules);
        }
        frames.extend(game.seal(10).unwrap());
        // The owner leaves, and comes back later.
        let left = EventBody::PlayerLeft {
            player: player(1),
            kicked: false,
        };
        game.append(left, &mut rules);
        frames.extend(game.seal(20).unwrap());
        game.append(joined(1), &mut rules);
        frames.extend(game.seal(30).unwrap());
        let start = StartRecord {
            version: persist::FORMAT_VERSION,
            history: 1,
            id,
            name: Text::new("owners").unwrap(),
            rules: native(),
            manifest: None,
            owner: player(1),
            max_players: 8,
            settings: RoomSettings::DEFAULT,
            invite_tag: vec![0; 32],
            password_tag: None,
            members: Vec::new(),
            base: None,
            native_mods: Vec::new(),
        };
        let mut log = RoomLog::create(&dir, &start).unwrap();
        for frame in &frames {
            log.append(frame).unwrap();
        }
        drop(log);
        let room = recover_room(&RoomLog::path_for(&dir, &id), &dir);
        assert_eq!(room.owner, player(2), "the second player took over");
        let seats: Vec<PlayerId> = room.members.iter().map(|m| m.player).collect();
        assert_eq!(seats, [player(2), player(3), player(1)]);
        drop(room);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_world_goes_to_every_member_not_on_it_and_is_owed_by_one_that_cannot_take_it() {
        let x = SnapshotId(FixedBytes([1; 32]));
        let y = SnapshotId(FixedBytes([2; 32]));
        // Already on the world handed out: nothing, linked or not.
        assert_eq!(refeed(Some(y), Some(y), true, true), Refeed::AlreadyOnIt);
        assert_eq!(refeed(Some(y), Some(y), false, false), Refeed::AlreadyOnIt);
        // On another world, or one the room forgot in a restart: it loads
        // this one with the others.
        assert_eq!(refeed(Some(x), Some(y), true, true), Refeed::Feed);
        assert_eq!(refeed(None, Some(y), true, true), Refeed::Feed);
        // Away, or not taking turns: it owes the world.
        assert_eq!(refeed(Some(x), Some(y), false, false), Refeed::Owes);
        assert_eq!(refeed(Some(x), Some(y), true, false), Refeed::Owes);
        assert_eq!(refeed(None, Some(y), false, false), Refeed::Owes);
    }

    #[tokio::test]
    async fn a_pending_world_offer_survives_other_joins_until_its_matching_load_ack() {
        use tpf3mp_net::{ServerIdentity, ServerTrust, client_config, server_config};

        // Real connections satisfy MemberLink; the room itself is driven
        // synchronously, so every queue assertion observes all preceding work.
        let identity = ServerIdentity::self_signed(&["localhost"]).unwrap();
        let trust = ServerTrust::Pinned(identity.leaf().clone());
        let server = quinn::Endpoint::server(
            server_config(identity).unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        let mut client = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        client.set_default_client_config(client_config(trust).unwrap());
        let mut peers = Vec::new();
        let mut links = Vec::new();
        for _ in 0..3 {
            let connecting = client
                .connect(server.local_addr().unwrap(), "localhost")
                .unwrap();
            let (accepted, connected) = tokio::join!(
                async { server.accept().await.unwrap().await.unwrap() },
                connecting,
            );
            links.push(accepted);
            peers.push(connected.unwrap());
        }
        let (control, _control) = mpsc::channel(32);
        let (advisory, _advisory) = mpsc::channel(32);
        let link = |id, connection| {
            let (turns, incoming) = mpsc::channel(32);
            (
                MemberLink {
                    id,
                    connection,
                    turns,
                    control: control.clone(),
                    advisory: advisory.clone(),
                },
                incoming,
            )
        };
        let (first_link, mut first_queue) = link(1, links.remove(0));
        let (second_link, mut second_queue) = link(2, links.remove(0));
        let (replacement_link, mut replacement_queue) = link(3, links.remove(0));
        let take = |queue: &mut mpsc::Receiver<TurnFeed>| {
            let TurnFeed::Open { start, .. } = queue.try_recv().expect("one world offered") else {
                panic!("an offered world opens its stream");
            };
            assert!(
                queue.try_recv().is_err(),
                "exactly one offer, without timing assumptions"
            );
            start
        };

        let temp = tempfile::tempdir().unwrap();
        let mut game = Game::new(RoomSettings::DEFAULT, 1);
        let mut rules = Recorder::default();
        game.append(joined(1), &mut rules);
        game.append(joined(2), &mut rules);
        let frames: Vec<Vec<u8>> = game.seal(10).unwrap().iter().map(|f| f.to_vec()).collect();
        let start = recovery_start(RoomId(FixedBytes([0x91; 16])), persist::FORMAT_VERSION);
        let path = RoomLog::path_for(temp.path(), &start.id);
        write_record(&path, &postcard::to_stdvec(&start).unwrap(), &frames);
        let mut room = recover_room(&path, temp.path());
        let snapshots = Arc::new(
            Snapshots::open(&snapshots::SnapshotConfig::new(
                temp.path().join("snapshots"),
            ))
            .unwrap(),
        );
        let manifest = Arc::new(
            snapshots
                .store
                .ingest(&b"first world"[..], tpf3mp_snapshot::ChunkParams::DEFAULT)
                .unwrap(),
        );
        let first_world = bulk::snapshot_id(&manifest.id());
        let point = SavePoint {
            event: 0,
            after_turn: 0,
            history: 1,
            sealed_through: 0,
        };
        running(&mut room).saves.current = Some(Agreed { manifest, point });
        room.snapshots = Some(snapshots.clone());
        assert!(
            room.members
                .iter()
                .all(|member| member.offered.is_none() && member.load_step.is_none())
        );
        room.members[0].link = Some(first_link);
        room.members[0].needs = Needs::World;
        room.members[1].link = Some(second_link);
        room.members[1].streaming = true;
        room.members[1].loaded = Some(first_world);
        room.offer_worlds(Instant::now());
        let first_offer = take(&mut first_queue);
        assert_eq!(first_offer.world.as_ref().unwrap().snapshot, first_world);
        assert!(second_queue.try_recv().is_err());

        // A second join needs this same world before the first game's ACK.
        room.members[1].loaded = None;
        room.members[1].needs = Needs::World;
        room.offer_worlds(Instant::now());
        let second_offer = take(&mut second_queue);
        assert!(
            first_queue.try_recv().is_err(),
            "an in-flight offer must not be restarted by another join"
        );
        assert_eq!(room.members[0].world_epoch, first_offer.world_epoch);
        assert_eq!(room.members[0].load_step, Some(0));
        assert_eq!(room.members[0].loaded, None, "offered is not acknowledged");
        assert!(!room.members[0].accepts_report(first_offer.world_epoch));
        room.loaded(player(1), 1, first_offer.world_epoch.wrapping_add(1), 0);
        room.loaded(player(1), 1, first_offer.world_epoch, 1);
        room.loaded(player(1), 99, first_offer.world_epoch, 0);
        assert_eq!(room.members[0].load_step, Some(0));
        assert_eq!(room.members[0].loaded, None);
        room.loaded(player(1), 1, first_offer.world_epoch, 0);
        assert_eq!(room.members[0].loaded, Some(first_world));
        assert!(room.members[0].accepts_report(first_offer.world_epoch));

        // A changed world supersedes the second game's still-pending offer.
        let manifest = Arc::new(
            snapshots
                .store
                .ingest(
                    &b"replacement world"[..],
                    tpf3mp_snapshot::ChunkParams::DEFAULT,
                )
                .unwrap(),
        );
        let next_world = bulk::snapshot_id(&manifest.id());
        running(&mut room).saves.current = Some(Agreed { manifest, point });
        room.members[0].needs = Needs::World;
        room.offer_worlds(Instant::now());
        let superseding_first = take(&mut first_queue);
        let superseding_second = take(&mut second_queue);
        assert_eq!(
            superseding_second.world.as_ref().unwrap().snapshot,
            next_world
        );
        assert_ne!(superseding_second.world_epoch, second_offer.world_epoch);
        room.loaded(player(2), 2, second_offer.world_epoch, 0);
        assert_eq!(room.members[1].loaded, None, "superseded ACK is stale");
        assert_eq!(room.members[1].load_step, Some(0));
        assert_eq!(room.members[0].loaded, Some(first_world));

        // A fresh link without a resume must be sent the world, even while
        // the same player's old link had an unacknowledged offer for it.
        let invite = Invite("ABC234".parse().unwrap());
        room.secrets.invite_tag =
            hmac::sign(&room.secrets.key, &RoomSecrets::invite_input(&invite))
                .as_ref()
                .to_vec();
        room.join(
            NewMember {
                player: player(1),
                name: Text::new("replacement").unwrap(),
                platform: Platform::current(),
                link: replacement_link,
                content: None,
                banner: None,
            },
            &invite,
            None,
            None,
            room.settings.scan_plan,
        )
        .unwrap();
        let resent = take(&mut replacement_queue);
        assert_eq!(resent.world.as_ref().unwrap().snapshot, next_world);
        assert_ne!(resent.world_epoch, superseding_first.world_epoch);
        assert!(
            second_queue.try_recv().is_err(),
            "the other link keeps its pending offer"
        );
        room.loaded(player(1), 1, superseding_first.world_epoch, 0);
        room.loaded(player(1), 3, superseding_first.world_epoch, 0);
        assert_eq!(room.members[0].load_step, Some(0));
        room.loaded(player(1), 3, resent.world_epoch, 0);
        assert_eq!(room.members[0].loaded, Some(next_world));

        // Equal snapshot bytes do not make a different replay frontier the
        // same offer: the pending second link must be rebased to that point.
        running(&mut room).saves.current.as_mut().unwrap().point = SavePoint {
            event: 2,
            after_turn: 1,
            history: 1,
            sealed_through: 10,
        };
        room.members[0].needs = Needs::World;
        room.offer_worlds(Instant::now());
        let _ = take(&mut replacement_queue);
        let advanced = take(&mut second_queue);
        assert_eq!(advanced.world.as_ref().unwrap().snapshot, next_world);
        assert_eq!(advanced.sealed_through, 10);
        assert_ne!(advanced.world_epoch, superseding_second.world_epoch);
    }

    #[test]
    fn a_returning_player_that_owes_a_world_is_handed_it_even_resuming() {
        assert!(
            rejoins_with_world(true, false, Needs::Nothing),
            "no turns of this game: a world"
        );
        assert!(
            rejoins_with_world(true, true, Needs::World),
            "owing a world: the world, not its own"
        );
        assert!(
            !rejoins_with_world(true, true, Needs::Nothing),
            "owing nothing: it resumes"
        );
        assert!(
            !rejoins_with_world(false, false, Needs::World),
            "a room that keeps no worlds hands none"
        );
    }

    /// A player's company passwords are counted by the room, not by their
    /// membership: leaving and joining again gives none back; and the room
    /// takes only so many in all, however many player keys send them.
    #[test]
    fn passwords_are_counted_by_player_and_by_room() {
        let mut budget = SecretBudget::new();
        let now = Instant::now();
        for _ in 0..SECRETS_PER_WINDOW {
            assert!(budget.take(&player(1), now));
        }
        assert!(
            !budget.take(&player(1), now),
            "the player's share is spent, left and joined again or not"
        );
        // Fresh keys, each with a share of its own, until the room's is spent.
        let mut taken = SECRETS_PER_WINDOW;
        let mut key = 2;
        while budget.take(&player(key), now) {
            taken += 1;
            if taken.is_multiple_of(SECRETS_PER_WINDOW) {
                key += 1;
            }
        }
        assert_eq!(taken, ROOM_SECRETS_PER_WINDOW);
        // A new window gives them back.
        assert!(budget.take(&player(1), now + SECRET_WINDOW));
    }

    /// A checkpoint report of lane 3 with `value`, from a game that played
    /// `loaded`.
    fn lane_report(n: u8, value: u8, loaded: Option<SnapshotId>) -> Report {
        Report {
            player: player(n),
            platform: Platform::current(),
            order: usize::from(n),
            lanes: vec![LaneDigest {
                lane: 3,
                digest: FixedBytes([value; 32]),
            }],
            loaded,
        }
    }

    /// The production desync of 2026-10-06 (room r-61f8…): three games
    /// played to steps 9171-9600 on the start world, the room then handed
    /// all of them its save of step 8208, and their replays, renumbered by
    /// the load, rightly differed from what they had played. The room kept
    /// the rounds the old worlds decided, ignored two replays' reports as
    /// repeats and judged the third against the old verdict: "diverged at
    /// step 9200, lanes [3, 4]", and a needless rebase. Handing the world
    /// out forgets every report past its step from games that did not play
    /// it.
    #[test]
    fn a_world_handed_out_forgets_what_older_worlds_reported_past_it() {
        let start = Some(SnapshotId(FixedBytes([1; 32])));
        let world = SnapshotId(FixedBytes([2; 32]));
        let now = Instant::now();
        let mut game = Game::new(RoomSettings::DEFAULT, 1);
        let mut decided = |step: u64, reports: Vec<Report>| {
            let mut round = Round {
                opened: now,
                reports,
                verdict: None,
            };
            assert!(decide_round(&mut round).is_empty());
            game.rounds.insert(step, round);
        };
        // Before the save: the old worlds agreed, and nothing replays it.
        decided(
            8200,
            vec![lane_report(1, 7, start), lane_report(2, 7, start)],
        );
        // Past the save, live: Porkster and Brad decided step 9200.
        decided(9150, (1..=3).map(|n| lane_report(n, 8, start)).collect());
        decided(
            9200,
            vec![lane_report(1, 9, start), lane_report(2, 9, start)],
        );
        // A game already on the world reported step 9250 from it.
        decided(
            9250,
            vec![lane_report(1, 5, start), lane_report(2, 5, Some(world))],
        );
        game.rounds_closed_through = 8000;
        // A save past the snapshot's event, reported from both worlds.
        let point = |event: u64| SavePoint {
            event,
            after_turn: event,
            history: 1,
            sealed_through: 9300,
        };
        let save_report = |n: u8, loaded: Option<SnapshotId>| SaveReport {
            report: lane_report(n, 1, loaded),
            world: None,
        };
        game.saves.rounds.insert(
            600,
            SaveRound {
                point: point(600),
                opened: now,
                reports: vec![save_report(1, start), save_report(2, Some(world))],
            },
        );
        game.saves.rounds.insert(
            500,
            SaveRound {
                point: point(500),
                opened: now,
                reports: vec![save_report(1, start)],
            },
        );

        assert_eq!(game.forget_reports_after(8208, 556, world, now), 7);

        let steps: Vec<u64> = game.rounds.keys().copied().collect();
        assert_eq!(
            steps,
            [8200, 9250],
            "rounds only old worlds reported are gone"
        );
        assert_eq!(game.rounds[&8200].reports.len(), 2, "before the save: kept");
        assert!(game.rounds[&8200].verdict.is_some());
        let mixed = &game.rounds[&9250];
        assert_eq!(mixed.reports.len(), 1, "only the report from the world");
        assert_eq!(mixed.reports[0].player, player(2));
        assert!(mixed.verdict.is_none(), "decided again from what is left");
        assert_eq!(game.rounds_closed_through, 8000, "nothing reopens below it");
        let later = &game.saves.rounds[&600].reports;
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].report.player, player(2));
        assert_eq!(
            game.saves.rounds[&500].reports.len(),
            1,
            "before the save: kept"
        );

        // The replays now report steps 9150 and 9200 afresh, and their own
        // agreement decides them.
        for step in [9150, 9200] {
            let mut round = Round {
                opened: now,
                reports: Vec::new(),
                verdict: None,
            };
            for n in 1..=3 {
                round.reports.push(lane_report(n, 4, Some(world)));
            }
            assert!(decide_round(&mut round).is_empty(), "step {step}");
            assert!(game.rounds.insert(step, round).is_none());
        }
    }

    fn test_member() -> Member {
        // A link needs a live QUIC connection. Pacing only reads `streaming`,
        // which the room keeps false whenever the link is gone.
        Member {
            player: PlayerId(FixedBytes([0; 32])),
            name: Text::new("t").unwrap(),
            platform: Platform::current(),
            banner: None,
            loading: None,
            loading_at: None,
            ready: false,
            content: None,
            declared: None,
            told_diff: None,
            told_room: None,
            link: None,
            streaming: false,
            pace: Pace::Loading,
            advanced: Instant::now(),
            intents: TokenBucket::new(1, 1),
            payload_bytes: TokenBucket::new(1, 1),
            chats: TokenBucket::new(1, 1),
            needs: Needs::Nothing,
            offered: None,
            loaded: None,
            world_epoch: 0,
            load_step: None,
            rebased: None,
            stream_from: 0,
        }
    }
}
