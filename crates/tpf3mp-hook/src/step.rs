//! The step gate in the game: what the detour on the game's simulation step
//! does each time the game calls it.
//!
//! Transport Fever 3's `GameSim::Step` (as TPF2's) is one batch of the
//! game's own pacing: the main thread calls it on its schedule, and it runs
//! as many simulation updates as its call of the speed getter answers: one
//! at 1x, none while paused, which takes the game's own paused path. The
//! renderer interpolates from each batch, so every call must run: a call
//! skipped looks to the game like a batch that ran without the world moving
//! on, and its clock goes back (TF3 then fails an assertion in its
//! particles). So the detour runs the game's step exactly once per call, and
//! [`StepDriver::on_step`] says how many updates that call runs:
//!
//! - of the steps the room has released, at most [`MAX_STEPS_PER_CALL`]
//!   and never past the next checkpoint step, as many as the room's pace
//!   over the game's call period, kept even from call to call
//!   ([`crate::cadence`]), and all of them when an action, save, load or
//!   end waits behind them, the room pauses or stops releasing, or the game
//!   is far behind (so a room faster than the game's own pace catches up);
//! - none while the room withholds the next step (the room is paused, or a
//!   player is behind): the game's paused path, and the world stands still;
//! - before a room has begun a game, and after it has ended, what the
//!   game's own speed says; before, each new world the player's game has up
//!   is told to the agent, which marks the player ready in the room's lobby
//!   ([`RoomGate::world_up`]);
//! - while the game saves its world for the room, or loads the room's
//!   (docs/HOOKS.md, "The room's world"), none;
//! - with no world up at all, at the game's main menu, no call comes: the
//!   menu's own frame drives [`StepDriver::on_menu`] instead, which follows
//!   the room until its game begins, tells the agent the game is at its
//!   menu, and starts a load of the room's save from there;
//! - on anything it cannot follow (the agent gone, a malformed message, a
//!   room's world that did not load, a world up the room never loaded:
//!   [`WorldMark`]) none, for good: the world stands still
//!   rather than run on apart from the room's (fail closed).
//!
//! Answering the step's own speed call rather than skipping calls is how
//! TPF2MP paced TPF2 (`tpf2-multiplayer/native/src/speedhook.cpp`, which
//! also describes the game's batch pacing).
//!
//! A batch that ends at a checkpoint step asks the game for the world's
//! lanes: the mod's game script reads them after the batch's last update
//! (docs/HOOKS.md, "The world's lanes"), and the driver reports their
//! digests for that step. A batch that does not bring them holds the world.
//!
//! Ordered actions run between simulation updates through the game's
//! engine-event path. The driver queues a token-only GUI wake and holds
//! steps, saves and loads until the game script has applied the actions
//! and stored its state. This also works while paused, and every replica
//! uses the same phase when running. No simulation update is invented.
//!
//! This module knows nothing of the process: the detour hands it the
//! game's step as a closure, and the room's side is a [`RoomGate`], the
//! real [`Session`] in the game and a script in the tests.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use ring::digest::{SHA256, digest};
use tpf3mp_bridge::{
    Begin, Game, LobbyAction, LobbyView, ModLists, Notice, SaveOrder, Session, SessionError,
    StepGate,
};
use tpf3mp_proto::{
    ChatText, Event, EventBody, FixedBytes, LaneDigest, Payload, PlayerId, RoomId, Seal, Secret,
    Speed, action::Action,
};

use crate::cadence::{Cadence, Choice, Pick};
use crate::lanedump::{self, DumpOrder, LaneDumps};

/// Most steps one call of the game's step runs, catching up with the room:
/// at the game's 1x (5 calls a second), rooms up to 16x keep up. The game
/// itself runs up to 64 in a call (its debug steps).
pub const MAX_STEPS_PER_CALL: u32 = 16;

/// How many updates one call of the game's step runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Updates {
    /// What the game's own speed says: no room's game.
    Own,
    /// Exactly this many; 0 is the game's paused path.
    Exactly(u32),
}

/// The room's side of the gate: what the detour needs of a [`Session`].
pub trait RoomGate {
    fn heartbeat(&self) {}
    fn poll_departure(&mut self) -> Result<bool, SessionError> {
        Ok(false)
    }
    fn return_to_lobby(&mut self) -> bool {
        false
    }
    fn try_begin(&mut self) -> Result<Option<Begin>, SessionError>;
    fn poll_step(&mut self, game: &mut HookGame) -> Result<StepGate, SessionError>;
    /// The step the game runs next.
    fn next_step(&self) -> u64;
    /// After `poll_step` said Run: the steps that may run as one batch.
    fn batch(&mut self, game: &mut HookGame, max: u32) -> Result<u32, SessionError>;
    /// After `poll_step` said Run: reads the releases waiting, up to the
    /// first message that must wait for them ([`Session::read_ahead`]).
    fn read_ahead(&mut self, game: &mut HookGame) -> Result<(), SessionError> {
        let _ = game;
        Ok(())
    }
    /// The last step released, as far as the link was read.
    fn released(&self) -> u64;
    /// Whether a message read ahead waits for the released steps
    /// ([`Session::barrier_waiting`]).
    fn barrier_waiting(&self) -> bool {
        false
    }
    fn after_step(&mut self, game: &mut HookGame) -> Result<u64, SessionError>;
    fn loaded(&mut self, next_step: u64) -> Result<(), SessionError>;
    fn request_speed(&mut self, speed: Speed) -> Result<(), SessionError>;
    /// Hands the room a player's action, with the password it needs, if
    /// any, which the room seals.
    fn command(&mut self, payload: Payload, secret: Option<Secret>) -> Result<u64, SessionError>;
    /// Says `text` to the room for the player.
    fn chat(&mut self, text: ChatText) -> Result<(), SessionError>;
    /// Shows the room's other members what the player's build tool shows,
    /// or that it shows nothing.
    fn preview(&mut self, preview: Option<Payload>) -> Result<(), SessionError>;
    /// Before the room begins: the game's world number `world` is up. Says
    /// whether the agent was told.
    fn world_up(&mut self, world: u64) -> Result<bool, SessionError>;
    /// Before the room begins: the game is at its main menu, arrived there
    /// for the `menu`th time. Says whether the agent was told now.
    fn menu_up(&mut self, menu: u64) -> Result<bool, SessionError>;
    fn saved(
        &mut self,
        game: &mut HookGame,
        outcome: Result<(), String>,
    ) -> Result<(), SessionError>;
    /// Hands the launcher an action of the main menu's Multiplayer window
    /// ([`Session::lobby_act`]).
    fn lobby_act(&mut self, action: LobbyAction) -> Result<(), SessionError> {
        let _ = action;
        Err(SessionError::Unexpected("a lobby action"))
    }
    /// Reads the link where no step does ([`Session::poll_lobby`]).
    fn poll_lobby(&mut self) -> Result<(), SessionError> {
        Ok(())
    }
    /// The launcher's lobby, if new ([`Session::take_lobby`]).
    fn take_lobby(&mut self) -> Option<LobbyView> {
        None
    }
}

/// Longest a save the room ordered may take the game before it is reported
/// as failed.
pub const SAVE_PATIENCE: Duration = Duration::from_secs(120);
/// Longest a load of the room's save may take before the world is held.
pub const LOAD_PATIENCE: Duration = Duration::from_secs(600);

/// What the driver asks of the game beyond its step: saving and loading
/// whole worlds, which the game's GUI does (the mod, through
/// [`crate::lua`], and [`crate::worlds`] for the files).
pub trait GameControl: Send {
    /// Apply ordered actions on the simulation thread, between updates.
    /// The GUI wakes the game script; it never receives the action payloads.
    fn request_replay(&mut self, step: u64, actions: &[Ordered]) -> Result<(), String>;
    /// Completion of that replay, after its script state was saved.
    fn replay_result(&mut self) -> Option<Result<(), String>>;
    /// Invalidate a delayed wake when the world stops following this room.
    fn cancel_replay(&mut self);
    /// Asks the game to save its world under `name`.
    fn request_save(&mut self, name: &str);
    /// The last save's outcome, once the game has one: the file written, or
    /// why not.
    fn save_result(&mut self) -> Option<Result<PathBuf, String>>;
    /// Asks the game to load the save `file` (the room's world), through
    /// the GUI of the world it has up or from its main menu.
    fn request_load(&mut self, file: &Path, from: LoadFrom) -> Result<(), String>;
    /// Whether the world the last load asked for is up. Once.
    fn load_done(&mut self) -> bool;
    /// Why the last load asked for could not be started, if it could not.
    /// Once.
    fn load_failed(&mut self) -> Option<String>;
    /// Tells the game's Multiplayer window what the room said: the room,
    /// its speed, its chat, a divergence, the game's end.
    fn room_notice(&mut self, notice: &Notice);
    /// Tells the game's Multiplayer window which player is this game's.
    fn set_me(&mut self, player: PlayerId);
    /// The stable room identity, which survives reconnects and restores.
    fn set_room(&mut self, room: RoomId);
    /// The mods the room's worlds load with (`ToHook::Begin`'s `mods`).
    fn set_mods(&mut self, mods: Option<ModLists>);
    /// The number of a world whose GUI started with the mod linked, since
    /// the last call, if one did: the latest. Once.
    fn world_up(&mut self) -> Option<u64>;
    /// Which world the game has up, as far as the hook can count worlds
    /// ([`WorldMark`]). Reads only.
    fn world_mark(&mut self) -> WorldMark;
}

/// Counts of the worlds a game has had, which tell one world up from the
/// next: a world up since the room's world was taken has another mark.
/// docs/HOOKS.md, "A world the room did not load".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorldMark {
    /// The worlds that closed in this game (`CMenuUI::m_game` cleared),
    /// other than for a load the hook started; `None` where the hook cannot
    /// see a world close (no `m_game` in the profile).
    pub closed: Option<u64>,
    /// The worlds whose GUI started with the mod linked in this game.
    pub started: u64,
}

impl WorldMark {
    /// Whether the world up at this mark is another than the one up at
    /// `then`: one closed since, or, where closes cannot be seen, another
    /// world's GUI started since.
    pub fn replaced_since(&self, then: &WorldMark) -> bool {
        match (self.closed, then.closed) {
            (Some(now), Some(before)) => now != before,
            _ => self.started != then.started,
        }
    }
}

/// Where a load of the room's save is started from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadFrom {
    /// The GUI of the world the game has up (the mod's, `app.loadGame`).
    Gui,
    /// The game's main menu, with no world up (`crate::menu`).
    Menu,
}

/// A save the game is making for the room.
#[derive(Debug)]
struct Saving {
    event: u64,
    since: Instant,
}

/// A load of the room's save the game is making.
#[derive(Debug)]
struct Loading {
    next_step: u64,
    since: Instant,
}

impl RoomGate for Session {
    fn heartbeat(&self) {
        Session::heartbeat(self);
    }
    fn poll_departure(&mut self) -> Result<bool, SessionError> {
        Session::poll_departure(self)
    }
    fn return_to_lobby(&mut self) -> bool {
        Session::return_to_lobby(self)
    }
    fn try_begin(&mut self) -> Result<Option<Begin>, SessionError> {
        Session::try_begin(self)
    }
    fn poll_step(&mut self, game: &mut HookGame) -> Result<StepGate, SessionError> {
        Session::poll_step(self, game)
    }
    fn next_step(&self) -> u64 {
        Session::next_step(self)
    }
    fn batch(&mut self, game: &mut HookGame, max: u32) -> Result<u32, SessionError> {
        Session::batch(self, game, max)
    }
    fn read_ahead(&mut self, game: &mut HookGame) -> Result<(), SessionError> {
        Session::read_ahead(self, game)
    }
    fn released(&self) -> u64 {
        Session::released(self)
    }
    fn barrier_waiting(&self) -> bool {
        Session::barrier_waiting(self)
    }
    fn after_step(&mut self, game: &mut HookGame) -> Result<u64, SessionError> {
        Session::after_step(self, game)
    }
    fn loaded(&mut self, next_step: u64) -> Result<(), SessionError> {
        Session::loaded(self, next_step)
    }
    fn request_speed(&mut self, speed: Speed) -> Result<(), SessionError> {
        Session::request_speed(self, speed)
    }
    fn command(&mut self, payload: Payload, secret: Option<Secret>) -> Result<u64, SessionError> {
        Session::command_with(self, payload, secret)
    }
    fn chat(&mut self, text: ChatText) -> Result<(), SessionError> {
        Session::chat(self, text)
    }
    fn preview(&mut self, preview: Option<Payload>) -> Result<(), SessionError> {
        Session::preview(self, preview)
    }
    fn world_up(&mut self, world: u64) -> Result<bool, SessionError> {
        Session::world_up(self, world)
    }
    fn menu_up(&mut self, menu: u64) -> Result<bool, SessionError> {
        Session::menu_up(self, menu)
    }
    fn saved(
        &mut self,
        game: &mut HookGame,
        outcome: Result<(), String>,
    ) -> Result<(), SessionError> {
        Session::saved(self, game, outcome)
    }
    fn lobby_act(&mut self, action: LobbyAction) -> Result<(), SessionError> {
        Session::lobby_act(self, action)
    }
    fn poll_lobby(&mut self) -> Result<(), SessionError> {
        Session::poll_lobby(self)
    }
    fn take_lobby(&mut self) -> Option<LobbyView> {
        Session::take_lobby(self)
    }
}

/// The game, as the session sees it. It keeps the actions the room orders
/// until the driver hands them to the game at their step. Lanes and saving
/// come next; until then it reports no lanes and refuses to save.
#[derive(Debug, Default)]
pub struct HookGame {
    pub events: u64,
    pub notices: Vec<String>,
    /// The local player, from the room's `Begin`: the room's events name it
    /// as the actor of the player's own commands.
    pub me: Option<PlayerId>,
    /// The actions the room ordered for the next step to run, in order,
    /// each with its client sequence number when the local player sent it,
    /// its sender, and the seal of the password sent with it.
    pub actions: Vec<(Action, Option<u64>, PlayerId, Option<Seal>)>,
    /// The player's commands the room refused: their sequence numbers, and
    /// why.
    pub refused: Vec<(u64, String)>,
    /// An event the game cannot follow.
    pub fault: Option<String>,
    /// The world's lanes after the batch that ran last, when it ended at a
    /// checkpoint step: what the session reports for that step.
    pub lanes: Option<Vec<LaneDigest>>,
    /// What the room said for the game's Multiplayer window, which the
    /// driver hands on (`GameControl::room_notice`).
    pub window: Vec<Notice>,
}

impl Game for HookGame {
    fn apply(&mut self, event: &Event) {
        self.events += 1;
        if let EventBody::Command {
            player,
            client_seq,
            payload,
            seal,
        } = &event.body
        {
            let own = (self.me == Some(*player)).then_some(*client_seq);
            match Action::from_payload(payload) {
                Ok(action) => self.actions.push((action, own, *player, *seal)),
                Err(error) => {
                    self.fault.get_or_insert(format!(
                        "the room ordered an action this game cannot read (event {}): {error}",
                        event.seq
                    ));
                }
            }
        }
    }

    fn lanes(&mut self) -> Vec<LaneDigest> {
        self.lanes.take().unwrap_or_default()
    }

    fn save(&mut self, _file: &std::path::Path) -> Result<(), String> {
        // Only `Session::before_step` saves here; the driver polls, and
        // saves at `StepGate::Save`, through the GUI.
        Err("the hook saves the world through the game's GUI, not here".into())
    }

    fn notice(&mut self, notice: Notice) {
        if let Notice::Refused { command, reason } = &notice {
            self.refused.push((*command, format!("{reason:?}")));
        }
        self.window.push(notice.clone());
        // The room and its chat are for the Multiplayer window, and the
        // other members' previews for the GUI, not the log.
        if !matches!(
            notice,
            Notice::Room(_) | Notice::Chat { .. } | Notice::Preview { .. }
        ) {
            self.notices.push(format!("{notice:?}"));
        }
    }
}

/// An action the room ordered, and, for one of the player's own, the ticket
/// the mod was given when it handed the action over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ordered {
    pub action: Action,
    pub ticket: Option<u64>,
    /// The player who sent it: the mod books it to that player's company.
    pub player: PlayerId,
    /// The seal of the password sent with it (a company's), which the mod
    /// compares with the seal it keeps; never the password.
    pub seal: Option<Seal>,
}

/// One of the player's actions as the mod handed it over: its ticket, its
/// payload, and the password it needs, if any.
pub type Handed = (u64, Payload, Option<Secret>);

/// What the detour hands each call to: a [`StepDriver`] over any room.
pub trait StepHandler: Send {
    /// See [`StepDriver::on_step`].
    fn on_step(&mut self, commands: Vec<Handed>, run: &mut RunStep<'_>) -> Outcome;
    fn take_log(&mut self) -> Vec<String>;
    /// See [`StepDriver::take_refused`].
    fn take_refused(&mut self) -> Vec<(u64, String)>;
    /// In the room's game: the game's speed is then the room's, and one call
    /// of the game's step must be one update.
    fn in_room(&self) -> bool;
    /// The speed the player picked in the game's speed row (the game's own
    /// speed: 0 paused, 1 for 1x, ...).
    fn chosen_speed(&mut self, speedup: u64);
    /// See [`StepDriver::say`].
    fn say(&mut self, text: ChatText);
    /// See [`StepDriver::preview`].
    fn preview(&mut self, preview: Option<Payload>);
    /// See [`StepDriver::on_menu`].
    fn on_menu(&mut self);
    /// See [`StepDriver::lobby`].
    fn lobby(&mut self, actions: Vec<LobbyAction>) -> Option<LobbyView>;
    /// See [`StepDriver::why`].
    fn why(&self) -> &'static str {
        "-"
    }
    /// See [`StepDriver::cadence_note`].
    fn cadence_note(&self) -> Option<String> {
        None
    }
    /// See [`StepDriver::batch_info`].
    fn batch_info(&self) -> crate::interval::BatchInfo {
        crate::interval::BatchInfo::default()
    }
}

impl<G: RoomGate + Send> StepHandler for StepDriver<G> {
    fn on_step(&mut self, commands: Vec<Handed>, run: &mut RunStep<'_>) -> Outcome {
        StepDriver::on_step(self, commands, run)
    }
    fn cadence_note(&self) -> Option<String> {
        StepDriver::cadence_note(self)
    }
    fn batch_info(&self) -> crate::interval::BatchInfo {
        StepDriver::batch_info(self)
    }
    fn why(&self) -> &'static str {
        StepDriver::why(self)
    }
    fn take_log(&mut self) -> Vec<String> {
        StepDriver::take_log(self)
    }
    fn take_refused(&mut self) -> Vec<(u64, String)> {
        StepDriver::take_refused(self)
    }
    fn in_room(&self) -> bool {
        StepDriver::in_room(self)
    }
    fn chosen_speed(&mut self, speedup: u64) {
        StepDriver::chosen_speed(self, speedup);
    }
    fn on_menu(&mut self) {
        StepDriver::on_menu(self);
    }
    fn say(&mut self, text: ChatText) {
        StepDriver::say(self, text);
    }
    fn preview(&mut self, preview: Option<Payload>) {
        StepDriver::preview(self, preview);
    }
    fn lobby(&mut self, actions: Vec<LobbyAction>) -> Option<LobbyView> {
        StepDriver::lobby(self, actions)
    }
}

/// Where the driver is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// The room has not begun a game: the game runs as it would.
    BeforeBegin,
    /// The room's game: steps run as the room releases them.
    Running,
    /// Something the driver cannot follow: the world stands still.
    Holding(String),
    /// The room's game is over: the game runs as it would.
    Ended,
}

/// What one call did: the game's step ran once, with these updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub updates: Updates,
}

/// One call of the game's step, as the driver plans it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Batch<'a> {
    pub updates: Updates,
    /// The batch ends at a checkpoint step: after its last update the game
    /// reads the world's lanes.
    pub lanes: bool,
    /// The call is the room's game's (the driver follows a room's game, or
    /// holds it): a paused call must not count a frame in the game's
    /// `tickCount` ([`crate::ticks`]).
    pub room: bool,
    /// The room's step the batch's first update runs, when it runs the
    /// room's steps.
    pub first_step: Option<u64>,
    /// With `lanes`: the lanes whose full text the game writes to the log
    /// after its last update, entry by entry ([`crate::lanedump`]).
    pub dump: Option<&'a DumpOrder>,
}

/// A lane the game read: its number and what the game read for it, which
/// the driver reports as a digest.
pub type LaneText = (u16, String);

/// Runs the game's own step exactly once. Ordered actions have already
/// finished before an update batch starts. Returns checkpoint lanes,
/// or why the game could not follow the batch.
pub type RunStep<'a> = dyn FnMut(&Batch<'_>) -> Result<Option<Vec<LaneText>>, String> + 'a;

/// Numbers as a comma-separated list, for the log.
fn join<T: ToString>(items: &[T]) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// The digests the session reports for the lanes the game read.
pub fn lane_digests(lanes: &[LaneText]) -> Vec<LaneDigest> {
    lanes
        .iter()
        .map(|(lane, text)| {
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(digest(&SHA256, text.as_bytes()).as_ref());
            LaneDigest {
                lane: *lane,
                digest: FixedBytes(bytes),
            }
        })
        .collect()
}

pub struct StepDriver<G> {
    gate: G,
    game: HookGame,
    control: Box<dyn GameControl>,
    /// Names this game's saves apart from another game's on this PC, which
    /// shares the save folder.
    tag: String,
    saving: Option<Saving>,
    loading: Option<Loading>,
    replaying: Option<Instant>,
    phase: Phase,
    /// The speed row's last value in the room's game, once seen.
    chosen: Option<u64>,
    /// Steps between checkpoints, from the room's `Begin`.
    checkpoint_interval: u64,
    /// The batch chosen last ends at a checkpoint step, this one.
    lanes_due: bool,
    checkpoint_step: u64,
    /// Why the last call of the step runs the updates it does, for the
    /// step trace ([`crate::steptrace`]).
    why: &'static str,
    /// The lane dumps to come ([`crate::lanedump`]).
    dumps: LaneDumps,
    /// The player's actions handed to the room and not yet ordered back:
    /// the ticket the mod was given for each, by its client sequence number.
    tickets: HashMap<u64, u64>,
    /// The tickets of the player's actions that will never happen, and why.
    refused: Vec<(u64, String)>,
    /// The times the game came to its main menu ([`StepDriver::on_menu`]).
    menus: u64,
    /// Whether the game is at its menu: no step ran since the last
    /// [`StepDriver::on_menu`].
    at_menu: bool,
    /// Leave requested; only the main menu may discard the old world's work.
    menu_departing: bool,
    /// What the menu last said it cannot do, so it is logged once.
    menu_said: Option<&'static str>,
    /// The room's step the next update runs, once a world is loaded: what
    /// the per-update reseed ([`crate::seeds`]) numbers updates by.
    next_step: Option<u64>,
    /// The mark of the world the room's game was taken in (its load, or
    /// the owner's world it starts from): a world up with another mark is
    /// none the room loaded, and is held ([`StepDriver::foreign_world`]).
    room_world: Option<WorldMark>,
    /// How many released steps each call runs ([`crate::cadence`]).
    cadence: Cadence,
    /// When the player last handed the room an action.
    handed_at: Option<Instant>,
    /// The batch interval times each call ([`crate::interval`]).
    batch_interval: bool,
    /// The driver ran the room's game after the last call: leaving it (an
    /// end, the menu, playing alone) starts a new epoch for the interval.
    was_running: bool,
    /// How the last call's count was chosen, when it ran the room's steps.
    choice: Option<Choice>,
    /// Lines for the hook's log.
    log: Vec<String>,
    /// What last went wrong with the lobby, logged once.
    lobby_fault: Option<String>,
}

impl<G: RoomGate> StepDriver<G> {
    pub fn new(gate: G, control: Box<dyn GameControl>) -> Self {
        Self {
            gate,
            game: HookGame::default(),
            control,
            tag: std::process::id().to_string(),
            saving: None,
            loading: None,
            replaying: None,
            phase: Phase::BeforeBegin,
            chosen: None,
            checkpoint_interval: u64::MAX,
            lanes_due: false,
            checkpoint_step: 0,
            why: "-",
            dumps: LaneDumps::default(),
            tickets: HashMap::new(),
            refused: Vec::new(),
            menus: 0,
            at_menu: false,
            menu_departing: false,
            menu_said: None,
            next_step: None,
            room_world: None,
            cadence: Cadence::new(false),
            handed_at: None,
            batch_interval: false,
            was_running: false,
            choice: None,
            log: Vec::new(),
            lobby_fault: None,
        }
    }

    /// Whether each call runs an even share of the released steps
    /// ([`crate::cadence`]) or, off, all of them it may.
    pub fn set_even_steps(&mut self, on: bool) {
        self.cadence.set_on(on);
    }

    /// After a call or a menu frame: leaving the room's running game starts
    /// a new epoch, so nothing published in it times anything after.
    fn note_phase(&mut self) {
        let running = self.phase == Phase::Running;
        if self.was_running && !running {
            crate::interval::SLOTS.next_epoch();
        }
        self.was_running = running;
    }

    /// Whether the batch interval times each call ([`crate::interval`]):
    /// the cadence then keeps a fixed nominal count.
    pub fn set_batch_interval(&mut self, on: bool) {
        self.batch_interval = on;
        self.cadence.set_fixed(on);
    }

    /// For the batch interval, after a call: whether its batch may be timed,
    /// the room's time per step, the nominal count, the steps released and
    /// not run, and whether the count was the steady one.
    pub fn batch_info(&self) -> crate::interval::BatchInfo {
        let next = self.gate.next_step();
        let released = self.gate.released();
        crate::interval::BatchInfo {
            eligible: self.batch_interval
                && self.cadence.fixed()
                && self.cadence.on()
                && self.phase == Phase::Running
                && !self.cadence.paused(),
            step_us: self.cadence.step_us(),
            nominal: self.cadence.fixed_nominal(),
            backlog: u32::try_from(released.saturating_add(1).saturating_sub(next))
                .unwrap_or(u32::MAX),
            steady: self
                .choice
                .is_some_and(|choice| choice.pick == Pick::Nominal),
            waiting: self.why == "wait",
        }
    }

    /// Whether the player's own action is on its way through the room: one
    /// handed over lately and not yet ordered back. Lately, because an
    /// action lost on a dropped link is never ordered back nor refused, and
    /// must not keep even steps off for the rest of the game.
    fn command_on_its_way(&self) -> bool {
        !self.tickets.is_empty()
            && self
                .handed_at
                .is_some_and(|at| at.elapsed() < crate::cadence::COMMAND_WAIT)
    }

    /// For the step trace: how the last call chose its count of the
    /// room's steps (released, cap, nominal, rule).
    pub fn cadence_note(&self) -> Option<String> {
        self.choice.as_ref().map(Choice::note)
    }

    /// Which lanes to dump besides those a divergence asks for (the game's
    /// [`lanedump::ENV`]).
    pub fn set_lane_dumps(&mut self, dumps: LaneDumps) {
        self.dumps = dumps;
    }

    /// The main menu's Multiplayer window (D17): hands the launcher the
    /// player's `actions` and returns its lobby, if it sent a new one.
    /// Outside the room's game the driver reads the link itself first: at
    /// the main menu no step of the game does. The lobby never holds the
    /// world: a link that fails here fails the step gate's next read too.
    pub fn lobby(&mut self, actions: Vec<LobbyAction>) -> Option<LobbyView> {
        let mut fault = None;
        for action in actions {
            let leaving = matches!(action, LobbyAction::Leave | LobbyAction::Disconnect)
                && matches!(self.phase, Phase::Running | Phase::Holding(_));
            if let Err(error) = self.gate.lobby_act(action) {
                fault = Some(format!(
                    "the launcher did not hear the lobby window: {error}"
                ));
            } else if leaving {
                self.menu_departing = true;
            }
        }
        if matches!(self.phase, Phase::BeforeBegin | Phase::Ended)
            && let Err(error) = self.gate.poll_lobby()
        {
            fault = Some(format!("reading the launcher's lobby failed: {error}"));
        }
        if fault.is_some() && fault != self.lobby_fault {
            self.log.extend(fault.clone());
        }
        self.lobby_fault = fault;
        self.gate.take_lobby()
    }

    /// The tickets of the player's actions that will never happen (the room
    /// refused them, or there was no room's game to hand them to), and why:
    /// the mod tells the window that sent each one.
    pub fn take_refused(&mut self) -> Vec<(u64, String)> {
        for (seq, why) in std::mem::take(&mut self.game.refused) {
            if let Some(ticket) = self.tickets.remove(&seq) {
                self.refused
                    .push((ticket, format!("the room refused it: {why}")));
            }
        }
        std::mem::take(&mut self.refused)
    }

    /// Says `text` to the room for the player, in the room's game only:
    /// what the Multiplayer window's chat sends.
    pub fn say(&mut self, text: ChatText) {
        if self.phase != Phase::Running {
            self.log
                .push("the player said something outside the room's game; nobody heard".into());
            return;
        }
        if let Err(error) = self.gate.chat(text) {
            self.log
                .push(format!("the room did not hear the player: {error}"));
        }
    }

    /// Shows the room's other members what the player's build tool shows
    /// now, in the room's game only; outside it, nobody is shown anything.
    pub fn preview(&mut self, preview: Option<Payload>) {
        if self.phase != Phase::Running {
            return;
        }
        if let Err(error) = self.gate.preview(preview) {
            self.log.push(format!(
                "the room was not shown the player's preview: {error}"
            ));
        }
    }

    /// The game's speed row says `speedup` (0 paused, 1 for 1x, ...). In the
    /// room's game, a change the player makes there asks the room for that
    /// speed; the value found on entering the room's game is taken as it is,
    /// so joining never resets a room's speed.
    pub fn chosen_speed(&mut self, speedup: u64) {
        if self.phase != Phase::Running {
            self.chosen = None;
            return;
        }
        match self.chosen {
            None => self.chosen = Some(speedup),
            Some(before) if before == speedup => {}
            Some(_) => {
                self.chosen = Some(speedup);
                let percent = u16::try_from(speedup.saturating_mul(100)).unwrap_or(u16::MAX);
                let speed = Speed(percent.min(Speed::MAX.0));
                match self.gate.request_speed(speed) {
                    Ok(()) => self.log.push(format!(
                        "the speed row asks the room for speed {}%",
                        speed.0
                    )),
                    // A speed the room did not hear is not a reason to stop
                    // following it: the room's speed simply stays.
                    Err(error) => self
                        .log
                        .push(format!("asking the room for a speed failed: {error}")),
                }
            }
        }
    }

    /// In the room's game: the driver follows a room's game, or holds it.
    pub fn in_room(&self) -> bool {
        matches!(self.phase, Phase::Running | Phase::Holding(_))
    }

    /// The room's step the next update runs, once a world is loaded.
    pub fn next_step(&self) -> Option<u64> {
        self.next_step
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn game(&self) -> &HookGame {
        &self.game
    }

    /// The log lines since the last call, for the hook's log file.
    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }

    /// In place of one call of the game's step. `commands` are the actions
    /// the player handed over since the last call, for the room; `run` runs
    /// the game's own step, exactly once, with the updates given. Ordered
    /// actions finish in the engine event phase before updates are released.
    pub fn on_step(&mut self, commands: Vec<Handed>, run: &mut RunStep<'_>) -> Outcome {
        // A world is up: the next time the menu drives, the game came back
        // to it.
        self.at_menu = false;
        self.menu_said = None;
        // The batch interval may have turned itself off (a fault in the
        // main thread's wrapper): the cadence then measures again.
        let fixed = self.batch_interval && crate::interval::armed();
        if self.cadence.fixed() != fixed {
            self.cadence.set_fixed(fixed);
            if !fixed {
                self.log.push(
                    "the batch interval is off: even steps measure the game's call period again"
                        .into(),
                );
            }
        }
        self.cadence.call(Instant::now());
        let mut updates = self.updates();
        if let Some(fault) = self.game.fault.take() {
            self.hold(fault);
            updates = Updates::Exactly(0);
        }
        // After the gate is read: the call that begins the room's game
        // already hands the player's actions over.
        self.hand_over(commands);
        for notice in self.game.notices.drain(..) {
            self.log.push(format!("the room says: {notice}"));
        }
        // The session hears the room only in the gate's calls above.
        for notice in std::mem::take(&mut self.game.window) {
            if let Notice::Speed(speed) = notice {
                self.cadence.speed(speed);
            }
            self.plan_dumps(&notice);
            self.control.room_notice(&notice);
        }
        // Ordered commands finish in the engine's event phase before any
        // simulation update is allowed to run.
        let runs = matches!(updates, Updates::Exactly(steps) if steps > 0);
        // The per-update reseed numbers exactly the updates this call runs
        // for the room; anything else disarms it.
        let released = match (self.phase == Phase::Running, updates) {
            (true, Updates::Exactly(steps)) if steps > 0 => self.next_step,
            _ => None,
        };
        let lanes = runs && self.lanes_due;
        let dump = if lanes && self.phase == Phase::Running {
            self.dumps.take(self.checkpoint_step)
        } else {
            None
        };
        if let Some(dump) = &dump {
            self.log.push(format!(
                "dumping lanes {} at the checkpoint after step {} ({}): lines \"lane <n> step {}\"",
                join(&dump.lanes),
                dump.step,
                dump.why,
                dump.step
            ));
        }
        let batch = Batch {
            updates,
            lanes,
            room: self.in_room(),
            first_step: released,
            dump: dump.as_ref(),
        };
        crate::seeds::before_updates(released, updates);
        match run(&batch) {
            Ok(lanes) => {
                if batch.lanes {
                    match lanes {
                        Some(lanes) => self.game.lanes = Some(lane_digests(&lanes)),
                        // The steps ran, but the room cannot hear whether the
                        // world is still its own: none is reported.
                        None => {
                            self.hold(format!(
                                "the game did not read the world's lanes at the checkpoint after step {}",
                                self.gate.next_step().saturating_add(u64::from(match updates {
                                    Updates::Exactly(steps) => steps,
                                    Updates::Own => 0,
                                })).saturating_sub(1)
                            ));
                            self.note_phase();
                            return Outcome { updates };
                        }
                    }
                }
                if let Updates::Exactly(steps) = updates {
                    for _ in 0..steps {
                        match self.gate.after_step(&mut self.game) {
                            Ok(step) => self.next_step = Some(step + 1),
                            Err(error) => {
                                self.hold(format!("reporting a step: {error}"));
                                break;
                            }
                        }
                    }
                }
            }
            // A failed update batch is not reported; the world holds.
            Err(reason) => self.hold(format!("the game did not follow the room's step: {reason}")),
        }
        self.note_phase();
        Outcome { updates }
    }

    /// A divergence, or a line of the room's chat asking for a lane dump:
    /// plans the dump (docs/HOOKS.md, "Lane dumps"). A divergence is said in
    /// the room's chat, for every game to dump the same lanes at the same
    /// steps.
    fn plan_dumps(&mut self, notice: &Notice) {
        if self.phase != Phase::Running {
            return;
        }
        let next = self.gate.next_step();
        let now = Instant::now();
        match notice {
            Notice::Diverged { step, lanes } => {
                let Some(ask) =
                    self.dumps
                        .diverged(*step, lanes, next, self.checkpoint_interval, now)
                else {
                    if !self.dumps.setting().off {
                        self.log.push(format!(
                            "step {step} diverged; lanes were dumped less than a minute ago, so not again yet"
                        ));
                    }
                    return;
                };
                self.log.push(format!(
                    "step {step} diverged: dumping lanes {} at steps {}, and asking every game in the room to",
                    join(&ask.lanes),
                    join(&ask.steps)
                ));
                match ChatText::new(lanedump::announce(&ask)) {
                    Ok(text) => {
                        if let Err(error) = self.gate.chat(text) {
                            self.log.push(format!(
                                "the room did not hear the lane dump asked: {error}"
                            ));
                        }
                    }
                    Err(_) => self
                        .log
                        .push("the lane dump asked is too long for the room's chat".into()),
                }
            }
            Notice::Chat { from, text } => {
                match self
                    .dumps
                    .heard(text.as_str(), next, self.checkpoint_interval, now)
                {
                    None => {}
                    Some(Ok(ask)) => {
                        self.log.push(format!(
                            "{} asks for a lane dump: lanes {} at steps {} (step {} diverged there)",
                            from.as_str(),
                            join(&ask.lanes),
                            join(&ask.steps),
                            ask.diverged
                        ));
                        // The road entry recorder's last steps, in every
                        // game that hears the ask (docs/HOOKS.md, "The road
                        // entry trace").
                        crate::roadtrace::flush_recorded(&format!(
                            "{} asked for a lane dump, step {} diverged",
                            from.as_str(),
                            ask.diverged
                        ));
                    }
                    Some(Err(why)) => self.log.push(format!(
                        "not taking the lane dump {} asks for: {why}",
                        from.as_str()
                    )),
                }
            }
            _ => {}
        }
    }

    /// Hands the player's actions to the room, in the room's game only: an
    /// action handed over nowhere never happens, which is what the mod
    /// expects of one it could not hand over.
    fn hand_over(&mut self, commands: Vec<Handed>) {
        if commands.is_empty() {
            return;
        }
        if self.phase != Phase::Running {
            self.log.push(format!(
                "refused {} action(s) of the player: the game is not following a room's game",
                commands.len()
            ));
            for (ticket, ..) in commands {
                self.refused
                    .push((ticket, "the game is not following a room's game".into()));
            }
            return;
        }
        let mut commands = commands.into_iter();
        for (ticket, payload, secret) in commands.by_ref() {
            let kind = Action::from_payload(&payload).map_or("unreadable", |a| a.kind());
            match self.gate.command(payload, secret) {
                Ok(number) => {
                    self.tickets.insert(number, ticket);
                    self.handed_at = Some(Instant::now());
                    self.log.push(format!(
                        "handed the player's action {number} ({kind}) to the room"
                    ));
                }
                Err(error) => {
                    self.refused.push((ticket, format!("{error}")));
                    self.hold(format!("handing an action to the room: {error}"));
                    break;
                }
            }
        }
        for (ticket, ..) in commands {
            self.refused
                .push((ticket, "the game stopped following the room".into()));
        }
    }

    /// How many updates this call of the game's step runs.
    fn updates(&mut self) -> Updates {
        self.why = "-";
        self.choice = None;
        let updates = self.choose_updates();
        if matches!(updates, Updates::Own) {
            self.why = "own";
        }
        updates
    }

    /// Why the last call of the step ran the updates it did: `run`, `wait`
    /// (the room's next step is not released), `reserve` (released steps
    /// kept back so the calls stay even, [`crate::cadence`]), `actions` and
    /// `replaying`
    /// (the room's actions), `save`, `load`, `own`; `hold` whenever the
    /// world is held once the call is done, whatever the call was doing.
    pub fn why(&self) -> &'static str {
        if matches!(self.phase, Phase::Holding(_)) {
            "hold"
        } else {
            self.why
        }
    }

    fn choose_updates(&mut self) -> Updates {
        if self.phase == Phase::BeforeBegin {
            match self.gate.try_begin() {
                Ok(Some(begin)) => self.began(&begin, ""),
                Ok(None) => {
                    self.tell_world_up();
                    return match self.phase {
                        Phase::Holding(_) => Updates::Exactly(0),
                        _ => Updates::Own,
                    };
                }
                Err(error) => self.play_alone(&error),
            }
        }
        match self.phase {
            Phase::BeforeBegin | Phase::Holding(_) => return Updates::Exactly(0),
            Phase::Ended => return Updates::Own,
            Phase::Running => {}
        }
        // Never replay room actions into a world the room did not load.
        if let Some(why) = self.foreign_world() {
            self.hold(why);
            return Updates::Exactly(0);
        }
        if let Some(since) = self.replaying {
            self.gate.heartbeat();
            match self.control.replay_result() {
                Some(Ok(())) => {
                    self.replaying = None;
                    self.log.push(
                        "the game applied the room's actions between simulation updates".into(),
                    );
                }
                Some(Err(why)) => {
                    self.hold(format!("applying the room's actions: {why}"));
                    return Updates::Exactly(0);
                }
                None => {
                    if since.elapsed() >= Duration::from_secs(30) {
                        self.hold("the game did not finish the room's actions within 30 s".into());
                    }
                    self.why = "replaying";
                    return Updates::Exactly(0);
                }
            }
        }
        loop {
            let gate = self.gate.poll_step(&mut self.game);
            // Apply before any release, save, load or end that follows these
            // events. Even a paused world receives commands on its sim thread.
            if gate.is_ok() && self.game.fault.is_none() && !self.game.actions.is_empty() {
                let actions: Vec<_> = std::mem::take(&mut self.game.actions)
                    .into_iter()
                    .map(|(action, own, player, seal)| Ordered {
                        action,
                        ticket: own.and_then(|seq| self.tickets.remove(&seq)),
                        player,
                        seal,
                    })
                    .collect();
                match self.control.request_replay(self.gate.next_step(), &actions) {
                    Ok(()) => self.replaying = Some(Instant::now()),
                    Err(why) => self.hold(format!("starting the room's actions: {why}")),
                }
                self.why = "actions";
                return Updates::Exactly(0);
            }
            match gate {
                Ok(StepGate::Run) => {
                    let first = self.gate.next_step();
                    if self.cadence.on()
                        && let Err(error) = self.gate.read_ahead(&mut self.game)
                    {
                        self.hold(format!("reading the steps released: {error}"));
                        return Updates::Exactly(0);
                    }
                    // A speed read just now counts for this call already.
                    for notice in &self.game.window {
                        if let Notice::Speed(speed) = notice {
                            self.cadence.speed(*speed);
                        }
                    }
                    return match self.gate.batch(&mut self.game, MAX_STEPS_PER_CALL) {
                        Ok(cap) => {
                            let interval = self.checkpoint_interval.max(1);
                            let to_checkpoint = (interval - first % interval) % interval + 1;
                            let choice = self.cadence.choose(
                                first,
                                self.gate.released(),
                                cap,
                                to_checkpoint,
                                self.gate.barrier_waiting(),
                                self.command_on_its_way(),
                                Instant::now(),
                            );
                            crate::perf::cadence_pick(choice.pick);
                            self.choice = Some(choice);
                            if choice.steps == 0 && choice.pick != Pick::Off {
                                self.why = "reserve";
                                return Updates::Exactly(0);
                            }
                            self.why = "run";
                            let steps = choice.steps.max(1);
                            let last = first.saturating_add(u64::from(steps) - 1);
                            self.lanes_due = last.is_multiple_of(self.checkpoint_interval);
                            self.checkpoint_step = last;
                            Updates::Exactly(steps)
                        }
                        Err(error) => {
                            self.hold(format!("reading the steps released: {error}"));
                            Updates::Exactly(0)
                        }
                    };
                }
                Ok(StepGate::Wait) => {
                    self.why = "wait";
                    return Updates::Exactly(0);
                }
                Ok(StepGate::Save(order)) => {
                    if !self.save(&order) {
                        self.why = "save";
                        return Updates::Exactly(0);
                    }
                }
                Ok(StepGate::Load(load)) => match load.file {
                    // The world every player starts from: the one this
                    // game has loaded, which the launcher started for the
                    // room.
                    None => {
                        if let Err(error) = self.gate.loaded(load.next_step) {
                            self.hold(format!("taking the loaded world: {error}"));
                            return Updates::Exactly(0);
                        }
                        self.log.push(format!(
                            "playing the room's world from step {}",
                            load.next_step
                        ));
                        self.world_loaded(load.next_step);
                    }
                    Some(file) => {
                        if !self.load(&file, load.next_step, LoadFrom::Gui) {
                            self.why = "load";
                            return Updates::Exactly(0);
                        }
                    }
                },
                Ok(StepGate::Ended) => {
                    self.log
                        .push("the room's game ended; the game runs on its own".into());
                    self.phase = Phase::Ended;
                    return Updates::Own;
                }
                Err(error) => {
                    self.hold(error.to_string());
                    return Updates::Exactly(0);
                }
            }
        }
    }

    /// The room began a game: follow it from here. `place` says where the
    /// game is, for the log.
    fn began(&mut self, begin: &Begin, place: &str) {
        self.log.push(format!(
            "the room began a game{place}: rules {}, {} steps a second, checkpoints every {}",
            begin.rules.as_str(),
            begin.steps_per_second,
            begin.checkpoint_interval
        ));
        self.checkpoint_interval = u64::from(begin.checkpoint_interval).max(1);
        self.cadence.begin(begin.steps_per_second);
        crate::interval::SLOTS.next_epoch();
        self.game.me = Some(begin.player);
        self.control.set_me(begin.player);
        self.control.set_room(begin.room);
        match &begin.mods {
            Some(mods) => self.log.push(format!(
                "the room's worlds load with the {} shared mods and this player's {} personal ones",
                mods.shared.len(),
                mods.personal.len()
            )),
            None => self.log.push(
                "no mod lists from the launcher: the room's worlds load with their saves' mods"
                    .into(),
            ),
        }
        self.control.set_mods(begin.mods.clone());
        self.phase = Phase::Running;
    }

    /// In the room's lobby: tells the agent of a new world the game has up,
    /// with the mod linked (it said so through `world()`), for the agent to
    /// mark the player ready. The game steps it, so it is up, not one being
    /// replaced; a world that started and was replaced before this call is
    /// never told.
    fn tell_world_up(&mut self) {
        let Some(world) = self.control.world_up() else {
            return;
        };
        match self.gate.world_up(world) {
            Ok(true) => self.log.push(format!(
                "world {world} is up with the mod linked: told the agent, which marks the player ready"
            )),
            Ok(false) => {}
            Err(error) => self.hold(format!("telling the agent the world is up: {error}")),
        }
    }

    /// Moves the room's save on: asks the game for it, waits, and reports
    /// it once the game answers. Returns whether it is reported, so the
    /// room's steps may go on; until then the world stands still.
    fn save(&mut self, order: &SaveOrder) -> bool {
        let now = Instant::now();
        let Some(saving) = &self.saving else {
            let name = format!("tpf3mp_{}_{}", self.tag, order.event);
            self.control.request_save(&name);
            self.log.push(format!(
                "saving the world for the room (event {}) as {name}",
                order.event
            ));
            self.saving = Some(Saving {
                event: order.event,
                since: now,
            });
            return false;
        };
        let outcome = match self.control.save_result() {
            None if now.saturating_duration_since(saving.since) < SAVE_PATIENCE => return false,
            None => Err(format!(
                "the game did not save within {} s",
                SAVE_PATIENCE.as_secs()
            )),
            Some(Err(reason)) => Err(reason),
            Some(Ok(written)) => move_file(&written, &order.file).map_err(|error| {
                format!(
                    "moving the save {} to {}: {error}",
                    written.display(),
                    order.file.display()
                )
            }),
        };
        let event = saving.event;
        self.saving = None;
        match &outcome {
            Ok(()) => self
                .log
                .push(format!("saved the world for the room (event {event})")),
            Err(reason) => self.log.push(format!(
                "the world was not saved for the room (event {event}): {reason}"
            )),
        }
        if let Err(error) = self.gate.saved(&mut self.game, outcome) {
            self.hold(format!("reporting a save: {error}"));
            return false;
        }
        true
    }

    /// Moves a load of the room's save on: asks the game to load it (from
    /// `from`, if it is not loading it yet), then waits for its world.
    /// Returns whether the world is loaded; until then the world stands
    /// still.
    fn load(&mut self, file: &Path, next_step: u64, from: LoadFrom) -> bool {
        let now = Instant::now();
        let Some(loading) = &self.loading else {
            match self.control.request_load(file, from) {
                Ok(()) => {
                    self.log.push(format!(
                        "loading the room's world from {}{} to run step {next_step} next",
                        file.display(),
                        match from {
                            LoadFrom::Gui => "",
                            LoadFrom::Menu => " from the game's main menu",
                        }
                    ));
                    self.loading = Some(Loading {
                        next_step,
                        since: now,
                    });
                }
                Err(reason) => self.hold(format!("loading the room's world: {reason}")),
            }
            return false;
        };
        if let Some(why) = self.control.load_failed() {
            self.loading = None;
            self.hold(format!("the room's world could not be loaded: {why}"));
            return false;
        }
        if !self.control.load_done() {
            if now.saturating_duration_since(loading.since) >= LOAD_PATIENCE {
                self.hold(format!(
                    "the room's world did not load within {} s",
                    LOAD_PATIENCE.as_secs()
                ));
            }
            return false;
        }
        let next_step = loading.next_step;
        self.loading = None;
        if let Err(error) = self.gate.loaded(next_step) {
            self.hold(format!("taking the room's world: {error}"));
            return false;
        }
        self.log.push(format!(
            "playing the room's world from its save, from step {next_step}"
        ));
        self.world_loaded(next_step);
        true
    }

    /// The room's world is loaded and runs `next_step` next: the reseed and
    /// the order measurement number updates by the room's steps from here.
    fn world_loaded(&mut self, next_step: u64) {
        self.next_step = Some(next_step);
        // The released counter starts again here, whatever world ran before;
        // so do the batches the interval times (new GameSims).
        self.cadence
            .reset(next_step.saturating_sub(1), Instant::now());
        crate::interval::SLOTS.next_epoch();
        self.room_world = Some(self.control.world_mark());
        crate::order::measure::room_step(next_step);
    }

    /// Whether the room's world closed in this game since it was taken,
    /// with no load of the room's under way: whatever world is up after is
    /// not the room's.
    fn room_world_gone(&mut self) -> bool {
        if self.loading.is_some() {
            return false;
        }
        let Some(then) = self.room_world else {
            return false;
        };
        self.control.world_mark().replaced_since(&then)
    }

    /// In the room's game, with a world up: why that world is not the one
    /// the room loaded, if it is not. The room's world closed (the player
    /// went back to the main menu) and the game started another of its
    /// own: a new game, or a save. Only a load the room orders replaces the
    /// room's world, in every game at the same step, from the same save
    /// (docs/HOOKS.md, "A world the room did not load").
    fn foreign_world(&mut self) -> Option<String> {
        if !self.room_world_gone() {
            return None;
        }
        let now = self.control.world_mark();
        let then = self.room_world.unwrap_or_default();
        Some(format!(
            "this game has a world up the room did not load (worlds closed: {} when the room's was taken, {} now; worlds started: {}, {}): the room's world closed in this game, and a new game or a save started here would run the room's steps from step {} on, apart from every other game's; leave the room and join again to play its world",
            then.closed.map_or("unknown".to_owned(), |n| n.to_string()),
            now.closed.map_or("unknown".to_owned(), |n| n.to_string()),
            then.started,
            now.started,
            self.gate.next_step()
        ))
    }

    /// At the game's main menu, with no world up, on each of the menu's
    /// frames (the step's detour gets no call there; `crate::install` calls
    /// this only while no world is loaded, after a world only once it closed
    /// and nothing loads, `crate::at_menu`, and the menu can load a save,
    /// `crate::menu`). It
    /// follows the room as the step does, but runs nothing:
    ///
    /// - before the room begins a game, it reads whether it did, and tells
    ///   the agent the game is at its menu, once per arrival
    ///   ([`RoomGate::menu_up`]), which marks a guest ready, and the owner
    ///   once the room has the save it starts from;
    /// - in the room's game, it starts a load of the room's save from the
    ///   menu ([`LoadFrom::Menu`]) and waits for it as the step does; the
    ///   step takes the loaded world once it runs;
    /// - a load without a file (the owner's own world) and a save need a
    ///   world up: the menu leaves them to the step, and logs so once.
    pub fn on_menu(&mut self) {
        // The world is closed. A wake queued for it must never reach a new
        // world, even if the player leaves or reconnects before it arrives.
        if self.replaying.take().is_some() {
            self.control.cancel_replay();
        }
        if self.menu_departing {
            match self.gate.poll_departure() {
                Ok(true) => {
                    self.phase = Phase::Ended;
                    self.menu_departing = false;
                }
                Ok(false) => return,
                Err(error) => {
                    self.hold(format!("leaving the old world: {error}"));
                    return;
                }
            }
        }
        if self.phase == Phase::Ended && self.gate.return_to_lobby() {
            self.phase = Phase::BeforeBegin;
            self.saving = None;
            self.loading = None;
            self.tickets.clear();
            self.menu_said = None;
            self.room_world = None;
            self.log
                .push("back at the menu: ready for another multiplayer room".into());
        }
        if !self.at_menu {
            self.at_menu = true;
            self.menus += 1;
        }
        if self.phase == Phase::BeforeBegin {
            match self.gate.try_begin() {
                Ok(Some(begin)) => self.began(&begin, " while this game is at its main menu"),
                Ok(None) => {
                    match self.gate.menu_up(self.menus) {
                        Ok(true) => self.log.push(format!(
                            "the game is at its main menu (arrival {}): told the agent, which marks a guest ready, or the owner once the room has the save it starts from",
                            self.menus
                        )),
                        Ok(false) => {}
                        Err(error) => {
                            self.hold(format!("telling the agent the game is at its menu: {error}"));
                        }
                    }
                    return;
                }
                Err(error) => {
                    self.play_alone(&error);
                    return;
                }
            }
        }
        if self.phase != Phase::Running {
            return;
        }
        if let Some(fault) = self.game.fault.take() {
            self.hold(fault);
            return;
        }
        match self.gate.poll_step(&mut self.game) {
            Ok(StepGate::Load(load)) => match load.file {
                Some(file) => {
                    self.load(&file, load.next_step, LoadFrom::Menu);
                }
                None => self.menu_says(
                    "the room plays the world this game starts from, which the main menu cannot pick: load it (the room's owner loads the world everyone plays)",
                ),
            },
            Ok(StepGate::Save(_) | StepGate::Run | StepGate::Wait) if self.room_world_gone() => self
                .menu_says(
                    "the room's world closed in this game: a new game or a save started now is not the room's, and is held (fail closed); leave the room and join again to play its world",
                ),
            Ok(StepGate::Save(_)) => self.menu_says(
                "the room asks this game to save its world, which needs a world up: load it from the main menu",
            ),
            Ok(StepGate::Ended) => {
                self.log
                    .push("the room's game ended; the game runs on its own".into());
                self.phase = Phase::Ended;
            }
            Ok(StepGate::Run | StepGate::Wait) => {}
            Err(error) => self.hold(error.to_string()),
        }
        for notice in self.game.notices.drain(..) {
            self.log.push(format!("the room says: {notice}"));
        }
        // The Multiplayer window of the world the menu loads shows what the
        // room said meanwhile; the room's speed paces the world it loads.
        for notice in std::mem::take(&mut self.game.window) {
            if let Notice::Speed(speed) = notice {
                self.cadence.speed(speed);
            }
            self.control.room_notice(&notice);
        }
        self.note_phase();
    }

    /// Logs what the menu cannot do, once until it changes.
    fn menu_says(&mut self, what: &'static str) {
        if self.menu_said != Some(what) {
            self.menu_said = Some(what);
            self.log.push(format!("at the main menu: {what}"));
        }
    }

    /// The launcher's link failed before any room's game began (a game
    /// frozen past the agent's heartbeat limit ends the agent's session):
    /// there is no room world to protect, so the game plays on by itself
    /// instead of being held, and says it cannot join a room until it is
    /// restarted from the launcher. In a room's game a failed link still
    /// holds the world.
    fn play_alone(&mut self, error: &SessionError) {
        self.log.push(format!(
            "the launcher's link failed before any room began ({error}): this game plays on alone; restart it from the launcher to join a room"
        ));
        self.phase = Phase::Ended;
    }

    fn hold(&mut self, reason: String) {
        if self.replaying.take().is_some() {
            self.control.cancel_replay();
        }
        self.log
            .push(format!("holding the world (fail closed): {reason}"));
        self.phase = Phase::Holding(reason);
        crate::interval::SLOTS.next_epoch();
    }
}

/// Moves a file, across drives too: a copy then a removal when a rename
/// cannot.
pub fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{collections::VecDeque, path::PathBuf};

    use tpf3mp_bridge::Load;
    use tpf3mp_proto::{FixedBytes, PlayerId, RulesName};

    use super::*;
    // Driver calls update the process-wide seed step used by the Lua link.
    use crate::lua::tests::{SERIAL, depot_build};

    /// A room that answers from a script.
    #[derive(Default)]
    pub(crate) struct Script {
        pub(crate) reset_ended: bool,
        pub(crate) departure_done: bool,
        pub(crate) departure_polls: usize,
        pub(crate) begin: VecDeque<Option<Begin>>,
        pub(crate) gates: VecDeque<StepGate>,
        pub(crate) ran: u64,
        pub(crate) loaded: Vec<u64>,
        pub(crate) fail_after: bool,
        pub(crate) speeds: Vec<Speed>,
        /// Events each poll applies before it answers, one list a poll.
        pub(crate) events: VecDeque<Vec<Event>>,
        pub(crate) commands: Vec<Payload>,
        /// The password handed over with each command.
        pub(crate) secrets: Vec<Option<Secret>>,
        pub(crate) saves: Vec<Result<(), String>>,
        /// Steps between checkpoints, from the begin handed out.
        pub(crate) interval: u64,
        /// The lanes reported, by checkpoint step.
        pub(crate) checkpoints: Vec<(u64, Vec<LaneDigest>)>,
        /// What the player said to the room.
        pub(crate) said: Vec<ChatText>,
        /// What the player's build tool showed the room.
        pub(crate) previews: Vec<Option<Payload>>,
        /// What the room says, handed to the game by each poll, one list a
        /// poll.
        pub(crate) notices: VecDeque<Vec<Notice>>,
        /// The worlds told up.
        pub(crate) worlds_up: Vec<u64>,
        /// The menu arrivals told, as the session tells them: once each.
        pub(crate) menus_up: Vec<u64>,
        /// The lobby window's actions the launcher heard.
        pub(crate) lobby_acts: Vec<LobbyAction>,
        /// The lobbies the link holds, one read by each `poll_lobby`.
        pub(crate) lobbies: VecDeque<LobbyView>,
        /// How often the link was read for the lobby, and whether that fails.
        pub(crate) lobby_polls: usize,
        pub(crate) lobby_fails: bool,
        /// The lobby read and not taken yet.
        pub(crate) lobby_heard: Option<LobbyView>,
        /// The link fails when asked whether the room began.
        pub(crate) begin_fails: bool,
    }

    impl RoomGate for Script {
        fn poll_departure(&mut self) -> Result<bool, SessionError> {
            self.departure_polls += 1;
            Ok(self.departure_done)
        }
        fn return_to_lobby(&mut self) -> bool {
            std::mem::take(&mut self.reset_ended)
        }
        fn try_begin(&mut self) -> Result<Option<Begin>, SessionError> {
            if self.begin_fails {
                return Err(SessionError::AgentGone);
            }
            let begin = self.begin.pop_front().flatten();
            if let Some(begin) = &begin {
                self.interval = u64::from(begin.checkpoint_interval).max(1);
            }
            Ok(begin)
        }
        fn next_step(&self) -> u64 {
            self.ran + 1
        }
        fn poll_step(&mut self, game: &mut HookGame) -> Result<StepGate, SessionError> {
            for event in self.events.pop_front().unwrap_or_default() {
                game.apply(&event);
            }
            for notice in self.notices.pop_front().unwrap_or_default() {
                game.notice(notice);
            }
            match self.gates.front() {
                // A Run stays until the step ran, a Save until it is
                // reported and a Load until the world is loaded, as the
                // session's do.
                Some(StepGate::Run) => Ok(StepGate::Run),
                Some(StepGate::Save(order)) => Ok(StepGate::Save(order.clone())),
                Some(StepGate::Load(load)) => Ok(StepGate::Load(load.clone())),
                _ => Ok(self.gates.pop_front().unwrap_or(StepGate::Wait)),
            }
        }
        /// The Runs in a row at the front of the script are one batch, up to
        /// the next checkpoint step, as the gate's are.
        fn batch(&mut self, _game: &mut HookGame, max: u32) -> Result<u32, SessionError> {
            let runs = self
                .gates
                .iter()
                .take_while(|gate| **gate == StepGate::Run)
                .count();
            let next = self.ran + 1;
            let to_checkpoint = match self.interval {
                0 => u64::MAX,
                interval => (interval - next % interval) % interval + 1,
            };
            let steps = u64::try_from(runs).unwrap_or(u64::MAX).min(to_checkpoint);
            Ok(u32::try_from(steps).unwrap_or(u32::MAX).min(max))
        }
        fn released(&self) -> u64 {
            let runs = self
                .gates
                .iter()
                .take_while(|gate| **gate == StepGate::Run)
                .count();
            self.ran + u64::try_from(runs).unwrap_or(u64::MAX)
        }
        fn after_step(&mut self, game: &mut HookGame) -> Result<u64, SessionError> {
            if self.fail_after {
                return Err(SessionError::AgentGone);
            }
            assert_eq!(
                self.gates.pop_front(),
                Some(StepGate::Run),
                "a step ran unreleased"
            );
            self.ran += 1;
            // As the session does: the lanes at checkpoint steps.
            if self.interval > 0 && self.ran.is_multiple_of(self.interval) {
                self.checkpoints.push((self.ran, game.lanes()));
            }
            Ok(self.ran)
        }
        fn loaded(&mut self, next_step: u64) -> Result<(), SessionError> {
            assert!(
                matches!(self.gates.pop_front(), Some(StepGate::Load(_))),
                "a world nobody ordered was loaded"
            );
            self.loaded.push(next_step);
            Ok(())
        }
        fn request_speed(&mut self, speed: Speed) -> Result<(), SessionError> {
            self.speeds.push(speed);
            Ok(())
        }
        fn command(
            &mut self,
            payload: Payload,
            secret: Option<Secret>,
        ) -> Result<u64, SessionError> {
            self.commands.push(payload);
            self.secrets.push(secret);
            Ok(self.commands.len() as u64 - 1)
        }
        fn chat(&mut self, text: ChatText) -> Result<(), SessionError> {
            self.said.push(text);
            Ok(())
        }
        fn preview(&mut self, preview: Option<Payload>) -> Result<(), SessionError> {
            self.previews.push(preview);
            Ok(())
        }
        fn world_up(&mut self, world: u64) -> Result<bool, SessionError> {
            self.worlds_up.push(world);
            Ok(true)
        }
        fn menu_up(&mut self, menu: u64) -> Result<bool, SessionError> {
            if self.menus_up.last() == Some(&menu) {
                return Ok(false);
            }
            self.menus_up.push(menu);
            Ok(true)
        }
        fn saved(
            &mut self,
            _game: &mut HookGame,
            outcome: Result<(), String>,
        ) -> Result<(), SessionError> {
            assert!(
                matches!(self.gates.pop_front(), Some(StepGate::Save(_))),
                "a save nobody ordered was reported"
            );
            self.saves.push(outcome);
            Ok(())
        }
        fn lobby_act(&mut self, action: LobbyAction) -> Result<(), SessionError> {
            self.lobby_acts.push(action);
            Ok(())
        }
        fn poll_lobby(&mut self) -> Result<(), SessionError> {
            self.lobby_polls += 1;
            if self.lobby_fails {
                return Err(SessionError::AgentGone);
            }
            if let Some(view) = self.lobbies.pop_front() {
                self.lobby_heard = Some(view);
            }
            Ok(())
        }
        fn take_lobby(&mut self) -> Option<LobbyView> {
            self.lobby_heard.take()
        }
    }

    /// The game's side of saves and loads, from the tests: what it was
    /// asked, and the answers they give it.
    #[derive(Default)]
    pub(crate) struct FakeControl {
        pub(crate) state: std::sync::Arc<std::sync::Mutex<ControlState>>,
    }

    #[derive(Default)]
    pub(crate) struct ControlState {
        pub(crate) replay_requests: Vec<(u64, Vec<Ordered>)>,
        pub(crate) replay_wait: bool,
        pub(crate) replay_answer: Option<Result<(), String>>,
        pub(crate) save_requests: Vec<String>,
        pub(crate) save_answer: Option<Result<PathBuf, String>>,
        pub(crate) load_requests: Vec<(PathBuf, LoadFrom)>,
        pub(crate) load_done: bool,
        pub(crate) load_failure: Option<String>,
        pub(crate) room_notices: Vec<Notice>,
        pub(crate) me: Option<PlayerId>,
        pub(crate) room: Option<RoomId>,
        pub(crate) mods: Option<ModLists>,
        pub(crate) world_up: Option<u64>,
        /// The worlds the game has had ([`GameControl::world_mark`]).
        pub(crate) mark: WorldMark,
    }

    impl GameControl for FakeControl {
        fn request_replay(&mut self, step: u64, actions: &[Ordered]) -> Result<(), String> {
            let mut s = self.state.lock().unwrap();
            s.replay_requests.push((step, actions.to_vec()));
            if !s.replay_wait {
                s.replay_answer = Some(Ok(()));
            }
            Ok(())
        }
        fn replay_result(&mut self) -> Option<Result<(), String>> {
            self.state.lock().unwrap().replay_answer.take()
        }
        fn cancel_replay(&mut self) {
            self.state.lock().unwrap().replay_answer = None;
        }
        fn request_save(&mut self, name: &str) {
            self.state
                .lock()
                .unwrap()
                .save_requests
                .push(name.to_owned());
        }
        fn save_result(&mut self) -> Option<Result<PathBuf, String>> {
            self.state.lock().unwrap().save_answer.take()
        }
        fn request_load(&mut self, file: &Path, from: LoadFrom) -> Result<(), String> {
            self.state
                .lock()
                .unwrap()
                .load_requests
                .push((file.to_owned(), from));
            Ok(())
        }
        fn load_done(&mut self) -> bool {
            std::mem::take(&mut self.state.lock().unwrap().load_done)
        }
        fn load_failed(&mut self) -> Option<String> {
            self.state.lock().unwrap().load_failure.take()
        }
        fn room_notice(&mut self, notice: &Notice) {
            self.state.lock().unwrap().room_notices.push(notice.clone());
        }
        fn set_me(&mut self, player: PlayerId) {
            self.state.lock().unwrap().me = Some(player);
        }
        fn set_room(&mut self, room: RoomId) {
            self.state.lock().unwrap().room = Some(room);
        }
        fn set_mods(&mut self, mods: Option<ModLists>) {
            self.state.lock().unwrap().mods = mods;
        }
        fn world_up(&mut self) -> Option<u64> {
            self.state.lock().unwrap().world_up.take()
        }
        fn world_mark(&mut self) -> WorldMark {
            self.state.lock().unwrap().mark
        }
    }

    pub(crate) fn command_event(seq: u64, step: u64, action: &Action) -> Event {
        Event {
            seq,
            step,
            body: EventBody::Command {
                player: PlayerId(FixedBytes([1; 32])),
                client_seq: seq,
                payload: action.to_payload().unwrap(),
                seal: None,
            },
        }
    }

    /// The local player of the tests' games: not the actor of
    /// `command_event`'s events.
    pub(crate) const ME: PlayerId = PlayerId(FixedBytes([9; 32]));

    pub(crate) fn begin() -> Begin {
        Begin {
            room: tpf3mp_proto::RoomId(tpf3mp_proto::FixedBytes([1; 16])),
            rules: RulesName::new("native").unwrap(),
            steps_per_second: 5,
            checkpoint_interval: 50,
            saves: PathBuf::from("saves"),
            player: ME,
            mods: None,
        }
    }

    /// The calls of the game's step, with the updates each ran.
    type Calls = Vec<Updates>;

    fn driver(script: Script) -> (StepDriver<Script>, Calls) {
        (
            StepDriver::new(script, Box::new(FakeControl::default())),
            Vec::new(),
        )
    }

    fn driver_with(
        script: Script,
    ) -> (
        StepDriver<Script>,
        std::sync::Arc<std::sync::Mutex<ControlState>>,
    ) {
        let control = FakeControl::default();
        let state = std::sync::Arc::clone(&control.state);
        (StepDriver::new(script, Box::new(control)), state)
    }

    fn call(driver: &mut StepDriver<Script>, calls: &mut Calls) -> Updates {
        let before = calls.len();
        let outcome = driver.on_step(Vec::new(), &mut |batch| {
            calls.push(batch.updates);
            Ok(batch.lanes.then(Vec::new))
        });
        assert_eq!(calls.len(), before + 1, "the game's step runs once a call");
        assert_eq!(calls.last(), Some(&outcome.updates));
        outcome.updates
    }

    /// One call, recording the actions the game was handed; the game
    /// applies them unless `applies` says not.
    fn call_applying(
        driver: &mut StepDriver<Script>,
        commands: Vec<Payload>,
        applied: &mut Vec<(Updates, Vec<Action>)>,
        applies: bool,
    ) -> Updates {
        let commands = commands
            .into_iter()
            .map(|payload| (0, payload, None))
            .collect();
        let outcome = driver.on_step(commands, &mut |batch| {
            applied.push((batch.updates, Vec::new()));
            if applies {
                Ok(batch.lanes.then(Vec::new))
            } else {
                Err("the script took nothing".into())
            }
        });
        outcome.updates
    }

    #[test]
    fn the_rooms_actions_apply_while_the_next_step_is_withheld() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: None,
                next_step: 1,
            }),
            StepGate::Run,
            StepGate::Wait,
            StepGate::Run,
            StepGate::Run,
            StepGate::Wait,
        ]);
        // The event for step 2 arrives while the game waits before it.
        script
            .events
            .extend([vec![], vec![], vec![command_event(1, 2, &depot_build())]]);
        let (mut d, _) = driver(script);
        let mut applied = Vec::new();
        assert_eq!(
            call_applying(&mut d, Vec::new(), &mut applied, true),
            Updates::Exactly(1)
        );
        assert_eq!(
            call_applying(&mut d, Vec::new(), &mut applied, true),
            Updates::Exactly(0),
            "the event arrived, step 2 is not released yet"
        );
        assert_eq!(
            call_applying(&mut d, Vec::new(), &mut applied, true),
            Updates::Exactly(2)
        );
        assert_eq!(
            applied,
            vec![
                (Updates::Exactly(1), vec![]),
                (Updates::Exactly(0), vec![]),
                (Updates::Exactly(2), vec![]),
            ],
            "the replay runs between updates, never in an update batch"
        );
        assert_eq!(d.phase(), &Phase::Running);
        assert!(d.take_log().iter().any(|l| l.contains("applied the room")));
    }

    fn begin_every(interval: u32) -> Begin {
        Begin {
            checkpoint_interval: interval,
            ..begin()
        }
    }

    #[test]
    fn ordered_actions_finish_before_resuming_and_are_not_applied_twice() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend([StepGate::Wait, StepGate::Run]);
        script.events.push_back(vec![
            command_event(1, 1, &depot_build()),
            command_event(2, 1, &depot_build()),
        ]);
        let control = FakeControl::default();
        let state = control.state.clone();
        state.lock().unwrap().replay_wait = true;
        let mut d = StepDriver::new(script, Box::new(control));
        let mut calls = Vec::new();
        assert_eq!(call(&mut d, &mut calls), PAUSED);
        assert_eq!(state.lock().unwrap().replay_requests[0].1.len(), 2);
        assert_eq!(
            call(&mut d, &mut calls),
            PAUSED,
            "resume waits for replay completion"
        );
        assert_eq!(d.gate.ran, 0);
        state.lock().unwrap().replay_answer = Some(Ok(()));
        assert_eq!(call(&mut d, &mut calls), Updates::Exactly(1));
        assert_eq!(call(&mut d, &mut calls), PAUSED);
        assert_eq!(state.lock().unwrap().replay_requests.len(), 1);
        assert_eq!(d.gate.ran, 1);
    }

    #[test]
    fn a_foreign_world_holds_before_pending_room_actions_can_finish() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.push_back(StepGate::Run);
        script
            .events
            .push_back(vec![command_event(1, 1, &depot_build())]);
        let control = FakeControl::default();
        let state = control.state.clone();
        state.lock().unwrap().mark = mark(0, 1);
        state.lock().unwrap().replay_wait = true;
        let mut d = StepDriver::new(script, Box::new(control));
        d.room_world = Some(mark(0, 1));
        let mut calls = Vec::new();
        assert_eq!(call(&mut d, &mut calls), PAUSED);
        assert_eq!(state.lock().unwrap().replay_requests.len(), 1);
        state.lock().unwrap().mark = mark(1, 2);
        state.lock().unwrap().replay_answer = Some(Ok(()));
        assert_eq!(call(&mut d, &mut calls), PAUSED);
        assert!(matches!(d.phase(), Phase::Holding(why) if why.contains("did not load")));
        assert_eq!(d.gate.ran, 0);
    }

    #[test]
    fn a_save_waits_for_all_preceding_ordered_actions() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script
            .gates
            .push_back(StepGate::Save(order(2, Path::new("unused.sav"))));
        script
            .events
            .push_back(vec![command_event(1, 1, &depot_build())]);
        let control = FakeControl::default();
        let state = control.state.clone();
        state.lock().unwrap().replay_wait = true;
        let mut d = StepDriver::new(script, Box::new(control));
        let mut calls = Vec::new();
        call(&mut d, &mut calls);
        call(&mut d, &mut calls);
        assert!(state.lock().unwrap().save_requests.is_empty());
        state.lock().unwrap().replay_answer = Some(Ok(()));
        call(&mut d, &mut calls);
        assert_eq!(state.lock().unwrap().save_requests.len(), 1);
        assert_eq!(d.gate.ran, 0);
    }

    #[test]
    fn a_failed_or_timed_out_replay_holds_before_any_update() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for timeout in [false, true] {
            let mut script = Script::default();
            script.begin.push_back(Some(begin()));
            script.gates.push_back(StepGate::Run);
            script
                .events
                .push_back(vec![command_event(1, 1, &depot_build())]);
            let control = FakeControl::default();
            let state = control.state.clone();
            state.lock().unwrap().replay_wait = true;
            let mut d = StepDriver::new(script, Box::new(control));
            let mut calls = Vec::new();
            call(&mut d, &mut calls);
            if timeout {
                d.replaying = Some(Instant::now() - Duration::from_secs(31));
            } else {
                state.lock().unwrap().replay_answer =
                    Some(Err("script state was not saved".into()));
            }
            assert_eq!(call(&mut d, &mut calls), PAUSED);
            assert!(matches!(d.phase(), Phase::Holding(_)));
            assert_eq!(d.gate.ran, 0);
        }
    }

    #[test]
    fn a_batch_ending_at_a_checkpoint_reports_the_lanes_the_game_read() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin_every(3)));
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: None,
                next_step: 1,
            }),
            StepGate::Run,
            StepGate::Run,
            StepGate::Run,
            StepGate::Run,
            StepGate::Wait,
        ]);
        let (mut d, _) = driver(script);
        let mut batches = Vec::new();
        for _ in 0..2 {
            d.on_step(Vec::new(), &mut |batch| {
                batches.push((batch.updates, batch.lanes));
                Ok(batch
                    .lanes
                    .then(|| vec![(3, "vehicles".to_owned()), (0, "net".to_owned())]))
            });
        }
        assert_eq!(
            batches,
            [(Updates::Exactly(3), true), (Updates::Exactly(1), false)],
            "the batch stops at step 3, a checkpoint, and asks for its lanes"
        );
        let checkpoints = &d.gate.checkpoints;
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].0, 3);
        assert_eq!(
            checkpoints[0].1,
            lane_digests(&[(3, "vehicles".to_owned()), (0, "net".to_owned())])
        );
        assert_ne!(
            checkpoints[0].1[0].digest, checkpoints[0].1[1].digest,
            "each lane its own digest"
        );
        assert_eq!(d.phase(), &Phase::Running);
    }

    /// Every call of the room's game says so, its paused ones included (they
    /// must not count a frame in the game's tickCount), and a batch that
    /// runs the room's steps says which step it starts at; a call before
    /// the room began is the game's own.
    #[test]
    fn a_batch_says_whether_it_is_the_rooms_and_the_step_it_starts_at() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.extend([None, Some(begin())]);
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: None,
                next_step: 1,
            }),
            StepGate::Run,
            StepGate::Wait,
            StepGate::Run,
            StepGate::Run,
            StepGate::Wait,
        ]);
        let (mut d, _) = driver(script);
        let mut seen = Vec::new();
        for _ in 0..4 {
            d.on_step(Vec::new(), &mut |batch| {
                seen.push((batch.updates, batch.room, batch.first_step));
                Ok(batch.lanes.then(Vec::new))
            });
        }
        assert_eq!(
            seen,
            [
                (Updates::Own, false, None),
                (Updates::Exactly(1), true, Some(1)),
                (Updates::Exactly(0), true, None),
                (Updates::Exactly(2), true, Some(2)),
            ]
        );
        assert!(d.in_room());
    }

    #[test]
    fn a_checkpoint_without_the_worlds_lanes_holds_the_world() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin_every(2)));
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: None,
                next_step: 1,
            }),
            StepGate::Run,
            StepGate::Run,
            StepGate::Run,
        ]);
        let (mut d, _) = driver(script);
        let outcome = d.on_step(Vec::new(), &mut |_batch| Ok(None));
        assert_eq!(outcome.updates, Updates::Exactly(2), "the steps ran");
        assert!(
            matches!(d.phase(), Phase::Holding(why) if why.contains("lanes at the checkpoint after step 2")),
            "{:?}",
            d.phase()
        );
        assert!(d.gate.checkpoints.is_empty(), "nothing reported for them");
        assert_eq!(d.gate.ran, 0);
        let mut calls = Vec::new();
        assert_eq!(call(&mut d, &mut calls), Updates::Exactly(0));
    }

    #[test]
    fn a_game_that_did_not_apply_the_rooms_actions_holds_the_world() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend([StepGate::Run, StepGate::Run]);
        script
            .events
            .push_back(vec![command_event(1, 1, &depot_build())]);
        let (mut d, _) = driver(script);
        let mut applied = Vec::new();
        call_applying(&mut d, Vec::new(), &mut applied, false);
        assert!(matches!(d.phase(), Phase::Holding(_)));
        assert_eq!(d.gate.ran, 0, "none of those steps is reported");
        assert_eq!(
            call_applying(&mut d, Vec::new(), &mut applied, true),
            PAUSED
        );
    }

    #[test]
    fn an_action_the_game_cannot_read_holds_the_world_before_its_step() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend([StepGate::Run]);
        let mut bad = command_event(1, 1, &depot_build());
        if let EventBody::Command { payload, .. } = &mut bad.body {
            *payload = Payload::new(vec![0xFF, 0xFF, 0xFF]).unwrap();
        }
        script.events.push_back(vec![bad]);
        let (mut d, _) = driver(script);
        let mut applied = Vec::new();
        assert_eq!(
            call_applying(&mut d, Vec::new(), &mut applied, true),
            PAUSED
        );
        assert!(matches!(d.phase(), Phase::Holding(reason) if reason.contains("cannot read")));
    }

    #[test]
    fn the_players_actions_go_to_the_room_only_in_its_game() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(None);
        script.begin.push_back(Some(begin()));
        script.gates.extend([StepGate::Run, StepGate::Ended]);
        let (mut d, _) = driver(script);
        let payload = depot_build().to_payload().unwrap();
        let mut applied = Vec::new();
        call_applying(&mut d, vec![payload.clone()], &mut applied, true);
        assert!(d.gate.commands.is_empty(), "no room's game yet");
        assert!(d.take_log().iter().any(|l| l.contains("refused 1 action")));
        call_applying(&mut d, vec![payload.clone()], &mut applied, true);
        assert_eq!(d.gate.commands, vec![payload.clone()]);
        call_applying(&mut d, Vec::new(), &mut applied, true);
        assert_eq!(d.phase(), &Phase::Ended);
        call_applying(&mut d, vec![payload], &mut applied, true);
        assert_eq!(d.gate.commands.len(), 1, "the room's game is over");
    }

    #[test]
    fn the_players_own_actions_come_back_with_their_tickets() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: None,
                next_step: 1,
            }),
            StepGate::Run,
            StepGate::Wait,
            StepGate::Run,
            StepGate::Wait,
        ]);
        // The room orders, for step 2, another player's action and then this
        // player's first command (the gate numbers it 0).
        let mut own = command_event(2, 2, &depot_build());
        // Ordered with the seal of the password it went with.
        let seal = Seal {
            scope: 3,
            tag: FixedBytes([5; 32]),
        };
        if let EventBody::Command {
            player,
            client_seq,
            seal: sealed,
            ..
        } = &mut own.body
        {
            *player = ME;
            *client_seq = 0;
            *sealed = Some(seal);
        }
        script.events.extend([
            vec![],
            vec![],
            vec![command_event(1, 2, &depot_build()), own],
        ]);
        let (mut d, _) = driver(script);
        let payload = depot_build().to_payload().unwrap();
        let secret = Secret {
            scope: 3,
            password: tpf3mp_proto::Text::new("pw").unwrap(),
        };
        let control = FakeControl::default();
        let state = control.state.clone();
        d.control = Box::new(control);
        for commands in [vec![(7, payload, Some(secret.clone()))], Vec::new()] {
            d.on_step(commands, &mut |batch| Ok(batch.lanes.then(Vec::new)));
        }
        let state = state.lock().unwrap();
        let (step, actions) = &state.replay_requests[0];
        assert_eq!(*step, 2);
        assert_eq!(
            actions.iter().map(|o| o.ticket).collect::<Vec<_>>(),
            [None, Some(7)]
        );
        assert_eq!(
            actions.iter().map(|o| o.seal).collect::<Vec<_>>(),
            [None, Some(seal)]
        );
        assert_eq!(
            d.gate.secrets,
            [Some(secret)],
            "the password went to the room"
        );
        assert!(d.take_refused().is_empty());
    }

    #[test]
    fn a_command_the_room_refuses_fails_its_ticket() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(None);
        script.begin.push_back(Some(begin()));
        script.gates.extend([StepGate::Run, StepGate::Run]);
        let (mut d, _) = driver(script);
        let payload = depot_build().to_payload().unwrap();
        // Before the room's game: refused at once.
        d.on_step(vec![(3, payload.clone(), None)], &mut |_| Ok(None));
        assert_eq!(
            d.take_refused(),
            [(3, "the game is not following a room's game".to_owned())]
        );
        // Handed over as the gate's command 0, then refused by the room.
        d.on_step(vec![(4, payload, None)], &mut |_| Ok(None));
        d.game.notice(Notice::Refused {
            command: 0,
            reason: tpf3mp_proto::IntentRejection::RateLimited,
        });
        let refused = d.take_refused();
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].0, 4);
        assert!(
            refused[0].1.starts_with("the room refused it"),
            "{refused:?}"
        );
    }

    const PAUSED: Updates = Updates::Exactly(0);

    fn lobby_named(name: &str) -> LobbyView {
        LobbyView {
            name: tpf3mp_proto::Text::new(name).unwrap(),
            ..LobbyView::default()
        }
    }

    #[test]
    fn the_menus_window_talks_to_the_launcher_before_during_and_after_the_rooms_game() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.lobbies.push_back(lobby_named("at the menu"));
        script.begin.push_back(Some(begin()));
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: None,
                next_step: 1,
            }),
            StepGate::Wait,
        ]);
        let (mut d, mut calls) = driver(script);
        // At the menu no step reads the link: the window's exchange does.
        assert_eq!(
            d.lobby(vec![LobbyAction::Start]),
            Some(lobby_named("at the menu"))
        );
        assert_eq!(d.gate.lobby_acts, vec![LobbyAction::Start]);
        assert_eq!(d.gate.lobby_polls, 1);
        assert_eq!(d.lobby(Vec::new()), None, "nothing new");
        // In the room's game the gate reads the link; the window only takes
        // what it kept.
        call(&mut d, &mut calls);
        assert_eq!(d.phase(), &Phase::Running);
        let polls = d.gate.lobby_polls;
        d.gate.lobby_heard = Some(lobby_named("in the game"));
        assert_eq!(
            d.lobby(vec![LobbyAction::Ready { ready: true }]),
            Some(lobby_named("in the game"))
        );
        assert_eq!(
            d.gate.lobby_polls, polls,
            "the gate's link is not read here"
        );
        assert_eq!(d.gate.lobby_acts.len(), 2);
        assert_eq!(d.phase(), &Phase::Running, "nor is the game disturbed");
    }

    #[test]
    fn a_lobby_that_cannot_be_read_is_logged_once_and_holds_nothing() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let script = Script {
            lobby_fails: true,
            ..Script::default()
        };
        let (mut d, _calls) = driver(script);
        assert_eq!(d.lobby(Vec::new()), None);
        assert_eq!(d.lobby(Vec::new()), None);
        let log = d.take_log();
        assert_eq!(log.len(), 1, "{log:?}");
        assert!(log[0].contains("lobby"), "{log:?}");
        assert_eq!(d.phase(), &Phase::BeforeBegin);
    }

    #[test]
    fn a_link_lost_before_any_room_began_leaves_the_game_playing_alone() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let script = Script {
            begin_fails: true,
            ..Script::default()
        };
        let (mut d, mut calls) = driver(script);
        assert_eq!(call(&mut d, &mut calls), Updates::Own);
        assert_eq!(d.phase(), &Phase::Ended);
        assert!(!d.in_room());
        assert!(
            d.take_log().iter().any(|l| l.contains("plays on alone")),
            "the player is told why"
        );
        assert_eq!(
            call(&mut d, &mut calls),
            Updates::Own,
            "and it keeps playing"
        );
    }

    #[test]
    fn before_the_room_begins_the_game_steps_as_it_would() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut d, mut calls) = driver(Script::default());
        assert_eq!(call(&mut d, &mut calls), Updates::Own);
        assert_eq!(d.phase(), &Phase::BeforeBegin);
    }

    #[test]
    fn a_world_up_in_the_lobby_is_told_to_the_agent_once_and_not_in_the_rooms_game() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.extend([None, None, None, Some(begin())]);
        script.gates.push_back(StepGate::Wait);
        let (mut d, state) = driver_with(script);
        let mut calls = Vec::new();
        // No world up yet: nothing told.
        assert_eq!(call(&mut d, &mut calls), Updates::Own);
        assert!(d.gate.worlds_up.is_empty());
        // The mod says a world's GUI started: the next call tells it, once.
        state.lock().unwrap().world_up = Some(1);
        assert_eq!(call(&mut d, &mut calls), Updates::Own);
        assert_eq!(d.gate.worlds_up, vec![1]);
        assert!(d.take_log().iter().any(|l| l.contains("world 1 is up")));
        assert_eq!(call(&mut d, &mut calls), Updates::Own);
        assert_eq!(d.gate.worlds_up, vec![1], "once a world");
        // The room begins; a world that starts in its game is the room's.
        state.lock().unwrap().world_up = Some(2);
        call(&mut d, &mut calls);
        assert_eq!(d.phase(), &Phase::Running);
        call(&mut d, &mut calls);
        assert_eq!(d.gate.worlds_up, vec![1]);
    }

    #[test]
    fn released_steps_run_one_by_one_and_a_withheld_one_holds_the_world() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: None,
                next_step: 1,
            }),
            StepGate::Run,
            StepGate::Run,
            StepGate::Wait,
        ]);
        let (mut d, mut calls) = driver(script);
        assert_eq!(
            call(&mut d, &mut calls),
            Updates::Exactly(2),
            "two released steps ran"
        );
        assert_eq!(d.phase(), &Phase::Running);
        // The room withholds the next step: the game's paused path.
        assert_eq!(call(&mut d, &mut calls), PAUSED);
        assert_eq!(d.gate.loaded, vec![1]);
        assert_eq!(d.gate.ran, 2);
        d.game.notices.push("Speed(Speed(400))".into());
        call(&mut d, &mut calls);
        assert!(
            d.take_log()
                .iter()
                .any(|line| line == "the room says: Speed(Speed(400))"),
            "what the room says is logged"
        );
    }

    #[test]
    fn a_room_far_ahead_is_caught_up_a_bounded_number_of_steps_a_call() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend((0..40).map(|_| StepGate::Run));
        let (mut d, mut calls) = driver(script);
        let full = Updates::Exactly(MAX_STEPS_PER_CALL);
        assert_eq!(call(&mut d, &mut calls), full);
        assert_eq!(call(&mut d, &mut calls), full);
        assert_eq!(
            call(&mut d, &mut calls),
            Updates::Exactly(40 - 2 * MAX_STEPS_PER_CALL)
        );
        assert_eq!(call(&mut d, &mut calls), PAUSED);
        assert_eq!(d.gate.ran, 40);
    }

    #[test]
    fn a_lost_agent_holds_the_world() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend([StepGate::Run, StepGate::Run]);
        script.fail_after = true;
        let (mut d, mut calls) = driver(script);
        assert_eq!(
            call(&mut d, &mut calls),
            Updates::Exactly(2),
            "the steps ran, their report failed"
        );
        assert!(matches!(d.phase(), Phase::Holding(_)));
        assert_eq!(call(&mut d, &mut calls), PAUSED);
    }

    #[test]
    fn the_speed_is_the_rooms_from_the_room_s_game_on_until_it_ends() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(None);
        script.begin.push_back(Some(begin()));
        script
            .gates
            .extend([StepGate::Run, StepGate::Wait, StepGate::Ended]);
        let (mut d, mut calls) = driver(script);
        call(&mut d, &mut calls);
        assert!(!d.in_room(), "before the room begins, the game's own speed");
        call(&mut d, &mut calls);
        assert!(d.in_room());
        call(&mut d, &mut calls);
        assert!(d.in_room(), "withheld");
        call(&mut d, &mut calls);
        assert!(!d.in_room(), "after it ends, the game's own speed again");
        assert_eq!(
            calls,
            vec![Updates::Own, Updates::Exactly(1), PAUSED, Updates::Own]
        );
    }

    #[test]
    fn a_change_in_the_speed_row_asks_the_room_and_joining_asks_nothing() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        let (mut d, mut calls) = driver(script);
        d.chosen_speed(4);
        assert!(
            d.gate.speeds.is_empty(),
            "before the room's game, nothing is asked"
        );
        call(&mut d, &mut calls);
        // Entering the room's game at 2x: taken as it is.
        d.chosen_speed(2);
        d.chosen_speed(2);
        assert!(d.gate.speeds.is_empty());
        d.chosen_speed(4);
        d.chosen_speed(0);
        d.chosen_speed(0);
        assert_eq!(d.gate.speeds, vec![Speed(400), Speed::PAUSED]);
        d.chosen_speed(1000);
        assert_eq!(
            d.gate.speeds.last(),
            Some(&Speed::MAX),
            "capped at the room's fastest"
        );
    }

    #[test]
    fn the_multiplayer_window_hears_the_room_through_the_game_control() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        let heard = vec![
            Notice::Speed(Speed(200)),
            Notice::Diverged {
                step: 50,
                lanes: vec![3],
            },
        ];
        script.notices.push_back(heard.clone());
        let (mut d, state) = driver_with(script);
        let mut calls = Vec::new();
        call(&mut d, &mut calls);
        let state = state.lock().unwrap();
        assert_eq!(state.me, Some(begin().player), "the game's own player");
        assert_eq!(state.room, Some(begin().room), "the stable room identity");
        assert_eq!(state.room_notices, heard, "what the room said, in order");
    }

    /// Runs the room's steps up to `through`, one call at a time, and
    /// returns each checkpoint's lane dump: its step and lanes.
    fn dumps_through(d: &mut StepDriver<Script>, through: u64) -> Vec<(u64, Vec<u16>)> {
        let mut dumps = Vec::new();
        while d.gate.ran < through {
            d.on_step(Vec::new(), &mut |batch| {
                if let Some(dump) = batch.dump {
                    assert!(batch.lanes, "a dump comes with a checkpoint");
                    dumps.push((dump.step, dump.lanes.clone()));
                }
                Ok(batch.lanes.then(Vec::new))
            });
        }
        dumps
    }

    fn room_running(interval: u32, steps: usize) -> Script {
        let mut script = Script::default();
        script.begin.push_back(Some(begin_every(interval)));
        script.gates.push_back(StepGate::Load(Load {
            world_epoch: 1,
            file: None,
            next_step: 1,
        }));
        script
            .gates
            .extend(std::iter::repeat_n(StepGate::Run, steps));
        script
    }

    #[test]
    fn a_divergence_asks_every_game_for_a_lane_dump_and_dumps_here_too() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = room_running(10, 60);
        script.notices.push_back(vec![Notice::Diverged {
            step: 10,
            lanes: vec![3],
        }]);
        let (mut d, _) = driver(script);
        let dumps = dumps_through(&mut d, 60);
        // Told before step 1: the first checkpoint 20 steps on or later.
        assert_eq!(dumps, [(30, vec![3]), (40, vec![3])]);
        let said: Vec<&str> = d.gate.said.iter().map(|t| t.as_str()).collect();
        assert_eq!(said.len(), 1, "{said:?}");
        assert_eq!(
            crate::lanedump::parse(said[0]),
            Some(crate::lanedump::Ask {
                lanes: vec![3],
                steps: vec![30, 40],
                diverged: 10
            }),
            "the room's chat carries the steps and lanes"
        );
        let log = d.take_log().join("\n");
        assert!(
            log.contains("step 10 diverged: dumping lanes 3 at steps 30,40"),
            "{log}"
        );
        assert!(
            log.contains("dumping lanes 3 at the checkpoint after step 30 (step 10 diverged)"),
            "{log}"
        );
    }

    #[test]
    fn a_game_that_did_not_diverge_dumps_what_the_rooms_chat_asks() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = room_running(10, 60);
        let ask = crate::lanedump::Ask {
            lanes: vec![0, 3],
            steps: vec![30, 40],
            diverged: 10,
        };
        script.notices.push_back(vec![Notice::Chat {
            from: tpf3mp_proto::Text::new("bob").unwrap(),
            text: ChatText::new(crate::lanedump::announce(&ask)).unwrap(),
        }]);
        let (mut d, state) = driver_with(script);
        let dumps = dumps_through(&mut d, 60);
        assert_eq!(dumps, [(30, vec![0, 3]), (40, vec![0, 3])]);
        assert!(d.gate.said.is_empty(), "it asks nobody else");
        assert!(
            d.take_log()
                .iter()
                .any(|l| l.contains("bob asks for a lane dump: lanes 0,3 at steps 30,40")),
        );
        assert_eq!(
            state.lock().unwrap().room_notices.len(),
            1,
            "the window still shows the line"
        );
    }

    #[test]
    fn the_environments_lanes_are_dumped_at_every_checkpoint() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut d, _) = driver(room_running(10, 30));
        d.set_lane_dumps(LaneDumps::new(
            crate::lanedump::Setting::from_env(Some("3")).0,
        ));
        assert_eq!(
            dumps_through(&mut d, 30),
            [(10, vec![3]), (20, vec![3]), (30, vec![3])]
        );
        // Off: a divergence asks nothing and dumps nothing.
        let mut script = room_running(10, 60);
        script.notices.push_back(vec![Notice::Diverged {
            step: 10,
            lanes: vec![3],
        }]);
        let (mut d, _) = driver(script);
        d.set_lane_dumps(LaneDumps::new(
            crate::lanedump::Setting::from_env(Some("off")).0,
        ));
        assert!(dumps_through(&mut d, 60).is_empty());
        assert!(d.gate.said.is_empty());
    }

    #[test]
    fn what_the_player_says_reaches_the_room_in_its_game_only() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        let (mut d, mut calls) = driver(script);
        let text = |s: &str| ChatText::new(s).unwrap();
        d.say(text("anyone there?"));
        assert!(
            d.gate.said.is_empty(),
            "before the room's game, nobody hears"
        );
        assert!(
            d.take_log()
                .iter()
                .any(|line| line.contains("outside the room's game")),
            "and the log says so"
        );
        call(&mut d, &mut calls);
        d.say(text("on my way"));
        assert_eq!(d.gate.said, vec![text("on my way")]);
    }

    #[test]
    fn the_players_preview_reaches_the_room_in_its_game_only() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        let (mut d, mut calls) = driver(script);
        let shown = Payload::new(vec![1, 2, 3]).unwrap();
        d.preview(Some(shown.clone()));
        assert!(
            d.gate.previews.is_empty(),
            "before the room's game, nobody sees"
        );
        call(&mut d, &mut calls);
        d.preview(Some(shown.clone()));
        d.preview(None);
        assert_eq!(d.gate.previews, vec![Some(shown), None]);
    }

    #[test]
    fn after_the_room_ends_the_game_steps_on_its_own() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.push_back(StepGate::Ended);
        let (mut d, mut calls) = driver(script);
        assert_eq!(call(&mut d, &mut calls), Updates::Own);
        assert_eq!(d.phase(), &Phase::Ended);
        assert_eq!(call(&mut d, &mut calls), Updates::Own);
    }

    #[test]
    fn leaving_after_a_world_closes_drains_its_session_only_at_the_menu() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend([StepGate::Run, StepGate::Run]);
        let (mut d, mut calls) = driver(script);
        call(&mut d, &mut calls);
        d.lobby(vec![LobbyAction::Leave]);
        assert_eq!(
            d.gate.departure_polls, 0,
            "never discard a live world's events"
        );
        d.on_menu();
        assert_eq!(d.gate.departure_polls, 1);
        assert_eq!(d.phase(), &Phase::Running, "wait for the real End");
        d.gate.departure_done = true;
        d.gate.reset_ended = true;
        d.on_menu();
        assert_eq!(d.phase(), &Phase::BeforeBegin);
        assert_eq!(d.gate.menus_up, vec![1]);
        assert!(!d.menu_departing);
        assert_eq!(d.gate.lobby_acts, vec![LobbyAction::Leave]);
    }

    #[test]
    fn a_batch_interval_that_is_not_armed_leaves_even_steps_measuring() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend((0..4).map(|_| StepGate::Run));
        let (mut d, mut calls) = driver(script);
        d.set_even_steps(true);
        d.set_batch_interval(true);
        call(&mut d, &mut calls);
        // The wrapper never armed (no layout, a fault): no fixed count, and
        // no batch may be timed.
        assert!(!crate::interval::armed());
        assert!(!d.batch_info().eligible);
    }

    #[test]
    fn a_call_with_nothing_released_is_a_wait_for_the_interval() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend([StepGate::Wait, StepGate::Run]);
        let (mut d, mut calls) = driver(script);
        d.set_even_steps(true);
        assert_eq!(call(&mut d, &mut calls), Updates::Exactly(0));
        assert!(d.batch_info().waiting, "nothing released: the room waits");
        call(&mut d, &mut calls);
        assert!(!d.batch_info().waiting);
    }

    #[test]
    fn a_speed_heard_at_the_menu_paces_the_world_loaded_after() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Joining a 4x room from the main menu: its speed comes before the
        // world, and only once.
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.notices.push_back(vec![Notice::Speed(Speed(400))]);
        script.gates.push_back(StepGate::Wait);
        script.gates.extend((0..6).map(|_| StepGate::Run));
        let (mut d, mut calls) = driver(script);
        d.set_even_steps(true);
        d.on_menu();
        assert_eq!(d.phase(), &Phase::Running);
        assert_eq!(call(&mut d, &mut calls), Updates::Exactly(4), "4x, not 1x");
    }

    #[test]
    fn another_room_can_begin_only_after_returning_to_the_menu() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script {
            reset_ended: true,
            ..Script::default()
        };
        script.begin.extend([Some(begin()), Some(begin())]);
        script.gates.extend([StepGate::Ended, StepGate::Wait]);
        let (mut d, mut calls) = driver(script);
        assert_eq!(call(&mut d, &mut calls), Updates::Own);
        assert_eq!(call(&mut d, &mut calls), Updates::Own);
        assert_eq!(d.phase(), &Phase::Ended);
        assert_eq!(
            d.gate.begin.len(),
            1,
            "the previous world cannot enter the next room"
        );
        d.on_menu();
        assert_eq!(d.phase(), &Phase::Running);
        assert!(d.gate.begin.is_empty());
        assert!(
            d.take_log()
                .iter()
                .any(|line| line.contains("ready for another multiplayer room"))
        );
    }

    fn order(event: u64, file: &Path) -> SaveOrder {
        SaveOrder {
            event,
            file: file.to_owned(),
        }
    }

    #[test]
    fn a_save_holds_the_world_until_the_game_saved_and_is_moved_to_the_room() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("tpf3mp-step-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let written = dir.join("written.sav");
        let wanted = dir.join("save-7.sav");
        std::fs::write(&written, b"world").unwrap();

        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend([
            StepGate::Save(order(7, &wanted)),
            StepGate::Run,
            StepGate::Wait,
        ]);
        let (mut d, state) = driver_with(script);
        let mut calls = Vec::new();
        assert_eq!(call(&mut d, &mut calls), PAUSED, "asked the game to save");
        let name = state.lock().unwrap().save_requests.clone();
        assert_eq!(name, vec![format!("tpf3mp_{}_7", std::process::id())]);
        assert_eq!(call(&mut d, &mut calls), PAUSED, "no answer yet: held");
        state.lock().unwrap().save_answer = Some(Ok(written.clone()));
        assert_eq!(
            call(&mut d, &mut calls),
            Updates::Exactly(1),
            "saved and reported: the steps go on"
        );
        assert_eq!(d.gate.saves, vec![Ok(())]);
        assert!(
            !written.exists() && wanted.exists(),
            "moved to the room's file"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_save_the_game_could_not_make_is_reported_failed_and_the_game_goes_on() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend([
            StepGate::Save(order(8, Path::new("never.sav"))),
            StepGate::Run,
        ]);
        let (mut d, state) = driver_with(script);
        let mut calls = Vec::new();
        call(&mut d, &mut calls);
        state.lock().unwrap().save_answer = Some(Err("disk full".into()));
        assert_eq!(call(&mut d, &mut calls), Updates::Exactly(1));
        assert_eq!(d.gate.saves, vec![Err("disk full".into())]);
        assert_eq!(d.phase(), &Phase::Running);
    }

    #[test]
    fn the_rooms_save_is_loaded_and_its_world_plays_once_it_is_up() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        let file = PathBuf::from("worlds/room.sav");
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: Some(file.clone()),
                next_step: 101,
            }),
            StepGate::Run,
            StepGate::Wait,
        ]);
        let (mut d, state) = driver_with(script);
        let mut calls = Vec::new();
        assert_eq!(call(&mut d, &mut calls), PAUSED, "asked to load");
        assert_eq!(
            state.lock().unwrap().load_requests,
            vec![(file, LoadFrom::Gui)]
        );
        assert_eq!(call(&mut d, &mut calls), PAUSED, "still loading");
        assert!(d.gate.loaded.is_empty());
        // The loaded world's GUI started.
        state.lock().unwrap().load_done = true;
        assert_eq!(call(&mut d, &mut calls), Updates::Exactly(1));
        assert_eq!(d.gate.loaded, vec![101]);
        assert!(
            d.take_log()
                .iter()
                .any(|line| line.contains("from its save, from step 101"))
        );
    }

    /// The marks of a game whose closes the hook sees.
    fn mark(closed: u64, started: u64) -> WorldMark {
        WorldMark {
            closed: Some(closed),
            started,
        }
    }

    /// The room's world closed and the player started another of their
    /// own (a new game from the main menu, as in the room of 2026-10-02):
    /// it never runs the room's steps, nor is it saved for the room. Each
    /// game started such a world at another room's step, from its own
    /// state, and the room split.
    #[test]
    fn a_world_the_room_did_not_load_is_held_never_stepped_nor_saved() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: None,
                next_step: 1,
            }),
            StepGate::Run,
            StepGate::Save(order(7, Path::new("room.sav"))),
            StepGate::Run,
        ]);
        let (mut d, state) = driver_with(script);
        state.lock().unwrap().mark = mark(0, 1);
        let mut calls = Vec::new();
        assert_eq!(call(&mut d, &mut calls), Updates::Exactly(1));
        assert_eq!(d.gate.ran, 1);
        // Back at the main menu: the room's world closed. The menu says why
        // a world started now is held, once.
        state.lock().unwrap().mark = mark(1, 1);
        d.on_menu();
        d.on_menu();
        let log = d.take_log();
        assert_eq!(
            log.iter()
                .filter(|l| l.contains("the room's world closed in this game"))
                .count(),
            1,
            "said once: {log:?}"
        );
        // A new game generated from the menu: its GUI started.
        state.lock().unwrap().mark = mark(1, 2);
        assert_eq!(call(&mut d, &mut calls), PAUSED);
        assert!(matches!(d.phase(), Phase::Holding(why) if why.contains("did not load")));
        assert_eq!(d.gate.ran, 1, "no room's step ran in the new world");
        assert!(
            state.lock().unwrap().save_requests.is_empty(),
            "the new world is never saved as the room's"
        );
        // For good, like any world the room cannot follow.
        assert_eq!(call(&mut d, &mut calls), PAUSED);
        assert_eq!(d.gate.ran, 1);
    }

    /// The room's own loads replace its world without a hold: one through
    /// the world's GUI (its close is the hook's, never counted), and one
    /// from the main menu after the player left the room's world, which
    /// makes the room's world the loaded one again.
    #[test]
    fn a_load_the_room_orders_replaces_its_world_and_plays_on() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        let file = PathBuf::from("worlds/room.sav");
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: None,
                next_step: 1,
            }),
            StepGate::Run,
            StepGate::Load(Load {
                world_epoch: 1,
                file: Some(file.clone()),
                next_step: 2,
            }),
            StepGate::Run,
            StepGate::Wait,
            StepGate::Load(Load {
                world_epoch: 1,
                file: Some(file.clone()),
                next_step: 3,
            }),
            StepGate::Run,
        ]);
        let (mut d, state) = driver_with(script);
        state.lock().unwrap().mark = mark(0, 1);
        let mut calls = Vec::new();
        assert_eq!(call(&mut d, &mut calls), Updates::Exactly(1));
        // The room's load through the GUI: another world's GUI starts, no
        // close is counted.
        assert_eq!(call(&mut d, &mut calls), PAUSED, "asked to load");
        state.lock().unwrap().mark = mark(0, 2);
        state.lock().unwrap().load_done = true;
        assert_eq!(call(&mut d, &mut calls), Updates::Exactly(1));
        assert_eq!(d.gate.loaded, vec![1, 2]);
        assert_eq!(call(&mut d, &mut calls), PAUSED, "the room waits");
        // The player went back to the main menu; the room orders its world
        // loaded there (a rejoin, a rebase).
        state.lock().unwrap().mark = mark(1, 2);
        d.on_menu();
        assert_eq!(
            state.lock().unwrap().load_requests.last(),
            Some(&(file, LoadFrom::Menu))
        );
        state.lock().unwrap().mark = mark(1, 3);
        state.lock().unwrap().load_done = true;
        assert_eq!(call(&mut d, &mut calls), Updates::Exactly(1));
        assert_eq!(d.gate.loaded, vec![1, 2, 3]);
        assert_eq!(d.phase(), &Phase::Running);
        assert_eq!(d.gate.ran, 3);
    }

    /// The step trace says why each call ran what it did, a room's load
    /// (a rejoin, a rebase) and a held world included.
    #[test]
    fn every_call_says_why_it_runs_the_updates_it_does() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        let file = PathBuf::from("worlds/room.sav");
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: None,
                next_step: 1,
            }),
            StepGate::Run,
            StepGate::Wait,
            StepGate::Load(Load {
                world_epoch: 1,
                file: Some(file),
                next_step: 2,
            }),
        ]);
        let (mut d, state) = driver_with(script);
        state.lock().unwrap().mark = mark(0, 1);
        let mut calls = Vec::new();
        assert_eq!(call(&mut d, &mut calls), Updates::Exactly(1));
        assert_eq!(d.why(), "run");
        assert_eq!(call(&mut d, &mut calls), PAUSED);
        assert_eq!(d.why(), "wait");
        assert_eq!(call(&mut d, &mut calls), PAUSED, "asked to load");
        assert_eq!(d.why(), "load");
        assert_eq!(call(&mut d, &mut calls), PAUSED, "still loading");
        assert_eq!(d.why(), "load");
        d.hold("a test's reason".into());
        assert_eq!(call(&mut d, &mut calls), PAUSED);
        assert_eq!(d.why(), "hold");
    }

    /// A call that ends holding the world says `hold`, not what it was
    /// doing when it held: here a room's load the game could not start.
    #[test]
    fn a_call_that_holds_the_world_says_hold() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: None,
                next_step: 1,
            }),
            StepGate::Run,
            StepGate::Load(Load {
                world_epoch: 1,
                file: Some(PathBuf::from("worlds/room.sav")),
                next_step: 2,
            }),
        ]);
        let (mut d, state) = driver_with(script);
        state.lock().unwrap().mark = mark(0, 1);
        let mut calls = Vec::new();
        assert_eq!(call(&mut d, &mut calls), Updates::Exactly(1));
        assert_eq!(call(&mut d, &mut calls), PAUSED, "asked to load");
        assert_eq!(d.why(), "load");
        // The game says the load failed: this call, a load's, holds.
        state.lock().unwrap().load_failure = Some("this Lua state has no app".into());
        assert_eq!(call(&mut d, &mut calls), PAUSED);
        assert!(matches!(d.phase(), Phase::Holding(_)), "{:?}", d.phase());
        assert_eq!(d.why(), "hold");
    }

    /// Where the hook cannot see a world close (no `CMenuUI::m_game` in the
    /// profile), another world's GUI starting tells a world the room did not
    /// load.
    #[test]
    fn without_closes_another_worlds_gui_tells_a_world_the_room_did_not_load() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let then = WorldMark {
            closed: None,
            started: 1,
        };
        assert!(!then.replaced_since(&then));
        assert!(
            WorldMark {
                closed: None,
                started: 2
            }
            .replaced_since(&then)
        );
        // With closes seen, only a close counts: the GUI of the room's own
        // load starts a world too.
        assert!(!mark(0, 2).replaced_since(&mark(0, 1)));
        assert!(mark(1, 1).replaced_since(&mark(0, 1)));
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: None,
                next_step: 1,
            }),
            StepGate::Run,
            StepGate::Wait,
            StepGate::Run,
        ]);
        let (mut d, state) = driver_with(script);
        state.lock().unwrap().mark = then;
        let mut calls = Vec::new();
        assert_eq!(call(&mut d, &mut calls), Updates::Exactly(1));
        state.lock().unwrap().mark.started = 2;
        assert_eq!(call(&mut d, &mut calls), PAUSED);
        assert!(matches!(d.phase(), Phase::Holding(_)));
    }

    /// The driver knows the room's step the next update runs, from the
    /// loaded world on: what the per-update reseed numbers updates by.
    #[test]
    fn the_driver_counts_the_rooms_steps_from_the_loaded_world() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(None);
        script.begin.push_back(Some(begin()));
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: None,
                next_step: 1,
            }),
            StepGate::Run,
            StepGate::Run,
            StepGate::Wait,
        ]);
        let (mut d, mut calls) = driver(script);
        assert_eq!(call(&mut d, &mut calls), Updates::Own);
        assert_eq!(d.next_step(), None, "no world of the room's yet");
        assert_eq!(call(&mut d, &mut calls), Updates::Exactly(2));
        assert_eq!(d.next_step(), Some(3), "loaded at 1, two steps ran");
        assert_eq!(call(&mut d, &mut calls), PAUSED);
        assert_eq!(d.next_step(), Some(3), "a paused call runs no step");
    }

    #[test]
    fn at_the_menu_the_agent_is_told_once_per_arrival_until_the_room_begins() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.extend([None, None, None, None, Some(begin())]);
        script.gates.push_back(StepGate::Wait);
        let (mut d, mut calls) = driver(script);
        d.on_menu();
        d.on_menu();
        assert_eq!(d.gate.menus_up, vec![1], "one arrival, told once");
        assert!(
            d.take_log()
                .iter()
                .any(|l| l.contains("at its main menu (arrival 1)"))
        );
        // A world comes up and goes: the game is back at its menu.
        call(&mut d, &mut calls);
        d.on_menu();
        assert_eq!(d.gate.menus_up, vec![1, 2]);
        assert_eq!(calls.len(), 1, "the menu runs no step");
        // The room begins while the game is at its menu.
        d.on_menu();
        assert_eq!(d.phase(), &Phase::Running);
        assert_eq!(d.gate.menus_up, vec![1, 2], "nothing told once it began");
    }

    #[test]
    fn the_rooms_save_is_loaded_from_the_menu_and_played_once_its_world_runs() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        let file = PathBuf::from("worlds/room.sav");
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: Some(file.clone()),
                next_step: 41,
            }),
            StepGate::Run,
            StepGate::Wait,
        ]);
        let (mut d, state) = driver_with(script);
        d.on_menu();
        assert_eq!(d.phase(), &Phase::Running);
        assert_eq!(state.lock().unwrap().me, Some(ME), "the room's player");
        assert_eq!(
            state.lock().unwrap().load_requests,
            vec![(file.clone(), LoadFrom::Menu)],
            "no world up: the menu loads it"
        );
        d.on_menu();
        assert_eq!(
            state.lock().unwrap().load_requests.len(),
            1,
            "asked once, then waited for"
        );
        assert!(d.gate.loaded.is_empty());
        // The room's world is up: its step takes it and plays on.
        state.lock().unwrap().load_done = true;
        let mut calls = Vec::new();
        assert_eq!(call(&mut d, &mut calls), Updates::Exactly(1));
        assert_eq!(d.gate.loaded, vec![41]);
        assert_eq!(state.lock().unwrap().load_requests.len(), 1);
        assert!(
            d.take_log()
                .iter()
                .any(|l| l.contains("from the game's main menu to run step 41 next"))
        );
    }

    #[test]
    fn a_load_the_menu_could_not_start_holds_the_world() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.push_back(StepGate::Load(Load {
            world_epoch: 1,
            file: Some(PathBuf::from("room.sav")),
            next_step: 1,
        }));
        let (mut d, state) = driver_with(script);
        d.on_menu();
        state.lock().unwrap().load_failure = Some("this Lua state has no app".into());
        d.on_menu();
        assert!(
            matches!(d.phase(), Phase::Holding(reason) if reason.contains("no app")),
            "{:?}",
            d.phase()
        );
    }

    #[test]
    fn the_menu_leaves_the_owners_world_and_a_save_to_a_world_up() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: None,
                next_step: 1,
            }),
            StepGate::Run,
        ]);
        let (mut d, state) = driver_with(script);
        d.on_menu();
        d.on_menu();
        assert!(state.lock().unwrap().load_requests.is_empty());
        assert!(d.gate.loaded.is_empty(), "no world to take at the menu");
        let log = d.take_log();
        assert_eq!(
            log.iter()
                .filter(|l| l.contains("the main menu cannot pick"))
                .count(),
            1,
            "said once: {log:?}"
        );
        // The player loads their world: its step takes it.
        let mut calls = Vec::new();
        assert_eq!(call(&mut d, &mut calls), Updates::Exactly(1));
        assert_eq!(d.gate.loaded, vec![1]);
    }
}

/// Even steps ([`crate::cadence`]) over the real session and link: what the
/// script double cannot show, the releases read ahead up to an event, and
/// the gate's own checkpoint cut.
#[cfg(test)]
mod even_steps_over_a_real_link {
    use std::time::Duration;

    use crate::lua::tests::SERIAL;

    use tpf3mp_bridge::{BRIDGE_VERSION, ToHook, encode};
    use tpf3mp_ipc::{Config, Link, Role};
    use tpf3mp_proto::{Event, EventBody, FixedBytes, PlayerId, RulesName, Speed, Text};

    use super::tests::{FakeControl, ME};
    use super::*;

    fn say(agent: &Link, message: &ToHook) {
        agent.send(&encode(message).unwrap()).unwrap();
    }

    /// A driver with even steps on, over a real link, in a room at `speed`
    /// whose world runs `next_step` next.
    fn playing(
        tag: &str,
        checkpoint_interval: u32,
        next_step: u64,
        speed: u16,
    ) -> (StepDriver<Session>, Link) {
        let name = format!("test.even.{tag}.{}", std::process::id());
        let agent = Link::create(&Config::new(name.clone()), Role::Agent).unwrap();
        say(
            &agent,
            &ToHook::Hello {
                version: BRIDGE_VERSION,
            },
        );
        let session = Session::attach(&name, "test", Duration::from_secs(10)).unwrap();
        let mut driver = StepDriver::new(session, Box::new(FakeControl::default()));
        driver.set_even_steps(true);
        say(
            &agent,
            &ToHook::Begin {
                room: tpf3mp_proto::RoomId(tpf3mp_proto::FixedBytes([1; 16])),
                rules: RulesName::new("native").unwrap(),
                steps_per_second: 5,
                checkpoint_interval,
                saves: Text::lossy("saves"),
                player: ME,
                mods: None,
            },
        );
        say(&agent, &ToHook::Speed(Speed(speed)));
        say(
            &agent,
            &ToHook::Load {
                world_epoch: 1,
                file: None,
                next_step,
            },
        );
        assert_eq!(
            step(&mut driver),
            (Updates::Exactly(0), false),
            "the world, loaded"
        );
        assert_eq!(driver.next_step(), Some(next_step));
        (driver, agent)
    }

    /// One call of the game's step: its updates, and whether it read lanes.
    fn step(driver: &mut StepDriver<Session>) -> (Updates, bool) {
        let mut ran = None;
        driver.on_step(Vec::new(), &mut |batch| {
            ran = Some((batch.updates, batch.lanes));
            Ok(batch.lanes.then(Vec::new))
        });
        ran.expect("the game's step runs once a call")
    }

    fn release(agent: &Link, from: u64, through: u64) {
        for step in from..=through {
            say(agent, &ToHook::Release { through: step });
        }
    }

    fn left(step: u64) -> Event {
        Event {
            seq: step,
            step,
            body: EventBody::PlayerLeft {
                player: PlayerId(FixedBytes([1; 32])),
                kicked: false,
            },
        }
    }

    #[test]
    fn steps_released_before_an_event_all_run_at_once_and_none_after_it() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut driver, agent) = playing("barrier", 50, 1, 400);
        release(&agent, 1, 5);
        say(&agent, &ToHook::Apply(left(6)));
        say(&agent, &ToHook::Release { through: 6 });
        // Four a call at 4x, but an event waits: all five, so it applies
        // on the next call, as without even steps.
        assert_eq!(step(&mut driver).0, Updates::Exactly(5));
        assert_eq!(
            driver.cadence_note().unwrap(),
            "avail=5 cap=5 nom=4 pick=barrier"
        );
        assert_eq!(
            step(&mut driver).0,
            Updates::Exactly(1),
            "step 6, after its event"
        );
        assert_eq!(driver.game().events, 1);
    }

    #[test]
    fn a_short_backlog_is_kept_in_reserve_and_the_checkpoint_still_ends_a_batch() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut driver, agent) = playing("reserve", 5, 1, 100);
        release(&agent, 1, 1);
        // At 1x one a call; the only step released is kept back once, so a
        // step released a little late later does not stop the world.
        assert_eq!(step(&mut driver).0, Updates::Exactly(0));
        assert_eq!(driver.why(), "reserve");
        release(&agent, 2, 2);
        assert_eq!(step(&mut driver).0, Updates::Exactly(1));
        let mut lanes_at = Vec::new();
        for through in 3..=12 {
            release(&agent, through, through);
            let (updates, lanes) = step(&mut driver);
            assert_eq!(updates, Updates::Exactly(1), "released through {through}");
            if lanes {
                lanes_at.push(driver.next_step().unwrap() - 1);
            }
        }
        assert_eq!(
            lanes_at,
            vec![5, 10],
            "each checkpoint the last update of its call"
        );
    }

    #[test]
    fn a_world_loaded_far_along_counts_its_releases_from_there() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut driver, agent) = playing("far-along", 50, 100_001, 400);
        release(&agent, 100_001, 100_006);
        assert_eq!(step(&mut driver).0, Updates::Exactly(4));
        assert_eq!(
            driver.cadence_note().unwrap(),
            "avail=6 cap=6 nom=4 pick=nominal"
        );
    }

    #[test]
    fn far_behind_it_catches_up_sixteen_a_call() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut driver, agent) = playing("far", 50, 1, 100);
        release(&agent, 1, 40);
        assert_eq!(step(&mut driver).0, Updates::Exactly(16));
        assert_eq!(
            driver.cadence_note().unwrap(),
            "avail=40 cap=16 nom=1 pick=far"
        );
    }

    #[test]
    fn a_paused_room_runs_on_to_its_frontier() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut driver, agent) = playing("paused", 50, 1, 100);
        release(&agent, 1, 3);
        say(&agent, &ToHook::Speed(Speed::PAUSED));
        assert_eq!(step(&mut driver).0, Updates::Exactly(3));
        assert_eq!(
            driver.cadence_note().unwrap(),
            "avail=3 cap=3 nom=1 pick=paused"
        );
    }

    #[test]
    fn off_it_runs_every_step_released() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut driver, agent) = playing("off", 50, 1, 400);
        driver.set_even_steps(false);
        release(&agent, 1, 9);
        assert_eq!(step(&mut driver).0, Updates::Exactly(9));
    }
}
