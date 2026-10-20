//! The launcher: where the player connects, creates or joins a room, gets
//! ready, chats and plays, with this agent doing the work. It is the
//! launcher backend of `docs/ARCHITECTURE.md`. Front ends read its
//! [`State`] and send it [`Action`]s: the native window of the
//! `tpf3mp-launcher` crate through a [`LauncherHandle`], a page in the
//! player's browser (`Launcher::start`), or the Multiplayer window on the
//! game's main menu (D17), over the link to the game's hook ([`lobby`]).
//!
//! The page is served on the loopback interface only. Every API request
//! carries a secret token that only the launched page knows, and requests
//! for any host but the loopback address are refused, so neither other web
//! pages nor DNS rebinding can drive it.

mod api;
mod http;
pub mod instance;
pub(crate) mod lobby;
pub mod servers;
pub mod setup;

use std::{
    collections::HashMap,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use tokio::{
    net::TcpListener,
    sync::{mpsc, oneshot, watch},
    task::{JoinHandle, JoinSet},
    time::MissedTickBehavior,
};
use tpf3mp_bridge::{LobbyAction, LobbyView};
use tpf3mp_net::{Identity, ServerTrust};
use tpf3mp_proto::{
    ContentDiff, ContentManifest, CreateRoom, Invite, JoinRoom, Request, RequestError, Response,
    RoomPage, RoomPhase, RoomSettings, StartSave, Text,
};
use tracing::{info, warn};

pub use self::api::{
    Action, ChatLine, Connection, Differences, Game, InstalledGame, Member, MemberContent,
    ModClass, ModHave, ModRow, Phase, PublicRoom, Room, RoomList, RoomModRow, RoomNativeRow,
    RoomStart, RulesChoice, ServerRow, StartProgress, State, World,
};
pub use self::servers::ListedServer;
use self::{api::View, http::Page, lobby::IdleLink};
use crate::native_mods::Change;
use crate::{
    Client, ClientError, ClientEvent, ConnectOptions, Events, TunnelChoice, Worlds,
    bridge::{
        self, Bridge, BridgeEnd, BridgeOptions, Control, LobbyLink, Rejoin, SharedStatus, Status,
    },
    connect,
    picker::Declaration,
};

/// How long the launcher keeps trying to rejoin a room after losing the
/// server: as long as a server holds a game nobody is connected to
/// (`--abandon-after-mins`, 5 by default).
const REJOIN_PATIENCE: Duration = Duration::from_secs(300);
/// How long disconnecting waits for the room session to leave before it
/// stops it.
const DISCONNECT_WAIT: Duration = Duration::from_secs(5);
/// Actions queued from the page before it waits.
const ACTION_QUEUE: usize = 32;
/// How often the launcher looks whether the game it started still runs.
const GAME_POLL: Duration = Duration::from_millis(500);
/// How often the launcher's lobby is worked out for the game's window and
/// the game's link served while no room session holds it.
const LOBBY_TICK: Duration = Duration::from_millis(100);
/// How often the player's saves are looked at again, for the game's window
/// to offer as a room's start save.
const SAVES_TICK: Duration = Duration::from_secs(5);
/// How long the launcher watches a game link it finds already made for
/// another launcher's heartbeat, before taking it.
const LINK_HELD_WAIT: Duration = Duration::from_millis(350);
/// How long each trusted region has to answer a non-mutating invite lookup.
const INVITE_RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the hook of a game this launcher did not start may fall silent
/// before the game counts as closed, once its process is gone too: a game
/// that followed the link from a launcher that closed ([`instance`]), which
/// this one did not start and cannot wait on. Long enough for a save to
/// load at the menu.
const ADOPTED_GAME_QUIET: Duration = Duration::from_secs(60);

/// What a launcher needs.
#[derive(Debug, Clone)]
pub struct LauncherConfig {
    /// Where the page is served: a loopback address.
    pub listen: SocketAddr,
    /// Which tunnel connections take when UDP does not get through.
    pub tunnel: TunnelChoice,
    /// Where the server and name of each connection are remembered for the
    /// next run (see [`Remembered`]).
    pub remember: Option<PathBuf>,
    /// The server the launcher plays on, as `host:port`: the one on its
    /// command line, else the player's setting, else
    /// [`Self::default_server`].
    pub server: Option<String>,
    /// Whether [`Self::server`] is the server this launcher plays on (D12,
    /// as amended): an invite then never takes the player to another, and
    /// only the player's server setting ([`Action::SetServer`]) changes it.
    /// Without, as in a test or a build with no server at all, the player
    /// types one and an invite may name its own.
    pub server_fixed: bool,
    /// The launcher's default server, `host:port`: the one its package was
    /// built for, or the project's relay. The server setting's "Reset to
    /// default" goes back to it.
    pub default_server: Option<String>,
    /// What players see of that server, such as `EU`, in place of its
    /// address.
    pub server_name: Option<String>,
    /// The servers the release vouches for, the default first
    /// ([`servers::release_list`]). With two or more, a launcher playing
    /// on its default plays on all of them: it lists every one's rooms and
    /// creates rooms on the closest (D12's approved regional amendment).
    /// Empty or one: the one server alone, as before; so with
    /// `--server`.
    pub servers: Vec<ListedServer>,
    /// How to trust servers.
    pub trust: ServerTrust,
    pub identity: Arc<Identity>,
    /// The name the page offers first.
    pub name: String,
    /// What this player's game runs, declared on every connection: the
    /// build and the shared mods (`crate::content::split`).
    pub content: ContentManifest,
    /// This player's mods for the room's worlds, shared and personal, when
    /// it listed them (`BridgeOptions::mods`).
    pub mods: Option<tpf3mp_bridge::ModLists>,
    /// The player's installed mods and those they chose, when the launcher
    /// found them itself (no `--mods`): `content` and `mods` then follow
    /// them and the room (`crate::picker`; docs/MODS.md).
    pub picker: Option<crate::picker::Mods>,
    /// Where native mods come from: the project's signed index, when this
    /// launcher was built with its key (docs/NATIVE_MODS.md). Without, it
    /// offers none.
    pub native: Option<crate::native_mods::NativeSetup>,
    /// Transport Fever 3 as Steam installed it, if it did.
    pub installed: Option<crate::steam::Installed>,
    /// The shared-memory link the game's hook opens.
    pub link: String,
    pub worlds: Worlds,
    /// The settings of rooms this player creates.
    pub room_settings: RoomSettings,
    /// Where lines of the player's log wait to go to the server, when the
    /// launcher sends them; `LauncherHandle` switches it.
    pub diagnostics: Option<crate::diagnostics::Recorder>,
    /// Where the hook's and the game's logs are, whose lines go with
    /// `diagnostics` (approved D10 amendment); `None` sends none of them.
    /// A `TPF3MP_DATA_DIR` in `game_env` moves the hook's log there.
    pub game_logs: Option<crate::game_logs::Places>,
    /// The hook library the game is started with: in the package, next to
    /// the launcher. `None` when the package has none.
    pub hook: Option<PathBuf>,
    /// The game's executable, when it is not where Steam's folder says.
    pub game_exe: Option<PathBuf>,
    /// More variables for the game's environment, on top of the link's: for
    /// playtests, such as the save the hook loads at the main menu
    /// ([`tpf3mp_ipc::AUTO_LOAD_ENV`]).
    pub game_env: Vec<(String, String)>,
    /// A save the rooms this player creates start from: handed to the room
    /// in its lobby, and loaded by every game from its main menu when the
    /// game starts, this player's too (see `BridgeOptions::start_world`).
    /// Without it, the owner's game loads the world and saves it for the
    /// room once the game began.
    pub start_save: Option<PathBuf>,
}

/// A running launcher.
pub struct Launcher {
    /// The page's address, when it serves one.
    url: Option<String>,
    shared: Arc<Shared>,
    /// The launcher controller.
    task: JoinHandle<()>,
    /// The page server, when this launcher serves a page.
    page_task: Option<JoinHandle<()>>,
}

impl Launcher {
    /// Starts serving the page and returns once it is reachable.
    pub async fn start(config: LauncherConfig) -> std::io::Result<Self> {
        if !config.listen.ip().is_loopback() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the launcher serves the loopback interface only",
            ));
        }
        let listener = TcpListener::bind(config.listen).await?;
        let address = listener.local_addr()?;
        let page = Arc::new(Page {
            token: random_token(),
            address,
        });
        let url = format!("http://{address}/#{}", page.token);
        let (shared, actions, lobby) = Shared::new(&config);
        let control = tokio::spawn(control(Arc::clone(&shared), config, actions, lobby));
        let serve = tokio::spawn(http::serve(listener, Arc::clone(&shared), page));
        Ok(Self {
            url: Some(url),
            shared,
            task: control,
            page_task: Some(serve),
        })
    }

    /// Starts the launcher without a page, for a front end in this process
    /// that drives it through [`Launcher::handle`]. Call it within a Tokio
    /// runtime.
    pub fn start_local(config: LauncherConfig) -> Self {
        let (shared, actions, lobby) = Shared::new(&config);
        let task = tokio::spawn(control(Arc::clone(&shared), config, actions, lobby));
        Self {
            url: None,
            shared,
            task,
            page_task: None,
        }
    }

    /// Starts serving the browser page from this existing local launcher.
    /// The page and any native frontend continue to use the same controller,
    /// room session and game link. It can be attached once; a browser-started
    /// launcher already has a page.
    pub async fn serve_local(&mut self, listen: SocketAddr) -> std::io::Result<()> {
        if self.url.is_some() || self.page_task.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "the launcher already serves a page",
            ));
        }
        if !listen.ip().is_loopback() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the launcher serves the loopback interface only",
            ));
        }
        let listener = TcpListener::bind(listen).await?;
        let address = listener.local_addr()?;
        let page = Arc::new(Page {
            token: random_token(),
            address,
        });
        self.url = Some(format!("http://{address}/#{}", page.token));
        self.page_task = Some(tokio::spawn(http::serve(
            listener,
            Arc::clone(&self.shared),
            page,
        )));
        Ok(())
    }

    /// The page's address, with the token it needs, if it serves one.
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// Reads the launcher's state and sends it actions.
    pub fn handle(&self) -> LauncherHandle {
        LauncherHandle {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Runs until the controller or page server ends.
    pub async fn wait(mut self) {
        if let Some(page_task) = self.page_task.as_mut() {
            tokio::select! {
                _ = &mut self.task => {}
                _ = page_task => {}
            }
        } else {
            let _ = (&mut self.task).await;
        }
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        self.task.abort();
        if let Some(page_task) = self.page_task.take() {
            page_task.abort();
        }
    }
}

/// A front end's way into a running launcher.
#[derive(Clone)]
pub struct LauncherHandle {
    shared: Arc<Shared>,
}

impl LauncherHandle {
    /// Carries out `action` after those sent before it. A refusal is also
    /// kept in the state's `error` until an action succeeds.
    pub async fn act(&self, action: Action) -> Result<(), String> {
        let (reply, answer) = oneshot::channel();
        self.shared
            .actions
            .send((action, reply))
            .await
            .map_err(|_| "the launcher stopped".to_owned())?;
        answer
            .await
            .unwrap_or_else(|_| Err("the launcher stopped".to_owned()))
    }

    /// What the launcher shows now.
    pub fn state(&self) -> State {
        // A room session's bridge may have learned the room's mods.
        self.shared.show_mods();
        self.shared.snapshot()
    }
}

type Actions = mpsc::Receiver<(Action, oneshot::Sender<Result<(), String>>)>;

/// The controller's ends of the lobby the game's window shows (D17): the
/// lobby it works out, and the actions the window sends.
struct LobbyEnds {
    views: watch::Sender<LobbyView>,
    actions: mpsc::UnboundedReceiver<LobbyAction>,
}

/// The server-local pages collected for the current public-room browse.
/// Keeping earlier pages avoids repeating rate-limited lookups while the
/// player moves through the globally merged list.
#[derive(Default)]
struct RoomPageCache {
    home: Option<String>,
    pages: HashMap<(String, u16), RoomPage>,
}

/// What the front ends and the controller share.
pub(crate) struct Shared {
    view: Mutex<View>,
    status: SharedStatus,
    actions: mpsc::Sender<(Action, oneshot::Sender<Result<(), String>>)>,
    /// The bridges' ends of the game window's lobby.
    lobby: LobbyLink,
    /// The player's mods, when the launcher found them itself.
    picker: Option<Arc<Mutex<crate::picker::Mods>>>,
    /// The quiet connections to the other listed servers, while the
    /// launcher plays on its release's servers.
    lookouts: servers::Lookouts,
    room_pages: Mutex<RoomPageCache>,
    /// The room last joined, with its password: joined again once the game
    /// started anew with the room's native mods.
    last_join: Mutex<Option<(Invite, Option<Text<64>>)>>,
}

/// Whether a launcher set up as `config` starts on its release's servers:
/// it has two or more, and plays on its default, not on a server of the
/// player's own.
fn starts_on_list(config: &LauncherConfig) -> bool {
    config.servers.len() >= 2
        && config.server_fixed
        && config
            .server
            .as_deref()
            .is_some_and(|server| same_server(server, &config.servers[0].address))
}

impl Shared {
    fn new(config: &LauncherConfig) -> (Arc<Self>, Actions, LobbyEnds) {
        let (actions, receiver) = mpsc::channel(ACTION_QUEUE);
        let (views, views_rx) = watch::channel(LobbyView::default());
        let (lobby_actions, lobby_rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Self {
            view: Mutex::new(View {
                server: config.server.clone(),
                server_fixed: config.server_fixed,
                server_default: config.default_server.clone(),
                server_name: config.server_name.clone(),
                name: config.name.clone(),
                banner: config
                    .remember
                    .as_deref()
                    .and_then(|file| Remembered::load(file).banner),
                player: Some(config.identity.player()),
                installed: config.installed.clone(),
                diagnostics: config.diagnostics.as_ref().map(|recorder| recorder.is_on()),
                log_session: config
                    .diagnostics
                    .as_ref()
                    .map(|recorder| recorder.run().to_string()),
                start_save: config.start_save.as_deref().and_then(save_name),
                listed: config.servers.clone(),
                on_list: starts_on_list(config),
                ..View::default()
            }),
            status: SharedStatus::default(),
            actions,
            lobby: LobbyLink {
                views: views_rx,
                actions: lobby_actions,
            },
            picker: config.picker.clone().map(|mods| Arc::new(Mutex::new(mods))),
            lookouts: servers::Lookouts::default(),
            room_pages: Mutex::new(RoomPageCache::default()),
            last_join: Mutex::new(None),
        });
        shared.show_mods();
        let lobby = LobbyEnds {
            views,
            actions: lobby_rx,
        };
        (shared, receiver, lobby)
    }

    fn view(&self) -> std::sync::MutexGuard<'_, View> {
        self.view.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What the front ends show now, the listed servers' pings included.
    fn snapshot(&self) -> State {
        let mut state = api::snapshot(&self.view(), &self.status());
        let (on_list, listed, home, ping) = {
            let view = self.view();
            (
                view.on_list,
                view.listed.clone(),
                // Played on while connected or in a room.
                view.server
                    .clone()
                    .filter(|_| view.connected || view.in_room),
                view.home_ping,
            )
        };
        // Keep the trusted region names available to the launcher's and
        // game's explicit invite selectors even before connecting. A row
        // without a live connection is marked unreachable.
        if on_list {
            state.servers = self.lookouts.rows(&listed, home.as_deref(), ping);
        }
        state
    }

    /// Whether the launcher plays on its release's servers now.
    fn on_list(&self) -> bool {
        self.view().on_list
    }

    /// Watches the listed servers but `home`, where the launcher now plays,
    /// as `name`; while on its release's servers only.
    async fn watch_others(&self, config: &LauncherConfig, home: &str, name: &Text<32>) {
        if self.on_list() {
            self.lookouts
                .watch(&config.servers, Some(home), &quiet_as(config, name));
        } else {
            self.lookouts.stop().await;
        }
    }

    fn status(&self) -> std::sync::MutexGuard<'_, Status> {
        self.status.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The picker's mods, locked, if the launcher found them itself.
    fn picker(&self) -> Option<std::sync::MutexGuard<'_, crate::picker::Mods>> {
        self.picker
            .as_ref()
            .map(|mods| mods.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// What this player declares to the room now: as the room's owner with
    /// the picker, their content and the room's mods. A room's list that
    /// does not hold together never stays the picker's ([`own_start`],
    /// `choose_room`), so the content alone is only a fallback.
    fn content(&self, config: &LauncherConfig) -> Declaration {
        match self.picker() {
            Some(mods) => mods
                .declaration()
                .unwrap_or_else(|_| Declaration::Content(mods.manifest())),
            None => Declaration::Content(
                config.content.clone().with_native(
                    self.view()
                        .native_terms
                        .iter()
                        .map(tpf3mp_proto::NativeMod::term)
                        .collect(),
                ),
            ),
        }
    }

    /// The native mods that change the simulation which this player's game
    /// runs, or the next one will: declared beside their mods. Returns
    /// whether they changed.
    fn set_native(&self, terms: Vec<tpf3mp_proto::NativeMod>) -> bool {
        let changed = {
            let mut view = self.view();
            let changed = view.native_terms != terms;
            view.native_terms.clone_from(&terms);
            changed
        };
        if let Some(mut mods) = self.picker() {
            mods.set_native(terms);
        }
        changed
    }

    /// Puts the picker's mods in the view.
    fn show_mods(&self) {
        let rows = self.picker().map(|mods| {
            let params = mods
                .room_params()
                .iter()
                .flat_map(|of| {
                    of.params.iter().map(|param| api::ParamRow {
                        id: of.id.as_str().to_owned(),
                        key: param.key.as_str().to_owned(),
                        value: param.value,
                    })
                })
                .collect();
            (api::mod_rows(&mods), params)
        });
        if let Some(((mods, room_mods), room_params)) = rows {
            let mut view = self.view();
            view.mods = mods;
            view.room_mods = room_mods;
            view.room_params = room_params;
        }
    }

    /// The picker as a room session asks it.
    fn picker_link(self: &Arc<Self>) -> Option<bridge::PickerLink> {
        let lists = Arc::clone(self.picker.as_ref()?);
        let learning = Arc::clone(&lists);
        let shared = Arc::clone(self);
        Some(bridge::PickerLink {
            lists: Arc::new(move || lists.lock().unwrap_or_else(PoisonError::into_inner).lists()),
            learn: Arc::new(move |diff| {
                let mut mods = learning.lock().unwrap_or_else(PoisonError::into_inner);
                mods.learn(diff)
                    .then(|| Declaration::Content(mods.manifest()))
            }),
            adopt: Arc::new(move |room, owns| {
                let again = shared.picker().and_then(|mut mods| {
                    mods.adopt(room, owns)
                        .then(|| Declaration::Content(mods.manifest()))
                });
                shared.show_mods();
                again
            }),
        })
    }
}

/// A connection not in any room.
struct Connected {
    client: Client,
    events: Events,
    options: ConnectOptions,
}

/// A room session, run by a bridge.
struct Session {
    controls: mpsc::Sender<Control>,
    task: JoinHandle<SessionEnded>,
    options: ConnectOptions,
    /// Its game closed after its hook attached: the session is ending, and
    /// gives the link back once it has.
    game_closed: bool,
}

/// How a room session ended, and the game's link it gives back, with the
/// game's build if its hook said hello.
type SessionEnded = (
    Result<BridgeEnd, bridge::BridgeFault>,
    tpf3mp_ipc::Link,
    Option<String>,
);

/// What the launcher's window says once it started the game: for its own
/// window only, not the game's (`lobby::view` leaves it out).
pub(crate) const GAME_STARTED: &str = "started Transport Fever 3 with TPF3-MP; its main menu has a Multiplayer entry, and it joins the room once it has loaded";

/// The game's link, served by the launcher while no room session holds it.
type Idle = Option<IdleLink<tpf3mp_ipc::Link>>;

/// Opens the link the game's hook attaches to (D11: the launcher names it in
/// the game's environment), as a new generation.
fn open_link(name: &str) -> Result<IdleLink<tpf3mp_ipc::Link>, String> {
    // Another launcher running with the same link would lose its game to
    // this one, and each game would show the other launcher's lobby.
    if let Some(pid) = tpf3mp_ipc::Link::held_by_another_agent(name, LINK_HELD_WAIT) {
        let message = format!(
            "another TPF3-MP launcher (process {pid}) uses the game link {name}: start this launcher with its own --game-link, or close the other"
        );
        warn!(%message);
        return Err(message);
    }
    tpf3mp_ipc::Link::create(&tpf3mp_ipc::Config::new(name), tpf3mp_ipc::Role::Agent)
        .map(IdleLink::new)
        .map_err(|error| format!("cannot open the link to the game: {error}"))
}

/// Whether a game is on the link: the one this launcher started, while it
/// runs, or else any game whose hook attached to it and whose process is
/// still there (one it took over, even if it fell silent).
fn game_on_link(game: &mut Option<tpf3mp_launch::Started>, link: &tpf3mp_ipc::Link) -> bool {
    match game {
        Some(started) => started.is_running(),
        None => hook_process_runs(link),
    }
}

/// Whether the process of the hook that last attached to the link may still
/// run. A process id used again by another program keeps the link as it
/// was: the next game may then find it taken, never a game cut off.
fn hook_process_runs(link: &tpf3mp_ipc::Link) -> bool {
    let pid = link.peer_pid();
    pid != 0 && tpf3mp_launch::process_runs(pid)
}

/// Opens the link anew, empty, unless a game is on it ([`game_on_link`]).
/// A game that closed never read what was sent last on its link (a room's
/// end, the lobby's updates); the next game's hook, finding that before
/// the launcher's hello, would refuse the link, and the game would say it
/// has no link to the launcher until the launcher restarted. Renewed only
/// when the link is about to serve again, not when a game closes: the
/// launcher does not always see that (a game it took over).
fn renew_unused_link(
    name: &str,
    game: &mut Option<tpf3mp_launch::Started>,
    idle: &mut Idle,
) -> Result<(), String> {
    if idle
        .as_ref()
        .is_some_and(|link| game_on_link(game, link.link()))
    {
        return Ok(());
    }
    // The old mapping goes first: on Unix its owner removes the name when
    // dropped, and would take the new one with it.
    *idle = None;
    *idle = Some(open_link(name)?);
    Ok(())
}

/// Ends a task when dropped.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The hook's and the game's logs at `places`, the hook's in the folder a
/// playtest gives its game (`TPF3MP_DATA_DIR`) when it gives one.
fn game_log_sources(
    config: &LauncherConfig,
    places: crate::game_logs::Places,
) -> crate::game_logs::Sources {
    let hook_log = config
        .game_env
        .iter()
        .find(|(name, value)| name == tpf3mp_ipc::DATA_DIR_ENV && !value.is_empty())
        .map_or(places.hook_log, |(_, dir)| {
            PathBuf::from(dir).join("hook.log")
        });
    crate::game_logs::Sources::new(hook_log, places.crash_dirs)
}

/// Carries out the page's actions, one at a time, and keeps the view.
async fn control(
    shared: Arc<Shared>,
    config: LauncherConfig,
    mut actions: Actions,
    mut lobby: LobbyEnds,
) {
    // The hook's and the game's logs from where they stand as the run
    // begins, for as long as it runs.
    let _game_logs = config
        .diagnostics
        .clone()
        .zip(config.game_logs.clone())
        .map(|(recorder, places)| {
            crate::game_logs::start(recorder, game_log_sources(&config, places))
        })
        .map(AbortOnDrop);
    let mut connected: Option<Connected> = None;
    let mut session: Option<Session> = None;
    // The game last started from here, while it may still be running.
    let mut game: Option<tpf3mp_launch::Started> = None;
    // The game's link, for as long as the launcher runs: the game can start
    // before a room is chosen, and its menu's window talks to the launcher
    // over it (D17).
    let mut idle: Idle = open_link(&config.link)
        .inspect_err(|error| {
            warn!(%error, "the game's link opens with the first room instead");
            // Said in the window too: two launchers on one link cross.
            shared.view().error = Some(error.clone());
        })
        .ok();
    // The native mods panel, and what the first game would run.
    refresh_native(&shared, &config, false).await;
    // The room's native mods last matched in its lobby, so a list that
    // cannot be matched is tried once; and the room to join again once the
    // game has started anew with them.
    let mut native_tried: Option<Vec<tpf3mp_proto::NativeMod>> = None;
    let mut rejoin: Option<(Invite, Option<Text<64>>)> = None;
    let mut tick = tokio::time::interval(LOBBY_TICK);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut saves_tick = tokio::time::interval(SAVES_TICK);
    saves_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // The bridge version of another TPF3-MP's hook, once said.
    let mut told_other_hook: Option<u32> = None;
    loop {
        tokio::select! {
            _ = saves_tick.tick() => {
                let saves = tokio::task::spawn_blocking(crate::steam::list_saves)
                    .await
                    .unwrap_or_default();
                shared.view().saves = saves;
            }
            action = actions.recv() => {
                let Some((action, reply)) = action else {
                    return;
                };
                let result = act(&shared, &config, action, &mut connected, &mut session, &mut game, &mut idle, &mut rejoin).await;
                shared.view().error = result.as_ref().err().cloned();
                let _ = reply.send(result);
            }
            Some(asked) = lobby.actions.recv() => {
                lobby_act(&shared, &config, asked, &mut connected, &mut session, &mut game, &mut idle, &mut rejoin).await;
            }
            _ = tick.tick() => {
                shared.show_mods();
                // The round trip to the server played on, for its row.
                if let Some(current) = &connected {
                    shared.view().home_ping = Some(current.client.rtt());
                }
                let view = lobby::view(&shared.snapshot());
                lobby.views.send_if_modified(|told| {
                    let changed = *told != view;
                    if changed {
                        told.clone_from(&view);
                    }
                    changed
                });
                // A game that has closed is no longer there to show.
                if session.is_none() && game.as_mut().is_some_and(|started| !started.is_running()) {
                    game = None;
                    shared.status().game = None;
                    idle = idle.take().map(IdleLink::forget_game);
                }
                let asked = match &mut idle {
                    Some(link) => {
                        let asked = link.pump(&view).unwrap_or_else(|error| {
                            warn!(%error, "serving the game's link failed");
                            Vec::new()
                        });
                        if let Some(build) = link.build() {
                            let mut status = shared.status();
                            if status.game.is_none() {
                                status.game = Some(build.to_owned());
                            }
                        }
                        asked
                    }
                    None => Vec::new(),
                };
                // A game this launcher did not start: one that followed the
                // link from a launcher that closed for this one. Its hook
                // falling silent and its process gone say it closed.
                if session.is_none()
                    && game.is_none()
                    && idle.as_ref().is_some_and(|link| {
                        link.build().is_some()
                            && link.hook_quiet(std::time::Instant::now()) > ADOPTED_GAME_QUIET
                            && !hook_process_runs(link.link())
                    })
                {
                    info!("the game another launcher started stopped answering; it counts as closed");
                    shared.status().game = None;
                    idle = idle.take().map(IdleLink::forget_game);
                }
                let other_hook = idle.as_ref().and_then(IdleLink::other_bridge);
                if other_hook != told_other_hook {
                    told_other_hook = other_hook;
                    if let Some(version) = other_hook {
                        shared.view().error = Some(other_hook_message(version));
                    }
                }
                for asked in asked {
                    lobby_act(&shared, &config, asked, &mut connected, &mut session, &mut game, &mut idle, &mut rejoin).await;
                }
                // A room's lobby whose native mods this game does not run:
                // get them, once for each list the room tells.
                if let Some(room) = native_to_match(&shared, &config, session.as_ref(), &native_tried) {
                    native_tried = Some(room.clone());
                    if let Err(error) = sync_room_native(&shared, &config, room, &mut connected, &session, &mut game, &mut idle, &mut rejoin).await {
                        warn!(%error, "cannot get the room's native mods");
                        shared.view().error = Some(error);
                    }
                }
            }
            ended = session_end(&mut session) => {
                let finished = session.take();
                let (ended, link) = match ended {
                    Ok((ended, link, build)) => {
                        let game_runs = game_on_link(&mut game, &link);
                        (ended, Some(IdleLink::given_back(link, build, game_runs)))
                    }
                    // Ended by Leave when the session would not take it.
                    Err(error) if error.is_cancelled() => (Ok(BridgeEnd::Left), None),
                    Err(error) => (Err(bridge::BridgeFault::Rejoin(error.to_string())), None),
                };
                // The game keeps its link for the next room; a session that
                // failed outright left none, so a new one is opened.
                idle = link.or_else(|| open_link(&config.link).ok());
                // Losing the room for good is said as it is, in both windows.
                let room_lost = room_lost(&ended);
                let message = match (&ended, &room_lost) {
                    (_, Some(lost)) => lost.clone(),
                    (Ok(end), None) => format!("the game session ended: {}", describe(end)),
                    (Err(fault), None) => format!("the game session failed: {fault}"),
                };
                info!(%message);
                {
                    // No game plays through a session that ended.
                    let mut status = shared.status();
                    status.notice(message);
                    status.game = None;
                    // Nor is it the room's any longer whose mods it lacked.
                    status.content_diff = None;
                    status.room_mods = None;
                }
                {
                    let mut view = shared.view();
                    view.invite = None;
                    view.in_room = false;
                }
                // Back to the server, ready for the next room.
                if let Some(finished) = finished {
                    if let Some(mut mods) = shared.picker() {
                        mods.forget_room();
                    }
                    shared.show_mods();
                    connected =
                        reconnect(&shared, finished.options, shared.content(&config)).await;
                }
                // Left to start the game again with the room's native mods:
                // start it, and join the room again.
                native_tried = None;
                if let Some((invite, password)) = rejoin.take() {
                    let again = async {
                        launch_game(&shared, &config, &session, &mut game, &mut idle).await?;
                        join(&shared, &config, &mut connected, &mut session, &mut idle, &mut game, invite, password).await
                    };
                    if let Err(error) = again.await {
                        warn!(%error, "cannot join the room again with its native mods");
                        shared.view().error = Some(error);
                    }
                }
                if let Some(lost) = room_lost {
                    let mut view = shared.view();
                    view.error = Some(match view.error.take() {
                        // Not back on the server either: say both.
                        Some(error) if connected.is_none() => format!("{lost}. {error}"),
                        _ => lost,
                    });
                }
            }
            () = game_exit(&mut game) => {
                // Its session cannot go on without it: end it now rather
                // than when the hook's heartbeat limit runs out, so the
                // player can start the game again at once.
                game = None;
                info!("Transport Fever 3 closed");
                match &mut session {
                    Some(session) => {
                        session.game_closed = shared.status().game.is_some();
                        let _ = session.controls.send(Control::GameClosed).await;
                    }
                    // Outside a room the launcher's own link held its hook.
                    None => {
                        shared.status().game = None;
                        idle = idle.take().map(IdleLink::forget_game);
                    }
                }
            }
            event = next_event(&mut connected) => match event {
                Some(ClientEvent::Closed(reason)) => {
                    connected = None;
                    let mut view = shared.view();
                    view.connected = false;
                    view.error = Some(format!("disconnected: {reason}"));
                }
                Some(ClientEvent::Notice(text)) => shared.status().announce(text.as_str()),
                // Outside a room there is nothing else to hear.
                Some(_) => {}
                None => {
                    connected = None;
                    shared.view().connected = false;
                }
            },
        }
    }
}

/// One action from the game's main-menu window (D17): the launcher's own,
/// on its own server. A refusal shows in both windows, as a page's does.
#[allow(clippy::too_many_arguments)]
async fn lobby_act(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    asked: LobbyAction,
    connected: &mut Option<Connected>,
    session: &mut Option<Session>,
    game: &mut Option<tpf3mp_launch::Started>,
    idle: &mut Idle,
    rejoin: &mut Option<(Invite, Option<Text<64>>)>,
) {
    let state = shared.snapshot();
    let action = lobby::action(asked, &state);
    // The kind only: a join carries its invite, which is not logged bare
    // (D13).
    info!(
        action = action_kind(&action),
        "the game's Multiplayer window asks"
    );
    let result = act(
        shared, config, action, connected, session, game, idle, rejoin,
    )
    .await;
    shared.view().error = result.err();
}

fn action_kind(action: &Action) -> &'static str {
    match action {
        Action::Connect { .. } => "connect",
        Action::Disconnect => "disconnect",
        Action::Create { .. } => "create",
        Action::Join { .. } => "join",
        Action::Ready { .. } => "ready",
        Action::Start => "start",
        Action::Kick { .. } => "kick",
        Action::Chat { .. } => "chat",
        Action::Leave => "leave",
        Action::ChooseMod { .. } => "choose_mod",
        Action::Diagnostics { .. } => "diagnostics",
        Action::LaunchGame => "launch_game",
        Action::ListRooms { .. } => "list_rooms",
        Action::SetServer { .. } => "set_server",
        Action::SetBanner { .. } => "set_banner",
        Action::ChooseStart { .. } => "choose_start",
        Action::ChooseRoomMods { .. } => "choose_room_mods",
        Action::RescanMods => "rescan_mods",
        Action::NativeRefresh => "native_refresh",
        Action::NativeInstall { .. } => "native_install",
        Action::NativeUninstall { .. } => "native_uninstall",
        Action::NativeRollback { .. } => "native_rollback",
        Action::NativeEnable { .. } => "native_enable",
        Action::NativeInstallProposed { .. } => "native_install_proposed",
        Action::NativeShowProposed { .. } => "native_show_proposed",
        Action::NativeMatchRoom => "native_match_room",
        Action::NativeConsentDecision { .. } => "native_consent_decision",
    }
}

/// One action from the page.
#[allow(clippy::too_many_arguments)]
async fn act(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    action: Action,
    connected: &mut Option<Connected>,
    session: &mut Option<Session>,
    game: &mut Option<tpf3mp_launch::Started>,
    idle: &mut Idle,
    rejoin: &mut Option<(Invite, Option<Text<64>>)>,
) -> Result<(), String> {
    if shared.view().native_consent.is_some() {
        if matches!(&action, Action::Disconnect) {
            // Disconnect cancels the held operation; an answer to its prompt
            // can no longer apply to the authenticated source it named.
            shared.view().native_consent = None;
        } else if !matches!(&action, Action::NativeConsentDecision { .. }) {
            return Err(
                "approve or decline the native plugin review before starting another action".into(),
            );
        }
    }
    match action {
        Action::Connect { server, name } => {
            // A whole invite, as "Copy invite" gives it, connects and joins.
            let passed = passed_invite(&server);
            let on_list = shared.on_list();
            let target = if on_list {
                // On a multi-region release, the server text in a pasted
                // message is not routing authority. A bare code is resolved
                // across every trusted region after connecting, and a
                // non-invite server address is not a region selector.
                if passed.is_some()
                    || server.trim().is_empty()
                    // The game's Connect action echoes the launcher state,
                    // which includes its current trusted address. Accept it
                    // as context only; `connect_closest` still picks the
                    // primary and an arbitrary address is never accepted.
                    || servers::find(&config.servers, &server).is_some()
                {
                    None
                } else {
                    return Err("that is not an invite".into());
                }
            } else {
                Some(server_for(
                    fixed_server(shared).as_deref(),
                    &server,
                    passed.as_ref(),
                )?)
            };
            if session.is_some() {
                return Err("leave the room first".into());
            }
            let name = Text::new(name.trim()).map_err(|_| "that name is too long".to_owned())?;
            if name.as_str().is_empty() {
                return Err("choose a name".into());
            }
            // Keep the current primary in place until a typed invite has a
            // unique positive lookup. When starting disconnected, the
            // closest trusted region is only the temporary connection used
            // to issue the read-only probes.
            if !(on_list && passed.is_some() && connected.is_some()) {
                match target {
                    Some(server) => connect_to(shared, config, connected, &server, name).await?,
                    None => connect_closest(shared, config, connected, name).await?,
                }
            }
            match passed {
                Some(passed) => {
                    if on_list {
                        join_on_list(
                            shared,
                            config,
                            connected,
                            session,
                            idle,
                            passed.invite,
                            None,
                            None,
                            game,
                        )
                        .await
                    } else {
                        renew_unused_link(&config.link, game, idle)?;
                        join(
                            shared,
                            config,
                            connected,
                            session,
                            idle,
                            game,
                            passed.invite,
                            None,
                        )
                        .await
                    }
                }
                None => Ok(()),
            }
        }
        Action::Disconnect => {
            if let Some(session) = session.take() {
                let _ = session.controls.try_send(Control::Leave);
                let mut task = session.task;
                match tokio::time::timeout(DISCONNECT_WAIT, &mut task).await {
                    Ok(Ok((_, link, build))) => {
                        let game_runs = game_on_link(game, &link);
                        *idle = Some(IdleLink::given_back(link, build, game_runs));
                    }
                    // Stuck: stopped, and its link with it; a new one opens.
                    Err(_) => {
                        task.abort();
                        let _ = task.await;
                        *idle = open_link(&config.link).ok();
                    }
                    Ok(Err(_)) => *idle = open_link(&config.link).ok(),
                }
            }
            if let Some(connected) = connected.take() {
                connected.client.close().await;
            }
            shared.lookouts.stop().await;
            let mut view = shared.view();
            view.connected = false;
            view.in_room = false;
            view.invite = None;
            Ok(())
        }
        Action::Create {
            room,
            max_players,
            password,
            rules,
            start_save,
            listing,
            competitive,
        } => {
            if connected.is_none() {
                return Err("connect to a server first".into());
            }
            // On the release's servers, a room goes to the closest.
            if shared.on_list() {
                move_to_closest(shared, config, connected).await?;
            }
            let current = connected.as_ref().ok_or("connect to a server first")?;
            let rules = match rules.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(name) => Some(Text::new(name).map_err(|_| "no such rules".to_owned())?),
            };
            // Before the room exists: a save that cannot be found creates
            // no room.
            let listed = shared.view().saves.clone();
            let start_world = start_world(
                start_save.as_deref(),
                &listed,
                config.start_save.as_ref(),
                crate::steam::find_save,
            )?;
            if !room_list_runs_own_mod(shared.picker().as_deref(), start_world.as_deref()) {
                check_start_save(start_world.as_deref())?;
            }
            // The room's shared mods are the start save's, less this
            // player's personal ones: declared before the room exists, so
            // the room compares every guest's with them (docs/MODS.md).
            if let Some(declaration) = own_start(shared, start_world.as_deref()) {
                current
                    .client
                    .declare(declaration)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            let create = CreateRoom {
                name: Text::new(room.trim())
                    .map_err(|_| "that room name is too long".to_owned())?,
                max_players,
                password: password_text(password)?,
                settings: tpf3mp_proto::RoomSettings {
                    scan_plan: tpf3mp_proto::ScanPlan::from_environment_overrides(&config.game_env)
                        .map_err(str::to_owned)?,
                    ..config.room_settings
                },
                rules,
                // A public room starts with one company, the save's own
                // (D21); its owner says when there are more.
                listing: listing.map(|listing| tpf3mp_proto::RoomListing {
                    map: Text::lossy(listing.map.trim()),
                    year: listing.year,
                    companies: 1,
                }),
                competitive,
            };
            // What the room shows everyone of its start save: the list's
            // map and year, when the owner's game read them.
            let start_world = start_world.map(|file| {
                let save = start_save_named(
                    &file,
                    create
                        .listing
                        .as_ref()
                        .map(|l| l.map.as_str())
                        .unwrap_or(""),
                    create.listing.as_ref().map_or(0, |l| l.year),
                );
                (file, save)
            });
            let (invite, room) = current
                .client
                .create_room(create.clone())
                .await
                .map_err(|error| error.to_string())?;
            shared.status().room = Some(room);
            let generate_world = start_save
                .as_deref()
                .is_some_and(|save| save.trim().is_empty());
            if let Some(picked) = start_save.as_deref() {
                shared.view().start_save = offered_start(picked);
            }
            renew_unused_link(&config.link, game, idle)?;
            begin_session(
                shared,
                config,
                connected,
                session,
                idle,
                invite,
                create.password,
                start_world,
                generate_world,
            )
        }
        Action::ChooseStart { save, map, year } => {
            choose_start(shared, config, session, &save, &map, year).await
        }
        Action::ChooseRoomMods {
            save,
            map,
            year,
            mods,
            params,
        } => {
            let start = (!save.trim().is_empty()).then_some((save.as_str(), map.as_str(), year));
            choose_room_mods(shared, config, session, start, &mods, &params).await
        }
        Action::RescanMods => rescan_mods(shared, config, connected, session).await,
        Action::NativeRefresh => {
            native_change(shared, config, Change::Refresh, connected, session, game).await
        }
        Action::NativeInstall { id } => {
            native_change(
                shared,
                config,
                Change::Install(id),
                connected,
                session,
                game,
            )
            .await
        }
        Action::NativeUninstall { id } => {
            native_change(
                shared,
                config,
                Change::Uninstall(id),
                connected,
                session,
                game,
            )
            .await
        }
        Action::NativeRollback { id } => {
            native_change(
                shared,
                config,
                Change::Rollback(id),
                connected,
                session,
                game,
            )
            .await
        }
        Action::NativeEnable { id, on } => {
            native_change(
                shared,
                config,
                Change::Enable(id, on),
                connected,
                session,
                game,
            )
            .await
        }
        Action::NativeInstallProposed { id } => {
            native_change(
                shared,
                config,
                Change::InstallProposed(id),
                connected,
                session,
                game,
            )
            .await
        }
        Action::NativeShowProposed { on } => {
            shared.view().native_proposed = on;
            let server = shared.view().server.clone();
            if on && let (Some(current), Some(server)) = (connected.as_ref(), server.as_deref()) {
                fetch_server_native(shared, &current.client, server, true).await;
            }
            refresh_native(shared, config, true).await;
            Ok(())
        }
        Action::NativeMatchRoom => {
            let room = shared
                .status()
                .room_mods
                .as_ref()
                .map(|room| room.native.clone())
                .ok_or("the room has not told its native mods")?;
            sync_room_native(shared, config, room, connected, session, game, idle, rejoin).await?;
            Ok(())
        }
        Action::NativeConsentDecision {
            request_id,
            approved,
        } => {
            native_consent_decision(
                shared, config, request_id, approved, connected, session, game, idle, rejoin,
            )
            .await
        }
        Action::Join {
            invite,
            server,
            password,
        } => {
            let passed = passed_invite(&invite).ok_or("that is not an invite")?;
            if shared.on_list() {
                // Only the region field supplied by an explicit choice or a
                // verified public-room card is routing authority. A server
                // token pasted beside the code is ignored: private rooms
                // are not listed, so a cached public match cannot prove
                // uniqueness.
                let selected = servers::selected_room_target(&config.servers, server.as_deref())?;
                let _current = connected.as_ref().ok_or("connect to a server first")?;
                let password = password_text(password)?;
                return join_on_list(
                    shared,
                    config,
                    connected,
                    session,
                    idle,
                    passed.invite,
                    password,
                    selected,
                    game,
                )
                .await;
            }
            if server.is_some() {
                return Err("that room card names a server this launcher does not play on".into());
            }
            // An invite to another server is refused, connected or not.
            if let (Some(fixed), Some(other)) = (fixed_server(shared), &passed.server)
                && !same_server(&fixed, other)
            {
                return Err(elsewhere(&fixed, other));
            }
            let current = connected.as_ref().ok_or("connect to a server first")?;
            let password = password_text(password)?;
            // Without a fixed server, an invite to another server takes the
            // player there first.
            let here = shared.view().server.clone();
            if let Some(server) = passed.server
                && !here.is_some_and(|here| same_server(&here, &server))
            {
                let name = current.options.name.clone();
                connect_to(shared, config, connected, &server, name).await?;
            }
            renew_unused_link(&config.link, game, idle)?;
            join(
                shared,
                config,
                connected,
                session,
                idle,
                game,
                passed.invite,
                password,
            )
            .await
        }
        Action::Ready { ready } => forward(session, Control::Ready(ready)).await,
        Action::Start => forward(session, Control::Start).await,
        Action::Kick { player } => {
            let player = api::parse_player(&player).ok_or("that is not a player")?;
            forward(session, Control::Kick(player)).await
        }
        Action::Chat { text } => {
            let text = Text::new(text.trim()).map_err(|_| "that message is too long".to_owned())?;
            if text.as_str().is_empty() {
                return Ok(());
            }
            forward(session, Control::Chat(text)).await
        }
        Action::Leave => leave(session),
        Action::LaunchGame => launch_game(shared, config, session, game, idle).await,
        Action::ChooseMod { id, chosen } => {
            let chosen_now = {
                let mut mods = shared
                    .picker()
                    .ok_or("this launcher takes its mods from --mods")?;
                mods.choose(&id, chosen)?;
                mods.chosen()
            };
            shared.show_mods();
            if let Some(file) = &config.remember {
                let mut remembered = Remembered::load(file);
                remembered.mods = Some(chosen_now);
                if let Err(error) = remembered.save(file) {
                    warn!(%error, "cannot remember the mods chosen for next time");
                }
            }
            info!(id, chosen, "a mod chosen");
            if session.is_some() {
                shared
                    .status()
                    .notice("the mods you choose now load with the room's next world".to_owned());
            }
            Ok(())
        }
        Action::ListRooms { page } => {
            let current = connected.as_ref().ok_or("connect to a server first")?;
            let here = shared.view().server.clone().unwrap_or_default();
            let home = servers::find(&config.servers, &here).filter(|_| shared.on_list());
            let Some(home) = home else {
                let own = current
                    .client
                    .list_rooms(page)
                    .await
                    .map_err(|error| error.to_string())?;
                shared.view().rooms = Some(api::RoomList::of(&own));
                return Ok(());
            };
            let list = global_room_page(shared, config, current, home, page).await?;
            shared.view().rooms = Some(list);
            Ok(())
        }
        Action::SetBanner { banner } => {
            let banner = banner
                .map(|id| id.trim().to_owned())
                .filter(|id| !id.is_empty());
            let id = match &banner {
                Some(id) if tpf3mp_proto::is_banner(id) => Some(Text::lossy(id)),
                Some(_) => return Err("there is no such banner".into()),
                None => None,
            };
            shared.view().banner.clone_from(&banner);
            if let Some(file) = &config.remember {
                let mut remembered = Remembered::load(file);
                remembered.banner = banner;
                if let Err(error) = remembered.save(file) {
                    warn!(%error, "cannot remember the banner for next time");
                }
            }
            match (session.as_ref(), connected.as_ref()) {
                (Some(_), _) => forward(session, bridge::Control::Banner(id)).await,
                (None, Some(current)) => current
                    .client
                    .request(tpf3mp_proto::Request::SetBanner(id))
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                (None, None) => Ok(()),
            }
        }
        Action::SetServer { server } => {
            set_server(shared, config, &server, connected, session).await
        }
        Action::Diagnostics { on } => {
            let recorder = config
                .diagnostics
                .as_ref()
                .ok_or("this launcher sends no diagnostics")?;
            recorder.set_on(on);
            shared.view().diagnostics = Some(on);
            if let Some(file) = &config.remember {
                let mut remembered = Remembered::load(file);
                remembered.diagnostics = Some(on);
                if let Err(error) = remembered.save(file) {
                    warn!(%error, "cannot remember the diagnostics choice for next time");
                }
            }
            info!(on, "diagnostics switched");
            Ok(())
        }
    }
}

/// What the player is told of a game whose hook speaks bridge `version`:
/// one started by another TPF3-MP's launcher, which this one cannot serve.
fn other_hook_message(version: u32) -> String {
    format!(
        "Transport Fever 3 runs the hook of another TPF3-MP (game link version {version}, this \
        launcher {}), started by another launcher: close the game, then start it again from here.",
        tpf3mp_bridge::BRIDGE_VERSION
    )
}

/// Starts Transport Fever 3 with the hook in it, told the launcher's link:
/// the only way the hook runs (D11). A game started from Steam is the plain
/// game. It may start before a room is chosen: its main menu's Multiplayer
/// window connects, creates and joins through this launcher (D17).
async fn launch_game(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    session: &Option<Session>,
    game: &mut Option<tpf3mp_launch::Started>,
    idle: &mut Idle,
) -> Result<(), String> {
    if !tpf3mp_launch::SUPPORTED {
        return Err(tpf3mp_launch::LaunchError::Unsupported.to_string());
    }
    // One game at a time: a second click while the first loads starts none.
    if game
        .as_mut()
        .is_some_and(tpf3mp_launch::Started::is_running)
    {
        return Err(
            "Transport Fever 3 is already running from here; it joins once it has loaded".into(),
        );
    }
    // The room still lets go of the game that closed; its link comes back
    // once it has, and is renewed for the next.
    // A game that exited before the launcher handled it counts as closing.
    if session.as_ref().is_some_and(|session| session.game_closed)
        || session.is_some() && game.as_mut().is_some_and(|started| !started.is_running())
    {
        return Err(
            "the room is still letting go of the game that closed: start it again in a moment"
                .into(),
        );
    }
    // A game started by a launcher this one took over from, linked here,
    // silent or not.
    if game.is_none()
        && idle
            .as_ref()
            .is_some_and(|link| link.build().is_some() || hook_process_runs(link.link()))
    {
        return Err(
            "Transport Fever 3 is already running with TPF3-MP, linked to this launcher: use its \
            Multiplayer window"
                .into(),
        );
    }
    // The game needs Steam to start; without it, it would quit or start
    // again through Steam, without the hook. TPF2MP's launcher asked the same.
    if tpf3mp_launch::steam_running() == Some(false) {
        return Err(tpf3mp_launch::LaunchError::NoSteam.to_string());
    }
    let exe = game_exe(config)?;
    let hook = config
        .hook
        .clone()
        .ok_or("this TPF3-MP has no hook library for the game")?;
    // The link the game's hook attaches to: the room session's, or the
    // launcher's own, empty.
    if session.is_none() {
        renew_unused_link(&config.link, game, idle)?;
    }
    // The native mods the player enabled, for this build (D29).
    let native_mods = match setup::data_dir() {
        Ok(data) => {
            let exe = exe.clone();
            tokio::task::spawn_blocking(move || crate::native_mods::game_env(&data, &exe))
                .await
                .map_err(|error| error.to_string())??
        }
        Err(_) => crate::native_mods::GameNative::default(),
    };
    let native_terms = native_mods.terms;
    let native_mods = native_mods.env;
    let launch = tpf3mp_launch::Launch {
        exe,
        args: Vec::new(),
        hook,
        env: vec![
            (tpf3mp_ipc::LINK_ENV.to_owned(), config.link.clone()),
            (
                tpf3mp_ipc::LAUNCHER_PID_ENV.to_owned(),
                std::process::id().to_string(),
            ),
        ]
        .into_iter()
        .chain(native_mods)
        .chain(config.game_env.iter().cloned())
        .collect(),
        ready_wait: tpf3mp_launch::HOOK_READY_WAIT,
    };
    let view = lobby::view(&shared.snapshot());
    let started = wait_for_launch(
        tokio::task::spawn_blocking(move || tpf3mp_launch::start(&launch)),
        idle,
        &view,
    )
    .await?
    .map_err(|error| error.to_string())?;
    info!(pid = started.pid, "started the game with the hook");
    *game = Some(started);
    shared.set_native(native_terms);
    shared.status().notice(GAME_STARTED);
    Ok(())
}

/// The game's executable the launcher starts.
fn game_exe(config: &LauncherConfig) -> Result<PathBuf, String> {
    match &config.game_exe {
        Some(exe) => Ok(exe.clone()),
        None => {
            let installed = config
                .installed
                .as_ref()
                .ok_or("Transport Fever 3 was not found in Steam")?;
            tpf3mp_launch::find_executable(&installed.dir).ok_or_else(|| {
                format!(
                    "cannot tell which program in {} is the game; start the launcher with --game-exe",
                    installed.dir.display()
                )
            })
        }
    }
}

/// Whether the game started from here runs.
fn game_runs(game: &mut Option<tpf3mp_launch::Started>) -> bool {
    game.as_mut()
        .is_some_and(tpf3mp_launch::Started::is_running)
}

/// Reads the native mods panel again, and with no game running from here
/// (`game_runs` false), what the next game will run: what this player
/// declares meanwhile.
async fn refresh_native(shared: &Arc<Shared>, config: &LauncherConfig, game_runs: bool) {
    let Ok(data) = setup::data_dir() else {
        return;
    };
    let setup = config.native.clone();
    let exe = game_exe(config).ok();
    let (server, proposed) = {
        let view = shared.view();
        (view.server.clone(), view.native_proposed)
    };
    let read = tokio::task::spawn_blocking(move || {
        let build = exe
            .as_deref()
            .and_then(|exe| crate::native_mods::build_of(exe).ok());
        let view = crate::native_mods::view(
            &data,
            setup.as_ref(),
            build.as_deref(),
            server.as_deref(),
            proposed,
        );
        let next = exe
            .as_deref()
            .map(|exe| crate::native_mods::next_terms(&data, exe));
        (view, next)
    })
    .await;
    let Ok((view, next)) = read else {
        return;
    };
    shared.view().native = view;
    if !game_runs {
        match next {
            Some(Ok(terms)) => {
                shared.set_native(terms);
            }
            Some(Err(error)) => warn!(%error, "cannot tell which native mods the next game runs"),
            None => {}
        }
    }
}

/// Takes in the native mods `server` offers over `client`: its index, and
/// with `proposed` the mods proposed for its registry. A server that offers
/// none, or one that cannot be read, leaves what was kept; why is logged.
async fn fetch_server_native(shared: &Shared, client: &Client, server: &str, proposed: bool) {
    let Ok(data) = setup::data_dir() else {
        return;
    };
    for proposals in [false, true] {
        if proposals && !proposed {
            continue;
        }
        match client.native_index(proposals).await {
            Ok(Some((serial, json))) => {
                let (data, server) = (data.clone(), server.to_owned());
                let kept = tokio::task::spawn_blocking(move || {
                    crate::native_mods::accept_server_index(
                        &data, &server, serial, &json, proposals,
                    )
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|kept| kept);
                if let Err(error) = kept {
                    warn!(%error, "cannot take in the server's native mods");
                    shared.view().error = Some(error);
                }
            }
            Ok(None) => {}
            Err(error) => warn!(%error, "the server did not say which native mods it offers"),
        }
    }
}

/// Whether the launcher may get a room's native mods by itself: from the
/// project's signed index, or from the index of a server the release lists.
/// From a server the player set themselves it does so only when asked.
fn auto_native(shared: &Shared, config: &LauncherConfig) -> bool {
    if config
        .native
        .as_ref()
        .is_some_and(|setup| !setup.keys.is_empty())
    {
        return true;
    }
    let view = shared.view();
    view.server.as_deref().is_some_and(|server| {
        servers::find(&config.servers, server).is_some()
            || view
                .server_default
                .as_deref()
                .is_some_and(|default| same_server(default, server))
    })
}

/// Declares this player's content anew: in the room, or on the server.
async fn redeclare(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    connected: &mut Option<Connected>,
    session: &Option<Session>,
) -> Result<(), String> {
    let declaration = shared.content(config);
    if session.is_some() {
        return forward(session, Control::Declare(declaration)).await;
    }
    if let Some(current) = connected.as_ref() {
        current
            .client
            .declare(declaration)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Re-fetches one native-mod index through the authenticated connection to
/// the selected server. A room owns its connection in the bridge, so a
/// short-lived read-only connection uses the same trusted connection options
/// while the room is active.
async fn refresh_authenticated_server_native(
    connected: &Option<Connected>,
    session: &Option<Session>,
    data: &Path,
    server: &str,
    proposed: bool,
) -> Result<bool, String> {
    let response = if let Some(current) = connected.as_ref() {
        current
            .client
            .native_index(proposed)
            .await
            .map_err(|error| error.to_string())?
    } else if let Some(session) = session.as_ref() {
        let mut options = session.options.clone();
        options.diagnostics = None;
        options.banner = None;
        let (client, _events) = connect(options).await.map_err(|error| error.to_string())?;
        let response = client.native_index(proposed).await;
        client.close().await;
        response.map_err(|error| error.to_string())?
    } else {
        return Err("connect to the server again before reviewing its native mods".into());
    };
    let Some((serial, json)) = response else {
        return Ok(false);
    };
    let data = data.to_owned();
    let server = server.to_owned();
    tokio::task::spawn_blocking(move || {
        crate::native_mods::accept_server_index(&data, &server, serial, &json, proposed)
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(true)
}

fn queue_plugin_consent(
    shared: &Arc<Shared>,
    action: String,
    packages: Vec<crate::native_mods::PluginConsentPackage>,
    operation: crate::native_mods::PendingPluginOperation,
) -> Result<bool, String> {
    if packages.iter().all(|package| package.approved) {
        return Ok(false);
    }
    let mut view = shared.view();
    if let Some(pending) = view.native_consent.as_ref() {
        if pending.operation == operation && pending.prompt.packages == packages {
            return Ok(true);
        }
        return Err(
            "finish the native plugin approval already waiting in the launcher first".into(),
        );
    }
    view.native_consent = Some(crate::native_mods::PendingPluginConsent {
        prompt: crate::native_mods::PluginConsentPrompt {
            request_id: random_token(),
            action,
            packages,
        },
        operation,
    });
    Ok(true)
}

fn native_change_consent_action(change: &Change) -> String {
    match change {
        Change::Install(id) => format!("Install native mod {id}"),
        Change::InstallProposed(id) => format!("Install proposed native mod {id}"),
        Change::Rollback(id) => format!("Roll back native mod {id}"),
        Change::Enable(id, true) => format!("Enable native mod {id}"),
        _ => "Change native mods".to_owned(),
    }
}

fn cached_server_for_offline_install(server: Option<&str>, proposed: bool) -> Option<String> {
    if proposed {
        None
    } else {
        server.map(str::to_owned)
    }
}

/// One change the player makes in the native mods panel, then the panel
/// read again and, when no game runs from here, the content declared anew.
async fn native_change(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    change: crate::native_mods::Change,
    connected: &mut Option<Connected>,
    session: &Option<Session>,
    game: &mut Option<tpf3mp_launch::Started>,
) -> Result<(), String> {
    let setup = config
        .native
        .clone()
        .ok_or("this launcher was built without native mods")?;
    let data = setup::data_dir().map_err(|error| error.to_string())?;
    let exe = game_exe(config).ok();
    let (server, proposed_shown) = {
        let view = shared.view();
        (view.server.clone(), view.native_proposed)
    };
    if change == Change::Refresh
        && let (Some(current), Some(server)) = (connected.as_ref(), server.as_deref())
    {
        shared.view().native_busy = Some("Looking for native mods".to_owned());
        fetch_server_native(shared, &current.client, server, proposed_shown).await;
    }
    let install = matches!(&change, Change::Install(_) | Change::InstallProposed(_));
    let proposed = matches!(&change, Change::InstallProposed(_));
    let mut action_server = if install && connected.is_none() && session.is_none() {
        cached_server_for_offline_install(server.as_deref(), proposed)
    } else {
        None
    };
    let mut build = None;
    if install {
        let exe = exe
            .as_deref()
            .ok_or("start the game once, or name it, so the launcher knows its build")?;
        build = Some(crate::native_mods::build_of(exe)?);
        if let Some(server) = server.as_deref() {
            if connected.is_some() || session.is_some() {
                shared.view().native_busy = Some("Checking the current native-mod index".into());
                let current = match refresh_authenticated_server_native(
                    connected, session, &data, server, proposed,
                )
                .await
                {
                    Ok(current) => current,
                    Err(error) => {
                        shared.view().native_busy = None;
                        return Err(error);
                    }
                };
                if current {
                    action_server = Some(server.to_owned());
                } else if proposed {
                    shared.view().native_busy = None;
                    return Err("the server no longer offers its proposed native-mod index".into());
                }
            } else if proposed {
                return Err("connect to the server that proposed this native mod".into());
            }
        }
        if proposed && action_server.is_none() {
            shared.view().native_busy = None;
            return Err("connect to the server that proposed this native mod".into());
        }
    }
    let review = if matches!(
        &change,
        Change::Install(_)
            | Change::InstallProposed(_)
            | Change::Enable(_, true)
            | Change::Rollback(_)
    ) {
        shared.view().native_busy = Some("Checking executable plugin approval".into());
        let data = data.clone();
        let setup = setup.clone();
        let server = action_server.clone();
        let change = change.clone();
        let build = build.clone();
        let refresh_project = install;
        match tokio::task::spawn_blocking(move || {
            crate::native_mods::plugin_consent_review(
                &data,
                &setup,
                build.as_deref(),
                server.as_deref(),
                &change,
                refresh_project,
            )
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|result| result)
        {
            Ok(review) => review,
            Err(error) => {
                shared.view().native_busy = None;
                return Err(error);
            }
        }
    } else {
        Vec::new()
    };
    shared.view().native_busy = None;
    if !review.iter().all(|package| package.approved)
        && action_server.is_some()
        && connected.is_none()
        && session.is_none()
    {
        return Err(
            "connect to the native-mod source before reviewing new executable plugin code".into(),
        );
    }
    if !review.iter().all(|package| package.approved)
        && queue_plugin_consent(
            shared,
            native_change_consent_action(&change),
            review,
            crate::native_mods::PendingPluginOperation::Change(change.clone()),
        )?
    {
        return Ok(());
    }
    shared.view().native_busy = Some(match &change {
        Change::Refresh => "Looking for native mods".to_owned(),
        Change::Install(id) => format!("Installing {id}"),
        Change::InstallProposed(id) => format!("Installing {id}, proposed and not reviewed"),
        _ => "Changing native mods".to_owned(),
    });
    let apply_build = build.clone();
    let result = tokio::task::spawn_blocking(move || {
        crate::native_mods::apply(
            &data,
            &setup,
            apply_build.as_deref(),
            action_server.as_deref(),
            change,
        )
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|result| result);
    shared.view().native_busy = None;
    let runs = game_runs(game);
    let before = shared.view().native_terms.clone();
    refresh_native(shared, config, runs).await;
    if !runs && shared.view().native_terms != before {
        redeclare(shared, config, connected, session).await?;
    }
    result
}

/// Applies the player's answer only to the exact prompt still displayed,
/// then revalidates that package closure against current metadata before it
/// stores consent or resumes the held action.
#[allow(clippy::too_many_arguments)]
async fn native_consent_decision(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    request_id: String,
    approved: bool,
    connected: &mut Option<Connected>,
    session: &mut Option<Session>,
    game: &mut Option<tpf3mp_launch::Started>,
    idle: &mut Idle,
    rejoin: &mut Option<(Invite, Option<Text<64>>)>,
) -> Result<(), String> {
    let pending = shared
        .view()
        .native_consent
        .clone()
        .ok_or("there is no native plugin approval waiting")?;
    if pending.prompt.request_id != request_id {
        return Err("that native plugin approval request has expired".into());
    }
    if !approved {
        shared.view().native_consent = None;
        return Err(match pending.operation {
            crate::native_mods::PendingPluginOperation::MatchRoom { .. } => {
                "you declined the room's native plugin code; joining remains blocked until you approve and match those mods".into()
            }
            crate::native_mods::PendingPluginOperation::Change(_) => {
                "you declined the native plugin code; the requested change was not run".into()
            }
        });
    }
    let setup = config
        .native
        .clone()
        .ok_or("this launcher was built without native mods")?;
    let data = setup::data_dir().map_err(|error| error.to_string())?;
    let build = game_exe(config)
        .ok()
        .and_then(|exe| crate::native_mods::build_of(&exe).ok());
    let (server, needs_server_index, proposed) = match &pending.operation {
        crate::native_mods::PendingPluginOperation::MatchRoom { room, .. } => {
            (shared.view().server.clone(), !room.is_empty(), false)
        }
        crate::native_mods::PendingPluginOperation::Change(change) => (
            shared.view().server.clone(),
            matches!(change, Change::Install(_) | Change::InstallProposed(_)),
            matches!(change, Change::InstallProposed(_)),
        ),
    };
    let mut review_server = None;
    if needs_server_index
        && (connected.is_some() || session.is_some())
        && let Some(server) = server.as_deref()
    {
        let current =
            match refresh_authenticated_server_native(connected, session, &data, server, proposed)
                .await
            {
                Ok(current) => current,
                Err(error) => return Err(error),
            };
        if current {
            review_server = Some(server.to_owned());
        } else if proposed {
            return Err("the server no longer offers the proposed native-mod index; the change remains blocked".into());
        }
    }
    if needs_server_index && proposed && review_server.is_none() {
        return Err("connect to the server that proposed this native mod".into());
    }
    let operation = pending.operation.clone();
    let expected = pending.prompt.packages.clone();
    let review_setup = setup.clone();
    let review_data = data.clone();
    let review_server_for_worker = review_server.clone();
    let build_for_worker = build.clone();
    let changed = tokio::task::spawn_blocking(move || {
        crate::native_mods::approve_plugin_consent(
            &review_data,
            &review_setup,
            build_for_worker.as_deref(),
            review_server_for_worker.as_deref(),
            &operation,
            &expected,
        )
    })
    .await
    .map_err(|error| error.to_string())??;
    if let Some(mut packages) = changed {
        if packages.is_empty() {
            shared.view().native_consent = None;
            return Err("the native package changed after review and no longer needs plugin consent; retry the action to inspect the current package before continuing".into());
        }
        // An approval click cannot approve a changed package. Present the
        // exact new closure as a distinct request even if it was approved in
        // an unrelated earlier action.
        for package in &mut packages {
            package.approved = false;
        }
        let mut view = shared.view();
        if view
            .native_consent
            .as_ref()
            .is_none_or(|current| current.prompt.request_id != request_id)
        {
            return Err("that native plugin approval request has expired".into());
        }
        view.native_consent = Some(crate::native_mods::PendingPluginConsent {
            prompt: crate::native_mods::PluginConsentPrompt {
                request_id: random_token(),
                action: pending.prompt.action,
                packages,
            },
            operation: pending.operation,
        });
        return Ok(());
    }
    {
        let mut view = shared.view();
        if view
            .native_consent
            .as_ref()
            .is_none_or(|current| current.prompt.request_id != request_id)
        {
            return Err("that native plugin approval request has expired".into());
        }
        view.native_consent = None;
    }
    match pending.operation {
        crate::native_mods::PendingPluginOperation::Change(change) => {
            native_change(shared, config, change, connected, session, game).await
        }
        crate::native_mods::PendingPluginOperation::MatchRoom { room, resume_join } => {
            if resume_join {
                let (invite, password) = shared
                    .last_join
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
                    .ok_or(
                        "the room's native mods were approved; join the room again to continue",
                    )?;
                join(
                    shared, config, connected, session, idle, game, invite, password,
                )
                .await
            } else if session.is_some() {
                sync_room_native(shared, config, room, connected, session, game, idle, rejoin).await
            } else {
                Err("the room session ended while approval was open; join it again to match its native mods".into())
            }
        }
    }
}

/// The room's native mods when this game must get them: in a room's lobby
/// whose list (as the room told it) is not what this game runs, and not
/// tried before; never for the room's owner, whose game's list is the
/// room's.
fn native_to_match(
    shared: &Shared,
    config: &LauncherConfig,
    session: Option<&Session>,
    tried: &Option<Vec<tpf3mp_proto::NativeMod>>,
) -> Option<Vec<tpf3mp_proto::NativeMod>> {
    config.native.as_ref()?;
    session?;
    if !auto_native(shared, config) {
        return None;
    }
    let me = shared.view().player;
    let status = shared.status();
    let room = status.room.as_ref()?;
    if room.phase != RoomPhase::Lobby || Some(room.owner) == me {
        return None;
    }
    let told = status.room_mods.as_ref()?.native.clone();
    let same = |a: &[tpf3mp_proto::NativeMod], b: &[tpf3mp_proto::NativeMod]| {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(a, b)| a.id == b.id && a.version == b.version)
    };
    if same(&told, &shared.view().native_terms) || tried.as_deref().is_some_and(|t| same(t, &told))
    {
        return None;
    }
    Some(told)
}

/// Makes the next game run the room's native mods `room` (installed from
/// the signed index, with the room's settings), and says which they are
/// then.
async fn get_room_native(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    room: Vec<tpf3mp_proto::NativeMod>,
    connected: &Option<Connected>,
    session: &Option<Session>,
    resume_join: bool,
) -> Result<Vec<tpf3mp_proto::NativeMod>, String> {
    let setup = config
        .native
        .clone()
        .ok_or("the room runs native mods, and this launcher was built without them")?;
    let data = setup::data_dir().map_err(|error| error.to_string())?;
    let exe = game_exe(config)?;
    let server = shared.view().server.clone();
    let build = crate::native_mods::build_of(&exe)?;
    let mut source_server = None;
    let names = room
        .iter()
        .map(|m| m.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    info!(mods = %names, "getting the room's native mods");
    shared.view().native_busy = Some(if names.is_empty() {
        "Switching off native mods the room does not run".to_owned()
    } else {
        format!("Getting the room's native mods: {names}")
    });
    if !room.is_empty()
        && let Some(server) = server.as_deref()
    {
        let current =
            match refresh_authenticated_server_native(connected, session, &data, server, false)
                .await
            {
                Ok(current) => current,
                Err(error) => {
                    shared.view().native_busy = None;
                    return Err(error);
                }
            };
        if current {
            source_server = Some(server.to_owned());
        }
    }
    let review_data = data.clone();
    let review_setup = setup.clone();
    let review_room = room.clone();
    let review_server = source_server.clone();
    let review_build = build.clone();
    let review = match tokio::task::spawn_blocking(move || {
        crate::native_mods::room_plugin_consent_review(
            &review_data,
            &review_setup,
            &review_build,
            review_server.as_deref(),
            &review_room,
            true,
        )
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|result| result)
    {
        Ok(review) => review,
        Err(error) => {
            shared.view().native_busy = None;
            return Err(error);
        }
    };
    if !review.iter().all(|package| package.approved) {
        let action = if resume_join {
            "Join this room and match its native mods"
        } else {
            "Match this room's native mods"
        };
        let queued = queue_plugin_consent(
            shared,
            action.to_owned(),
            review,
            crate::native_mods::PendingPluginOperation::MatchRoom {
                room: room.clone(),
                resume_join,
            },
        );
        shared.view().native_busy = None;
        if queued? {
            return Err("review the native plugin packages before matching the room".into());
        }
    }
    let match_data = data.clone();
    let match_setup = setup.clone();
    let match_exe = exe.clone();
    let match_room = room.clone();
    let match_server = source_server.clone();
    let result = tokio::task::spawn_blocking(move || {
        crate::native_mods::match_room(
            &match_data,
            &match_setup,
            &build,
            match_server.as_deref(),
            &match_room,
        )?;
        crate::native_mods::next_terms(&match_data, &match_exe)
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|result| result);
    shared.view().native_busy = None;
    refresh_native(shared, config, true).await;
    result
}

/// Closes the game started from here, for it to start again with other
/// native mods, and waits until it has.
async fn close_game(shared: &Shared, game: &mut Option<tpf3mp_launch::Started>, idle: &mut Idle) {
    if let Some(mut started) = game.take() {
        info!(
            pid = started.pid,
            "closing the game to start it again with the room's native mods"
        );
        started.kill();
        let _ =
            tokio::time::timeout(Duration::from_secs(20), exited(|| started.is_running())).await;
    }
    shared.status().game = None;
    *idle = idle.take().map(IdleLink::forget_game);
}

/// In a room's lobby: gets the room's native mods, then, when the game
/// running from here runs others, leaves the room and closes the game, for
/// the session's end to start it again and join the room anew (`rejoin`).
/// With no game running, this player declares the room's at once.
#[allow(clippy::too_many_arguments)]
async fn sync_room_native(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    room: Vec<tpf3mp_proto::NativeMod>,
    connected: &mut Option<Connected>,
    session: &Option<Session>,
    game: &mut Option<tpf3mp_launch::Started>,
    idle: &mut Idle,
    rejoin: &mut Option<(Invite, Option<Text<64>>)>,
) -> Result<(), String> {
    let next = get_room_native(shared, config, room, connected, session, false).await?;
    if game_runs(game) && shared.view().native_terms != next {
        let again = shared
            .last_join
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or("start the game again to run the room's native mods")?;
        shared
            .status()
            .notice("starting Transport Fever 3 again with the room's native mods; it joins the room again by itself");
        *rejoin = Some(again);
        leave(session)?;
        close_game(shared, game, idle).await;
        return Ok(());
    }
    if shared.set_native(next) {
        redeclare(shared, config, connected, session).await?;
    }
    Ok(())
}

/// Bootstrap waits for the agent's Hello before signalling readiness. Keep
/// answering that handshake while the blocking Windows launch waits for it.
/// Once greeted, leave menu actions queued for the normal launcher loop.
async fn wait_for_launch<T: Send + 'static>(
    mut launch: JoinHandle<T>,
    idle: &mut Idle,
    view: &LobbyView,
) -> Result<T, String> {
    let mut tick = tokio::time::interval(LOBBY_TICK);
    loop {
        tokio::select! {
            biased;
            result = &mut launch => return result.map_err(|error| error.to_string()),
            _ = tick.tick() => {
                if let Some(link) = idle {
                    link.link().heartbeat();
                    if link.build().is_none() {
                        // Only read through Hello: a fast-starting game's first
                        // UI actions must remain for the normal event loop.
                        if let Err(error) = link.greet(view) {
                            warn!(%error, "greeting the game's hook during startup failed");
                        }
                    }
                }
            }
        }
    }
}

/// The save a room this player creates starts from: the one `picked` names,
/// which must be one of the player's `listed` saves (a name, never a path:
/// the game's window names it), none when `picked` is empty, and the
/// launcher's own `default` when nothing was picked.
fn start_world(
    picked: Option<&str>,
    listed: &[String],
    default: Option<&PathBuf>,
    find: impl Fn(&str) -> Result<PathBuf, String>,
) -> Result<Option<PathBuf>, String> {
    let Some(picked) = picked.map(str::trim) else {
        return Ok(default.cloned());
    };
    if picked.is_empty() {
        return Ok(None);
    }
    if !listed.iter().any(|name| name == picked) {
        return Err(format!("there is no save {picked} in your save folder"));
    }
    find(picked).map(Some)
}

/// Whether the room's list made of the save `file` would run TPF3-MP's mod
/// whatever the save lists: with the picker, a save whose mods read and fit
/// a room's list ([`crate::picker::Mods::own_start`]) is loaded by every
/// game with the room's list, which always runs it. Otherwise each game
/// loads the save's own mods, and [`check_start_save`] decides.
fn room_list_runs_own_mod(picker: Option<&crate::picker::Mods>, file: Option<&Path>) -> bool {
    let Some(Ok(listed)) = file.map(tpf3mp_modscan::save::mods) else {
        return false;
    };
    let Some(picker) = picker else {
        return false;
    };
    let mut trial = picker.clone();
    trial.own_start(&listed).is_ok()
}

/// Refuses a save to start a room from that does not run TPF3-MP's mod:
/// every game would load the room's world without the mod's game script,
/// and hold it paused for good. A save whose mods do not read is not
/// refused here: the room's mods then follow no save (`own_start` says so),
/// and the world is checked again as it arrives (`Bridge`).
fn check_start_save(file: Option<&Path>) -> Result<(), String> {
    let Some(file) = file else {
        return Ok(());
    };
    match crate::save_check::runs_own_mod(file) {
        Ok(true) => Ok(()),
        Ok(false) => {
            warn!(save = %file.display(), "the save picked to start a room from does not run TPF3-MP's mod");
            Err(crate::save_check::SAVE_WITHOUT_OWN_MOD.to_owned())
        }
        Err(why) => {
            warn!(%why, "cannot tell whether the start save runs TPF3-MP's mod");
            Ok(())
        }
    }
}

/// A save's name, as the game's save list shows it: its file name without
/// `.sav`.
fn save_name(file: &Path) -> Option<String> {
    file.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_owned)
}

/// What the room shows everyone of the save `file`: its name, and the map
/// and year the owner's game read of it.
fn start_save_named(file: &Path, map: &str, year: u16) -> StartSave {
    StartSave {
        name: Text::lossy(&save_name(file).unwrap_or_default()),
        map: Text::lossy(map.trim()),
        year,
    }
}

/// The owner picks another save for the room to start from, in its lobby,
/// or none (an empty `picked`): checked as a new room's is, the room's
/// shared mods follow it, and the room session hands it over in place of
/// the one before, asking everyone to get ready again.
/// The room's owner, in its lobby, picked the room's mods in the game's mod
/// selector (docs/MODS.md, "The room's mods"): the picker takes them, with
/// the settings of the room's mods, and the room session declares them,
/// asking everyone to get ready again. Refused, changing nothing, for a mod
/// not installed here or a list that does not hold together.
///
/// With `start` (a save, with the map and year the owner's game read of it)
/// the room starts from that save too: both go to the room as one change.
async fn choose_room_mods(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    session: &Option<Session>,
    start: Option<(&str, &str, u16)>,
    mods: &[api::SelectedMod],
    params: &[api::ModSetting],
) -> Result<(), String> {
    if session.is_none() {
        return Err("join a room first".into());
    }
    {
        let status = shared.status();
        let room = status.room.as_ref().ok_or("join a room first")?;
        if room.owner != config.identity.player() {
            return Err("only the room's owner chooses the room's mods".into());
        }
        if room.phase != RoomPhase::Lobby {
            return Err("the room's game has begun: it plays the mods it has".into());
        }
    }
    let selection: Vec<crate::picker::Selected> = mods
        .iter()
        .map(|m| crate::picker::Selected {
            id: m.id.clone(),
            info: tpf3mp_proto::ModInfo {
                name: Text::lossy(if m.name.is_empty() { &m.id } else { &m.name }),
                source: Text::lossy(&m.source),
                modio: m.modio.filter(|_| m.source == tpf3mp_proto::MODIO_SOURCE),
            },
        })
        .collect();
    let settings = mod_settings(params)?;
    // The save, found before anything changes: one that cannot be had
    // leaves the room as it was.
    let start = match start {
        Some((picked, map, year)) => {
            let listed = shared.view().saves.clone();
            // A save without TPF3-MP is no hindrance here: the room's
            // list always runs it, and every game loads the room's list.
            let file = start_world(Some(picked), &listed, None, crate::steam::find_save)?
                .ok_or("that save is not there")?;
            Some((picked.trim().to_owned(), file, map.to_owned(), year))
        }
        None => None,
    };
    let (declaration, chosen) = {
        let mut picker = shared
            .picker()
            .ok_or("this launcher takes its mods from --mods")?;
        picker.choose_room(&selection, settings)?;
        (picker.declaration()?, picker.chosen())
    };
    shared.show_mods();
    remember_mods(config, chosen);
    info!(
        mods = declaration.manifest().mods.len(),
        save = start.is_some(),
        "the owner picks the room's mods"
    );
    match start {
        Some((picked, file, map, year)) => {
            // Offered first next time.
            shared.view().start_save = Some(picked);
            let save = start_save_named(&file, &map, year);
            forward(
                session,
                Control::StartWorld {
                    start: Some((file, save)),
                    declare: Some(declaration),
                },
            )
            .await
        }
        None => forward(session, Control::Declare(declaration)).await,
    }
}

/// The selector's settings, by mod. Refused when one does not fit what a
/// room carries.
fn mod_settings(params: &[api::ModSetting]) -> Result<Vec<tpf3mp_proto::ModParams>, String> {
    let mut out: Vec<tpf3mp_proto::ModParams> = Vec::new();
    for setting in params {
        let id = Text::new(setting.id.as_str()).map_err(|_| format!("no mod {}", setting.id))?;
        let key = Text::new(setting.key.as_str())
            .map_err(|_| format!("the setting {} of {} is too long", setting.key, setting.id))?;
        let param = tpf3mp_proto::ModParam {
            key,
            value: setting.value,
        };
        match out.iter_mut().find(|of| of.id == id) {
            Some(of) => of.params.push(param),
            None => out.push(tpf3mp_proto::ModParams {
                id,
                params: vec![param],
            }),
        }
    }
    Ok(out)
}

/// Remembers the personal mods chosen, for next time.
fn remember_mods(config: &LauncherConfig, chosen: Vec<String>) {
    if let Some(file) = &config.remember {
        let mut remembered = Remembered::load(file);
        remembered.mods = Some(chosen);
        if let Err(error) = remembered.save(file) {
            warn!(%error, "cannot remember the mods chosen for next time");
        }
    }
}

/// Finds the installed mods again, as after installing one from Mod Hub:
/// what this player declares follows, at once, to the room or the server.
async fn rescan_mods(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    connected: &mut Option<Connected>,
    session: &Option<Session>,
) -> Result<(), String> {
    if shared.picker.is_none() {
        return Err("this launcher takes its mods from --mods".into());
    }
    let game = config.installed.as_ref().map(|game| game.dir.clone());
    let found = tokio::task::spawn_blocking(move || {
        crate::picker::discover(game.as_deref(), &crate::steam::steam_roots())
    })
    .await
    .map_err(|error| format!("finding the mods failed: {error}"))?;
    let changed = shared.picker().is_some_and(|mut mods| mods.rescan(found));
    shared.show_mods();
    if !changed {
        info!("the installed mods are as they were");
        return Ok(());
    }
    info!("the installed mods changed: declaring anew");
    redeclare(shared, config, connected, session).await
}

/// Whether `picked` names the save `room` already starts from: then only
/// what the room shows of it changes, never the room's mods.
fn names_start_again(room: &tpf3mp_proto::RoomView, picked: &str) -> bool {
    let picked = picked.trim();
    !picked.is_empty()
        && room
            .start
            .as_ref()
            .is_some_and(|start| start.save.name.as_str() == picked)
}

/// The save the launcher offers first after the owner named `picked` for a
/// room: that save, next time too; none for a new world, which the room's
/// owner then sets up (the game's Multiplayer window offers Set up world
/// instead of Ready while it names none).
fn offered_start(picked: &str) -> Option<String> {
    let picked = picked.trim();
    (!picked.is_empty()).then(|| picked.to_owned())
}

async fn choose_start(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    session: &Option<Session>,
    picked: &str,
    map: &str,
    year: u16,
) -> Result<(), String> {
    if session.is_none() {
        return Err("join a room first".into());
    }
    // The save the room already starts from, named again: only what the
    // room shows of it changes (the map and year the owner's game read once
    // the room was made). The room's mods stay as the owner picked them.
    let described = {
        let status = shared.status();
        let room = status.room.as_ref().ok_or("join a room first")?;
        if room.owner != config.identity.player() {
            return Err("only the room's owner chooses the save it starts from".into());
        }
        if room.phase != RoomPhase::Lobby {
            return Err("the room's game has begun: it plays the world it has".into());
        }
        names_start_again(room, picked)
    };
    let listed = shared.view().saves.clone();
    let file = start_world(Some(picked), &listed, None, crate::steam::find_save)?;
    let declare = if described {
        None
    } else {
        if !room_list_runs_own_mod(shared.picker().as_deref(), file.as_deref()) {
            check_start_save(file.as_deref())?;
        }
        own_start(shared, file.as_deref())
    };
    let picked = picked.trim();
    shared.view().start_save = offered_start(picked);
    info!(
        none = picked.is_empty(),
        "the owner picks the save the room starts from"
    );
    let start = file.map(|file| {
        let save = start_save_named(&file, map, year);
        (file, save)
    });
    forward(session, Control::StartWorld { start, declare }).await
}

/// Hands the connection to a bridge, which runs the room from its lobby to
/// the end of its game, and plays it through the game's hook. A room this
/// player owns starts from `start_world`, if it names a save.
#[allow(clippy::too_many_arguments)]
fn begin_session(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    connected: &mut Option<Connected>,
    session: &mut Option<Session>,
    idle: &mut Idle,
    invite: Invite,
    password: Option<Text<64>>,
    start_world: Option<(PathBuf, StartSave)>,
    generate_world: bool,
) -> Result<(), String> {
    let Connected {
        client,
        events,
        options,
    } = connected.take().ok_or("not connected")?;
    // The game's link, greeted already if its hook said hello: the session
    // takes it over and gives it back when it ends.
    let (link, build) = match idle.take() {
        Some(link) => link.into_parts(),
        None => open_link(&config.link)?.into_parts(),
    };
    let (controls, controls_rx) = mpsc::channel(ACTION_QUEUE);
    // A fresh status for the new session, with the room already known.
    let owned = {
        let mut status = shared.status();
        let room = status.room.take();
        let owned = room
            .as_ref()
            .is_some_and(|room| room.owner == config.identity.player());
        *status = Status {
            room,
            ..Status::default()
        };
        owned
    };
    let bridge_options = BridgeOptions {
        scan_plan: Some(
            tpf3mp_proto::ScanPlan::from_environment_overrides(&config.game_env)
                .map_err(str::to_owned)?,
        ),
        worlds: Some(config.worlds.clone()),
        status: Some(Arc::clone(&shared.status)),
        lobby: Some(shared.lobby.clone()),
        // A room this player created starts from the save named for it.
        start_save: start_world
            .as_ref()
            .filter(|_| owned)
            .map(|(_, save)| save.clone()),
        start_world: start_world.filter(|_| owned).map(|(file, _)| file),
        start_generated_world: owned && generate_world,
        mods: config.mods.clone(),
        picker: shared.picker_link(),
        ..BridgeOptions::default()
    };
    let rejoin = Rejoin {
        options: options.clone(),
        invite,
        password,
        content: Some(shared.content(config)),
        give_up_after: REJOIN_PATIENCE,
    };
    let task = tokio::spawn(async move {
        let mut bridge = Bridge::new(link, bridge_options).with_controls(controls_rx);
        if let Some(build) = &build {
            bridge = bridge.greeted(build);
        }
        let ended = bridge::play(&mut bridge, client, events, &rejoin).await;
        let (link, build) = bridge.into_link();
        (ended, link, build)
    });
    {
        let mut view = shared.view();
        view.invite = Some(invite_for_copy(
            &invite,
            view.server.as_deref(),
            config.server_fixed || config.servers.len() >= 2,
        ));
        view.in_room = true;
        view.error = None;
    }
    *session = Some(Session {
        controls,
        task,
        options,
        game_closed: false,
    });
    Ok(())
}

/// Fixed-server and regional launchers copy a bare invite code. A dynamic
/// one-server playtest names its server so another launcher can connect too.
fn invite_for_copy(invite: &Invite, server: Option<&str>, code_only: bool) -> String {
    if code_only {
        invite.to_string()
    } else if let Some(server) = server.filter(|server| !server.is_empty()) {
        format!("{server} {invite}")
    } else {
        invite.to_string()
    }
}

async fn forward(session: &Option<Session>, control: Control) -> Result<(), String> {
    let session = session.as_ref().ok_or("join a room first")?;
    session
        .controls
        .send(control)
        .await
        .map_err(|_| "the room session has ended".to_owned())
}

/// Connects again after a room session, which took the old connection.
/// What the game runs goes with this connection too, as with the first
/// (`connect_to`): without it the server knows no content for the player,
/// and every room made on it refuses to start.
async fn reconnect(
    shared: &Arc<Shared>,
    options: ConnectOptions,
    content: Declaration,
) -> Option<Connected> {
    let connected = match connect(options.clone()).await {
        Ok((client, events)) => match client.declare(content).await {
            Ok(()) => Ok((client, events)),
            Err(error) => Err(error.to_string()),
        },
        Err(error) => Err(error.for_player()),
    };
    match connected {
        Ok((client, events)) => {
            let mut view = shared.view();
            view.connected = true;
            view.tunneled = client.tunneled();
            drop(view);
            Some(Connected {
                client,
                events,
                options,
            })
        }
        Err(error) => {
            warn!(%error, "cannot reconnect after the session");
            let mut view = shared.view();
            view.connected = false;
            view.error = Some(error);
            None
        }
    }
}

/// Connects to `server` as `name`, replacing any connection.
async fn connect_to(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    connected: &mut Option<Connected>,
    server: &str,
    name: Text<32>,
) -> Result<(), String> {
    let options = connect_options(config, server, name).await?;
    let previous_server = connected
        .as_ref()
        .and_then(|_| shared.view().server.clone());
    let on_list = shared.on_list();
    if servers::find(&config.servers, server).is_some() {
        // A target lookout already uses one per-address session. Stop it
        // fully before opening the primary connection, or the server's
        // default eight-session cap can reject the promotion.
        shared.lookouts.stop_at(server).await;
    }
    let previous = connected.take();
    {
        let mut view = shared.view();
        view.connecting = true;
        view.server = Some(server.to_owned());
    }
    let result = match connect(options.clone()).await {
        // What the game runs goes with every connection, so rooms can
        // compare it and say how it differs.
        Ok((client, events)) => match client.declare(shared.content(config)).await {
            Ok(()) => {
                // The native mods this server offers, for the panel and for
                // its rooms.
                let proposed = shared.view().native_proposed;
                fetch_server_native(shared, &client, server, proposed).await;
                refresh_native(shared, config, true).await;
                Ok((client, events))
            }
            Err(error) => Err(error.to_string()),
        },
        Err(error) => {
            shared.view().outdated = error.client_is_older();
            if let Some((client, server)) = error.mismatch() {
                warn!(client, server, "the server speaks another protocol");
            }
            Err(error.for_player())
        }
    };
    let (client, events) = match result {
        Ok(connected) => connected,
        Err(error) => {
            *connected = previous;
            {
                let mut view = shared.view();
                view.connecting = false;
                view.connected = connected.is_some();
                view.server.clone_from(&previous_server);
            }
            if on_list {
                let quiet = quiet_as(config, &options.name);
                shared
                    .lookouts
                    .watch(&config.servers, previous_server.as_deref(), &quiet);
            }
            return Err(error);
        }
    };
    {
        let mut view = shared.view();
        view.connecting = false;
        view.connected = true;
        view.outdated = false;
        view.tunneled = client.tunneled();
        view.error = None;
        view.name = options.name.as_str().to_owned();
        view.server_version = Some(client.welcome().server_version.as_str().to_owned());
        view.session = Some(client.welcome().session_id.to_string());
        view.rules = client.welcome().rules.clone();
    }
    if let Some(file) = &config.remember {
        let mut remembered = Remembered::load(file);
        remembered.server = Some(server.to_owned());
        remembered.name = Some(options.name.as_str().to_owned());
        if let Err(error) = remembered.save(file) {
            warn!(%error, "cannot remember the server and name for next time");
        }
    }
    // The other listed servers, watched from here on.
    drop(previous);
    shared.watch_others(config, server, &options.name).await;
    *connected = Some(Connected {
        options: options.again_after(&client),
        client,
        events,
    });
    Ok(())
}

/// How lookouts connect to the other listed servers: as `name`.
fn quiet_as(config: &LauncherConfig, name: &Text<32>) -> servers::Quiet {
    servers::Quiet {
        trust: config.trust.clone(),
        identity: Arc::clone(&config.identity),
        tunnel: config.tunnel.clone(),
        name: name.clone(),
    }
}

/// Connects to the closest of the release's servers as `name`: every one
/// is looked at, the lowest ping wins ([`servers::fastest`]), the default
/// when none answers, which then fails as connecting always did.
async fn connect_closest(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    connected: &mut Option<Connected>,
    name: Text<32>,
) -> Result<(), String> {
    shared.view().connecting = true;
    shared
        .lookouts
        .watch(&config.servers, None, &quiet_as(config, &name));
    let pings = shared.lookouts.pings(&config.servers, None).await;
    let best = servers::fastest(&pings, None).unwrap_or(0);
    info!(
        server = %config.servers[best].name,
        pings = ?pings.iter().map(|ping| ping.map(servers::millis)).collect::<Vec<_>>(),
        "playing on the closest listed server"
    );
    let result = connect_to(
        shared,
        config,
        connected,
        &config.servers[best].address,
        name,
    )
    .await;
    shared.view().connecting = false;
    result
}

/// Builds page `page` of the globally merged public-room list. Each server's
/// first `page + 1` local pages may contribute to it, so earlier results are
/// cached while the player pages through the combined list.
async fn global_room_page(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    current: &Connected,
    home: &servers::ListedServer,
    page: u16,
) -> Result<api::RoomList, String> {
    let lookouts = shared
        .lookouts
        .all_up(&config.servers, &home.address)
        .await
        .ok_or_else(|| {
            "the public room list is incomplete because a listed server is unavailable or still connecting; retry when it is reachable"
                .to_owned()
        })?;
    {
        let mut cache = shared
            .room_pages
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if page == 0
            || cache
                .home
                .as_deref()
                .is_none_or(|cached| !same_server(cached, &home.address))
        {
            *cache = RoomPageCache {
                home: Some(home.address.clone()),
                pages: HashMap::new(),
            };
        }
    }

    let mut sources = vec![(
        home.clone(),
        Some(current.client.rtt()),
        current.client.requests(),
        true,
    )];
    sources.extend(
        lookouts
            .into_iter()
            .map(|(server, requests, ping)| (server, Some(ping), requests, false)),
    );

    let mut fetched: Vec<(servers::ListedServer, Option<Duration>, u16, RoomPage)> = Vec::new();
    for (server, ping, requests, is_home) in sources {
        let mut local_pages = Vec::new();
        for page_index in 0..=page {
            if local_pages
                .last()
                .is_some_and(|previous: &RoomPage| !previous.more)
            {
                break;
            }
            let key = (server.address.clone(), page_index);
            let cached = shared
                .room_pages
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pages
                .get(&key)
                .cloned();
            let listed = match cached {
                Some(listed) => listed,
                None => match servers::list(&requests, page_index).await {
                    Some(listed) => {
                        shared
                            .room_pages
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .pages
                            .insert(key, listed.clone());
                        listed
                    }
                    None if is_home => {
                        return Err("the server did not return its room list".into());
                    }
                    None => {
                        return Err(
                            "the public room list became incomplete because a listed server stopped answering; retry in a moment"
                                .into(),
                        );
                    }
                },
            };
            let has_more = listed.more;
            local_pages.push(listed);
            if !has_more {
                break;
            }
        }
        fetched.extend(
            local_pages
                .into_iter()
                .enumerate()
                .map(|(page_index, listed)| {
                    (
                        server.clone(),
                        ping,
                        u16::try_from(page_index).unwrap_or(u16::MAX),
                        listed,
                    )
                }),
        );
    }

    let merging: Vec<servers::Page<'_>> = fetched
        .iter()
        .map(|(server, ping, page_index, listed)| servers::Page {
            server,
            ping: *ping,
            page_index: *page_index,
            page: listed,
        })
        .collect();
    Ok(servers::merge(page, &merging))
}

/// Before a room is created on the release's servers: moves to the closest
/// of them, if that is not the one played on. Pings within
/// [`servers::TIE_MARGIN`] keep the one played on. Should the closest not
/// take the connection, the player is back where they were.
async fn move_to_closest(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    connected: &mut Option<Connected>,
) -> Result<(), String> {
    let Some(current) = connected.as_ref() else {
        return Err("connect to a server first".into());
    };
    let here = shared.view().server.clone().unwrap_or_default();
    let name = current.options.name.clone();
    let ping = current.client.rtt();
    // Lookouts run, as the player is connected; started if they did not.
    shared.watch_others(config, &here, &name).await;
    let pings = shared
        .lookouts
        .pings(&config.servers, Some((&here, ping)))
        .await;
    let at = config
        .servers
        .iter()
        .position(|server| same_server(&server.address, &here));
    let Some(best) = servers::fastest(&pings, at) else {
        return Ok(());
    };
    if Some(best) == at {
        return Ok(());
    }
    let closest = &config.servers[best];
    info!(
        server = %closest.name,
        pings = ?pings.iter().map(|ping| ping.map(servers::millis)).collect::<Vec<_>>(),
        "the room goes to the closest listed server"
    );
    if let Err(error) = connect_to(shared, config, connected, &closest.address, name.clone()).await
    {
        warn!(%error, server = %closest.name, "the closest listed server would not take the room");
        connect_to(shared, config, connected, &here, name).await?;
    }
    Ok(())
}

/// Joins the room of `invite` on the release's servers. An explicit card or
/// region choice names one trusted target; otherwise every region must
/// answer the credential-aware lookup and exactly one must match before
/// the launcher switches or sends JoinRoom.
#[allow(clippy::too_many_arguments)]
async fn join_on_list(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    connected: &mut Option<Connected>,
    session: &mut Option<Session>,
    idle: &mut Idle,
    invite: Invite,
    password: Option<Text<64>>,
    selected: Option<servers::ListedServer>,
    game: &mut Option<tpf3mp_launch::Started>,
) -> Result<(), String> {
    // A clicked public room or an explicit region selection is already a
    // trusted route. A bare code must pass the all-region credential check
    // before the launcher promotes a lookout or sends JoinRoom.
    let target = match selected {
        Some(server) => server,
        None => resolve_invite_target(shared, config, connected, invite, password.clone()).await?,
    };
    let current = connected.as_ref().ok_or("connect to a server first")?;
    let here = shared.view().server.clone();
    if !here.is_some_and(|here| same_server(&here, &target.address)) {
        let name = current.options.name.clone();
        connect_to(shared, config, connected, &target.address, name).await?;
    }
    renew_unused_link(&config.link, game, idle)?;
    // JoinRoom rechecks the code and password, as well as current capacity
    // and build eligibility. A change after ResolveInvite is a normal join
    // refusal; it must not trigger a fallback to another region.
    join(
        shared, config, connected, session, idle, game, invite, password,
    )
    .await
}

/// Asks every trusted region whether it can admit `invite` with `password`.
/// The response contains no room details. No region is joined, and the
/// primary connection is left alone, until exactly one positive answer has
/// arrived and every other answer is a generic no-match.
async fn resolve_invite_target(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    connected: &Option<Connected>,
    invite: Invite,
    password: Option<Text<64>>,
) -> Result<servers::ListedServer, String> {
    let current = connected.as_ref().ok_or("connect to a server first")?;
    let here = shared
        .view()
        .server
        .clone()
        .ok_or("connect to a listed server first")?;
    let home = servers::find(&config.servers, &here)
        .ok_or("the current server is not in this release's trusted list")?;
    let lookouts = shared
        .lookouts
        .all_up(&config.servers, &home.address)
        .await
        .ok_or_else(|| {
            "invite lookup is incomplete because a trusted region is unavailable; retry when every listed server is reachable"
                .to_owned()
        })?;

    let candidates = std::iter::once((home.clone(), current.client.requests())).chain(
        lookouts
            .into_iter()
            .map(|(server, requests, _ping)| (server, requests)),
    );
    let mut checks = JoinSet::new();
    for (server, requests) in candidates {
        let request = Request::ResolveInvite {
            invite,
            password: password.clone(),
        };
        checks.spawn(async move {
            (
                server,
                requests
                    .request_with_timeout(request, INVITE_RESOLVE_TIMEOUT)
                    .await,
            )
        });
    }

    let mut probes = Vec::with_capacity(config.servers.len());
    let mut incomplete = false;
    let mut rate_limited = false;
    while let Some(answer) = checks.join_next().await {
        match answer {
            Ok((server, response)) => {
                let (result, limited) = invite_probe(response);
                incomplete |= result == servers::InviteProbe::Incomplete;
                rate_limited |= limited;
                probes.push((server, result));
            }
            Err(error) => {
                warn!(%error, "a trusted region's invite lookup task failed");
                incomplete = true;
            }
        }
    }
    if incomplete {
        return Err(if rate_limited {
            "a trusted region is busy checking invites; wait briefly and retry".into()
        } else {
            "invite lookup is incomplete because a trusted region did not answer; check the connection and retry"
                .into()
        });
    }
    servers::select_invite_target(probes).map_err(invite_target_error)
}

fn invite_target_error(error: servers::InviteTargetError) -> String {
    match error {
        servers::InviteTargetError::NoMatch => {
            ClientError::Refused(RequestError::BadInvite).to_string()
        }
        servers::InviteTargetError::Ambiguous(matches) => {
            let regions = matches
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "more than one trusted region accepts that invite and password ({regions}); choose a region and retry"
            )
        }
        servers::InviteTargetError::Incomplete => {
            "invite lookup is incomplete because a trusted region did not answer; check the connection and retry"
                .into()
        }
    }
}

/// Which non-mutating probe result a server response represents. Only the
/// intentionally generic BadInvite is a negative match; every other error
/// leaves the cross-region answer incomplete.
fn invite_probe(response: Result<Response, ClientError>) -> (servers::InviteProbe, bool) {
    match response {
        Ok(Response::InviteMatch) => (servers::InviteProbe::Match, false),
        Err(ClientError::Refused(RequestError::BadInvite)) => {
            (servers::InviteProbe::NoMatch, false)
        }
        Err(ClientError::Refused(RequestError::RateLimited)) => {
            (servers::InviteProbe::Incomplete, true)
        }
        _ => (servers::InviteProbe::Incomplete, false),
    }
}

/// With the picker, the room this player creates starts from `save`: its
/// mods, less this player's personal ones, become the room's shared mods.
/// Returns what to declare, or `None` without the picker. A save whose mods
/// cannot be read leaves the room's unknown: its worlds load with the save's
/// own mods, as without the picker, and the player is told.
fn own_start(shared: &Shared, save: Option<&Path>) -> Option<Declaration> {
    let read = save.map(tpf3mp_modscan::save::mods);
    let mut mods = shared.picker()?;
    let mut problem = None;
    match read {
        Some(Ok(listed)) => {
            info!(
                mods = listed.len(),
                "the room's mods come from its start save"
            );
            // A list that cannot be the room's (too many mods) is still
            // compared whole; every game loads the save's own mods.
            if let Err(why) = mods.own_start(&listed) {
                warn!(%why, "the start save's mods cannot be the room's list");
                problem = Some(format!(
                    "the start save's mods cannot be the room's list ({why}): every game loads the save's own mods, still compared"
                ));
            }
        }
        Some(Err(why)) => {
            warn!(%why, "the start save's mods do not read");
            mods.forget_room();
            problem = Some(format!(
                "the start save's mods could not be read ({why}): everyone loads its own list of mods"
            ));
        }
        None => mods.forget_room(),
    }
    let declaration = mods
        .declaration()
        .unwrap_or_else(|_| Declaration::Content(mods.manifest()));
    drop(mods);
    if let Some(problem) = problem {
        shared.status().notice(problem);
    }
    shared.show_mods();
    Some(declaration)
}

/// The player's server setting (D12, as amended): play on `typed`, or on
/// the launcher's default when it is empty. Refused in a room, and for
/// anything but a `host:port`. Remembered for the next run; a connected
/// launcher leaves its server and connects to the new one, under the same
/// name.
async fn set_server(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    typed: &str,
    connected: &mut Option<Connected>,
    session: &Option<Session>,
) -> Result<(), String> {
    if session.is_some() {
        return Err("leave the room first: the server changes between rooms".into());
    }
    let default = config.default_server.clone();
    let chosen = server_setting_choice(config, typed)?;
    let server = chosen
        .clone()
        .or(default)
        .ok_or("this launcher has no default server: type one")?;
    if let Some(file) = &config.remember {
        let mut remembered = Remembered::load(file);
        remembered.chosen_server.clone_from(&chosen);
        if let Err(error) = remembered.save(file) {
            warn!(%error, "cannot remember the server for next time");
        }
    }
    let was_connected = connected.is_some();
    if let Some(connected) = connected.take() {
        connected.client.close().await;
    }
    // Back on the default, a release with several servers plays on all
    // of them again; a server of the player's own is played on alone.
    let on_list = chosen.is_none() && config.servers.len() >= 2;
    let name = {
        let mut view = shared.view();
        view.server = Some(server.clone());
        view.server_fixed = true;
        view.on_list = on_list;
        view.connected = false;
        view.rooms = None;
        view.server_version = None;
        view.session = None;
        view.name.clone()
    };
    shared.lookouts.stop().await;
    info!(%server, chosen = chosen.is_some(), on_list, "the player set the server");
    if was_connected {
        let name = Text::new(name.trim()).map_err(|_| "that name is too long".to_owned())?;
        if on_list {
            connect_closest(shared, config, connected, name).await?;
        } else {
            connect_to(shared, config, connected, &server, name).await?;
        }
    }
    Ok(())
}

/// Resolves a settings value before any remembered or connection state is
/// changed. A release with multiple trusted regions accepts only addresses
/// from that compiled list; a developer's `--server` build has no such list.
fn server_setting_choice(config: &LauncherConfig, typed: &str) -> Result<Option<String>, String> {
    let default = config.default_server.as_deref();
    let chosen = match typed.trim() {
        "" => None,
        typed => Some(server_address(typed)?)
            .filter(|chosen| !default.is_some_and(|server| same_server(server, chosen))),
    };
    if config.servers.len() < 2 {
        return Ok(chosen);
    }
    chosen
        .map(|address| {
            servers::find(&config.servers, &address)
                .map(|server| server.address.clone())
                .ok_or_else(|| "this release can use only its trusted servers".to_owned())
        })
        .transpose()
}

/// A server as the player typed it for the setting, as `host:port`: the
/// host a name or an IPv4 address, or an IPv6 address in brackets, and a
/// port from 1. Trimmed; or why it is not one.
pub fn server_address(typed: &str) -> Result<String, String> {
    const HOW: &str = "the server must be host:port, such as tpf3mp.example.org:29470";
    let typed = typed.trim();
    let (host, port) = typed.rsplit_once(':').ok_or(HOW)?;
    let port_ok = !port.starts_with('+') && port.parse::<u16>().is_ok_and(|port| port > 0);
    let host_ok = match host.strip_prefix('[') {
        Some(inner) => inner
            .strip_suffix(']')
            .is_some_and(|ip| ip.parse::<std::net::Ipv6Addr>().is_ok()),
        None => {
            host.len() <= 253
                && host.split('.').all(|label| {
                    !label.is_empty()
                        && label.len() <= 63
                        && !label.starts_with('-')
                        && !label.ends_with('-')
                        && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                })
        }
    };
    if typed.len() > 128 || !port_ok || !host_ok {
        return Err(HOW.into());
    }
    Ok(typed.to_owned())
}

/// What the launcher remembers between runs: the server and the name the
/// player last connected with, which the page then offers first, and the
/// player's settings.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remembered {
    /// The server last connected to.
    pub server: Option<String>,
    pub name: Option<String>,
    /// The player's server setting, when they chose one other than the
    /// default ([`Action::SetServer`]).
    #[serde(default)]
    pub chosen_server: Option<String>,
    /// Whether the player's diagnostics go to the server; on unless they
    /// switched them off.
    #[serde(default)]
    pub diagnostics: Option<bool>,
    /// The personal mods the player chose, by id (docs/MODS.md).
    #[serde(default)]
    pub mods: Option<Vec<String>>,
    /// The banner the player picked ([`Action::SetBanner`]).
    #[serde(default)]
    pub banner: Option<String>,
}

impl Remembered {
    /// What `file` holds, or nothing if it is missing or not ours.
    pub fn load(file: &Path) -> Self {
        let small = fs::metadata(file).is_ok_and(|metadata| metadata.len() <= 64 * 1024);
        small
            .then(|| fs::read(file).ok())
            .flatten()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self, file: &Path) -> std::io::Result<()> {
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(file, serde_json::to_vec_pretty(self)?)
    }
}

/// Joins the room of `invite` on the current connection.
#[allow(clippy::too_many_arguments)]
async fn join(
    shared: &Arc<Shared>,
    config: &LauncherConfig,
    connected: &mut Option<Connected>,
    session: &mut Option<Session>,
    idle: &mut Idle,
    game: &mut Option<tpf3mp_launch::Started>,
    invite: Invite,
    password: Option<Text<64>>,
) -> Result<(), String> {
    *shared
        .last_join
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some((invite, password.clone()));
    connected.as_ref().ok_or("connect to a server first")?;
    // Another's room: its shared mods are learned from what it says this
    // game lacks (docs/MODS.md). Until then, this game declares none.
    let fresh = shared.picker().map(|mut mods| {
        mods.forget_room();
        mods.manifest()
    });
    if let Some(manifest) = fresh {
        shared.show_mods();
        connected
            .as_mut()
            .ok_or("connect to a server first")?
            .client
            .declare_content(manifest)
            .await
            .map_err(|error| error.to_string())?;
    }
    let current = connected.as_mut().ok_or("connect to a server first")?;
    // The room's mods, as the room tells them, come as the room is joined:
    // in the lobby on joining, at a running game before its refusal.
    let mut joined = current
        .client
        .join_room(JoinRoom {
            scan_plan: tpf3mp_proto::ScanPlan::from_environment_overrides(&config.game_env)
                .map_err(str::to_owned)?,
            invite,
            password: password.clone(),
            resume: None,
        })
        .await;
    // A running game compares at once: take the room's mods, which come
    // before its refusal, else learn them from the refusal, and try once
    // more with those this game has.
    if matches!(
        joined,
        Err(ClientError::Refused(RequestError::ContentMismatch))
    ) && (shared.picker.is_some() || config.native.is_some())
    {
        let (room, diff) = refusal(&mut current.events).await;
        // The running game's native mods first: from the signed index, with
        // the room's settings, the game started again with them if it runs
        // others (docs/NATIVE_MODS.md, "In a room").
        let told_native = room.as_ref().map(|room| room.native.clone());
        let native_changed = match told_native {
            Some(native)
                if native.iter().map(|m| (&m.id, &m.version)).ne(shared
                    .view()
                    .native_terms
                    .iter()
                    .map(|m| (&m.id, &m.version))) =>
            {
                let next =
                    get_room_native(shared, config, native, connected, session, true).await?;
                if game_runs(game) && shared.view().native_terms != next {
                    close_game(shared, game, idle).await;
                    launch_game(shared, config, session, game, idle).await?;
                } else {
                    shared.set_native(next);
                }
                true
            }
            _ => false,
        };
        let current = connected.as_mut().ok_or("connect to a server first")?;
        let again = match shared.picker() {
            Some(mut mods) => {
                let changed = match &room {
                    // Refused a join: not this room's owner.
                    Some(room) => mods.adopt(Some(room), false),
                    None => diff.as_ref().is_some_and(|diff| mods.learn(diff)),
                };
                (changed || native_changed).then(|| mods.manifest())
            }
            None => native_changed.then(|| shared.content(config).manifest().clone()),
        };
        if let Some(room) = room {
            shared.status().room_mods = Some(room);
        }
        if diff.is_some() {
            shared.status().content_diff = diff;
        }
        shared.show_mods();
        if let Some(manifest) = again {
            current
                .client
                .declare_content(manifest)
                .await
                .map_err(|error| error.to_string())?;
            joined = current
                .client
                .join_room(JoinRoom {
                    scan_plan: tpf3mp_proto::ScanPlan::from_environment_overrides(&config.game_env)
                        .map_err(str::to_owned)?,
                    invite,
                    password: password.clone(),
                    resume: None,
                })
                .await;
        }
    }
    let room = match joined {
        Ok(room) => room,
        Err(ClientError::Refused(RequestError::ContentMismatch)) => {
            // The room says how, on its own message.
            let current = connected.as_mut().ok_or("connect to a server first")?;
            let diff = content_diff(&mut current.events).await;
            let message = match &diff {
                Some(diff) => format!("your game differs from the room's: {diff}"),
                None => RequestError::ContentMismatch.to_string(),
            };
            shared.status().content_diff = diff;
            return Err(message);
        }
        Err(error) => return Err(error.to_string()),
    };
    shared.status().room = Some(room);
    begin_session(
        shared,
        config,
        connected,
        session,
        idle,
        invite,
        password,
        config.start_save.clone().map(|file| {
            let save = start_save_named(&file, "", 0);
            (file, save)
        }),
        false,
    )
}

/// How the game differs from a room that refused it, if the room says so
/// within a second.
/// What a refused join to a running game told: the game's mods, then how
/// this game differs; each `None` when not told within a second.
async fn refusal(events: &mut Events) -> (Option<tpf3mp_proto::RoomMods>, Option<ContentDiff>) {
    let mut room = None;
    let diff = tokio::time::timeout(Duration::from_secs(1), async {
        while let Some(event) = events.recv().await {
            match event {
                ClientEvent::RoomMods(told) => room = told.map(|told| *told),
                ClientEvent::ContentDiff(diff) => return diff,
                _ => {}
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    (room, diff)
}

async fn content_diff(events: &mut Events) -> Option<ContentDiff> {
    tokio::time::timeout(Duration::from_secs(1), async {
        while let Some(event) = events.recv().await {
            if let ClientEvent::ContentDiff(diff) = event {
                return diff;
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// The server this launcher plays on, when it has one (D12): no invite
/// takes the player elsewhere.
fn fixed_server(shared: &Shared) -> Option<String> {
    let view = shared.view();
    view.server.clone().filter(|_| view.server_fixed)
}

/// The server to connect to for what the player gave to Connect: `typed`,
/// and the invite in it if any. With a `fixed` server, that one, and an
/// invite only to it; otherwise what was typed, or the invite's server.
fn server_for(fixed: Option<&str>, typed: &str, passed: Option<&Passed>) -> Result<String, String> {
    match (fixed, passed) {
        (
            Some(fixed),
            Some(Passed {
                server: Some(other),
                ..
            }),
        ) if !same_server(fixed, other) => Err(elsewhere(fixed, other)),
        (Some(fixed), None) if !typed.trim().is_empty() && !same_server(fixed, typed) => {
            Err("that is not an invite".into())
        }
        (Some(fixed), _) => Ok(fixed.to_owned()),
        (
            None,
            Some(Passed {
                server: Some(server),
                ..
            }),
        ) => Ok(server.clone()),
        (None, Some(Passed { server: None, .. })) => {
            Err("that is an invite: put the server's address before it".into())
        }
        (None, None) => Ok(typed.trim().to_owned()),
    }
}

/// Whether two `host:port`s name the same server, as players type them.
fn same_server(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Why an invite to `other` is refused by a launcher fixed to `fixed`.
fn elsewhere(fixed: &str, other: &str) -> String {
    format!(
        "that invite is for another server, {other}: you play on {fixed}. To play there, change the server in Settings"
    )
}

/// An invite as players pass it on: the room's invite, perhaps with the
/// server's address before it, as "Copy invite" gives it, inside whatever
/// message it came in.
#[derive(Debug, PartialEq, Eq)]
struct Passed {
    server: Option<String>,
    invite: Invite,
}

fn passed_invite(text: &str) -> Option<Passed> {
    let tokens: Vec<&str> = text
        .split_whitespace()
        .map(|token| {
            token.trim_matches(|c: char| {
                matches!(
                    c,
                    '"' | '\'' | '`' | '<' | '>' | '(' | ')' | ',' | ';' | '*'
                )
            })
        })
        .collect();
    let (at, invite) = tokens
        .iter()
        .enumerate()
        .find_map(|(at, token)| token.parse::<Invite>().ok().map(|invite| (at, invite)))?;
    let server = tokens[..at]
        .iter()
        .rev()
        .find(|token| names_a_server(token))
        .map(|token| (*token).to_owned());
    Some(Passed { server, invite })
}

/// Whether `token` reads as `host:port`, the host a name or an address, as
/// in `tpf3mp.example.org:29470` or `[2001:db8::1]:29470`: not a time of
/// day like `12:30`.
fn names_a_server(token: &str) -> bool {
    token.rsplit_once(':').is_some_and(|(host, port)| {
        port.parse::<u16>().is_ok_and(|port| port > 0)
            && host.contains(|c: char| c == '.' || c == '[' || c.is_ascii_alphabetic())
    })
}

/// How to reach `server` as `name`, trusted as `trust`, by the `tunnel`
/// choice: what every connection to a server shares.
pub(crate) async fn base_options(
    trust: &ServerTrust,
    identity: &Arc<Identity>,
    tunnel: &TunnelChoice,
    server: &str,
    name: Text<32>,
) -> Result<ConnectOptions, String> {
    let server = server.trim();
    let (host, _port) = server
        .rsplit_once(':')
        .ok_or("the server address must be host:port")?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let address = crate::resolve(server)
        .await
        .map_err(|error| format!("cannot find {server}: {error}"))?;
    let mut options = ConnectOptions::new(address, host, trust.clone(), Arc::clone(identity), name);
    options.route = tunnel.route(host).map_err(|error| error.to_string())?;
    Ok(options)
}

async fn connect_options(
    config: &LauncherConfig,
    server: &str,
    name: Text<32>,
) -> Result<ConnectOptions, String> {
    let mut options = base_options(
        &config.trust,
        &config.identity,
        &config.tunnel,
        server,
        name,
    )
    .await?;
    // Every connection sends the recorder's lines, the rejoins' too.
    options.diagnostics.clone_from(&config.diagnostics);
    // And shows the player's banner, the rejoins too.
    options.banner = config
        .remember
        .as_deref()
        .and_then(|file| Remembered::load(file).banner)
        .and_then(|banner| Text::new(banner).ok())
        .filter(|banner| tpf3mp_proto::is_banner(banner.as_str()));
    Ok(options)
}

fn password_text(password: Option<String>) -> Result<Option<Text<64>>, String> {
    password
        .filter(|password| !password.is_empty())
        .map(|password| Text::new(password).map_err(|_| "that password is too long".to_owned()))
        .transpose()
}

/// The end of the current session, or never without one: how it ended and
/// the link it gives back, or why its task failed.
async fn session_end(
    session: &mut Option<Session>,
) -> Result<SessionEnded, tokio::task::JoinError> {
    match session {
        Some(session) => (&mut session.task).await,
        None => std::future::pending().await,
    }
}

/// What the player is told, in both windows, when a session ended because
/// its room was lost for good: gone from the server, not rejoined in time,
/// or with a world this game cannot play. `None` for any other end.
fn room_lost(ended: &Result<BridgeEnd, bridge::BridgeFault>) -> Option<String> {
    match ended {
        Err(
            fault @ (bridge::BridgeFault::RoomGone
            | bridge::BridgeFault::Rejoin(_)
            | bridge::BridgeFault::WorldWithoutOwnMod),
        ) => Some(fault.to_string()),
        _ => None,
    }
}

/// Leaves the room, always: the session is asked to, and when it cannot
/// take the request (its queue full, or it is stuck), it is stopped here.
/// Either way it ends, and the launcher is back on the server in no room;
/// a seat the server was not told about is let go after the room's grace.
fn leave(session: &Option<Session>) -> Result<(), String> {
    let session = session.as_ref().ok_or("join a room first")?;
    leave_or_stop(&session.controls, &session.task);
    Ok(())
}

fn leave_or_stop<T>(controls: &mpsc::Sender<Control>, task: &JoinHandle<T>) {
    if let Err(error) = controls.try_send(Control::Leave) {
        warn!(%error, "the room session cannot take the leave; stopping it");
        task.abort();
    }
}

/// When the game started from here has exited, or never without one.
async fn game_exit(game: &mut Option<tpf3mp_launch::Started>) {
    match game {
        Some(started) => exited(|| started.is_running()).await,
        None => std::future::pending().await,
    }
}

/// Returns once `running` says no, asking every [`GAME_POLL`].
async fn exited(mut running: impl FnMut() -> bool) {
    while running() {
        tokio::time::sleep(GAME_POLL).await;
    }
}

/// The next event of a connection not in a room, or never without one.
async fn next_event(connected: &mut Option<Connected>) -> Option<ClientEvent> {
    match connected {
        Some(connected) => connected.events.recv().await,
        None => std::future::pending().await,
    }
}

fn describe(end: &BridgeEnd) -> String {
    match end {
        BridgeEnd::Closed(reason) => format!("the connection closed ({reason})"),
        BridgeEnd::Kicked => "the owner removed you from the room".into(),
        BridgeEnd::EventsEnded => "the connection ended".into(),
        BridgeEnd::WorldUnavailable => "the world could not be fetched".into(),
        BridgeEnd::Left => "you left the room".into(),
    }
}

fn random_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the operating system's random source is available");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_installs_keep_cached_server_indexes_but_not_unreviewed_proposals() {
        assert_eq!(
            cached_server_for_offline_install(Some("eu.example.org:29470"), false).as_deref(),
            Some("eu.example.org:29470")
        );
        assert_eq!(
            cached_server_for_offline_install(Some("eu.example.org:29470"), true),
            None
        );
        assert_eq!(cached_server_for_offline_install(None, false), None);
    }

    fn local_config(worlds_dir: &Path) -> LauncherConfig {
        let (identity, _) = Identity::generate().unwrap();
        LauncherConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            tunnel: crate::TunnelChoice::Off,
            remember: None,
            server: None,
            server_fixed: false,
            default_server: None,
            server_name: None,
            servers: Vec::new(),
            trust: ServerTrust::WebPki,
            identity: Arc::new(identity),
            name: "renderer recovery test".into(),
            content: ContentManifest::new(tpf3mp_proto::Text::new("test").unwrap(), Vec::new()),
            mods: None,
            picker: None,
            native: None,
            installed: None,
            link: "renderer-recovery-test".into(),
            worlds: crate::Worlds::open(worlds_dir, 1 << 20).unwrap(),
            room_settings: RoomSettings::DEFAULT,
            diagnostics: None,
            game_logs: None,
            hook: None,
            game_exe: None,
            game_env: Vec::new(),
            start_save: None,
        }
    }

    #[tokio::test]
    async fn a_regional_server_setting_refuses_unlisted_hosts_before_side_effects() {
        let root = tempfile::tempdir().unwrap();
        let remember = root.path().join("launcher.json");
        Remembered::default().save(&remember).unwrap();
        let eu = "eu.example.org:29470";
        let us = "us.example.org:29470";
        let mut config = local_config(root.path());
        config.remember = Some(remember.clone());
        config.server = Some(eu.into());
        config.server_fixed = true;
        config.default_server = Some(eu.into());
        config.server_name = Some("EU".into());
        config.servers = vec![
            ListedServer {
                name: "EU".into(),
                address: eu.into(),
            },
            ListedServer {
                name: "US".into(),
                address: us.into(),
            },
        ];
        let (shared, _, _) = Shared::new(&config);
        let mut connected = None;
        let session = None;
        let error = set_server(
            &shared,
            &config,
            "outside.example.org:29470",
            &mut connected,
            &session,
        )
        .await
        .unwrap_err();
        assert!(error.contains("trusted servers"), "{error}");
        assert!(connected.is_none(), "no connection was promoted or closed");
        assert_eq!(shared.view().server.as_deref(), Some(eu));
        assert!(shared.view().on_list, "the regional route is unchanged");
        assert_eq!(
            Remembered::load(&remember).chosen_server,
            None,
            "the refused choice is not remembered"
        );

        set_server(&shared, &config, us, &mut connected, &session)
            .await
            .unwrap();
        assert_eq!(shared.view().server.as_deref(), Some(us));
        assert!(!shared.view().on_list, "a listed region may be pinned");
        assert_eq!(
            Remembered::load(&remember).chosen_server.as_deref(),
            Some(us),
            "trusted listed choices remain supported by the backend"
        );
    }

    #[tokio::test]
    async fn a_single_server_development_launcher_keeps_its_override() {
        let root = tempfile::tempdir().unwrap();
        let remember = root.path().join("launcher.json");
        let mut config = local_config(root.path());
        config.remember = Some(remember);
        config.server = Some("127.0.0.1:29470".into());
        config.server_fixed = true;
        config.default_server = Some("eu.example.org:29470".into());
        let (shared, _, _) = Shared::new(&config);
        let mut connected = None;
        let session = None;
        set_server(
            &shared,
            &config,
            "other.example.org:29470",
            &mut connected,
            &session,
        )
        .await
        .unwrap();
        assert_eq!(
            shared.view().server.as_deref(),
            Some("other.example.org:29470")
        );
    }

    #[test]
    fn copied_invites_name_a_server_only_for_dynamic_single_server_playtests() {
        let invite = Invite("K7QM2X".parse().unwrap());
        assert_eq!(
            invite_for_copy(&invite, Some("play.example:29470"), true),
            "K7QM2X"
        );
        assert_eq!(
            invite_for_copy(&invite, Some("play.example:29470"), false),
            "play.example:29470 K7QM2X"
        );
    }

    #[tokio::test]
    async fn a_local_page_reuses_the_live_controller_and_only_attaches_once() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let worlds = tempfile::tempdir().unwrap();
        let mut launcher = Launcher::start_local(local_config(worlds.path()));
        let original_handle = launcher.handle();
        let original_shared = Arc::clone(&launcher.shared);
        let original_control_task = launcher.task.id();
        let banner = tpf3mp_proto::BANNERS[0].to_owned();
        original_handle
            .act(Action::SetBanner {
                banner: Some(banner.clone()),
            })
            .await
            .unwrap();

        launcher
            .serve_local("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();

        assert!(Arc::ptr_eq(&launcher.shared, &original_shared));
        assert!(Arc::ptr_eq(&launcher.shared, &original_handle.shared));
        assert_eq!(launcher.task.id(), original_control_task);
        let page_task = launcher.page_task.as_ref().unwrap().id();
        let url = launcher.url().unwrap().to_owned();
        let (address_and_path, token) = url.split_once('#').unwrap();
        let address = address_and_path
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap();
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream
            .write_all(
                format!(
                    "GET /api/state HTTP/1.1\r\nHost: {address}\r\nx-launcher-token: {token}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(headers.starts_with("HTTP/1.1 200 OK"));
        let page_state: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(page_state["banner"], banner);

        let error = launcher
            .serve_local("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(launcher.url(), Some(url.as_str()));
        assert_eq!(launcher.page_task.as_ref().unwrap().id(), page_task);
        assert_eq!(
            original_handle.state().banner.as_deref(),
            Some(banner.as_str())
        );
    }

    fn invite() -> Invite {
        Invite("K7QM2X".parse().unwrap())
    }

    #[test]
    fn invite_lookup_counts_only_generic_bad_invite_as_a_negative_answer() {
        assert_eq!(
            invite_probe(Ok(Response::InviteMatch)),
            (servers::InviteProbe::Match, false)
        );
        assert_eq!(
            invite_probe(Err(ClientError::Refused(RequestError::BadInvite))),
            (servers::InviteProbe::NoMatch, false)
        );
        assert_eq!(
            invite_probe(Err(ClientError::Refused(RequestError::RateLimited))),
            (servers::InviteProbe::Incomplete, true)
        );
        assert_eq!(
            invite_probe(Err(ClientError::Timeout)),
            (servers::InviteProbe::Incomplete, false)
        );
    }

    #[test]
    fn an_ambiguous_invite_tells_the_player_which_trusted_regions_to_choose() {
        let message = invite_target_error(servers::InviteTargetError::Ambiguous(vec![
            servers::ListedServer {
                name: "EU".into(),
                address: "eu.example.org:29470".into(),
            },
            servers::ListedServer {
                name: "US".into(),
                address: "us.example.org:29470".into(),
            },
        ]));
        assert!(
            message.contains("EU") && message.contains("US"),
            "{message}"
        );
        assert!(message.contains("choose a region and retry"), "{message}");
    }

    #[tokio::test]
    async fn leave_asks_the_session_and_stops_one_that_cannot_take_it() {
        // A session that takes requests is asked.
        let (controls, mut asked) = mpsc::channel(1);
        let task = tokio::spawn(std::future::pending::<()>());
        leave_or_stop(&controls, &task);
        assert_eq!(asked.recv().await, Some(Control::Leave));
        assert!(!task.is_finished(), "left by the session itself");
        task.abort();

        // One whose requests back up (stuck, or rejoining in an older
        // build) is stopped: leaving never waits on it.
        let (controls, _backed_up) = mpsc::channel(1);
        controls
            .try_send(Control::Chat(Text::new("hi").unwrap()))
            .unwrap();
        let task = tokio::spawn(std::future::pending::<()>());
        leave_or_stop(&controls, &task);
        let stopped = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("stopped at once");
        assert!(stopped.unwrap_err().is_cancelled());
    }

    #[test]
    fn a_room_lost_for_good_is_said_as_it_is_and_nothing_else_is() {
        assert_eq!(
            room_lost(&Err(bridge::BridgeFault::RoomGone)).as_deref(),
            Some("The room is gone (closed or the server restarted)")
        );
        let gave_up = room_lost(&Err(bridge::BridgeFault::Rejoin("timed out".into()))).unwrap();
        assert!(gave_up.contains("could not rejoin") && gave_up.contains("may be gone"));
        assert_eq!(room_lost(&Ok(BridgeEnd::Left)), None);
        assert_eq!(room_lost(&Err(bridge::BridgeFault::GameClosed)), None);
        assert_eq!(
            room_lost(&Err(bridge::BridgeFault::WorldWithoutOwnMod)).as_deref(),
            Some(crate::save_check::WORLD_WITHOUT_OWN_MOD)
        );
    }

    /// Seen live: a host started a room from a save without TPF3-MP's mod,
    /// and both games loaded its world and held it paused, without a word.
    #[test]
    fn a_start_save_without_tpf3mps_mod_is_refused_saying_how_to_fix_it() {
        let dir = tempfile::tempdir().unwrap();
        let without = dir.path().join("without.sav");
        crate::save_check::saves::write(
            &without,
            &["urbangames_deluxe_upgrade_pack", "urbangames_preorder_pack"],
        );
        assert_eq!(
            check_start_save(Some(&without)),
            Err(
                "This save doesn't have the TPF3-MP mod enabled: load it once, turn TPF3-MP on \
                 in its mods, save it, then pick it again"
                    .to_owned()
            )
        );
        let with = dir.path().join("with.sav");
        crate::save_check::saves::write(&with, &["tpf3mp_1", "urbangames_preorder_pack"]);
        assert_eq!(check_start_save(Some(&with)), Ok(()));
        // No save (a generated world), or one whose mods do not read: not
        // refused here.
        assert_eq!(check_start_save(None), Ok(()));
        let junk = dir.path().join("junk.sav");
        std::fs::write(&junk, b"not a save").unwrap();
        assert_eq!(check_start_save(Some(&junk)), Ok(()));
    }

    /// The window tells the room the map and year of the save it already
    /// starts from once the owner's game read them: the room's mods, which
    /// the owner picked with it, stay as they are.
    #[test]
    fn the_save_the_room_starts_from_named_again_only_describes_it() {
        let start = |name: &str| tpf3mp_proto::StartView {
            save: StartSave {
                name: Text::new(name).unwrap(),
                map: Text::lossy(""),
                year: 0,
            },
            arrived: true,
        };
        let mut room = tpf3mp_proto::RoomView {
            id: tpf3mp_proto::RoomId(tpf3mp_proto::FixedBytes([7; 16])),
            name: Text::new("Sunday line").unwrap(),
            rules: Text::new("native").unwrap(),
            owner: tpf3mp_proto::PlayerId(tpf3mp_proto::FixedBytes([1; 32])),
            max_players: 4,
            has_password: false,
            phase: RoomPhase::Lobby,
            settings: tpf3mp_proto::RoomSettings::DEFAULT,
            members: Vec::new(),
            competitive: false,
            start: Some(start("mptest")),
        };
        assert!(names_start_again(&room, "mptest"));
        assert!(names_start_again(&room, " mptest "));
        assert!(!names_start_again(&room, "other"));
        assert!(!names_start_again(&room, ""));
        room.start = None;
        assert!(!names_start_again(&room, "mptest"));
    }

    /// A save without TPF3-MP's mod starts a room all the same when the
    /// room's list made of it runs the mod: every game loads the room's
    /// list, which always does. Without the picker, each game would load
    /// the save's own mods, and it is refused.
    #[test]
    fn a_save_without_tpf3mps_mod_starts_a_room_whose_list_runs_it() {
        let dir = tempfile::tempdir().unwrap();
        let without = dir.path().join("without.sav");
        crate::save_check::saves::write(&without, &["urbangames_preorder_pack"]);
        let own = crate::picker::Installed {
            id: "tpf3mp_1".into(),
            name: "TPF3-MP".into(),
            version: "1+0123456789abcdef".into(),
            hub: None,
            class: tpf3mp_modscan::Class::Shared,
            reason: String::new(),
            path: dir.path().join("tpf3mp_1"),
        };
        let picker = crate::picker::Mods::new(Text::lossy("40408"), vec![own], [], false);
        assert!(room_list_runs_own_mod(Some(&picker), Some(&without)));
        assert!(!room_list_runs_own_mod(None, Some(&without)));
        assert!(!room_list_runs_own_mod(Some(&picker), None));
    }

    #[test]
    fn a_game_with_another_tpf3mps_hook_is_told_to_restart_from_here() {
        let message = other_hook_message(tpf3mp_bridge::BRIDGE_VERSION - 1);
        assert!(
            message.starts_with(&format!(
                "Transport Fever 3 runs the hook of another TPF3-MP (game link version {}, this launcher {})",
                tpf3mp_bridge::BRIDGE_VERSION - 1,
                tpf3mp_bridge::BRIDGE_VERSION
            )),
            "{message}"
        );
        assert!(!message.contains("  "), "{message}");
    }

    #[test]
    fn the_server_and_name_are_remembered_for_next_time() {
        let dir = std::env::temp_dir().join(format!("tpf3mp-remember-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let file = dir.join("launcher.json");
        assert_eq!(Remembered::load(&file), Remembered::default());
        let remembered = Remembered {
            server: Some("tpf3mp.example.org:29470".into()),
            name: Some("Ann".into()),
            chosen_server: Some("play.example.net:29470".into()),
            diagnostics: Some(false),
            mods: Some(vec!["schbrongx_minimap".into()]),
            banner: Some("m03".into()),
        };
        remembered.save(&file).unwrap();
        assert_eq!(Remembered::load(&file), remembered);
        // A file from before diagnostics were remembered still loads.
        fs::write(
            &file,
            br#"{"server":"tpf3mp.example.org:29470","name":"Ann"}"#,
        )
        .unwrap();
        assert_eq!(
            Remembered::load(&file),
            Remembered {
                chosen_server: None,
                diagnostics: None,
                mods: None,
                banner: None,
                ..remembered
            }
        );
        fs::write(&file, b"not json").unwrap();
        assert_eq!(Remembered::load(&file), Remembered::default());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_invite_is_found_in_whatever_message_it_came_in() {
        let code = invite().to_string();
        let passed = |text: String| passed_invite(&text);
        let at = |server: &str| {
            Some(Passed {
                server: Some(server.to_owned()),
                invite: invite(),
            })
        };
        // As "Copy invite" gives it.
        assert_eq!(
            passed(format!("tpf3mp.example.org:29470 {code}")),
            at("tpf3mp.example.org:29470")
        );
        // Pasted from a chat, formatted.
        assert_eq!(
            passed(format!(
                "join us at `play.example.net:29470` with \"{code}\", at 12:30!"
            )),
            at("play.example.net:29470")
        );
        assert_eq!(
            passed(format!("[2001:db8::1]:29470 {code}")),
            at("[2001:db8::1]:29470")
        );
        assert_eq!(
            passed(format!("localhost:29470\n{code}")),
            at("localhost:29470")
        );
        // A time of day is no server.
        assert_eq!(
            passed(format!("at 12:30 {code}")),
            Some(Passed {
                server: None,
                invite: invite(),
            })
        );
        // The bare invite, typed in lower case, and no invite at all.
        assert_eq!(passed(code.clone()).map(|p| p.server), Some(None));
        assert_eq!(
            passed(code.to_lowercase()).map(|p| p.invite),
            Some(invite())
        );
        assert_eq!(passed("tpf3mp.example.org:29470".into()), None);
        // Six-letter words are not taken for it.
        assert_eq!(
            passed(format!("thanks! STREET party: {code}")).map(|p| p.invite),
            Some(invite())
        );
        assert_eq!(passed("thanks for STREET".into()), None);
        // A cut-off invite is none.
        assert_eq!(passed(code[..code.len() - 1].to_owned()), None);
    }

    #[tokio::test]
    async fn a_game_that_exits_is_noticed_within_a_poll() {
        let mut asked = 0;
        let started = std::time::Instant::now();
        exited(|| {
            asked += 1;
            asked < 3
        })
        .await;
        assert_eq!(asked, 3, "asked until the game had gone");
        assert!(started.elapsed() < GAME_POLL * 3, "{:?}", started.elapsed());
        // A launcher that started no game waits on none.
        let none = tokio::time::timeout(Duration::from_millis(50), game_exit(&mut None)).await;
        assert!(none.is_err());
    }

    #[test]
    fn a_room_starts_from_a_listed_save_picked_by_name_or_the_launchers_own() {
        let listed = vec!["mptest".to_owned(), "older".to_owned()];
        let default = PathBuf::from("launcher.sav");
        let find = |name: &str| Ok(PathBuf::from(format!("/saves/{name}.sav")));
        assert_eq!(
            start_world(Some(" mptest "), &listed, Some(&default), find),
            Ok(Some(PathBuf::from("/saves/mptest.sav")))
        );
        assert_eq!(
            start_world(None, &listed, Some(&default), find),
            Ok(Some(default.clone())),
            "nothing picked: the launcher's own"
        );
        assert_eq!(start_world(None, &listed, None, find), Ok(None));
        assert_eq!(
            start_world(Some(""), &listed, Some(&default), find),
            Ok(None),
            "none picked: the owner's game loads a world itself"
        );
        // Only a save the player was offered, by its name.
        assert!(start_world(Some("other"), &listed, None, find).is_err());
        assert!(start_world(Some("C:/Windows/win.ini"), &listed, None, find).is_err());
        let gone = |_: &str| Err("no save".to_owned());
        assert!(start_world(Some("mptest"), &listed, None, gone).is_err());
        assert_eq!(
            save_name(Path::new("/x/y/twomptest.sav")).as_deref(),
            Some("twomptest")
        );
        // What the room shows everyone of it: its name, with what the
        // owner's game read.
        let named = start_save_named(Path::new("/saves/Güterzug.sav"), " dry ", 1900);
        assert_eq!(
            (named.name.as_str(), named.map.as_str(), named.year),
            ("Güterzug", "dry", 1900)
        );
    }

    #[test]
    fn a_new_world_picked_in_the_room_offers_no_save() {
        // The window offers Set up world instead of Ready while the
        // launcher offers no save: a new world picked after a save must
        // not leave that save offered, or the owner readies a room with no
        // world and its start waits for one the main menu cannot load.
        assert_eq!(offered_start(" Güterzug "), Some("Güterzug".to_owned()));
        assert_eq!(offered_start(""), None);
        assert_eq!(offered_start("  "), None);
    }

    #[test]
    fn the_server_setting_takes_host_and_port_only() {
        for good in [
            "tpf3mp.213-133-98-90.sslip.io:29470",
            " localhost:29470 ",
            "127.0.0.1:29470",
            "[2001:db8::1]:29470",
            "EU.Example.org:1",
        ] {
            assert_eq!(server_address(good), Ok(good.trim().to_owned()), "{good}");
        }
        for bad in [
            "",
            "tpf3mp.example.org",
            "tpf3mp.example.org:",
            "tpf3mp.example.org:0",
            "tpf3mp.example.org:65536",
            "tpf3mp.example.org:+80",
            ":29470",
            "two words:29470",
            "https://tpf3mp.example.org:29470",
            "evil.example:29470 K7QM2X",
            "-bad.example:29470",
            "a..b:29470",
            "2001:db8::1:29470",
            "[not-ip]:29470",
            "\u{e9}.example:29470",
        ] {
            assert!(server_address(bad).is_err(), "{bad:?}");
        }
        let long = format!("{}.{}.example:29470", "a".repeat(55), "b".repeat(55));
        assert!(server_address(&long).is_ok());
        let too_long = format!("{}:29470", vec!["a".repeat(60); 3].join("."));
        assert!(
            server_address(&too_long).is_err(),
            "past the lobby's 128 bytes"
        );
    }

    #[test]
    fn a_launcher_with_its_own_server_goes_nowhere_else() {
        let code = invite().to_string();
        let own = "tpf3mp.example.org:29470";
        let connect = |fixed: Option<&str>, typed: String| {
            server_for(fixed, &typed, passed_invite(&typed).as_ref())
        };
        // Its own server, whatever the invite says of it, and for nothing.
        assert_eq!(connect(Some(own), String::new()), Ok(own.to_owned()));
        assert_eq!(connect(Some(own), own.to_uppercase()), Ok(own.to_owned()));
        assert_eq!(connect(Some(own), code.clone()), Ok(own.to_owned()));
        assert_eq!(
            connect(Some(own), format!("{own} {code}")),
            Ok(own.to_owned())
        );
        // Another server, alone or with an invite, is refused.
        let refused = connect(Some(own), format!("evil.example:29470 {code}")).unwrap_err();
        assert!(refused.contains("another server"), "{refused}");
        assert!(refused.contains(own), "{refused}");
        assert_eq!(
            connect(Some(own), "evil.example:29470".into()),
            Err("that is not an invite".into())
        );

        // Without one, as a build for development: what the player typed.
        assert_eq!(
            connect(None, " play.example.net:29470 ".into()),
            Ok("play.example.net:29470".into())
        );
        assert_eq!(
            connect(None, format!("play.example.net:29470 {code}")),
            Ok("play.example.net:29470".into())
        );
        assert!(connect(None, code).is_err(), "an invite needs its server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn suspended_startup_can_finish_its_hook_handshake() {
        let name = format!("test.launcher.bootstrap.{}", std::process::id());
        let mut idle = Some(open_link(&name).unwrap());
        // Stand in for Windows launch: it cannot finish until the hook's
        // blocking attach receives Hello. No game or relay is started.
        let launch = tokio::task::spawn_blocking(move || {
            tpf3mp_bridge::Session::attach(&name, "40408", Duration::from_secs(2))
        });
        let attached = wait_for_launch(launch, &mut idle, &LobbyView::default())
            .await
            .unwrap();
        assert!(attached.is_ok(), "{:?}", attached.err());
        assert_eq!(idle.as_ref().unwrap().build(), Some("40408"));
    }

    /// A game started again after the first one closed: the hook of the
    /// next attaches although the first never read the room's end.
    #[test]
    fn a_game_started_again_attaches_past_what_the_closed_one_left() {
        use crate::bridge::HookLink;

        let name = format!("test.launcher.restart.{}", std::process::id());
        let (mut link, _) = open_link(&name).unwrap().into_parts();
        HookLink::heartbeat(&mut link);
        // The room ended while the closed game's hook no longer read it.
        let end = tpf3mp_bridge::ToHook::End {
            reason: Text::new("the room ended").unwrap(),
        };
        assert!(HookLink::send(&mut link, &tpf3mp_bridge::encode(&end).unwrap()).unwrap());
        let mut idle = Some(IdleLink::new(link));
        renew_unused_link(&name, &mut None, &mut idle).unwrap();
        let mut idle = idle.expect("a new link");

        let hook = std::thread::spawn({
            let name = name.clone();
            move || tpf3mp_bridge::Session::attach(&name, "40408", Duration::from_secs(10))
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !hook.is_finished() && std::time::Instant::now() < deadline {
            idle.pump(&LobbyView::default()).unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        let attached = hook.join().unwrap();
        assert!(attached.is_ok(), "{:?}", attached.err());
        assert_eq!(idle.build(), Some("40408"), "the launcher greeted it");
    }

    /// A game this launcher took over keeps its link while its process
    /// runs, silent or not.
    #[test]
    fn a_link_with_a_game_on_it_is_kept() {
        let name = format!("test.launcher.kept.{}", std::process::id());
        let (link, _) = open_link(&name).unwrap().into_parts();
        let generation = link.session();
        // Its hook, in a process that runs: this one.
        let _hook = tpf3mp_ipc::Link::open(&name, tpf3mp_ipc::Role::Hook).unwrap();
        let mut idle = Some(IdleLink::resumed(link, Some("40408".into())));
        renew_unused_link(&name, &mut None, &mut idle).unwrap();
        let (link, build) = idle.unwrap().into_parts();
        assert_eq!(link.session(), generation, "the same link");
        assert_eq!(build.as_deref(), Some("40408"));
    }
}
