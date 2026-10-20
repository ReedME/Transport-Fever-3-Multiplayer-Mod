//! Rooms and the lobby: invites, passwords, limits, ownership and cleanup.

#![allow(clippy::unwrap_used)]

mod common;

use std::{sync::Arc, time::Duration};

use common::{FAST, RunningServer, content, join, modded, room, room_of};
use tpf3mp_agent::{ClientError, ClientEvent};
use tpf3mp_proto::{
    Code, CreateRoom, Invite, JoinRoom, Request, RequestError, Response, RoomPhase, RoomSettings,
    Text,
};
use tpf3mp_server::{AcceptAll, RulesChoice, RulesMenu};

/// An invite other than `invite`.
fn other_than(invite: &Invite) -> Invite {
    loop {
        let other = Invite(Code::random());
        if other != *invite {
            return other;
        }
    }
}

#[tokio::test]
async fn a_room_is_joined_with_its_invite() {
    let server = RunningServer::start(|_| {}).await;
    let mut ann = server.client("ann").await;
    let mut bob = server.client("bob").await;
    let (invite, created) = ann.client.create_room(room("table", FAST)).await.unwrap();
    assert_eq!(created.owner, ann.client.player());
    assert_eq!(created.phase, RoomPhase::Lobby);
    // The invite survives a round trip through its text form, as players
    // paste it into chat, and it is six letters and digits, typed in either
    // case.
    let text = invite.to_string();
    assert_eq!(text.len(), 6, "{text}");
    let invite: Invite = text.to_lowercase().parse().unwrap();
    let joined = bob.client.join_room(join(&invite)).await.unwrap();
    assert_eq!(joined.members.len(), 2);
    // Both see the full table.
    ann.room_where(|room| room.members.len() == 2).await;
    bob.room_where(|room| room.members.len() == 2).await;
    server.shut_down().await;
}

/// Wrong invites one address may try before it is stopped a while, as
/// the server counts them.
const WRONG_INVITES: usize = 20;

#[tokio::test]
async fn an_address_trying_codes_is_stopped_before_it_finds_a_room() {
    let server = RunningServer::start(|_| {}).await;
    let ann = server.client("ann").await;
    let (invite, _) = ann.client.create_room(room("table", FAST)).await.unwrap();
    // One connection may try five joins at once; a guesser opens more.
    let mut tried = 0;
    while tried < WRONG_INVITES {
        let guesser = server.client("guesser").await;
        for _ in 0..5.min(WRONG_INVITES - tried) {
            let wrong = other_than(&invite);
            let attempt = if tried.is_multiple_of(2) {
                guesser
                    .client
                    .join_room(join(&wrong))
                    .await
                    .map(Response::RoomJoined)
            } else {
                guesser
                    .client
                    .request(Request::ResolveInvite {
                        invite: wrong,
                        password: None,
                    })
                    .await
            };
            assert_eq!(attempt, Err(ClientError::Refused(RequestError::BadInvite)));
            tried += 1;
        }
    }
    // Now even the right code is refused from that address, and says so.
    let guesser = server.client("guesser").await;
    assert_eq!(
        guesser
            .client
            .request(Request::ResolveInvite {
                invite,
                password: None,
            })
            .await,
        Err(ClientError::Refused(RequestError::RateLimited))
    );
    let error = guesser.client.join_room(join(&invite)).await.unwrap_err();
    assert_eq!(error, ClientError::Refused(RequestError::RateLimited));
    server.shut_down().await;
}

#[tokio::test]
async fn every_bad_invite_fails_the_same_way() {
    let server = RunningServer::start(|_| {}).await;
    let ann = server.client("ann").await;
    let bob = server.client("bob").await;
    let mut create = room("secret", FAST);
    create.password = Some(Text::new("hunter2").unwrap());
    let (invite, created) = ann.client.create_room(create).await.unwrap();
    assert!(created.has_password);

    let attempts = [
        // Codes no room has, with the password and without.
        (other_than(&invite), Some("hunter2")),
        (other_than(&invite), None),
        (invite, Some("hunter3")),
        (invite, None),
    ];
    for (invite, password) in attempts {
        let error = bob
            .client
            .join_room(JoinRoom {
                scan_plan: tpf3mp_proto::ScanPlan::DEFAULT,
                invite,
                password: password.map(|p| Text::new(p).unwrap()),
                resume: None,
            })
            .await
            .unwrap_err();
        assert_eq!(error, ClientError::Refused(RequestError::BadInvite));
    }
    // The right invite and password still work.
    bob.client
        .join_room(JoinRoom {
            scan_plan: tpf3mp_proto::ScanPlan::DEFAULT,
            invite,
            password: Some(Text::new("hunter2").unwrap()),
            resume: None,
        })
        .await
        .unwrap();
    server.shut_down().await;
}

#[tokio::test]
async fn invite_resolution_checks_passwords_without_telling_why_a_code_failed() {
    let server = RunningServer::start(|_| {}).await;
    let ann = server.client("ann").await;
    let bob = server.client("bob").await;
    let (open_invite, _) = ann.client.create_room(room("open", FAST)).await.unwrap();
    assert_eq!(
        bob.client
            .request(Request::ResolveInvite {
                invite: open_invite,
                password: None,
            })
            .await,
        Ok(Response::InviteMatch)
    );
    let joined = bob.client.join_room(join(&open_invite)).await.unwrap();
    assert_eq!(joined.members.len(), 2, "resolution did not take a seat");
    ann.client.leave_room().await.unwrap();
    bob.client.leave_room().await.unwrap();
    server.wait_for_rooms(0).await;

    let mut protected = room("protected", FAST);
    protected.password = Some(Text::new("right-password").unwrap());
    let (invite, _) = ann.client.create_room(protected).await.unwrap();
    let carol = server.client("carol").await;
    let unknown = other_than(&invite);
    let attempts = [
        (unknown, Some("right-password")),
        (unknown, None),
        (invite, Some("wrong-password")),
        (invite, None),
    ];
    for (invite, password) in attempts {
        assert_eq!(
            carol
                .client
                .request(Request::ResolveInvite {
                    invite,
                    password: password.map(|value| Text::new(value).unwrap()),
                })
                .await,
            Err(ClientError::Refused(RequestError::BadInvite))
        );
    }
    let dave = server.client("dave").await;
    assert_eq!(
        carol
            .client
            .request(Request::ResolveInvite {
                invite,
                password: Some(Text::new("right-password").unwrap()),
            })
            .await,
        Ok(Response::InviteMatch)
    );
    assert_eq!(
        dave.client
            .join_room(JoinRoom {
                scan_plan: tpf3mp_proto::ScanPlan::DEFAULT,
                invite,
                password: Some(Text::new("right-password").unwrap()),
                resume: None,
            })
            .await
            .unwrap()
            .members
            .len(),
        2,
        "the authoritative join still seats the player"
    );
    server.shut_down().await;
}

#[tokio::test]
async fn invite_resolution_leaves_capacity_to_join_room() {
    let server = RunningServer::start(|_| {}).await;
    let ann = server.client("ann").await;
    let bob = server.client("bob").await;
    let mut only_one = room("full", FAST);
    only_one.max_players = 1;
    let (invite, _) = ann.client.create_room(only_one).await.unwrap();

    assert_eq!(
        bob.client
            .request(Request::ResolveInvite {
                invite,
                password: None,
            })
            .await,
        Ok(Response::InviteMatch),
        "valid credentials resolve even when the room has no free seat"
    );
    assert_eq!(
        bob.client.join_room(join(&invite)).await.unwrap_err(),
        ClientError::Refused(RequestError::RoomFull)
    );
    server.shut_down().await;
}

#[tokio::test]
async fn resolving_a_running_room_changes_neither_its_members_nor_its_log() {
    let dir = std::env::temp_dir().join(format!("tpf3mp-resolve-invite-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let server = RunningServer::start(|config| {
        config.data_dir = Some(dir.clone());
        config.tick = Duration::from_secs(60);
    })
    .await;
    let ann = server.client("ann").await;
    let bob = server.client("bob").await;
    let (invite, room) = ann.client.create_room(room("logged", FAST)).await.unwrap();
    ann.client
        .declare_content(common::content(1))
        .await
        .unwrap();
    ann.client.set_ready(true).await.unwrap();
    ann.client.start_game().await.unwrap();
    let path = dir.join(format!("{}.log", room.id));
    let before = std::fs::read(&path).unwrap();

    let resolved = bob
        .client
        .request(Request::ResolveInvite {
            invite,
            password: None,
        })
        .await;
    let join = bob.client.join_room(join(&invite)).await;
    let after = std::fs::read(&path).unwrap();
    server.shut_down().await;
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(resolved, Ok(Response::InviteMatch));
    assert_eq!(join, Err(ClientError::Refused(RequestError::GameRunning)));
    assert_eq!(before, after, "lookup must not append to the room log");
}

#[tokio::test]
async fn a_full_room_refuses_more_players() {
    let server = RunningServer::start(|_| {}).await;
    let ann = server.client("ann").await;
    let bob = server.client("bob").await;
    let cat = server.client("cat").await;
    let mut create = room("pair", FAST);
    create.max_players = 2;
    let (invite, _) = ann.client.create_room(create).await.unwrap();
    bob.client.join_room(join(&invite)).await.unwrap();
    assert_eq!(
        cat.client.join_room(join(&invite)).await.unwrap_err(),
        ClientError::Refused(RequestError::RoomFull)
    );
    server.shut_down().await;
}

#[tokio::test]
async fn ownership_passes_on_and_an_empty_room_closes() {
    let server = RunningServer::start(|_| {}).await;
    let ann = server.client("ann").await;
    let mut bob = server.client("bob").await;
    let (invite, _) = ann.client.create_room(room("table", FAST)).await.unwrap();
    bob.client.join_room(join(&invite)).await.unwrap();
    assert_eq!(server.stats.rooms(), 1);

    ann.client.leave_room().await.unwrap();
    let room = bob.room_where(|room| room.members.len() == 1).await;
    assert_eq!(room.owner, bob.client.player());

    bob.client.leave_room().await.unwrap();
    server.wait_for_rooms(0).await;
    // The invite of a closed room is just a bad invite.
    assert_eq!(
        ann.client.join_room(join(&invite)).await.unwrap_err(),
        ClientError::Refused(RequestError::BadInvite)
    );
    server.shut_down().await;
}

#[tokio::test]
async fn a_lobby_seat_is_freed_when_its_player_disconnects() {
    let server = RunningServer::start(|_| {}).await;
    let mut ann = server.client("ann").await;
    let bob = server.client("bob").await;
    let (invite, _) = ann.client.create_room(room("table", FAST)).await.unwrap();
    bob.client.join_room(join(&invite)).await.unwrap();
    ann.room_where(|room| room.members.len() == 2).await;
    bob.client.close().await;
    ann.room_where(|room| room.members.len() == 1).await;
    server.shut_down().await;
}

#[tokio::test]
async fn a_connection_is_in_one_room_at_a_time() {
    let server = RunningServer::start(|_| {}).await;
    let ann = server.client("ann").await;
    ann.client.create_room(room("first", FAST)).await.unwrap();
    assert_eq!(
        ann.client
            .create_room(room("second", FAST))
            .await
            .unwrap_err(),
        ClientError::Refused(RequestError::AlreadyInRoom)
    );
    server.shut_down().await;
}

#[tokio::test]
async fn settings_out_of_range_are_refused() {
    let server = RunningServer::start(|_| {}).await;
    let ann = server.client("ann").await;
    let bad = [
        RoomSettings {
            steps_per_second: 0,
            ..FAST
        },
        RoomSettings {
            input_delay_ms: 5,
            ..FAST
        },
        RoomSettings {
            checkpoint_interval: 0,
            ..FAST
        },
    ];
    for settings in bad {
        assert_eq!(
            ann.client
                .create_room(room("bad", settings))
                .await
                .unwrap_err(),
            ClientError::Refused(RequestError::InvalidSettings)
        );
    }
    let zero_players = CreateRoom {
        max_players: 0,
        ..room("bad", FAST)
    };
    assert_eq!(
        ann.client.create_room(zero_players).await.unwrap_err(),
        ClientError::Refused(RequestError::InvalidSettings)
    );
    server.shut_down().await;
}

#[tokio::test]
async fn the_host_picks_the_rules_the_room_is_played_by() {
    let server = RunningServer::start(|config| {
        config.rules = RulesMenu::native().with(RulesChoice {
            name: Text::new("strict").unwrap(),
            description: Text::new("Checked by the server").unwrap(),
            factory: Arc::new(|| Box::new(AcceptAll)),
        });
    })
    .await;
    let ann = server.client("ann").await;
    let bob = server.client("bob").await;
    let offered: Vec<&str> = ann
        .client
        .welcome()
        .rules
        .iter()
        .map(|offer| offer.name.as_str())
        .collect();
    assert_eq!(offered, ["native", "strict"]);

    // Without a choice, the server's default: the game's own rules.
    let (_, native) = ann.client.create_room(room("plain", FAST)).await.unwrap();
    assert_eq!(native.rules.as_str(), "native");
    ann.client.leave_room().await.unwrap();

    let strict = CreateRoom {
        rules: Some(Text::new("strict").unwrap()),
        ..room("strict", FAST)
    };
    let (invite, created) = ann.client.create_room(strict).await.unwrap();
    assert_eq!(created.rules.as_str(), "strict");
    let joined = bob.client.join_room(join(&invite)).await.unwrap();
    assert_eq!(joined.rules.as_str(), "strict");

    let unknown = CreateRoom {
        rules: Some(Text::new("nonesuch").unwrap()),
        ..room("other", FAST)
    };
    let carl = server.client("carl").await;
    assert_eq!(
        carl.client.create_room(unknown).await.unwrap_err(),
        ClientError::Refused(RequestError::UnknownRules)
    );
    server.shut_down().await;
}

#[tokio::test]
async fn the_server_caps_its_rooms() {
    let server = RunningServer::start(|config| config.max_rooms = 1).await;
    let ann = server.client("ann").await;
    let bob = server.client("bob").await;
    ann.client.create_room(room("one", FAST)).await.unwrap();
    assert_eq!(
        bob.client.create_room(room("two", FAST)).await.unwrap_err(),
        ClientError::Refused(RequestError::TooManyRooms)
    );
    server.shut_down().await;
}

#[tokio::test]
async fn an_address_has_only_so_many_open_rooms() {
    let server = RunningServer::start(|config| config.max_rooms_per_address = 1).await;
    let ann = server.client("ann").await;
    let bob = server.client("bob").await;
    ann.client.create_room(room("one", FAST)).await.unwrap();
    // Bob connects from the same address as Ann.
    assert_eq!(
        bob.client.create_room(room("two", FAST)).await.unwrap_err(),
        ClientError::Refused(RequestError::TooManyRooms)
    );
    ann.client.leave_room().await.unwrap();
    server.wait_for_rooms(0).await;
    bob.client.create_room(room("two", FAST)).await.unwrap();
    server.shut_down().await;
}

#[tokio::test]
async fn the_owner_can_kick_a_player_for_good() {
    let server = RunningServer::start(|_| {}).await;
    let ann = server.client("ann").await;
    let mut bob = server.client("bob").await;
    let cat = server.client("cat").await;
    let (invite, _) = ann.client.create_room(room("table", FAST)).await.unwrap();
    bob.client.join_room(join(&invite)).await.unwrap();
    cat.client.join_room(join(&invite)).await.unwrap();
    let bob_id = bob.client.player();

    assert_eq!(
        cat.client.kick(bob_id).await.unwrap_err(),
        ClientError::Refused(RequestError::NotOwner)
    );
    assert_eq!(
        ann.client.kick(ann.client.player()).await.unwrap_err(),
        ClientError::Refused(RequestError::CannotKickSelf)
    );
    ann.client.kick(bob_id).await.unwrap();
    bob.wait_for(|event| matches!(event, ClientEvent::Kicked).then_some(()))
        .await;
    assert_eq!(
        ann.client.kick(bob_id).await.unwrap_err(),
        ClientError::Refused(RequestError::NoSuchPlayer)
    );
    // Bob cannot come back, even with the invite.
    assert_eq!(
        bob.client.join_room(join(&invite)).await.unwrap_err(),
        ClientError::Refused(RequestError::BadInvite)
    );
    assert_eq!(
        bob.client
            .request(Request::ResolveInvite {
                invite,
                password: None,
            })
            .await,
        Err(ClientError::Refused(RequestError::BadInvite)),
        "a kicked player cannot use resolution to distinguish their invite"
    );
    // He is free to make a room of his own.
    bob.client.create_room(room("mine", FAST)).await.unwrap();
    server.shut_down().await;
}

#[tokio::test]
async fn starting_requires_the_owner_readiness_and_matching_content() {
    let server = RunningServer::start(|_| {}).await;
    let ann = server.client("ann").await;
    let bob = server.client("bob").await;
    let (invite, _) = ann.client.create_room(room("table", FAST)).await.unwrap();
    bob.client.join_room(join(&invite)).await.unwrap();

    let refused = |error| Err(ClientError::Refused(error));
    assert_eq!(
        ann.client.start_game().await,
        refused(RequestError::NotAllReady)
    );
    ann.client.set_ready(true).await.unwrap();
    bob.client.set_ready(true).await.unwrap();
    assert_eq!(
        ann.client.start_game().await,
        refused(RequestError::ContentMismatch)
    );
    ann.client.declare_content(content(1)).await.unwrap();
    bob.client.declare_content(content(2)).await.unwrap();
    assert_eq!(
        ann.client.start_game().await,
        refused(RequestError::ContentMismatch)
    );
    bob.client.declare_content(content(1)).await.unwrap();
    assert_eq!(
        bob.client.start_game().await,
        refused(RequestError::NotOwner)
    );
    ann.client.start_game().await.unwrap();
    assert_eq!(
        ann.client.start_game().await,
        refused(RequestError::GameRunning)
    );
    // The lobby is closed to changes once the game runs.
    assert_eq!(
        bob.client.set_ready(false).await,
        refused(RequestError::GameRunning)
    );
    // The game's content can be declared again, but not changed.
    bob.client.declare_content(content(1)).await.unwrap();
    assert_eq!(
        bob.client.declare_content(content(2)).await,
        refused(RequestError::GameRunning)
    );
    server.shut_down().await;
}

#[tokio::test]
async fn players_learn_which_mods_differ_from_the_owners() {
    let server = RunningServer::start(|_| {}).await;
    let ann = server.client("ann").await;
    let mut bob = server.client("bob").await;
    let mut cat = server.client("cat").await;
    // Content goes with the connection: declared before creating or joining.
    ann.client
        .declare_content(modded(&["trains 1.2", "stations 3", "maps 1"]))
        .await
        .unwrap();
    bob.client
        .declare_content(modded(&["trains 1.1", "maps 1", "trees 2"]))
        .await
        .unwrap();
    cat.client
        .declare_content(modded(&["trains 1.2", "stations 3", "maps 1"]))
        .await
        .unwrap();
    let (invite, _) = ann.client.create_room(room("table", FAST)).await.unwrap();
    bob.client.join_room(join(&invite)).await.unwrap();
    let room = cat.client.join_room(join(&invite)).await.unwrap();
    let fingerprints: Vec<_> = room.members.iter().map(|m| m.content).collect();
    assert_eq!(fingerprints[0], fingerprints[2]);
    assert_ne!(fingerprints[0], fingerprints[1]);

    // Bob hears exactly what to change; Cat, who matches, hears nothing.
    let diff = bob.content_diff().await.expect("Bob's game differs");
    let named = |mods: &[tpf3mp_proto::ModRef]| -> Vec<String> {
        mods.iter()
            .map(|m| format!("{} {}", m.id, m.version))
            .collect()
    };
    assert_eq!(diff.game, None);
    assert_eq!(named(&diff.missing), ["stations 3"]);
    assert_eq!(named(&diff.extra), ["trees 2"]);
    assert_eq!(diff.changed.len(), 1);
    assert_eq!(
        (
            diff.changed[0].id.as_str(),
            diff.changed[0].room.as_str(),
            diff.changed[0].yours.as_str()
        ),
        ("trains", "1.2", "1.1")
    );
    for player in [&ann, &bob, &cat] {
        player.client.set_ready(true).await.unwrap();
    }
    assert_eq!(
        ann.client.start_game().await,
        Err(ClientError::Refused(RequestError::ContentMismatch))
    );

    // Once Bob matches, he hears that he does, and the game can start.
    bob.client
        .declare_content(modded(&["trains 1.2", "stations 3", "maps 1"]))
        .await
        .unwrap();
    assert_eq!(bob.content_diff().await, None);
    ann.client.start_game().await.unwrap();
    // Cat never differed, so was never told anything.
    let told = tokio::time::timeout(std::time::Duration::from_millis(300), async {
        while let Some(event) = cat.events.recv().await {
            if matches!(event, ClientEvent::ContentDiff(_)) {
                return true;
            }
        }
        false
    })
    .await;
    assert_ne!(told, Ok(true));
    server.shut_down().await;
}

/// Two copies of TPF3-MP's own mod of the same revision whose files differ
/// declare different versions (`tpf3mp_agent::own_mod`): the room does not
/// start, and the player with the other copy is told plainly.
#[tokio::test]
async fn another_copy_of_tpf3mp_itself_keeps_the_room_from_starting() {
    const HOSTS: &str = "tpf3mp_1 1+0123456789abcdef";
    let server = RunningServer::start(|_| {}).await;
    let ann = server.client("ann").await;
    let mut bob = server.client("bob").await;
    ann.client
        .declare_content(modded(&["trains 1", HOSTS]))
        .await
        .unwrap();
    bob.client
        .declare_content(modded(&["trains 1", "tpf3mp_1 1+fedcba9876543210"]))
        .await
        .unwrap();
    let (invite, _) = ann.client.create_room(room("table", FAST)).await.unwrap();
    bob.client.join_room(join(&invite)).await.unwrap();
    ann.client.set_ready(true).await.unwrap();
    bob.client.set_ready(true).await.unwrap();
    assert_eq!(
        ann.client.start_game().await,
        Err(ClientError::Refused(RequestError::ContentMismatch))
    );
    let told = bob.content_diff().await.expect("Bob's copy differs");
    assert_eq!(
        told.to_string(),
        "Your TPF3-MP mod differs from the host's (yours fedcba98, host 01234567): reinstall the same version"
    );
    // The same files: the room starts.
    bob.client
        .declare_content(modded(&["trains 1", HOSTS]))
        .await
        .unwrap();
    assert_eq!(bob.content_diff().await, None);
    ann.client.start_game().await.unwrap();
    server.shut_down().await;
}

#[tokio::test]
async fn an_overlong_mod_list_is_refused() {
    let server = RunningServer::start(|_| {}).await;
    let ann = server.client("ann").await;
    let manifest = tpf3mp_proto::ContentManifest {
        game: Text::new("build-1").unwrap(),
        mods: (0..=tpf3mp_proto::MAX_LISTED_MODS)
            .map(|n| tpf3mp_proto::ModRef {
                id: Text::new(format!("m{n}")).unwrap(),
                version: Text::new("1").unwrap(),
            })
            .collect(),
        unlisted: None,
        native: Vec::new(),
    };
    assert_eq!(
        ann.client.declare_content(manifest).await,
        Err(ClientError::Refused(RequestError::InvalidContent))
    );
    server.shut_down().await;
}

#[tokio::test]
async fn chat_reaches_everyone_in_the_room_at_a_measured_pace() {
    let server = RunningServer::start(|_| {}).await;
    let mut ann = server.client("ann").await;
    let mut bob = server.client("bob").await;
    let outsider = server.client("eve").await;
    let (invite, _) = ann.client.create_room(room("table", FAST)).await.unwrap();
    bob.client.join_room(join(&invite)).await.unwrap();
    let said = Text::new("gg, rail is free").unwrap();
    let speaker = ann.client.player();
    ann.client.chat(said.clone()).await.unwrap();
    // Both hear it, the sender too, so everyone sees one conversation.
    for listener in [&mut ann, &mut bob] {
        let (from, text) = listener
            .wait_for(|event| match event {
                ClientEvent::Chat { from, text } => Some((from, text)),
                _ => None,
            })
            .await;
        assert_eq!((from, text), (speaker, said.clone()));
    }
    // Someone in no room has nobody to talk to.
    assert_eq!(
        outsider.client.chat(said.clone()).await.unwrap_err(),
        ClientError::Refused(RequestError::NotInRoom)
    );
    // A burst of five, then one a second.
    let mut refused = 0;
    for _ in 0..8 {
        if bob.client.chat(said.clone()).await
            == Err(ClientError::Refused(RequestError::RateLimited))
        {
            refused += 1;
        }
    }
    assert!(refused >= 2, "{refused} of 8 refused");
    server.shut_down().await;
}

/// A member who comes back on a new connection in the lobby (a launcher
/// restarted) hears the room's mods again: the new connection was told
/// nothing yet.
#[tokio::test]
async fn a_member_back_on_a_new_connection_hears_the_rooms_mods_again() {
    let server = RunningServer::start(|_| {}).await;
    let ann = server.client("ann").await;
    let mut bob = server.client("bob").await;
    let mods = room_of(&["trains 1.2"]);
    ann.client.declare_room(mods.clone()).await.unwrap();
    let (invite, _) = ann.client.create_room(room("table", FAST)).await.unwrap();
    bob.client.join_room(join(&invite)).await.unwrap();
    assert_eq!(
        bob.room_mods().await.map(|room| *room),
        Some(mods.room_mods())
    );
    let identity = Arc::clone(&bob.identity);
    drop(bob);
    let mut again = server.client_as(identity, "bob").await;
    again.client.join_room(join(&invite)).await.unwrap();
    assert_eq!(
        again.room_mods().await.map(|room| *room),
        Some(mods.room_mods()),
        "told again on the new connection"
    );
    server.shut_down().await;
}

/// The owner declares the room's mods with what players are told of them:
/// every member hears the list on joining and whenever it changes, sees
/// how each member's game differs, and is asked to get ready again when it
/// changes. Nobody else may declare one, nor a list that does not hold
/// together.
#[tokio::test]
async fn every_member_hears_the_rooms_mods_from_its_owner() {
    let server = RunningServer::start(|_| {}).await;
    let mut ann = server.client("ann").await;
    let mut bob = server.client("bob").await;
    let first = room_of(&["trains 1.2", "stations 3"]);
    ann.client.declare_room(first.clone()).await.unwrap();
    let (invite, _) = ann.client.create_room(room("table", FAST)).await.unwrap();
    assert_eq!(
        ann.room_mods().await.map(|room| *room),
        Some(first.room_mods()),
        "the owner hears the list too"
    );
    bob.client
        .declare_content(modded(&["trains 1.2", "tpf3mp_1 1+0123456789abcdef"]))
        .await
        .unwrap();
    bob.client.join_room(join(&invite)).await.unwrap();
    let told = bob.room_mods().await.unwrap();
    let ids: Vec<&str> = told.mods.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["trains", "stations", "tpf3mp_1"]);
    // Ann sees what Bob's game lacks, in counts.
    let view = ann
        .room_where(|room| room.members.len() == 2 && room.members[1].differs.is_some())
        .await;
    let differs = view.members[1].differs.unwrap();
    assert_eq!(
        (
            differs.missing,
            differs.changed,
            differs.extra,
            differs.game
        ),
        (1, 0, 0, false),
        "Bob lacks the stations"
    );
    assert_eq!(view.members[0].differs, None, "the owner's is the room's");

    // Bob declares a list of his own: not his to declare.
    assert_eq!(
        bob.client.declare_room(room_of(&["trains 1.2"])).await,
        Err(ClientError::Refused(RequestError::NotOwner))
    );
    // A list without TPF3-MP's own last does not hold together.
    let mut broken = room_of(&["trains 1.2"]);
    broken.manifest.mods.reverse();
    assert_eq!(
        ann.client.declare_room(broken).await,
        Err(ClientError::Refused(RequestError::InvalidContent))
    );

    // Bob gets ready; Ann changes the room's mods: Bob hears the new list
    // and is not ready any more.
    bob.client.set_ready(true).await.unwrap();
    bob.room_where(|room| {
        room.members
            .iter()
            .all(|m| m.ready || m.player == room.owner)
    })
    .await;
    let second = room_of(&["trains 1.2"]);
    ann.client.declare_room(second.clone()).await.unwrap();
    // The room's mods come first, then the view that asks Bob to get
    // ready again: on one ordered stream, Bob has what he agrees to before
    // he can.
    assert_eq!(
        bob.room_mods().await.map(|room| *room),
        Some(second.room_mods())
    );
    let view = bob
        .room_where(|room| room.members.iter().all(|m| !m.ready))
        .await;
    assert_eq!(view.members.len(), 2);
    // Now Bob's game matches: no difference to show.
    let view = ann
        .room_where(|room| room.members.iter().all(|m| m.differs.is_none()))
        .await;
    assert_eq!(view.members[0].content, view.members[1].content);

    // An owner's plain declaration leaves the room without a list.
    ann.client
        .declare_content(second.manifest.clone())
        .await
        .unwrap();
    assert_eq!(bob.room_mods().await, None);
    server.shut_down().await;
}
