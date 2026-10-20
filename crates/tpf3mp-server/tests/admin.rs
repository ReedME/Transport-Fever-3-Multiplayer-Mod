//! The operator's admin endpoint finds rooms: all open now, the one an
//! invite opens (in either case), those a player is in or with a member of
//! a name; each with its members, by player ID and name, kept current. An
//! invite opening no room is not found, a malformed one is refused, and so
//! are two filters at once.

#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use common::{FAST, RunningServer, WAIT, join, room};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tpf3mp_proto::{Code, Invite};
use tpf3mp_server::serve_admin;

#[tokio::test]
async fn the_admin_endpoint_finds_a_room_by_invite_player_or_name() {
    let server = RunningServer::start(|_| {}).await;
    let ann = server.client("ann").await;
    let bob = server.client("Bobby").await;
    let (invite, _) = ann.client.create_room(room("table", FAST)).await.unwrap();
    bob.client.join_room(join(&invite)).await.unwrap();
    let (ann_id, bob_id) = (
        ann.client.player().to_string(),
        bob.client.player().to_string(),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let admin = listener.local_addr().unwrap();
    tokio::spawn(serve_admin(listener, server.stats.clone()));

    // Every room open now, with its members.
    let (status, all) = get(admin, "/rooms").await;
    assert_eq!(status, 200, "{all}");
    assert!(all.contains(r#""name": "table""#), "{all}");
    assert!(all.contains(&format!(r#""player": "{ann_id}""#)), "{all}");
    assert!(all.contains(&format!(r#""player": "{bob_id}""#)), "{all}");
    assert!(all.contains(r#""name": "Bobby""#), "{all}");
    assert!(all.contains(&format!(r#""owner": "{ann_id}""#)), "{all}");
    // Never the invite itself.
    assert!(!all.contains(&invite.to_string()), "{all}");

    // By the invite a player quotes, typed in either case.
    for typed in [invite.to_string(), invite.to_string().to_lowercase()] {
        let (status, found) = get(admin, &format!("/rooms?invite={typed}")).await;
        assert_eq!(status, 200, "{found}");
        assert!(found.contains(r#""name": "table""#), "{found}");
    }
    // An invite opening no room, and one that is no invite at all.
    let other = loop {
        let other = Invite(Code::random());
        if other != invite {
            break other;
        }
    };
    assert_eq!(get(admin, &format!("/rooms?invite={other}")).await.0, 404);
    assert_eq!(get(admin, "/rooms?invite=K7QM0X").await.0, 400);

    // By a member's name, in any case, and by a player's ID.
    let (_, by_name) = get(admin, "/rooms?name=bobb").await;
    assert!(by_name.contains(r#""name": "table""#), "{by_name}");
    let (_, nobody) = get(admin, "/rooms?name=carol").await;
    assert_eq!(nobody.trim(), "[]");
    let (_, by_player) = get(admin, &format!("/rooms?player={bob_id}")).await;
    assert!(by_player.contains(r#""name": "table""#), "{by_player}");

    // One filter at most, and only these.
    assert_eq!(
        get(admin, &format!("/rooms?name=ann&player={ann_id}"))
            .await
            .0,
        404
    );
    assert_eq!(get(admin, "/rooms?owner=ann").await.0, 404);
    assert_eq!(get(admin, "/rooms/../metrics").await.0, 404);

    // The view follows the room: a player who disconnects in the lobby
    // leaves it, and is no longer found in it.
    bob.client.close().await;
    tokio::time::timeout(WAIT, async {
        loop {
            let (_, found) = get(admin, &format!("/rooms?player={bob_id}")).await;
            if found.trim() == "[]" {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the room no longer shows the player who left");
    let (_, still) = get(admin, &format!("/rooms?player={ann_id}")).await;
    assert!(still.contains(r#""players": 1"#), "{still}");
    drop(ann);
}

/// One GET to the admin endpoint; its status code and body.
async fn get(address: std::net::SocketAddr, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await.unwrap();
    let status = answer
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap();
    let body = answer
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default();
    (status, body)
}
