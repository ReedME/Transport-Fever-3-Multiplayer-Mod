//! A release with several servers (docs/DECISIONS.md, D12's approved
//! amendment): launchers play on all of them. Rooms are hosted on the closest
//! server that answers, every server's public rooms are listed together, and
//! a six-character invite is resolved against all regions before a unique
//! credential match can be joined.

#![allow(clippy::unwrap_used)]

use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tpf3mp_agent::{
    TunnelChoice, Worlds,
    launcher::{Launcher, LauncherConfig, ListedServer},
};
use tpf3mp_net::{Identity, ServerIdentity, ServerTrust};
use tpf3mp_proto::RoomSettings;
use tpf3mp_server::{Server, ServerConfig};
use tpf3mp_testkit::{scenario::toy_content, toy::toy_rules_menu};

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
        let (status, body) = self.request("GET", "/api/state", None).await;
        assert_eq!(status, 200, "{body}");
        body
    }

    /// Sends `action`; the status and the answer.
    async fn try_act(&self, action: &Value) -> (u16, Value) {
        self.request("POST", "/api/action", Some(&action.to_string()))
            .await
    }

    async fn act(&self, action: Value) {
        let (status, body) = self.try_act(&action).await;
        assert_eq!(status, 200, "{action} was refused: {body}");
    }

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

    async fn request(&self, method: &str, path: &str, body: Option<&str>) -> (u16, Value) {
        let mut stream = TcpStream::connect(self.address).await.unwrap();
        let body = body.unwrap_or_default();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nX-Launcher-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.address,
            self.token,
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
}

/// A launcher of a release that lists `servers`, the first its default.
fn launcher_config(
    root: &Path,
    name: &str,
    trust: &ServerTrust,
    servers: &[(&str, &str)],
) -> LauncherConfig {
    let servers: Vec<ListedServer> = servers
        .iter()
        .map(|(name, address)| ListedServer {
            name: (*name).to_owned(),
            address: (*address).to_owned(),
        })
        .collect();
    LauncherConfig {
        diagnostics: None,
        game_logs: None,
        hook: None,
        game_exe: None,
        game_env: Vec::new(),
        start_save: None,
        listen: "127.0.0.1:0".parse().unwrap(),
        server: Some(servers[0].address.clone()),
        server_fixed: true,
        default_server: Some(servers[0].address.clone()),
        server_name: Some(servers[0].name.clone()),
        servers,
        tunnel: TunnelChoice::Off,
        remember: None,
        trust: trust.clone(),
        identity: Arc::new(Identity::generate().unwrap().0),
        name: name.into(),
        content: toy_content(),
        mods: None,
        picker: None,
        native: None,
        installed: None,
        link: format!("tpf3mp-regional-{}-{name}", std::process::id()),
        worlds: Worlds::open(&root.join(name), 1 << 30).unwrap(),
        room_settings: RoomSettings {
            scan_plan: tpf3mp_proto::ScanPlan::DEFAULT,
            steps_per_second: 100,
            input_delay_ms: 60,
            checkpoint_interval: 20,
        },
    }
}

/// Starts a server on a free port with `identity`; its address and the
/// sender that stops it.
fn start_server(identity: ServerIdentity) -> (String, tokio::sync::oneshot::Sender<()>) {
    start_server_with_session_limit(identity, Some(100))
}

/// Starts a server with the configured session limit, or the runtime default
/// when `limit` is `None`.
fn start_server_with_session_limit(
    identity: ServerIdentity,
    limit: Option<usize>,
) -> (String, tokio::sync::oneshot::Sender<()>) {
    let mut config = ServerConfig::new("127.0.0.1:0".parse().unwrap(), identity);
    config.rules = toy_rules_menu();
    config.tick = Duration::from_millis(25);
    if let Some(limit) = limit {
        config.max_sessions_per_address = limit;
    }
    config.max_handshakes_per_address = 100;
    let server = Server::bind(config).unwrap();
    let address = server.local_addr().unwrap().to_string();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(server.run(async {
        let _ = stopped.await;
    }));
    (address, stop)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rooms_go_to_the_closest_server_and_are_found_on_every_one() {
    let root = tempfile::tempdir().unwrap();
    // Both servers show the same certificate, which the launchers pin.
    let identity = ServerIdentity::self_signed(&["localhost", "127.0.0.1"]).unwrap();
    let trust = ServerTrust::Pinned(identity.leaf().clone());
    let (eu, _stop_eu) = start_server(identity.clone());
    let (us, _stop_us) = start_server(identity);
    // A server that never answers: nothing listens there.
    let silent = std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();

    // Ann's release lists a server that does not answer first: she plays,
    // and hosts, on the one that does.
    let ann = Launcher::start(launcher_config(
        root.path(),
        "ann",
        &trust,
        &[("EU", &silent), ("US", &us)],
    ))
    .await
    .unwrap();
    let ann_page = Page::of(&ann);
    ann_page
        .act(json!({ "action": "connect", "server": "", "name": "Ann" }))
        .await;
    let state = ann_page.state().await;
    assert_eq!(state["server"], us, "the server that answers");
    assert_eq!(state["server_name"], "US");
    ann_page
        .act(json!({
            "action": "create", "room": "transatlantic", "max_players": 4, "password": null,
            "listing": { "map": "dry", "year": 1850 },
        }))
        .await;
    let state = ann_page
        .wait_for("Ann's room", |state| state["room"]["invite"].is_string())
        .await;
    let invite = state["room"]["invite"].as_str().unwrap().to_owned();
    assert!(
        !invite.contains(' '),
        "a code alone, as with one server: {invite}"
    );
    assert_eq!(state["server"], us);
    let rows = state["servers"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{state}");
    assert_eq!(rows[0]["reachable"], false);
    assert_eq!(rows[1]["here"], true);

    // Bob plays on EU, the first of two equally close servers, and joins
    // Ann's room by its code alone: EU has no such room, US has.
    let bob = Launcher::start(launcher_config(
        root.path(),
        "bob",
        &trust,
        &[("EU", &eu), ("US", &us)],
    ))
    .await
    .unwrap();
    let bob_page = Page::of(&bob);
    bob_page
        // The game's lobby echoes the current trusted address when it asks
        // Connect; the address is context, while the release still chooses
        // the closest primary server itself.
        .act(json!({ "action": "connect", "server": eu.clone(), "name": "Bob" }))
        .await;
    assert_eq!(bob_page.state().await["server"], eu, "ties go to the first");
    bob_page
        .wait_for("Bob's lookout on US", |state| {
            state["servers"]
                .as_array()
                .is_some_and(|rows| rows.len() == 2 && rows[1]["reachable"] == true)
        })
        .await;
    bob_page
        .act(json!({ "action": "join", "invite": invite, "password": null }))
        .await;
    let state = bob_page
        .wait_for("Bob in Ann's room", |state| state["room"].is_object())
        .await;
    assert_eq!(state["server"], us, "joined where the room is");
    assert_eq!(state["room"]["invite"], invite.as_str());

    // Cat lists the rooms of both servers, each with its server, and joins
    // from the listed room card, whose region is explicit and trusted.
    let cat = Launcher::start(launcher_config(
        root.path(),
        "cat",
        &trust,
        &[("EU", &eu), ("US", &us)],
    ))
    .await
    .unwrap();
    let cat_page = Page::of(&cat);
    cat_page
        // Connect to the release's default; the US lookout may still be
        // starting when the immediate room-list request arrives.
        .act(json!({ "action": "connect", "server": "", "name": "Cat" }))
        .await;
    cat_page
        .act(json!({ "action": "list_rooms", "page": 0 }))
        .await;
    let state = cat_page.state().await;
    let rooms = state["rooms"]["rooms"].as_array().unwrap();
    assert_eq!(
        rooms.len(),
        1,
        "a connecting region is not silently omitted: {state}"
    );
    assert_eq!(rooms[0]["name"], "transatlantic");
    assert_eq!(rooms[0]["server"], "US");

    assert!(rooms[0]["ping_ms"].is_u64(), "{state}");
    cat_page
        .act(json!({
            "action": "join",
            "invite": rooms[0]["invite"],
            "server": rooms[0]["server"],
            "password": null
        }))
        .await;
    let state = cat_page
        .wait_for("Cat in Ann's room", |state| state["room"].is_object())
        .await;
    assert_eq!(state["server"], us);
    ann_page
        .wait_for("all three in the room", |state| {
            state["room"]["members"]
                .as_array()
                .is_some_and(|members| members.len() == 3)
        })
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pasted_server_address_does_not_route_a_typed_invite() {
    let root = tempfile::tempdir().unwrap();
    let identity = ServerIdentity::self_signed(&["localhost", "127.0.0.1"]).unwrap();
    let trust = ServerTrust::Pinned(identity.leaf().clone());
    let (eu, _stop_eu) = start_server(identity.clone());
    let (us, _stop_us) = start_server(identity);

    let host = Launcher::start(launcher_config(
        root.path(),
        "prefix-host",
        &trust,
        &[("US", &us)],
    ))
    .await
    .unwrap();
    let host_page = Page::of(&host);
    host_page
        .act(json!({ "action": "connect", "server": "", "name": "Host" }))
        .await;
    host_page
        .act(json!({
            "action": "create", "room": "trusted room", "max_players": 4, "password": null,
            "listing": { "map": "dry", "year": 1850 },
        }))
        .await;
    let invite = host_page
        .wait_for("the room invite", |state| {
            state["room"]["invite"].is_string()
        })
        .await["room"]["invite"]
        .as_str()
        .unwrap()
        .to_owned();

    let guest = Launcher::start(launcher_config(
        root.path(),
        "prefix-guest",
        &trust,
        &[("EU", &eu), ("US", &us)],
    ))
    .await
    .unwrap();
    let guest_page = Page::of(&guest);
    guest_page
        .act(json!({ "action": "connect", "server": "", "name": "Guest" }))
        .await;
    assert_eq!(guest_page.state().await["server"], eu);

    // A server-looking prefix in copied chat text is not route authority;
    // the six-character credential is resolved only among compiled regions.
    guest_page
        .act(json!({
            "action": "connect",
            "server": format!("evil.example.org:29470 {invite}"),
            "name": "Guest",
        }))
        .await;
    let joined = guest_page
        .wait_for("the trusted region room", |state| {
            state["server"] == us && state["room"].is_object()
        })
        .await;
    assert_eq!(joined["room"]["invite"], invite);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_private_invite_requires_its_password_without_switching_regions() {
    let root = tempfile::tempdir().unwrap();
    let identity = ServerIdentity::self_signed(&["localhost", "127.0.0.1"]).unwrap();
    let trust = ServerTrust::Pinned(identity.leaf().clone());
    let (eu, _stop_eu) = start_server(identity.clone());
    let (us, _stop_us) = start_server(identity);

    // A one-server launcher creates a private room on EU. Its invite never
    // appears in a public room list, and the code is shared out of band.
    let host = Launcher::start(launcher_config(
        root.path(),
        "private-host",
        &trust,
        &[("EU", &eu)],
    ))
    .await
    .unwrap();
    let host_page = Page::of(&host);
    host_page
        .act(json!({ "action": "connect", "server": "", "name": "Host" }))
        .await;
    host_page
        .act(json!({
            "action": "create", "room": "private room", "max_players": 4,
            "password": "correct horse", "listing": null,
        }))
        .await;
    let invite = host_page
        .wait_for("the private invite", |state| {
            state["room"]["invite"].is_string()
        })
        .await["room"]["invite"]
        .as_str()
        .unwrap()
        .to_owned();

    let guest = Launcher::start(launcher_config(
        root.path(),
        "private-guest",
        &trust,
        &[("US", &us), ("EU", &eu)],
    ))
    .await
    .unwrap();
    let guest_page = Page::of(&guest);
    guest_page
        .act(json!({ "action": "connect", "server": "", "name": "Guest" }))
        .await;
    assert_eq!(guest_page.state().await["server"], us);
    guest_page
        .wait_for("the EU lookout", |state| {
            state["servers"].as_array().is_some_and(|rows| {
                rows.iter()
                    .any(|row| row["name"] == "EU" && row["reachable"] == true)
            })
        })
        .await;

    let (wrong_status, wrong) = guest_page
        .try_act(&json!({
            "action": "join", "invite": invite, "password": "wrong password",
        }))
        .await;
    let (absent_status, absent) = guest_page
        .try_act(&json!({
            "action": "join", "invite": "A2BCDE", "password": "wrong password",
        }))
        .await;
    assert_eq!(wrong_status, 409, "{wrong}");
    assert_eq!(
        wrong, absent,
        "a hidden private room and bad password reveal no distinction"
    );
    assert_eq!(absent_status, wrong_status);
    let after_wrong = guest_page.state().await;
    assert_eq!(
        after_wrong["server"], us,
        "no region promotion before a unique match"
    );
    assert!(after_wrong["room"].is_null(), "ResolveInvite never joins");

    guest_page
        .act(json!({
            "action": "join", "invite": invite, "password": "correct horse",
        }))
        .await;
    let joined = guest_page
        .wait_for("the private room after credential resolution", |state| {
            state["server"] == eu && state["room"].is_object()
        })
        .await;
    assert_eq!(joined["room"]["invite"], invite);
    assert_eq!(joined["room"]["name"], "private room");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_room_list_refuses_to_hide_a_region_that_is_still_connecting() {
    let root = tempfile::tempdir().unwrap();
    let identity = ServerIdentity::self_signed(&["localhost", "127.0.0.1"]).unwrap();
    let trust = ServerTrust::Pinned(identity.leaf().clone());
    let (eu, _stop_eu) = start_server(identity);
    // Keep a UDP socket bound but unread so the lookout stays Connecting
    // throughout its bounded probe instead of receiving a quick refusal.
    let silent_socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let silent = silent_socket.local_addr().unwrap().to_string();

    let launcher = Launcher::start(launcher_config(
        root.path(),
        "partial-list",
        &trust,
        &[("EU", &eu), ("US", &silent)],
    ))
    .await
    .unwrap();
    let page = Page::of(&launcher);
    page.act(json!({ "action": "connect", "server": "", "name": "Player" }))
        .await;

    let (status, body) = page
        .try_act(&json!({ "action": "list_rooms", "page": 0 }))
        .await;
    assert_eq!(status, 409, "a partial list must be refused: {body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|error| error.contains("incomplete") && error.contains("retry")),
        "the response must explain that the list is incomplete and retryable: {body}"
    );

    // A bare code must refuse to choose EU while US is unchecked, even if
    // EU can already answer a generic miss. No primary switch is allowed.
    let (status, body) = page
        .try_act(&json!({ "action": "join", "invite": "A2BCDE", "password": null }))
        .await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("incomplete"),
        "{body}"
    );
    assert_eq!(page.state().await["server"], eu);

    // An explicit trusted region is a user-selected route. It may be joined
    // directly even when another configured region is unavailable.
    let started = tokio::time::Instant::now();
    let (status, body) = page
        .try_act(&json!({
            "action": "join", "invite": "A2BCDE", "server": "EU", "password": null,
        }))
        .await;
    assert_eq!(status, 409, "{body}");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "explicit route probed the unavailable region"
    );
    assert!(
        !body["error"].as_str().unwrap().contains("incomplete"),
        "{body}"
    );
    assert_eq!(page.state().await["server"], eu);
    drop(silent_socket);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn promoting_a_room_card_releases_the_lookout_slot_at_the_default_limit() {
    let root = tempfile::tempdir().unwrap();
    let identity = ServerIdentity::self_signed(&["localhost", "127.0.0.1"]).unwrap();
    let trust = ServerTrust::Pinned(identity.leaf().clone());
    let (eu, _stop_eu) = start_server(identity.clone());
    // Keep the server's actual default: eight sessions per peer address.
    let (us, _stop_us) = start_server_with_session_limit(identity, None);

    let host = Launcher::start(launcher_config(root.path(), "host", &trust, &[("US", &us)]))
        .await
        .unwrap();
    let host_page = Page::of(&host);
    host_page
        .act(json!({ "action": "connect", "server": "", "name": "Host" }))
        .await;
    host_page
        .act(json!({
            "action": "create", "room": "full address", "max_players": 8, "password": null,
            "listing": { "map": "dry", "year": 1850 },
        }))
        .await;
    host_page
        .wait_for("the listed room", |state| {
            state["room"]["invite"].is_string()
        })
        .await;

    // One host and seven regional lookouts fill the US server's eight
    // sessions for this loopback address. Joining from one lookout must
    // release that slot before opening its primary US connection.
    let mut players = Vec::new();
    for index in 0..7 {
        // Both servers run on loopback. A busy CI runner can make US more
        // than the tie margin faster, so keep only players whose primary is
        // EU; otherwise US holds a primary connection, not a lookout slot.
        for attempt in 0..20 {
            let name = format!("player-{index}-{attempt}");
            let launcher = Launcher::start(launcher_config(
                root.path(),
                &name,
                &trust,
                &[("EU", &eu), ("US", &us)],
            ))
            .await
            .unwrap();
            let page = Page::of(&launcher);
            page.act(json!({ "action": "connect", "server": "", "name": name }))
                .await;
            if page.state().await["server"] == eu {
                page.wait_for("its US lookout", |state| {
                    state["servers"].as_array().is_some_and(|rows| {
                        rows.len() == 2 && rows[0]["here"] == true && rows[1]["reachable"] == true
                    })
                })
                .await;
                players.push((launcher, page));
                break;
            }
            page.act(json!({ "action": "disconnect" })).await;
            if attempt == 19 {
                panic!("could not establish seven EU primaries with US lookouts");
            }
        }
    }

    players[0]
        .1
        .act(json!({ "action": "list_rooms", "page": 0 }))
        .await;
    let listed = players[0]
        .1
        .wait_for("the room card on US", |state| {
            state["rooms"]["rooms"].as_array().is_some_and(|rooms| {
                rooms
                    .iter()
                    .any(|room| room["name"] == "full address" && room["server"] == "US")
            })
        })
        .await;
    let card = listed["rooms"]["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|room| room["name"] == "full address")
        .unwrap();
    let invite = card["invite"].as_str().unwrap().to_owned();
    let region = card["server"].as_str().unwrap().to_owned();
    players[0]
        .1
        .act(json!({ "action": "join", "invite": invite, "server": region, "password": null }))
        .await;
    let joined = players[0]
        .1
        .wait_for("the promoted US connection joining", |state| {
            state["server"] == us && state["room"].is_object()
        })
        .await;
    assert_eq!(joined["room"]["invite"], invite);
    host_page
        .wait_for("the player's room membership", |state| {
            state["room"]["members"]
                .as_array()
                .is_some_and(|members| members.len() == 2)
        })
        .await;
}
