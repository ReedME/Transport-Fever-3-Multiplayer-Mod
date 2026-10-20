//! The window itself: the mod's `gui/menu/lobby.lua`, drawn in every state
//! against a stand-in for the game's main menu (`tests/lua/fake_menu.lua`),
//! its buttons clicked, and every action it sends parsed as the hook parses
//! it ([`parse_action`]).

use mlua::{Function, Lua, Table};
use tpf3mp_bridge::{
    LobbyAction, LobbyConnection, LobbyLine, LobbyListing, LobbyMember, LobbyPublicRoom, LobbyRoom,
    LobbyRoomList, LobbyRules, LobbyStart, LobbyUpload, LobbyView, LobbyWorld,
};
use tpf3mp_proto::{BoundedVec, FixedBytes, PlayerId, Text};

use super::{LobbyState, parse_action};

const FAKE_MENU: &str = include_str!("../../tests/lua/fake_menu.lua");
const WINDOW: &str = include_str!("../../../../mod/tpf3mp_1/content/gui/menu/lobby.lua");
const ROOM_MODS: &str = include_str!("../../../../mod/tpf3mp_1/content/gui/menu/roommods.lua");

#[test]
fn new_world_setup_selects_multiplayer_once_and_preserves_other_settings() {
    let lua = menu();
    lua.load(
        r#"
        CONFIG = { mainMenuState = { activeModsState = { "other_mod" }, seed = "kept" } }
        api.type.AppConfig = { new = function(value) return value end }
        api.util = {
            getAppConfig = function() return CONFIG end,
            setAppConfig = function(value, restart) assert(not restart); CONFIG = value end,
        }
    "#,
    )
    .exec()
    .unwrap();
    let lobby: Table = lua.load(WINDOW).eval().unwrap();
    let prepare: Function = lobby.get("prepareNewWorld").unwrap();
    prepare.call::<()>(()).unwrap();
    prepare.call::<()>(()).unwrap();
    lua.load(
        r#"
        assert(CONFIG.mainMenuState.seed == "kept")
        local mods = CONFIG.mainMenuState.activeModsState
        assert(#mods == 2 and mods[1] == "other_mod" and mods[2] == "tpf3mp_1")
        CONFIG.mainMenuState.activeModsState = nil
    "#,
    )
    .exec()
    .unwrap();
    prepare.call::<()>(()).unwrap();
    lua.load("assert(CONFIG.mainMenuState.activeModsState[1] == 'tpf3mp_1')")
        .exec()
        .unwrap();
}

fn menu() -> Lua {
    let lua = Lua::new();
    lua.globals().set("LOBBY_SOURCE", WINDOW).unwrap();
    lua.globals().set("ROOMMODS_SOURCE", ROOM_MODS).unwrap();
    lua.globals()
        .set(
            "BANNERS_SOURCE",
            include_str!("../../../../mod/tpf3mp_1/content/scripts/tpf3mp/banners.lua"),
        )
        .unwrap();
    lua.load(FAKE_MENU)
        .set_name("@fake_menu.lua")
        .exec()
        .unwrap();
    lua
}

fn show(lua: &Lua, view: Option<&LobbyView>) {
    let literal = LobbyState::of(view, true).to_lua();
    lua.globals().set("STATE", literal).unwrap();
}

fn call(lua: &Lua, name: &str, args: impl mlua::IntoLuaMulti) {
    lua.globals()
        .get::<Function>(name)
        .unwrap()
        .call::<()>(args)
        .unwrap_or_else(|error| panic!("{name}: {error}"));
}

/// Draws the window as the game does: once, then a poll of the hook and a
/// redraw.
fn open(lua: &Lua, focus: Option<&str>) {
    call(lua, "render", focus);
    call(lua, "tick", ());
}

fn texts(lua: &Lua) -> String {
    lua.globals()
        .get::<Function>("texts")
        .unwrap()
        .call(())
        .unwrap()
}

fn click(lua: &Lua, label: &str) {
    call(lua, "click", label);
}

fn enabled(lua: &Lua, label: &str) -> bool {
    lua.globals()
        .get::<Function>("enabled")
        .unwrap()
        .call(label)
        .unwrap()
}

fn has_button(lua: &Lua, label: &str) -> bool {
    lua.globals()
        .get::<Function>("find")
        .unwrap()
        .call::<Option<Table>>(label)
        .unwrap()
        .is_some()
}

/// What the window sent, as the hook parses it, but the room list it asks
/// for by itself; and forgets it.
fn sent(lua: &Lua) -> Vec<LobbyAction> {
    sent_all(lua)
        .into_iter()
        .filter(|action| !matches!(action, LobbyAction::ListRooms { .. }))
        .collect()
}

/// Everything the window sent, as the hook parses it; and forgets it.
fn sent_all(lua: &Lua) -> Vec<LobbyAction> {
    let list: Table = lua.globals().get("SENT").unwrap();
    let actions = list
        .sequence_values::<String>()
        .map(|json| {
            let json = json.unwrap();
            parse_action(&json).unwrap_or_else(|error| panic!("{json}: {error}"))
        })
        .collect();
    lua.globals()
        .set("SENT", lua.create_table().unwrap())
        .unwrap();
    actions
}

fn player(n: u8) -> PlayerId {
    PlayerId(FixedBytes([n; 32]))
}

fn member(n: u8, name: &str, owner: bool, you: bool, ready: bool) -> LobbyMember {
    LobbyMember {
        player: player(n),
        name: Text::new(name).unwrap(),
        ready,
        connected: true,
        owner,
        you,
        same_content: Some(true),
        differs: None,
        banner: None,
        loading: None,
    }
}

fn online() -> LobbyView {
    LobbyView {
        connection: LobbyConnection::Connected,
        server: Text::new("EU").unwrap(),
        name: Text::new("Ann").unwrap(),
        rules: BoundedVec::new(vec![
            LobbyRules {
                name: Text::new("native").unwrap(),
                description: Text::new("The game's own economy").unwrap(),
            },
            LobbyRules {
                name: Text::new("canonical").unwrap(),
                description: Text::new("The server settles the economy").unwrap(),
            },
        ])
        .unwrap(),
        saves: BoundedVec::new(vec![
            Text::new("newest").unwrap(),
            // The hook's own copies, never offered.
            Text::new("tpf3mp_room_41856").unwrap(),
            Text::new("tpf3mp_41856_21").unwrap(),
            Text::new("mptest").unwrap(),
        ])
        .unwrap(),
        start_save: Some(Text::new("mptest").unwrap()),
        ..LobbyView::default()
    }
}

fn in_room(members: Vec<LobbyMember>, you_own: bool) -> LobbyView {
    LobbyView {
        room: Some(LobbyRoom {
            name: Text::new("Friday trains").unwrap(),
            rules: Text::new("native").unwrap(),
            invite: Some(Text::new("K7QM2X").unwrap()),
            running: false,
            you_own,
            max_players: 4,
            has_password: true,
            members: BoundedVec::new(members).unwrap(),
            competitive: false,
            start: None,
            upload: None,
        }),
        chat: BoundedVec::new(vec![LobbyLine {
            from: Text::new("Bob").unwrap(),
            text: Text::new("I'll take the coal line").unwrap(),
            you: false,
        }])
        .unwrap(),
        ..online()
    }
}

#[test]
fn before_the_hook_answers_the_window_waits_and_can_be_closed() {
    let lua = menu();
    open(&lua, None);
    let shown = texts(&lua);
    assert!(shown.contains("Waiting for the hook"), "{shown}");
    assert!(shown.contains("The hook did not answer"), "{shown}");
    call(&lua, "page_back", ());
    assert_eq!(lua.globals().get::<u32>("CLOSED").unwrap(), 1);
}

#[test]
fn not_connected_it_connects_with_the_name_typed_to_the_launchers_server() {
    let lua = menu();
    show(&lua, Some(&LobbyView::default()));
    // The launcher's server, named as players see it (D12).
    let mut view = LobbyView {
        server: Text::new("EU").unwrap(),
        name: Text::new("Ann").unwrap(),
        ..LobbyView::default()
    };
    show(&lua, Some(&view));
    open(&lua, None);
    let shown = texts(&lua);
    assert!(shown.contains("Not connected"), "{shown}");
    assert!(
        shown.contains("Join a room") && shown.contains("Host a room"),
        "the first page's two choices: {shown}"
    );
    assert!(
        card_enabled(&lua, "Join a room"),
        "discovery explains how to connect"
    );
    call(&lua, "type_into", ("Ann", "Ada"));
    click(&lua, "Connect to EU");
    assert_eq!(
        sent(&lua),
        [LobbyAction::Connect {
            name: Text::new("Ada").unwrap()
        }]
    );
    // Under way until the launcher answers, and not sent twice.
    assert!(texts(&lua).contains("Connecting to EU..."));
    assert!(!enabled(&lua, "Connect to EU"));
    view.connection = LobbyConnection::Connecting;
    show(&lua, Some(&view));
    call(&lua, "tick", ());
    assert!(texts(&lua).contains("Connecting"));
    assert!(!enabled(&lua, "Connecting..."));
}

#[test]
fn a_game_without_its_launcher_says_so_and_offers_nothing() {
    let lua = menu();
    let literal = LobbyState::of(None, false).to_lua();
    lua.globals().set("STATE", literal).unwrap();
    open(&lua, None);
    let shown = texts(&lua);
    assert!(shown.contains("no link to the TPF3-MP launcher"), "{shown}");
    assert!(!enabled(&lua, "Connect to the TPF3-MP server"));
}

/// A second pick from the Host page that ends without a save (the player
/// goes back) keeps the save and mods picked before.
#[test]
fn a_second_pick_left_without_a_save_keeps_the_first() {
    let lua = menu();
    show(&lua, Some(&online()));
    open(&lua, None);
    call(&lua, "click_card", "Host a room");
    let pick = r#"
        assert(PAGE == "LoadGame")
        unmount()
        INSTALLED = { tpf3mp_1 = { name = "TPF3-MP", source = "StagingArea" } }
        app.loadGame({ saveGameName = "tpf3mp_room_4294967296" }, false, {
            mods = { { name = "tpf3mp_1" } },
            modParams = {},
            configDict = {},
            metadata = { date = 1950 },
        })
        assert(LOBBY.endPick() == true)
    "#;
    call(&lua, "click_card", "Click to choose the save and mods");
    lua.load(pick).exec().unwrap();
    call(&lua, "render", ());
    call(&lua, "tick", ());
    assert!(texts(&lua).contains("tpf3mp_room_4294967296"));
    // Again, and back without a save.
    call(&lua, "click_card", "tpf3mp_room_4294967296");
    lua.load("assert(PAGE == 'LoadGame'); unmount(); assert(LOBBY.endPick() == true)")
        .exec()
        .unwrap();
    call(&lua, "render", ());
    call(&lua, "tick", ());
    let shown = texts(&lua);
    assert!(
        shown.contains("tpf3mp_room_4294967296") && shown.contains("Create room"),
        "{shown}"
    );
}

/// A Load Game page this mod does not know (a game patch) is left as it
/// is: nothing is picked there, and the page says so.
#[test]
fn a_load_game_page_this_mod_does_not_know_is_left_alone_and_said() {
    let lua = menu();
    show(&lua, Some(&online()));
    open(&lua, None);
    call(&lua, "click_card", "Host a room");
    lua.load("savegame_react_util.SavegameCard = nil")
        .exec()
        .unwrap();
    call(&lua, "click_card", "Click to choose the save and mods");
    lua.load("assert(PAGE == nil, tostring(PAGE))")
        .exec()
        .unwrap();
    assert!(
        texts(&lua).contains("The save can't be picked on this game's Load Game page"),
        "{}",
        texts(&lua)
    );
}

/// Hosting, the save is picked as in the room, on the game's Load Game
/// page, by any name the game gives it; its mods follow once the room is
/// made.
#[test]
fn hosting_picks_the_save_and_its_mods_on_the_games_load_game_page() {
    let lua = menu();
    show(&lua, Some(&online()));
    open(&lua, None);
    call(&lua, "click_card", "Host a room");
    call(&lua, "type_into", ("Ann's room", "Weekend"));
    call(&lua, "click_card", "Click to choose the save and mods");
    // The game drops the main page, and the window with it, while its Load
    // Game page shows; the window is made anew when the main page is back.
    lua.load(
        r#"
        assert(PAGE == "LoadGame")
        unmount()
        INSTALLED = { tpf3mp_1 = { name = "TPF3-MP", source = "StagingArea" } }
        app.loadGame({ saveGameName = "tpf3mp_room_4294967296" }, false, {
            mods = { { name = "tpf3mp_1" } },
            modParams = {},
            configDict = {},
            metadata = { date = 1950 },
        })
        assert(LOBBY.endPick() == true)
    "#,
    )
    .exec()
    .unwrap();
    call(&lua, "render", ());
    call(&lua, "tick", ());
    // Back on the Host page as it was, with the save picked.
    let shown = texts(&lua);
    assert!(
        shown.contains("tpf3mp_room_4294967296") && shown.contains("Create room"),
        "{shown}"
    );
    click(&lua, "Create room");
    let actions: [LobbyAction; 1] = sent(&lua).try_into().unwrap();
    let [
        LobbyAction::Create {
            start_save, room, ..
        },
    ] = actions
    else {
        panic!("not a create")
    };
    assert_eq!(room.as_str(), "Weekend", "the name typed before the pick");
    assert_eq!(
        start_save.as_ref().map(Text::as_str),
        Some("tpf3mp_room_4294967296")
    );
    // Made: the mods the page held are the room's.
    show(
        &lua,
        Some(&starting_from(
            true,
            Some(start("tpf3mp_room_4294967296", "", 0, true)),
            None,
        )),
    );
    call(&lua, "tick", ());
    let actions = sent(&lua);
    assert!(
        matches!(&actions[..], [LobbyAction::ChooseRoomMods { save: None, mods, .. }] if mods.len() == 1),
        "{actions:?}"
    );
    call(&lua, "tick", ());
    assert!(
        !sent(&lua)
            .iter()
            .any(|a| matches!(a, LobbyAction::ChooseRoomMods { .. })),
        "once"
    );
}

#[test]
fn a_room_is_created_with_the_rules_players_and_save_picked() {
    let lua = menu();
    show(&lua, Some(&online()));
    open(&lua, None);
    call(&lua, "click_card", "Host a room");
    assert!(texts(&lua).contains("Private: invite only"));
    // The launcher's own first choice picked.
    assert!(texts(&lua).contains("mptest"));
    call(&lua, "type_into", ("Ann's room", "Alps"));
    call(&lua, "choose", ("Players", 6));
    call(&lua, "choose", ("Rules", "canonical"));
    assert!(texts(&lua).contains("The server settles the economy"));
    click(&lua, "Create room");
    assert_eq!(
        sent(&lua),
        [LobbyAction::Create {
            room: Text::new("Alps").unwrap(),
            max_players: 6,
            password: None,
            rules: Some(Text::new("canonical").unwrap()),
            start_save: Some(Text::new("mptest").unwrap()),
            listing: None,
            competitive: false,
        }]
    );
    assert!(texts(&lua).contains("Creating the room..."));
}

#[test]
fn a_room_can_start_without_a_save_and_is_named_for_its_owner() {
    let lua = menu();
    show(
        &lua,
        Some(&LobbyView {
            start_save: None,
            ..online()
        }),
    );
    open(&lua, None);
    call(&lua, "click_card", "Host a room");
    let shown = texts(&lua);
    assert!(
        shown.contains("New world"),
        "new worlds are the default without a chosen save: {shown}"
    );
    assert!(shown.contains("Choose your map and settings on the next screen"));
    click(&lua, "Create room");
    let actions: [LobbyAction; 1] = sent(&lua).try_into().unwrap();
    let [
        LobbyAction::Create {
            room, start_save, ..
        },
    ] = actions
    else {
        panic!("not a create")
    };
    assert_eq!(room.as_str(), "Ann's room");
    assert_eq!(start_save.as_ref().map(Text::as_str), Some(""), "none");
}

#[test]
fn a_room_is_joined_with_a_code_from_its_popup() {
    let lua = menu();
    show(&lua, Some(&online()));
    open(&lua, Some("join"));
    let shown = texts(&lua);
    assert!(
        shown.contains("Public rooms on EU") && !shown.contains("Invite code"),
        "the Join page shows only the public rooms: {shown}"
    );
    click(&lua, "Join with code");
    let shown = texts(&lua);
    assert!(
        shown.contains("Invite code") && !shown.contains("Public rooms on EU"),
        "the popup over the list: {shown}"
    );
    // Cancel closes it.
    click(&lua, "Cancel");
    assert!(texts(&lua).contains("Public rooms on EU"));
    click(&lua, "Join with code");
    // Nothing typed: said, and nothing sent.
    click(&lua, "Join");
    assert!(sent(&lua).is_empty());
    assert!(texts(&lua).contains("Type the invite code"));
    click(&lua, "Join with code");
    call(&lua, "type_into", ("K7QM2X", " k7qm2x "));
    // The field without a placeholder: the room's password.
    call(&lua, "type_into", ("", "pw"));
    click(&lua, "Join");
    assert_eq!(
        sent(&lua),
        [LobbyAction::Join {
            invite: Text::new("K7QM2X").unwrap(),
            server: None,
            password: Some(Text::new("pw").unwrap()),
        }]
    );
}

#[test]
fn what_the_hook_refuses_is_shown_until_the_next_action() {
    let lua = menu();
    show(&lua, Some(&online()));
    open(&lua, None);
    call(&lua, "click_card", "Host a room");
    lua.globals()
        .set("REPLY", "error: that room name is too long")
        .unwrap();
    click(&lua, "Create room");
    let shown = texts(&lua);
    assert!(shown.contains("that room name is too long"), "{shown}");
    assert!(!shown.contains("Creating the room..."));
    // And the launcher's own errors, as it sends them.
    lua.globals().set("REPLY", "ok").unwrap();
    show(
        &lua,
        Some(&LobbyView {
            error: Some(Text::new("no room has that invite").unwrap()),
            ..online()
        }),
    );
    call(&lua, "page_back", ());
    click(&lua, "Disconnect");
    call(&lua, "tick", ());
    assert!(texts(&lua).contains("no room has that invite"));
}

#[test]
fn in_the_room_the_owner_starts_once_everyone_is_ready() {
    let lua = menu();
    show(
        &lua,
        Some(&in_room(
            vec![
                member(1, "Ann", true, true, true),
                member(2, "Bob", false, false, false),
            ],
            true,
        )),
    );
    open(&lua, None);
    let shown = texts(&lua);
    for word in [
        "Friday trains",
        "K7QM2X",
        "Players  ·  2 of 4  ·  1 ready",
        "Owner",
        "You",
        "Not ready",
        "I'll take the coal line",
    ] {
        assert!(shown.contains(word), "{word}: {shown}");
    }
    assert!(!enabled(&lua, "Start the game"), "Bob is not ready");
    // The owner's button says Ready, and a click takes it back.
    click(&lua, "Ready");
    assert_eq!(sent(&lua), [LobbyAction::Ready { ready: false }]);
    show(
        &lua,
        Some(&in_room(
            vec![
                member(1, "Ann", true, true, true),
                member(2, "Bob", false, false, true),
            ],
            true,
        )),
    );
    call(&lua, "tick", ());
    click(&lua, "Start the game");
    assert_eq!(sent(&lua), [LobbyAction::Start]);
}

/// A room in its lobby, Ann owning it if `you_own` (else Bob), both ready,
/// starting from `start` while `upload` goes up.
fn starting_from(
    you_own: bool,
    start: Option<LobbyStart>,
    upload: Option<(&str, u8)>,
) -> LobbyView {
    let mut view = in_room(
        vec![
            member(1, "Ann", you_own, you_own, true),
            member(2, "Bob", !you_own, !you_own, true),
        ],
        you_own,
    );
    let room = view.room.as_mut().unwrap();
    room.start = start;
    room.upload = upload.map(|(save, percent)| LobbyUpload {
        save: Text::new(save).unwrap(),
        percent,
    });
    view
}

fn start(name: &str, map: &str, year: u16, arrived: bool) -> LobbyStart {
    LobbyStart {
        name: Text::new(name).unwrap(),
        map: Text::new(map).unwrap(),
        year,
        arrived,
    }
}

/// The values the choice under `caption` offers and the one chosen; no
/// choice there, `None`.
fn offered(lua: &Lua, caption: &str) -> (Vec<String>, Option<String>) {
    lua.globals()
        .get::<Function>("offered")
        .unwrap()
        .call(caption)
        .unwrap()
}

#[test]
fn the_owner_picks_the_rooms_save_and_mods_on_the_games_load_game_page() {
    let lua = menu();
    show(
        &lua,
        Some(&starting_from(
            true,
            Some(start("mptest", "", 0, true)),
            None,
        )),
    );
    open(&lua, None);
    assert_eq!(
        offered(&lua, "Start from this save").1,
        None,
        "no list of saves"
    );
    assert!(enabled(&lua, "Start the game"), "the room has its save");
    call(&lua, "click_card", "Click to change the save and mods");
    // On the game's own page, which says what it does here.
    lua.load(
        r#"
        assert(PAGE == "LoadGame", tostring(PAGE))
        local page = LOAD_PAGE()
        assert(page.title == "The room's save and mods", page.title)
        assert(page.button.text == "Use for the room", page.button.text)
        assert(not page.button.classes:find("load-savegame-sound", 1, true), page.button.classes)
        assert(page.button.classes:find("loadSavegameButton", 1, true), page.button.classes)
        assert(page.card.params.save == true and page.card.params.onClickMain == "details")
        INSTALLED = {
            signals = { name = "Auto Signals", source = "mod.io", hub = "6414521" },
            tpf3mp_1 = { name = "TPF3-MP", source = "StagingArea" },
        }
        app.setWaitForStartReadyGame()
        app.loadGame({ saveGameName = "newest" }, false, {
            mods = { { name = "signals" }, { name = "tpf3mp_1" } },
            modParams = { [""] = { ["economy.industryDevelopment.closureProbability"] = 2 }, signals = { distance = 5 } },
            configDict = { { "climate", "::/climates/dry/dry.clima" } },
            metadata = { date = 1925 },
        })
        assert(LOADS == nil and WAITS == nil, "nothing loads")
        assert(PAGE == "Main", tostring(PAGE))
        -- The page is the game's again, as is its load.
        local page = LOAD_PAGE()
        assert(page.title == "Load Game" and page.button.text == "Load Game")
        assert(page.button.classes:find("load-savegame-sound", 1, true))
        assert(page.card.params.save == nil)
        assert(LOBBY.endPick() == true, "main_page.tl opens the window again")
        assert(LOBBY.endPick() == false, "once")
    "#,
    )
    .exec()
    .unwrap();
    call(&lua, "tick", ());
    let setting = |id: &str, key: &str, value: i64| tpf3mp_bridge::LobbySetting {
        id: Text::new(id).unwrap(),
        key: Text::new(key).unwrap(),
        value,
    };
    assert_eq!(
        sent(&lua),
        [LobbyAction::ChooseRoomMods {
            save: Some(Text::new("newest").unwrap()),
            map: Text::new("dry").unwrap(),
            year: 1925,
            mods: BoundedVec::new(vec![
                tpf3mp_bridge::LobbySelected {
                    id: Text::new("signals").unwrap(),
                    name: Text::new("Auto Signals").unwrap(),
                    source: Text::new("mod.io").unwrap(),
                    modio: Some(6414521),
                },
                tpf3mp_bridge::LobbySelected {
                    id: Text::new("tpf3mp_1").unwrap(),
                    name: Text::new("TPF3-MP").unwrap(),
                    source: Text::new("StagingArea").unwrap(),
                    modio: None,
                },
            ])
            .unwrap(),
            params: BoundedVec::new(vec![
                setting("", "economy.industryDevelopment.closureProbability", 2),
                setting("signals", "distance", 5),
            ])
            .unwrap(),
        }]
    );
    assert!(texts(&lua).contains("Taking the save and mods for the room..."));
    call(&lua, "tick", ());
    assert_eq!(sent(&lua), [], "sent once");
}

#[test]
fn a_save_loaded_straight_from_the_pages_list_keeps_its_own_mods() {
    let lua = menu();
    show(&lua, Some(&starting_from(true, None, None)));
    open(&lua, None);
    call(&lua, "click_card", "Click to change the save and mods");
    lua.load(
        r#"
        app.loadGame({ saveGameName = "newest.sav" }, false, nil)
        assert(LOBBY.endPick() == true)
    "#,
    )
    .exec()
    .unwrap();
    call(&lua, "tick", ());
    assert_eq!(
        sent(&lua),
        [LobbyAction::ChooseStart {
            save: Text::new("newest").unwrap(),
            map: Text::new("").unwrap(),
            year: 0,
        }]
    );
}

/// The room may start from a new world instead of a save: the page's list
/// of saves starts with a tile for it.
#[test]
fn a_new_world_is_picked_from_the_tile_before_the_saves() {
    let lua = menu();
    show(
        &lua,
        Some(&starting_from(
            true,
            Some(start("mptest", "", 0, true)),
            None,
        )),
    );
    open(&lua, None);
    assert!(!has_button(&lua, "New world"), "picked on the page now");
    call(&lua, "click_card", "Click to change the save and mods");
    lua.load(
        r#"
        local tiles = LOAD_PAGE().tiles
        assert(#tiles == 2, "the new world's tile and the save's")
        assert(tiles[1].tile.title == "New world", tostring(tiles[1].tile and tiles[1].tile.title))
        tiles[1].tile.onClickMain()
        assert(PAGE == "Main" and LOADS == nil)
        assert(LOBBY.endPick() == true)
        assert(#LOAD_PAGE().tiles == 1, "the page is the game's again")
    "#,
    )
    .exec()
    .unwrap();
    call(&lua, "tick", ());
    assert!(
        matches!(sent(&lua).as_slice(), [LobbyAction::ChooseStart { save, .. }] if save.as_str().is_empty()),
        "a new world"
    );
}

#[test]
fn leaving_the_load_game_page_puts_it_back_and_picks_nothing() {
    let lua = menu();
    show(&lua, Some(&starting_from(true, None, None)));
    open(&lua, None);
    call(&lua, "click_card", "Click to change the save and mods");
    lua.load(
        r#"
        assert(LOBBY.endPick() == true, "Back to the main menu: the window again")
        local page = LOAD_PAGE()
        assert(page.title == "Load Game" and page.button.text == "Load Game")
        app.loadGame({ saveGameName = "newest" }, false, nil)
        assert(LOADS == 1, "the game's own load again")
        assert(LOBBY.endPick() == false)
    "#,
    )
    .exec()
    .unwrap();
    call(&lua, "tick", ());
    assert_eq!(sent(&lua), []);
}

#[test]
fn a_guest_picks_neither_save_nor_mods() {
    let lua = menu();
    show(
        &lua,
        Some(&starting_from(
            false,
            Some(start("Güterzug", "dry", 1900, true)),
            None,
        )),
    );
    open(&lua, None);
    assert!(!texts(&lua).contains("Click to change the save and mods"));
    assert!(!has_button(&lua, "New world"));
}

/// A room whose mods this guest (Bob) partly lacks: Auto Signals from Mod
/// Hub, Trees from somewhere else, and TPF3-MP's own, which Bob has.
fn lacking_mods() -> LobbyView {
    let mut view = starting_from(false, Some(start("Güterzug", "dry", 1900, true)), None);
    let room_mod = |id: &str, name: &str, have, source: &str, modio: Option<u64>| {
        tpf3mp_bridge::LobbyRoomMod {
            id: Text::new(id).unwrap(),
            name: Text::lossy(name),
            version: Text::new("3+m77").unwrap(),
            yours: None,
            have,
            source: Text::lossy(source),
            modio,
        }
    };
    view.room_mods = BoundedVec::new(vec![
        room_mod(
            "signals",
            "Auto Signals",
            tpf3mp_bridge::LobbyHave::No,
            "mod.io",
            Some(6414521),
        ),
        room_mod(
            "trees",
            "Trees",
            tpf3mp_bridge::LobbyHave::No,
            "UserMods",
            None,
        ),
        room_mod(
            "tpf3mp_1",
            "TPF3-MP",
            tpf3mp_bridge::LobbyHave::Yes,
            "StagingArea",
            None,
        ),
    ])
    .unwrap();
    view.room_mods_missing = 2;
    view
}

/// Refused by a running room for its mods, a player sees the room's mods
/// outside it, and installs the missing ones before joining again.
#[test]
fn a_player_refused_for_its_mods_installs_them_before_joining_again() {
    let lua = menu();
    let mut view = lacking_mods();
    view.room = None;
    show(&lua, Some(&view));
    open(&lua, Some("join"));
    click(&lua, "Mods (2 missing)");
    let shown = texts(&lua);
    assert!(
        shown.contains("Auto Signals") && shown.contains("Missing"),
        "{shown}"
    );
    assert!(has_button(&lua, "Install"), "{shown}");
}

#[test]
fn a_guest_sees_which_of_the_rooms_mods_it_lacks() {
    let lua = menu();
    show(&lua, Some(&lacking_mods()));
    open(&lua, None);
    let shown = texts(&lua);
    assert!(shown.contains("3 mods  ·  2 missing"), "{shown}");
    call(&lua, "tab", "The room's mods (3) · 2 missing");
    let shown = texts(&lua);
    for word in [
        "Auto Signals",
        "Trees",
        "Missing",
        "Mod Hub",
        "UserMods",
        "Installed",
    ] {
        assert!(shown.contains(word), "{word}: {shown}");
    }
}

/// A tile's Details button is the game's own, once, however often the
/// window draws.
#[test]
fn a_tiles_details_button_shows_once_however_often_it_is_drawn() {
    let lua = menu();
    show(&lua, Some(&lacking_mods()));
    open(&lua, None);
    call(&lua, "tab", "The room's mods (3) · 2 missing");
    for _ in 0..5 {
        call(&lua, "tick", ());
    }
    let details: u32 = lua.load("return count_buttons('Details')").eval().unwrap();
    assert_eq!(details, 1, "the one tile with a button has one Details");
}

#[test]
fn a_guest_installs_the_rooms_missing_mods_from_mod_hub() {
    let lua = menu();
    lua.load(
        r#"HUB.mods["6414521"] = { title = "Auto Signals", author = "tearded", installSize = 10 }"#,
    )
    .exec()
    .unwrap();
    show(&lua, Some(&lacking_mods()));
    open(&lua, None);
    call(&lua, "tab", "The room's mods (3) · 2 missing");
    // Looked up first, then asked: nothing is subscribed yet.
    click(&lua, "Install all missing (1)");
    let shown = texts(&lua);
    // Asked in place of the tiles, as Mod Hub names it: title and author.
    assert!(
        shown.contains("Subscribe to Auto Signals on Mod Hub?")
            && shown.contains("by tearded")
            && !shown.contains("Install all missing"),
        "{shown}"
    );
    lua.load("assert(#HUB.subscribed == 0)").exec().unwrap();
    // No: back to the tiles, nothing subscribed.
    click(&lua, "Cancel");
    assert!(!texts(&lua).contains("Subscribe to"));
    lua.load("assert(#HUB.subscribed == 0)").exec().unwrap();
    click(&lua, "Install all missing (1)");
    click(&lua, "Subscribe & install");
    lua.load(r#"assert(HUB.subscribed[1] == "6414521" and #HUB.subscribed == 1)"#)
        .exec()
        .unwrap();
    assert!(has_button(&lua, "Installing..."));
    call(&lua, "tick", ());
    assert_eq!(sent(&lua), [], "still downloading");
    // Installed, under the room's id: the launcher looks again.
    lua.load(
        r#"
        HUB.state["6414521"] = "Installed"
        INSTALLED.signals = { name = "Auto Signals", source = "mod.io", hub = "6414521" }
    "#,
    )
    .exec()
    .unwrap();
    call(&lua, "tick", ());
    assert_eq!(sent(&lua), [LobbyAction::RescanMods]);
    // Until the launcher finds it, the room does not count it: said so, and
    // it can look again.
    assert!(texts(&lua).contains("Installed, not found yet"));
    click(&lua, "Look again");
    assert_eq!(sent(&lua), [LobbyAction::RescanMods]);
    // Found: the room's row says so.
    let mut found = lacking_mods();
    let mut rows = found.room_mods.to_vec();
    rows[0].have = tpf3mp_bridge::LobbyHave::Yes;
    found.room_mods = BoundedVec::new(rows).unwrap();
    found.room_mods_missing = 1;
    show(&lua, Some(&found));
    call(&lua, "tick", ());
    let shown = texts(&lua);
    assert!(
        shown.contains("Installed") && !shown.contains("not found yet"),
        "{shown}"
    );
}

/// Mod Hub answers for several mods before the window draws again: each
/// answer counts, and the player is asked about all of them at once.
#[test]
fn every_mod_hub_answer_counts_however_many_come_at_once() {
    let lua = menu();
    lua.load(
        r#"
        HUB.mods["6414521"] = { title = "Auto Signals", author = "tearded", installSize = 2097152 }
        HUB.mods["6415791"] = { title = "Signal Distance", author = "tearded" }
    "#,
    )
    .exec()
    .unwrap();
    let mut view = lacking_mods();
    let mut rows = view.room_mods.to_vec();
    rows[1].modio = Some(6415791);
    rows[1].source = Text::lossy("mod.io");
    view.room_mods = BoundedVec::new(rows).unwrap();
    show(&lua, Some(&view));
    open(&lua, None);
    call(&lua, "tab", "The room's mods (3) · 2 missing");
    click(&lua, "Install all missing (2)");
    let shown = texts(&lua);
    assert!(
        shown.contains("Subscribe to these 2 on Mod Hub?")
            && shown.contains("Auto Signals")
            && shown.contains("2.0 MB")
            && shown.contains("The room calls it Trees"),
        "{shown}"
    );
    click(&lua, "Subscribe & install");
    lua.load(r#"assert(#HUB.subscribed == 2, #HUB.subscribed)"#)
        .exec()
        .unwrap();
}

/// Mod Hub answers later, as the game's does, and aborts a request whose
/// handle was collected: the lobby keeps each handle until the answer comes,
/// so the mods looked up do not wait at Looking up... for ever.
#[test]
fn mod_hub_answers_after_the_window_drew_again() {
    let lua = menu();
    lua.load(
        r#"
        HUB.later = true
        HUB.mods["6414521"] = { title = "Auto Signals", author = "tearded" }
    "#,
    )
    .exec()
    .unwrap();
    show(&lua, Some(&lacking_mods()));
    open(&lua, None);
    call(&lua, "tab", "The room's mods (3) · 2 missing");
    click(&lua, "Install all missing (1)");
    assert!(has_button(&lua, "Looking up..."));
    call(&lua, "tick", ());
    let answered: u32 = lua.load("return HUB.answer()").eval().unwrap();
    assert_eq!(answered, 1, "the lookup's handle was dropped");
    call(&lua, "tick", ());
    assert!(texts(&lua).contains("Subscribe to Auto Signals on Mod Hub?"));
    click(&lua, "Subscribe & install");
    let answered: u32 = lua.load("return HUB.answer()").eval().unwrap();
    assert_eq!(answered, 1, "the subscription's handle was dropped");
    lua.load(r#"assert(HUB.subscribed[1] == "6414521")"#)
        .exec()
        .unwrap();
    call(&lua, "tick", ());
    assert!(has_button(&lua, "Installing..."));
}

/// Mod Hub that never answers: the lookup fails after a while, the mod can
/// be installed again, and an answer that comes after that changes nothing.
#[test]
fn a_lookup_mod_hub_does_not_answer_fails() {
    let lua = menu();
    lua.load(
        r#"
        HUB.later = true
        HUB.mods["6414521"] = { title = "Auto Signals", author = "tearded" }
    "#,
    )
    .exec()
    .unwrap();
    show(&lua, Some(&lacking_mods()));
    open(&lua, None);
    call(&lua, "tab", "The room's mods (3) · 2 missing");
    click(&lua, "Install all missing (1)");
    for _ in 0..70 {
        call(&lua, "tick", ());
    }
    assert!(has_button(&lua, "Looking up..."), "waits a while");
    for _ in 0..10 {
        call(&lua, "tick", ());
    }
    let shown = texts(&lua);
    assert!(
        !has_button(&lua, "Looking up...") && shown.contains("Install failed"),
        "{shown}"
    );
    assert!(enabled(&lua, "Install all missing (1)"));
    // Mod Hub answers late: the player is not asked out of the blue.
    let answered: u32 = lua.load("return HUB.answer()").eval().unwrap();
    assert_eq!(answered, 1);
    call(&lua, "tick", ());
    assert!(!texts(&lua).contains("Subscribe to"));
}

/// A subscription Mod Hub never answers, and never takes: it fails after a
/// while, and the mod can be installed again.
#[test]
fn a_subscription_mod_hub_does_not_answer_fails() {
    let lua = menu();
    lua.load(r#"HUB.mods["6414521"] = { title = "Auto Signals", author = "tearded" }"#)
        .exec()
        .unwrap();
    show(&lua, Some(&lacking_mods()));
    open(&lua, None);
    call(&lua, "tab", "The room's mods (3) · 2 missing");
    click(&lua, "Install all missing (1)");
    lua.load("HUB.later = true; HUB.silent = true")
        .exec()
        .unwrap();
    click(&lua, "Subscribe & install");
    lua.load("HUB.answer()").exec().unwrap();
    assert!(has_button(&lua, "Installing..."));
    for _ in 0..80 {
        call(&lua, "tick", ());
    }
    let shown = texts(&lua);
    assert!(
        !has_button(&lua, "Installing...") && shown.contains("Install failed"),
        "{shown}"
    );
    assert!(enabled(&lua, "Install all missing (1)"));
}

/// A tile's Install shows the mod on the game's own Mod Hub page, where the
/// player sees what it is and subscribes; an install begun there is
/// followed once the page is closed.
#[test]
fn a_tiles_install_shows_the_mod_on_the_games_mod_hub_page() {
    let lua = menu();
    show(&lua, Some(&lacking_mods()));
    open(&lua, None);
    call(&lua, "tab", "The room's mods (3) · 2 missing");
    click(&lua, "Install");
    lua.load(
        r#"
        local page = assert(WINDOWS.ModDetailsWindow, "the game's Mod Hub page")
        assert(page.title == "Auto Signals", page.title)
        local p = page.modManagerParams
        assert(p.modId.value == "6414521" and p.context.backendId == HUB.backend)
        assert(p.context.wc and p.onClose == page.onClose and MODAL)
        assert(#HUB.subscribed == 0, "nothing subscribed by the lobby")
    "#,
    )
    .exec()
    .unwrap();
    // Closed without subscribing: nothing to follow.
    lua.load(
        "WINDOWS.ModDetailsWindow.onClose(); assert(WINDOWS.ModDetailsWindow == nil and not MODAL)",
    )
    .exec()
    .unwrap();
    call(&lua, "tick", ());
    assert!(has_button(&lua, "Install"));
    // Subscribed on the page: followed until installed, then found again.
    click(&lua, "Install");
    lua.load(
        r#"
        HUB.subscribed[1] = "6414521"
        HUB.state["6414521"] = "Downloading"
        WINDOWS.ModDetailsWindow.onClose()
    "#,
    )
    .exec()
    .unwrap();
    call(&lua, "tick", ());
    call(&lua, "tick", ());
    assert!(has_button(&lua, "Installing..."), "{}", texts(&lua));
    lua.load(
        r#"
        HUB.state["6414521"] = "Installed"
        INSTALLED.signals = { name = "Auto Signals", source = "mod.io", hub = "6414521" }
    "#,
    )
    .exec()
    .unwrap();
    call(&lua, "tick", ());
    assert_eq!(sent(&lua), [LobbyAction::RescanMods]);
}

/// Without the game's window container, the lobby asks itself.
#[test]
fn without_the_games_mod_hub_page_the_lobby_asks_itself() {
    let lua = menu();
    lua.load(r#"NO_WINDOWS = true; HUB.mods["6414521"] = { title = "Auto Signals" }"#)
        .exec()
        .unwrap();
    show(&lua, Some(&lacking_mods()));
    open(&lua, None);
    call(&lua, "tab", "The room's mods (3) · 2 missing");
    click(&lua, "Install");
    assert!(texts(&lua).contains("Subscribe to Auto Signals on Mod Hub?"));
}

#[test]
fn a_mod_hub_mod_installed_under_another_id_is_not_the_rooms() {
    let lua = menu();
    lua.load(
        r#"
        HUB.mods["6414521"] = { title = "Something else" }
        HUB.state["6414521"] = "Installed"
        INSTALLED.other_mod = { name = "Other", source = "mod.io", hub = "6414521" }
    "#,
    )
    .exec()
    .unwrap();
    show(&lua, Some(&lacking_mods()));
    open(&lua, None);
    call(&lua, "tab", "The room's mods (3) · 2 missing");
    click(&lua, "Install all missing (1)");
    click(&lua, "Subscribe & install");
    call(&lua, "tick", ());
    assert_eq!(sent(&lua), [], "nothing to find again");
    let why: String = lua
        .load("local s = '' ; for _i, n in ipairs(LOG) do s = s .. n end ; return s")
        .eval()
        .unwrap();
    assert!(has_button(&lua, "Install"), "offered again: {why}");
    assert!(texts(&lua).contains("Install failed"));
}

#[test]
fn signed_out_of_mod_hub_the_guest_is_sent_to_sign_in() {
    let lua = menu();
    lua.load("HUB.signedIn = false").exec().unwrap();
    show(&lua, Some(&lacking_mods()));
    open(&lua, None);
    call(&lua, "tab", "The room's mods (3) · 2 missing");
    assert!(texts(&lua).contains("Sign in to Mod Hub to install them"));
    assert!(!enabled(&lua, "Install"));
    click(&lua, "Mod Hub");
    assert_eq!(lua.globals().get::<u32>("MODHUB").unwrap(), 1);
}

#[test]
fn a_players_card_says_how_their_mods_differ() {
    let lua = menu();
    let mut bob = member(2, "Bob", false, false, false);
    bob.same_content = Some(false);
    bob.differs = Some(tpf3mp_proto::ContentStatus {
        missing: 2,
        changed: 1,
        extra: 0,
        game: false,
        reordered: false,
        unlisted: false,
    });
    show(
        &lua,
        Some(&in_room(
            vec![member(1, "Ann", true, true, true), bob.clone()],
            true,
        )),
    );
    open(&lua, None);
    let shown = texts(&lua);
    assert!(shown.contains("2 mods missing, 1 other version"), "{shown}");
    // Ready, but with other mods: the server would refuse the start.
    let mut view = in_room(vec![member(1, "Ann", true, true, true), bob], true);
    view.room.as_mut().unwrap().members = BoundedVec::new(
        view.room
            .as_ref()
            .unwrap()
            .members
            .iter()
            .cloned()
            .map(|mut m| {
                m.ready = true;
                m
            })
            .collect(),
    )
    .unwrap();
    show(&lua, Some(&view));
    call(&lua, "tick", ());
    assert!(
        !enabled(&lua, "Start the game"),
        "said before the server would refuse"
    );
}

#[test]
fn a_private_rooms_save_is_described_once_the_game_read_it() {
    let lua = menu();
    lua.load(r#"LOBBY.saveDetails = function(name) return { map = "tropical", year = 1960 } end"#)
        .exec()
        .unwrap();
    show(
        &lua,
        Some(&starting_from(
            true,
            Some(start("mptest", "", 0, false)),
            None,
        )),
    );
    open(&lua, None);
    assert_eq!(
        sent(&lua),
        [LobbyAction::ChooseStart {
            save: Text::new("mptest").unwrap(),
            map: Text::new("tropical").unwrap(),
            year: 1960,
        }],
        "the room named it without them"
    );
    call(&lua, "tick", ());
    assert_eq!(sent(&lua), [], "once");
}

#[test]
fn start_waits_while_the_owners_save_goes_up() {
    let lua = menu();
    show(
        &lua,
        Some(&starting_from(
            true,
            Some(start("mptest", "", 0, true)),
            Some(("newest", 40)),
        )),
    );
    open(&lua, None);
    let shown = texts(&lua);
    assert!(shown.contains("Sending newest to the room: 40%"), "{shown}");
    assert!(
        !enabled(&lua, "Start the game"),
        "everyone is ready, but the save is on its way"
    );
    // Uploaded, but the room not told yet that it has it.
    show(
        &lua,
        Some(&starting_from(
            true,
            Some(start("newest", "dry", 1925, false)),
            None,
        )),
    );
    call(&lua, "tick", ());
    assert!(texts(&lua).contains("on its way to the room"));
    assert!(!enabled(&lua, "Start the game"));
    show(
        &lua,
        Some(&starting_from(
            true,
            Some(start("newest", "dry", 1925, true)),
            None,
        )),
    );
    call(&lua, "tick", ());
    assert!(enabled(&lua, "Start the game"));
    click(&lua, "Start the game");
    assert_eq!(sent(&lua), [LobbyAction::Start]);
}

#[test]
fn a_guest_sees_the_rooms_save_but_cannot_pick_it() {
    let lua = menu();
    show(
        &lua,
        Some(&starting_from(
            false,
            Some(start("Güterzug", "dry", 1900, true)),
            None,
        )),
    );
    open(&lua, None);
    let shown = texts(&lua);
    assert!(shown.contains("Güterzug · Dry · 1900"), "{shown}");
    assert!(
        !texts(&lua).contains("Click to change the save and mods"),
        "no picker"
    );
    // Without one handed over, the owner's game has the world.
    show(&lua, Some(&starting_from(false, None, None)));
    call(&lua, "tick", ());
    assert!(texts(&lua).contains("The world the owner's game has"));
}

#[test]
fn removing_a_player_and_leaving_ask_first() {
    let lua = menu();
    show(
        &lua,
        Some(&in_room(
            vec![
                member(1, "Ann", true, true, true),
                member(2, "Bob", false, false, true),
            ],
            true,
        )),
    );
    open(&lua, None);
    click(&lua, "Remove Bob from the room");
    assert!(sent(&lua).is_empty(), "asked first");
    click(&lua, "Keep");
    click(&lua, "Remove Bob from the room");
    click(&lua, "Remove");
    assert_eq!(sent(&lua), [LobbyAction::Kick { player: player(2) }]);
    click(&lua, "Leave room");
    assert!(sent(&lua).is_empty(), "asked first");
    assert!(texts(&lua).contains("Leave the room?"));
    click(&lua, "Stay");
    click(&lua, "Leave room");
    click(&lua, "Leave");
    assert_eq!(sent(&lua), [LobbyAction::Leave]);
}

#[test]
fn a_guest_gets_ready_and_chats_but_neither_starts_nor_removes() {
    let lua = menu();
    show(
        &lua,
        Some(&in_room(
            vec![
                member(1, "Ann", true, false, true),
                member(2, "Bob", false, true, false),
            ],
            false,
        )),
    );
    open(&lua, None);
    assert!(!has_button(&lua, "Start the game"));
    assert!(!has_button(&lua, "Remove Ann from the room"));
    // Bob's button says Not ready, and a click makes him ready.
    click(&lua, "Not ready");
    assert_eq!(sent(&lua), [LobbyAction::Ready { ready: true }]);
    call(&lua, "type_into", ("Say something to the room", "hi all"));
    assert_eq!(
        sent(&lua),
        [LobbyAction::Chat {
            text: Text::new("hi all").unwrap()
        }]
    );
}

#[test]
fn while_the_rooms_world_comes_the_window_says_how_far_and_stays_usable() {
    let lua = menu();
    let mut view = in_room(
        vec![
            member(1, "Ann", true, false, true),
            member(2, "Bob", false, true, true),
        ],
        false,
    );
    view.room.as_mut().unwrap().running = true;
    view.world = LobbyWorld::Fetching {
        bytes: 5_000_000,
        total: 20_000_000,
    };
    view.differences = Some(Text::new("you lack stations 3").unwrap());
    show(&lua, Some(&view));
    open(&lua, None);
    let shown = texts(&lua);
    assert!(
        shown.contains("Receiving the room's world: 25% (5.0 MB of 20.0 MB)"),
        "{shown}"
    );
    assert!(shown.contains("you lack stations 3"));
    assert!(shown.contains("The room's game is under way."));
    assert!(
        !has_button(&lua, "Ready")
            && !has_button(&lua, "Not ready")
            && !has_button(&lua, "Start the game")
    );
    // The chat and Leave still work.
    call(
        &lua,
        "type_into",
        ("Say something to the room", "almost there"),
    );
    assert_eq!(sent(&lua).len(), 1);
    assert!(enabled(&lua, "Leave room"));
    view.world = LobbyWorld::Loading;
    show(&lua, Some(&view));
    call(&lua, "tick", ());
    assert!(texts(&lua).contains("Loading the room's world..."));
    view.room = None;
    view.world = LobbyWorld::Playing;
    show(&lua, Some(&view));
    call(&lua, "tick", ());
    assert!(!texts(&lua).contains("Playing the room's game"));
}

/// Copy beside the room's invite code asks the hook to put the code on the
/// clipboard, and says "Copied" for a moment.
#[test]
fn the_invite_code_is_copied_with_a_click() {
    let lua = menu();
    show(
        &lua,
        Some(&in_room(vec![member(1, "Ann", true, true, true)], true)),
    );
    open(&lua, None);
    assert!(enabled(&lua, "Copy"));
    click(&lua, "Copy");
    let asked: Vec<String> = lua
        .load(
            "local out = {} for i, json in ipairs(SENT) do out[i] = json end SENT = {} return out",
        )
        .eval()
        .unwrap();
    assert_eq!(asked, [r#"{"action":"copy","text":"K7QM2X"}"#]);
    call(&lua, "render", ());
    assert!(has_button(&lua, "Copied") && !has_button(&lua, "Copy"));
    for _ in 0..5 {
        call(&lua, "tick", ());
    }
    assert!(has_button(&lua, "Copy"), "back after a moment");
    // A refusal shows as any other.
    lua.load("REPLY = 'error: the clipboard is busy'")
        .exec()
        .unwrap();
    click(&lua, "Copy");
    assert!(
        texts(&lua).contains("the clipboard is busy"),
        "{}",
        texts(&lua)
    );
    assert!(!has_button(&lua, "Copied"));
}

#[test]
fn the_cards_say_where_the_player_is() {
    let lua = menu();
    let line = |view: Option<&LobbyView>, linked: bool, what: &str| -> String {
        let literal = LobbyState::of(view, linked).to_lua();
        let state: Table = lua.load(format!("return {literal}")).eval().unwrap();
        let lobby: Table = lua.globals().get("LOBBY").unwrap();
        lobby.get::<Function>(what).unwrap().call(state).unwrap()
    };
    assert_eq!(
        line(None, false, "summary"),
        "Start the game from the TPF3-MP launcher"
    );
    assert_eq!(line(Some(&online()), true, "summary"), "Online on EU");
    let room = in_room(vec![member(1, "Ann", true, true, true)], true);
    assert_eq!(
        line(Some(&room), true, "summary"),
        "Friday trains · 1/4 players · 1 ready"
    );
    assert_eq!(
        line(Some(&room), true, "joinLine"),
        "Your room: invite K7QM2X"
    );
    assert_eq!(
        line(Some(&online()), true, "joinLine"),
        "With the invite code they send you"
    );
}

fn public_room(name: &str, map: &str, players: u8, password: bool) -> LobbyPublicRoom {
    LobbyPublicRoom {
        invite: Text::new(format!("INV{}", name.len())).unwrap(),
        name: Text::new(name).unwrap(),
        rules: Text::new("native").unwrap(),
        players,
        max_players: 4,
        has_password: password,
        running: false,
        map: Text::new(map).unwrap(),
        year: 1873,
        companies: 2,
        competitive: false,
        server: Text::new("").unwrap(),
        ping_ms: 0,
    }
}

fn browsing(rooms: Vec<LobbyPublicRoom>, page: u16, more: bool) -> LobbyView {
    LobbyView {
        rooms: Some(LobbyRoomList {
            page,
            rooms: BoundedVec::new(rooms).unwrap(),
            more,
            servers: BoundedVec::empty(),
        }),
        ..online()
    }
}

fn card_enabled(lua: &Lua, title: &str) -> bool {
    all_cards(lua).iter().any(|card| {
        card.get::<String>("text").unwrap().contains(title) && card.get::<bool>("enabled").unwrap()
    })
}

/// The room cards shown: the first page's Join and Host are cards too, and
/// are left out.
fn cards(lua: &Lua) -> Vec<Table> {
    all_cards(lua)
        .into_iter()
        .filter(|card| {
            let text: String = card.get("text").unwrap();
            !text.starts_with("Join a room") && !text.starts_with("Host a room")
        })
        .collect()
}

fn all_cards(lua: &Lua) -> Vec<Table> {
    let list: Table = lua
        .globals()
        .get::<Function>("room_cards")
        .unwrap()
        .call(())
        .unwrap();
    list.sequence_values::<Table>()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn the_window_asks_for_the_room_list_and_shows_each_room_as_a_card() {
    let lua = menu();
    show(&lua, Some(&online()));
    open(&lua, None);
    assert!(sent_all(&lua).is_empty(), "the first page asks for no list");
    call(&lua, "click_card", "Join a room");
    call(&lua, "tick", ());
    assert_eq!(
        sent_all(&lua),
        [LobbyAction::ListRooms { page: 0 }],
        "asked for at once"
    );
    assert!(texts(&lua).contains("Asking the server for its rooms"));
    show(
        &lua,
        Some(&browsing(
            vec![
                public_room("Dry run", "dry", 2, false),
                public_room("Snowy", "subarctic", 1, true),
                public_room("Somewhere", "", 3, false),
                public_room("Fourth", "temperate", 1, false),
            ],
            0,
            true,
        )),
    );
    call(&lua, "tick", ());
    let shown = cards(&lua);
    assert_eq!(shown.len(), 4);
    let first = &shown[0];
    let text: String = first.get("text").unwrap();
    assert!(text.contains("Dry run"), "{text}");
    assert!(text.contains("2/4 players · Co-op"), "{text}");
    let detail: String = first.get("tooltip").unwrap();
    assert!(detail.contains("2 companies · 1873"), "{detail}");
    assert!(detail.contains("Dry"), "the climate's own name: {detail}");
    assert_eq!(
        first.get::<String>("picture").unwrap(),
        "::/climates/dry/icon.tga",
        "the climate's own picture"
    );
    assert_eq!(
        shown[1].get::<String>("picture").unwrap(),
        "::/gui/menu/images/subarctic_ingame.tga",
        "the menu's picture of it"
    );
    assert_eq!(
        shown[2].get::<String>("picture").unwrap(),
        "::/gui/menu/images/m05_ingame.tga",
        "a map it does not know"
    );
    // A room without a password joins with a click.
    first
        .get::<Function>("click")
        .unwrap()
        .call::<()>(())
        .unwrap();
    call(&lua, "render", ());
    assert_eq!(
        sent(&lua),
        [LobbyAction::Join {
            invite: Text::new("INV7").unwrap(),
            server: None,
            password: None,
        }]
    );
}

#[test]
fn a_room_with_a_password_asks_for_it_and_pages_move_on() {
    let lua = menu();
    let mut snowy = public_room("Snowy", "subarctic", 1, true);
    snowy.server = Text::new("US").unwrap();
    show(&lua, Some(&browsing(vec![snowy], 1, false)));
    open(&lua, Some("join"));
    sent_all(&lua);
    assert!(enabled(&lua, "Previous") && !enabled(&lua, "Next"));
    click(&lua, "Previous");
    assert_eq!(sent_all(&lua), [LobbyAction::ListRooms { page: 0 }]);
    cards(&lua)[0]
        .get::<Function>("click")
        .unwrap()
        .call::<()>(())
        .unwrap();
    call(&lua, "render", ());
    assert!(sent(&lua).is_empty(), "the password first");
    assert!(texts(&lua).contains("Snowy has a password"));
    call(&lua, "type_into", ("Password", "pw"));
    assert_eq!(
        sent(&lua),
        [LobbyAction::Join {
            invite: Text::new("INV5").unwrap(),
            server: Some(Text::new("US").unwrap()),
            password: Some(Text::new("pw").unwrap()),
        }]
    );
}

#[test]
fn a_public_room_is_listed_with_its_saves_climate_and_year() {
    let lua = menu();
    let saves: Table = lua.globals().get("SAVES").unwrap();
    let save = lua.create_table().unwrap();
    save.set("climate", "::/climates/dry/dry.clima").unwrap();
    save.set("year", 1900).unwrap();
    saves.set("mptest", save).unwrap();
    show(&lua, Some(&online()));
    open(&lua, None);
    call(&lua, "click_card", "Host a room");
    call(&lua, "choose", ("Who can find it", "public"));
    assert!(
        texts(&lua).contains("Listed for everyone on EU: Dry, 1900."),
        "{}",
        texts(&lua)
    );
    click(&lua, "Create room");
    let actions: [LobbyAction; 1] = sent(&lua).try_into().unwrap();
    let [LobbyAction::Create { listing, .. }] = actions else {
        panic!("not a create")
    };
    assert_eq!(
        listing,
        Some(LobbyListing {
            map: Text::new("dry").unwrap(),
            year: 1900,
        })
    );
    assert_eq!(lua.globals().get::<u32>("READS").unwrap(), 1, "read once");
}

/// The window itself is a wrapper recipe in the mod's `main_page.tl`
/// (Teal, which the stand-in cannot run): its widget's meta may hold its
/// class only. The game asserted and closed on a styleSheet there
/// ("Wrapper recipe must return child", 2026-09-30).
#[test]
fn the_menus_wrapper_recipes_pass_meta_for_their_class_only() {
    let page = include_str!("../../../../mod/tpf3mp_1/content/gui/menu/main_page.tl");
    let mut checked = 0;
    for block in page.split("RegisterWrapperRecipe(").skip(1) {
        let block = &block[..block.find("\nend)").expect("the recipe ends")];
        for meta in block.split("meta = {").skip(1) {
            let inside = &meta[..meta.find('}').expect("the meta table closes")];
            let keys: Vec<&str> = inside
                .split(',')
                .filter_map(|entry| entry.split_once('=').map(|(key, _)| key.trim()))
                .collect();
            assert!(
                keys.iter().all(|key| *key == "class"),
                "a wrapper recipe's meta holds {keys:?}"
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "the Multiplayer window's meta was checked");
}

/// The launcher's log session shows on the first page with its Copy,
/// and nothing of it while diagnostics are off.
#[test]
fn the_log_session_is_shown_and_copied() {
    let lua = menu();
    let mut view = online();
    view.log_session = Text::new("AB2CD3").unwrap();
    show(&lua, Some(&view));
    open(&lua, None);
    let shown = texts(&lua);
    assert!(shown.contains("AB2CD3"), "{shown}");
    click(&lua, "Copy");
    let asked: Vec<String> = lua
        .load(
            "local out = {} for i, json in ipairs(SENT) do out[i] = json end SENT = {} return out",
        )
        .eval()
        .unwrap();
    assert_eq!(asked, [r#"{"action":"copy","text":"AB2CD3"}"#]);
    view.log_session = Text::new("").unwrap();
    show(&lua, Some(&view));
    call(&lua, "tick", ());
    assert!(!texts(&lua).contains("Log session"), "{}", texts(&lua));
}

#[test]
fn the_first_page_leads_to_join_or_host_and_back() {
    let lua = menu();
    show(&lua, Some(&online()));
    open(&lua, None);
    let shown = texts(&lua);
    assert!(shown.contains("Host a room"), "{shown}");
    assert!(!shown.contains("Room name") && !shown.contains("Public rooms on EU"));
    call(&lua, "click_card", "Host a room");
    assert!(texts(&lua).contains("Room name"));
    call(&lua, "page_back", ());
    assert!(!texts(&lua).contains("Room name"));
    call(&lua, "click_card", "Join a room");
    assert!(texts(&lua).contains("Public rooms on EU"));
    call(&lua, "page_back", ());
    assert!(texts(&lua).contains("Host a room"));
    // In a room, the room's page, whatever was picked.
    show(
        &lua,
        Some(&in_room(vec![member(1, "Ann", true, true, false)], true)),
    );
    call(&lua, "tick", ());
    assert!(texts(&lua).contains("Your room"));
}

#[test]
fn your_mods_are_chosen_from_join_host_and_the_room() {
    let lua = menu();
    let mods = |view: LobbyView| LobbyView {
        mods: BoundedVec::new(vec![
            tpf3mp_bridge::LobbyMod {
                id: Text::new("schbrongx_minimap").unwrap(),
                name: Text::new("Minimap").unwrap(),
                class: tpf3mp_bridge::LobbyModClass::Personal,
                reason: Text::new("only what this player sees").unwrap(),
                chosen: false,
                choosable: true,
            },
            tpf3mp_bridge::LobbyMod {
                id: Text::new("vehicles_pack").unwrap(),
                name: Text::new("Vehicles").unwrap(),
                class: tpf3mp_bridge::LobbyModClass::Shared,
                reason: Text::new("every player needs it: it adds vehicles").unwrap(),
                chosen: true,
                choosable: false,
            },
        ])
        .unwrap(),
        room_mods: BoundedVec::new(vec![tpf3mp_bridge::LobbyRoomMod {
            id: Text::new("trees_pack").unwrap(),
            version: Text::new("2").unwrap(),
            have: tpf3mp_bridge::LobbyHave::No,
            name: Text::lossy("Pack"),
            yours: None,
            source: Text::lossy("StagingArea"),
            modio: None,
        }])
        .unwrap(),
        room_mods_more: 3,
        room_mods_missing: 0,
        room_mods_other: 0,
        room_params: tpf3mp_proto::BoundedVec::empty(),
        ..view
    };
    show(&lua, Some(&mods(online())));
    open(&lua, Some("join"));
    click(&lua, "Your mods (0 chosen)");
    let shown = texts(&lua);
    for word in ["Minimap", "only you see it"] {
        assert!(shown.contains(word), "{word}: {shown}");
    }
    assert!(
        !shown.contains("Vehicles"),
        "a shared mod is the room's, not one of your own: {shown}"
    );
    click(&lua, "Activate");
    assert_eq!(
        sent(&lua),
        [LobbyAction::ChooseMod {
            id: Text::new("schbrongx_minimap").unwrap(),
            chosen: true,
        }]
    );
    call(&lua, "page_back", ());
    assert!(texts(&lua).contains("Public rooms on EU"));
    // In the room's lobby too, on its own tab, beside the room's mods; but
    // not once its game runs.
    let mut room = mods(in_room(vec![member(1, "Ann", true, true, true)], true));
    show(&lua, Some(&room));
    call(&lua, "tick", ());
    call(&lua, "tab", "The room's mods (4)");
    let shown = texts(&lua);
    for word in ["Pack", "Missing", "and 3 more"] {
        assert!(shown.contains(word), "{word}: {shown}");
    }
    call(&lua, "tab", "Only for you (1)");
    click(&lua, "Activate");
    assert_eq!(sent(&lua).len(), 1);
    room.room.as_mut().unwrap().running = true;
    show(&lua, Some(&room));
    call(&lua, "tick", ());
    click(&lua, "Activate");
    assert_eq!(sent(&lua), [], "the game runs: its mods hold");
}

#[test]
fn the_server_is_shown_changed_and_put_back_from_the_first_page() {
    let lua = menu();
    let on = |address: &str| LobbyView {
        server_address: Text::new(address).unwrap(),
        server_default: Text::new("relay.example:29470").unwrap(),
        ..online()
    };
    show(&lua, Some(&on("relay.example:29470")));
    open(&lua, None);
    click(&lua, "Server...");
    let shown = texts(&lua);
    assert!(shown.contains("EU (default)"), "{shown}");
    assert!(shown.contains("Invites only join rooms on your own server."));
    assert!(
        !has_button(&lua, "Reset to default"),
        "already on the default"
    );
    // The launcher refuses an address it cannot use: said under the field.
    lua.globals()
        .set(
            "REPLY",
            "error: the server must be host:port, such as play.example:29470",
        )
        .unwrap();
    call(&lua, "type_into", ("relay.example:29470", "nonsense"));
    assert!(texts(&lua).contains("the server must be host:port"));
    lua.globals().set("REPLY", "ok").unwrap();
    call(
        &lua,
        "type_into",
        ("relay.example:29470", "lan.example:29470"),
    );
    assert!(
        !enabled(&lua, "Use this server"),
        "not again while it changes"
    );
    let asked = sent(&lua);
    assert_eq!(asked.len(), 2, "{asked:?}");
    assert_eq!(
        asked[1],
        LobbyAction::SetServer {
            server: Text::new("lan.example:29470").unwrap()
        }
    );
    // On another server: Reset puts the default back.
    show(&lua, Some(&on("lan.example:29470")));
    call(&lua, "tick", ());
    let shown = texts(&lua);
    assert!(
        shown.contains("another server") && !shown.contains("lan.example"),
        "the address only in the field: {shown}"
    );
    click(&lua, "Reset to default");
    assert_eq!(
        sent(&lua),
        [LobbyAction::SetServer {
            server: Text::new("").unwrap()
        }]
    );
    // Not from a room: the room's page is shown, and no server page.
    show(
        &lua,
        Some(&LobbyView {
            ..in_room(vec![member(1, "Ann", true, true, true)], true)
        }),
    );
    call(&lua, "tick", ());
    assert!(!has_button(&lua, "Use this server"));
}

#[test]
fn no_server_address_shows_in_the_window() {
    let lua = menu();
    let mut view = LobbyView {
        server: Text::new("127.0.0.1:29470").unwrap(),
        error: Some(Text::new("cannot reach 127.0.0.1:29470: timed out").unwrap()),
        ..online()
    };
    show(&lua, Some(&view));
    open(&lua, None);
    let shown = texts(&lua);
    assert!(!shown.contains("127.0.0.1"), "{shown}");
    assert!(shown.contains("another server"), "{shown}");
    assert!(
        shown.contains("cannot reach the server: timed out"),
        "{shown}"
    );
    // An invite with the server before its code shows the code alone.
    view = in_room(vec![member(1, "Ann", true, true, true)], true);
    view.room.as_mut().unwrap().invite = Some(Text::new("play.example:29470 K7QM2X").unwrap());
    show(&lua, Some(&view));
    call(&lua, "tick", ());
    let shown = texts(&lua);
    assert!(
        shown.contains("K7QM2X") && !shown.contains("play.example"),
        "{shown}"
    );
}

#[test]
fn the_windows_banners_are_the_servers_in_its_order() {
    let lua = menu();
    let lobby: Table = lua.globals().get("LOBBY").unwrap();
    let banners: Table = lobby.get("BANNERS").unwrap();
    let ids: Vec<String> = banners
        .sequence_values::<Table>()
        .map(|banner| banner.unwrap().get::<String>(1).unwrap())
        .collect();
    assert_eq!(
        ids,
        tpf3mp_proto::BANNERS,
        "the default is the same in every game"
    );
}

#[test]
fn the_players_show_as_cards_of_their_banners_or_their_default() {
    let lua = menu();
    let mut ann = member(1, "Ann", true, true, true);
    ann.banner = Some(Text::new("dry").unwrap());
    let bob = member(0x2a, "Bob", false, false, false);
    show(&lua, Some(&in_room(vec![ann, bob], true)));
    open(&lua, None);
    let cards = all_cards(&lua);
    let find = |name: &str| {
        cards
            .iter()
            .find(|card| card.get::<String>("text").unwrap().starts_with(name))
            .unwrap_or_else(|| panic!("no card for {name}"))
            .clone()
    };
    let ann_card = find("Ann");
    assert_eq!(
        ann_card.get::<String>("picture").unwrap(),
        "::/gui/menu/images/dry_ingame.tga",
        "her pick"
    );
    let text: String = ann_card.get("text").unwrap();
    assert!(
        text.contains("Owner") && text.contains("Ready") && text.contains("You"),
        "{text}"
    );
    // Bob's default, from his key: 0x2a2a2a2a modulo the set.
    let n = 0x2a2a_2a2a_usize % tpf3mp_proto::BANNERS.len();
    let bob_card = find("Bob");
    let expected: String = lua
        .load(format!(
            "return LOBBY.bannerPicture(\"{}\")",
            tpf3mp_proto::BANNERS[n]
        ))
        .eval()
        .unwrap();
    assert_eq!(bob_card.get::<String>("picture").unwrap(), expected);
    assert!(
        bob_card
            .get::<String>("text")
            .unwrap()
            .contains("Not ready")
    );
}

/// While the room's world comes in, each player's row says how far their
/// game is: its download, then its load, then in the game.
#[test]
fn each_players_row_shows_their_loading_progress() {
    use tpf3mp_proto::LoadingStage;
    fn text_of(view: &LobbyView, name: &str) -> String {
        let lua = menu();
        show(&lua, Some(view));
        open(&lua, None);
        all_cards(&lua)
            .iter()
            .map(|card| card.get::<String>("text").unwrap())
            .find(|text| text.starts_with(name))
            .unwrap_or_else(|| panic!("no card for {name}"))
    }
    let mut ann = member(1, "Ann", true, true, true);
    ann.loading = Some(LoadingStage::Loading);
    let mut bob = member(2, "Bob", false, false, true);
    bob.loading = Some(LoadingStage::Fetching { percent: 42 });
    let cat = member(3, "Cat", false, false, true);
    let mut view = in_room(vec![ann, bob, cat], true);
    if let Some(room) = view.room.as_mut() {
        room.running = true;
    }
    let ann = text_of(&view, "Ann");
    assert!(ann.contains("Loading..."), "{ann}");
    let bob = text_of(&view, "Bob");
    assert!(bob.contains("Downloading 42%"), "{bob}");
    let cat = text_of(&view, "Cat");
    assert!(cat.contains("Playing") && !cat.contains("Ready"), "{cat}");
    // Before the room starts, a player still loading shows that, not ready.
    let mut dan = member(4, "Dan", false, false, true);
    dan.loading = Some(LoadingStage::Fetching { percent: 7 });
    let view = in_room(vec![member(1, "Ann", true, true, true), dan], true);
    let dan = text_of(&view, "Dan");
    assert!(
        dan.contains("Downloading 7%") && !dan.contains("Ready"),
        "{dan}"
    );
}

#[test]
fn a_banner_is_picked_from_the_first_page() {
    let lua = menu();
    show(&lua, Some(&online()));
    open(&lua, None);
    click(&lua, "Your banner");
    let cards = all_cards(&lua);
    assert_eq!(cards.len(), tpf3mp_proto::BANNERS.len());
    assert!(!enabled(&lua, "Default"), "already the default");
    cards[3]
        .get::<Function>("click")
        .unwrap()
        .call::<()>(())
        .unwrap();
    assert_eq!(
        sent(&lua),
        [LobbyAction::SetBanner {
            banner: Some(Text::new(tpf3mp_proto::BANNERS[3]).unwrap())
        }]
    );
    show(
        &lua,
        Some(&LobbyView {
            banner: Some(Text::new(tpf3mp_proto::BANNERS[3]).unwrap()),
            ..online()
        }),
    );
    call(&lua, "tick", ());
    assert!(texts(&lua).contains("Yours"));
    click(&lua, "Default");
    assert_eq!(sent(&lua), [LobbyAction::SetBanner { banner: None }]);
}

fn images(lua: &Lua) -> Vec<String> {
    let list: Table = lua
        .globals()
        .get::<Function>("images")
        .unwrap()
        .call(())
        .unwrap();
    list.sequence_values::<String>()
        .map(Result::unwrap)
        .collect()
}

/// The campaign's characters this game has are picked like banners, by
/// their names; the launcher sends only those it has.
#[test]
fn a_campaign_portrait_is_picked_beside_the_banners() {
    let lua = menu();
    let portraits = |ids: &[&str]| {
        BoundedVec::new(ids.iter().map(|id| Text::new(*id).unwrap()).collect()).unwrap()
    };
    show(
        &lua,
        Some(&LobbyView {
            portraits: portraits(&["andrew", "dr_karl_brandt", "richard_o_sullivan"]),
            ..online()
        }),
    );
    open(&lua, None);
    click(&lua, "Your banner");
    let cards = all_cards(&lua);
    assert_eq!(cards.len(), tpf3mp_proto::BANNERS.len() + 3);
    let shown = texts(&lua);
    assert!(shown.contains("Characters"), "{shown}");
    let karl = cards
        .iter()
        .find(|card| {
            card.get::<String>("text")
                .unwrap()
                .starts_with("Dr. Karl Brandt")
        })
        .expect("a card named for the character");
    assert_eq!(
        karl.get::<String>("picture").unwrap(),
        "tpf3mp_1::/gui/tpf3mp/portraits/dr_karl_brandt.tga"
    );
    assert!(shown.contains("Richard O'Sullivan"), "{shown}");
    karl.get::<Function>("click")
        .unwrap()
        .call::<()>(())
        .unwrap();
    assert_eq!(
        sent(&lua),
        [LobbyAction::SetBanner {
            banner: Some(Text::new("dr_karl_brandt").unwrap())
        }]
    );
    show(
        &lua,
        Some(&LobbyView {
            banner: Some(Text::new("dr_karl_brandt").unwrap()),
            portraits: portraits(&["andrew", "dr_karl_brandt"]),
            ..online()
        }),
    );
    call(&lua, "tick", ());
    let karl: String = all_cards(&lua)
        .iter()
        .map(|card| card.get::<String>("text").unwrap())
        .find(|text| text.starts_with("Dr. Karl Brandt"))
        .unwrap();
    assert!(karl.contains("Yours"), "{karl}");
    assert!(enabled(&lua, "Default"));

    // Without the campaign, the picker offers the banners alone.
    show(&lua, Some(&online()));
    call(&lua, "tick", ());
    assert_eq!(all_cards(&lua).len(), tpf3mp_proto::BANNERS.len());
    assert!(!texts(&lua).contains("Characters"));
}

/// A member's portrait shows beside their card, which keeps their key's
/// banner; an id the window does not know shows the banner alone.
#[test]
fn a_players_portrait_shows_beside_their_name_in_the_room() {
    let lua = menu();
    let mut ann = member(1, "Ann", true, true, true);
    ann.banner = Some(Text::new("tom_mclaren").unwrap());
    let mut bob = member(0x2a, "Bob", false, false, false);
    bob.banner = Some(Text::new("selfie").unwrap());
    show(&lua, Some(&in_room(vec![ann, bob], true)));
    open(&lua, None);
    let pictures = images(&lua);
    let portrait = "tpf3mp_1::/gui/tpf3mp/portraits/tom_mclaren.tga";
    assert_eq!(
        pictures.iter().filter(|path| *path == portrait).count(),
        1,
        "{pictures:?}"
    );
    let card = |name: &str| {
        all_cards(&lua)
            .into_iter()
            .find(|card| card.get::<String>("text").unwrap().starts_with(name))
            .unwrap()
            .get::<String>("picture")
            .unwrap()
    };
    let key_banner = |n: usize| -> String {
        lua.load(format!(
            "return LOBBY.bannerPicture(\"{}\")",
            tpf3mp_proto::BANNERS[n % tpf3mp_proto::BANNERS.len()]
        ))
        .eval()
        .unwrap()
    };
    assert_eq!(card("Ann"), key_banner(0x0101_0101), "her key's banner");
    assert_eq!(
        card("Bob"),
        key_banner(0x2a2a_2a2a),
        "an unknown id: the default"
    );
    assert!(
        !pictures.iter().any(|path| path.contains("selfie")),
        "{pictures:?}"
    );
    let most: u32 = lua.load("return most_cards_in_a_row()").eval().unwrap();
    assert_eq!(most, 2, "portraits preserve our two-column player cards");
}

#[test]
fn the_host_picks_co_op_or_competitive_from_two_pictures() {
    let lua = menu();
    show(&lua, Some(&online()));
    open(&lua, None);
    call(&lua, "click_card", "Host a room");
    let styles: Vec<(String, String)> = all_cards(&lua)
        .iter()
        .map(|card| {
            (
                card.get::<String>("text").unwrap(),
                card.get::<String>("picture").unwrap(),
            )
        })
        .collect();
    assert!(
        styles.iter().any(|(text, picture)| text.starts_with(
            "Co-op
Picked"
        ) && picture == "::/gui/menu/images/campaign.tga"),
        "co-op is picked first: {styles:?}"
    );
    assert!(
        styles
            .iter()
            .any(|(text, picture)| text.starts_with("Competitive")
                && picture.ends_with("m03_loadscreen.tga"))
    );
    call(&lua, "click_card", "Competitive");
    assert!(
        texts(&lua).contains("Competitive\nPicked"),
        "{}",
        texts(&lua)
    );
    click(&lua, "Create room");
    let actions: [LobbyAction; 1] = sent(&lua).try_into().unwrap();
    let [LobbyAction::Create { competitive, .. }] = actions else {
        panic!("not a create")
    };
    assert!(competitive);
}

#[test]
fn a_rooms_play_style_shows_in_the_room_and_the_list() {
    let lua = menu();
    let mut view = in_room(vec![member(1, "Ann", true, true, true)], true);
    view.room.as_mut().unwrap().competitive = true;
    show(&lua, Some(&view));
    open(&lua, None);
    assert!(texts(&lua).contains("Competitive"));
    let mut listed = public_room("Race", "dry", 2, false);
    listed.competitive = true;
    show(&lua, Some(&browsing(vec![listed], 0, false)));
    call(&lua, "tick", ());
    call(&lua, "render", "join");
    let text: String = cards(&lua)[0].get("text").unwrap();
    assert!(text.contains("Competitive"), "{text}");
}

#[test]
fn rooms_from_several_servers_show_their_server_and_ping() {
    let lua = menu();
    let mut near = public_room("Near", "dry", 2, false);
    let duplicated_invite = near.invite.clone();
    near.server = Text::new("EU").unwrap();
    near.ping_ms = 24;
    let mut far = public_room("Far away", "temperate", 1, false);
    far.invite = duplicated_invite.clone();
    far.server = Text::new("US").unwrap();
    far.ping_ms = 110;
    let mut view = browsing(vec![near, far], 0, false);
    view.rooms.as_mut().unwrap().servers = BoundedVec::new(vec![
        tpf3mp_bridge::LobbyServer {
            name: Text::new("EU").unwrap(),
            ping_ms: 24,
            here: true,
            reachable: true,
        },
        tpf3mp_bridge::LobbyServer {
            name: Text::new("US").unwrap(),
            ping_ms: 110,
            here: false,
            reachable: true,
        },
    ])
    .unwrap();
    show(&lua, Some(&view));
    open(&lua, Some("join"));
    call(&lua, "tick", ());
    let shown = cards(&lua);
    let text: String = shown[1].get("text").unwrap();
    assert!(text.contains("US · 110 ms"), "{text}");
    let detail: String = shown[0].get("tooltip").unwrap();
    assert!(detail.contains("On EU · 24 ms"), "{detail}");
    let all = texts(&lua);
    assert!(
        all.contains("Public rooms on EU (24 ms) and US (110 ms)"),
        "{all}"
    );
    // Even with the same six-character code on EU, the card carries its own
    // listed server so clicking the US room cannot join the EU room.
    shown[1]
        .get::<Function>("click")
        .unwrap()
        .call::<()>(())
        .unwrap();
    call(&lua, "render", ());
    assert_eq!(
        sent(&lua),
        [LobbyAction::Join {
            invite: duplicated_invite,
            server: Some(Text::new("US").unwrap()),
            password: None,
        }]
    );
}

#[test]
fn friend_join_connects_with_typed_name_then_joins_exactly_once() {
    let lua = menu();
    let mut view = LobbyView {
        connection: LobbyConnection::Disconnected,
        ..online()
    };
    show(&lua, Some(&view));
    open(&lua, Some("friend"));
    assert!(texts(&lua).contains("Invite code"));
    assert!(!texts(&lua).contains("Public rooms on"));
    click(&lua, "Join room");
    assert!(sent_all(&lua).is_empty(), "empty code must not connect");
    call(&lua, "type_into", ("Ann", "Ada"));
    call(&lua, "type_into", ("K7QM2X", " k7qm2x "));
    call(&lua, "type_into", ("", "secret"));
    click(&lua, "Join room");
    assert_eq!(
        sent_all(&lua),
        [LobbyAction::Connect {
            name: Text::new("Ada").unwrap()
        }]
    );
    assert!(!enabled(&lua, "Join room"));
    view.connection = LobbyConnection::Connecting;
    show(&lua, Some(&view));
    call(&lua, "tick", ());
    assert!(sent_all(&lua).is_empty());
    view.connection = LobbyConnection::Connected;
    view.name = Text::new("Ada").unwrap();
    show(&lua, Some(&view));
    call(&lua, "tick", ());
    assert_eq!(
        sent_all(&lua),
        [LobbyAction::Join {
            invite: Text::new("K7QM2X").unwrap(),
            server: None,
            password: Some(Text::new("secret").unwrap())
        }]
    );
    call(&lua, "tick", ());
    assert!(
        sent_all(&lua).is_empty(),
        "no duplicate joins or unsolicited room listing"
    );
}

#[test]
fn friend_join_sends_the_selected_trusted_region() {
    let lua = menu();
    let mut view = online();
    view.servers = BoundedVec::new(vec![
        tpf3mp_bridge::LobbyServer {
            name: Text::new("EU").unwrap(),
            ping_ms: 24,
            here: true,
            reachable: true,
        },
        tpf3mp_bridge::LobbyServer {
            name: Text::new("US").unwrap(),
            ping_ms: 110,
            here: false,
            reachable: true,
        },
    ])
    .unwrap();
    show(&lua, Some(&view));
    open(&lua, Some("friend"));
    call(&lua, "choose", ("Region (optional)", "US"));
    call(&lua, "type_into", ("K7QM2X", "K7QM2X"));
    click(&lua, "Join room");
    assert_eq!(
        sent(&lua),
        [LobbyAction::Join {
            invite: Text::new("K7QM2X").unwrap(),
            server: Some(Text::new("US").unwrap()),
            password: None,
        }]
    );
}

#[test]
fn friend_connection_failure_cancels_the_join_and_allows_retry() {
    let lua = menu();
    let mut view = LobbyView {
        connection: LobbyConnection::Disconnected,
        ..online()
    };
    show(&lua, Some(&view));
    open(&lua, Some("friend"));
    call(&lua, "type_into", ("K7QM2X", "K7QM2X"));
    click(&lua, "Join room");
    assert_eq!(sent(&lua).len(), 1);
    view.error = Some(Text::new("The server is unavailable").unwrap());
    show(&lua, Some(&view));
    call(&lua, "tick", ());
    assert!(enabled(&lua, "Join room"));
    assert!(texts(&lua).contains("server is unavailable"));
    view.connection = LobbyConnection::Connected;
    view.error = None;
    show(&lua, Some(&view));
    call(&lua, "tick", ());
    assert!(
        sent_all(&lua).is_empty(),
        "a failed intention must not run later"
    );
    click(&lua, "Join room");
    assert!(matches!(sent(&lua).as_slice(), [LobbyAction::Join { .. }]));
}

#[test]
fn generating_a_world_waits_for_room_creation_before_opening_stock_setup() {
    let lua = menu();
    show(&lua, Some(&online()));
    open(&lua, None);
    call(&lua, "click_card", "Host a room");
    call(&lua, "click_card", "Click to choose the save and mods");
    lua.load("LOAD_PAGE().tiles[1].tile.onClickMain(); assert(LOBBY.endPick())")
        .exec()
        .unwrap();
    call(&lua, "tick", ());
    click(&lua, "Create room");
    assert!(
        lua.globals()
            .get::<Option<u32>>("GENERATED")
            .unwrap()
            .is_none()
    );
    assert!(
        matches!(sent(&lua).as_slice(), [LobbyAction::Create { start_save: Some(save), .. }] if save.as_str().is_empty())
    );
    let mut created = in_room(vec![member(1, "Ann", true, true, false)], true);
    created.start_save = None;
    show(&lua, Some(&created));
    call(&lua, "tick", ());
    call(&lua, "tick", ());
    assert_eq!(lua.globals().get::<u32>("GENERATED").unwrap(), 1);
    click(&lua, "Set up world");
    assert_eq!(
        lua.globals().get::<u32>("GENERATED").unwrap(),
        2,
        "cancelling stock setup must allow reopening it from the room"
    );
}

#[test]
fn unchanged_lobby_polls_leave_native_controls_open() {
    let lua = menu();
    show(&lua, Some(&online()));
    open(&lua, None);
    call(&lua, "tick", ());
    lua.globals().set("STATE_WRITES", 0).unwrap();
    for _ in 0..5 {
        call(&lua, "tick", ());
    }
    assert_eq!(
        lua.globals().get::<u32>("STATE_WRITES").unwrap(),
        0,
        "unchanged polling must not redraw and collapse native dropdowns"
    );
    let mut changed = online();
    changed.notice = Some(Text::new("A new notice").unwrap());
    show(&lua, Some(&changed));
    call(&lua, "tick", ());
    assert!(texts(&lua).contains("A new notice"));
}

#[test]
fn switching_from_a_saved_world_restores_stock_world_setup() {
    let lua = menu();
    let mut view = starting_from(true, Some(start("mptest", "temperate", 1850, true)), None);
    show(&lua, Some(&view));
    open(&lua, None);
    call(&lua, "click_card", "Click to change the save and mods");
    lua.load("LOAD_PAGE().tiles[1].tile.onClickMain(); assert(LOBBY.endPick())")
        .exec()
        .unwrap();
    call(&lua, "tick", ());
    assert!(
        matches!(sent(&lua).as_slice(), [LobbyAction::ChooseStart { save, .. }] if save.as_str().is_empty())
    );
    view.start_save = None;
    let room = view.room.as_mut().unwrap();
    room.start = None;
    room.members = BoundedVec::new(
        room.members
            .iter()
            .cloned()
            .map(|mut member| {
                member.ready = false;
                member
            })
            .collect(),
    )
    .unwrap();
    show(&lua, Some(&view));
    call(&lua, "tick", ());
    assert!(
        !has_button(&lua, "Start the game"),
        "not ready: the one button sets the world up"
    );
    click(&lua, "Set up world");
    assert_eq!(lua.globals().get::<u32>("GENERATED").unwrap(), 1);
}

#[test]
fn the_loader_can_close_the_lobby_before_menu_callbacks_are_suspended() {
    for already_in_lobby in [false, true] {
        let lua = menu();
        let mut view = in_room(vec![member(1, "Ann", true, true, true)], true);
        let connected = online();
        show(
            &lua,
            Some(if already_in_lobby { &view } else { &connected }),
        );
        open(&lua, None);
        view.room.as_mut().unwrap().running = true;
        show(&lua, Some(&view));
        lua.load("resolveutil.__tpf3mp_before_load()")
            .exec()
            .unwrap();
        assert_eq!(lua.globals().get::<u32>("CLOSED").unwrap(), 1);
    }
    let lua = menu();
    let mut view = in_room(vec![member(1, "Ann", true, true, true)], true);
    view.room.as_mut().unwrap().running = true;
    show(&lua, Some(&view));
    open(&lua, None);
    assert_eq!(lua.globals().get::<u32>("CLOSED").unwrap(), 0);
    assert!(enabled(&lua, "Leave room"));
}

/// The window is one of the game's menu pages: "Multiplayer" in its top
/// bar, whose Back steps back to where the player came from, and out of
/// the window last.
#[test]
fn the_page_back_steps_back_then_leaves() {
    let lua = menu();
    show(&lua, Some(&online()));
    open(&lua, Some("join"));
    let title: String = lua.load("return page_title()").eval().unwrap();
    assert_eq!(title, "Multiplayer");
    assert!(texts(&lua).contains("Public rooms on EU"));
    call(&lua, "page_back", ());
    assert!(
        !texts(&lua).contains("Public rooms on EU"),
        "back to the first page"
    );
    assert_eq!(
        lua.globals().get::<u32>("CLOSED").unwrap(),
        0,
        "the first page"
    );
    call(&lua, "page_back", ());
    assert_eq!(
        lua.globals().get::<u32>("CLOSED").unwrap(),
        1,
        "out of the window"
    );
}

/// Back from the Join with code form closes the form, back to the room
/// list, and only then leaves the page.
#[test]
fn the_page_back_closes_the_join_with_code_form_first() {
    let lua = menu();
    show(&lua, Some(&online()));
    open(&lua, Some("join"));
    click(&lua, "Join with code");
    assert!(texts(&lua).contains("Invite code"));
    call(&lua, "page_back", ());
    assert!(texts(&lua).contains("Public rooms on EU"), "the list again");
    assert_eq!(lua.globals().get::<u32>("CLOSED").unwrap(), 0);
}

/// The launcher's latest notice shows for a few seconds, then goes; its
/// errors stay.
#[test]
fn a_notice_goes_after_a_few_seconds_and_an_error_stays() {
    let lua = menu();
    let mut view = online();
    view.notice = Some(Text::new("the game session ended: you left the room").unwrap());
    show(&lua, Some(&view));
    open(&lua, None);
    assert!(texts(&lua).contains("you left the room"));
    for _ in 0..25 {
        call(&lua, "tick", ());
    }
    assert!(!texts(&lua).contains("you left the room"), "gone");
    view.error = Some(Text::new("the server is gone").unwrap());
    show(&lua, Some(&view));
    for _ in 0..25 {
        call(&lua, "tick", ());
    }
    assert!(texts(&lua).contains("the server is gone"), "an error stays");
}
