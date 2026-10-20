//! Renders the launcher's screens to images for a person to look at:
//!
//! ```text
//! cargo test -p tpf3mp-launcher --test screenshots -- --ignored
//! ```
//!
//! The images land in `target/launcher-screenshots/`, one for each of the
//! sample states the page it copies offers (its `preview.js`), with the
//! same data and the lobby in the window, so the two can be laid side by
//! side (D20); and the `g*` ones, the window as it opens, with the lobby
//! in the game's menu (D17). They need a GPU (or a software renderer), so
//! the test is not part of the normal run.

#![allow(clippy::unwrap_used)]

use std::{cell::RefCell, path::PathBuf};

use eframe::egui;
use egui_kittest::{Harness, kittest::Queryable};
use tpf3mp_agent::launcher::{
    Action, ChatLine, Connection, Differences, Game, InstalledGame, Member, MemberContent, ModHave,
    Phase, Room, RoomNativeRow, RulesChoice, ServerRow, State, World,
};
use tpf3mp_agent::native_mods::{NativeRow, NativeView, PluginConsentPackage, PluginConsentPrompt};
use tpf3mp_launcher::{
    app::{Extras, LauncherApp, Shown},
    backend::Backend,
    notes::{Block, Notes, ReleaseNotes},
    probe::{Probe, Reach},
    update::UpdateState,
    view::Place,
};
use tpf3mp_nativemods::registry::Proposal;

struct Still(RefCell<State>);

impl Backend for Still {
    fn state(&self) -> State {
        self.0.borrow().clone()
    }

    fn act(&self, _action: Action) {}

    fn busy(&self) -> bool {
        false
    }
}

fn render(name: &str, state: State, scroll_left: f32) {
    render_clicking(name, state, scroll_left, None, Place::Launcher);
}

fn render_in_game(name: &str, state: State) {
    render_clicking(name, state, 0.0, None, Place::Game);
}

fn render_clicking(name: &str, state: State, scroll_left: f32, click: Option<&str>, place: Place) {
    let app = LauncherApp::new(
        Still(RefCell::new(state)),
        Extras {
            updater: None,
            probe: Some(Probe::showing(Reach::Online)),
            notes: Some(ReleaseNotes::showing(Notes::Release {
                version: "0.2.0".into(),
                blocks: vec![
                    Block::Heading("New".into()),
                    Block::Item("Rooms remember their rules across a server restart.".into()),
                    Block::Item("The launcher shows release notes.".into()),
                    Block::Heading("Fixes".into()),
                    Block::Item("Joining by invite works in either case.".into()),
                ],
            })),
            shown: Shown {
                update: Some(UpdateState::Ready {
                    version: "0.2.0".into(),
                }),
                installed_mod: Some(Some("0.1.0".into())),
            },
        },
    )
    .with_place(place)
    .with_preview(&preview_url("big-maps"), sample_preview(0))
    .with_preview(&preview_url("terrain-tweaks"), sample_preview(1))
    .with_preview(&preview_url("faster-trains"), sample_preview(2));
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 690.0))
        .wgpu()
        .build_ui_state(|ui, app: &mut LauncherApp<Still>| app.show(ui), app);
    harness.run_steps(4);
    if let Some(label) = click {
        harness.get_by_label(label).click();
        harness.run_steps(6);
    }
    if scroll_left > 0.0 {
        harness.hover_at(egui::pos2(300.0, 300.0));
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -scroll_left),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run_steps(8);
    }
    let image = harness.render().unwrap();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/launcher-screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    image.save(dir.join(format!("{name}.png"))).unwrap();
}

/// Where a sample mod's preview would be fetched from.
fn preview_url(id: &str) -> String {
    format!("https://raw.githubusercontent.com/example/{id}/HEAD/preview.png")
}

/// A sample preview: a landscape drawn from `seed`, 640 by 360.
fn sample_preview(seed: u32) -> egui::ColorImage {
    let (w, h) = (640usize, 360usize);
    let skies = [[96, 146, 196], [222, 150, 104], [58, 74, 120]];
    let lands = [[78, 120, 70], [120, 104, 66], [70, 92, 84]];
    let sky = skies[seed as usize % 3];
    let land = lands[seed as usize % 3];
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let hill = 0.55
                + 0.08 * (fx * 7.0 + seed as f32).sin()
                + 0.05 * (fx * 17.0 + 2.0 * seed as f32).cos();
            let near = 0.78 + 0.04 * (fx * 11.0 + 3.0 * seed as f32).sin();
            let rail =
                (fy - (0.86 - 0.1 * fx)).abs() < 0.006 || (fy - (0.9 - 0.1 * fx)).abs() < 0.006;
            let c = if rail {
                [52, 48, 46]
            } else if fy > near {
                land.map(|v| (v as f32 * 0.8) as u8)
            } else if fy > hill {
                land
            } else {
                let t = fy / hill;
                [0, 1, 2].map(|i| (sky[i] as f32 * (0.75 + 0.35 * t)).min(255.0) as u8)
            };
            pixels.push(egui::Color32::from_rgb(c[0], c[1], c[2]));
        }
    }
    egui::ColorImage {
        size: [w, h],
        source_size: egui::vec2(w as f32, h as f32),
        pixels,
    }
}

/// A server's native mods in Settings: one installed with an update, one proposed in a pull request, shown.
fn native() -> NativeView {
    NativeView {
        available: true,
        serial: Some(1_760_000_000),
        server: Some("play.tpf3mp.example:29470".into()),
        proposed_shown: true,
        packages: vec![
            NativeRow {
                id: "big-maps".into(),
                name: "Big Maps".into(),
                description: "Maps up to four times the base size.".into(),
                homepage: Some("https://github.com/example/tpf3-big-maps".into()),
                simulation: true,
                available: Some("1.2.0".into()),
                installed: Some("1.1.0".into()),
                previous: Some("1.0.3".into()),
                enabled: true,
                listed: true,
                proposed: None,
                authors: vec!["Juliansgith".into(), "silver2127".into()],
                preview: Some(preview_url("big-maps")),
                available_size: Some(18_400_000),
                installed_size: Some(17_900_000),
                load_order: Some(1),
                downloads: Some(12_480),
                updated: Some("2026-10-09".into()),
                clash: None,
                code: false,
                unsupported: None,
            },
            NativeRow {
                id: "terrain-tweaks".into(),
                name: "Terrain Tweaks".into(),
                description: "Steeper hills and deeper valleys on new maps.".into(),
                homepage: Some("https://github.com/example/terrain-tweaks".into()),
                simulation: true,
                available: Some("2.0.0".into()),
                installed: Some("2.0.0".into()),
                previous: None,
                enabled: false,
                listed: true,
                proposed: None,
                authors: vec!["GrayM8".into()],
                preview: Some(preview_url("terrain-tweaks")),
                available_size: Some(2_300_000),
                installed_size: Some(2_300_000),
                load_order: None,
                downloads: Some(3_210),
                updated: Some("2026-08-21".into()),
                clash: None,
                code: false,
                unsupported: None,
            },
            NativeRow {
                id: "faster-trains".into(),
                name: "Faster Trains".into(),
                description: "High-speed lines up to 400 km/h, with the tracks and \
                              signals they need."
                    .into(),
                homepage: Some("https://github.com/example/faster-trains".into()),
                simulation: true,
                available: Some("0.4.1".into()),
                installed: None,
                previous: None,
                enabled: false,
                listed: true,
                proposed: None,
                authors: vec!["Max".into()],
                preview: Some(preview_url("faster-trains")),
                available_size: Some(41_800_000),
                installed_size: None,
                load_order: None,
                downloads: Some(980),
                updated: Some("2026-10-01".into()),
                clash: None,
                code: true,
                unsupported: None,
            },
            NativeRow {
                id: "station-signs".into(),
                name: "Station Signs".into(),
                description: "Bigger, clearer signs at stations.".into(),
                homepage: Some("https://github.com/example/station-signs".into()),
                simulation: false,
                available: Some("0.3.0".into()),
                installed: None,
                previous: None,
                enabled: false,
                listed: true,
                proposed: Some(Proposal {
                    number: 12,
                    title: "Add Station Signs".into(),
                    url: "https://github.com/example/tpf3mp-registry/pull/12".into(),
                }),
                authors: vec!["someone".into()],
                preview: None,
                available_size: Some(640_000),
                installed_size: None,
                load_order: None,
                downloads: None,
                updated: None,
                clash: None,
                code: false,
                unsupported: None,
            },
        ],
        problem: None,
    }
}

/// The page's `base` state.
fn base() -> State {
    State {
        name: "Ann".into(),
        player: Some("7QM2".into()),
        server: Some("play.tpf3mp.example:29470".into()),
        server_fixed: true,
        server_default: Some("play.tpf3mp.example:29470".into()),
        server_name: Some("EU".into()),
        rules: vec![
            RulesChoice {
                name: "native".into(),
                description: "The game's own economy, as in single player.".into(),
            },
            RulesChoice {
                name: "canonical".into(),
                description: "The server settles the economy.".into(),
            },
        ],
        installed: Some(InstalledGame {
            dir: r"C:\Program Files (x86)\Steam\steamapps\common\Transport Fever 3".into(),
            build: "20364158".into(),
        }),
        diagnostics: Some(true),
        ..State::default()
    }
}

fn connected() -> State {
    State {
        connection: Connection::Connected,
        server_version: Some("0.1.0".into()),
        support_id: Some("S4TK9Q".into()),
        log_session: Some("AB2CD3".into()),
        ..base()
    }
}

fn native_plugin_consent() -> State {
    let source = format!(
        "{}native-mods.example.org:29470",
        "regional-registry.".repeat(3)
    );
    let digest = "0123456789abcdef".repeat(4);
    State {
        native_consent: Some(PluginConsentPrompt {
            request_id: "screenshot-review".into(),
            action: "Join this room and match its native mods".into(),
            packages: vec![
                PluginConsentPackage {
                    id: "terrain-engine".into(),
                    name: "Terrain Engine dependency".into(),
                    version: "2.3.1".into(),
                    authors: vec!["Map Systems Collective".into(), "A. Developer".into()],
                    source: source.clone(),
                    digest: digest.clone(),
                    approved: false,
                },
                PluginConsentPackage {
                    id: "bigmap".into(),
                    name: "Big Maps".into(),
                    version: "0.4.0".into(),
                    authors: vec!["Transport Fever Mod Team".into()],
                    source,
                    digest,
                    approved: false,
                },
            ],
        }),
        ..connected()
    }
}

fn member(
    id: &str,
    name: &str,
    platform: &str,
    owner: bool,
    you: bool,
    ready: bool,
    content: MemberContent,
) -> Member {
    Member {
        id: id.into(),
        name: name.into(),
        platform: platform.into(),
        ready,
        connected: true,
        owner,
        you,
        content,
        differs: None,
        banner: None,
        loading: None,
    }
}

fn members() -> Vec<Member> {
    vec![
        member(
            "p1",
            "Ann",
            "Windows x86-64",
            true,
            true,
            true,
            MemberContent::Same,
        ),
        member(
            "p2",
            "Bob",
            "Windows x86-64",
            false,
            false,
            true,
            MemberContent::Same,
        ),
        member(
            "p3",
            "Cat",
            "Linux x86-64",
            false,
            false,
            false,
            MemberContent::Differs,
        ),
    ]
}

fn room(phase: Phase, members: Vec<Member>) -> Room {
    Room {
        name: "Friday trains".into(),
        rules: "native".into(),
        phase,
        invite: Some("K7QM2X".into()),
        you_own: true,
        max_players: 4,
        has_password: false,
        members,
        competitive: false,
    }
}

fn chat() -> Vec<ChatLine> {
    vec![
        ChatLine {
            from: "Bob".into(),
            text: "I'll take the coal line up north.".into(),
            you: false,
        },
        ChatLine {
            from: "Ann".into(),
            text: "Fine by me, I'm on passengers.".into(),
            you: true,
        },
    ]
}

/// The window as it opens: the lobby in the game's menu (D17). Part of
/// [`screens`]: two tests rendering at once crash some GPU drivers.
fn screens_with_the_lobby_in_the_game() {
    render_in_game("g1-start-the-game", base());
    let attached = Game {
        attached: Some("40408".into()),
        ..Game::default()
    };
    render_in_game(
        "g2-game-running",
        State {
            game: attached.clone(),
            ..base()
        },
    );
    render_in_game(
        "g3-in-the-room",
        State {
            room: Some(room(Phase::Lobby, members())),
            chat: chat(),
            game: attached,
            ..connected()
        },
    );
    render_in_game(
        "g4-receiving-the-world",
        State {
            room: Some(room(Phase::Running, members())),
            game: Game {
                attached: Some("40408".into()),
                world: World::Fetching,
                bytes: 48_000_000,
                total: 112_000_000,
                step: None,
                speed: 100,
            },
            ..connected()
        },
    );
    render_in_game(
        "g5-playing",
        State {
            room: Some(room(Phase::Running, members())),
            notices: vec!["Bob joined the room.".into(), "The room started.".into()],
            game: Game {
                attached: Some("40408".into()),
                world: World::Playing,
                bytes: 0,
                total: 0,
                step: Some(18432),
                speed: 200,
            },
            ..connected()
        },
    );
}

#[test]
#[ignore = "renders images for review; needs a GPU or a software renderer"]
fn screens() {
    screens_with_the_lobby_in_the_game();
    render("1-not-connected", base(), 0.0);
    render(
        "2-connecting",
        State {
            connection: Connection::Connecting,
            ..base()
        },
        0.0,
    );
    render("3-connected", connected(), 0.0);
    render("9e-native-plugin-consent", native_plugin_consent(), 0.0);
    render_clicking(
        "9-settings",
        connected(),
        0.0,
        Some("Settings"),
        Place::Launcher,
    );
    // A release with several servers (D12's proposed amendment of
    // 2026-10-06): the setting names each with its ping.
    render_clicking(
        "9b-settings-servers",
        State {
            servers: vec![
                ServerRow {
                    name: "EU".into(),
                    ping_ms: Some(24),
                    here: true,
                    reachable: true,
                },
                ServerRow {
                    name: "US".into(),
                    ping_ms: Some(108),
                    here: false,
                    reachable: true,
                },
            ],
            ..connected()
        },
        0.0,
        Some("Settings"),
        Place::Launcher,
    );
    let lobby = State {
        room: Some(room(Phase::Lobby, members())),
        chat: chat(),
        content_diff: Some(Differences {
            summary: "Cat runs other mods than the room.".into(),
            game: None,
            missing: vec![],
            missing_more: 0,
            extra: vec!["gw_cheats_1 1".into()],
            extra_more: 0,
            changed: vec![],
            changed_more: 0,
            reordered: false,
            unlisted: false,
        }),
        ..connected()
    };
    render("4-in-the-lobby", lobby.clone(), 0.0);
    render("4b-in-the-lobby-scrolled", lobby, 420.0);
    let ready: Vec<Member> = members()
        .into_iter()
        .map(|member| Member {
            ready: true,
            content: MemberContent::Same,
            differs: None,
            ..member
        })
        .collect();
    render(
        "5-game-started",
        State {
            room: Some(room(Phase::Lobby, ready)),
            chat: chat(),
            game: Game {
                attached: Some("40391".into()),
                ..Game::default()
            },
            ..connected()
        },
        0.0,
    );
    render(
        "6-receiving-the-world",
        State {
            room: Some(room(Phase::Running, members())),
            chat: chat(),
            game: Game {
                attached: Some("40391".into()),
                world: World::Fetching,
                bytes: 48_000_000,
                total: 112_000_000,
                step: None,
                speed: 100,
            },
            ..connected()
        },
        0.0,
    );
    render(
        "7-playing",
        State {
            room: Some(room(Phase::Running, members())),
            chat: chat(),
            notices: vec![
                "Bob joined the room.".into(),
                "The room started.".into(),
                "Speed 2×.".into(),
            ],
            game: Game {
                attached: Some("40391".into()),
                world: World::Playing,
                bytes: 0,
                total: 0,
                step: Some(18432),
                speed: 200,
            },
            ..connected()
        },
        0.0,
    );
    for (name, button) in [
        ("9c-mod-manager", "Mod manager"),
        ("9d-mod-browser", "Mod browser"),
    ] {
        render_clicking(
            name,
            State {
                native: native(),
                ..connected()
            },
            0.0,
            Some(button),
            Place::Launcher,
        );
    }
    render(
        "4c-in-the-lobby-native-mods",
        State {
            room: Some(room(Phase::Lobby, members())),
            chat: chat(),
            native: native(),
            room_native: vec![
                RoomNativeRow {
                    id: "big-maps".into(),
                    name: "Big Maps".into(),
                    version: "1.2.0+3fa9c01b2e".into(),
                    have: ModHave::OtherVersion,
                },
                RoomNativeRow {
                    id: "faster-trains".into(),
                    name: "Faster Trains".into(),
                    version: "0.4.1+aa01b3c4d5".into(),
                    have: ModHave::No,
                },
                RoomNativeRow {
                    id: "terrain-tweaks".into(),
                    name: "Terrain Tweaks".into(),
                    version: "2.0.0+9be4410c7a".into(),
                    have: ModHave::Yes,
                },
            ],
            ..connected()
        },
        520.0,
    );
    render(
        "8-update-needed",
        State {
            outdated: true,
            error: Some("The server speaks a newer protocol: update TPF3-MP to play there.".into()),
            ..base()
        },
        0.0,
    );
}

#[test]
#[ignore = "renders an image for review; needs a GPU or a software renderer"]
fn native_plugin_consent_screen() {
    render("native-plugin-consent", native_plugin_consent(), 0.0);
}
