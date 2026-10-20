//! Two players use the launcher as a browser would, over its HTTP API, each
//! with a fake game attached: connect, create and join a room, get ready,
//! start, chat and play to the end in the same world.

#![allow(clippy::unwrap_used)]

use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tpf3mp_agent::{
    TunnelChoice, Worlds,
    launcher::{Launcher, LauncherConfig},
};
use tpf3mp_net::{Identity, ServerIdentity, ServerTrust};
use tpf3mp_proto::{ContentManifest, Invite, ModRef, RoomSettings, Text};
use tpf3mp_server::{Server, ServerConfig, SnapshotConfig};
use tpf3mp_testkit::{
    fake_hook::{self, FakeHookConfig},
    scenario::toy_content,
    toy::toy_rules_menu,
};

const WAIT: Duration = Duration::from_secs(60);

/// A page's view of one launcher: where it listens and its token.
struct Page {
    address: SocketAddr,
    token: String,
}

impl Page {
    fn of(launcher: &Launcher) -> Self {
        let url = launcher.url().unwrap().strip_prefix("http://").unwrap();
        let (address, token) = url.split_once("/#").unwrap();
        Self {
            address: address.parse().unwrap(),
            token: token.to_owned(),
        }
    }

    async fn state(&self) -> Value {
        let (status, body) = request(
            self.address,
            &self.address.to_string(),
            "GET",
            "/api/state",
            Some(&self.token),
            None,
        )
        .await;
        assert_eq!(status, 200, "{body}");
        body
    }

    async fn act(&self, action: Value) -> Value {
        let (status, body) = request(
            self.address,
            &self.address.to_string(),
            "POST",
            "/api/action",
            Some(&self.token),
            Some(&action.to_string()),
        )
        .await;
        assert_eq!(status, 200, "{action} was refused: {body}");
        body
    }

    /// Waits until the page's state satisfies `done`.
    async fn wait_for(&self, what: &str, done: impl Fn(&Value) -> bool) -> Value {
        let found = tokio::time::timeout(WAIT, async {
            loop {
                let state = self.state().await;
                if done(&state) {
                    return state;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
        match found {
            Ok(state) => state,
            Err(_) => panic!(
                "timed out waiting for {what}; the page shows {}",
                self.state().await
            ),
        }
    }
}

/// One HTTP request, as a browser would send it. Returns the status code and
/// the body as JSON, or `Null` for a body that is not JSON.
async fn request(
    address: SocketAddr,
    host: &str,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> (u16, Value) {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let body = body.unwrap_or_default();
    let token = token
        .map(|token| format!("X-Launcher-Token: {token}\r\n"))
        .unwrap_or_default();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{token}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(body.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8(response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

fn launcher_config(
    root: &Path,
    name: &str,
    trust: &ServerTrust,
    identity: Arc<Identity>,
) -> LauncherConfig {
    LauncherConfig {
        diagnostics: None,
        game_logs: None,
        hook: None,
        game_exe: None,
        game_env: Vec::new(),
        start_save: None,
        listen: "127.0.0.1:0".parse().unwrap(),
        server: None,
        server_fixed: false,
        default_server: None,
        server_name: None,
        servers: Vec::new(),
        tunnel: TunnelChoice::Off,
        remember: None,
        trust: trust.clone(),
        identity,
        name: name.into(),
        content: toy_content(),
        mods: None,
        picker: None,
        native: None,
        installed: None,
        link: format!("tpf3mp-launcher-{}-{name}", std::process::id()),
        worlds: Worlds::open(&root.join(name), 1 << 30).unwrap(),
        room_settings: RoomSettings {
            scan_plan: tpf3mp_proto::ScanPlan::DEFAULT,
            steps_per_second: 100,
            input_delay_ms: 60,
            checkpoint_interval: 20,
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_players_play_a_room_from_their_launchers() {
    let root = tempfile::tempdir().unwrap();
    let identity = ServerIdentity::self_signed(&["localhost", "127.0.0.1"]).unwrap();
    let trust = ServerTrust::Pinned(identity.leaf().clone());
    let mut config = ServerConfig::new("127.0.0.1:0".parse().unwrap(), identity);
    config.rules = toy_rules_menu();
    config.tick = Duration::from_millis(25);
    config.max_sessions_per_address = 100;
    config.max_handshakes_per_address = 100;
    config.snapshots = Some(SnapshotConfig::new(root.path().join("server")));
    let server = Server::bind(config).unwrap();
    let server_address = server.local_addr().unwrap().to_string();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server_task = tokio::spawn(server.run(async {
        let _ = stopped.await;
    }));

    let ann_identity = Arc::new(Identity::generate().unwrap().0);
    let bob_identity = Arc::new(Identity::generate().unwrap().0);
    let ann_config = launcher_config(root.path(), "ann", &trust, Arc::clone(&ann_identity));
    let bob_config = launcher_config(root.path(), "bob", &trust, Arc::clone(&bob_identity));
    let hooks = [
        (ann_config.link.clone(), "ann", ann_identity.player(), 42),
        // Bob's own world differs; he plays the owner's.
        (bob_config.link.clone(), "bob", bob_identity.player(), 7),
    ]
    .map(|(link_name, player_name, player, world_seed)| {
        fake_hook::spawn(FakeHookConfig {
            link_name,
            player,
            player_name: player_name.into(),
            seed: world_seed,
            world_seed,
            act_every: 9,
            target_step: 300,
            drift_at: None,
            patience: WAIT,
            at_menu: false,
            notices: None,
        })
    });
    let ann = Launcher::start(ann_config).await.unwrap();
    let bob = Launcher::start(bob_config).await.unwrap();
    let (ann_page, bob_page) = (Page::of(&ann), Page::of(&bob));

    // Only the page with the token, naming the launcher's own address, gets
    // in.
    let (status, _) = request(
        ann_page.address,
        &ann_page.address.to_string(),
        "GET",
        "/api/state",
        None,
        None,
    )
    .await;
    assert_eq!(status, 401, "no token");
    let (status, _) = request(
        ann_page.address,
        &ann_page.address.to_string(),
        "GET",
        "/api/state",
        Some("0123456789abcdef0123456789abcdef"),
        None,
    )
    .await;
    assert_eq!(status, 401, "a wrong token");
    let (status, _) = request(
        ann_page.address,
        "evil.example:80",
        "GET",
        "/api/state",
        Some(&ann_page.token),
        None,
    )
    .await;
    assert_eq!(status, 403, "a rebound host");
    let (status, _) = request(
        ann_page.address,
        &ann_page.address.to_string(),
        "GET",
        "/",
        None,
        None,
    )
    .await;
    assert_eq!(status, 200, "the page itself holds no secret");

    ann_page
        .act(json!({ "action": "connect", "server": server_address, "name": "Ann" }))
        .await;
    ann_page
        .act(json!({
            "action": "create", "room": "table", "max_players": 4, "password": null,
            "rules": "toy",
        }))
        .await;
    let state = ann_page
        .wait_for("Ann's room", |state| state["room"]["invite"].is_string())
        .await;
    let invite = state["room"]["invite"].as_str().unwrap().to_owned();
    // The host chose among the server's rules; the room names its choice.
    let offered: Vec<&str> = state["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rules| rules["name"].as_str().unwrap())
        .collect();
    assert_eq!(offered, ["toy", "native"]);
    assert_eq!(state["room"]["rules"], "toy");
    // A launcher without a server of its own names the server with the
    // room's code.
    let (named, code) = invite.split_once(' ').unwrap();
    assert_eq!(
        named, server_address,
        "the invite names its server: {invite}"
    );
    assert!(code.parse::<Invite>().is_ok(), "{invite}");

    // Cat's game runs a mod the room does not: Cat's page says which, and
    // Ann's page shows that Cat's game differs. Cat leaves again.
    let mut cat_config = launcher_config(
        root.path(),
        "cat",
        &trust,
        Arc::new(Identity::generate().unwrap().0),
    );
    cat_config.content = ContentManifest::new(
        toy_content().game,
        vec![ModRef {
            id: Text::new("trains").unwrap(),
            version: Text::new("1.2").unwrap(),
        }],
    );
    let cat = Launcher::start(cat_config).await.unwrap();
    let cat_page = Page::of(&cat);
    cat_page
        .act(json!({ "action": "connect", "server": invite, "name": "Cat" }))
        .await;
    let state = cat_page
        .wait_for("Cat hearing how the game differs", |state| {
            state["content_diff"].is_object()
        })
        .await;
    assert_eq!(state["content_diff"]["extra"], json!(["trains 1.2"]));
    assert_eq!(
        state["content_diff"]["summary"],
        "the room lacks trains 1.2"
    );
    ann_page
        .wait_for("Cat's game marked as different", |state| {
            state["room"]["members"]
                .as_array()
                .is_some_and(|members| members.iter().any(|m| m["content"] == "differs"))
        })
        .await;
    cat_page.act(json!({ "action": "leave" })).await;
    // Out of the room, Cat's page no longer says how Cat's game differs
    // from it.
    cat_page
        .wait_for("Cat's page forgetting the room's differences", |state| {
            state["content_diff"].is_null()
        })
        .await;
    ann_page
        .wait_for("Cat gone", |state| {
            state["room"]["members"]
                .as_array()
                .is_some_and(|members| members.len() == 1)
        })
        .await;
    drop(cat);

    // Bob pastes Ann's whole invite where the server goes: connected and
    // joined in one step.
    bob_page
        .act(json!({ "action": "connect", "server": invite, "name": "Bob" }))
        .await;
    for page in [&ann_page, &bob_page] {
        page.act(json!({ "action": "ready", "ready": true })).await;
    }
    ann_page
        .wait_for("everyone ready", |state| {
            let members = state["room"]["members"].as_array();
            members.is_some_and(|members| {
                members.len() == 2 && members.iter().all(|member| member["ready"] == true)
            })
        })
        .await;
    ann_page.act(json!({ "action": "start" })).await;
    ann_page
        .act(json!({ "action": "chat", "text": "good luck" }))
        .await;
    let state = bob_page
        .wait_for("Ann's message", |state| {
            state["chat"]
                .as_array()
                .is_some_and(|chat| chat.iter().any(|line| line["text"] == "good luck"))
        })
        .await;
    let line = &state["chat"][0];
    assert_eq!(
        (line["from"].as_str(), line["you"].as_bool()),
        (Some("Ann"), Some(false))
    );

    let mut reports = Vec::new();
    for hook in hooks {
        let report = tokio::task::spawn_blocking(move || hook.join())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        reports.push(report);
    }
    assert_eq!(reports[0].lanes, reports[1].lanes, "the worlds agree");
    assert_eq!(reports[0].ran, 300);
    assert_eq!(reports[1].ran, 300);
    assert_eq!(reports[1].received, 1, "Bob played the owner's world");
    let state = bob_page.state().await;
    assert_eq!(state["room"]["phase"], "running");
    assert!(state["game"]["attached"].is_string(), "{state}");

    drop((ann, bob));
    let _ = stop.send(());
    let _ = tokio::time::timeout(Duration::from_secs(10), server_task).await;
}

/// The native window's path: the launcher in this process, driven through
/// its handle rather than a page.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_window_drives_the_launcher_in_process() {
    use tpf3mp_agent::launcher::{Action, Connection, Phase};

    let root = tempfile::tempdir().unwrap();
    let identity = ServerIdentity::self_signed(&["localhost", "127.0.0.1"]).unwrap();
    let trust = ServerTrust::Pinned(identity.leaf().clone());
    let mut config = ServerConfig::new("127.0.0.1:0".parse().unwrap(), identity);
    config.rules = toy_rules_menu();
    config.max_sessions_per_address = 100;
    config.max_handshakes_per_address = 100;
    let server = Server::bind(config).unwrap();
    let server_address = server.local_addr().unwrap().to_string();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server_task = tokio::spawn(server.run(async {
        let _ = stopped.await;
    }));

    let config = launcher_config(
        root.path(),
        "dan",
        &trust,
        Arc::new(Identity::generate().unwrap().0),
    );
    let launcher = Launcher::start_local(config);
    assert_eq!(launcher.url(), None, "no page");
    let handle = launcher.handle();
    assert_eq!(handle.state().connection, Connection::Disconnected);

    // A refusal comes back, and stays in the state until something works.
    let refused = handle
        .act(Action::Connect {
            server: server_address.clone(),
            name: String::new(),
        })
        .await;
    assert!(refused.is_err());
    assert!(handle.state().error.is_some());

    // The game may start before a room is chosen (D17): here it is refused
    // only because this launcher has no game to start.
    let refused = handle.act(Action::LaunchGame).await.unwrap_err();
    assert!(!refused.contains("room"), "{refused}");

    handle
        .act(Action::Connect {
            server: server_address,
            name: "Dan".into(),
        })
        .await
        .unwrap();
    let state = handle.state();
    assert_eq!(state.connection, Connection::Connected);
    assert_eq!(state.error, None, "cleared by the connection that worked");
    assert!(
        state
            .support_id
            .as_deref()
            .is_some_and(|id| id.parse::<tpf3mp_proto::Code>().is_ok()),
        "{state:?}"
    );
    assert_eq!(state.rules[0].name, "toy");

    handle
        .act(Action::Create {
            room: "window room".into(),
            max_players: 2,
            password: None,
            rules: None,
            start_save: None,
            listing: None,
            competitive: false,
        })
        .await
        .unwrap();
    let room = handle.state().room.unwrap();
    assert_eq!(room.name, "window room");
    assert_eq!(room.phase, Phase::Lobby);
    assert!(room.you_own);
    assert!(room.invite.is_some());

    drop(launcher);
    let _ = stop.send(());
    let _ = tokio::time::timeout(Duration::from_secs(10), server_task).await;
}

/// After leaving a room the launcher connects again on its own; that
/// connection declares the game's content too, so the next room it makes
/// can start. Without it the server holds no content for the player and
/// refuses every start as "different game versions or mods" (found in the
/// two-instance playtest of 2026-09-29).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_room_made_after_leaving_one_can_start() {
    use tpf3mp_agent::launcher::{Action, Connection, MemberContent, Phase};

    let root = tempfile::tempdir().unwrap();
    let identity = ServerIdentity::self_signed(&["localhost", "127.0.0.1"]).unwrap();
    let trust = ServerTrust::Pinned(identity.leaf().clone());
    let mut config = ServerConfig::new("127.0.0.1:0".parse().unwrap(), identity);
    config.rules = toy_rules_menu();
    config.max_sessions_per_address = 100;
    config.max_handshakes_per_address = 100;
    let server = Server::bind(config).unwrap();
    let server_address = server.local_addr().unwrap().to_string();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server_task = tokio::spawn(server.run(async {
        let _ = stopped.await;
    }));

    let config = launcher_config(
        root.path(),
        "gus",
        &trust,
        Arc::new(Identity::generate().unwrap().0),
    );
    let launcher = Launcher::start_local(config);
    let handle = launcher.handle();
    handle
        .act(Action::Connect {
            server: server_address,
            name: "Gus".into(),
        })
        .await
        .unwrap();
    let create = || Action::Create {
        room: "again".into(),
        max_players: 1,
        password: None,
        rules: None,
        start_save: None,
        listing: None,
        competitive: false,
    };
    handle.act(create()).await.unwrap();
    handle.act(Action::Leave).await.unwrap();
    // The session hands its connection back and the launcher connects again.
    tokio::time::timeout(WAIT, async {
        while handle.state().room.is_some() || handle.state().connection != Connection::Connected {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("connected again after leaving");

    handle.act(create()).await.unwrap();
    handle.act(Action::Ready { ready: true }).await.unwrap();
    let me = tokio::time::timeout(WAIT, async {
        loop {
            let me = handle
                .state()
                .room
                .and_then(|room| room.members.into_iter().find(|member| member.you));
            if let Some(me) = me.filter(|me| me.ready) {
                return me;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("ready in the new room");
    assert_eq!(
        me.content,
        MemberContent::Same,
        "the new connection declared no content"
    );
    handle.act(Action::Start).await.unwrap();
    let started = tokio::time::timeout(WAIT, async {
        loop {
            let state = handle.state();
            if state
                .room
                .as_ref()
                .is_some_and(|room| room.phase != Phase::Lobby)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        started.is_ok(),
        "the room never started; notices: {:?}",
        handle.state().notices
    );

    drop(launcher);
    let _ = stop.send(());
    let _ = tokio::time::timeout(Duration::from_secs(10), server_task).await;
}

/// A launcher plays on its server (D12): Connect takes no server, and an
/// invite to another is refused, not followed. Only the player's server
/// setting changes the server (D12, as amended): it reconnects there, is
/// remembered, and goes back to the default.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_launcher_with_its_own_server_plays_there_alone() {
    use tpf3mp_agent::launcher::{Action, Connection};

    let root = tempfile::tempdir().unwrap();
    let identity = ServerIdentity::self_signed(&["localhost", "127.0.0.1"]).unwrap();
    let trust = ServerTrust::Pinned(identity.leaf().clone());
    let mut config = ServerConfig::new("127.0.0.1:0".parse().unwrap(), identity);
    config.rules = toy_rules_menu();
    config.max_sessions_per_address = 100;
    config.max_handshakes_per_address = 100;
    let server = Server::bind(config).unwrap();
    let server_address = server.local_addr().unwrap().to_string();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server_task = tokio::spawn(server.run(async {
        let _ = stopped.await;
    }));

    let mut config = launcher_config(
        root.path(),
        "eve",
        &trust,
        Arc::new(Identity::generate().unwrap().0),
    );
    config.server = Some(server_address.clone());
    config.server_fixed = true;
    config.default_server = Some(server_address.clone());
    let remember = root.path().join("launcher.json");
    config.remember = Some(remember.clone());
    let launcher = Launcher::start_local(config);
    let handle = launcher.handle();
    let state = handle.state();
    assert!(state.server_fixed);
    assert_eq!(state.server.as_deref(), Some(server_address.as_str()));

    // Words that are no invite are not taken for a server.
    let refused = handle
        .act(Action::Connect {
            server: "elsewhere.example:29470".into(),
            name: "Eve".into(),
        })
        .await
        .unwrap_err();
    assert!(refused.contains("not an invite"), "{refused}");
    assert_eq!(handle.state().connection, Connection::Disconnected);

    // Nothing typed: the launcher's own server.
    handle
        .act(Action::Connect {
            server: String::new(),
            name: "Eve".into(),
        })
        .await
        .unwrap();
    assert_eq!(handle.state().connection, Connection::Connected);

    handle
        .act(Action::Create {
            room: "own server".into(),
            max_players: 2,
            password: None,
            rules: None,
            start_save: None,
            listing: None,
            competitive: false,
        })
        .await
        .unwrap();
    // With a server of its own, the invite is the room's code alone.
    let invite = handle.state().room.unwrap().invite.unwrap();
    assert!(invite.parse::<Invite>().is_ok(), "{invite}");
    handle.act(Action::Leave).await.unwrap();

    // The same room's invite, sent from another server, is not followed.
    let code = &invite;
    let foreign = format!("elsewhere.example:29470 {code}");
    let refused = handle
        .act(Action::Join {
            invite: foreign.clone(),
            server: None,
            password: None,
        })
        .await
        .unwrap_err();
    assert!(refused.contains("another server"), "{refused}");
    let refused = handle
        .act(Action::Connect {
            server: foreign,
            name: "Eve".into(),
        })
        .await
        .unwrap_err();
    assert!(refused.contains("another server"), "{refused}");

    // Back on the server once the room is left.
    let back = tokio::time::timeout(WAIT, async {
        while handle.state().room.is_some() || handle.state().connection != Connection::Connected {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(back.is_ok(), "never back on the server after leaving");

    // The server setting takes a host:port only.
    let refused = handle
        .act(Action::SetServer {
            server: "elsewhere.example".into(),
        })
        .await
        .unwrap_err();
    assert!(refused.contains("host:port"), "{refused}");
    assert_eq!(
        handle.state().server.as_deref(),
        Some(server_address.as_str())
    );

    // Another server (the same one by name): the launcher leaves and
    // connects there, and remembers it.
    let port = server_address.rsplit_once(':').unwrap().1;
    let other = format!("localhost:{port}");
    handle
        .act(Action::SetServer {
            server: other.clone(),
        })
        .await
        .unwrap();
    let state = handle.state();
    assert_eq!(state.server.as_deref(), Some(other.as_str()));
    assert_eq!(state.connection, Connection::Connected, "reconnected there");
    assert_eq!(
        state.server_default.as_deref(),
        Some(server_address.as_str())
    );
    let remembered = tpf3mp_agent::launcher::Remembered::load(&remember);
    assert_eq!(remembered.chosen_server.as_deref(), Some(other.as_str()));
    // Invites still never switch servers: one naming the default is refused
    // now.
    let refused = handle
        .act(Action::Join {
            invite: format!("{server_address} {code}"),
            server: None,
            password: None,
        })
        .await
        .unwrap_err();
    assert!(refused.contains("another server"), "{refused}");

    // Reset to default.
    handle
        .act(Action::SetServer {
            server: String::new(),
        })
        .await
        .unwrap();
    let state = handle.state();
    assert_eq!(state.server.as_deref(), Some(server_address.as_str()));
    assert_eq!(state.connection, Connection::Connected);
    let remembered = tpf3mp_agent::launcher::Remembered::load(&remember);
    assert_eq!(remembered.chosen_server, None);

    drop(launcher);
    let _ = stop.send(());
    let _ = tokio::time::timeout(Duration::from_secs(10), server_task).await;
}

/// The game's main menu drives the launcher (D17): a game at its menu,
/// before any room, connects, creates a room, chats, gets ready and starts
/// it from its Multiplayer window, and the launcher's own window shows the
/// same room.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_game_at_its_main_menu_plays_the_lobby_through_the_launcher() {
    use tpf3mp_agent::launcher::{Connection, Phase};
    use tpf3mp_bridge::{LobbyAction, LobbyConnection, LobbyView, Session};

    let root = tempfile::tempdir().unwrap();
    let identity = ServerIdentity::self_signed(&["localhost", "127.0.0.1"]).unwrap();
    let trust = ServerTrust::Pinned(identity.leaf().clone());
    let mut config = ServerConfig::new("127.0.0.1:0".parse().unwrap(), identity);
    config.rules = toy_rules_menu();
    config.max_sessions_per_address = 100;
    config.max_handshakes_per_address = 100;
    let server = Server::bind(config).unwrap();
    let server_address = server.local_addr().unwrap().to_string();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server_task = tokio::spawn(server.run(async {
        let _ = stopped.await;
    }));

    let mut config = launcher_config(
        root.path(),
        "fay",
        &trust,
        Arc::new(Identity::generate().unwrap().0),
    );
    // The server the launcher plays on: the window names none (D12).
    config.server = Some(server_address);
    let link_name = config.link.clone();
    let launcher = Launcher::start_local(config);
    let handle = launcher.handle();

    // The game, at its main menu: nothing but the menu's window reads the
    // link, as the hook's menu entry does (crates/tpf3mp-hook/src/lobby.rs).
    let game = tokio::task::spawn_blocking(move || {
        let mut session = Session::attach(&link_name, "menu test", WAIT).unwrap();
        let mut shown = LobbyView::default();
        let mut wait_for =
            |session: &mut Session, what: &str, done: &dyn Fn(&LobbyView) -> bool| {
                let deadline = std::time::Instant::now() + WAIT;
                loop {
                    session.poll_lobby().unwrap();
                    if let Some(view) = session.take_lobby() {
                        shown = view;
                    }
                    if done(&shown) {
                        return shown.clone();
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the window never showed {what}: {shown:?}"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            };
        let act = |session: &mut Session, action: LobbyAction| session.lobby_act(action).unwrap();

        wait_for(&mut session, "the launcher's lobby", &|view| {
            view.connection == LobbyConnection::Disconnected && view.name.as_str() == "fay"
        });
        act(
            &mut session,
            LobbyAction::Connect {
                name: Text::new("Fay").unwrap(),
            },
        );
        wait_for(&mut session, "the connection", &|view| {
            view.connection == LobbyConnection::Connected
        });
        act(
            &mut session,
            LobbyAction::Create {
                room: Text::new("menu room").unwrap(),
                max_players: 2,
                password: None,
                rules: None,
                start_save: None,
                listing: None,
                competitive: false,
            },
        );
        let view = wait_for(&mut session, "the room", &|view| view.room.is_some());
        let room = view.room.unwrap();
        assert_eq!(room.name.as_str(), "menu room");
        assert!(room.you_own && !room.running);
        assert!(room.invite.is_some());
        act(
            &mut session,
            LobbyAction::Chat {
                text: Text::new("hello from the menu").unwrap(),
            },
        );
        wait_for(&mut session, "the chat", &|view| {
            view.chat
                .iter()
                .any(|line| line.you && line.text.as_str() == "hello from the menu")
        });
        act(&mut session, LobbyAction::Ready { ready: true });
        wait_for(&mut session, "the player ready", &|view| {
            view.room
                .as_ref()
                .is_some_and(|room| room.members.iter().any(|member| member.you && member.ready))
        });
        act(&mut session, LobbyAction::Start);
        // The room's game begins: the game takes it at its gate.
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            session.poll_lobby().unwrap();
            if let Some(begin) = session.try_begin().unwrap() {
                return begin;
            }
            assert!(std::time::Instant::now() < deadline, "the room never began");
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    let begin = tokio::time::timeout(WAIT * 2, game).await.unwrap().unwrap();
    assert_eq!(begin.rules.as_str(), "toy");

    // The launcher's own window saw it all.
    let state = handle.state();
    assert_eq!(state.connection, Connection::Connected);
    assert_eq!(state.game.attached.as_deref(), Some("menu test"));
    let room = state.room.unwrap();
    assert_eq!(room.name, "menu room");
    assert_eq!(room.phase, Phase::Running);
    assert!(
        state
            .chat
            .iter()
            .any(|line| line.you && line.text == "hello from the menu")
    );

    drop(launcher);
    let _ = stop.send(());
    let _ = tokio::time::timeout(Duration::from_secs(10), server_task).await;
}

/// A guest's launcher that found its own mods (docs/MODS.md, "Choosing
/// mods"): it chooses its personal mods, remembers them, and learns the
/// room's shared mods from what the room says it lacks, declaring those it
/// has, so that the room starts though the guest runs a mod the owner does
/// not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guest_with_its_own_mods_learns_the_rooms_and_the_room_starts() {
    use tpf3mp_agent::{
        launcher::{Action, MemberContent, ModHave, Phase},
        picker::{Installed, Mods},
    };
    use tpf3mp_modscan::Class;

    let root = tempfile::tempdir().unwrap();
    let identity = ServerIdentity::self_signed(&["localhost", "127.0.0.1"]).unwrap();
    let trust = ServerTrust::Pinned(identity.leaf().clone());
    let mut config = ServerConfig::new("127.0.0.1:0".parse().unwrap(), identity);
    config.rules = toy_rules_menu();
    config.max_sessions_per_address = 100;
    config.max_handshakes_per_address = 100;
    let server = Server::bind(config).unwrap();
    let server_address = server.local_addr().unwrap().to_string();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server_task = tokio::spawn(server.run(async {
        let _ = stopped.await;
    }));

    let listed = |id: &str, version: &str| ModRef {
        id: Text::new(id).unwrap(),
        version: Text::new(version).unwrap(),
    };
    // The owner declares the room's shared mods as its start save made them.
    let mut ann = launcher_config(
        root.path(),
        "cora",
        &trust,
        Arc::new(Identity::generate().unwrap().0),
    );
    ann.content = ContentManifest::new(
        toy_content().game,
        vec![listed("vehicles_pack", "3"), listed("tpf3mp_1", "1")],
    );
    // The guest found its mods: the room's two, and a minimap of its own.
    let mut gus = launcher_config(
        root.path(),
        "hal",
        &trust,
        Arc::new(Identity::generate().unwrap().0),
    );
    let remember = root.path().join("hal-launcher.json");
    gus.remember = Some(remember.clone());
    let mod_of = |id: &str, class: Class, version: &str| Installed {
        id: id.into(),
        name: id.into(),
        version: version.into(),
        class,
        reason: String::new(),
        path: root.path().join(id),
        hub: None,
    };
    gus.picker = Some(Mods::new(
        toy_content().game,
        vec![
            mod_of("tpf3mp_1", Class::Shared, "1"),
            mod_of("vehicles_pack", Class::Shared, "3"),
            mod_of("schbrongx_minimap", Class::Personal, "1"),
        ],
        [],
        false,
    ));
    let ann = Launcher::start_local(ann);
    let gus = Launcher::start_local(gus);
    let (ann_handle, gus_handle) = (ann.handle(), gus.handle());

    // Choosing: a personal mod yes, a shared one never; remembered.
    gus_handle
        .act(Action::ChooseMod {
            id: "schbrongx_minimap".into(),
            chosen: true,
        })
        .await
        .unwrap();
    assert!(
        gus_handle
            .act(Action::ChooseMod {
                id: "vehicles_pack".into(),
                chosen: true
            })
            .await
            .is_err()
    );
    let rows = gus_handle.state().mods;
    assert_eq!(rows[0].id, "schbrongx_minimap", "the choosable first");
    assert!(rows[0].chosen && rows[0].choosable);
    assert!(!rows[1].choosable);
    let remembered = std::fs::read_to_string(&remember).unwrap();
    assert!(remembered.contains("schbrongx_minimap"), "{remembered}");

    for (handle, name) in [(&ann_handle, "Ann"), (&gus_handle, "Gus")] {
        handle
            .act(Action::Connect {
                server: server_address.clone(),
                name: name.into(),
            })
            .await
            .unwrap();
    }
    ann_handle
        .act(Action::Create {
            room: "mods".into(),
            max_players: 2,
            password: None,
            rules: None,
            start_save: None,
            listing: None,
            competitive: false,
        })
        .await
        .unwrap();
    let invite = ann_handle.state().room.unwrap().invite.unwrap();
    gus_handle
        .act(Action::Join {
            invite,
            server: None,
            password: None,
        })
        .await
        .unwrap();
    // The room tells the guest what it lacks; the guest declares what it
    // has of it, and matches the owner.
    let learned = tokio::time::timeout(WAIT, async {
        loop {
            let state = gus_handle.state();
            let same = state.room.as_ref().is_some_and(|room| {
                room.members
                    .iter()
                    .find(|member| member.you)
                    .is_some_and(|me| me.content == MemberContent::Same)
            });
            if same {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the guest matches the owner");
    let room_mods: Vec<(String, ModHave)> = learned
        .room_mods
        .iter()
        .map(|m| (m.id.clone(), m.have))
        .collect();
    assert_eq!(
        room_mods,
        [
            ("vehicles_pack".to_owned(), ModHave::Yes),
            ("tpf3mp_1".to_owned(), ModHave::Yes)
        ]
    );

    for handle in [&ann_handle, &gus_handle] {
        handle.act(Action::Ready { ready: true }).await.unwrap();
    }
    let started = tokio::time::timeout(WAIT, async {
        loop {
            // Readiness takes a moment to reach the room.
            let _ = ann_handle.act(Action::Start).await;
            if ann_handle
                .state()
                .room
                .is_some_and(|room| room.phase != Phase::Lobby)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(
        started.is_ok(),
        "the room never started; notices: {:?}",
        ann_handle.state().notices
    );

    drop((ann, gus));
    let _ = stop.send(());
    let _ = tokio::time::timeout(Duration::from_secs(10), server_task).await;
}
