//! What the window shows, worked out from the launcher's state alone: every
//! label, button and line, as the page's `view.js` works them out (D20).
//! It draws nothing, so its tests need no window.
//!
//! The room's lobby is in the game (D17, amended 2026-09-30): by default
//! the window only starts the game and shows where things stand
//! ([`present_in_game`]); the lobby the page has ([`present`]) stays one
//! click away, for a game whose menu the hook cannot reach.

use tpf3mp_agent::launcher::{
    Action, Connection, Differences, Member, MemberContent, Phase, State, World,
};

use crate::{probe::Reach, theme::Pill, update::UpdateState};

/// Where the player uses the room's lobby.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Place {
    /// In the game's Multiplayer window, on its main menu (D17): this
    /// window starts the game and shows where things stand.
    #[default]
    Game,
    /// In this window, as the page has it.
    Launcher,
}

/// A form the panel shows over its main button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Connect,
    Create,
    Join,
}

/// What the main button does.
#[derive(Debug, Clone, PartialEq)]
pub enum Does {
    /// Sends the form it is under.
    Submit(Form),
    Act(Action),
    /// Installs the downloaded update and restarts.
    InstallUpdate,
    /// Nothing: it only says what is happening.
    Nothing,
}

/// The big button.
#[derive(Debug, Clone, PartialEq)]
pub struct Main {
    pub label: String,
    pub icon: &'static str,
    pub does: Does,
    /// From 0 to 1 while the world downloads.
    pub progress: Option<f32>,
}

impl Main {
    pub fn enabled(&self) -> bool {
        self.does != Does::Nothing
    }
}

/// What a quieter button does.
#[derive(Debug, Clone, PartialEq)]
pub enum Then {
    /// Opens a form in place of the main one, or closes it again.
    Open(Form),
    Act(Action),
    Copy(String),
    /// Asks first, then leaves the room.
    Leave,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Secondary {
    pub label: &'static str,
    pub icon: &'static str,
    pub then: Then,
    pub danger: bool,
}

/// A line of the panel's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub label: &'static str,
    pub value: String,
    pub large: bool,
}

/// How the line under the buttons reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Error,
    Ready,
    Update,
}

/// A player of the room, as the list draws them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Player {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub badges: Vec<(&'static str, Pill)>,
    pub removable: bool,
}

/// What the launcher-update badge and Settings say about updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Updates {
    pub badge: Option<String>,
    pub copy: String,
    pub installable: bool,
}

/// Everything the window draws.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub pill: (&'static str, Pill),
    pub rows: Vec<Row>,
    pub main: Main,
    pub secondary: Vec<Secondary>,
    pub status: Option<(String, Tone)>,
    pub steps: Vec<(&'static str, bool)>,
    pub notes: Vec<String>,
    pub differences: Vec<String>,
    pub players: Vec<Player>,
    pub updates: Updates,
}

/// The room's speed, as players read it: "2×", "paused".
pub fn speed_text(percent: u32) -> String {
    if percent == 0 {
        return "paused".into();
    }
    if percent.is_multiple_of(100) {
        format!("{}×", percent / 100)
    } else {
        let text = format!("{:.2}", f64::from(percent) / 100.0);
        format!("{}×", text.trim_end_matches('0'))
    }
}

/// Bytes as "12.3 MB".
pub fn size_text(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let value = bytes as f64;
    if value >= 1e9 {
        format!("{:.1} GB", value / 1e9)
    } else if value >= 1e6 {
        format!("{:.1} MB", value / 1e6)
    } else if value >= 1e3 {
        format!("{} kB", (value / 1e3).round())
    } else {
        format!("{bytes} B")
    }
}

fn server_label(state: &State) -> String {
    state
        .server_name
        .clone()
        .or_else(|| state.server.clone())
        .unwrap_or_default()
}

fn reach_text(reach: Reach) -> &'static str {
    match reach {
        Reach::Online => "online",
        Reach::Offline => "offline",
        Reach::Unknown => "checking",
    }
}

/// The five steps of a game together, and which are done.
pub fn steps(state: &State) -> Vec<(&'static str, bool)> {
    let room = state.room.as_ref();
    let running = room.is_some_and(|room| room.phase == Phase::Running);
    let everyone = running
        || room.is_some_and(|room| {
            !room.members.is_empty() && room.members.iter().all(|member| member.ready)
        });
    vec![
        (
            "Connect to a server",
            state.connection == Connection::Connected,
        ),
        ("Create or join a room", room.is_some()),
        ("Start the game from here", state.game.attached.is_some()),
        ("Everyone ready", everyone),
        ("Play together", running),
    ]
}

fn pill(state: &State) -> (&'static str, Pill) {
    if state.outdated {
        return ("Update needed", Pill::Update);
    }
    match state.connection {
        Connection::Connecting => return ("Connecting", Pill::Unknown),
        Connection::Disconnected => return ("Not connected", Pill::Unknown),
        Connection::Connected => {}
    }
    let Some(room) = &state.room else {
        return ("Connected", Pill::Ready);
    };
    if state.game.world == World::Playing {
        ("Playing", Pill::Ready)
    } else if room.phase == Phase::Running {
        ("Game running", Pill::Update)
    } else {
        ("In the lobby", Pill::Update)
    }
}

fn you(state: &State) -> Option<&Member> {
    state
        .room
        .as_ref()?
        .members
        .iter()
        .find(|member| member.you)
}

fn main_action(state: &State, update: &UpdateState) -> Main {
    let main = |label: &str, icon, does| Main {
        label: label.to_owned(),
        icon,
        does,
        progress: None,
    };
    if state.outdated {
        return if matches!(update, UpdateState::Ready { .. }) {
            main("Restart and update", "download", Does::InstallUpdate)
        } else {
            main("Update TPF3-MP to play here", "download", Does::Nothing)
        };
    }
    match state.connection {
        Connection::Connecting => return main("Connecting…", "link", Does::Nothing),
        Connection::Disconnected => return main("Connect", "link", Does::Submit(Form::Connect)),
        Connection::Connected => {}
    }
    let Some(room) = &state.room else {
        return main("Create room", "users", Does::Submit(Form::Create));
    };
    let game = &state.game;
    if game.attached.is_none() {
        let does = if state.installed.is_some() {
            Does::Act(Action::LaunchGame)
        } else {
            Does::Nothing
        };
        return main("Start Transport Fever 3", "play", does);
    }
    match game.world {
        World::Fetching => {
            #[allow(clippy::cast_precision_loss)]
            let done = if game.total > 0 {
                game.bytes as f32 / game.total as f32
            } else {
                0.0
            };
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let percent = (done * 100.0).floor() as u32;
            let label = if percent > 0 {
                format!("Receiving the world {percent}%")
            } else {
                "Receiving the world…".to_owned()
            };
            return Main {
                label,
                icon: "download",
                does: Does::Nothing,
                progress: Some(done),
            };
        }
        World::Loading => {
            return Main {
                progress: Some(0.0),
                ..main("Loading the world…", "download", Does::Nothing)
            };
        }
        World::Playing => {
            return main(
                &format!("Playing · {}", speed_text(u32::from(game.speed))),
                "play",
                Does::Nothing,
            );
        }
        World::None => {}
    }
    if room.phase == Phase::Running {
        return main("Joining the game…", "play", Does::Nothing);
    }
    let everyone = !room.members.is_empty() && room.members.iter().all(|member| member.ready);
    if room.you_own && everyone {
        return main("Start the game", "play", Does::Act(Action::Start));
    }
    // The ready button says where the player stands; a click turns it over.
    if you(state).is_some_and(|me| !me.ready) {
        return main(
            "Not ready",
            "close",
            Does::Act(Action::Ready { ready: true }),
        );
    }
    let waiting = if room.you_own {
        "Waiting for everyone to be ready"
    } else {
        "Waiting for the owner to start"
    };
    main(waiting, "play", Does::Nothing)
}

fn secondary(state: &State) -> Vec<Secondary> {
    // Before a room the game may start too: its main menu's Multiplayer
    // window connects, creates and joins through this launcher (D17).
    let start_game =
        (state.room.is_none() && state.installed.is_some() && state.game.attached.is_none())
            .then_some(Secondary {
                label: "Start Transport Fever 3",
                icon: "play",
                then: Then::Act(Action::LaunchGame),
                danger: false,
            });
    if state.connection != Connection::Connected {
        return start_game.into_iter().collect();
    }
    let Some(room) = &state.room else {
        return [
            Some(Secondary {
                label: "Join with an invite",
                icon: "users",
                then: Then::Open(Form::Join),
                danger: false,
            }),
            start_game,
            Some(Secondary {
                label: "Disconnect",
                icon: "close",
                then: Then::Act(Action::Disconnect),
                danger: false,
            }),
        ]
        .into_iter()
        .flatten()
        .collect();
    };
    let mut buttons = Vec::new();
    let me = you(state);
    // The ready button says where the player stands, Ready or Not ready;
    // a click turns it over.
    if room.phase == Phase::Lobby && me.is_some_and(|me| me.ready) {
        buttons.push(Secondary {
            label: "Ready",
            icon: "check",
            then: Then::Act(Action::Ready { ready: false }),
            danger: false,
        });
    } else if room.phase == Phase::Lobby && me.is_some() && state.game.attached.is_none() {
        buttons.push(Secondary {
            label: "Not ready",
            icon: "close",
            then: Then::Act(Action::Ready { ready: true }),
            danger: false,
        });
    }
    if let Some(invite) = &room.invite {
        buttons.push(Secondary {
            label: "Copy invite",
            icon: "archive",
            then: Then::Copy(invite.clone()),
            danger: false,
        });
    }
    buttons.push(Secondary {
        label: "Leave room",
        icon: "close",
        then: Then::Leave,
        danger: true,
    });
    buttons
}

fn status(state: &State, reach: Reach) -> Option<(String, Tone)> {
    if let Some(error) = &state.error {
        return Some((error.clone(), Tone::Error));
    }
    if state.outdated {
        return Some((
            "This TPF3-MP is older than the server's. Update to play there.".into(),
            Tone::Update,
        ));
    }
    if state.connection != Connection::Connected {
        if state.server_fixed && reach == Reach::Offline {
            return Some((
                format!(
                    "The server {} does not answer right now.",
                    server_label(state)
                ),
                Tone::Error,
            ));
        }
        return None;
    }
    if state.room.is_none() {
        return Some((
            "Create a room, or join one with the invite a friend sent you.".into(),
            Tone::Plain,
        ));
    }
    if state.installed.is_none() {
        return Some((
            "Transport Fever 3 was not found in Steam. Install it, then come back here.".into(),
            Tone::Error,
        ));
    }
    let game = &state.game;
    let Some(build) = &game.attached else {
        return Some((
            "Start Transport Fever 3 from here: only a game TPF3-MP starts joins the room. \
             Started from Steam, it is the plain game."
                .into(),
            Tone::Plain,
        ));
    };
    match game.world {
        World::Fetching if game.total > 0 => Some((
            format!(
                "{} of {} received.",
                size_text(game.bytes),
                size_text(game.total)
            ),
            Tone::Plain,
        )),
        World::Playing => {
            let step = game
                .step
                .map(|step| format!("Step {step}, "))
                .unwrap_or_default();
            let speed = if game.speed == 0 {
                "paused".to_owned()
            } else {
                format!("at {}", speed_text(u32::from(game.speed)))
            };
            Some((format!("{step}{speed}."), Tone::Ready))
        }
        // Readiness is automatic: the agent marks the player ready once the
        // game has a world up with the mod linked.
        World::None if you(state).is_some_and(|me| me.ready) => Some((
            format!(
                "The game is connected ({build}). You are ready; the world loads when the room starts."
            ),
            Tone::Plain,
        )),
        World::None => Some((
            format!(
                "The game is connected ({build}). Load your save in the game: you are marked \
                 ready automatically once its world is up."
            ),
            Tone::Plain,
        )),
        _ => None,
    }
}

fn rows(state: &State, reach: Reach) -> Vec<Row> {
    let mut rows = Vec::new();
    let server = server_label(state);
    if !server.is_empty() {
        let reach = if state.connection == Connection::Connected {
            "connected"
        } else {
            reach_text(reach)
        };
        rows.push(Row {
            label: "Server",
            value: format!("{server} · {reach}"),
            large: false,
        });
    }
    if let Some(room) = &state.room {
        rows.push(Row {
            label: "Room",
            value: room.name.clone(),
            large: true,
        });
        rows.push(Row {
            label: "Players",
            value: format!("{} of {}", room.members.len(), room.max_players),
            large: false,
        });
    } else if state.connection == Connection::Connected
        && let Some(version) = &state.server_version
    {
        // The second line is drawn large, as the page's style sheet has it.
        rows.push(Row {
            label: "Server version",
            value: version.clone(),
            large: true,
        });
    }
    rows
}

/// How this player's game differs from the room's, as lines.
pub fn differences(diff: Option<&Differences>) -> Vec<String> {
    let Some(diff) = diff else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    if let Some((room, yours)) = &diff.game {
        lines.push(format!(
            "Game build: the room runs {room}, you run {yours}."
        ));
    }
    let mut list = |title: &str, items: Vec<String>, more: u32| {
        if items.is_empty() {
            return;
        }
        let more = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        lines.push(format!("{title} {}{more}.", items.join(", ")));
    };
    list("Mods you lack:", diff.missing.clone(), diff.missing_more);
    list(
        "Mods the room lacks (turn them off):",
        diff.extra.clone(),
        diff.extra_more,
    );
    list(
        "Other versions:",
        diff.changed
            .iter()
            .map(|(name, room, yours)| format!("{name} (room {room}, you {yours})"))
            .collect(),
        diff.changed_more,
    );
    if diff.reordered {
        lines.push("The same mods load in another order.".into());
    }
    if diff.unlisted {
        lines.push("The mods beyond the listed ones differ.".into());
    }
    lines
}

fn players(state: &State) -> Vec<Player> {
    let Some(room) = &state.room else {
        return Vec::new();
    };
    room.members
        .iter()
        .map(|member| {
            let mut badges = Vec::new();
            if member.owner {
                badges.push(("Owner", Pill::Ready));
            }
            if !member.connected {
                badges.push(("Away", Pill::Unknown));
            }
            if room.phase == Phase::Lobby {
                badges.push(if member.ready {
                    ("Ready", Pill::Ready)
                } else {
                    ("Not ready", Pill::Unknown)
                });
            }
            if !member.owner {
                match member.content {
                    MemberContent::Same => badges.push(("Same mods", Pill::Ready)),
                    MemberContent::Differs => badges.push(("Other mods", Pill::Update)),
                    MemberContent::Unknown => {}
                }
            }
            Player {
                id: member.id.clone(),
                name: if member.you {
                    format!("{} (you)", member.name)
                } else {
                    member.name.clone()
                },
                platform: member.platform.clone(),
                badges,
                removable: room.you_own && !member.you,
            }
        })
        .collect()
}

/// What the badge and Settings say about updates.
pub fn updates(update: &UpdateState) -> Updates {
    let quiet = |copy: String| Updates {
        badge: None,
        copy,
        installable: false,
    };
    match update {
        UpdateState::Ready { version } => Updates {
            badge: Some(format!("Launcher update · v{version}")),
            copy: format!("Version {version} is ready. It installs when you restart the launcher."),
            installable: true,
        },
        UpdateState::Downloading {
            version,
            bytes,
            total,
        } => {
            let percent = if *total > 0 {
                format!(" {}%", bytes.saturating_mul(100) / total)
            } else {
                String::new()
            };
            quiet(format!("Downloading version {version}…{percent}"))
        }
        UpdateState::Installing { version } => quiet(format!("Installing version {version}…")),
        UpdateState::Checking => quiet("Checking for launcher updates…".into()),
        UpdateState::UpToDate => quiet(format!(
            "Launcher {} is up to date.",
            env!("CARGO_PKG_VERSION")
        )),
        UpdateState::Failed(reason) => quiet(format!("Could not update: {reason}")),
        UpdateState::Off(reason) => quiet(format!("Updates are off: {reason}.")),
    }
}

/// The five steps of playing from the game's Multiplayer window, and which
/// are done.
pub fn steps_in_game(state: &State) -> Vec<(&'static str, bool)> {
    let room = state.room.as_ref();
    let running = room.is_some_and(|room| room.phase == Phase::Running);
    let everyone = running
        || room.is_some_and(|room| {
            !room.members.is_empty() && room.members.iter().all(|member| member.ready)
        });
    vec![
        (
            "Start Transport Fever 3 from here",
            state.game.attached.is_some() || running,
        ),
        (
            "Click Multiplayer on its main menu",
            state.connection == Connection::Connected,
        ),
        ("Create a room, or join with an invite", room.is_some()),
        ("Everyone ready", everyone),
        ("Play together", running),
    ]
}

/// The big button when the lobby is in the game: it starts the game, then
/// says how the room's world comes along.
fn main_in_game(state: &State, update: &UpdateState) -> Main {
    let main = |label: &str, icon, does| Main {
        label: label.to_owned(),
        icon,
        does,
        progress: None,
    };
    if state.outdated {
        return main_action(state, update);
    }
    if state.game.attached.is_none() {
        let does = if state.installed.is_some() {
            Does::Act(Action::LaunchGame)
        } else {
            Does::Nothing
        };
        return main("Start Transport Fever 3", "play", does);
    }
    let running = state
        .room
        .as_ref()
        .is_some_and(|room| room.phase == Phase::Running);
    if state.connection == Connection::Connected && (state.game.world != World::None || running) {
        // Receiving, loading, playing: as the lobby's button says it.
        return main_action(state, update);
    }
    main("Continue in the game", "users", Does::Nothing)
}

/// The line under the buttons when the lobby is in the game.
fn status_in_game(state: &State, reach: Reach) -> Option<(String, Tone)> {
    if state.error.is_some() || state.outdated {
        return status(state, reach);
    }
    if state.installed.is_none() {
        return Some((
            "Transport Fever 3 was not found in Steam. Install it, then come back here.".into(),
            Tone::Error,
        ));
    }
    let Some(build) = &state.game.attached else {
        return Some((
            "Start Transport Fever 3 from here, then click Multiplayer on its main menu to \
             connect, create a room or join one. Started from Steam, it is the plain game."
                .into(),
            Tone::Plain,
        ));
    };
    if state.connection != Connection::Connected {
        if state.server_fixed && reach == Reach::Offline {
            return status(state, reach);
        }
        return Some((
            format!(
                "The game is running ({build}). Click Multiplayer on its main menu to connect."
            ),
            Tone::Plain,
        ));
    }
    let Some(room) = &state.room else {
        return Some((
            "Connected. Create a room or join one in the game's Multiplayer window.".into(),
            Tone::Plain,
        ));
    };
    match state.game.world {
        World::None if room.phase == Phase::Lobby => Some((
            "In the room: its players, chat, Ready and Start are in the game's Multiplayer \
             window."
                .into(),
            Tone::Plain,
        )),
        _ => status(state, reach),
    }
}

/// Everything the window draws when the lobby is in the game (D17): the
/// big button starts the game, and the rest says where things stand. The
/// room is shown, not played: its buttons are in the game's window.
pub fn present_in_game(state: &State, reach: Reach, update: Option<&UpdateState>) -> View {
    let mut view = present(state, reach, update);
    let none = UpdateState::Off("this launcher does not update itself".into());
    view.main = main_in_game(state, update.unwrap_or(&none));
    view.secondary = Vec::new();
    view.status = status_in_game(state, reach);
    view.steps = steps_in_game(state);
    for player in &mut view.players {
        player.removable = false;
    }
    view
}

/// Everything the window draws, for `state`, the server's `reach` and the
/// updater's state (`None` without an updater).
pub fn present(state: &State, reach: Reach, update: Option<&UpdateState>) -> View {
    let none = UpdateState::Off("this launcher does not update itself".into());
    let update = update.unwrap_or(&none);
    let mut notes = Vec::new();
    if let Some(text) = &state.announcement {
        notes.push(format!("From the server: {text}"));
    }
    if let Some(diff) = &state.content_diff {
        notes.push(diff.summary.clone());
    }
    if state.tunneled {
        notes.push("Connected through the WebSocket fallback: your network blocks UDP.".into());
    }
    View {
        pill: pill(state),
        rows: rows(state, reach),
        main: main_action(state, update),
        secondary: secondary(state),
        status: status(state, reach),
        steps: steps(state),
        notes,
        differences: differences(state.content_diff.as_ref()),
        players: players(state),
        updates: match update {
            UpdateState::Off(_) if update == &none => Updates {
                badge: None,
                copy: "This launcher does not update itself.".into(),
                installable: false,
            },
            _ => updates(update),
        },
    }
}

#[cfg(test)]
mod tests {
    use tpf3mp_agent::launcher::{Game, InstalledGame, Room};

    use super::*;

    fn member(name: &str, you: bool, owner: bool, ready: bool) -> Member {
        Member {
            id: format!("p-{name}"),
            name: name.into(),
            platform: "Windows x86-64".into(),
            owner,
            you,
            ready,
            connected: true,
            content: MemberContent::Same,
            differs: None,
            banner: None,
            loading: None,
        }
    }

    fn in_room(members: Vec<Member>, you_own: bool) -> State {
        State {
            name: "Ann".into(),
            server: Some("tpf3mp.example.org:29470".into()),
            server_fixed: true,
            server_name: Some("EU".into()),
            connection: Connection::Connected,
            installed: Some(InstalledGame {
                dir: r"C:\Games\Transport Fever 3".into(),
                build: "20364158".into(),
            }),
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

    #[test]
    fn speeds_read_as_players_say_them() {
        assert_eq!(speed_text(0), "paused");
        assert_eq!(speed_text(100), "1×");
        assert_eq!(speed_text(200), "2×");
        assert_eq!(speed_text(150), "1.5×");
        assert_eq!(size_text(48_000_000), "48.0 MB");
        assert_eq!(size_text(1_500), "2 kB");
    }

    #[test]
    fn not_connected_the_button_connects() {
        let state = State {
            server: Some("tpf3mp.example.org:29470".into()),
            server_fixed: true,
            server_name: Some("EU".into()),
            ..State::default()
        };
        let view = present(&state, Reach::Online, None);
        assert_eq!(view.main.label, "Connect");
        assert_eq!(view.main.does, Does::Submit(Form::Connect));
        assert_eq!(view.pill, ("Not connected", Pill::Unknown));
        assert_eq!(view.rows[0].value, "EU · online");
        assert!(view.secondary.is_empty());
        let offline = present(&state, Reach::Offline, None);
        assert_eq!(
            offline.status,
            Some((
                "The server EU does not answer right now.".into(),
                Tone::Error
            ))
        );
    }

    #[test]
    fn before_a_room_the_game_can_start_for_its_menus_window() {
        let installed = Some(InstalledGame {
            dir: r"C:\Games\Transport Fever 3".into(),
            build: "20364158".into(),
        });
        let start = |state: &State| {
            present(state, Reach::Online, None)
                .secondary
                .iter()
                .any(|button| {
                    button.label == "Start Transport Fever 3"
                        && button.then == Then::Act(Action::LaunchGame)
                })
        };
        let mut state = State {
            installed: installed.clone(),
            ..State::default()
        };
        assert!(start(&state), "not connected yet: the game's menu connects");
        state.connection = Connection::Connected;
        let labels: Vec<_> = present(&state, Reach::Online, None)
            .secondary
            .iter()
            .map(|b| b.label)
            .collect();
        assert_eq!(
            labels,
            [
                "Join with an invite",
                "Start Transport Fever 3",
                "Disconnect"
            ]
        );
        state.game.attached = Some("40408".into());
        assert!(!start(&state), "running already");
        state.game.attached = None;
        state.installed = None;
        assert!(!start(&state), "no game to start");
    }

    #[test]
    fn in_a_room_the_game_starts_first_then_everyone_gets_ready() {
        let state = in_room(
            vec![
                member("Ann", true, true, false),
                member("Bob", false, false, true),
            ],
            true,
        );
        let view = present(&state, Reach::Online, None);
        assert_eq!(view.main.label, "Start Transport Fever 3");
        assert_eq!(view.main.does, Does::Act(Action::LaunchGame));
        let labels: Vec<_> = view.secondary.iter().map(|b| b.label).collect();
        // The ready button says where Ann stands.
        assert_eq!(labels, ["Not ready", "Copy invite", "Leave room"]);
        assert_eq!(
            view.rows[1],
            Row {
                label: "Room",
                value: "Friday trains".into(),
                large: true
            }
        );
        assert_eq!(view.rows[2].value, "2 of 4");
        assert_eq!(view.players[0].name, "Ann (you)");
        assert_eq!(
            view.players[0].badges,
            [("Owner", Pill::Ready), ("Not ready", Pill::Unknown)]
        );
        assert!(view.players[1].removable);

        // The game is attached, everyone ready: the owner starts.
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
        let view = present(&state, Reach::Online, None);
        assert_eq!(view.main.does, Does::Act(Action::Start));
        // Ann is ready, and her button says so.
        assert_eq!(view.secondary[0].label, "Ready");
    }

    #[test]
    fn in_the_lobby_readiness_follows_the_save_loading_and_ready_stays_a_button() {
        let mut state = in_room(
            vec![
                member("Ann", true, true, false),
                member("Bob", false, false, true),
            ],
            true,
        );
        state.game = Game {
            attached: Some("40391".into()),
            ..Game::default()
        };
        let view = present(&state, Reach::Online, None);
        let (text, _) = view.status.unwrap();
        assert!(
            text.contains("Load your save in the game") && text.contains("marked ready"),
            "{text}"
        );
        // Pressing it by hand still works.
        assert_eq!(view.main.does, Does::Act(Action::Ready { ready: true }));

        state.room.as_mut().unwrap().members[0].ready = true;
        let view = present(&state, Reach::Online, None);
        let (text, _) = view.status.unwrap();
        assert!(text.contains("You are ready"), "{text}");
        assert_eq!(view.secondary[0].label, "Ready");
        assert_eq!(
            view.secondary[0].then,
            Then::Act(Action::Ready { ready: false })
        );
    }

    #[test]
    fn playing_says_the_speed() {
        let mut state = in_room(vec![member("Ann", true, true, true)], true);
        state.room.as_mut().unwrap().phase = Phase::Running;
        state.game = Game {
            attached: Some("40391".into()),
            world: World::Playing,
            step: Some(18432),
            speed: 200,
            ..Game::default()
        };
        let view = present(&state, Reach::Online, None);
        assert_eq!(view.main.label, "Playing · 2×");
        assert!(!view.main.enabled());
        assert_eq!(
            view.status,
            Some(("Step 18432, at 2×.".into(), Tone::Ready))
        );
        assert_eq!(view.pill, ("Playing", Pill::Ready));
    }

    #[test]
    fn with_the_lobby_in_the_game_the_window_starts_the_game_and_shows_where_things_stand() {
        // Nothing yet: the game starts from here, and its menu does the rest.
        let state = State {
            server: Some("tpf3mp.example.org:29470".into()),
            server_fixed: true,
            server_name: Some("EU".into()),
            installed: Some(InstalledGame {
                dir: r"C:\Games\Transport Fever 3".into(),
                build: "20364158".into(),
            }),
            ..State::default()
        };
        let view = present_in_game(&state, Reach::Online, None);
        assert_eq!(view.main.label, "Start Transport Fever 3");
        assert_eq!(view.main.does, Does::Act(Action::LaunchGame));
        assert!(view.secondary.is_empty(), "no forms, no lobby buttons");
        let (text, _) = view.status.clone().unwrap();
        assert!(
            text.contains("click Multiplayer on its main menu"),
            "{text}"
        );
        assert_eq!(view.steps[0], ("Start Transport Fever 3 from here", false));

        // The game runs: it says where to click.
        let mut state = state;
        state.game.attached = Some("40408".into());
        let view = present_in_game(&state, Reach::Online, None);
        assert_eq!(view.main.label, "Continue in the game");
        assert!(!view.main.enabled());
        assert!(view.status.unwrap().0.contains("Click Multiplayer"));

        // In a room: shown, not played from here.
        let mut state = in_room(
            vec![
                member("Ann", true, true, false),
                member("Bob", false, false, true),
            ],
            true,
        );
        state.game.attached = Some("40408".into());
        let view = present_in_game(&state, Reach::Online, None);
        assert!(!view.main.enabled(), "Ready and Start are in the game");
        assert!(view.players.iter().all(|player| !player.removable));
        assert!(view.status.unwrap().0.contains("game's Multiplayer window"));
        assert_eq!(
            view.steps.iter().filter(|(_, done)| *done).count(),
            3,
            "started, connected, in a room"
        );

        // The room's world on its way: as the lobby says it.
        state.room.as_mut().unwrap().phase = Phase::Running;
        state.game.world = World::Fetching;
        state.game.bytes = 50;
        state.game.total = 100;
        let view = present_in_game(&state, Reach::Online, None);
        assert_eq!(view.main.label, "Receiving the world 50%");
        assert_eq!(view.main.progress, Some(0.5));

        // An error is said, whatever else.
        state.error = Some("the room is full".into());
        let view = present_in_game(&state, Reach::Online, None);
        assert_eq!(view.status, Some(("the room is full".into(), Tone::Error)));
    }

    #[test]
    fn an_outdated_launcher_offers_its_update() {
        let state = State {
            outdated: true,
            ..State::default()
        };
        let ready = UpdateState::Ready {
            version: "0.2.0".into(),
        };
        let view = present(&state, Reach::Online, Some(&ready));
        assert_eq!(view.main.label, "Restart and update");
        assert_eq!(view.main.does, Does::InstallUpdate);
        assert_eq!(
            view.updates.badge.as_deref(),
            Some("Launcher update · v0.2.0")
        );
        let view = present(&state, Reach::Online, Some(&UpdateState::Checking));
        assert_eq!(view.main.label, "Update TPF3-MP to play here");
    }
}
