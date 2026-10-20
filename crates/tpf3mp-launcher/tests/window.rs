//! The launcher window, clicked through as a player would, over a stand-in
//! launcher that records what the window asks for.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;

use eframe::egui::{self, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use tpf3mp_agent::launcher::{
    Action, Connection, Differences, Game, InstalledGame, Member, MemberContent, Phase, Room,
    RulesChoice, State,
};
use tpf3mp_launcher::{
    app::{Extras, LauncherApp, Shown},
    backend::Backend,
    view::Place,
};

#[derive(Default)]
struct Recorder {
    state: RefCell<State>,
    actions: RefCell<Vec<Action>>,
}

impl Backend for Recorder {
    fn state(&self) -> State {
        self.state.borrow().clone()
    }

    fn act(&self, action: Action) {
        self.actions.borrow_mut().push(action);
    }

    fn busy(&self) -> bool {
        false
    }
}

/// The window with the lobby in it, as the page has it: most tests click
/// through that lobby.
fn window(state: State) -> Harness<'static, LauncherApp<Recorder>> {
    window_sized(state, 690.0, Place::Launcher)
}

/// The window as it opens: the lobby in the game's menu (D17).
fn window_for_the_game(state: State) -> Harness<'static, LauncherApp<Recorder>> {
    window_sized(state, 690.0, Place::Game)
}

/// A window this tall: tall enough, the room's players and chat are in
/// view without scrolling.
fn window_sized(
    state: State,
    height: f32,
    place: Place,
) -> Harness<'static, LauncherApp<Recorder>> {
    let recorder = Recorder {
        state: RefCell::new(state),
        ..Recorder::default()
    };
    let app = LauncherApp::new(
        recorder,
        Extras {
            updater: None,
            probe: None,
            notes: None,
            shown: Shown {
                update: None,
                installed_mod: Some(None),
            },
        },
    )
    .with_place(place);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, height))
        .build_ui_state(|ui, app: &mut LauncherApp<Recorder>| app.show(ui), app);
    harness.run_steps(4);
    harness
}

fn actions(harness: &Harness<'static, LauncherApp<Recorder>>) -> Vec<Action> {
    harness.state().backend().actions.borrow().clone()
}

fn member(name: &str, owner: bool, you: bool, ready: bool) -> Member {
    Member {
        id: format!("{name}-key"),
        name: name.to_owned(),
        platform: "Windows x86-64".to_owned(),
        ready,
        connected: true,
        owner,
        you,
        content: MemberContent::Same,
        differs: None,
        banner: None,
        loading: None,
    }
}

fn installed() -> Option<InstalledGame> {
    Some(InstalledGame {
        dir: r"C:\Games\Transport Fever 3".into(),
        build: "20364158".into(),
    })
}

fn in_room(members: Vec<Member>, you_own: bool) -> State {
    State {
        name: "Ann".into(),
        connection: Connection::Connected,
        installed: installed(),
        room: Some(Room {
            name: "Friday trains".into(),
            rules: "native".into(),
            phase: Phase::Lobby,
            invite: Some("K7QM2X".into()),
            you_own,
            max_players: 4,
            has_password: false,
            members,
            competitive: false,
        }),
        ..State::default()
    }
}

fn typed(window: &mut Harness<'static, LauncherApp<Recorder>>, field: &str, text: &str) {
    let input = window.get_by_role_and_label(Role::TextInput, field);
    input.focus();
    input.type_text(text);
    window.run_steps(4);
}

#[test]
fn connecting_uses_the_server_and_name_offered() {
    let mut window = window(State {
        name: "Ann".into(),
        server: Some("tpf3mp.example.org:29470".into()),
        ..State::default()
    });
    window.get_by_label("Connect").click();
    window.run_steps(4);
    assert_eq!(
        actions(&window),
        [Action::Connect {
            server: "tpf3mp.example.org:29470".into(),
            name: "Ann".into(),
        }]
    );
}

/// A build without a server of its own: the invite goes with the server
/// typed, and the launcher connects and joins in one step.
#[test]
fn an_invite_given_with_the_server_connects_and_joins() {
    let mut window = window(State::default());
    typed(&mut window, "SERVER", "tpf3mp.example.org:29470");
    typed(&mut window, "YOUR NAME", "Bob");
    typed(&mut window, "INVITE", "k7qm2x");
    window.get_by_label("Connect").click();
    window.run_steps(4);
    assert_eq!(
        actions(&window),
        [Action::Connect {
            server: "tpf3mp.example.org:29470 K7QM2X".into(),
            name: "Bob".into(),
        }]
    );
}

#[test]
fn a_room_is_created_with_the_rules_the_host_picks() {
    let mut window = window(State {
        name: "Ann".into(),
        connection: Connection::Connected,
        rules: vec![
            RulesChoice {
                name: "native".into(),
                description: "The game's own rules and economy".into(),
            },
            RulesChoice {
                name: "strict".into(),
                description: "Checked by the server".into(),
            },
        ],
        ..State::default()
    });
    typed(&mut window, "ROOM NAME", "Friday trains");
    window.get_by_label("Create room").click();
    window.run_steps(4);
    assert_eq!(
        actions(&window),
        [Action::Create {
            room: "Friday trains".into(),
            max_players: 4,
            password: None,
            rules: Some("native".into()),
            start_save: None,
            listing: None,
            competitive: false,
        }]
    );
}

#[test]
fn a_room_is_joined_with_its_code() {
    let mut window = window(State {
        name: "Ann".into(),
        connection: Connection::Connected,
        ..State::default()
    });
    window.get_by_label("Join with an invite").click();
    window.run_steps(4);
    window.get_by_label("Back to creating a room");
    typed(&mut window, "INVITE", "k7qm2x");
    window.get_by_label("Join room").click();
    window.run_steps(4);
    assert_eq!(
        actions(&window),
        [Action::Join {
            invite: "K7QM2X".into(),
            server: None,
            password: None,
        }]
    );
}

#[test]
fn an_invite_can_be_joined_on_a_picked_region_or_automatically() {
    use tpf3mp_agent::launcher::ServerRow;
    let servers = vec![
        ServerRow {
            name: "EU".into(),
            ping_ms: Some(24),
            here: true,
            reachable: true,
        },
        ServerRow {
            name: "US".into(),
            ping_ms: Some(110),
            here: false,
            reachable: true,
        },
    ];
    let state = || State {
        name: "Ann".into(),
        connection: Connection::Connected,
        regional: true,
        servers: servers.clone(),
        ..State::default()
    };

    let mut automatic = window(state());
    automatic.get_by_label("Join with an invite").click();
    automatic.run_steps(4);
    typed(&mut automatic, "INVITE", "K7QM2X");
    automatic.get_by_label("Join room").click();
    automatic.run_steps(4);
    assert_eq!(
        actions(&automatic),
        [Action::Join {
            invite: "K7QM2X".into(),
            server: None,
            password: None,
        }]
    );

    let mut picked = window(state());
    picked.get_by_label("Join with an invite").click();
    picked.run_steps(4);
    picked.get_by_role(Role::ComboBox).hover();
    picked.run_steps(2);
    picked.get_by_role(Role::ComboBox).click();
    picked.run_steps(4);
    picked.get_by_label("US").click();
    picked.run_steps(2);
    typed(&mut picked, "INVITE", "K7QM2X");
    picked.get_by_label("Join room").click();
    picked.run_steps(4);
    assert_eq!(
        actions(&picked),
        [Action::Join {
            invite: "K7QM2X".into(),
            server: Some("US".into()),
            password: None,
        }]
    );

    let mut after_server_change = window(state());
    after_server_change
        .get_by_label("Join with an invite")
        .click();
    after_server_change.run_steps(4);
    after_server_change.get_by_role(Role::ComboBox).hover();
    after_server_change.run_steps(2);
    after_server_change.get_by_role(Role::ComboBox).click();
    after_server_change.run_steps(4);
    after_server_change.get_by_label("US").click();
    after_server_change.run_steps(2);
    after_server_change
        .state()
        .backend()
        .state
        .borrow_mut()
        .server = Some("us.example.org:29470".into());
    after_server_change.run_steps(4);
    typed(&mut after_server_change, "INVITE", "K7QM2X");
    after_server_change.get_by_label("Join room").click();
    after_server_change.run_steps(4);
    assert_eq!(
        actions(&after_server_change),
        [Action::Join {
            invite: "K7QM2X".into(),
            server: None,
            password: None,
        }],
        "a choice made for another server does not follow the form"
    );
}

#[test]
fn a_regional_settings_card_only_offers_reset_for_a_pinned_trusted_server() {
    use tpf3mp_launcher::app::ServerSetting;
    let mut window = window(State {
        server: Some("us.example.org:29470".into()),
        server_name: Some("US".into()),
        server_default: Some("eu.example.org:29470".into()),
        regional: true,
        ..State::default()
    });
    window.get_by_label("Settings").click();
    window.run_steps(4);
    assert!(window.query_by_label("SERVER ADDRESS").is_none());
    assert!(window.query_by_label("Use this server").is_none());
    assert!(
        !ServerSetting::of(
            "evil.example.org:29470",
            &window.state().backend().state.borrow()
        )
        .can_apply
    );
    let reset = window.get_by_role_and_label(Role::Button, "Reset to default");
    assert!(!reset.accesskit_node().is_disabled());
    reset.hover();
    window.run_steps(2);
    window.get_by_label("Reset to default").click();
    window.run_steps(2);
    assert_eq!(
        actions(&window),
        [Action::SetServer {
            server: String::new()
        }]
    );
}

#[test]
fn the_owner_starts_once_everyone_is_ready() {
    let mut state = in_room(
        vec![
            member("Ann", true, true, true),
            member("Bob", false, false, true),
        ],
        true,
    );
    state.game = Game {
        attached: Some("40391".into()),
        ..Game::default()
    };
    let mut window = window(state);
    window.get_by_label("Start the game").click();
    window.run_steps(4);
    assert_eq!(actions(&window), [Action::Start]);
}

#[test]
fn removing_a_player_asks_first() {
    let mut window = window_sized(
        in_room(
            vec![
                member("Ann", true, true, false),
                member("Bob", false, false, false),
            ],
            true,
        ),
        1200.0,
        Place::Launcher,
    );
    window.get_by_label("Remove").click();
    window.run_steps(4);
    assert!(actions(&window).is_empty(), "nothing before the answer");
    window.get_by_label("Remove them").click();
    window.run_steps(4);
    assert_eq!(
        actions(&window),
        [Action::Kick {
            player: "Bob-key".into()
        }]
    );
}

#[test]
fn leaving_asks_first() {
    let mut window = window(in_room(vec![member("Ann", true, true, false)], true));
    window.get_by_label("Leave room").click();
    window.run_steps(4);
    window.get_by_label("Leave the room?");
    assert!(actions(&window).is_empty(), "nothing before the answer");
    window.get_by_label("Leave").click();
    window.run_steps(4);
    assert_eq!(actions(&window), [Action::Leave]);
}

#[test]
fn a_player_whose_mods_differ_sees_what_to_change() {
    let mut state = in_room(
        vec![
            member("Ann", true, false, false),
            Member {
                content: MemberContent::Differs,
                differs: None,
                ..member("Bob", false, true, false)
            },
        ],
        false,
    );
    state.content_diff = Some(Differences {
        summary: "you lack stations 3".into(),
        missing: vec!["stations 3".into()],
        changed: vec![("trains".into(), "1.2".into(), "1.1".into())],
        ..Differences::default()
    });
    let window = window(state);
    window.get_by_label("you lack stations 3");
    window.get_by_label("Mods you lack: stations 3.");
    window.get_by_label("Other versions: trains (room 1.2, you 1.1).");
    window.get_by_label("Other mods");
}

#[test]
fn a_launcher_older_than_the_server_says_so() {
    // As the launcher words it, naming the file that runs.
    let exe = std::path::Path::new("TPF3-MP.exe");
    let message = tpf3mp_agent::about::protocol_mismatch(5, 6, Some(exe));
    let window = window(State {
        name: "Ann".into(),
        outdated: true,
        error: Some(message.clone()),
        ..State::default()
    });
    window.get_by_label("Update needed");
    window.get_by_label("Update TPF3-MP to play here");
    window.get_by_label(&message);
    assert!(message.contains("too old for the server") && message.contains("TPF3-MP.exe"));
}

#[test]
fn the_operators_notice_stands_out() {
    let window = window(State {
        name: "Ann".into(),
        connection: Connection::Connected,
        announcement: Some("Restarting for an update in 5 minutes".into()),
        ..State::default()
    });
    window.get_by_label("From the server: Restarting for an update in 5 minutes");
}

#[test]
fn chat_is_sent_to_the_room() {
    let mut window = window_sized(
        in_room(vec![member("Ann", true, true, false)], true),
        1200.0,
        Place::Launcher,
    );
    let chat = window.get_by_role(Role::TextInput);
    chat.focus();
    chat.type_text("good luck");
    window.run_steps(4);
    window.get_by_label("Send").click();
    window.run_steps(4);
    assert_eq!(
        actions(&window),
        [Action::Chat {
            text: "good luck".into()
        }]
    );
}

#[test]
fn diagnostics_can_be_switched_off_in_settings() {
    let mut window = window(State {
        diagnostics: Some(true),
        ..State::default()
    });
    window.get_by_label("Settings").click();
    window.run_steps(4);
    window.get_by_label("Send diagnostics");
    // The pointer comes to the drop-down before it is pressed, as a
    // player's does.
    window.get_by_role(Role::ComboBox).hover();
    window.run_steps(2);
    window.get_by_role(Role::ComboBox).click();
    window.run_steps(4);
    window.get_by_label("Off").click();
    window.run_steps(4);
    assert_eq!(actions(&window), [Action::Diagnostics { on: false }]);
    window.get_by_label("Done").click();
    window.run_steps(4);
    assert!(window.query_by_label("Send diagnostics").is_none());

    // A launcher that sends none offers no switch.
    let mut window = self::window(State::default());
    window.get_by_label("Settings").click();
    window.run_steps(4);
    assert!(window.query_by_label("Send diagnostics").is_none());
}

#[test]
fn a_mod_with_code_says_so_and_one_this_launcher_cannot_run_is_not_offered() {
    use tpf3mp_agent::native_mods::{NativeRow, NativeView};
    let row = |id: &str, name: &str, code: bool, unsupported: Option<&str>| NativeRow {
        id: id.into(),
        name: name.into(),
        description: String::new(),
        homepage: None,
        authors: vec!["Ann".into()],
        preview: None,
        simulation: true,
        available: Some("1.0.0".into()),
        available_size: None,
        installed: None,
        installed_size: None,
        load_order: None,
        clash: None,
        code,
        unsupported: unsupported.map(str::to_owned),
        downloads: None,
        updated: None,
        previous: None,
        enabled: false,
        listed: true,
        proposed: None,
    };
    let state = |rows: Vec<NativeRow>| State {
        native: NativeView {
            available: true,
            serial: Some(3),
            server: Some("play.example:29470".into()),
            proposed_shown: false,
            packages: rows,
            problem: None,
        },
        ..State::default()
    };
    // Code of its own: said plainly, and still installed by choice.
    let mut window = window(state(vec![row("trains", "Faster Trains", true, None)]));
    window.get_by_label("Mod browser").click();
    window.run_steps(4);
    window.get_by_label("Runs code");
    window.get_by_label(
        "Runs its own code in the game, with your account's rights on this PC. Install it only \
         from a server you trust.",
    );
    // The executable-code warning can push the action below the details
    // pane's visible area. Reveal it before sending a physical pointer click.
    window.get_by_label("Install").scroll_to_me();
    window.run_steps(20);
    assert!(
        !window
            .get_by_label("Install")
            .accesskit_node()
            .is_disabled()
    );
    window.get_by_label("Install").hover();
    window.run_steps(2);
    window.get_by_label("Install").click();
    window.run_steps(2);
    assert_eq!(
        actions(&window),
        [Action::NativeInstall {
            id: "trains".into()
        }]
    );

    // A plugin for another platform: why, and no install.
    let mut window = self::window(state(vec![row(
        "trains",
        "Faster Trains",
        true,
        Some("it has no plugin for this platform"),
    )]));
    window.get_by_label("Mod browser").click();
    window.run_steps(4);
    window.get_by_label("Can't be installed: it has no plugin for this platform.");
    window.get_by_label("Install").scroll_to_me();
    window.run_steps(20);
    assert!(
        window
            .get_by_label("Install")
            .accesskit_node()
            .is_disabled()
    );
    window.get_by_label("Install").hover();
    window.run_steps(2);
    window.get_by_label("Install").click();
    window.run_steps(2);
    assert!(actions(&window).is_empty(), "the button is off");
}

#[test]
fn a_native_mod_that_clashes_cannot_be_switched_on() {
    use tpf3mp_agent::native_mods::{NativeRow, NativeView};
    let pages = NativeRow {
        id: "pages".into(),
        name: "Bigger Pages".into(),
        description: String::new(),
        homepage: None,
        authors: Vec::new(),
        preview: None,
        simulation: false,
        available: Some("1.0.0".into()),
        available_size: None,
        installed: Some("1.0.0".into()),
        installed_size: None,
        load_order: None,
        clash: Some("it clashes with big_maps: pages.own and bigmap.page both patch ui::page, and one takes it over".into()),
        downloads: None,
        updated: None,
        previous: None,
        enabled: false,
        listed: true,
        proposed: None,
        code: false,
        unsupported: None,
    };
    let mut window = window(State {
        native: NativeView {
            available: true,
            serial: Some(3),
            server: None,
            proposed_shown: false,
            packages: vec![pages],
            problem: None,
        },
        ..State::default()
    });
    window.get_by_label("Mod manager").click();
    window.run_steps(4);
    window.get_by_label(
        "Can't be switched on: it clashes with big_maps: pages.own and bigmap.page both patch \
         ui::page, and one takes it over. Switch that mod off first.",
    );
    window.get_by_label("Switch on").hover();
    window.run_steps(2);
    window.get_by_label("Switch on").click();
    window.run_steps(2);
    assert!(actions(&window).is_empty(), "the button is off");
}

#[test]
fn native_mods_are_managed_in_the_mod_manager_and_found_in_the_mod_browser() {
    use tpf3mp_agent::native_mods::{NativeRow, NativeView};
    let big_maps = NativeRow {
        id: "bigmap".into(),
        name: "Big Maps".into(),
        description: "Maps up to 128 km a side.".into(),
        homepage: Some("https://example.invalid/bigmap".into()),
        simulation: true,
        available: Some("0.4.0".into()),
        installed: Some("0.3.0".into()),
        previous: None,
        enabled: true,
        listed: true,
        proposed: None,
        authors: vec!["Ann".into(), "Bob".into()],
        preview: None,
        available_size: Some(5 << 20),
        installed_size: Some(4 << 20),
        load_order: Some(1),
        downloads: Some(1234),
        updated: Some("2026-09-30".into()),
        clash: None,
        code: false,
        unsupported: None,
    };
    let signs = NativeRow {
        id: "signs".into(),
        name: "Station Signs".into(),
        description: "Bigger signs at stations.".into(),
        homepage: None,
        simulation: false,
        available: Some("0.3.0".into()),
        installed: None,
        previous: None,
        enabled: false,
        listed: true,
        proposed: Some(tpf3mp_nativemods::registry::Proposal {
            number: 12,
            title: "Add Station Signs".into(),
            url: "https://example.invalid/pull/12".into(),
        }),
        authors: Vec::new(),
        preview: None,
        available_size: None,
        installed_size: None,
        load_order: None,
        downloads: None,
        updated: None,
        clash: None,
        code: false,
        unsupported: None,
    };
    let state = State {
        native: NativeView {
            available: true,
            serial: Some(3),
            server: Some("play.example:29470".into()),
            proposed_shown: true,
            packages: vec![big_maps, signs],
            problem: None,
        },
        ..State::default()
    };

    // The mod manager lists what is installed, and only that, with the
    // first picked: its details beside the table.
    let mut window = window(state.clone());
    window.get_by_label("Mod manager").click();
    window.run_steps(4);
    assert_eq!(
        window.query_all_by_label("Big Maps").count(),
        2,
        "its row, and its details"
    );
    window.get_by_label("0.4.0 available");
    window.get_by_label("by Ann and Bob");
    assert!(window.query_by_label("Station Signs").is_none());
    // The pointer comes to the button before it is pressed, as a player's
    // does.
    window.get_by_label("Update").hover();
    window.run_steps(2);
    window.get_by_label("Update").click();
    window.run_steps(2);
    window.get_by_label("Switch off").click();
    window.run_steps(2);
    assert_eq!(
        actions(&window),
        [
            Action::NativeInstall {
                id: "bigmap".into()
            },
            Action::NativeEnable {
                id: "bigmap".into(),
                on: false
            },
        ]
    );

    // The mod browser lists what the server offers and the proposed ones;
    // a mod picked in its table shows its details, and a proposed one is
    // installed as proposed.
    let mut window = self::window(state);
    window.get_by_label("Mod browser").click();
    window.run_steps(4);
    assert!(
        window
            .query_by_label("Proposed in pull request #12: not reviewed")
            .is_none()
    );
    window.get_by_label("Station Signs").hover();
    window.run_steps(2);
    window.get_by_label("Station Signs").click();
    window.run_steps(4);
    assert_eq!(
        window.query_all_by_label("Station Signs").count(),
        2,
        "picked"
    );
    window.get_by_label("Proposed in pull request #12: not reviewed");
    window.get_by_label("Install").hover();
    window.run_steps(2);
    window.get_by_label("Install").click();
    window.run_steps(2);
    assert_eq!(
        actions(&window),
        [Action::NativeInstallProposed { id: "signs".into() }]
    );
    // The search finds mods by name, author or description.
    let search = window.get_by_role_and_label(Role::TextInput, "SEARCH");
    search.focus();
    search.type_text("ann");
    window.run_steps(4);
    assert!(window.query_by_label("Station Signs").is_none());
    assert_eq!(window.query_all_by_label("Big Maps").count(), 2);

    // Without an index from the server or the project, the browser says
    // the server offers none, and the manager that none is installed.
    let mut window = self::window(State::default());
    window.get_by_label("Mod browser").click();
    window.run_steps(4);
    window.get_by_label("Your server offers no native mods yet.");
    window.get_by_label("Installed (0)").click();
    window.run_steps(4);
    window.get_by_label("No native mods installed yet");
}

#[test]
fn the_game_is_started_from_a_room() {
    let mut window = window(in_room(vec![member("Ann", true, true, false)], true));
    window.get_by_label("Start Transport Fever 3").click();
    window.run_steps(2);
    assert_eq!(actions(&window), [Action::LaunchGame]);

    // Without the game installed, it cannot start.
    let mut state = in_room(vec![member("Ann", true, true, false)], true);
    state.installed = None;
    let mut window = self::window(state);
    window.get_by_label("Start Transport Fever 3").click();
    window.run_steps(2);
    assert!(actions(&window).is_empty());

    // Outside a room there is nothing for the game to connect to.
    let window = self::window(State {
        connection: Connection::Connected,
        ..State::default()
    });
    assert!(window.query_by_label("Start Transport Fever 3").is_none());
}

/// A package built for its own server offers no other (D12): the server
/// is shown, not asked for, and an invite may go with the name.
#[test]
fn a_package_with_its_own_server_offers_no_other() {
    let own = State {
        name: "Ann".into(),
        server: Some("tpf3mp.example.org:29470".into()),
        server_fixed: true,
        ..State::default()
    };
    let mut window = window(own.clone());
    assert!(
        window
            .query_by_role_and_label(Role::TextInput, "SERVER")
            .is_none(),
        "no server to type"
    );
    window.get_by_label("tpf3mp.example.org:29470");
    // A package that names its server shows the name, not the address.
    let named = self::window(State {
        server_name: Some("EU".into()),
        ..own.clone()
    });
    named.get_by_label("EU");
    assert!(named.query_by_label("tpf3mp.example.org:29470").is_none());
    window.get_by_label("Connect").click();
    window.run_steps(2);
    assert_eq!(
        actions(&window),
        [Action::Connect {
            server: String::new(),
            name: "Ann".into(),
        }]
    );

    let mut window = self::window(own);
    typed(&mut window, "INVITE", "K7QM2X");
    window.get_by_label("Connect").click();
    window.run_steps(2);
    assert_eq!(
        actions(&window),
        [Action::Connect {
            server: "K7QM2X".into(),
            name: "Ann".into(),
        }]
    );
}

#[test]
fn by_default_the_window_starts_the_game_and_the_lobby_is_in_its_menu() {
    let state = State {
        name: "Ann".into(),
        server: Some("tpf3mp.example.org:29470".into()),
        server_fixed: true,
        server_name: Some("EU".into()),
        installed: installed(),
        ..State::default()
    };
    let mut window = window_for_the_game(state);
    // No lobby forms here: the game's Multiplayer window has them.
    assert!(window.query_by_label("Connect").is_none());
    assert!(
        window
            .query_by_role_and_label(Role::TextInput, "YOUR NAME")
            .is_none()
    );
    window.get_by_label("HOW TO PLAY: IN THE GAME");
    window.get_by_label("Click Multiplayer on its main menu");
    window.get_by_label("Start Transport Fever 3").click();
    window.run_steps(2);
    assert_eq!(actions(&window), [Action::LaunchGame]);

    // The lobby comes back here with one click, and goes again.
    window.get_by_label("Lobby in this window instead").click();
    window.run_steps(4);
    window.get_by_label("Connect");
    window
        .get_by_label("Lobby in the game's menu instead")
        .click();
    window.run_steps(4);
    assert!(window.query_by_label("Connect").is_none());
}

#[test]
fn in_a_room_the_window_shows_it_but_its_buttons_are_in_the_game() {
    let mut state = in_room(
        vec![
            member("Ann", true, true, true),
            member("Bob", false, false, false),
        ],
        true,
    );
    state.game = Game {
        attached: Some("40408".into()),
        ..Game::default()
    };
    // In the lobby here, the same room has them.
    let here = window_sized(state.clone(), 1200.0, Place::Launcher);
    for there in ["Remove", "Send", "Leave room", "Ready", "Copy invite"] {
        assert!(
            here.query_by_role_and_label(Role::Button, there).is_some(),
            "{there}"
        );
    }
    let window = window_for_the_game(state);
    window.get_by_label("Friday trains");
    window.get_by_label("Bob");
    window.get_by_label("Continue in the game");
    window.get_by_label(
        "Chat, Ready, Start and removing players are in the game's Multiplayer window.",
    );
    for gone in [
        "Remove",
        "Send",
        "Leave room",
        "Ready",
        "Not ready",
        "Copy invite",
    ] {
        assert!(
            window.query_by_role_and_label(Role::Button, gone).is_none(),
            "{gone}"
        );
    }
    assert!(
        window.query_by_role(Role::TextInput).is_none(),
        "no chat field"
    );
}

fn on_the_relay() -> State {
    State {
        name: "Ann".into(),
        server: Some("relay.example.org:29470".into()),
        server_fixed: true,
        server_default: Some("relay.example.org:29470".into()),
        server_name: Some("Relay".into()),
        connection: Connection::Connected,
        ..State::default()
    }
}

/// Replaces what the field holds with `text`, as a player selecting it all
/// and typing would.
fn retyped(window: &mut Harness<'static, LauncherApp<Recorder>>, field: &str, text: &str) {
    let input = window.get_by_role_and_label(Role::TextInput, field);
    input.focus();
    window.run_steps(1);
    window.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    window.run_steps(1);
    window
        .get_by_role_and_label(Role::TextInput, field)
        .type_text(text);
    window.run_steps(4);
}

/// The server setting (D12, as amended): Settings shows the server played
/// on, takes another as host:port only, and goes back to the default.
#[test]
fn the_server_is_changed_in_settings() {
    let mut window = window(on_the_relay());
    window.get_by_label("Settings").click();
    window.run_steps(4);
    window.get_by_label("You play on Relay (relay.example.org:29470), the default server.");
    let reset = window.get_by_role_and_label(Role::Button, "Reset to default");
    assert!(
        reset.accesskit_node().is_disabled(),
        "already on the default"
    );
    assert!(
        window
            .get_by_role_and_label(Role::Button, "Use this server")
            .accesskit_node()
            .is_disabled(),
        "the field holds the server played on"
    );

    // Not a host:port: said, and not taken.
    retyped(&mut window, "SERVER ADDRESS", "eu.example.org");
    window.get_by_label("the server must be host:port, such as tpf3mp.example.org:29470");
    window.get_by_label("Use this server").click();
    window.run_steps(2);
    assert!(actions(&window).is_empty());

    retyped(&mut window, "SERVER ADDRESS", "eu.example.org:29470");
    assert!(
        window
            .query_by_label("the server must be host:port, such as tpf3mp.example.org:29470")
            .is_none()
    );
    window.get_by_label("Use this server").click();
    window.run_steps(2);
    assert_eq!(
        actions(&window),
        [Action::SetServer {
            server: "eu.example.org:29470".into()
        }]
    );

    // On another server: its address, and a way back.
    let mut window = self::window(State {
        server: Some("eu.example.org:29470".into()),
        server_name: None,
        ..on_the_relay()
    });
    window.get_by_label("Settings").click();
    window.run_steps(4);
    window.get_by_label("You play on eu.example.org:29470.");
    let reset = window.get_by_role_and_label(Role::Button, "Reset to default");
    assert!(!reset.accesskit_node().is_disabled());
    reset.hover();
    window.run_steps(2);
    window.get_by_label("Reset to default").click();
    window.run_steps(2);
    assert_eq!(
        actions(&window),
        [Action::SetServer {
            server: String::new()
        }]
    );
}

#[test]
fn the_server_stays_while_in_a_room() {
    let mut state = in_room(vec![member("Ann", true, true, true)], true);
    state.server = Some("eu.example.org:29470".into());
    state.server_fixed = true;
    state.server_default = Some("relay.example.org:29470".into());
    let mut window = window_sized(state, 1200.0, Place::Launcher);
    window.get_by_label("Settings").click();
    window.run_steps(4);
    window.get_by_label("Leave the room to change the server.");
    assert!(
        window
            .get_by_role_and_label(Role::Button, "Reset to default")
            .accesskit_node()
            .is_disabled()
    );
    retyped(&mut window, "SERVER ADDRESS", "us.example.org:29470");
    assert!(
        window
            .get_by_role_and_label(Role::Button, "Use this server")
            .accesskit_node()
            .is_disabled()
    );
    window.get_by_label("Use this server").click();
    window.run_steps(2);
    assert!(actions(&window).is_empty());
}

#[test]
fn the_server_setting_checks_what_was_typed() {
    use tpf3mp_launcher::app::ServerSetting;
    let state = on_the_relay();
    let setting = |typed: &str| ServerSetting::of(typed, &state);
    assert!(setting("eu.example.org:29470").can_apply);
    assert!(
        !setting(" RELAY.example.org:29470 ").can_apply,
        "the same server"
    );
    assert!(!setting("").can_apply && setting("").problem.is_none());
    assert!(setting("eu.example.org").problem.is_some());
    assert!(!setting("anything").can_reset, "already the default");
    let elsewhere = State {
        server: Some("eu.example.org:29470".into()),
        ..on_the_relay()
    };
    assert!(ServerSetting::of("", &elsewhere).can_reset);
}

#[test]
fn on_the_releases_servers_settings_name_each_with_its_ping() {
    use tpf3mp_agent::launcher::ServerRow;
    use tpf3mp_launcher::app::{ServerSetting, server_setting_line, servers_line};
    let state = State {
        // Played on the farther server, after a room there: still the
        // default, so nothing to reset.
        server: Some("us.example.org:29470".into()),
        servers: vec![
            ServerRow {
                name: "EU".into(),
                ping_ms: Some(24),
                here: false,
                reachable: true,
            },
            ServerRow {
                name: "US".into(),
                ping_ms: Some(110),
                here: true,
                reachable: true,
            },
            ServerRow {
                name: "Asia".into(),
                ping_ms: None,
                here: false,
                reachable: false,
            },
        ],
        ..on_the_relay()
    };
    assert_eq!(
        servers_line(&state).as_deref(),
        Some("EU · 24 ms, US · 110 ms (you are here), Asia · not answering")
    );
    assert!(server_setting_line(&state).contains("Rooms you host go to the closest"));
    assert!(!ServerSetting::of("", &state).can_reset);
    assert_eq!(servers_line(&on_the_relay()), None, "one server: as before");
}
