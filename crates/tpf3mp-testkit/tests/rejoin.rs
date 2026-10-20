//! A bridge that lost its server stops rejoining when there is nothing to
//! rejoin, and lets the player leave meanwhile (seen live: a launcher kept
//! rejoining a room the restarted server no longer had, and its player
//! could not leave it).

#![allow(clippy::unwrap_used)]

use std::{
    collections::VecDeque,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};
use tpf3mp_agent::{
    ConnectOptions,
    bridge::{
        self, Bridge, BridgeEnd, BridgeFault, BridgeOptions, Control, HookLink, LobbyLink,
        ROOM_GONE, Rejoin, SharedStatus,
    },
    connect,
};
use tpf3mp_bridge::{BRIDGE_VERSION, LobbyAction, LobbyView, ToAgent, encode};
use tpf3mp_net::{Identity, ServerIdentity, ServerTrust};
use tpf3mp_proto::{CreateRoom, RoomSettings, Text};
use tpf3mp_server::{Server, ServerConfig, ServerError};

/// A hook that says what the test gives it to say, takes everything, and
/// stays alive.
#[derive(Clone, Default)]
struct ScriptedHook {
    said: Arc<Mutex<VecDeque<Vec<u8>>>>,
    beat: Arc<AtomicU64>,
}

impl ScriptedHook {
    fn say(&self, message: &ToAgent) {
        self.said
            .lock()
            .unwrap()
            .push_back(encode(message).unwrap());
    }
}

impl HookLink for ScriptedHook {
    fn send(&mut self, _message: &[u8]) -> Result<bool, BridgeFault> {
        Ok(true)
    }

    fn recv(&mut self, buf: &mut Vec<u8>) -> Result<bool, BridgeFault> {
        match self.said.lock().unwrap().pop_front() {
            Some(message) => {
                *buf = message;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn heartbeat(&mut self) {}

    fn peer_heartbeat(&self) -> u64 {
        self.beat.fetch_add(1, Ordering::Relaxed)
    }
}

struct TestServer {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl TestServer {
    /// Starts a server, waiting a little for the address when one just
    /// stopped there.
    async fn start(config: &ServerConfig) -> (Self, SocketAddr) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let server = loop {
            match Server::bind(config.clone()) {
                Ok(server) => break server,
                Err(ServerError::Bind(_)) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => panic!("cannot start the server: {error}"),
            }
        };
        let address = server.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(server.run(async {
            let _ = stopped.await;
        }));
        (Self { stop, task }, address)
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = tokio::time::timeout(Duration::from_secs(10), self.task).await;
    }
}

fn config(listen: SocketAddr, identity: ServerIdentity) -> ServerConfig {
    let mut config = ServerConfig::new(listen, identity);
    config.tick = Duration::from_millis(20);
    config.max_sessions_per_address = 1000;
    config.max_handshakes_per_address = 1000;
    config.max_rooms_per_address = 1000;
    config
}

/// A player in a lobby, its session played through a bridge on `hook`:
/// returns the session, its controls and its status.
async fn in_a_room(
    address: SocketAddr,
    trust: ServerTrust,
    hook: ScriptedHook,
    lobby: Option<LobbyLink>,
) -> (
    JoinHandle<Result<BridgeEnd, BridgeFault>>,
    mpsc::Sender<Control>,
    SharedStatus,
) {
    in_a_room_with_patience(address, trust, hook, lobby, Duration::from_secs(120), None).await
}

async fn in_a_room_with_patience(
    address: SocketAddr,
    trust: ServerTrust,
    hook: ScriptedHook,
    lobby: Option<LobbyLink>,
    patience: Duration,
    rejoin_address: Option<SocketAddr>,
) -> (
    JoinHandle<Result<BridgeEnd, BridgeFault>>,
    mpsc::Sender<Control>,
    SharedStatus,
) {
    let options = ConnectOptions::new(
        address,
        "localhost",
        trust,
        Arc::new(Identity::generate().unwrap().0),
        Text::new("ann").unwrap(),
    );
    let (client, events) = connect(options.clone()).await.unwrap();
    let (invite, _) = client
        .create_room(CreateRoom {
            name: Text::new("table").unwrap(),
            max_players: 8,
            password: None,
            settings: RoomSettings {
                scan_plan: tpf3mp_proto::ScanPlan::DEFAULT,
                steps_per_second: 50,
                input_delay_ms: 40,
                checkpoint_interval: 10,
            },
            rules: None,
            listing: None,
            competitive: false,
        })
        .await
        .unwrap();
    let mut options = options;
    if let Some(address) = rejoin_address {
        options.server = address;
    }
    let rejoin = Rejoin {
        options,
        invite,
        password: None,
        content: None,
        give_up_after: patience,
    };
    hook.say(&ToAgent::Hello {
        version: BRIDGE_VERSION,
        build: Text::new("test").unwrap(),
    });
    let status = SharedStatus::default();
    let bridge_options = BridgeOptions {
        status: Some(Arc::clone(&status)),
        lobby,
        ..BridgeOptions::default()
    };
    let (controls, controls_rx) = mpsc::channel(8);
    let session = tokio::spawn(async move {
        let mut bridge = Bridge::new(hook, bridge_options).with_controls(controls_rx);
        bridge::play(&mut bridge, client, events, &rejoin).await
    });
    (session, controls, status)
}

/// Waits until the status has a notice `wanted` holds for.
async fn notice(status: &SharedStatus, wanted: impl Fn(&str) -> bool) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while !status
            .lock()
            .unwrap()
            .notices
            .iter()
            .any(|notice| wanted(notice))
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the notice came");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejoining_a_room_the_restarted_server_no_longer_has_stops_and_says_so() {
    let identity = ServerIdentity::self_signed(&["localhost"]).unwrap();
    let trust = ServerTrust::Pinned(identity.leaf().clone());
    let (server, address) =
        TestServer::start(&config("127.0.0.1:0".parse().unwrap(), identity.clone())).await;
    let (session, _controls, status) =
        in_a_room(address, trust, ScriptedHook::default(), None).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The server restarts without the room (a lobby is never kept).
    server.stop().await;
    let (server, _) = TestServer::start(&config(address, identity)).await;

    let ended = tokio::time::timeout(Duration::from_secs(30), session)
        .await
        .expect("rejoining stops, long before its patience runs out")
        .unwrap();
    assert!(
        matches!(ended, Err(BridgeFault::RoomGone)),
        "ended for the room being gone: {ended:?}"
    );
    assert_eq!(BridgeFault::RoomGone.to_string(), ROOM_GONE);
    assert_eq!(
        ROOM_GONE,
        "The room is gone (closed or the server restarted)"
    );
    notice(&status, |notice| notice == ROOM_GONE).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn leaving_works_while_rejoining_from_the_launcher_and_from_the_game() {
    let identity = ServerIdentity::self_signed(&["localhost"]).unwrap();
    let trust = ServerTrust::Pinned(identity.leaf().clone());
    let (server, address) =
        TestServer::start(&config("127.0.0.1:0".parse().unwrap(), identity)).await;
    let hook = ScriptedHook::default();
    let (_views, views_rx) = watch::channel(LobbyView::default());
    let (actions, mut actions_rx) = mpsc::unbounded_channel();
    let lobby = LobbyLink {
        views: views_rx,
        actions,
    };
    let (session, controls, status) = in_a_room(address, trust, hook.clone(), Some(lobby)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The server goes away and stays away: the bridge keeps trying.
    server.stop().await;
    notice(&status, |notice| notice.contains("rejoining the room")).await;

    // The game's Multiplayer window's Leave reaches the launcher meanwhile.
    hook.say(&ToAgent::Lobby(LobbyAction::Leave));
    let forwarded = tokio::time::timeout(Duration::from_secs(5), actions_rx.recv())
        .await
        .expect("the game's Leave reaches the launcher while rejoining");
    assert_eq!(forwarded, Some(LobbyAction::Leave));

    // Which the launcher carries out as its own Leave: the session ends at
    // once, left, with nobody to tell.
    controls.send(Control::Leave).await.unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(5), session)
        .await
        .expect("leaving ends the rejoining at once")
        .unwrap();
    assert!(
        matches!(ended, Ok(BridgeEnd::Left)),
        "ended as left: {ended:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unanswered_connect_cannot_outlive_the_rejoin_deadline() {
    // A bound UDP port that never answers makes the reconnect wait for its handshake timeout.
    let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let identity = ServerIdentity::self_signed(&["localhost"]).unwrap();
    let trust = ServerTrust::Pinned(identity.leaf().clone());
    let (server, address) =
        TestServer::start(&config("127.0.0.1:0".parse().unwrap(), identity)).await;
    let (session, _controls, status) = in_a_room_with_patience(
        address,
        trust,
        ScriptedHook::default(),
        None,
        Duration::from_millis(100),
        Some(silent.local_addr().unwrap()),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    server.stop().await;
    notice(&status, |n| n.contains("rejoining the room")).await;
    let ended = tokio::time::timeout(Duration::from_secs(3), session)
        .await
        .expect("a connect attempt must obey the session's deadline")
        .unwrap();
    assert!(matches!(ended, Err(BridgeFault::Rejoin(_))), "{ended:?}");
}
