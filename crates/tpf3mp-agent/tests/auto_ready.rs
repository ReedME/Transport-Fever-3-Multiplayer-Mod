//! A player is marked ready by the agent once their game has a world up with
//! the mod linked (`ToAgent::WorldUp`), in the room's lobby: nobody has to
//! press Ready. Once a world: a player who then says Not ready stays so until
//! another world is up. Never outside the lobby. A game at its main menu
//! (`ToAgent::MenuUp`) marks a guest ready too, since it loads the room's
//! world from there, but never the room's owner, whose world the room plays,
//! unless the owner hands the room a save to start from: then the owner is
//! marked ready at the menu too, once the room has that save. An owner who
//! names another save in the lobby hands that one over and is ready again
//! once the room has it; one who takes the save back readies with a world
//! up, as without one.

#![allow(clippy::unwrap_used)]

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, mpsc as std_mpsc},
    time::Duration,
};

use tokio::sync::mpsc;
use tpf3mp_agent::{
    ConnectOptions, Worlds,
    bridge::{Bridge, BridgeFault, BridgeOptions, Control, HookLink},
    connect,
};
use tpf3mp_bridge::{BRIDGE_VERSION, ToAgent, encode};
use tpf3mp_net::{
    Identity, ServerIdentity, ServerTrust, bulk, read_message, read_preamble, server_config,
    write_message, write_preamble,
};
use tpf3mp_proto::{
    BULK_REQUEST_MAX_FRAME, BulkOpen, CONTROL_MAX_FRAME, ChatText, ClientMessage, ContentManifest,
    FixedBytes, PROTOCOL_VERSION, PlayerId, Request, Response, RoomId, RoomPhase, RoomSettings,
    RoomView, RulesName, ServerMessage, SessionId, StartSave, Text, Welcome,
};
use tpf3mp_snapshot::{ChunkStore, StoreConfig};

/// A hook that says what the test hands it, when it does, and takes
/// everything.
struct ScriptedHook {
    said: std_mpsc::Receiver<Vec<u8>>,
}

impl HookLink for ScriptedHook {
    fn send(&mut self, _message: &[u8]) -> Result<bool, BridgeFault> {
        Ok(true)
    }

    fn recv(&mut self, buf: &mut Vec<u8>) -> Result<bool, BridgeFault> {
        match self.said.try_recv() {
            Ok(message) => {
                *buf = message;
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }

    fn heartbeat(&mut self) {}

    fn peer_heartbeat(&self) -> u64 {
        0
    }
}

fn room(phase: RoomPhase, owner: PlayerId) -> RoomView {
    RoomView {
        id: RoomId(FixedBytes([3; 16])),
        name: Text::new("Friday trains").unwrap(),
        rules: RulesName::new("native").unwrap(),
        owner,
        max_players: 4,
        has_password: false,
        phase,
        settings: RoomSettings {
            scan_plan: tpf3mp_proto::ScanPlan::DEFAULT,
            steps_per_second: 5,
            input_delay_ms: 100,
            checkpoint_interval: 50,
        },
        members: Vec::new(),
        competitive: false,
        start: None,
    }
}

/// A server that completes the handshake, announces a room in `phase`
/// `announce_after` later, answers every request done and passes it on.
/// A world the room is to start from it asks for, and takes, if `uploads`.
async fn server(
    phase: RoomPhase,
    announce_after: Duration,
    owner: PlayerId,
    uploads: bool,
) -> (SocketAddr, ServerTrust, mpsc::UnboundedReceiver<Request>) {
    let identity = ServerIdentity::self_signed(&["localhost"]).unwrap();
    let leaf = identity.leaf().clone();
    let endpoint = quinn::Endpoint::server(
        server_config(identity).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let address = endpoint.local_addr().unwrap();
    let (heard_tx, heard) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let connection = endpoint.accept().await.unwrap().await.unwrap();
        let (mut send, mut recv) = connection.accept_bi().await.unwrap();
        read_preamble(&mut recv).await.unwrap();
        write_preamble(&mut send, PROTOCOL_VERSION).await.unwrap();
        let _hello: ClientMessage = read_message(&mut recv, CONTROL_MAX_FRAME).await.unwrap();
        let welcome = ServerMessage::Welcome(Welcome {
            server_version: Text::new("test").unwrap(),
            session_id: SessionId("AB2CD3".parse().unwrap()),
            rules: Vec::new(),
        });
        write_message(&mut send, &welcome, CONTROL_MAX_FRAME)
            .await
            .unwrap();
        tokio::time::sleep(announce_after).await;
        write_message(
            &mut send,
            &ServerMessage::RoomUpdate(room(phase, owner)),
            CONTROL_MAX_FRAME,
        )
        .await
        .unwrap();
        while let Ok(message) = read_message::<ClientMessage>(&mut recv, CONTROL_MAX_FRAME).await {
            if let ClientMessage::Request { id, request } = message {
                let asked = match &request {
                    Request::StartWorld { world, .. } if uploads => Some(world.snapshot),
                    _ => None,
                };
                let _ = heard_tx.send(request);
                let answer = ServerMessage::Response {
                    id,
                    result: Ok(Response::Done),
                };
                if write_message(&mut send, &answer, CONTROL_MAX_FRAME)
                    .await
                    .is_err()
                {
                    break;
                }
                if let Some(snapshot) = asked {
                    tokio::spawn(take_upload(connection.clone()));
                    let upload = ServerMessage::Upload { event: 0, snapshot };
                    if write_message(&mut send, &upload, CONTROL_MAX_FRAME)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
        drop((endpoint, connection, send));
    });
    (address, ServerTrust::Pinned(leaf), heard)
}

/// Takes the one upload a client opens, into a store of its own.
async fn take_upload(connection: quinn::Connection) {
    let (mut send, mut recv) = connection.accept_bi().await.unwrap();
    read_preamble(&mut recv).await.unwrap();
    write_preamble(&mut send, PROTOCOL_VERSION).await.unwrap();
    let BulkOpen::Serve { snapshot } = read_message(&mut recv, BULK_REQUEST_MAX_FRAME)
        .await
        .unwrap()
    else {
        panic!("the client fetches instead of serving");
    };
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path(), StoreConfig::new(1 << 30)).unwrap();
    bulk::fetch(
        &mut send,
        &mut recv,
        &store,
        &bulk::manifest_id(&snapshot),
        bulk::Completion::Retain,
        Duration::from_secs(20),
        |_| {},
    )
    .await
    .unwrap();
    let _ = send.finish();
    // The client learns the upload ended from the stream's end.
    tokio::time::sleep(Duration::from_secs(1)).await;
}

/// Whose game is at the menu.
#[derive(Clone, Copy)]
enum Seat {
    Guest,
    Owner,
}

/// What the test drives: the hook's words, the front end's controls and what
/// the server heard.
struct Session {
    hook: std_mpsc::Sender<Vec<u8>>,
    controls: mpsc::Sender<Control>,
    heard: mpsc::UnboundedReceiver<Request>,
    bridge: tokio::task::JoinHandle<()>,
}

impl Session {
    async fn start(phase: RoomPhase) -> Self {
        Self::announced_after(phase, Duration::ZERO).await
    }

    async fn announced_after(phase: RoomPhase, announce_after: Duration) -> Self {
        Self::with(phase, announce_after, Seat::Guest, None, None, false).await
    }

    /// At the main menu: a guest or the room's owner, with an agent that
    /// keeps worlds in `worlds` or none.
    async fn at_menu(phase: RoomPhase, seat: Seat, worlds: Option<Worlds>) -> Self {
        Self::with(phase, Duration::ZERO, seat, worlds, None, false).await
    }

    /// The room's owner at the main menu, handing the room the save
    /// `start_world` to start from, to a room that takes it if `uploads`.
    async fn owner_starting_from(start_world: PathBuf, tag: &str, uploads: bool) -> Self {
        Self::with(
            RoomPhase::Lobby,
            Duration::ZERO,
            Seat::Owner,
            Some(worlds(tag)),
            Some(start_world),
            uploads,
        )
        .await
    }

    async fn with(
        phase: RoomPhase,
        announce_after: Duration,
        seat: Seat,
        worlds: Option<Worlds>,
        start_world: Option<PathBuf>,
        uploads: bool,
    ) -> Self {
        let player = Arc::new(Identity::generate().unwrap().0);
        let owner = match seat {
            Seat::Owner => player.player(),
            Seat::Guest => PlayerId(FixedBytes([1; 32])),
        };
        let (address, trust, heard) = server(phase, announce_after, owner, uploads).await;
        let (client, mut events) = connect(ConnectOptions::new(
            address,
            "localhost",
            trust,
            player,
            Text::new("player").unwrap(),
        ))
        .await
        .unwrap();
        let (hook, said) = std_mpsc::channel();
        let (controls, controls_rx) = mpsc::channel(8);
        let bridge = tokio::spawn(async move {
            let options = BridgeOptions {
                worlds,
                start_world,
                ..BridgeOptions::default()
            };
            let mut bridge = Bridge::new(ScriptedHook { said }, options).with_controls(controls_rx);
            let _ = bridge.run(&client, &mut events).await;
        });
        let session = Self {
            hook,
            controls,
            heard,
            bridge,
        };
        session.hook_says(&ToAgent::Hello {
            version: BRIDGE_VERSION,
            build: Text::new("test").unwrap(),
        });
        session
    }

    fn hook_says(&self, message: &ToAgent) {
        self.hook.send(encode(message).unwrap()).unwrap();
    }

    /// The next request the server hears.
    async fn next(&mut self) -> Request {
        tokio::time::timeout(Duration::from_secs(20), self.heard.recv())
            .await
            .expect("the server hears a request")
            .unwrap()
    }

    /// The requests the server hears until the hook's chat `marker`, which
    /// the hook says after what the test watches, and a moment more.
    async fn until(&mut self, marker: &str) -> Vec<Request> {
        self.hook_says(&ToAgent::Chat {
            text: ChatText::new(marker).unwrap(),
        });
        let mut heard = Vec::new();
        loop {
            match self.next().await {
                Request::Chat(text) if text.as_str() == marker => break,
                other => heard.push(other),
            }
        }
        // Requests go out on tasks of their own: one sent before the
        // marker may land just after it.
        while let Ok(Some(request)) =
            tokio::time::timeout(Duration::from_millis(300), self.heard.recv()).await
        {
            heard.push(request);
        }
        heard
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.bridge.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_world_up_in_the_lobby_marks_the_player_ready() {
    let mut session = Session::start(RoomPhase::Lobby).await;
    assert!(
        session.until("attached").await.is_empty(),
        "an attached hook alone marks nobody ready"
    );
    session.hook_says(&ToAgent::WorldUp { world: 1 });
    assert_eq!(session.next().await, Request::SetReady(true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_ready_after_a_world_up_stays_until_another_world_is_up() {
    let mut session = Session::start(RoomPhase::Lobby).await;
    session.hook_says(&ToAgent::WorldUp { world: 1 });
    assert_eq!(session.next().await, Request::SetReady(true));
    // The player presses Not ready.
    session.controls.send(Control::Ready(false)).await.unwrap();
    assert_eq!(session.next().await, Request::SetReady(false));
    // The same world again, or an older one: the player stays not ready.
    session.hook_says(&ToAgent::WorldUp { world: 1 });
    session.hook_says(&ToAgent::WorldUp { world: 0 });
    let heard = session.until("same world").await;
    assert!(
        !heard.contains(&Request::SetReady(true)),
        "the player said Not ready for this world: {heard:?}"
    );
    // Another world: ready again.
    session.hook_says(&ToAgent::WorldUp { world: 2 });
    assert_eq!(session.next().await, Request::SetReady(true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_world_up_outside_the_lobby_marks_nobody_ready() {
    let mut session = Session::start(RoomPhase::Running).await;
    session.hook_says(&ToAgent::WorldUp { world: 1 });
    let heard = session.until("running").await;
    assert!(
        !heard.contains(&Request::SetReady(true)),
        "the room's game runs: {heard:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_world_up_before_the_room_is_announced_waits_for_the_lobby() {
    let mut session = Session::announced_after(RoomPhase::Lobby, Duration::from_millis(500)).await;
    session.hook_says(&ToAgent::WorldUp { world: 1 });
    assert_eq!(session.next().await, Request::SetReady(true));
}

fn worlds(tag: &str) -> Worlds {
    let dir = std::env::temp_dir().join(format!("tpf3mp-auto-ready-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Worlds::open(&dir, 1 << 30).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_at_the_main_menu_is_marked_ready_once_per_arrival() {
    let mut session = Session::at_menu(RoomPhase::Lobby, Seat::Guest, Some(worlds("guest"))).await;
    session.hook_says(&ToAgent::MenuUp { menu: 1 });
    assert_eq!(session.next().await, Request::SetReady(true));
    session.controls.send(Control::Ready(false)).await.unwrap();
    assert_eq!(session.next().await, Request::SetReady(false));
    session.hook_says(&ToAgent::MenuUp { menu: 1 });
    let heard = session.until("same menu").await;
    assert!(
        !heard.contains(&Request::SetReady(true)),
        "Not ready holds for this arrival: {heard:?}"
    );
    session.hook_says(&ToAgent::MenuUp { menu: 2 });
    assert_eq!(session.next().await, Request::SetReady(true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_at_the_main_menu_is_not_marked_ready() {
    let mut session = Session::at_menu(RoomPhase::Lobby, Seat::Owner, Some(worlds("owner"))).await;
    session.hook_says(&ToAgent::MenuUp { menu: 1 });
    let heard = session.until("owner at menu").await;
    assert!(
        !heard.contains(&Request::SetReady(true)),
        "the room plays the owner's world, which needs one up: {heard:?}"
    );
    // A world up is the owner's way to ready.
    session.hook_says(&ToAgent::WorldUp { world: 1 });
    assert_eq!(session.next().await, Request::SetReady(true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_at_the_menu_without_worlds_or_outside_the_lobby_is_not_marked_ready() {
    let mut session = Session::at_menu(RoomPhase::Lobby, Seat::Guest, None).await;
    session.hook_says(&ToAgent::MenuUp { menu: 1 });
    let heard = session.until("no worlds").await;
    assert!(!heard.contains(&Request::SetReady(true)), "{heard:?}");

    let mut session =
        Session::at_menu(RoomPhase::Running, Seat::Guest, Some(worlds("running"))).await;
    session.hook_says(&ToAgent::MenuUp { menu: 1 });
    let heard = session.until("running").await;
    assert!(!heard.contains(&Request::SetReady(true)), "{heard:?}");
}

/// A save of the player's own, as the game keeps it in its save folder.
fn start_save(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tpf3mp-start-save-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("mptest.sav");
    let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 253) as u8).collect();
    std::fs::write(&file, bytes).unwrap();
    file
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_with_a_start_save_is_ready_at_the_menu_once_the_room_has_it() {
    let save = start_save("uploaded");
    let mut session = Session::owner_starting_from(save.clone(), "start-uploaded", true).await;
    // The room is handed the save before anything else.
    let Request::StartWorld { world, save: named } = session.next().await else {
        panic!("the owner's agent hands the room its save first");
    };
    assert_eq!(world.size, 200_000);
    assert_eq!(named.name.as_str(), "mptest", "named for everyone to see");
    session.hook_says(&ToAgent::MenuUp { menu: 1 });
    // Once uploaded, the owner's game waits at its menu like everyone's.
    assert_eq!(session.next().await, Request::SetReady(true));
    assert!(save.is_file(), "the player's own save stays");
    let _ = std::fs::remove_dir_all(save.parent().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_is_not_ready_while_the_start_save_is_on_its_way() {
    let save = start_save("pending");
    let mut session = Session::owner_starting_from(save.clone(), "start-pending", false).await;
    assert!(matches!(session.next().await, Request::StartWorld { .. }));
    session.hook_says(&ToAgent::MenuUp { menu: 1 });
    session.hook_says(&ToAgent::WorldUp { world: 1 });
    let heard = session.until("never asked for it").await;
    assert!(
        !heard.contains(&Request::SetReady(true)),
        "the room cannot start before it has the save: {heard:?}"
    );
    let _ = std::fs::remove_dir_all(save.parent().unwrap());
}

/// Another save of the player's own, next to `start_save`'s.
fn other_save(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tpf3mp-other-save-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("other.sav");
    let bytes: Vec<u8> = (0..150_000u32).map(|i| (i % 241) as u8).collect();
    std::fs::write(&file, bytes).unwrap();
    file
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_naming_another_save_hands_it_over_and_readies_once_the_room_has_it() {
    let first = start_save("replaced");
    let second = other_save("replacing");
    let mut session = Session::owner_starting_from(first.clone(), "start-replaced", true).await;
    let Request::StartWorld { world: before, .. } = session.next().await else {
        panic!("the first save goes first");
    };
    session.hook_says(&ToAgent::MenuUp { menu: 1 });
    assert_eq!(session.next().await, Request::SetReady(true));

    // The owner picks another on the room's page: the room's shared mods
    // follow it, then the room is handed it.
    let declared = ContentManifest::new(Text::new("40408").unwrap(), Vec::new());
    let named = StartSave {
        name: Text::new("other").unwrap(),
        map: Text::new("dry").unwrap(),
        year: 1900,
    };
    session
        .controls
        .send(Control::StartWorld {
            start: Some((second.clone(), named.clone())),
            declare: Some(tpf3mp_agent::picker::Declaration::Content(declared.clone())),
        })
        .await
        .unwrap();
    assert_eq!(session.next().await, Request::DeclareContent(declared));
    let Request::StartWorld { world, save } = session.next().await else {
        panic!("the new save is handed over");
    };
    assert_ne!(world.snapshot, before.snapshot);
    assert_eq!(world.size, 150_000);
    assert_eq!(
        save, named,
        "everyone sees what the owner's game read of it"
    );
    // The room asked everyone to agree again; the owner's game still waits
    // at its menu, and once the room has the new save the owner is ready.
    assert_eq!(session.next().await, Request::SetReady(true));
    for file in [first, second] {
        assert!(file.is_file(), "the player's own saves stay");
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_taking_the_save_back_readies_with_a_world_up_again() {
    let save = start_save("taken-back");
    let mut session = Session::owner_starting_from(save.clone(), "start-taken-back", true).await;
    assert!(matches!(session.next().await, Request::StartWorld { .. }));
    session.hook_says(&ToAgent::MenuUp { menu: 1 });
    assert_eq!(session.next().await, Request::SetReady(true));

    session
        .controls
        .send(Control::StartWorld {
            start: None,
            declare: None,
        })
        .await
        .unwrap();
    assert_eq!(session.next().await, Request::ClearStartWorld);
    let heard = session.until("taken back").await;
    assert!(
        !heard.contains(&Request::SetReady(true)),
        "at the menu, the owner has no world for the room: {heard:?}"
    );
    // As without a save: a world up readies the owner.
    session.hook_says(&ToAgent::WorldUp { world: 1 });
    assert_eq!(session.next().await, Request::SetReady(true));
    let _ = std::fs::remove_dir_all(save.parent().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_cannot_name_the_rooms_save() {
    let mut session =
        Session::at_menu(RoomPhase::Lobby, Seat::Guest, Some(worlds("guest-names"))).await;
    // Wait until the room is known.
    let _ = session.until("announced").await;
    session
        .controls
        .send(Control::StartWorld {
            start: Some((
                start_save("guest-names"),
                StartSave {
                    name: Text::new("mptest").unwrap(),
                    map: Text::new("").unwrap(),
                    year: 0,
                },
            )),
            declare: None,
        })
        .await
        .unwrap();
    let heard = session.until("guest named").await;
    assert!(
        !heard
            .iter()
            .any(|request| matches!(request, Request::StartWorld { .. })),
        "only the owner chooses: {heard:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_naming_the_same_save_again_only_describes_it() {
    let save = start_save("described");
    let mut session = Session::owner_starting_from(save.clone(), "start-described", true).await;
    let Request::StartWorld { world, .. } = session.next().await else {
        panic!("the save goes first");
    };
    session.hook_says(&ToAgent::MenuUp { menu: 1 });
    assert_eq!(session.next().await, Request::SetReady(true));
    // The window read the save's map and year once the room was made.
    let described = StartSave {
        name: Text::new("mptest").unwrap(),
        map: Text::new("temperate").unwrap(),
        year: 1875,
    };
    session
        .controls
        .send(Control::StartWorld {
            start: Some((save.clone(), described.clone())),
            declare: None,
        })
        .await
        .unwrap();
    assert_eq!(
        session.next().await,
        Request::StartWorld {
            world,
            save: described.clone()
        },
        "the same save, unchanged: named again without reading it again"
    );
    // The same save picked again with other mods or settings: the room
    // takes them first, then what it shows of the save.
    let picked = ContentManifest::new(Text::new("40408").unwrap(), Vec::new());
    session
        .controls
        .send(Control::StartWorld {
            start: Some((save.clone(), described.clone())),
            declare: Some(tpf3mp_agent::picker::Declaration::Content(picked.clone())),
        })
        .await
        .unwrap();
    assert_eq!(session.next().await, Request::DeclareContent(picked));
    assert_eq!(
        session.next().await,
        Request::StartWorld {
            world,
            save: described
        }
    );
    let _ = std::fs::remove_dir_all(save.parent().unwrap());
}
