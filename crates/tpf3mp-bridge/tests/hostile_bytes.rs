//! Whatever bytes cross the game link, decoding gives an error or a valid
//! message, never a panic: a broken agent cannot crash the game, and a
//! broken hook cannot crash the agent. Most cases corrupt a valid message
//! of each kind, since random bytes rarely get past the first tag.

#![allow(clippy::unwrap_used)]

use std::fmt::Debug;

use proptest::{collection::vec, prelude::*, sample::Index};
use serde::{Serialize, de::DeserializeOwned};
use tpf3mp_bridge::{
    LobbyAction, LobbyConnection, LobbyLine, LobbyMember, LobbyRoom, LobbyRules, LobbyView,
    LobbyWorld, RoomInfo, RoomMember, ToAgent, ToHook, decode, encode,
};
use tpf3mp_proto::{
    BoundedVec, Event, EventBody, FixedBytes, IntentRejection, LaneDigest, Payload, PlayerId,
    Speed, Text,
};

fn check<T>(bytes: &[u8])
where
    T: Serialize + DeserializeOwned + PartialEq + Debug,
{
    if let Ok(message) = decode::<T>(bytes) {
        let encoded = encode(&message).unwrap();
        assert_eq!(decode::<T>(&encoded).unwrap(), message);
    }
}

type Check = fn(&[u8]);

fn lanes() -> Vec<LaneDigest> {
    (0..4)
        .map(|lane| LaneDigest {
            lane,
            digest: FixedBytes([lane as u8; 32]),
        })
        .collect()
}

fn samples() -> Vec<(Check, Vec<u8>)> {
    let to_hook = [
        ToHook::Hello { version: 2 },
        ToHook::Begin {
            room: tpf3mp_proto::RoomId(tpf3mp_proto::FixedBytes([1; 16])),
            scan_plan: tpf3mp_proto::ScanPlan::DEFAULT,
            rules: Text::new("native").unwrap(),
            steps_per_second: 10,
            checkpoint_interval: 50,
            saves: Text::new("C:/Users/player/TPF3-MP/worlds/saves").unwrap(),
            player: PlayerId(FixedBytes([7; 32])),
            mods: Some(tpf3mp_bridge::ModLists {
                shared: BoundedVec::new(vec![Text::new("vehicles_pack").unwrap()]).unwrap(),
                personal: BoundedVec::new(vec![Text::new("minimap").unwrap()]).unwrap(),
                params: Vec::new(),
            }),
        },
        ToHook::Apply(Event {
            seq: 12,
            step: 401,
            body: EventBody::Command {
                player: PlayerId(FixedBytes([1; 32])),
                client_seq: 3,
                payload: Payload::new(vec![9; 40]).unwrap(),
                seal: Some(tpf3mp_proto::Seal {
                    scope: 2,
                    tag: FixedBytes([4; 32]),
                }),
            },
        }),
        ToHook::Release { through: 410 },
        ToHook::Speed(Speed::NORMAL),
        ToHook::Diverged {
            step: 500,
            lanes: vec![1, 3],
        },
        ToHook::Refused {
            command: 3,
            reason: IntentRejection::Refused { code: 17 },
        },
        ToHook::End {
            reason: Text::new("the room closed").unwrap(),
        },
        ToHook::Load {
            world_epoch: 1,
            file: Some(Text::new("/home/player/.local/share/TPF3-MP/received/x.sav").unwrap()),
            next_step: 401,
        },
        ToHook::Chat {
            from: Text::new("Ann").unwrap(),
            text: Text::new("gg").unwrap(),
        },
        ToHook::Preview {
            from: PlayerId(FixedBytes([2; 32])),
            preview: Some(Payload::new(vec![5; 300]).unwrap()),
        },
        ToHook::Preview {
            from: PlayerId(FixedBytes([2; 32])),
            preview: None,
        },
        ToHook::Room(RoomInfo {
            name: Text::new("Sunday line").unwrap(),
            owner: PlayerId(FixedBytes([1; 32])),
            members: BoundedVec::new(vec![
                RoomMember {
                    banner: None,
                    loading: None,

                    player: PlayerId(FixedBytes([1; 32])),
                    name: Text::new("Ann").unwrap(),
                    connected: true,
                },
                RoomMember {
                    banner: None,
                    loading: None,

                    player: PlayerId(FixedBytes([2; 32])),
                    name: Text::new("Bo").unwrap(),
                    connected: false,
                },
            ])
            .unwrap(),
        }),
        ToHook::Lobby(Box::new(LobbyView {
            connection: LobbyConnection::Connected,
            server: Text::new("EU").unwrap(),
            server_address: Text::new("tpf3mp.example.org:29470").unwrap(),
            server_default: Text::new("tpf3mp.example.org:29470").unwrap(),
            servers: BoundedVec::empty(),
            banner: Some(Text::new("freiherr_von_schlitzwiesen").unwrap()),
            portraits: BoundedVec::new(vec![Text::new("andrew").unwrap()]).unwrap(),
            name: Text::new("Ann").unwrap(),
            error: None,
            notice: Some(Text::new("created the room").unwrap()),
            room: Some(LobbyRoom {
                name: Text::new("Sunday line").unwrap(),
                rules: Text::new("native").unwrap(),
                invite: Some(Text::new("K7QM2X").unwrap()),
                running: false,
                you_own: true,
                max_players: 4,
                has_password: false,
                members: BoundedVec::new(vec![LobbyMember {
                    player: PlayerId(FixedBytes([1; 32])),
                    name: Text::new("Ann").unwrap(),
                    ready: true,
                    connected: true,
                    owner: true,
                    you: true,
                    same_content: Some(true),
                    differs: Some(tpf3mp_proto::ContentStatus {
                        missing: 1,
                        ..Default::default()
                    }),
                    banner: None,
                    loading: None,
                }])
                .unwrap(),
                competitive: false,
                start: Some(tpf3mp_bridge::LobbyStart {
                    name: Text::new("mptest").unwrap(),
                    map: Text::new("temperate").unwrap(),
                    year: 1900,
                    arrived: true,
                }),
                upload: None,
            }),
            chat: BoundedVec::new(vec![LobbyLine {
                from: Text::new("Ann").unwrap(),
                text: Text::new("gg").unwrap(),
                you: true,
            }])
            .unwrap(),
            rules: BoundedVec::new(vec![LobbyRules {
                name: Text::new("native").unwrap(),
                description: Text::new("The game's own economy").unwrap(),
            }])
            .unwrap(),
            saves: BoundedVec::new(vec![Text::new("mptest").unwrap()]).unwrap(),
            start_save: Some(Text::new("mptest").unwrap()),
            world: LobbyWorld::Fetching {
                bytes: 1 << 20,
                total: 1 << 24,
            },
            differences: Some(Text::new("you lack stations 3").unwrap()),
            mods: BoundedVec::new(vec![tpf3mp_bridge::LobbyMod {
                id: Text::new("schbrongx_minimap").unwrap(),
                name: Text::new("Minimap").unwrap(),
                class: tpf3mp_bridge::LobbyModClass::Personal,
                reason: Text::new("only what this player sees").unwrap(),
                chosen: true,
                choosable: true,
            }])
            .unwrap(),
            room_mods: BoundedVec::new(vec![tpf3mp_bridge::LobbyRoomMod {
                id: Text::new("vehicles_pack").unwrap(),
                name: Text::new("Fahrzeuge").unwrap(),
                version: Text::new("3+m8264750").unwrap(),
                yours: Some(Text::new("2+m1").unwrap()),
                have: tpf3mp_bridge::LobbyHave::OtherVersion,
                source: Text::new("mod.io").unwrap(),
                modio: Some(6414521),
            }])
            .unwrap(),
            room_mods_more: 0,
            room_mods_missing: 0,
            room_mods_other: 1,
            room_params: BoundedVec::new(vec![tpf3mp_bridge::LobbySetting {
                id: Text::new("signals").unwrap(),
                key: Text::new("distance").unwrap(),
                value: 3,
            }])
            .unwrap(),
            rooms: None,
            log_session: Text::new("K7QM2X").unwrap(),
        })),
    ];
    let to_agent = [
        ToAgent::Hello {
            version: 2,
            build: Text::new("tpf3 1.0.0 (build 1234)").unwrap(),
        },
        ToAgent::Loaded {
            world_epoch: 1,
            next_step: 401,
        },
        ToAgent::Command {
            payload: Payload::new(vec![7; 64]).unwrap(),
            secret: Some(tpf3mp_proto::Secret {
                scope: 2,
                password: Text::new("pw").unwrap(),
            }),
        },
        ToAgent::Ran {
            world_epoch: 1,
            step: 402,
        },
        ToAgent::Checkpoint {
            world_epoch: 1,
            step: 500,
            lanes: lanes(),
        },
        ToAgent::Log {
            message: Text::new("loaded the world in 2.3 s").unwrap(),
        },
        ToAgent::Saved {
            world_epoch: 1,
            event: 12,
            lanes: lanes(),
            file: Some(Text::new("saves/12.sav").unwrap()),
        },
        ToAgent::Chat {
            text: Text::new("brb").unwrap(),
        },
        ToAgent::Preview {
            preview: Some(Payload::new(vec![6; 300]).unwrap()),
        },
        ToAgent::Speed {
            speed: Speed::PAUSED,
        },
        ToAgent::Lobby(LobbyAction::Join {
            invite: Text::new("tpf3mp.example.org:29470 K7QM2X").unwrap(),
            server: None,
            password: Some(Text::new("pw").unwrap()),
        }),
        ToAgent::Lobby(LobbyAction::ChooseStart {
            save: Text::new("Güterzug").unwrap(),
            map: Text::new("dry").unwrap(),
            year: 1925,
        }),
        ToAgent::Lobby(LobbyAction::ChooseRoomMods {
            save: Some(Text::new("Güterzug").unwrap()),
            map: Text::new("dry").unwrap(),
            year: 1925,
            mods: BoundedVec::new(vec![tpf3mp_bridge::LobbySelected {
                id: Text::new("revyn112_towns_de").unwrap(),
                name: Text::new("Deutsche Städte").unwrap(),
                source: Text::new("mod.io").unwrap(),
                modio: Some(6414521),
            }])
            .unwrap(),
            params: BoundedVec::new(vec![tpf3mp_bridge::LobbySetting {
                id: Text::new("revyn112_towns_de").unwrap(),
                key: Text::new("size").unwrap(),
                value: -2,
            }])
            .unwrap(),
        }),
        ToAgent::Lobby(LobbyAction::RescanMods),
    ];
    let mut samples: Vec<(Check, Vec<u8>)> = Vec::new();
    samples.extend(
        to_hook
            .iter()
            .map(|m| (check::<ToHook> as Check, encode(m).unwrap())),
    );
    samples.extend(
        to_agent
            .iter()
            .map(|m| (check::<ToAgent> as Check, encode(m).unwrap())),
    );
    samples
}

#[test]
fn the_samples_are_valid() {
    for (check, bytes) in samples() {
        check(&bytes);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    #[test]
    fn corrupted_messages_decode_or_fail_cleanly(
        pick in any::<Index>(),
        edits in vec((any::<Index>(), any::<u8>(), 0u8..4), 1..8),
    ) {
        let samples = samples();
        let (check, sample) = &samples[pick.index(samples.len())];
        let mut bytes = sample.clone();
        for (at, value, kind) in edits {
            match kind {
                0 | 1 if !bytes.is_empty() => {
                    let index = at.index(bytes.len());
                    bytes[index] = value;
                }
                2 => bytes.insert(at.index(bytes.len() + 1), value),
                3 if !bytes.is_empty() => bytes.truncate(at.index(bytes.len())),
                _ => {}
            }
        }
        check(&bytes);
    }

    #[test]
    fn arbitrary_bytes_decode_or_fail_cleanly(bytes in vec(any::<u8>(), 0..300)) {
        check::<ToHook>(&bytes);
        check::<ToAgent>(&bytes);
    }
}
