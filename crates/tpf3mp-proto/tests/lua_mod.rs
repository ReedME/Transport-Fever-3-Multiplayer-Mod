//! The Lua mod (`mod/tpf3mp_1`) as Transport Fever 3 loads it: laid out as
//! mods made for build 40391 are (`mod.json`, `_content.json`,
//! `_metadata/modinfo.json`, `content/`), and its entry script run in a
//! stand-in for the game's GUI state (`tests/lua/fake_gui.lua`), with and
//! without the hook. What the layout rests on is in
//! `investigation/TF3_MODS_2026-09-27.md`.

#![allow(clippy::unwrap_used)]

mod common;

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use mlua::{Function, Lua, Table};

const MOD_ID: &str = "tpf3mp_1";
const FAKE_GUI: &str = include_str!("lua/fake_gui.lua");

fn mod_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../mod")
        .join(MOD_ID)
}

fn json(path: &str) -> serde_json::Value {
    let text = std::fs::read_to_string(mod_dir().join(path)).unwrap();
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// Every file under `content/`, by its path relative to it.
fn content_files() -> BTreeSet<String> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeSet<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_string_lossy();
                out.insert(rel.replace('\\', "/"));
            }
        }
    }
    let root = mod_dir().join("content");
    let mut out = BTreeSet::new();
    walk(&root, &root, &mut out);
    out
}

#[test]
fn the_mod_is_laid_out_as_tf3_mods_are() {
    assert!(
        !mod_dir().join("mod.lua").exists(),
        "a TPF2 mod.lua is left over"
    );

    let manifest = json("mod.json");
    assert_eq!(manifest["modId"], MOD_ID, "modId is the folder's name");
    assert_eq!(manifest["severityAdd"], "None");
    assert_eq!(manifest["severityRemove"], "None");
    // Build 40408's ModRep::couldAchievementsBeEarned is true when any mod
    // of the save has this flag (framework/mod/modrep.cpp, 0x2f929f0), so
    // a game with TPF3-MP active still earns achievements.
    assert_eq!(manifest["forceActivateAchievements"], true);
    assert!(manifest["revision"].is_u64());
    for script in ["preRunScript", "runScript", "postRunScript"] {
        let file = manifest[script]["fileName"].as_str().unwrap();
        assert!(
            file.is_empty() || file.starts_with(&format!("{MOD_ID}::/")),
            "{script} names another mod's file: {file}"
        );
    }

    let info = json("_metadata/modinfo.json");
    assert_eq!(info["name"], "TPF3-MP");
    assert!(info["summary"].is_string() && info["description"].is_string());
    assert!(
        info["tags"]
            .as_array()
            .unwrap()
            .contains(&"Script Mod".into())
    );

    let listed: Vec<String> = json("_content.json")["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file.as_str().unwrap().to_owned())
        .collect();
    let listed_set: BTreeSet<String> = listed.iter().cloned().collect();
    assert_eq!(listed.len(), listed_set.len(), "a file is listed twice");
    assert_eq!(
        listed_set,
        content_files(),
        "_content.json must list exactly the files under content/"
    );
}

#[test]
fn every_resource_names_a_script_the_mod_has() {
    let files = content_files();
    let resources: Vec<&String> = files.iter().filter(|f| f.ends_with(".res.lua")).collect();
    assert!(!resources.is_empty(), "no GUI resource loads the mod");
    for resource in resources {
        let text = std::fs::read_to_string(mod_dir().join("content").join(resource)).unwrap();
        let prefix = format!("\"{MOD_ID}::/");
        let start = text
            .find(&prefix)
            .unwrap_or_else(|| panic!("{resource} names no file of this mod"))
            + prefix.len();
        let target = &text[start..];
        let target = &target[..target.find('"').unwrap()];
        // A plugin names "script@recipe"; a replacement config names its
        // script and, apart, the function the game calls (doReplaceFn).
        let (script, recipe) = match target.split_once('@') {
            Some(named) => named,
            None => {
                let key = "doReplaceFn = \"";
                let at = text
                    .find(key)
                    .unwrap_or_else(|| panic!("{resource} names no recipe or doReplaceFn"))
                    + key.len();
                (target, &text[at..at + text[at..].find('"').unwrap()])
            }
        };
        assert!(
            files.contains(&format!("{script}.lua")),
            "{resource} names {script}.lua, which is not in content/"
        );
        assert!(!recipe.is_empty());
    }
}

/// A Lua state standing in for the game's GUI state, with the mod's files
/// readable through `mod_source`.
fn gui() -> Lua {
    let lua = Lua::new();
    let source = lua
        .create_function(|_, rel: String| {
            assert!(!rel.contains(".."), "{rel} leaves the mod");
            let path = mod_dir().join("content").join(&rel);
            std::fs::read_to_string(&path)
                .map_err(|error| mlua::Error::external(format!("{rel}: {error}")))
        })
        .unwrap();
    lua.globals().set("mod_source", source).unwrap();
    // What the hook does with an action table: convert it with the schema.
    let schema_check = lua
        .create_function(|_, action: mlua::Value| {
            Ok(
                match tpf3mp_proto::lua::action_from_lua(&common::tree(&action)) {
                    Ok(_) => (true, None),
                    Err(error) => (false, Some(error.to_string())),
                },
            )
        })
        .unwrap();
    lua.globals().set("schema_check", schema_check).unwrap();
    lua.load(FAKE_GUI).set_name("@fake_gui.lua").exec().unwrap();
    lua
}

fn log(lua: &Lua) -> String {
    lua.load("return logText()").eval().unwrap()
}

/// Loads the plugin, mounts it and runs `steps` frames.
fn run_frames(lua: &Lua, steps: usize) {
    lua.load(format!(
        "local m = mount(loadPlugin()); for _ = 1, {steps} do m.render(); m.step() end"
    ))
    .set_name("@frames")
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(lua)));
}

#[test]
fn without_the_hook_the_mod_loads_and_does_nothing() {
    let lua = gui();
    let before: Vec<String> = loaded_names(&lua);
    run_frames(&lua, 3);
    let log = log(&lua);
    assert_eq!(
        log,
        "[tpf3mp] modules loaded\n\
         [tpf3mp] no hook in this game; this is the plain game",
        "started once, however many frames"
    );

    // Only the mod's own names were added to package.loaded.
    let added: Vec<String> = loaded_names(&lua)
        .into_iter()
        .filter(|name| !before.contains(name))
        .collect();
    assert_eq!(
        added,
        [
            "tpf3mp.acceptance",
            "tpf3mp.apply",
            "tpf3mp.banners",
            "tpf3mp.bridge",
            "tpf3mp.capture",
            "tpf3mp.companies",
            "tpf3mp.engine",
            "tpf3mp.follow",
            "tpf3mp.geom",
            "tpf3mp.guard",
            "tpf3mp.hudguard",
            "tpf3mp.junctions",
            "tpf3mp.previews",
            "tpf3mp.progression",
            "tpf3mp.registry",
            "tpf3mp.roads",
            "tpf3mp.ui",
            "tpf3mp.worldload"
        ]
    );
}

/// A player who picked a campaign character shows its portrait beside
/// their name, after their key's banner; an id that is no portrait shows
/// nothing more (docs/LOBBY.md, "Portraits").
#[test]
fn the_multiplayer_window_shows_a_players_portrait_beside_their_name() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    let pictures: Vec<String> = lua
        .load(
            "HOOK.room = true              HOOK.status = { room = 'Sunday line', speed = 200, players = {                  { name = 'Julian', connected = true, owner = true, me = false, id = '00000000',                    banner = 'dr_karl_brandt' },                  { name = 'Sam', connected = true, owner = false, me = true, id = '00000001',                    banner = 'selfie' } } }              BAR = mount(loadPlugin()) BAR.step() BAR.render()              views(BAR.layout)[1].params.onClick()              WINDOWS.Tpf3mpWindow.step()              local pictures = {}              for _, v in ipairs(views(WINDOWS.Tpf3mpWindow.render())) do                  if v.view == 'ImageView' then pictures[#pictures + 1] = v.params.path end              end              return pictures",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}
{}", log(&lua)));
    assert_eq!(
        pictures,
        [
            // Julian's key banner, the first, then his portrait.
            "::/gui/menu/images/m01_ingame.tga",
            "tpf3mp_1::/gui/tpf3mp/portraits/dr_karl_brandt.tga",
            // Sam's unknown id: his key's banner alone.
            "::/gui/menu/images/m02_ingame.tga",
        ]
    );
}

#[test]
fn the_multiplayer_window_shows_the_room_and_sends_what_the_player_says() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load(
        "HOOK.room = true \
         HOOK.status = { room = 'Sunday line', speed = 200, players = { \
             { name = 'Julian', connected = true, owner = true, me = false }, \
             { name = 'Sam', connected = true, owner = false, me = true } } } \
         HOOK.heard = { { from = 'Julian', text = 'the bus is late' } } \
         BAR = mount(loadPlugin()) BAR.step() BAR.render() \
         MODS = mount(loadPlugin(nil, 'Tpf3mpButton', 'MainModButtonAreaExtension')) \
         function texts() \
             local out = {} \
             for _, v in ipairs(views(WINDOWS.Tpf3mpWindow.render())) do \
                 if v.view == 'TextView' then out[#out + 1] = v.params.text end \
             end \
             return out \
         end",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    // The game bar: the room in one line, and a new chat line; the button
    // in the mods' button area says the same new line; no window yet.
    let (label, button, open): (String, String, bool) = lua
        .load(
            "return views(BAR.layout)[1].params.content.params.text, \
                    views(MODS.layout)[1].params.content.params.text, \
                    WINDOWS.Tpf3mpWindow ~= nil",
        )
        .eval()
        .unwrap();
    assert_eq!(label, "Multiplayer: Sunday line · 2/2 playing · 2x · 1 new");
    assert_eq!(button, "Multiplayer (1)");
    assert!(!open, "closed until a button is pressed");
    // The game bar's button opens the window in the game's window
    // container: the room, its players, the chat.
    let (title, texts): (String, Vec<String>) = lua
        .load(
            "views(BAR.layout)[1].params.onClick() \
             WINDOWS.Tpf3mpWindow.step() \
             return WINDOWS.Tpf3mpWindow.layout.params.title, texts()",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(title, "Multiplayer");
    assert_eq!(
        texts,
        [
            "Sunday line",
            "Speed: 2x",
            "Host controls speed",
            "Worlds match",
            "Players",
            "2 of 2 online",
            "Julian",
            "host · Playing",
            "Sam",
            "you · Playing",
            "Companies",
            "Choose who you build with",
            "Waiting for the companies...",
            "Chat",
            "Julian: the bus is late",
            "Send"
        ]
    );
    // What the player types goes to the room.
    let said: Vec<String> = lua
        .load(
            "for _, v in ipairs(views(WINDOWS.Tpf3mpWindow.layout)) do \
                 if v.view == 'TextInputField' then v.params.onValueChange('on my way') end \
             end \
             return HOOK.said",
        )
        .eval()
        .unwrap();
    assert_eq!(said, ["on my way"]);
    // A line typed and then left (a click elsewhere cancels the field) stays
    // in the field as typed, so Send sends what the field shows.
    let (kept, resets): (String, bool) = lua
        .load(
            "local function field() \
                 for _, v in ipairs(views(WINDOWS.Tpf3mpWindow.layout)) do \
                     if v.view == 'TextInputField' then return v end \
                 end \
             end \
             field().params.onTyping('half typed') \
             field().params.onCancel() \
             WINDOWS.Tpf3mpWindow.step() \
             WINDOWS.Tpf3mpWindow.render() \
             return field().params.value, field().params.resetValueOnCancel",
        )
        .eval()
        .unwrap();
    assert_eq!(kept, "half typed");
    assert!(!resets, "the field keeps what was typed");
    // The other button closes it, and opens it again; so does the window's
    // own close button.
    let (closed, reopened, closed_by_itself): (bool, bool, bool) = lua
        .load(
            "views(MODS.layout)[1].params.onClick() \
             local closed = WINDOWS.Tpf3mpWindow == nil \
             views(MODS.layout)[1].params.onClick() \
             local reopened = WINDOWS.Tpf3mpWindow ~= nil \
             WINDOWS.Tpf3mpWindow.layout.params.onClose() \
             return closed, reopened, WINDOWS.Tpf3mpWindow == nil",
        )
        .eval()
        .unwrap();
    assert!(closed && reopened && closed_by_itself);
}

#[test]
fn chat_a_new_world_is_given_again_is_not_new() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    let label: String = lua
        .load(
            "HOOK.room = true \
             HOOK.status = { room = 'r', players = {} } \
             HOOK.heard = { { from = 'Sam', text = 'before', old = true }, \
                            { from = 'Sam', text = 'after' } } \
             BAR = mount(loadPlugin()) BAR.step() BAR.render() \
             return views(BAR.layout)[1].params.content.params.text",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(label, "Multiplayer: r · 0/0 playing · 1 new");
}

#[test]
fn the_room_panel_keeps_large_rosters_and_unicode_chat_inside_scroll_areas() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load(
        r#"
        HOOK.room = true
        HOOK.status = { room = 'A busy room', me_id = 'me', players = {} }
        for i = 1, 8 do
            HOOK.status.players[i] = { name = string.rep('W', 64), id = 'p' .. i,
                connected = i ~= 8, me = i == 1 }
        end
        BAR = mount(loadPlugin()) BAR.step() BAR.render()
        local shared = package.loaded['tpf3mp.ui']
        shared.companies = { list = {}, members = {}, loans = {} }
        api.type.Vec3f = { new = function(x,y,z) return { x=x, y=y, z=z } end }
        for i = 0, 7 do
            shared.companies.list[i+1] = { id=i, entity=i+1, name=string.rep('ç•Œ', 64),
                color={0.8,0.2,0.1}, balance=44149292, owed=50050007 }
        end
        shared.lines = { string.rep('ç•Œ', 280) }
        views(BAR.layout)[1].params.onClick()
        LAYOUT = WINDOWS.Tpf3mpWindow.render()
        "#,
    )
    .exec()
    .unwrap();
    let (areas, cards, company_text, chat_text, composer_outside): (
        usize,
        usize,
        String,
        String,
        bool,
    ) = lua
        .load(
            r#"
            local areas, cards, companyText, chatText = {}, 0, '', ''
            for _, v in ipairs(views(LAYOUT)) do
                if v.view == 'ScrollArea' then
                    assert(v.params.horizontalPolicy == 'AlwaysOff')
                    assert(v.params.verticalPolicy == 'AsNeeded')
                    assert(v.params.meta.styleSheet.size.y > 0 and v.params.meta.styleSheet.size.y <= 310)
                    areas[#areas+1] = v
                end
            end
            for _, v in ipairs(views(areas[2])) do
                if v.view == 'TextInputField' and v.params.placeholderText:find('A new name', 1, true) then
                    assert(cards == 1, 'manage your own company before scrolling past the other companies')
                end
                if v.view == 'TextView' and v.params.meta.tooltip == string.rep('ç•Œ',64) then
                    cards = cards + 1
                    companyText = v.params.text:gsub('\n','')
                end
            end
            for _, v in ipairs(views(areas[3])) do
                if v.view == 'TextView' then chatText = v.params.text:gsub('\n','') end
            end
            local composer, inside = false, false
            for _, v in ipairs(views(LAYOUT)) do
                if v.view == 'TextInputField' and v.params.placeholderText == 'Say something to the room' then
                    composer = true
                    for _, area in ipairs(areas) do
                        for _, child in ipairs(views(area)) do if child == v then inside = true end end
                    end
                end
            end
            return #areas, cards, companyText, chatText, composer and not inside
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!((areas, cards), (3, 8));
    assert_eq!(company_text, "ç•Œ".repeat(64));
    assert_eq!(chat_text, "ç•Œ".repeat(280));
    assert!(
        composer_outside,
        "chat can be sent without scrolling past a roster"
    );
}

#[test]
fn only_the_newest_chat_lines_show_in_the_window() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    let chat: Vec<String> = lua
        .load(
            "HOOK.room = true \
             HOOK.status = { room = 'r', players = {} } \
             for i = 1, 60 do HOOK.heard[i] = { from = 'Sam', text = 'line ' .. i } end \
             BAR = mount(loadPlugin()) BAR.step() BAR.render() \
             views(BAR.layout)[1].params.onClick() \
             local out, after = {}, false \
             for _, v in ipairs(views(WINDOWS.Tpf3mpWindow.render())) do \
                 if v.view == 'TextView' and after and v.params.text ~= 'Send' then out[#out + 1] = v.params.text end \
                 if v.view == 'TextView' and v.params.text == 'Chat' then after = true end \
             end \
             return out",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let expected: Vec<String> = (49..=60).map(|i| format!("Sam: line {i}")).collect();
    assert_eq!(chat, expected);
}

#[test]
fn a_window_the_game_will_not_show_is_said_in_the_game_bar() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    let (shown, open): (String, bool) = lua
        .load(
            "HOOK.room = true \
             HOOK.status = { room = 'r', players = {} } \
             BAR = mount(loadPlugin()) BAR.step() BAR.render() \
             ug_require('::/gui/main/game_react_globals.tl').getDefaultWindowApi = function() error('no window container') end \
             views(BAR.layout)[1].params.onClick() \
             BAR.step() BAR.render() \
             local v = views(BAR.layout) \
             return v[#v].params.text, package.loaded['tpf3mp.ui'].open",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(shown, "The Multiplayer window did not open");
    assert!(!open);
    assert!(
        log(&lua).contains("the Multiplayer window did not open: "),
        "{}",
        log(&lua)
    );
}

/// The names in package.loaded, sorted.
fn loaded_names(lua: &Lua) -> Vec<String> {
    let loaded: Table = lua.load("return package.loaded").eval::<Table>().unwrap();
    let mut names: Vec<String> = loaded
        .pairs::<String, mlua::Value>()
        .map(|pair| pair.unwrap().0)
        .collect();
    names.sort();
    names
}

const FAKE_HOOK: &str = r#"
HOOK = { logged = {}, commands = {}, batch = nil, request = nil, saved = {}, worlds = 0,
         room = false, checkpoint = false, lanes = nil, clicks = nil, replaying = {},
         applied = {}, results = {}, status = nil, heard = {}, said = {}, built = {},
         dump = nil, dumped = {} }
tpf3mp_native = {
    copy = function(text) HOOK.copied = text return true end,
    version = 15,
    note = function(key, value)
        HOOK.notes = HOOK.notes or {}
        if value == nil then return HOOK.notes[key] end
        HOOK.notes[key] = value ~= "" and value or nil
    end,
    command = function(action, password)
        local ok, why = schema_check(action)
        if ok then
            HOOK.commands[#HOOK.commands + 1] = action
            -- What the hook would send the room beside it, by ticket.
            HOOK.passwords = HOOK.passwords or {}
            HOOK.passwords[#HOOK.commands] = password
            return true, #HOOK.commands
        end
        return ok, why
    end,
    take = function()
        local batch, origins, seals = HOOK.batch, HOOK.origins, HOOK.seals
        HOOK.batch, HOOK.origins, HOOK.seals = nil, nil, nil
        return batch, origins, seals
    end,
    takeReplay = function(token)
        if token ~= HOOK.replayToken or HOOK.replayTaken then return nil end
        HOOK.replayTaken = true
        local batch = HOOK.replayBatch
        return batch, HOOK.replayOrigins, HOOK.replaySeals
    end,
    replayed = function(token, ok, why)
        HOOK.replayDone = { token = token, ok = ok, why = why }
    end,
    log = function(line) HOOK.logged[#HOOK.logged + 1] = line end,
    poll = function()
        local request = HOOK.request
        HOOK.request = nil
        return request
    end,
    saved = function(name, ok, why)
        HOOK.saved[#HOOK.saved + 1] = tostring(name) .. ' ' .. tostring(ok) .. ' ' .. tostring(why)
    end,
    world = function() HOOK.worlds = HOOK.worlds + 1 end,
    room = function() return HOOK.room end,
    checkpoint = function() return HOOK.checkpoint, HOOK.scanStep end,
    scanned = function(ok, why) HOOK.scanResult = {ok=ok,why=why} return ok end,
    seed = function() return HOOK.seed end,
    lanes = function(lanes)
        if not HOOK.checkpoint then return false, 'no checkpoint is due in this update' end
        HOOK.lanes = lanes
        HOOK.checkpoint = false
        return true
    end,
    clicks = function() return HOOK.clicks end,
    -- The module editor's builds the hook read, by click: { proposal = t }
    -- or { why = text }, each taken once.
    built = function(n)
        local b = HOOK.built[n]
        HOOK.built[n] = nil
        if b == nil then return nil end
        if b.why then return nil, b.why end
        return b.proposal
    end,
    -- The street and track tools' toolbar crossing type by click, as the
    -- hook reads every player build's: none picked, unless a test says.
    toolCrossing = function(n) return false end,
    replaying = function(on) HOOK.replaying[#HOOK.replaying + 1] = on end,
    applied = function(i, ok, entity, why)
        HOOK.applied[#HOOK.applied + 1] = { i = i, ok = ok, entity = entity, why = why }
    end,
    results = function()
        local results = HOOK.results
        HOOK.results = {}
        return results
    end,
    status = function() return HOOK.status end,
    chat = function()
        local heard = HOOK.heard
        HOOK.heard = {}
        return heard
    end,
    say = function(text)
        if text:match('^%s*$') then return false, 'nothing to say' end
        HOOK.said[#HOOK.said + 1] = text
        return true
    end,
    -- The player's previews, 'none' for nothing shown; and the other
    -- members' changes the test puts in HOOK.incoming, each taken once.
    preview = function(action)
        if action ~= nil then
            local ok, why = schema_check(action)
            if not ok then return ok, why end
        end
        HOOK.previewed = HOOK.previewed or {}
        HOOK.previewed[#HOOK.previewed + 1] = action or 'none'
        return true
    end,
    previews = function()
        local changes = HOOK.incoming or {}
        HOOK.incoming = {}
        return changes
    end,
    -- Drawing another member's preview: armed for one member, then what
    -- makeProposalData evaluated while armed is drawn; HOOK.drawing says
    -- what happened, in order.
    draw = function(from)
        if #from ~= 64 then return false, 'a member is 64 hex digits' end
        HOOK.armed = from
        return true
    end,
    drawn = function()
        local evaluated = HOOK.evaluated
        HOOK.armed, HOOK.evaluated = nil, nil
        if evaluated == nil then return nil end
        HOOK.drawing = HOOK.drawing or {}
        HOOK.drawing[#HOOK.drawing + 1] = 'drew ' .. evaluated
        return true
    end,
    undraw = function(from)
        HOOK.drawing = HOOK.drawing or {}
        HOOK.drawing[#HOOK.drawing + 1] = 'undrew ' .. from:sub(1, 2)
        return true
    end,
    -- A lane dump the hook asks for ({ step =, lanes = }), once; the
    -- entries go to HOOK.dumped as the hook writes them to its log.
    dump = function()
        local order = HOOK.dump
        HOOK.dump = nil
        HOOK.dumping = order
        return order
    end,
    dumped = function(lane, entry)
        local order = HOOK.dumping
        if order == nil then return false end
        HOOK.dumped[#HOOK.dumped + 1] = 'lane ' .. lane .. ' step ' .. order.step .. ' ' .. entry
        return true
    end,
}
"#;

/// The GUI state's api.cmd, as much of it as the guard's tests use: three
/// factories, and a sendCommand that keeps what it was sent.
const FAKE_CMD: &str = r#"
SENT = {}
api = api or {}
api.cmd = {
    makeGameSetSpeedCmd = function(speed) return { kind = 'speed', speed = speed } end,
    makeVehicleBuyCmd = function(player, depot, config) return { kind = 'buy', depot = depot } end,
    makeLineCreateCmd = function(line) return { kind = 'line' } end,
    makeScriptingSendEventCmd = function(src, id, name, param)
        return { kind = 'event', id = id, name = name, param = param }
    end,
    sendCommand = function(command, ...)
        SENT[#SENT + 1] = { command = command, extra = select('#', ...), callback = (...) }
    end,
}
"#;

#[test]
fn the_speed_row_shows_the_room_and_only_the_host_can_use_it() {
    for with_hook in [false, true] {
        let lua = gui();
        if with_hook {
            lua.load(FAKE_HOOK).exec().unwrap();
        }
        lua.load(include_str!("lua/speed_ui.lua"))
            .set_name("@speed_ui.lua")
            .exec()
            .unwrap_or_else(|error| panic!("hook={with_hook}: {error}"));
    }
}

/// The game's save and load, as the GUI state has them.
const FAKE_APP: &str = r#"
APP = { saves = {}, loads = {} }
app = {
    saveGame = function(name, callback, isMapEditor, skipSetName)
        APP.saves[#APP.saves + 1] = { name = name, callback = callback,
                                      isMapEditor = isMapEditor, skipSetName = skipSetName }
    end,
    loadGame = function(id, isMapEditor, info)
        APP.loads[#APP.loads + 1] = { id = id, isMapEditor = isMapEditor }
    end,
    SaveGameNamespace = { getSavegame = function() return "savegame" end },
}
api = { type = { SavegameId = { new = function() return {} end } } }
"#;

#[test]
fn with_the_hook_the_gui_links_once() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    run_frames(&lua, 2);
    assert_eq!(
        log(&lua),
        "[tpf3mp] modules loaded\n[tpf3mp] linked to the hook"
    );
    let logged: String = lua
        .load("return table.concat(HOOK.logged, '|')")
        .eval()
        .unwrap();
    assert_eq!(
        logged,
        "the GUI is linked|the guard is on 4 command factories|\
         the GUI's company cannot follow the player's: no api.engine.util.getPlayer (nil, nil)|\
         the company window shows the game's own rank only: the game's company progression did \
         not load: fake_gui.lua:149: ug_require of an unknown path \
         /game_mechanics/company/company_progression_util.tl|\
         the game's permits count the whole world's constructions: the game's company_metadata \
         did not load: fake_gui.lua:149: ug_require of an unknown path \
         /game_mechanics/company/company_metadata.tl|\
         the line manager offers other companies' open stations (1 entity_util table(s))"
    );
    let worlds: u32 = lua.load("return HOOK.worlds").eval().unwrap();
    assert_eq!(worlds, 1, "the world's GUI started once");
}

#[test]
fn the_gui_saves_what_the_hook_asks_and_answers_when_written() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_APP).exec().unwrap();
    lua.load("M = mount(loadPlugin())").exec().unwrap();
    lua.load("M.step() HOOK.request = { save = 'tpf3mp_77_5' } M.step()")
        .exec()
        .unwrap();
    let (name, map_editor, skip): (String, bool, bool) = lua
        .load("local s = APP.saves[1] return s.name, s.isMapEditor, s.skipSetName")
        .eval()
        .unwrap();
    assert_eq!(name, "tpf3mp_77_5");
    assert!(!map_editor);
    assert!(skip, "the player's own save name is left alone");
    let answered: usize = lua.load("return #HOOK.saved").eval().unwrap();
    assert_eq!(answered, 0, "not written yet");
    lua.load("APP.saves[1].callback()").exec().unwrap();
    let saved: Vec<String> = lua.load("return HOOK.saved").eval().unwrap();
    assert_eq!(saved, ["tpf3mp_77_5 true nil"]);
    // A save the game refuses at once is answered as failed.
    lua.load(
        "app.saveGame = function() error('no disk') end \
         HOOK.request = { save = 'tpf3mp_77_6' } M.step()",
    )
    .exec()
    .unwrap();
    let saved: Vec<String> = lua.load("return HOOK.saved").eval().unwrap();
    assert!(saved[1].starts_with("tpf3mp_77_6 false "), "{}", saved[1]);
    assert!(saved[1].ends_with("no disk"), "{}", saved[1]);
}

#[test]
fn the_gui_loads_the_rooms_world_from_the_save_folder() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_APP).exec().unwrap();
    lua.load(
        "M = mount(loadPlugin()) M.step() HOOK.request = { load = 'tpf3mp_room_77' } M.step()",
    )
    .exec()
    .unwrap();
    let loaded: String = lua
        .load(
            "local l = APP.loads[1] \
             return l.id.path .. '|' .. l.id.saveGameName .. '|' .. l.id.saveGameNamespace \
               .. '|' .. tostring(l.isMapEditor)",
        )
        .eval()
        .unwrap();
    assert_eq!(loaded, "|tpf3mp_room_77|savegame|false");
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert_eq!(logged.last().unwrap(), "loading the room's world");
}

/// The hook's `mods` as the room's lists make it (crates/tpf3mp-bridge,
/// `mods::plan`), and the game's save details, mods and ModId, for a load
/// with the room's mods (docs/MODS.md).
const FAKE_MODS: &str = r#"
SHARED = { vehicles_pack = true }
MINE = { 'my_colours' }
tpf3mp_native.mods = function(list)
    if list == nil then return true end
    local keep, dropped, added = {}, {}, {}
    for name in string.gmatch(list, '[^\n]+') do
        if SHARED[name] or name == 'tpf3mp_1' or name == MINE[1] then keep[#keep + 1] = name
        else dropped[#dropped + 1] = name end
    end
    keep[#keep + 1] = MINE[1] added[1] = MINE[1]
    return table.concat(keep, '\n'), table.concat(dropped, '\n'), table.concat(added, '\n')
end
SAVED = { 'vehicles_pack', 'tpf3mp_1', 'owner_minimap' }
INSTALLED = { vehicles_pack = true, tpf3mp_1 = true, my_colours = true }
READY = false
api.type.ModId = { new = function() return {} end }
api.type.SaveGameDetails = { new = function(info)
    local copy = {} for k, v in pairs(info) do copy[k] = v end return copy end }
app.getSavegameInfo = function(id)
    local mods = {}
    for i, name in ipairs(SAVED) do mods[i] = { name = name } end
    return { isCompleted = function() return READY end,
             get = function() return { errorMsg = '', info = { mods = mods } } end }
end
app.getUserProfile = function() return { getModRep = function() return {
    exists = function(_, m) return INSTALLED[m.name] == true end } end } end
local load = app.loadGame
app.loadGame = function(id, isMapEditor, info)
    load(id, isMapEditor, info)
    local names = {}
    for _, m in ipairs(info and info.mods or {}) do names[#names + 1] = m.name end
    APP.loads[#APP.loads].mods = table.concat(names, ',')
end
"#;

#[test]
fn the_gui_loads_the_rooms_world_with_the_rooms_mods_and_its_own() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_APP).exec().unwrap();
    lua.load(FAKE_MODS).exec().unwrap();
    lua.load(
        "M = mount(loadPlugin()) M.step() HOOK.request = { load = 'tpf3mp_room_77' } M.step()",
    )
    .exec()
    .unwrap();
    // The game reads the save's details over a few frames.
    let loads: usize = lua.load("M.step() return #APP.loads").eval().unwrap();
    assert_eq!(loads, 0);
    let (name, mods): (String, String) = lua
        .load("READY = true M.step() return APP.loads[1].id.saveGameName, APP.loads[1].mods")
        .eval()
        .unwrap();
    assert_eq!(name, "tpf3mp_room_77");
    assert_eq!(
        mods, "vehicles_pack,tpf3mp_1,my_colours",
        "the owner's minimap left out, this player's colours added"
    );
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert_eq!(logged.last().unwrap(), "loading the room's world");

    // A shared mod this player lacks: not loaded, and said why.
    lua.load("INSTALLED.vehicles_pack = nil HOOK.request = { load = 'tpf3mp_room_78' } M.step()")
        .exec()
        .unwrap();
    let (loads, logged): (usize, Vec<String>) =
        lua.load("return #APP.loads, HOOK.logged").eval().unwrap();
    assert_eq!(loads, 1);
    assert_eq!(
        logged.last().unwrap(),
        "loading the room's world failed: the room's world needs the mod vehicles_pack, which is not installed"
    );
}

#[test]
fn a_hook_of_another_version_is_not_used() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load("tpf3mp_native.version = 1").exec().unwrap();
    run_frames(&lua, 1);
    assert!(
        log(&lua).ends_with(
            "[tpf3mp] the hook speaks bridge version 1, the mod 15; this is the plain game"
        ),
        "{}",
        log(&lua)
    );
    let logged: usize = lua.load("return #HOOK.logged").eval().unwrap();
    assert_eq!(logged, 0, "nothing was said to a hook of another version");
}

/// The bridge on its own, loaded as the entry script loads it.
fn bridge(lua: &Lua) -> Table {
    lua.load("return ug_require('tpf3mp_1::/scripts/tpf3mp/bridge.lua')")
        .eval()
        .unwrap()
}

#[test]
fn the_bridge_hands_over_only_actions_the_schema_takes() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    let bridge = bridge(&lua);
    let attach: Function = bridge.get("attach").unwrap();
    let native: Table = lua.globals().get("tpf3mp_native").unwrap();
    let link: Table = attach.call(native).unwrap();
    lua.globals().set("LINK", link).unwrap();

    let refusals: Vec<String> = lua
        .load(
            "local out = {}
             local function try(p) local ok, why = LINK:command(p); out[#out + 1] = ok and 'ok' or why end
             try('bytes')
             try({ SellVehicle = { vehicles = { 7, 9 } } })
             try({ SellVehicle = { vehicles = { 0.5 } } })
             try({ SellVehicle = { vehicles = {}, colour = 'red' } })
             tpf3mp_native.command = function() return false end
             try({ SellVehicle = { vehicles = { 7 } } })
             tpf3mp_native.command = function() error('ring full') end
             try({ SellVehicle = { vehicles = { 7 } } })
             return out",
        )
        .eval()
        .unwrap();
    assert_eq!(refusals[0], "an action is a table");
    assert_eq!(refusals[1], "ok");
    assert_eq!(
        refusals[2],
        "the hook refused the action: SellVehicle.vehicles[1]: not a whole number: 0.5"
    );
    assert_eq!(
        refusals[3],
        "the hook refused the action: SellVehicle: variant has no field colour"
    );
    assert_eq!(refusals[4], "the hook refused the action: no reason given");
    assert!(refusals[5].starts_with("the hook refused: "));
    assert!(refusals[5].ends_with("ring full"));
    let sent: usize = lua.load("return #HOOK.commands").eval().unwrap();
    assert_eq!(sent, 1, "only the action the schema took reached the room");
}

#[test]
fn take_is_a_list_or_nothing_and_never_raises() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    let bridge = bridge(&lua);
    lua.globals().set("BRIDGE", bridge).unwrap();
    let results: Vec<String> = lua
        .load(
            "local link = BRIDGE.attach(tpf3mp_native)
             local out = {}
             out[#out + 1] = tostring(link:take())
             HOOK.batch = { { SellVehicle = { vehicles = { 7 } } } }
             out[#out + 1] = tostring(#link:take())
             out[#out + 1] = tostring(link:take())
             tpf3mp_native.take = function() error('boom') end
             out[#out + 1] = tostring(link:take())
             return out",
        )
        .eval()
        .unwrap();
    assert_eq!(results, ["nil", "1", "nil", "nil"]);
}

#[test]
fn attach_refuses_a_partial_hook() {
    let lua = gui();
    let bridge = bridge(&lua);
    lua.globals().set("BRIDGE", bridge).unwrap();
    let reasons: Vec<String> = lua
        .load(
            "local out = {}
             local function why(t) local _, r = BRIDGE.attach(t); out[#out + 1] = r end
             why(nil)
             why('hook')
             why({ version = 15, command = print, log = print })
             return out",
        )
        .eval()
        .unwrap();
    assert_eq!(
        reasons,
        [
            "no hook in this game",
            "tpf3mp_native is not a table",
            "the hook has no take()",
        ]
    );
}

/// The game bar's text, if the plugin shows any.
fn shown(lua: &Lua) -> Option<String> {
    lua.load(
        "local c = M.render().params.children[1] \
         return c and c.params.text",
    )
    .eval()
    .unwrap()
}

#[test]
fn in_the_rooms_game_the_gui_refuses_what_the_room_cannot_carry() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load("M = mount(loadPlugin()) M.step()").exec().unwrap();

    // Before the room's game every command is sent, arguments as given.
    lua.load("api.cmd.sendCommand(api.cmd.makeVehicleBuyCmd(1, 2, {}))")
        .exec()
        .unwrap();
    let (sent, extra): (usize, usize) = lua.load("return #SENT, SENT[1].extra").eval().unwrap();
    assert_eq!(
        (sent, extra),
        (1, 0),
        "a callback left out is not passed as nil"
    );

    // In the room's game the speed row's speed is sent; a vehicle bought at
    // a depot the room cannot name is refused, and its callback hears so on
    // the next frame.
    lua.load(
        "HOOK.room = true \
         api.cmd.sendCommand(api.cmd.makeGameSetSpeedCmd(4)) \
         CALLED = nil \
         BUY = api.cmd.makeVehicleBuyCmd(1, 2, {}) \
         api.cmd.sendCommand(BUY, function(data, ok, entities) \
             CALLED = { data = data, ok = ok, entities = #entities } end)",
    )
    .exec()
    .unwrap();
    let (sent, speed): (usize, u32) = lua
        .load("return #SENT, SENT[2].command.speed")
        .eval()
        .unwrap();
    assert_eq!((sent, speed), (2, 4), "the speed went, the vehicle did not");
    let called: bool = lua.load("return CALLED ~= nil").eval().unwrap();
    assert!(!called, "not within sendCommand");
    lua.load("M.step()").exec().unwrap();
    let (same, ok, entities): (bool, bool, usize) = lua
        .load("return CALLED.data == BUY, CALLED.ok, CALLED.entities")
        .eval()
        .unwrap();
    assert!(same && !ok, "the callback heard the command failed");
    assert_eq!(entities, 0);
    assert_eq!(
        shown(&lua).as_deref(),
        Some("Not in multiplayer yet: buying vehicles")
    );

    // A command no factory made is refused too.
    lua.load("api.cmd.sendCommand({ kind = 'forged' }) M.step()")
        .exec()
        .unwrap();
    assert_eq!(
        shown(&lua).as_deref(),
        Some("Not in multiplayer yet: this action")
    );
    let sent: usize = lua.load("return #SENT").eval().unwrap();
    assert_eq!(sent, 2);
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged.contains(
            &"refused the player's makeVehicleBuyCmd in the room's game (1 so far): \
               a depot the room cannot name: no construction component to read"
                .to_owned()
        ),
        "{logged:?}"
    );
    assert!(
        logged.contains(
            &"refused the player's command no factory made in the room's game (1 so far)"
                .to_owned()
        ),
        "{logged:?}"
    );

    // The notice goes after a few seconds; after the room's game, commands
    // are sent again.
    lua.load("for _ = 1, 400 do M.step() end").exec().unwrap();
    assert_eq!(shown(&lua), None);
    lua.load("HOOK.room = false api.cmd.sendCommand(api.cmd.makeLineCreateCmd({}))")
        .exec()
        .unwrap();
    let sent: usize = lua.load("return #SENT").eval().unwrap();
    assert_eq!(sent, 3);
}

/// A loan offer as the game's loan script and finance window keep it.
const OFFER: &str = "{ type = 'Small', amount = 5000000, duration = 1095000, \
                       percentage = 0.03, birthDay = 400000 }";

#[test]
fn in_the_rooms_game_a_loan_goes_to_the_room_and_nothing_else_of_its_kind() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load("M = mount(loadPlugin()) M.step() HOOK.room = true")
        .exec()
        .unwrap();
    // The finance window's "Obtain", as it sends it.
    lua.load(format!(
        "NEXT = {OFFER} NEXT.amount = 7000000 \
         CALLED = nil \
         api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Loan', 'Obtain', {{ NEXT, {OFFER} }}), \
             function(data, ok) CALLED = ok end) \
         M.step()"
    ))
    .exec()
    .unwrap();
    let (sent, handed, called): (usize, usize, bool) = lua
        .load("return #SENT, #HOOK.commands, CALLED ~= nil")
        .eval()
        .unwrap();
    assert_eq!(sent, 0, "not run here: the room orders it for every game");
    assert_eq!(handed, 1, "handed to the room, through the schema");
    assert!(!called, "the room has not applied it yet");
    // This game applied the room's action: the window hears it went.
    lua.load("HOOK.results = { { ticket = 1, ok = true } } M.step()")
        .exec()
        .unwrap();
    let called: bool = lua.load("return CALLED == true").eval().unwrap();
    assert!(called, "the window hears it went");
    let (take, amount): (bool, u32) = lua
        .load("local l = HOOK.commands[1].Loan return l.Take ~= nil, l.Take.offer.amount")
        .eval()
        .unwrap();
    assert!(take);
    assert_eq!(amount, 5_000_000);
    // Paying back goes too.
    lua.load(format!(
        "api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Loan', 'Repay', {{ nil, {OFFER} }}))"
    ))
    .exec()
    .unwrap();
    let repay: bool = lua
        .load("return HOOK.commands[2].Loan.Repay.loan.amount == 5000000")
        .eval()
        .unwrap();
    assert!(repay);
    // Another script event is refused, as is a loan the schema does not
    // take.
    lua.load(
        "api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'MakeGreen', 'go', {})) \
         api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Loan', 'Obtain', \
             { { type = 'Small' }, { type = 'Small', amount = 1.5 } }))",
    )
    .exec()
    .unwrap();
    let (sent, handed): (usize, usize) = lua.load("return #SENT, #HOOK.commands").eval().unwrap();
    assert_eq!((sent, handed), (0, 2));
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged
            .iter()
            .any(|l| l.contains("makeScriptingSendEventCmd")
                && l.contains("the hook refused the action: Loan.Take")),
        "{logged:?}"
    );
}

#[test]
fn the_guard_goes_on_once_and_a_hook_that_cannot_say_means_the_room() {
    let lua = gui();
    lua.load(FAKE_CMD).exec().unwrap();
    let results: Vec<String> = lua
        .load(
            "local guard = ug_require('tpf3mp_1::/scripts/tpf3mp/guard.lua')
             local env = { inRoom = function() return true end,
                           refused = function() end, later = function() end }
             local out = {}
             out[#out + 1] = tostring(guard.install(api.cmd, env))
             local send = api.cmd.sendCommand
             out[#out + 1] = tostring(guard.install(api.cmd, env))
             out[#out + 1] = tostring(api.cmd.sendCommand == send)
             out[#out + 1] = select(2, guard.install(nil, env))
             out[#out + 1] = select(2, guard.install({}, env))
             local bridge = ug_require('tpf3mp_1::/scripts/tpf3mp/bridge.lua')
             local native = { version = 15 }
             for _, n in ipairs({ 'command', 'take', 'log', 'poll', 'saved', 'world',
                                  'checkpoint', 'lanes', 'clicks', 'replaying', 'applied', 'results',
                                  'status', 'chat', 'say', 'takeReplay', 'replayed', 'scanned' }) do
                 native[n] = function() end
             end
             native.room = function() error('gone') end
             out[#out + 1] = tostring(bridge.attach(native):room())
             native.room = function() return 1 end
             out[#out + 1] = tostring(bridge.attach(native):room())
             return out",
        )
        .eval()
        .unwrap();
    assert_eq!(
        results,
        [
            "4",
            "4",
            "true",
            "api.cmd is not a table",
            "api.cmd has no sendCommand",
            "true",
            "false"
        ]
    );
}

#[test]
fn every_game_script_names_a_script_the_mod_has() {
    let files = content_files();
    let scripts: Vec<&String> = files.iter().filter(|f| f.ends_with(".gs.lua")).collect();
    assert!(
        !scripts.is_empty(),
        "no game script applies the room's actions"
    );
    for script in scripts {
        let folder = script.rsplit_once('/').map_or("", |(folder, _)| folder);
        let text = std::fs::read_to_string(mod_dir().join("content").join(script)).unwrap();
        let mut named = 0;
        for part in text.split("fileName = \"").skip(1) {
            let target = &part[..part.find('"').unwrap()];
            let (file, function) = target.split_once('@').unwrap();
            assert!(
                files.contains(&format!("{folder}/{file}.lua")),
                "{script} names {file}.lua, which is not in {folder}/"
            );
            assert!(!function.is_empty());
            named += 1;
        }
        assert!(named > 0, "{script} names no script");
    }
}

/// A stand-in for an engine (game script) state: the commands it is sent
/// run at once, as the game's do there. REFUSE makes sendCommand raise,
/// FAILS makes a callback hear that the command failed, NO_CALLBACKS
/// refuses every callback.
const FAKE_ENGINE: &str = r#"
SENT = {}
REFUSE, FAILS, NO_CALLBACKS = false, false, false
local function vec4(x, y, z, w) return { x, y, z, w } end
api = {
    type = {
        ComponentType = { CONSTRUCTION = 2, TRANSPORT_VEHICLE = 4, STATION_GROUP = 9 },
        Vec4f = { new = vec4 },
        Mat4f = { new = function(a, b, c, d) return { a, b, c, d } end },
        SimpleProposal = {
            new = function() return { constructionsToAdd = {} } end,
            ConstructionEntity = { new = function() return {} end },
        },
        Context = { new = function() return {} end },
        Vec3f = { new = function(x, y, z) return { x = x, y = y, z = z } end },
    },
    engine = {
        util = { getPlayer = function() return 25 end },
        -- Nothing to list, unless a test's world says otherwise.
        getEntitiesWithComponent = function() return {} end,
        system = { lineSystem = { getLines = function() return {} end } },
    },
    cmd = {
        makeWorldBuildProposalCmd = function(proposal, context, ignoreErrors, playerInitiated)
            return { proposal = proposal, context = context, ignoreErrors = ignoreErrors,
                     playerInitiated = playerInitiated }
        end,
        makeScriptingSendEventCmd = function(src, id, name, param)
            return { event = { src = src, id = id, name = name, param = param } }
        end,
        makeGameAddPlayerCmd = function(name, color)
            NEXT_PLAYER = (NEXT_PLAYER or 900) + 1
            return { addPlayer = name, color = color, resultEntity = NEXT_PLAYER }
        end,
        makeEntitySetNameCmd = function(entity, name) return { setName = name, entity = entity } end,
        sendCommand = function(command, callback)
            -- As the game in a game script: no callback in update; in
            -- postUpdate one is called at once, with the command's data (the
            -- command here), whether it went, and what it made.
            if callback ~= nil and (PHASE == 'update' or NO_CALLBACKS) then
                error('Callbacks are currently disallowed')
            end
            if REFUSE then error('the proposal collides') end
            SENT[#SENT + 1] = command
            if callback ~= nil then
                callback(command, not FAILS, command.made and { { command.made, 1 } } or {})
            end
        end,
    },
}
STATE = {
    subscribed = {},
    value = nil,
    get = function(self) return self.value end,
    set = function(self, value) self.value = value end,
    hasEventSubscriptions = function(self) return next(self.subscribed) ~= nil end,
    subscribeToEvent = function(self, name) self.subscribed[name] = true end,
}
"#;

/// The mod's game script in a stand-in engine state with the fake hook:
/// returns the state and the script's functions.
fn engine() -> (Lua, Table) {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_ENGINE).exec().unwrap();
    let source = std::fs::read_to_string(
        mod_dir()
            .join("content")
            .join("tpf3mp_sim")
            .join("tpf3mp_sim.script.lua"),
    )
    .unwrap();
    lua.load(&source)
        .set_name("@tpf3mp_sim.script.lua")
        .exec()
        .unwrap();
    let script: Table = lua.load("return data()").eval().unwrap();
    // How long an action took depends on the machine: no test's log has
    // the line a slow one adds, but the one that asks for it.
    lua.load("ug_require('tpf3mp_1::/scripts/tpf3mp/apply.lua').SLOW_MS = math.huge")
        .exec()
        .unwrap();
    // One simulation update as the game runs it: update, then postUpdate
    // with what update returned, and not when that is nil.
    lua.globals().set("SCRIPT", script.clone()).unwrap();
    lua.load(
        "UPDATE = function(p, s, dt) \
             if BEFORE_UPDATE then BEFORE_UPDATE() end \
             PHASE = 'update' \
             local r = SCRIPT.update(p, s, dt) \
             PHASE = 'post' \
             if r ~= nil then SCRIPT.postUpdate(p, s, dt, r) end \
             PHASE = nil \
             return r \
         end",
    )
    .exec()
    .unwrap();
    (lua, script)
}

fn junction_game(offset: u32) -> Lua {
    let (lua, _) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.globals().set("OFFSET", offset).unwrap();
    lua.load(include_str!("lua/junction_world.lua"))
        .exec()
        .unwrap();
    lua
}

fn lua_value(lua: &Lua, value: &tpf3mp_proto::lua::LuaValue) -> mlua::Value {
    use tpf3mp_proto::lua::LuaValue as V;
    match value {
        V::Nil => mlua::Value::Nil,
        V::Boolean(v) => mlua::Value::Boolean(*v),
        V::Integer(v) => mlua::Value::Integer(*v),
        V::Number(v) => mlua::Value::Number(*v),
        V::String(v) => mlua::Value::String(lua.create_string(v).unwrap()),
        V::Table(entries) => {
            let table = lua.create_table().unwrap();
            for (k, v) in entries {
                table.set(lua_value(lua, k), lua_value(lua, v)).unwrap();
            }
            mlua::Value::Table(table)
        }
    }
}

#[test]
fn junction_tools_round_trip_through_the_wire_and_apply_with_each_games_ids() {
    use tpf3mp_proto::{
        action::Action,
        lua::{action_from_lua, action_to_lua},
    };
    let source = junction_game(0);
    let captured: mlua::Value = source
        .load("return C.windowBuild(nil,PROPOSAL)")
        .eval()
        .unwrap();
    let action = action_from_lua(&common::tree(&captured)).unwrap();
    let Action::EditJunctions(edit) = &action else {
        panic!("{action:?}");
    };
    let config = edit.changes[0].config.as_ref().unwrap();
    assert_eq!(
        (config.phases[0].duration, config.phases[0].minimum),
        (12_375, 4_125)
    );
    assert_eq!(config.crosswalks.len(), 2);
    let portable = Action::from_payload(&action.to_payload().unwrap()).unwrap();
    assert_eq!(portable, action);
    for offset in [0, 5000] {
        let replica = junction_game(offset);
        replica
            .globals()
            .set(
                "ACTION",
                lua_value(&replica, &action_to_lua(&portable).unwrap()),
            )
            .unwrap();
        let ids: Vec<u32> = replica
            .load(
                r#"
            HOOK.batch = {ACTION} UPDATE({},STATE,0.2)
            assert(#SENT==1, table.concat(HOOK.logged,'\n'))
            local s = SENT[1].proposal.streetProposal
            local c = s.nodeConfigsToAdd[1].comp
            assert(c.trafficLightPreference==1 and c.userModifiedTrafficLightStates)
            assert(c.laneConnections[2].withTram)
            assert(c.trafficLightConfig.states[1].duration==12.375)
            assert(c.trafficLightConfig.states[1].minDuration==4.125)
            assert(c.trafficLightConfig.states[1].lockedLanes[2]==2)
            return {s.nodeConfigsToAdd[1].entity,s.nodeConfigsToRemove[1],
                c.laneConnections[1].segment0,c.laneConnections[1].segment1,
                c.crosswalks[2],c.trafficLightConfig.trafficLightType}
        "#,
            )
            .eval()
            .unwrap();
        assert_eq!(ids, [1, 1, 101, 102, 103, 40].map(|n| n + offset));
    }
}

/// A junction's settings name its node and edges by places in millimetres;
/// in another game a node may lie a fraction of a millimetre off, across a
/// metre line from the place named (the replay looks for edges in the
/// metre squares around it): still found, the same settings built.
#[test]
fn junction_edges_are_found_across_a_metre_line() {
    use tpf3mp_proto::{
        action::Action,
        lua::{action_from_lua, action_to_lua},
    };
    let source = junction_game(0);
    let captured: mlua::Value = source
        .load("return C.windowBuild(nil,PROPOSAL)")
        .eval()
        .unwrap();
    let action = action_from_lua(&common::tree(&captured)).unwrap();
    let portable = Action::from_payload(&action.to_payload().unwrap()).unwrap();
    for shift in [0.0, -0.0003, 0.0003] {
        let replica = junction_game(0);
        replica.globals().set("SHIFT", shift).unwrap();
        // Every node moved: the junction at the origin into the squares
        // below or above, its arms' ends with it.
        replica
            .load("for _, p in pairs(NODES) do p.x = p.x + SHIFT p.y = p.y + SHIFT end")
            .exec()
            .unwrap();
        replica
            .globals()
            .set(
                "ACTION",
                lua_value(&replica, &action_to_lua(&portable).unwrap()),
            )
            .unwrap();
        let segments: Vec<u32> = replica
            .load(
                r#"
            HOOK.batch = {ACTION} UPDATE({},STATE,0.2)
            assert(#SENT==1, table.concat(HOOK.logged,'\n'))
            local c = SENT[1].proposal.streetProposal.nodeConfigsToAdd[1].comp
            return {c.laneConnections[1].segment0, c.laneConnections[1].segment1, c.crosswalks[1], c.crosswalks[2]}
        "#,
            )
            .eval()
            .unwrap_or_else(|error| panic!("{shift}: {error}"));
        assert_eq!(segments, [101, 102, 101, 103], "shift {shift}");
    }
}

#[test]
fn a_native_junction_click_overrides_an_older_tools_preview() {
    let lua = junction_game(0);
    lua.load(format!(r#"
        HOOK.room=true HOOK.clicks=0 SCRIPT.guiUpdate({{}},nil,nil)
        SCRIPT.guiHandleEvent({{}},nil,nil,'','constructionBuilder','builder.proposalCreate',{{{CONSTRUCTION_PROPOSAL}}})
        PROPOSAL.junctionEdit=true HOOK.built[0]={{proposal=PROPOSAL}}
        HOOK.clicks=1 SCRIPT.guiUpdate({{}},nil,nil)
        assert(#HOOK.commands==1,table.concat(HOOK.logged,'\n'))
    "#)).exec().unwrap();
    let sent: mlua::Value = lua.load("return HOOK.commands[1]").eval().unwrap();
    // The fake hook stores action tables unchanged; never replay the old building.
    assert!(
        common::tree(&sent).get("EditJunctions").is_some(),
        "{sent:?}"
    );
}

#[test]
fn junction_checkpoint_rows_ignore_entity_and_connection_order_but_detect_settings() {
    let a = junction_game(0);
    let b = junction_game(5000);
    let before = read_lanes(&a)[0].clone();
    assert_ne!(before.1, "err");
    assert_eq!(before, read_lanes(&b)[0]);
    b.load(
        r#"
        local c=CONFIGS[5001]
        c.laneConnections[1],c.laneConnections[2]=c.laneConnections[2],c.laneConnections[1]
        c.crosswalks[1],c.crosswalks[2]=c.crosswalks[2],c.crosswalks[1]
        c.trafficLightConfig.states[1].lockedLanes={1,3}
        c.trafficLightConfig.states[2].lockedLanes={0,2}
    "#,
    )
    .exec()
    .unwrap();
    assert_eq!(before, read_lanes(&b)[0]);
    for mutation in [
        "CONFIGS[1].trafficLightPreference=2",
        "CONFIGS[1].laneConnections[1].withTram=true",
        "CONFIGS[1].crosswalks[1]=104",
        "CONFIGS[1].trafficLightConfig.states[1].duration=13",
        "EDGES[101].laneConfigs[1].transportModes[2]=false",
    ] {
        let changed = junction_game(0);
        changed.load(mutation).exec().unwrap();
        let after = read_lanes(&changed)[0].clone();
        assert_ne!(after.1, "err", "{mutation}");
        assert_ne!(before, after, "{mutation}");
    }
}

#[test]
fn checkpoint_reads_each_edge_and_node_adjacency_once_and_refreshes_next_time() {
    let game = junction_game(0);
    game.load(
        r#"
        COUNTS = { edges = {}, adjacency = {}, configs = {}, maps = {} }
        local CT, get = api.type.ComponentType, api.engine.getComponent
        api.engine.getComponent = function(id, kind)
            local value = get(id, kind)
            if kind == CT.BASE_EDGE then
                COUNTS.edges[id] = (COUNTS.edges[id] or 0) + 1
                if value then
                    return setmetatable({}, { __index = function(_, key)
                        if key == 'laneConfigs' then
                            COUNTS.configs[id] = (COUNTS.configs[id] or 0) + 1
                        end
                        return value[key]
                    end })
                end
            end
            return value
        end
        local system = api.engine.system.streetSystem
        for _, kind in ipairs({'Street', 'Track'}) do
            local mapName = 'getNode2' .. kind .. 'EdgeMap'
            local map = system[mapName]
            system[mapName] = function()
                COUNTS.maps[kind] = (COUNTS.maps[kind] or 0) + 1
                return map()
            end
            local name = 'getNode' .. kind .. 'Segments'
            local original = system[name]
            system[name] = function(id)
                local key = kind .. id
                COUNTS.adjacency[key] = (COUNTS.adjacency[key] or 0) + 1
                return original(id)
            end
        end
    "#,
    )
    .exec()
    .unwrap();
    let before = read_lanes(&game)[0].clone();
    assert_ne!(before.1, "err");
    for round in 1..=2 {
        game.globals().set("ROUND", round).unwrap();
        game.load(
            r#"
            for _, group in pairs(COUNTS) do
                for key, count in pairs(group) do
                    assert(count == ROUND, tostring(key) .. ': ' .. count .. ' reads')
                end
            end
            assert(COUNTS.edges[101] == ROUND and COUNTS.edges[104] == ROUND)
            assert(next(COUNTS.adjacency) == nil, 'adjacency must come from the complete maps')
            assert(COUNTS.maps.Street == ROUND and COUNTS.maps.Track == ROUND)
            assert(COUNTS.configs[101] == ROUND)
        "#,
        )
        .exec()
        .unwrap();
        if round == 1 {
            game.load("CONFIGS[1].trafficLightPreference = 2; NODES[2].x = 103")
                .exec()
                .unwrap();
            let after = read_lanes(&game)[0].clone();
            assert_ne!(after.1, "err");
            assert_ne!(before, after, "the next checkpoint must see edits");
        }
    }
    game.load("EDGES[101] = nil").exec().unwrap();
    assert_eq!(
        read_lanes(&game)[0].1,
        "err",
        "missing junction edges still fail closed"
    );
}

#[test]
fn missing_diagnostic_clock_does_not_break_checkpoints() {
    let game = junction_game(0);
    let before = read_lanes(&game);
    game.load("os = nil").exec().unwrap();
    assert_eq!(before, read_lanes(&game));
}

#[test]
fn spatial_network_covers_full_rows_without_global_maps_or_entity_id_dependence() {
    let mut replicas = Vec::new();
    for offset in [0, 5000] {
        let game = junction_game(offset);
        let baseline = read_lanes(&game)[0].1.clone();
        let actual: String = game
            .load(
                r#"
            local lanes = ug_require('tpf3mp_1::/scripts/tpf3mp/lanes.lua')
            api.engine.system.octreeSystem = {
                findIntersectingEntities = function(box, visit)
                    for e in pairs(EDGES) do visit(e) visit(e) end
                end,
            }
            local street = api.engine.system.streetSystem
            street.getNode2SegmentMap = function() error('a spatial read enumerated the world') end
            street.getNode2StreetEdgeMap = street.getNode2SegmentMap
            street.getNode2TrackEdgeMap = street.getNode2SegmentMap
            return lanes.spatial(api, {})[0]
        "#,
            )
            .eval()
            .unwrap();
        assert_ne!(actual, "err");
        assert_eq!(
            baseline, actual,
            "same coverage, duplicate query results deduplicated"
        );
        let edited: String = game
            .load("CONFIGS[1+OFFSET].doubleSlipSwitch=true; return ug_require('tpf3mp_1::/scripts/tpf3mp/lanes.lua').spatial(api,{})[0]")
            .eval()
            .unwrap();
        assert_ne!(actual, edited, "spatial reads see fresh junction edits");
        replicas.push(actual);
    }
    assert_eq!(replicas[0], replicas[1]);
}

#[test]
fn native_checkpoint_snapshots_and_fallback_preserve_hashes_and_refresh() {
    let game = junction_game(0);
    let before = read_lanes(&game);
    game.load(r#"
        rawget = nil -- the game script sandbox omits this standard global
        api.type.BaseEdge = { new = function(edge) return edge end }
        api.type.BaseNodeConfig = { new = function(node) return node end }
        local function rows(edge, reversed)
            local out = {}
            for _, l in ipairs(edge.laneConfigs) do
                local modes = {}
                for m = 0,15 do modes[m+1] = l.transportModes[m] == true and '1' or '0' end
                out[#out+1] = string.format('%.3f/%.3f/%.3f/%.3f/%s/%s',l.speed,l.width,l.height,
                    l.offset * (reversed and -1 or 1),tostring(l.forward ~= reversed),table.concat(modes))
            end
            table.sort(out)
            return table.concat(out,';'), #edge.laneConfigs
        end
        SNAPSHOTS = { lanes = 0, junctions = 0 }
        tpf3mp_native.laneRows = function(edge,reversed)
            SNAPSHOTS.lanes = SNAPSHOTS.lanes+1
            return rows(edge,reversed)
        end
        tpf3mp_native.junctionConfig = function(c)
            SNAPSHOTS.junctions = SNAPSHOTS.junctions+1
            return c
        end
    "#).exec().unwrap();
    assert_eq!(before, read_lanes(&game));
    game.load("assert(SNAPSHOTS.lanes > 0 and SNAPSHOTS.junctions > 0); CONFIGS[1].trafficLightPreference = 2")
        .exec().unwrap();
    let changed = read_lanes(&game);
    assert_ne!(before[0], changed[0]);
    game.load("tpf3mp_native.laneRows = function() return nil end; tpf3mp_native.junctionConfig = function() return nil end")
        .exec().unwrap();
    assert_eq!(changed, read_lanes(&game));
}

/// Where the hook decodes junctions, every reader names a phase's
/// crosswalks by the owned copy's order, which copying can change: the
/// rows read with the hook's decoder and those read here without it
/// (`junctions.decoders = false`, compare mode's reference) are the same,
/// and both differ from a read of the borrowed original.
#[test]
fn junction_rows_read_the_owned_copy_with_or_without_the_hooks_decoder() {
    let game = junction_game(0);
    let rows: Vec<String> = game
        .load(
            r#"
        local J = ug_require('tpf3mp_1::/scripts/tpf3mp/junctions.lua')
        local borrowed = table.concat(J.rows(api), "\n")
        -- A copy lays the crosswalk set out anew: here, reversed.
        api.type.BaseNodeConfig = { new = function(c)
            local copy = {}
            for k, v in pairs(c) do copy[k] = v end
            local walks = {}
            for i = #c.crosswalks, 1, -1 do walks[#walks + 1] = c.crosswalks[i] end
            copy.crosswalks = walks
            return copy
        end }
        DECODED = 0
        tpf3mp_native.junctionConfig = function(c) DECODED = DECODED + 1 return c end
        local decoded = table.concat(J.rows(api), "\n")
        assert(DECODED > 0)
        J.decoders = false
        local own = table.concat(J.rows(api), "\n")
        J.decoders = nil
        -- Compare mode's reference of the whole network lane, as one part,
        -- is the lane the hook's decoders read, row for row.
        local L = ug_require('tpf3mp_1::/scripts/tpf3mp/lanes.lua')
        local ref = L.partRows(api, 1, 0, 'all')[L.NETWORK]
        table.sort(ref)
        local text = #ref .. ":" .. L.hash(table.concat(ref, "\30"))
        assert(J.decoders == nil and L.decoders == nil, "the switches are as before")
        J.decoders = false
        L.partRows(api, 1, 0, 'all')
        assert(J.decoders == false, "a switch set before stays set")
        J.decoders = nil
        -- A decoder that reads nothing leaves the rows to the same copy.
        tpf3mp_native.junctionConfig = function() return nil end
        local undecoded = table.concat(J.rows(api), "\n")
        return { borrowed, decoded, own, text, L.read(api)[L.NETWORK], undecoded }
    "#,
        )
        .eval()
        .unwrap();
    assert_eq!(rows[1], rows[2], "the reference reads the same copy");
    assert_ne!(rows[0], rows[1], "the copy's order shows in the rows");
    assert_eq!(
        rows[3], rows[4],
        "compare mode's reference is the lane read"
    );
    assert_eq!(rows[5], rows[1], "nothing decoded, the copy still counts");
}

#[test]
fn every_junction_in_one_checkpoint_keeps_its_own_light_settings() {
    // One read names each light preference and resource once for all the
    // junctions after it; a second junction must still read as its own.
    let game = junction_game(0);
    let rows: Vec<String> = game
        .load(
            r#"
            CONFIGS[2] = {laneConnections={}, crosswalks={}, trafficLightPreference=2,
                trafficLightConfig={trafficLightType=-1, states={}},
                doubleSlipSwitch=false, userModifiedTrafficLightStates=false}
            CONFIGS[3] = {laneConnections={}, crosswalks={}, trafficLightPreference=1,
                trafficLightConfig={trafficLightType=40, states={}},
                doubleSlipSwitch=false, userModifiedTrafficLightStates=false}
            return J.rows(api)
        "#,
        )
        .eval()
        .unwrap();
    let settings: Vec<String> = rows
        .iter()
        .map(|row| row.split('|').skip(2).take(2).collect::<Vec<_>>().join("|"))
        .collect();
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(
        settings
            .iter()
            .filter(|s| *s == "Yes|::/traffic_light/standard.lua")
            .count(),
        2,
        "{rows:?}"
    );
    assert_eq!(
        settings.iter().filter(|s| *s == "No|default").count(),
        1,
        "{rows:?}"
    );
}

#[test]
fn junction_replay_refuses_ambiguous_stale_private_and_mixed_edits() {
    for (mutation, expected) in [
        (
            "EDGES[999]=EDGES[101] STREETS[1][5]=999",
            "ambiguous junction edge",
        ),
        ("EDGES[101].laneConfigs={}", "junction's lanes changed"),
        ("NODES[2].z=1", "road or track changed"),
        (
            "ACTION.EditJunctions.changes[1].config.light='missing'",
            "missing traffic light resource",
        ),
        (
            "ACTION.EditJunctions.changes[1].config.phases[1].locked={99}",
            "invalid locked lane",
        ),
    ] {
        let lua = junction_game(0);
        lua.load("ACTION=C.junction(PROPOSAL)").exec().unwrap();
        lua.load(mutation).exec().unwrap();
        let (ok, why): (bool, String) = lua.load("return A.run(ACTION)").eval().unwrap();
        assert!(!ok && why.contains(expected), "{mutation}: {why}");
        assert_eq!(lua.load("return #SENT").eval::<usize>().unwrap(), 0);
    }
    let lua = junction_game(0);
    lua.load(r#"
        local changes=C.junction(PROPOSAL).EditJunctions.changes
        local ok,why=pcall(J.into,api.type.SimpleProposal.new(),changes,{},function() error('private edge') end)
        assert(not ok and tostring(why):find('private edge',1,true))
        PROPOSAL.proposal.addedSegments={{entity=-1}}
        ok,why=pcall(C.junction,PROPOSAL)
        assert(not ok and tostring(why):find('geometry',1,true))
        PROPOSAL.proposal.addedSegments=nil PROPOSAL.proposal.nodeConfigsToAdd={}
        local reset=C.junction(PROPOSAL)
        assert(reset.EditJunctions.changes[1].config==nil)
        assert(A.run(reset))
        local s=SENT[1].proposal.streetProposal
        assert(#s.nodeConfigsToRemove==1 and not s.nodeConfigsToAdd)
    "#).exec().unwrap();
}

/// The junction of `junction_world.lua` with no traffic lights (unless
/// `KEEP_LIGHTS`) and the preference Auto, its four arms tracks with no
/// street at the node (unless `STREET_NODE`), `strict_junctions` off, and
/// `EDIT(f)`: the capture of the window's proposal of its configuration as
/// `f` changes a copy of it. The stand-in's enum: AUTO=0, YES=1, NO=2.
const NARROW_BASE: &str = r#"
    local c = CONFIGS[1+OFFSET]
    if not KEEP_LIGHTS then
        c.trafficLightConfig = {trafficLightType=-1, states={}}
        c.userModifiedTrafficLightStates = false
    end
    c.trafficLightPreference = 0
    if not STREET_NODE then
        local system = api.engine.system.streetSystem
        system.getNode2TrackEdgeMap = function() return STREETS end
        system.getNode2StreetEdgeMap = function() return {} end
        system.getNodeTrackSegments = function(n) return STREETS[n] or {} end
        system.getNodeStreetSegments = function() return {} end
    end
    J.strict_junctions = false
    -- Imported upstream exception vectors run only in an explicit acceptance copy.
    J.double_slip_only = true
    local function copy(v)
        if type(v) ~= 'table' then return v end
        local o = {}
        for k, x in pairs(v) do o[k] = copy(x) end
        return o
    end
    function EDIT(f)
        local e = copy(CONFIGS[1+OFFSET])
        f(e)
        PROPOSAL.proposal.nodeConfigsToAdd = {{entity=1+OFFSET, comp=e}}
        return C.junction(PROPOSAL)
    end
"#;

fn narrow_game(offset: u32, keep_lights: bool, street_node: bool) -> Lua {
    let lua = junction_game(offset);
    lua.globals().set("KEEP_LIGHTS", keep_lights).unwrap();
    lua.globals().set("STREET_NODE", street_node).unwrap();
    lua.load(NARROW_BASE).exec().unwrap();
    lua
}

/// Captures `edit` in one game and replays it in another with other ids,
/// both set up by `setup`; returns whether `check` holds for the
/// configuration the replica sent.
fn narrow_round_trip(keep_lights: bool, setup: &str, edit: &str, check: &str) -> bool {
    use tpf3mp_proto::lua::{action_from_lua, action_to_lua};
    let source = narrow_game(0, keep_lights, false);
    source.load(setup).exec().unwrap();
    let captured: mlua::Value = source
        .load(format!("return EDIT(function(c) {edit} end)"))
        .eval()
        .unwrap_or_else(|e| panic!("{edit}: {e}"));
    let action = action_from_lua(&common::tree(&captured)).unwrap();
    let replica = narrow_game(5000, keep_lights, false);
    replica.load(setup).exec().unwrap();
    replica
        .globals()
        .set(
            "ACTION",
            lua_value(&replica, &action_to_lua(&action).unwrap()),
        )
        .unwrap();
    replica
        .load(format!(
            r#"
        HOOK.batch = {{ACTION}} UPDATE({{}},STATE,0.2)
        assert(#SENT==1, table.concat(HOOK.logged,'\n'))
        local cfg = SENT[1].proposal.streetProposal.nodeConfigsToAdd[1].comp
        return ({check}) == true
    "#
        ))
        .eval()
        .unwrap_or_else(|e| panic!("{edit}: {e}"))
}

#[test]
fn the_upstream_double_slip_exception_is_off_in_ordinary_reedme_rooms() {
    let lua = narrow_game(0, false, false);
    lua.load(
        r#"
        J.double_slip_only = nil
        local ok, why = pcall(EDIT, function(c) c.doubleSlipSwitch = true end)
        assert(not ok and tostring(why):find('strict_junctions', 1, true))
        assert(#SENT == 0)
        "#,
    )
    .exec()
    .unwrap();
}

#[test]
fn an_explicit_acceptance_copy_carries_the_upstream_double_slip_vectors() {
    // As the window was seen to send it: the switch, and with it the
    // preference from Auto to No at a node no street joins.
    for (setup, edit, check) in [
        ("", "c.doubleSlipSwitch=true", "cfg.doubleSlipSwitch"),
        (
            "",
            "c.doubleSlipSwitch=true c.trafficLightPreference=2",
            "cfg.doubleSlipSwitch and cfg.trafficLightPreference==2",
        ),
        (
            "CONFIGS[1+OFFSET].doubleSlipSwitch=true",
            "c.doubleSlipSwitch=false",
            "not cfg.doubleSlipSwitch",
        ),
    ] {
        assert!(narrow_round_trip(false, setup, edit, check), "{edit}");
    }
    // A phase kept as it is compares alike after the wire rounded it to
    // milliseconds (12.3125 s: half a millisecond).
    assert!(narrow_round_trip(
        true,
        "CONFIGS[1+OFFSET].trafficLightConfig.states[1].duration = 12.3125",
        "c.doubleSlipSwitch=true",
        "cfg.doubleSlipSwitch",
    ));
}

#[test]
fn every_other_junction_edit_stays_refused_before_the_junction_acceptance() {
    use tpf3mp_proto::lua::{action_from_lua, action_to_lua};
    let slip = "c.doubleSlipSwitch=true";
    for (keep_lights, street_node, setup, edit, expected) in [
        // Lanes stay exactly as they are, the switch's own included, and
        // tram lanes wait for the crossing tool's own acceptance.
        (false, false, "", format!("{slip} c.laneConnections[3]={{segment0=101,lane0=1,segment1=103,lane1=0,withRoad=false,withTram=false}}"), "lane connections"),
        (false, false, "", format!("{slip} table.remove(c.laneConnections,2)"), "lane connections"),
        (false, false, "", format!("{slip} c.laneConnections[2].withTram=false"), "lane connections"),
        (false, false, "", "c.laneConnections[3]={segment0=102,lane0=1,segment1=103,lane1=0,withRoad=false,withTram=true}".to_string(), "no double slip switch change"),
        (false, false, "", "c.laneConnections[2].withTram=false".to_string(), "no double slip switch change"),
        // The preference: Auto to No alone, with the switch, where no
        // street joins.
        (false, false, "", "c.trafficLightPreference=2".to_string(), "no double slip switch change"),
        (false, false, "", format!("{slip} c.trafficLightPreference=1"), "traffic lights"),
        (false, false, "CONFIGS[1+OFFSET].trafficLightPreference=2", format!("{slip} c.trafficLightPreference=0"), "traffic lights"),
        (false, true, "", format!("{slip} c.trafficLightPreference=2"), "traffic lights"),
        // Everything else at the node.
        (false, false, "", format!("{slip} c.crosswalks={{101}}"), "crosswalks"),
        (false, false, "", format!("{slip} c.trafficLightConfig.trafficLightType=40"), "traffic lights"),
        (false, false, "", format!("{slip} c.userModifiedTrafficLightStates=true"), "traffic lights"),
        (true, false, "", format!("{slip} c.trafficLightConfig.states[1].duration=13"), "phases"),
        (false, false, "", "c.doubleSlipSwitch=false".to_string(), "no double slip switch change"),
    ] {
        let lua = narrow_game(0, keep_lights, street_node);
        lua.load(setup).exec().unwrap();
        let (ok, why): (bool, String) = lua
            .load(format!(
                "local ok, why = pcall(EDIT, function(c) {edit} end) return ok, tostring(why)"
            ))
            .eval()
            .unwrap();
        assert!(
            !ok && why.contains("strict_junctions") && why.contains(expected),
            "{edit}: {why}"
        );
    }
    // A reset.
    let lua = narrow_game(0, false, false);
    lua.load(
        r#"
        PROPOSAL.proposal.nodeConfigsToAdd = {}
        local ok, why = pcall(C.junction, PROPOSAL)
        assert(not ok and tostring(why):find('reset',1,true), tostring(why))
    "#,
    )
    .exec()
    .unwrap();
    // Every game asks again at the replay: an edit captured where the gate
    // was on changes nothing in a game where it is off.
    let source = narrow_game(0, false, false);
    let captured: mlua::Value = source
        .load("J.strict_junctions=true return EDIT(function(c) c.doubleSlipSwitch=true c.crosswalks={101} end)")
        .eval()
        .unwrap();
    let action = action_from_lua(&common::tree(&captured)).unwrap();
    let replica = narrow_game(5000, false, false);
    replica
        .globals()
        .set(
            "ACTION",
            lua_value(&replica, &action_to_lua(&action).unwrap()),
        )
        .unwrap();
    let (ok, why): (bool, String) = replica
        .load("local ok, why = A.run(ACTION) return ok == true, tostring(why)")
        .eval()
        .unwrap();
    assert!(!ok && why.contains("crosswalks"), "{why}");
    assert_eq!(replica.load("return #SENT").eval::<usize>().unwrap(), 0);
}

#[test]
fn unverified_junction_tools_are_refused_on_capture_and_replay() {
    let lua = junction_game(0);
    lua.load(
        r#"
        local action=C.junction(PROPOSAL)
        J.strict_junctions=false
        local ok,why=pcall(C.junction,PROPOSAL)
        assert(not ok and tostring(why):find('strict_junctions',1,true))
        ok,why=A.run(action)
        assert(not ok and tostring(why):find('strict_junctions',1,true))
        assert(#SENT==0)
    "#,
    )
    .exec()
    .unwrap();
}

#[test]
fn rebuilding_a_curved_road_preserves_turns_crosswalks_and_light_phases() {
    let lua = junction_game(0);
    lua.load(
        r#"
        -- Split a curve whose chord changes but its tangent at the junction doesn't.
        EDGES[101].tangent0={x=80,y=40,z=0}
        local e={entity=-2,type=0,comp={node0=1,node1=-1,tangent0={x=40,y=20,z=0},
            tangent1={x=40,y=0,z=0},laneConfigs=EDGES[101].laneConfigs}}
        local p=api.type.SimpleProposal.new()
        p.streetProposal={nodesToAdd={{entity=-1,comp={position={x=45,y=20,z=0}}}},
            edgesToAdd={e},edgesToRemove={101}}
        J.into(p,{}, {1})
        local c=p.streetProposal.nodeConfigsToAdd[1].comp
        assert(c.laneConnections[1].segment0==-2)
        assert(c.crosswalks[1]==-2 and c.crosswalks[2]==103)
        assert(c.trafficLightConfig.states[1].lockedLanes[2]==2)
        assert(c.trafficLightConfig.states[1].duration==12.375)
        -- Adding a second possible replacement must refuse the whole proposal.
        p.streetProposal.edgesToAdd[2]={entity=-3,type=0,comp=e.comp}
        local ok,why=pcall(J.into,p,{}, {1})
        assert(not ok and tostring(why):find('ambiguous replacement',1,true))
    "#,
    )
    .exec()
    .unwrap();
}

/// A small world for the lanes, over the stand-in engine state: two edges,
/// two constructions, a line, two vehicles, a player, a town and people.
const FAKE_WORLD: &str = r#"
local CT = { BASE_EDGE = 1, CONSTRUCTION = 2, LINE = 3, TRANSPORT_VEHICLE = 4, PLAYER = 5,
             ACCOUNT = 6, TOWN = 7, SIM_PERSON = 8, MOVE_PATH = 9 }
WORLD = {
    [CT.BASE_EDGE] = {
        [101] = { position0 = { x = 0, y = 0, z = 0 }, position1 = { x = 100.04, y = 0, z = 1 },
                  roadTemplate = 'street/country.lua', laneConfigs = {} },
        [102] = { position0 = { x = 100, y = 0, z = 1 }, position1 = { x = 100, y = 80, z = 2 },
                  roadTemplate = 'street/country.lua', laneConfigs = {} },
    },
    [CT.CONSTRUCTION] = {
        [201] = { fileName = 'depot/road_depot.con', transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 600,0,0.45,1 } },
        [202] = { fileName = 'station/bus_stop.con', transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 40,8,0,1 } },
    },
    [CT.LINE] = { [301] = { stops = { {}, {} } } },
    [CT.TRANSPORT_VEHICLE] = { [401] = { state = 1, stopIndex = 0 }, [402] = { state = 2, stopIndex = 1 } },
    -- The simulation's path state, and the state as a frame began, which
    -- is each game's own.
    [CT.MOVE_PATH] = {
        [401] = { dyn = { pathPos = { edgeIndex = 3, pos = 10.2 }, speed = 5 },
                  dyn0 = { pathPos = { edgeIndex = 3, pos = 9.7 }, speed = 5 } },
        [402] = { dyn = { pathPos = { edgeIndex = 0, pos = 0 }, speed = 0 } },
    },
    [CT.PLAYER] = { [25] = true },
    [CT.ACCOUNT] = { [25] = { balance = 1234567 } },
    [CT.TOWN] = { [7] = true },
    [CT.SIM_PERSON] = { [801] = true, [802] = true, [803] = true },
}
REVERSED = false
api.type.ComponentType = CT
api.engine.getEntitiesWithComponent = function(kind)
    -- As the game: some components cannot be listed.
    if kind == CT.BASE_EDGE or kind == CT.LINE or kind == CT.PLAYER then
        error('Cannot loop over this component type')
    end
    local list = {}
    for e in pairs(WORLD[kind] or {}) do list[#list + 1] = e end
    table.sort(list, function(a, b) if REVERSED then return a > b end return a < b end)
    return list
end
api.engine.getComponent = function(e, kind)
    local c = (WORLD[kind] or {})[e]
    if c == true then return {} end
    return c
end
local function sorted(kind)
    local list = {}
    for e in pairs(WORLD[kind]) do list[#list + 1] = e end
    table.sort(list, function(a, b) if REVERSED then return a > b end return a < b end)
    return list
end
api.engine.system = {
    townBuildingSystem = { getTown2BuildingMap = function()
        return { [7] = { 901, 902, 903 } }
    end },
    -- Each edge under both its nodes, as the street system lists them.
    streetSystem = { getNode2StreetEdgeMap = function() return {} end,
        getNode2TrackEdgeMap = function() return {} end, getNode2SegmentMap = function()
        local edges = sorted(CT.BASE_EDGE)
        return { [11] = { edges[1] }, [12] = edges, [13] = { edges[#edges] } }
    end },
    lineSystem = { getLines = function() return sorted(CT.LINE) end },
}
"#;

/// The lanes the mod reads in the stand-in world.
fn read_lanes(lua: &Lua) -> Vec<(u16, String)> {
    let lanes: Table = lua
        .load("return ug_require('tpf3mp_1::/scripts/tpf3mp/lanes.lua').read(api)")
        .eval()
        .unwrap();
    let mut out: Vec<(u16, String)> = lanes
        .pairs::<u16, String>()
        .map(|pair| pair.unwrap())
        .collect();
    out.sort();
    out
}

const FAKE_SPATIAL: &str = r#"
api.type.Vec3f = { new = function(x,y,z) return {x=x,y=y,z=z} end }
api.type.Box3 = { new = function(a,b) return {min=a,max=b} end }
api.engine.terrain = { getBoundingBox = function()
    return {min={x=0,y=0},max={x=1024,y=1024}}
end }
WORLD[1][101].node0, WORLD[1][101].node1 = 11,12
WORLD[1][102].node0, WORLD[1][102].node1 = 12,13
api.engine.system.octreeSystem = { findIntersectingEntities = function(box, visit)
    for kind, entries in pairs(WORLD) do
        if kind == 1 or kind == 2 then
            for e,c in pairs(entries) do
                local a,b
                if kind == 1 then a,b=c.position0,c.position1
                else a={x=c.transf[13],y=c.transf[14]} b=a end
                if math.max(a.x,b.x) >= box.min.x and math.min(a.x,b.x) <= box.max.x
                    and math.max(a.y,b.y) >= box.min.y and math.min(a.y,b.y) <= box.max.y then
                    visit(e)
                end
            end
        end
    end
end }
ROLL = ug_require('tpf3mp_1::/scripts/tpf3mp/lanes.lua')
ROLL_STEP = function(step, checkpoint)
    local out
    SCAN, out = ROLL.rolling(api, SCAN, step, checkpoint)
    return out
end
"#;

fn rolling_game() -> Lua {
    let (lua, _) = engine();
    lua.load(FAKE_WORLD).exec().unwrap();
    lua.load(FAKE_SPATIAL).exec().unwrap();
    lua
}

#[test]
fn rolling_checks_resume_from_saved_history_and_refuse_gaps_and_read_failures() {
    let a = rolling_game();
    let b = rolling_game();
    a.load("for step=1,7 do ROLL_STEP(step,false) end")
        .exec()
        .unwrap();
    let saved: mlua::Value = a.globals().get("SCAN").unwrap();
    b.globals()
        .set("SCAN", lua_value(&b, &common::tree(&saved)))
        .unwrap();
    let read = |lua: &Lua| -> Vec<String> {
        lua.load(
            "local r=ROLL_STEP(8,true) local out={} for n=0,6 do out[#out+1]=r[n] end return out",
        )
        .eval()
        .unwrap()
    };
    assert_eq!(
        read(&a),
        read(&b),
        "a fresh Lua state resumes a mid-window save"
    );
    assert!(
        b.load("ROLL_STEP(10,false)").exec().is_err(),
        "skipped update"
    );
    assert!(
        b.load("ROLL_STEP(8,false)").exec().is_err(),
        "repeated update"
    );
    assert!(
        b.load("SCAN=nil ROLL_STEP(9,false)").exec().is_err(),
        "missing saved history"
    );
    b.load("ROLL_STEP(1,false)").exec().unwrap();
    assert!(
        b.load("api.engine.system.octreeSystem=nil ROLL_STEP(2,false)")
            .exec()
            .is_err()
    );
}

/// A hook that reads parts (`tpf3mp_native.part`), faked: it records what
/// it was asked, answers with `NATIVE_TEXT`, fails with `PART_FAIL`, and
/// compares with `PART_MODE = 'compare'`.
const FAKE_PARTS: &str = r#"
api.type.enum = api.type.enum or {}
api.type.enum.TrafficLightPreference = api.type.enum.TrafficLightPreference or { AUTO = 0, YES = 1, NO = 2 }
PART_CALLS, PART_TEXTS = {}, {}
NATIVE_TEXT = 'native'
tpf3mp_native.part = function(n, k, kind)
    if n == nil then return { mode = PART_MODE or 'on', parts = PLAN_PARTS or 10, stride = PLAN_STRIDE or 1 } end
    PART_CALLS[#PART_CALLS + 1] = n .. '/' .. k .. '/' .. kind
    if PART_FAIL then return { mode = 'on', why = PART_FAIL } end
    -- A hook from before it read the crossings hands no crossingTypes.
    if PART_OLD then return { mode = PART_MODE or 'on', ms = 1, timing = 'fake', lights = {}, deferred = {} } end
    return { mode = PART_MODE or 'on', ms = 1, timing = 'fake', lights = {}, deferred = {},
             crossingTypes = PART_CROSSINGS or {} }
end
tpf3mp_native.partTexts = function(n, k, kind, preferences, lights, deferred, rows, crossingTypes)
    PART_TEXTS[#PART_TEXTS + 1] = n .. '/' .. k .. '/' .. kind
    if PART_TEXTS_FAIL then return nil, nil, PART_TEXTS_FAIL end
    local net, cons = '1:' .. NATIVE_TEXT, '1:cons'
    if kind == 'constructions' then net = nil else cons = nil end
    -- The crossings' types as the Lua named them, on a junctions turn.
    if kind == 'junctions' then
        local names = {}
        for _, index in ipairs(PART_CROSSINGS or {}) do names[#names + 1] = tostring(crossingTypes and crossingTypes[index]) end
        net = net .. '|crossings ' .. table.concat(names, ',')
    end
    if rows then return net, cons, { 'row' }, { 'con@0,0' } end
    return net, cons
end
REPORTS = {}
ROLL_STEP = function(step, checkpoint)
    local out, report
    SCAN, out, report = ROLL.rolling(api, SCAN, step, checkpoint)
    if report then REPORTS[#REPORTS + 1] = report end
    return out
end
"#;

/// Where the hook reads parts, a room's rolling history starts in parts:
/// one part an update, in turn; the hook's texts count, compared ones give
/// way to this Lua's own and a difference is said; a part that did not
/// read, or a game no longer reading parts, holds the game; a saved
/// history resumes in a fresh state; without parts it stays version 1.
#[test]
fn rolling_checks_read_native_parts_in_turn_and_fail_closed() {
    let game = |setup: &str| {
        let lua = rolling_game();
        lua.load(FAKE_PARTS).exec().unwrap();
        lua.load(setup).exec().unwrap();
        lua
    };
    let read = |lua: &Lua, from: u32, to: u32| -> Vec<String> {
        lua.load(format!(
            "for s={from},{} do ROLL_STEP(s,false) end local r=ROLL_STEP({to},true) \
             local out={{}} for n=0,6 do out[#out+1]=r[n] end return out",
            to - 1
        ))
        .eval()
        .unwrap()
    };
    let a = game("");
    let first = read(&a, 1, 12);
    // Edges and junctions are the network lane's: 8 of 12 updates.
    assert!(first[0].starts_with("rolling-v2:1-12:8:"), "{first:?}");
    assert!(first[1].starts_with("rolling-v2:1-12:4:"), "{first:?}");
    let calls: Vec<String> = a.load("return PART_CALLS").eval().unwrap();
    let kinds = ["edges", "junctions", "constructions"];
    let turns: Vec<String> = (0..12)
        .map(|s| format!("10/{}/{}", s / 3 % 10, kinds[s % 3]))
        .collect();
    assert_eq!(calls, turns);
    let texts: Vec<String> = a.load("return PART_TEXTS").eval().unwrap();
    assert_eq!(texts, turns, "texts asked for the part just read");
    // The hook's text counts: another text, another history.
    let other = read(&game("NATIVE_TEXT='other'"), 1, 12);
    assert_ne!(other[0], first[0]);
    assert_eq!(other[2..], first[2..], "the dynamic lanes are this Lua's");
    // Compared, this Lua's own text counts whatever the hook says.
    let c = game("PART_MODE='compare'");
    let d = game("PART_MODE='compare' NATIVE_TEXT='other'");
    let (rc, rd) = (read(&c, 1, 12), read(&d, 1, 12));
    assert_eq!(rc[..2], rd[..2]);
    assert_ne!(rc[0], first[0]);
    let reports: Vec<String> = d.load("return REPORTS").eval().unwrap();
    assert!(reports.iter().any(|r| r.contains("differs")), "{reports:?}");
    // A save in the middle of a window resumes in a fresh state.
    let e = game("for s=1,7 do ROLL_STEP(s,false) end");
    let f = game("");
    let saved: mlua::Value = e.globals().get("SCAN").unwrap();
    f.globals()
        .set("SCAN", lua_value(&f, &common::tree(&saved)))
        .unwrap();
    assert_eq!(read(&e, 8, 12), read(&f, 8, 12));
    // Fail closed: texts refused on a constructions turn are no text of
    // that lane.
    let j = game("for s=1,2 do ROLL_STEP(s,false) end");
    assert!(
        j.load("PART_TEXTS_FAIL='no part was read' ROLL_STEP(3,false)")
            .exec()
            .is_err()
    );
    // Fail closed: a part that did not read, a game that reads no parts.
    assert!(
        f.load("PART_FAIL='no pool' ROLL_STEP(13,false)")
            .exec()
            .is_err()
    );
    // Fail closed: a junctions turn whose hook hands no crossings' types
    // (one from before it read them) checks no crossings, so it holds.
    let old = game("ROLL_STEP(1,false)");
    assert!(
        old.load("PART_OLD=true ROLL_STEP(2,false)").exec().is_err(),
        "a part without its crossings"
    );
    let g = game("for s=1,3 do ROLL_STEP(s,false) end tpf3mp_native.part=nil");
    assert!(g.load("ROLL_STEP(4,false)").exec().is_err());
    // Every `stride` updates one part of one kind: 3 in 12 updates.
    let h = game("PLAN_PARTS=4 PLAN_STRIDE=5");
    let strided = read(&h, 1, 12);
    assert!(strided[0].starts_with("rolling-v2:1-12:2:"), "{strided:?}");
    let calls: Vec<String> = h.load("return PART_CALLS").eval().unwrap();
    assert_eq!(calls, ["4/0/edges", "4/0/junctions", "4/0/constructions"]);
    // Between two parts too, a history of parts holds once the game reads
    // none.
    let i = game(
        "PLAN_PARTS=4 PLAN_STRIDE=5 for s=1,2 do ROLL_STEP(s,false) end tpf3mp_native.part=nil",
    );
    assert!(
        i.load("ROLL_STEP(3,false)").exec().is_err(),
        "not a slot, still held"
    );
    // Without parts, the rolling history of version 1.
    assert!(read(&rolling_game(), 1, 12)[0].starts_with("rolling-v1:"));

    // A saved history cannot silently resume under a different local mode
    // or partition plan; refuse before producing another checkpoint hash.
    let changed_plan = game("ROLL_STEP(1,false) PLAN_PARTS=4 PLAN_STRIDE=1");
    let why = changed_plan
        .load("ROLL_STEP(2,true)")
        .exec()
        .unwrap_err()
        .to_string();
    assert!(
        why.contains("saved rolling-v2 plan does not match"),
        "{why}"
    );
    assert_eq!(
        changed_plan.load("return SCAN.step").eval::<u32>().unwrap(),
        1,
        "a mismatched partition must be refused before advancing saved history"
    );
    let calls: Vec<String> = changed_plan.load("return PART_CALLS").eval().unwrap();
    assert_eq!(
        calls.len(),
        1,
        "no second native part may be read: {calls:?}"
    );
    let reports: Vec<String> = changed_plan.load("return REPORTS").eval().unwrap();
    assert!(
        reports.is_empty(),
        "no checkpoint hash may be published: {reports:?}"
    );

    let changed_stride = game("ROLL_STEP(1,false) ROLL_STEP(2,false) PLAN_STRIDE=2");
    let why = changed_stride
        .load("ROLL_STEP(3,true)")
        .exec()
        .unwrap_err()
        .to_string();
    assert!(
        why.contains("saved rolling-v2 plan does not match"),
        "{why}"
    );
    assert_eq!(
        changed_stride
            .load("return SCAN.step")
            .eval::<u32>()
            .unwrap(),
        2,
        "a mismatched stride must be refused before advancing saved history"
    );
    let calls: Vec<String> = changed_stride.load("return PART_CALLS").eval().unwrap();
    assert_eq!(
        calls.len(),
        2,
        "no third native part may be read: {calls:?}"
    );

    let changed_mode = game("PART_MODE='compare' ROLL_STEP(1,false) PART_MODE='on'");
    let why = changed_mode
        .load("ROLL_STEP(2,true)")
        .exec()
        .unwrap_err()
        .to_string();
    assert!(
        why.contains("saved rolling-v2 plan does not match"),
        "{why}"
    );
    assert_eq!(
        changed_mode.load("return SCAN.step").eval::<u32>().unwrap(),
        1,
        "a mismatched mode must be refused before advancing saved history"
    );
    let calls: Vec<String> = changed_mode.load("return PART_CALLS").eval().unwrap();
    assert_eq!(
        calls.len(),
        1,
        "no second native part may be read: {calls:?}"
    );

    let native_disabled = game("ROLL_STEP(1,false) tpf3mp_native.part=nil");
    let why = native_disabled
        .load("ROLL_STEP(2,true)")
        .exec()
        .unwrap_err()
        .to_string();
    assert!(
        why.contains("saved rolling-v2 history has no native part plan"),
        "{why}"
    );
    assert_eq!(
        native_disabled
            .load("return SCAN.step")
            .eval::<u32>()
            .unwrap(),
        1,
        "missing native plan must be refused before advancing saved history"
    );

    let native_enabled = game(
        "PART_QUERY=tpf3mp_native.part tpf3mp_native.part=nil ROLL_STEP(1,false) \
         tpf3mp_native.part=PART_QUERY",
    );
    let why = native_enabled
        .load("ROLL_STEP(2,true)")
        .exec()
        .unwrap_err()
        .to_string();
    assert!(
        why.contains("saved rolling-v1 history does not match"),
        "{why}"
    );
    assert_eq!(
        native_enabled
            .load("return SCAN.step")
            .eval::<u32>()
            .unwrap(),
        1,
        "native plan appearing after a v1 save must be refused before history advances"
    );

    // The default/off rolling-v1 path records and validates the complete
    // configured plan too, including an off-to-off partition change.
    let native_off = game("PART_MODE='off' PLAN_PARTS=4 PLAN_STRIDE=2 ROLL_STEP(1,false)");
    assert_eq!(
        native_off
            .load(
                "return SCAN.version .. ':' .. SCAN.mode .. ':' .. SCAN.parts .. ':' .. SCAN.stride"
            )
            .eval::<String>()
            .unwrap(),
        "1:off:4:2"
    );
    let calls: Vec<String> = native_off.load("return PART_CALLS").eval().unwrap();
    assert!(calls.is_empty(), "off mode must keep rolling-v1: {calls:?}");
    let why = native_off
        .load("PLAN_PARTS=5 ROLL_STEP(2,true)")
        .exec()
        .unwrap_err()
        .to_string();
    assert!(
        why.contains("saved rolling-v1 history does not match"),
        "{why}"
    );
    assert_eq!(
        native_off.load("return SCAN.step").eval::<u32>().unwrap(),
        1,
        "an off-mode partition change must be refused before history advances"
    );

    let legacy_off = game(
        "PART_MODE='off' ROLL_STEP(1,false) \
         SCAN.mode=nil SCAN.parts=nil SCAN.stride=nil",
    );
    let legacy_result: bool = legacy_off
        .load("local out=ROLL_STEP(2,true); return out[0]:sub(1,11) == 'rolling-v1:'")
        .eval()
        .unwrap();
    assert!(legacy_result, "an old off-mode v1 save remains rolling-v1");
    assert_eq!(
        legacy_off
            .load("return SCAN.mode .. ':' .. SCAN.parts .. ':' .. SCAN.stride")
            .eval::<String>()
            .unwrap(),
        "off:10:1",
        "the legacy save adopts the current off plan exactly once"
    );

    let partial_off = game("PART_MODE='off' ROLL_STEP(1,false) SCAN.parts=nil");
    let why = partial_off
        .load("ROLL_STEP(2,true)")
        .exec()
        .unwrap_err()
        .to_string();
    assert!(
        why.contains("saved rolling-v1 history does not match"),
        "partial plan metadata must not be migrated: {why}"
    );
    assert_eq!(
        partial_off.load("return SCAN.step").eval::<u32>().unwrap(),
        1,
        "partial plan metadata is refused before history advances"
    );

    let off_to_on = game("PART_MODE='off' ROLL_STEP(1,false) PART_MODE='on'");
    let why = off_to_on
        .load("ROLL_STEP(2,true)")
        .exec()
        .unwrap_err()
        .to_string();
    assert!(
        why.contains("saved rolling-v1 history does not match"),
        "{why}"
    );
    assert_eq!(
        off_to_on.load("return SCAN.step").eval::<u32>().unwrap(),
        1,
        "enabling native mode after a v1 save must be refused before history advances"
    );
}

#[test]
fn rolling_checks_detect_builds_deletions_geometry_and_dynamic_changes() {
    for change in [
        "WORLD[2][201].fileName='changed.con'",
        "WORLD[2][201]=nil",
        "WORLD[2][203]={fileName='new.con',transf={1,0,0,0,0,1,0,0,0,0,1,0,610,0,0,1}}",
        "WORLD[1][101].position1.x=123",
        "WORLD[9][401].dyn.pathPos.pos=14",
        "WORLD[6][25].balance=123",
    ] {
        let a = rolling_game();
        let b = rolling_game();
        b.load(change).exec().unwrap();
        let read = |lua: &Lua| -> Vec<String> {
            lua.load("for s=1,15 do ROLL_STEP(s,false) end local r=ROLL_STEP(16,true) local out={} for n=0,6 do out[#out+1]=r[n] end return out").eval().unwrap()
        };
        assert_ne!(read(&a), read(&b), "{change}");
    }
}

#[test]
fn rolling_checks_split_dense_areas_before_serializing_and_resume_the_split_queue() {
    let a = rolling_game();
    let b = rolling_game();
    for lua in [&a, &b] {
        lua.load(
            r#"
            WORLD[1]={} WORLD[2]={}
            for n=0,99 do
                WORLD[2][1000+n]={fileName='house.con',transf={1,0,0,0,0,1,0,0,0,0,1,0,
                    20+(n%10)*100,20+math.floor(n/10)*100,0,1}}
            end
            local get=api.engine.getComponent
            ROWS=0
            api.engine.getComponent=function(e,kind)
                local c=get(e,kind)
                if kind==2 and c then
                    return setmetatable({}, {__index=function(_,key)
                        if key=='fileName' then ROWS=ROWS+1 end
                        return c[key]
                    end})
                end
                return c
            end
        "#,
        )
        .exec()
        .unwrap();
    }
    a.load("ROLL_STEP(1,false) assert(ROWS==0 and #SCAN.pending==4)")
        .exec()
        .unwrap();
    let saved: mlua::Value = a.globals().get("SCAN").unwrap();
    b.globals()
        .set("SCAN", lua_value(&b, &common::tree(&saved)))
        .unwrap();
    let sweep = |lua: &Lua| -> String {
        lua.load(
            r#"
            local step=2
            while SCAN.sweeps==0 do
                ROWS=0 ROLL_STEP(step,false)
                assert(ROWS<=32, 'too much canonicalization in one update')
                assert(step<30, 'the sweep did not complete')
                step=step+1
            end
            return ROLL_STEP(step,true)[1]
        "#,
        )
        .eval()
        .unwrap()
    };
    assert_eq!(sweep(&a), sweep(&b));
}

#[test]
fn the_game_script_runs_rolling_checks_between_checkpoints_and_saves_their_history() {
    let lua = rolling_game();
    lua.load(
        r#"
        HOOK.scanStep=1 UPDATE({},STATE,0.2)
        assert(HOOK.scanResult.ok, HOOK.scanResult.why)
        assert(HOOK.lanes==nil and STATE:get().worldCheck.step==1)
        HOOK.scanStep=2 HOOK.checkpoint=true UPDATE({},STATE,0.2)
        assert(HOOK.scanResult.ok, HOOK.scanResult.why)
        assert(HOOK.lanes[0]:find('rolling-v1:1-2:',1,true)==1)
        HOOK.scanStep=4 UPDATE({},STATE,0.2)
        assert(not HOOK.scanResult.ok and HOOK.scanResult.why:find('skipped',1,true))
    "#,
    )
    .exec()
    .unwrap();
}

/// A simulation state without `os` (or its clock) times nothing but still
/// applies the room's actions and runs its world checks.
#[test]
fn the_game_script_runs_without_a_clock() {
    for missing in ["os = nil", "os = { }"] {
        let lua = rolling_game();
        lua.load(format!(
            "{missing} HOOK.scanStep=1 HOOK.batch = {{ {{ SellVehicle = {{ vehicles = {{ 7 }} }} }} }}              UPDATE({{}},STATE,0.2) assert(HOOK.scanResult.ok, HOOK.scanResult.why)              assert(HOOK.batch == nil, 'the action was taken')"
        ))
        .exec()
        .unwrap();
    }
}

#[test]
fn lanes_sum_up_the_world_part_by_part() {
    let (lua, _) = engine();
    lua.load(FAKE_WORLD).exec().unwrap();
    let lanes = read_lanes(&lua);
    assert_eq!(
        lanes.iter().map(|(lane, _)| *lane).collect::<Vec<_>>(),
        [0, 1, 2, 3, 4, 5, 6]
    );
    assert!(lanes.iter().all(|(_, text)| text != "err"), "{lanes:?}");
    assert!(lanes[0].1.starts_with("2:"), "two edges: {}", lanes[0].1);
    assert_eq!(lanes[6].1, "3", "three people");
    // The order the engine lists entities in changes nothing.
    lua.load("REVERSED = true").exec().unwrap();
    assert_eq!(read_lanes(&lua), lanes);
    // The state a frame began with, each game's own, changes nothing; a
    // vehicle 2 cm on along its path changes the vehicles' lane alone.
    lua.load("WORLD[9][401].dyn0.pathPos.pos = 10.1")
        .exec()
        .unwrap();
    assert_eq!(read_lanes(&lua), lanes);
    lua.load("WORLD[9][401].dyn.pathPos.pos = 10.22")
        .exec()
        .unwrap();
    let moved = read_lanes(&lua);
    for (before, after) in lanes.iter().zip(&moved) {
        assert_eq!(before.0 == 3, before.1 != after.1, "lane {}", before.0);
    }
    // Money spent changes the economy's lane.
    lua.load("WORLD[6][25].balance = 1234000").exec().unwrap();
    assert_ne!(read_lanes(&lua)[4], moved[4]);
    // A lane the engine cannot read is err, on every game alike, and says
    // why; the others still count.
    let (text, failed): (String, Vec<String>) = lua
        .load(
            "api.engine.system.townBuildingSystem = nil \
             local lanes, failed = ug_require('tpf3mp_1::/scripts/tpf3mp/lanes.lua').read(api) \
             return lanes[5], failed",
        )
        .eval()
        .unwrap();
    assert_eq!(text, "err");
    assert_eq!(failed.len(), 1);
    assert!(failed[0].starts_with("5: "), "{failed:?}");
}

#[test]
fn the_game_script_hands_the_lanes_over_at_a_checkpoint_only() {
    let (lua, _script) = engine();
    lua.load(FAKE_WORLD).exec().unwrap();
    // No checkpoint: nothing read.
    lua.load("UPDATE({}, STATE, 0.2)").exec().unwrap();
    let none: bool = lua.load("return HOOK.lanes == nil").eval().unwrap();
    assert!(none);
    // The last update of a batch that ends at a checkpoint: the lanes go
    // to the hook, the same the lanes module reads.
    lua.load("HOOK.checkpoint = true UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let handed: Table = lua.load("return HOOK.lanes").eval().unwrap();
    let mut handed: Vec<(u16, String)> = handed
        .pairs::<u16, String>()
        .map(|pair| pair.unwrap())
        .collect();
    handed.sort();
    assert_eq!(handed, read_lanes(&lua));
}

/// A simulation state without debug.getinfo cannot tell a personal mod's
/// command from the game's own: it notes so for the hook, which loads the
/// room's worlds without this player's personal mods from then on, and
/// says why in the log.
#[test]
fn a_simulation_state_that_cannot_guard_personal_mods_has_them_left_out() {
    let (lua, _script) = engine();
    lua.load(FAKE_WORLD).exec().unwrap();
    lua.load(
        "tpf3mp_native.personal = function() return 'celmi_timetables' end          debug = nil UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let (noted, logged): (Option<String>, String) = lua
        .load(
            "return HOOK.notes and HOOK.notes['personal-mods-unguarded'],              table.concat(HOOK.logged, '|')",
        )
        .eval()
        .unwrap();
    assert_eq!(noted.as_deref(), Some("1"));
    assert!(
        logged.contains("personal mods are left out of the room's worlds"),
        "{logged}"
    );
}

/// A game of the room with the stand-in world: its registry begun at the
/// room's first update, the engine listing entities in its own order.
fn dumping_game(reversed: bool) -> Lua {
    let (lua, _script) = engine();
    lua.load(FAKE_WORLD).exec().unwrap();
    lua.load(format!(
        "REVERSED = {reversed} HOOK.room = true UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    lua
}

/// The lines the game script hands the hook at a checkpoint whose lanes
/// the hook wants dumped.
fn dump_at_checkpoint(lua: &Lua, step: u64, lanes: &str) -> Vec<String> {
    lua.load(format!(
        "HOOK.dumped = {{}} HOOK.checkpoint = true HOOK.dump = {{ step = {step}, lanes = {{ {lanes} }} }} \
         UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    lua.load("return HOOK.dumped").eval().unwrap()
}

#[test]
fn a_lane_dump_is_the_lanes_text_entry_by_entry_keyed_and_in_the_same_order_on_every_game() {
    let a = dumping_game(false);
    let b = dumping_game(true);
    let dump_a = dump_at_checkpoint(&a, 300, "3, 0, 1, 2, 4, 5, 6");
    let dump_b = dump_at_checkpoint(&b, 300, "3, 0, 1, 2, 4, 5, 6");
    assert_eq!(
        dump_a, dump_b,
        "the same world dumps the same lines, whatever order the engine lists it in"
    );
    // Vehicles by their registry ids, with the raw values the lane rounds.
    let vehicles: Vec<&String> = dump_a
        .iter()
        .filter(|l| l.starts_with("lane 3 step 300 "))
        .collect();
    assert_eq!(
        vehicles,
        [
            "lane 3 step 300 vehicle-0/core state=1 stop=0 line=nil edge=3 pos=10.199999999999999 speed=5 \
             path_count=0 path_hash=nil entity=401 row=1:0:3~10.20 v5.00",
            "lane 3 step 300 vehicle-1/core state=2 stop=1 line=nil edge=0 pos=0 speed=0 \
             path_count=0 path_hash=nil entity=402 row=2:1:0~0.00 v0.00",
            "lane 3 step 300 vehicle-0 state=1 stop=0 line=nil edge=3 pos=10.199999999999999 speed=5 \
             arrival=nil/nil arrival_locked=nil load=nil pending=nil free=nil \
             entity=401 row=1:0:3~10.20 v5.00",
            "lane 3 step 300 vehicle-1 state=2 stop=1 line=nil edge=0 pos=0 speed=0 \
             arrival=nil/nil arrival_locked=nil load=nil pending=nil free=nil entity=402 \
             row=2:1:0~0.00 v0.00",
            &format!("lane 3 step 300 summary {}", read_lanes(&a)[3].1),
        ],
        "keyed, full precision, then the text the hook hashes"
    );
    // Every lane ends with the text lanes.read reads for it.
    for (lane, text) in read_lanes(&a) {
        let summary = format!("lane {lane} step 300 summary {text}");
        assert!(dump_a.contains(&summary), "{summary} in {dump_a:#?}");
    }
    assert!(
        dump_a.contains(
            &"lane 5 step 300 town-0 buildings=3 size=nil,nil,nil experience=nil level=nil entity=7 row=7:3"
                .to_owned()
        ),
        "{dump_a:#?}"
    );
    assert!(
        dump_a.iter().any(|l| l.starts_with(
            "lane 0 step 300 row:0,0,0>100,0,1:street/country.lua|lanes: p0=0,0,0 \
                                   p1=100.04000000000001,0,1"
        )),
        "an edge by its row, with its raw ends: {dump_a:#?}"
    );
    // Hashing changes nothing: the lanes are what they were.
    assert_eq!(read_lanes(&a), read_lanes(&b));

    // A vehicle a millimetre on in one game: the lanes still agree (1 cm),
    // the dumps name it and how.
    b.load("WORLD[9][402].dyn.pathPos.pos = 0.001")
        .exec()
        .unwrap();
    assert_eq!(read_lanes(&a), read_lanes(&b));
    let dump_a = dump_at_checkpoint(&a, 350, "3");
    let dump_b = dump_at_checkpoint(&b, 350, "3");
    let differ: Vec<(&String, &String)> =
        dump_a.iter().zip(&dump_b).filter(|(x, y)| x != y).collect();
    assert_eq!(
        differ.len(),
        2,
        "compact and verbose records both name the position"
    );
    assert!(differ[0].0.starts_with("lane 3 step 350 vehicle-1/core "));
    assert!(differ[0].1.contains(" pos=0.001 "), "{}", differ[0].1);
    assert!(differ[1].0.starts_with("lane 3 step 350 vehicle-1 "));

    // No dump asked: nothing handed over at a checkpoint.
    a.load("HOOK.dumped = {} HOOK.checkpoint = true UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let none: Vec<String> = a.load("return HOOK.dumped").eval().unwrap();
    assert!(none.is_empty());
}

#[test]
fn edge_ends_are_dumped_from_the_games_userdata_vectors_at_full_precision() {
    let a = dumping_game(false);
    let b = dumping_game(false);
    // As the game: an edge's ends are Vec3f userdata, whose x, y and z read,
    // which print as their address and raise on a field they lack.
    b.load(
        r#"
        local function vec(x, y, z)
            local v = newproxy(true)
            local fields = { x = x, y = y, z = z }
            local mt = getmetatable(v)
            mt.__index = function(_, k)
                if fields[k] == nil then error("Vec3f has no member " .. tostring(k)) end
                return fields[k]
            end
            mt.__tostring = function() return "Vec3f: 0x24bb8caeb48" end
            return v
        end
        WORLD[1][101].position0 = vec(0, 0, 0)
        WORLD[1][101].position1 = vec(100.04, 0, 1)
    "#,
    )
    .exec()
    .unwrap();
    assert_eq!(read_lanes(&a), read_lanes(&b));
    let da = dump_at_checkpoint(&a, 50, "0");
    let db = dump_at_checkpoint(&b, 50, "0");
    assert_eq!(da, db, "a userdata vector dumps as its table twin");
    assert!(
        db.iter()
            .any(|l| l.contains(" p0=0,0,0 p1=100.04000000000001,0,1 ")),
        "{db:#?}"
    );
    assert!(!db.iter().any(|l| l.contains("Vec3f")), "{db:#?}");
}

#[test]
fn a_towns_dump_carries_its_size_factors_experience_and_level() {
    let a = dumping_game(false);
    // The town's component, and the base game's town growth script's state
    // as its own town_cargo_util reads it (state_native:findPath).
    a.load(
        r#"
        api.type.ComponentType.GAME_SCRIPT = 20
        WORLD[7][7] = { sizeFactors = { 1.5, 0.1, 2 } }
        WORLD[20] = { [55] = { state_native = {
            findPath = function(self, path)
                assert(path[1] == "townState")
                if path[2] ~= 7 then return nil end
                return { asTable = function() return { experience = 1200, level = 3 } end }
            end } } }
        api.engine.system.gameScriptSystem = { getEntityForGameScript = function(name)
            assert(name == "::/game_mechanics/towns/town_cargo.gs")
            return 55
        end }
    "#,
    )
    .exec()
    .unwrap();
    let lanes = read_lanes(&a);
    let dump = dump_at_checkpoint(&a, 50, "5");
    assert_eq!(
        dump,
        [
            "lane 5 step 50 town-0 buildings=3 size=1.5,0.10000000000000001,2 experience=1200 level=3 entity=7 row=7:3"
                .to_owned(),
            format!("lane 5 step 50 summary {}", lanes[5].1),
        ]
    );
    // Without the native state, the plain one; the lane's text never
    // changes with them.
    a.load("WORLD[20][55] = { state = { townState = { [7] = { experience = 5, level = 0 } } } }")
        .exec()
        .unwrap();
    let dump = dump_at_checkpoint(&a, 100, "5");
    assert!(dump[0].contains(" experience=5 level=0 "), "{dump:#?}");
    assert_eq!(read_lanes(&a), lanes);
}

#[test]
fn a_network_dump_cut_to_a_box_keeps_the_edges_with_an_end_inside() {
    let a = dumping_game(false);
    let lanes = read_lanes(&a);
    let dump = |lua: &Lua, order: &str| -> Vec<String> {
        lua.load(format!(
            "HOOK.dumped = {{}} HOOK.checkpoint = true HOOK.dump = {order} UPDATE({{}}, STATE, 0.2)"
        ))
        .exec()
        .unwrap();
        lua.load("return HOOK.dumped").eval().unwrap()
    };
    // Edge 102 runs from (100, 0) to (100, 80): its far end is in the box,
    // edge 101's ends are not.
    let cut = dump(
        &a,
        "{ step = 12800, lanes = { 0, 5 }, box = { 90, 50, 110, 90 } }",
    );
    let network: Vec<&String> = cut.iter().filter(|l| l.starts_with("lane 0 ")).collect();
    assert_eq!(network.len(), 2, "{cut:#?}");
    assert!(network[0].contains(" entity=102 "), "{cut:#?}");
    assert_eq!(
        network[1],
        &format!("lane 0 step 12800 summary {}", lanes[0].1),
        "the summary is the whole lane's"
    );
    // The box is the network lane's only: the towns lane is whole.
    assert!(
        cut.iter()
            .any(|l| l.starts_with("lane 5 step 12800 town-0 "))
    );
    // Without a box, the whole lane.
    let whole = dump(&a, "{ step = 12850, lanes = { 0 } }");
    assert_eq!(whole.len(), 3, "{whole:#?}");
    // A box nothing lies in: the summary alone.
    let empty = dump(
        &a,
        "{ step = 12900, lanes = { 0 }, box = { -9, -9, -8, -8 } }",
    );
    assert_eq!(empty.len(), 1, "{empty:#?}");
}

#[test]
fn the_edge_watch_reads_each_entity_every_update_it_is_asked_for() {
    let a = dumping_game(false);
    a.load(
        r#"
        api.type.ComponentType.BASE_NODE = 10
        WORLD[10] = { [11] = { position = { x = 0, y = 0, z = 0 } },
                      [12] = { position = { x = 100.04, y = 0, z = 1 } } }
        local e = WORLD[1][101]
        e.node0, e.node1 = 11, 12
        e.tangent0, e.tangent1 = { x = 100, y = 0, z = 1 }, { x = 100, y = 0.5, z = 1 }
        e.type = 0
        HOOK.watched = {}
        tpf3mp_native.edgewatch = function() return HOOK.watch end
        tpf3mp_native.edgewatched = function(e, text) HOOK.watched[#HOOK.watched + 1] = e .. ' ' .. text end
        "#,
    )
    .exec()
    .unwrap();
    // Not asked: no read, and nothing for postUpdate to do.
    let work: mlua::Value = a.load("return UPDATE({}, STATE, 0.2)").eval().unwrap();
    assert!(work.is_nil());
    // Asked: postUpdate runs and reads each one, an edge, a node and none.
    a.load("HOOK.watch = { 101, 11, 999 } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let watched: Vec<String> = a.load("return HOOK.watched").eval().unwrap();
    assert_eq!(
        watched,
        [
            "101 edge node0=11 node1=12 p0=0,0,0 p1=100.04000000000001,0,1 t0=100,0,1 \
             t1=100,0.5,1 n0=0,0,0 n1=100.04000000000001,0,1 type=0 template=street/country.lua",
            "11 node pos=0,0,0",
            "999 absent",
        ]
    );
    // An older hook without the watch: nothing asked, nothing read.
    a.load("tpf3mp_native.edgewatch = nil HOOK.watched = {} UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let watched: Vec<String> = a.load("return HOOK.watched").eval().unwrap();
    assert!(watched.is_empty());
}

#[test]
fn terminal_choices_and_locks_are_dumped_without_changing_the_vehicle_digest() {
    let a = dumping_game(false);
    let b = dumping_game(false);
    a.load(
        "WORLD[4][401].arrivalStationTerminal = { station = 0, terminal = 1 } \
            WORLD[4][401].arrivalStationTerminalLocked = false",
    )
    .exec()
    .unwrap();
    b.load(
        "WORLD[4][401].arrivalStationTerminal = { station = 0, terminal = 2 } \
            WORLD[4][401].arrivalStationTerminalLocked = true",
    )
    .exec()
    .unwrap();
    assert_eq!(read_lanes(&a), read_lanes(&b));
    let da = dump_at_checkpoint(&a, 50, "3");
    let db = dump_at_checkpoint(&b, 50, "3");
    let differences: Vec<_> = da.iter().zip(&db).filter(|(x, y)| x != y).collect();
    assert_eq!(differences.len(), 1);
    assert!(
        differences[0]
            .0
            .contains(" arrival=0/1 arrival_locked=false ")
    );
    assert!(
        differences[0]
            .1
            .contains(" arrival=0/2 arrival_locked=true ")
    );
}

/// The engine's finance reads, as the game script sees them: each vehicle's
/// and line's takings from the journal, the player's income and the
/// finance window's table.
const FAKE_FINANCE: &str = r#"
api.type.ComponentType.GAME_TIME = 10
WORLD[10] = { [1] = { gameTime = 5000 } }
WORLD[4][401].line = 301
WORLD[4][401].loadState = 3
WORLD[4][401].unloadPendingIncome = { amount = 77 }
WORLD[4][401].lineStop2cargo2available = { { 40, 0 }, { 38, 2 } }
TAKINGS = { [401] = 1200, [402] = -300, [301] = 900 }
CALLS = {}
api.engine.util.getWorld = function() return 1 end
api.engine.util.finance = {
    calculateBalance = function(list, from, to, incomeOnly)
        CALLS[#CALLS + 1] = from .. '-' .. to .. ':' .. tostring(incomeOnly)
        return TAKINGS[list[1]]
    end,
    calcIncomeSince = function(time, player) return 4321 end,
    getLastIncomeTime = function(player) return 4990 end,
    computeFinanceTable = function(player, config)
        local data = { total = { 1, 2, 3, config.count }, loan = { 0, 0, 0, 0 } }
        -- The engine's own map order: the dump sorts it.
        function data:foreach_carrier(f) f(2) f(0) end
        function data:foreach_transport(f, carrier) f(1, { carrier * 10, 5 }) end
        function data:foreach_investment(f) f(4, { -8 }) end
        function data:foreach_other(f) end
        return data
    end,
}
api.type.ChartConfig = { new = function() return {} end }
"#;

#[test]
fn an_economy_dump_names_each_vehicles_and_lines_takings_without_changing_the_digest() {
    let a = dumping_game(false);
    let before = read_lanes(&a);
    a.load(FAKE_FINANCE).exec().unwrap();
    assert_eq!(read_lanes(&a)[4], before[4], "the balance alone is hashed");
    let dump = dump_at_checkpoint(&a, 400, "4");
    let economy: Vec<&str> = dump
        .iter()
        .filter(|l| l.starts_with("lane 4 step 400 "))
        .map(String::as_str)
        .collect();
    let summary = format!("lane 4 step 400 summary {}", before[4].1);
    assert_eq!(
        economy,
        [
            "lane 4 step 400 line-0 takings=900 entity=301 row=takings:301",
            "lane 4 step 400 player balance=1234567 loan=nil time=5000 income=4321 \
             last_income=4990 balance=nil interest=nil investment4=-8 loan=0/0/0/0 \
             loanBorrowing=nil loanRepayment=nil total=1/2/3/4 transport0.1=0/5 \
             transport2.1=20/5 entity=25 row=25:1234567",
            "lane 4 step 400 vehicle-0 takings=1200 line=line-0 entity=401 row=takings:401",
            "lane 4 step 400 vehicle-1 takings=-300 line=nil entity=402 row=takings:402",
            summary.as_str(),
        ],
        "each line's and vehicle's takings by its id, the balance with the finance table sorted"
    );
    let calls: Vec<String> = a.load("return CALLS").eval().unwrap();
    assert_eq!(
        calls, ["0-5000:true"; 3],
        "from the game's start to now, income and maintenance, as the game's windows read it"
    );

    // The vehicles' dump carries what they have room for and the income
    // pending; their digest does not.
    let b = dumping_game(false);
    assert_eq!(read_lanes(&a)[3], read_lanes(&b)[3]);
    let vehicles = dump_at_checkpoint(&a, 450, "3");
    assert!(
        vehicles
            .iter()
            .any(|l| l.contains(" load=3 pending=77 free=40/0|38/2 ")),
        "{vehicles:#?}"
    );

    // A world without the finance reads still dumps its balance.
    let plain = dump_at_checkpoint(&b, 400, "4");
    assert!(
        plain.iter().any(|l| l.starts_with(
            "lane 4 step 400 player balance=1234567 loan=nil time=nil income=nil last_income=nil err "
        )),
        "{plain:#?}"
    );
    assert!(
        plain
            .iter()
            .any(|l| l.starts_with("lane 4 step 400 vehicle-0 takings=nil ")),
        "{plain:#?}"
    );
}

#[test]
fn physical_paths_are_dumped_without_changing_the_vehicle_digest() {
    let a = dumping_game(false);
    let b = dumping_game(false);
    for lua in [&a, &b] {
        lua.load(
            "WORLD[9][401].path = { edges = { \
                { edgeId = { entity = 81, index = 2 }, dir = false }, \
                { edgeId = { entity = 82, index = 0 }, dir = true } \
              }, endOffset = 1.5, terminalDecisionOffset = 20 }",
        )
        .exec()
        .unwrap();
    }
    b.load("WORLD[9][401].path.edges[2].edgeId.entity = 83")
        .exec()
        .unwrap();
    assert_eq!(read_lanes(&a), read_lanes(&b));
    let da = dump_at_checkpoint(&a, 50, "3");
    let db = dump_at_checkpoint(&b, 50, "3");
    let va = da.iter().find(|l| l.contains(" vehicle-0 ")).unwrap();
    let vb = db.iter().find(|l| l.contains(" vehicle-0 ")).unwrap();
    let field = |s: &str, key: &str| {
        s.split_whitespace()
            .find(|f| f.starts_with(key))
            .unwrap()
            .to_owned()
    };
    assert_ne!(field(va, "path_hash="), field(vb, "path_hash="));
    assert!(va.contains(" path_count=2 "));
    assert!(va.contains(" path_end=1.5 decision_offset=20 "));
    // The real API uses named fields; the documented tuple form remains supported.
    b.load(
        "WORLD[9][401].path.edges = { \
            { { entity = 81, index = 2 }, false }, \
            { { entity = 82, index = 0 }, true } }",
    )
    .exec()
    .unwrap();
    assert_eq!(da, dump_at_checkpoint(&b, 50, "3"));
}

#[test]
fn long_vehicle_diagnostics_survive_upload_and_bound_route_reads() {
    let game = dumping_game(false);
    game.load(r#"
        WORLD[9][401].path = { edges = {}, endOffset = 7, terminalDecisionOffset = 98 }
        for i = 1, 300 do
            WORLD[9][401].path.edges[i] = { edgeId = { entity = 8000 + i, index = i % 3 }, dir = false }
        end
        WORLD[4][401].lineStop2cargo2available = {}
        for i = 1, 15 do
            local cargo = {}
            for j = 1, 40 do cargo[j] = j end
            WORLD[4][401].lineStop2cargo2available[i] = cargo
        end
        local original = api.engine.getComponent
        GEOMETRY_READS = 0
        api.engine.getComponent = function(e, kind)
            if e > 8000 and e <= 8300 then
                GEOMETRY_READS = GEOMETRY_READS + 1
                return { position0 = {x=e,y=0,z=5}, position1 = {x=e+1,y=0,z=5},
                    tangent0 = {x=1,y=0,z=0}, tangent1 = {x=1,y=0,z=0} }
            end
            return original(e, kind)
        end
    "#).exec().unwrap();
    let before = read_lanes(&game);
    assert_eq!(
        game.load("return GEOMETRY_READS").eval::<usize>().unwrap(),
        0
    );
    let dump = dump_at_checkpoint(&game, 50, "3");
    assert_eq!(
        before,
        read_lanes(&game),
        "diagnostics never change lane digests"
    );
    assert_eq!(
        game.load("return GEOMETRY_READS").eval::<usize>().unwrap(),
        14
    );
    for line in &dump {
        assert!(line.len() < 1024, "{} bytes: {line}", line.len());
        assert_eq!(&tpf3mp_proto::redact(line), line);
    }
    let text = dump.join("\n");
    assert!(text.contains("route_records=14 route_omitted=286"));
    assert!(text.contains("free_part0="));
    assert!(text.contains("decision_offset=98"));
    assert!(text.contains(
        "/path-0299 route_index=299 edge_entity=8300 lane_index=0 direction=false p0=8300,0,5"
    ));
    assert!(text.contains("path_hash_scope=local_ids"));
    assert!(text.contains("row=1:0:3~10.20"));
}

#[test]
fn shifty_sized_vehicle_dumps_fit_two_checkpoints_in_the_hook_upload_budget() {
    let game = dumping_game(false);
    game.load(
        r#"
        local edges = {}
        for i = 1, 300 do
            edges[i] = { edgeId = { entity = 8000 + i, index = i % 3 }, dir = false }
        end
        WORLD[9][401].path = { edges = edges }
        local cargo = {}
        for i = 1, 15 do
            cargo[i] = {}
            for j = 1, 40 do cargo[i][j] = j end
        end
        WORLD[4][401].lineStop2cargo2available = cargo
        for e = 403, 413 do
            WORLD[4][e] = WORLD[4][401]
            WORLD[9][e] = WORLD[9][401]
        end
    "#,
    )
    .exec()
    .unwrap();
    let first = dump_at_checkpoint(&game, 50, "3");
    let second = dump_at_checkpoint(&game, 100, "3");
    let wire_size = |dump: &[String]| -> usize { dump.iter().map(|line| line.len() + 16).sum() };
    assert!(
        first.len() > 100,
        "13 vehicles still have sampled route detail"
    );
    assert!(
        wire_size(&first) + wire_size(&second) <= 192 << 10,
        "two 13-vehicle checkpoints must fit one minute of hook-log upload: {} + {} bytes",
        wire_size(&first),
        wire_size(&second)
    );
    assert_eq!(
        first
            .iter()
            .filter(|line| line.contains(" state=")
                && !line.contains("/path-")
                && !line.contains("/core"))
            .count(),
        13,
        "every vehicle keeps its base record"
    );
}

#[test]
fn peter_sized_vehicle_dump_keeps_both_compact_checkpoints_under_the_hook_budget() {
    let game = dumping_game(false);
    game.load(
        r#"
        local edges = {}
        for i = 1, 300 do
            edges[i] = { edgeId = { entity = 8000 + i, index = i % 3 }, dir = false }
        end
        WORLD[9][401].path = { edges = edges }
        for e = 403, 557 do
            WORLD[4][e] = WORLD[4][401]
            WORLD[9][e] = WORLD[9][401]
        end
    "#,
    )
    .exec()
    .unwrap();
    let first = dump_at_checkpoint(&game, 50, "3");
    let second = dump_at_checkpoint(&game, 100, "3");
    let bytes: usize = second.iter().map(|line| line.len() + 16).sum();
    assert_eq!(
        second
            .iter()
            .filter(|line| line.contains(" state=")
                && !line.contains("/path-")
                && !line.contains("/core"))
            .count(),
        157
    );
    assert_eq!(
        second.iter().filter(|line| line.contains("/core ")).count(),
        157
    );
    assert!(
        bytes <= 192 << 10,
        "one 157-vehicle checkpoint used {bytes} bytes before other hook lines"
    );
    let logged = |dump: Vec<String>| -> String {
        let mut lines = dump;
        lines.sort_by_key(|line| {
            let key = line.split_whitespace().nth(4).unwrap_or("");
            match key {
                key if key.contains("/path-") => 0,
                "summary" | "err" => 3,
                key if key.ends_with("/core") => 2,
                _ => 1,
            }
        });
        lines
            .into_iter()
            .map(|line| format!("[1759240010] {line}\n"))
            .collect::<String>()
    };
    let combined = logged(first) + &logged(second);
    let tail = &combined[combined.len().saturating_sub(192 << 10)..];
    for step in [50, 100] {
        let count = tail
            .lines()
            .filter(|line| line.contains(&format!(" step {step} ")) && line.contains("/core "))
            .count();
        assert_eq!(
            count,
            157,
            "checkpoint {step}'s core records survived the newest 192 KiB of {} bytes",
            combined.len()
        );
    }
}

#[test]
fn an_unsampled_middle_route_change_is_visible_in_the_full_path_hash() {
    let a = dumping_game(false);
    let b = dumping_game(false);
    for game in [&a, &b] {
        game.load(
            r#"
            WORLD[9][401].path = { edges = {} }
            for i = 1, 300 do
                WORLD[9][401].path.edges[i] = {
                    edgeId = { entity = 8000 + i, index = i % 3 }, dir = false
                }
            end
        "#,
        )
        .exec()
        .unwrap();
    }
    b.load("WORLD[9][401].path.edges[139].edgeId.entity = 9000")
        .exec()
        .unwrap();
    let da = dump_at_checkpoint(&a, 50, "3");
    let db = dump_at_checkpoint(&b, 50, "3");
    let routes = |dump: &[String]| -> Vec<String> {
        dump.iter()
            .filter(|line| line.contains("/path-"))
            .cloned()
            .collect()
    };
    assert_eq!(
        routes(&da),
        routes(&db),
        "middle edge is deliberately unsampled"
    );
    let base = |dump: &[String]| -> String {
        dump.iter()
            .find(|line| line.contains(" path_hash="))
            .unwrap()
            .clone()
    };
    assert_ne!(
        base(&da),
        base(&db),
        "the full path hash still exposes the change"
    );
}

#[test]
fn a_lane_that_cannot_be_read_dumps_why_and_a_hook_without_dumps_is_left_alone() {
    let lua = dumping_game(false);
    lua.load("api.engine.system.townBuildingSystem = nil")
        .exec()
        .unwrap();
    let dump = dump_at_checkpoint(&lua, 50, "5");
    assert_eq!(dump.len(), 1, "{dump:?}");
    assert!(dump[0].starts_with("lane 5 step 50 err "), "{dump:?}");
    // An older hook has neither function: the lanes still go over.
    lua.load("tpf3mp_native.dump, tpf3mp_native.dumped = nil, nil")
        .exec()
        .unwrap();
    let dump = dump_at_checkpoint(&lua, 100, "5");
    assert!(dump.is_empty());
    let handed: bool = lua.load("return HOOK.lanes ~= nil").eval().unwrap();
    assert!(handed);
}

const DEPOT: &str = "{ BuildConstruction = { \
    file = 'depot/road_depot_era_a.con', \
    transform = { basis = { 0, 1, 0, -1, 0, 0, 0, 0, 1 }, origin = { x = 1250.5, y = -300, z = 20 } }, \
    params = { { key = 'seed', value = { Int = 1234 } }, \
               { key = 'modules[3801].name', value = { Text = 'depot/module.module' } }, \
               { key = 'paramX', value = { Fixed = 2.5 } }, \
               { key = 'lit', value = { Bool = true } } }, \
    name = 'Depot' } }";

#[test]
fn tutorial_cleanup_runs_in_each_rooms_simulation() {
    // Three independent worlds, not a GUI-only unlock. The command is the
    // native Quit Tutorial event; its handler marks isComplete=false before
    // the mission script's next postUpdate removes tasks and restrictions.
    for _ in 0..3 {
        let (lua, _) = engine();
        lua.load(
            r#"
            MISSION = { isTutorialActive = true }
            api.type.ComponentType.GAME_SCRIPT = 100
            api.engine.system.gameScriptSystem = {
                getEntityForGameScript = function(name)
                    if name == '::/mission/mission.gs' then return 777 end
                    return -1
                end
            }
            api.engine.getComponent = function(entity, kind)
                if entity == 777 and kind == 100 then return { state = MISSION } end
            end
            local send = api.cmd.sendCommand
            QUITS = 0
            api.cmd.sendCommand = function(command, callback)
                if command.event and command.event.name == 'abortMission' then
                    assert(PHASE == 'post')
                    assert(command.event.src == '' and command.event.id == 'MissionWindow')
                    assert(command.event.param == true)
                    QUITS = QUITS + 1
                    MISSION.isComplete = false
                end
                send(command, callback)
            end
            -- An installed mod must leave a single-player tutorial alone.
            HOOK.room = false
            UPDATE({}, STATE, 0.2)
            assert(QUITS == 0 and MISSION.isComplete == nil)
            HOOK.room = true
            WORK = SCRIPT.update({}, STATE, 0.2)
            assert(QUITS == 0 and WORK.quitTutorial == true)
            PHASE = 'post'
            SCRIPT.postUpdate({}, STATE, 0.2, WORK)
            PHASE = nil
            assert(QUITS == 1 and MISSION.isComplete == false)
            UPDATE({}, STATE, 0.2)
            assert(QUITS == 1) -- false means quitting, not still active
            MISSION = { isComplete = true, tasks = {} }
            UPDATE({}, STATE, 0.2)
            assert(QUITS == 1)
            -- Campaign and ordinary saves must not be aborted.
            MISSION = { tasks = {} }
            UPDATE({}, STATE, 0.2)
            assert(QUITS == 1)
            -- Stale work cannot quit a tutorial after leaving the room.
            MISSION = { isTutorialActive = true }
            WORK = SCRIPT.update({}, STATE, 0.2)
            HOOK.room = false
            SCRIPT.postUpdate({}, STATE, 0.2, WORK)
            assert(QUITS == 1)
        "#,
        )
        .exec()
        .unwrap();
    }
}

#[test]
fn the_game_script_applies_the_rooms_actions_as_the_players_own_builds() {
    let (lua, _script) = engine();
    // No action ordered: nothing sent, and nothing for postUpdate.
    let work: mlua::Value = lua.load("return UPDATE({}, STATE, 0.2)").eval().unwrap();
    assert!(work.is_nil());
    assert_eq!(lua.load("return #SENT").eval::<usize>().unwrap(), 0);
    // update only takes the actions; the world changes in postUpdate, as
    // the game's own scripts change it.
    lua.load(STATION_REFRESH).exec().unwrap();
    lua.load(format!(
        "HOOK.batch = {{ {DEPOT} }} WORK = SCRIPT.update({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    assert_eq!(lua.load("return #SENT").eval::<usize>().unwrap(), 0);
    lua.load("SCRIPT.postUpdate({}, STATE, 0.2, WORK)")
        .exec()
        .unwrap();
    assert_eq!(lua.load("return #SENT").eval::<usize>().unwrap(), 1);
    lua.load("SENT = {}").exec().unwrap();
    lua.load(format!(
        "HOOK.batch = {{ {DEPOT} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    let built: String = lua
        .load(
            "local c = SENT[1] local e = c.proposal.constructionsToAdd[1]
             local t = e.transf
             return table.concat({ e.fileName, e.name, e.playerEntity,
                 t[1][1], t[1][2], t[2][1], t[4][1], t[4][2], t[4][3], t[4][4],
                 e.params.seed, e.params.modules[3801].name, e.params.paramX, tostring(e.params.lit),
                 tostring(c.ignoreErrors), tostring(c.playerInitiated), tostring(c.context.player),                  tostring(c.context.gatherBuildings), tostring(c.context.gatherFields) }, '|')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        built,
        "depot/road_depot_era_a.con|Depot|25|0|1|-1|1250.5|-300|20|1|1234|depot/module.module|2.5|true|true|true|25|true|true"
    );
    // Subscribed to its console event, linked once.
    assert!(
        lua.load(
            "return STATE.subscribed.command and STATE.subscribed['builder.proposalCreate'] \
                    and STATE.subscribed['builder.proposalPrepareForApply']"
        )
        .eval::<bool>()
        .unwrap()
    );
    assert_eq!(
        lua.load("return table.concat(HOOK.logged, '|')")
            .eval::<String>()
            .unwrap(),
        "the game script is linked"
    );
}

#[test]
fn a_paused_game_applies_ordered_builds_without_running_an_update() {
    let (lua, _) = engine();
    lua.load(format!(
        "HOOK.room=true HOOK.replayToken='1' HOOK.replayBatch={{ {DEPOT} }} \
        SCRIPT.handleEvent({{}}, STATE, '', '', 'handleLegacy', {{}}) \
        assert(STATE.subscribed.replay) \
        SCRIPT.handleEvent({{}}, STATE, '', 'tpf3mp', 'replay', 'stale') \
        assert(#SENT == 0) \
        SCRIPT.handleEvent({{}}, STATE, '', 'tpf3mp', 'command', '1') \
        assert(#SENT == 1 and #HOOK.applied == 1 and HOOK.replayDone.ok) \
        assert(STATE.value and STATE.value.registry and STATE.value.companies) \
        assert(HOOK.lanes == nil and HOOK.replaying[#HOOK.replaying] == false) \
        SCRIPT.handleEvent({{}}, STATE, '', 'tpf3mp', 'command', '1') \
        assert(#SENT == 1 and #HOOK.applied == 1)"
    ))
    .exec()
    .unwrap();
}

#[test]
fn a_replay_script_error_is_reported_and_always_clears_the_build_bypass() {
    let (lua, _) = engine();
    lua.load(format!(
        "HOOK.room=true HOOK.replayToken='1' HOOK.replayBatch={{ {DEPOT} }} \
        STATE.set=function() error('state storage failed') end \
        SCRIPT.handleEvent({{}}, STATE, '', 'tpf3mp', 'command', '1') \
        assert(not HOOK.replayDone.ok and HOOK.replayDone.why:find('state storage failed',1,true)) \
        assert(HOOK.replaying[#HOOK.replaying] == false)"
    ))
    .exec()
    .unwrap();
}

#[test]
fn the_gui_wakes_only_the_ordered_replay_and_never_forwards_its_payload() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    run_frames(&lua, 1);
    lua.load("HOOK.room=true HOOK.request={ replay='123' }")
        .exec()
        .unwrap();
    run_frames(&lua, 1);
    lua.load(
        "assert(#SENT == 1, 'sent=' .. #SENT .. ' wake=' .. tostring(HOOK.replayDone and HOOK.replayDone.why) .. ' logs=' .. table.concat(HOOK.logged, ' | ')) local c=SENT[1].command \
        assert(c.id=='tpf3mp' and c.name=='command' and c.param=='123') \
        local guard=require('tpf3mp.guard') \
        assert(not guard.wakeReplay(api.cmd, { BuildRoad={} })) \
        api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'tpf3mp', 'replay', '123')) \
        assert(#SENT == 1)",
    )
    .exec()
    .unwrap();
}

#[test]
fn the_game_script_takes_and_repays_loans_through_the_loan_scripts_events() {
    let (lua, _script) = engine();
    lua.load(format!(
        "HOOK.batch = {{ {{ Loan = {{ Take = {{ next = {OFFER}, offer = {OFFER} }} }} }}, \
                         {{ Loan = {{ Repay = {{ loan = {OFFER} }} }} }} }} \
         UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    let events: String = lua
        .load(
            "local out = {} \
             for _, c in ipairs(SENT) do \
                 local e = c.event \
                 local p1 = e.param[1] and e.param[1].amount or 'nil' \
                 out[#out + 1] = e.src .. '|' .. e.id .. '|' .. e.name .. '|' .. tostring(p1) \
                     .. '|' .. e.param[2].amount .. '|' .. e.param[2].percentage .. '|' .. e.param[2].type \
             end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        events,
        "|Loan|Obtain|5000000|5000000|0.03|Small |Loan|Repay|nil|5000000|0.03|Small"
    );
}

/// A construction tool's proposal, as build 40408 hands it to game scripts:
/// a maintenance building placed by the construction tool.
const CONSTRUCTION_PROPOSAL: &str = "{ \
    proposal = { addedNodes = {}, addedSegments = {}, removedNodes = {}, removedSegments = {}, \
                 edgeObjectsToAdd = {} }, \
    toRemove = {}, \
    toAdd = { { fileName = '::/depots/road/road_maint_station.con', \
                name = 'Okehampton Maintenance Building', playerEntity = 3869, \
                transf = { 0.707107, -0.707107, 0, 0, 0.707107, 0.707107, 0, 0, 0, 0, 1, 0, \
                           -421.93572998047, -252.93925476074, 0.50797754526138, 1 }, \
                params = { modules = { [3801] = { name = 'depot/module.module', variant = 2 } }, \
                           year = 1990, seed = 0, scale = 1.5, lit = true } } } }";

/// A PLAYER_OWNED component as build 40408 hands it to Lua: userdata, its
/// `player` read through the binding, never a table.
struct NativeOwner(i64);

impl mlua::UserData for NativeOwner {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("player", |_, this| Ok(this.0));
    }
}

/// A company's depots did not count as its own (2026-10-02: "0
/// construction(s)" after a depot built by company #1, and its vehicles
/// bought from a far depot). The owner read took the game's userdata
/// component for no one's; and the room never made sure of what the engine
/// made from the build. Every game now hands the new construction, its
/// depots, stations, their own group and its own edges to the acting
/// company where anyone else owns them, and reads an owner through the
/// binding.
#[test]
fn a_depot_the_room_builds_is_the_acting_companys() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    let owned = lua
        .create_function(|lua, player: i64| lua.create_userdata(NativeOwner(player)))
        .unwrap();
    lua.globals().set("NATIVE_OWNER", owned).unwrap();
    lua.load(format!(
        "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         ACTION = capture.construction({CONSTRUCTION_PROPOSAL})"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    lua.load(STATION_REFRESH).exec().unwrap();
    lua.load(
        "local CT = api.type.ComponentType \
         CT.STATION_GROUP, CT.PLAYER_OWNED = 9, 15 \
         OWNERS = { [5010] = 25, [5020] = 30 } \
         EDGES[5020] = { node0 = 8, node1 = 9, objects = { { 5030, 0 } } } \
         api.engine.system.stationGroupSystem = { getStationGroup = function(s) \
             if s == 5040 then return 5050 end return -1 end } \
         local get = api.engine.getComponent \
         api.engine.getComponent = function(e, kind) \
             if kind == 15 then return OWNERS[e] and NATIVE_OWNER(OWNERS[e]) or nil end \
             if kind == 9 and e == 5050 then return { stations = { 5040 } } end \
             return get(e, kind) \
         end \
         api.cmd.makeEntitySetPlayerCmd = function(entity, player) \
             return { setPlayer = entity, player = player } end \
         local send = api.cmd.sendCommand \
         api.cmd.sendCommand = function(cmd, ...) \
             local r = send(cmd, ...) \
             if cmd.setPlayer then OWNERS[cmd.setPlayer] = cmd.player end \
             local c = CONSTRUCTIONS[5000] \
             if c and not c.depots then \
                 c.depots, c.stations, c.frozenEdges = { 5010 }, { 5040 }, { 5020 } \
                 OWNERS[5000] = 901 \
             end \
             return r \
         end \
         A = string.rep('a', 64) \
         HOOK.batch = { { CompanyOp = { Create = { name = 'Rival' } } } } HOOK.origins = { A } \
         UPDATE({}, STATE, 0.2) \
         SENT = {} HOOK.applied = {} \
         HOOK.batch = { ACTION } HOOK.origins = { A } UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    let (ok, built, owners): (bool, String, String) = lua
        .load(
            "local o = {} \
             for _, e in ipairs({ 5000, 5010, 5020, 5030, 5040, 5050 }) do o[#o + 1] = e .. '=' .. tostring(OWNERS[e]) end \
             local p = SENT[1].proposal \
             return HOOK.applied[1].ok == true, \
                 p.constructionsToAdd[1].playerEntity .. '>' .. SENT[1].context.player, table.concat(o, ' ')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert!(ok, "{}", hook_log(&lua));
    assert_eq!(built, "901>901", "built for the acting company, paid by it");
    assert_eq!(
        owners, "5000=901 5010=901 5020=901 5030=901 5040=901 5050=901",
        "the construction, its depot, its own edge and what stands on it, its station and group"
    );
    let log = hook_log(&lua);
    assert!(
        log.contains(
            "the new ::/depots/road/road_maint_station.con made the acting company's (901): \
             depot 5010 (was 25), station 5040 (was nil), station group 5050 (was nil), \
             edge 5020 (was 30), edge object 5030 (was nil)"
        ),
        "{log}"
    );
}

/// A construction may already exist when its ownership cannot be settled.
/// The room must hear that its build was not applied, and a missing result
/// lookup must not be silently reported as success.
#[test]
fn a_construction_owner_command_or_lookup_failure_is_not_applied() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(STATION_REFRESH).exec().unwrap();
    lua.load(format!("DEPOT_ACTION = {DEPOT}")).exec().unwrap();
    lua.load(
        r#"
        api.type.ComponentType.PLAYER_OWNED = 15
        OWNERS, FAIL_OWNER, HIDE_CONSTRUCTION, FAIL_READ_OWNER = {}, false, false, false
        local get = api.engine.getComponent
        api.engine.getComponent = function(e, kind)
            if kind == 15 and e == 5000 and FAIL_READ_OWNER then error('owner component read failed') end
            if kind == 15 then return OWNERS[e] and { player = OWNERS[e] } end
            return get(e, kind)
        end
        local list = api.engine.getEntitiesWithComponent
        api.engine.getEntitiesWithComponent = function(kind)
            if kind == 2 and HIDE_CONSTRUCTION then return {} end
            return list(kind)
        end
        api.cmd.makeEntitySetPlayerCmd = function(entity, player)
            return { setPlayer = entity, player = player }
        end
        local send = api.cmd.sendCommand
        api.cmd.sendCommand = function(cmd, ...)
            if cmd.setPlayer then
                if FAIL_OWNER then error('the game refused owner settlement') end
                OWNERS[cmd.setPlayer] = cmd.player
            elseif cmd.proposal and cmd.proposal.constructionsToAdd
                and cmd.proposal.constructionsToAdd[1] then
                local result = send(cmd, ...)
                -- The game committed the construction before owner settlement.
                OWNERS[5000] = 25
                return result
            end
            return send(cmd, ...)
        end
        A = string.rep('a', 64)
        HOOK.batch = { { CompanyOp = { Create = { name = 'Rival' } } } }
        HOOK.origins = { A }
        UPDATE({}, STATE, 0.2)
        function buildWithFailure(fail_owner, hide_construction, fail_read_owner)
            FAIL_OWNER, HIDE_CONSTRUCTION, FAIL_READ_OWNER = fail_owner, hide_construction, fail_read_owner
            HOOK.applied = {}
            HOOK.batch = { DEPOT_ACTION }
            HOOK.origins = { A }
            UPDATE({}, STATE, 0.2)
            local result = HOOK.applied[1]
            return result.ok, result.why, CONSTRUCTIONS[5000] ~= nil
        end
        "#,
    )
    .exec()
    .unwrap();

    let (applied, why, construction_exists): (bool, String, bool) = lua
        .load("return buildWithFailure(true, false, false)")
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert!(!applied, "a refused owner command cannot report success");
    assert!(why.contains("the game refused owner settlement"), "{why}");
    assert!(
        construction_exists,
        "the engine build happened before settlement"
    );

    let (applied, why, construction_exists): (bool, String, bool) = lua
        .load("return buildWithFailure(false, false, true)")
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert!(!applied, "an owner lookup failure cannot report success");
    assert!(why.contains("owner component read failed"), "{why}");
    assert!(
        construction_exists,
        "the engine build happened before owner lookup"
    );

    let (applied, why, construction_exists): (bool, String, bool) = lua
        .load("return buildWithFailure(false, true, false)")
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert!(
        !applied,
        "a missing construction lookup cannot report success"
    );
    assert!(why.contains("no depot/road_depot_era_a.con there"), "{why}");
    assert!(
        construction_exists,
        "the lookup failure follows the engine build"
    );
}

#[test]
fn a_construction_the_tool_placed_becomes_the_rooms_action() {
    let (lua, _script) = engine();
    let (file, name, origin_x, seed, module, ok): (String, String, f64, i64, String, bool) = lua
        .load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local action = capture.construction({CONSTRUCTION_PROPOSAL}) \
             local b = action.BuildConstruction \
             local seed, module \
             for _, p in ipairs(b.params) do \
                 if p.key == 'seed' then seed = p.value.Int end \
                 if p.key == 'modules[3801].name' then module = p.value.Text end \
             end \
             return b.file, b.name, b.transform.origin.x, seed, module, schema_check(action)"
        ))
        .eval()
        .unwrap();
    assert_eq!(file, "::/depots/road/road_maint_station.con");
    assert_eq!(name, "Okehampton Maintenance Building");
    assert!((origin_x + 421.935_729_980_47).abs() < 1e-9);
    assert_eq!(seed, 0);
    assert_eq!(module, "depot/module.module");
    assert!(ok, "the schema takes it");
    // What the room cannot carry yet says why.
    let refusals: Vec<String> = lua
        .load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local out = {{}} \
             local function why(p) local _, r = capture.construction(p) out[#out + 1] = r end \
             local two = {CONSTRUCTION_PROPOSAL} two.toAdd[2] = two.toAdd[1] \
             why(two) \
             local unnamed = {CONSTRUCTION_PROPOSAL} unnamed.toAdd[1].name = '' \
             why(unnamed) \
             local odd = {CONSTRUCTION_PROPOSAL} odd.toAdd[1].params.f = print \
             why(odd) \
             return out"
        ))
        .eval()
        .unwrap();
    assert_eq!(refusals[0], "more than one construction at once");
    assert_eq!(refusals[1], "an unnamed construction");
    assert!(
        refusals[2].contains("parameter f is a function"),
        "{}",
        refusals[2]
    );
    // A construction whose tool built no streets carries none.
    let (connection, ok): (bool, bool) = lua
        .load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local action = capture.construction({CONSTRUCTION_PROPOSAL}) \
             return action.BuildConstruction.connection ~= nil, schema_check(action)"
        ))
        .eval()
        .unwrap();
    assert!(!connection);
    assert!(ok);
    // Town buildings in the way go, as the replay clears them again, and
    // are no construction replaced; a construction the room cannot name
    // does not travel.
    let (cleared, replaced): (bool, String) = lua
        .load(format!(
            "api.type.ComponentType = {{ CONSTRUCTION = 2 }} \
             local CONSTRUCTIONS = {{ [5618] = {{ townBuildings = {{ 9001 }} }}, [77] = {{ townBuildings = {{}} }} }} \
             api.engine.getComponent = function(e, kind) return CONSTRUCTIONS[e] end \
             local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local town = {CONSTRUCTION_PROPOSAL} town.toRemove = {{ 5618 }} \
             local own = {CONSTRUCTION_PROPOSAL} own.toRemove = {{ 5618, 77 }} \
             local _, why = capture.construction(own) \
             local action = capture.construction(town) \
             return action ~= nil and action.BuildConstruction.replaces == nil, why"
        ))
        .eval()
        .unwrap();
    assert!(cleared);
    assert_eq!(replaced, "a construction the room cannot name");
}

/// The player's bus station 77 as the game has it, standing at (80, 0, 0):
/// its own entrance edge 6000 and node 6001, and no town building. In a
/// world of the stand-in engine state, whose constructions any test can
/// add to.
const FAKE_STATION: &str = r#"
api.type.ComponentType.CONSTRUCTION = 2
CONSTRUCTIONS = { [77] = { fileName = '::/stations/street/modular_street_station/modular_terminal.con',
    transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 80,0,0,1 }, townBuildings = {},
    frozenEdges = { 6000 }, frozenNodes = { 6001 }, params = { seed = 7, length = 2 } } }
local get = api.engine.getComponent
api.engine.getComponent = function(e, kind)
    if kind == 2 then return CONSTRUCTIONS[e] end
    if get then return get(e, kind) end
end
api.engine.getEntitiesWithComponent = function(kind)
    local l = {}
    if kind == 2 then for e in pairs(CONSTRUCTIONS) do l[#l + 1] = e end end
    table.sort(l)
    return l
end
api.engine.util = api.engine.util or {}
api.engine.util.getEntityName = function(e)
    if e == 77 then return 'Okehampton Station' end
end
"#;

/// The station 77 of FAKE_STATION given a longer platform, as an edit of its
/// modules or parameters proposes it (docs/BUILDING.md: the old construction
/// in `toRemove`, the new one in `toAdd`, same file, new parameters): its
/// own entrance removed and made again.
const EDIT_PROPOSAL: &str = "{ toRemove = { 77 }, \
    toAdd = { { fileName = '::/stations/street/modular_street_station/modular_terminal.con', \
                name = 'Okehampton Station', playerEntity = 25, \
                transf = { 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 80, 0, 0, 1 }, \
                params = { seed = 7, length = 3, modules = { [12] = { name = 'station/platform.module' } } } } }, \
    proposal = { addedNodes = { { entity = -1, comp = { position = { x = 70, y = 0, z = 0 } } } }, \
                 addedSegments = { { entity = -2, type = 0, comp = { node0 = -1, node1 = 7 } } }, \
                 removedSegments = { { entity = 6000, type = 0, comp = { node0 = 6001, node1 = 7 } } }, \
                 removedNodes = { { entity = 6001, comp = { position = { x = 70, y = 0, z = 0 } } } }, \
                 edgeObjectsToAdd = {} } }";

#[test]
fn an_edit_of_a_station_travels_with_the_station_it_replaces() {
    let (lua, _script) = engine();
    lua.load(FAKE_STATION).exec().unwrap();
    let (carried, ok): (String, bool) = lua
        .load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local action, why = capture.construction({EDIT_PROPOSAL}) \
             if not action then error(why) end \
             local b = action.BuildConstruction \
             local length \
             for _, p in ipairs(b.params) do if p.key == 'length' then length = p.value.Int end end \
             return table.concat({{ b.replaces.file, b.replaces.at.x, b.replaces.at.y, b.replaces.at.z, \
                 b.file, b.name, length, tostring(b.connection) }}, '|'), schema_check(action)"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        carried,
        "::/stations/street/modular_street_station/modular_terminal.con|80|0|0\
         |::/stations/street/modular_street_station/modular_terminal.con|Okehampton Station|3|nil",
        "the station it replaces by its file and place, the new parameters, no connection"
    );
    assert!(ok, "the schema takes it");
    // An edit whose proposal leaves the name out keeps the station's; the
    // bulldozer's proposal of the same shape is the same edit.
    let (named, bulldozed): (String, String) = lua
        .load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local unnamed = {EDIT_PROPOSAL} unnamed.toAdd[1].name = '' \
             local b = capture.bulldoze({EDIT_PROPOSAL}) \
             return capture.construction(unnamed).BuildConstruction.name, b.BuildConstruction.replaces.file"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(named, "Okehampton Station");
    assert_eq!(
        bulldozed,
        "::/stations/street/modular_street_station/modular_terminal.con"
    );
    // What an edit the room cannot carry says.
    let refusals: Vec<String> = lua
        .load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             CONSTRUCTIONS[78] = {{ fileName = 'depot/road_depot.con', transf = {{ 1,0,0,0, 0,1,0,0, 0,0,1,0, 9,9,0,1 }}, \
                 townBuildings = {{}} }} \
             CONSTRUCTIONS[79] = {{ townBuildings = {{}} }} \
             local out = {{}} \
             local function why(p) local _, r = capture.construction(p) out[#out + 1] = r end \
             local two = {EDIT_PROPOSAL} two.toRemove = {{ 77, 78 }} why(two) \
             local nameless = {EDIT_PROPOSAL} nameless.toRemove = {{ 79 }} why(nameless) \
             local nothing = {EDIT_PROPOSAL} nothing.toRemove = {{ 80 }} why(nothing) \
             local road = {EDIT_PROPOSAL} \
             road.proposal.removedSegments[2] = {{ entity = 100, type = 0, comp = {{ node0 = 8, node1 = 9 }} }} \
             why(road) \
             CONSTRUCTIONS[5618] = {{ townBuildings = {{ 9001 }} }} \
             local town = {EDIT_PROPOSAL} town.toRemove = {{ 5618 }} town.proposal.removedSegments = {{}} \
             town.proposal.removedNodes = {{}} \
             local _, b = capture.bulldoze(town) \
             out[#out + 1] = b \
             return out"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        refusals,
        [
            "a construction that replaces more than one",
            "a construction the room cannot name",
            "removing something that is no construction",
            "a construction edit that changes the streets around it: roadTemplate is not a resource name: nil",
            "a bulldozer proposal that builds"
        ]
    );
}

#[test]
fn station_edits_include_unfrozen_track_ends_but_not_external_connections() {
    let (lua, _script) = engine();
    lua.load(FAKE_STATION).exec().unwrap();
    // Steam 40408, 2026-10-01: the two-track station's edit removes 50
    // nodes and 48 edges, but only 46 nodes are frozen. Each of its four
    // unfrozen track ends has one incident edge, frozen in the station.
    // Keep only that boundary here; native module capture supplies IDs,
    // without the removed nodes' or edges' components.
    lua.load(
        "CONSTRUCTIONS[77].frozenNodes = { 7522, 7846, 7848, 7871 } \
         CONSTRUCTIONS[77].frozenEdges = { 7873, 7896, 7897, 7920 } \
         INCIDENT = { [7824] = {7873}, [7847] = {7896}, [7849] = {7897}, [7872] = {7920} } \
         api.engine.system.streetSystem = api.engine.system.streetSystem or {} \
         api.engine.system.streetSystem.getNodeSegments = function(n) return INCIDENT[n] end",
    )
    .exec()
    .unwrap();
    let outcomes: Vec<bool> = lua
        .load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local p = {EDIT_PROPOSAL} \
             p.proposal.removedSegments = {{ {{entity=7873}}, {{entity=7896}}, {{entity=7897}}, {{entity=7920}} }} \
             p.proposal.removedNodes = {{ {{entity=7824}}, {{entity=7847}}, {{entity=7849}}, {{entity=7872}} }} \
             local out = {{}} \
             local function check() local a = capture.construction(p) out[#out+1] = a ~= nil end \
             check() \
             INCIDENT[7847] = {{7896, 100}} check() \
             INCIDENT[7847] = {{100}} check() \
             INCIDENT[7847] = {{}} check() \
             INCIDENT[7847] = nil check() \
             INCIDENT[7847] = {{7896}} \
             table.remove(p.proposal.removedSegments, 2) check() \
             table.insert(p.proposal.removedSegments, 2, {{entity=7896}}) \
             api.engine.system.streetSystem.getNodeSegments = function() error('unavailable') end check() \
             api.engine.system.streetSystem.getNodeSegments = nil check() \
             return out"
        ))
        .eval()
        .unwrap();
    assert_eq!(
        outcomes,
        [true, false, false, false, false, false, false, false],
        "only endpoints attached exclusively to this construction's removed edges travel"
    );
}

#[test]
fn a_station_edit_a_click_saw_goes_to_the_room_and_unhandled_events_are_logged() {
    let (lua, _script) = engine();
    lua.load(FAKE_STATION).exec().unwrap();
    lua.load(format!(
        "HOOK.room = true HOOK.clicks = 0 \
         SCRIPT.guiUpdate({{}}, nil, nil) \
         R = SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'constructionBuilder', 'builder.proposalCreate', \
             {{ {EDIT_PROPOSAL} }}) \
         HOOK.clicks = 1 SCRIPT.guiUpdate({{}}, nil, nil) \
         R2 = SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'moduleBuilder', 'builder.proposalCreate', \
             {{ {EDIT_PROPOSAL} }}) \
         HOOK.clicks = 2 SCRIPT.guiUpdate({{}}, nil, nil)"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let (refused, handed, file): (bool, usize, String) = lua
        .load(
            "return R ~= nil or R2 ~= nil, #HOOK.commands, \
                 HOOK.commands[2].BuildConstruction.replaces.file",
        )
        .eval()
        .unwrap();
    assert!(!refused, "neither tool is told no");
    assert_eq!(handed, 2, "each click's edit went to the room");
    assert_eq!(
        file,
        "::/stations/street/modular_street_station/modular_terminal.con"
    );
    // A click with no proposal before it, as the module editor's on build
    // 40408, is stopped and says so.
    lua.load("HOOK.clicks = 3 SCRIPT.guiUpdate({}, nil, nil)")
        .exec()
        .unwrap();
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged.iter().any(|l| l.starts_with(
            "stopped a build the room cannot carry: no proposal seen (a tool that tells game scripts nothing"
        )),
        "{logged:?}"
    );
    // Events the script does not handle, in the room's game: each id and
    // name once, a few dozen at most; outside it, none.
    lua.load(
        "SCRIPT.guiHandleEvent({}, nil, nil, '', 'someWindow', 'select', {}) \
         SCRIPT.guiHandleEvent({}, nil, nil, '', 'someWindow', 'select', {}) \
         SCRIPT.guiHandleEvent({}, nil, nil, '', 'moduleThing', 'builder.proposalCreate', {}) \
         for i = 1, 60 do SCRIPT.guiHandleEvent({}, nil, nil, '', 'window' .. i, 'idAdded', {}) end \
         HOOK.room = false \
         SCRIPT.guiHandleEvent({}, nil, nil, '', 'elsewhere', 'select', {})",
    )
    .exec()
    .unwrap();
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    let unhandled: Vec<&String> = logged
        .iter()
        .filter(|l| l.starts_with("an event the mod does not handle: "))
        .collect();
    assert_eq!(unhandled.len(), 40, "{unhandled:?}");
    assert_eq!(
        unhandled[0],
        "an event the mod does not handle: id someWindow, name select"
    );
    assert_eq!(
        unhandled[1],
        "an event the mod does not handle: id moduleThing, name builder.proposalCreate"
    );
    assert!(!logged.iter().any(|l| l.contains("elsewhere")));
}

/// What the fake hook was handed as the player's previews: each one's
/// action kind, or "none".
fn previewed(lua: &Lua) -> Vec<String> {
    lua.load(
        "local out = {} \
         for i, p in ipairs(HOOK.previewed or {}) do \
             out[i] = p == 'none' and 'none' or next(p) \
         end \
         return out",
    )
    .eval()
    .unwrap()
}

#[test]
fn the_players_build_preview_goes_to_the_room_until_the_tool_shows_nothing() {
    let (lua, _script) = engine();
    lua.load(FAKE_STATION).exec().unwrap();
    lua.load(format!(
        "HOOK.room = true HOOK.clicks = 0 \
         SCRIPT.guiUpdate({{}}, nil, nil) \
         SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'constructionBuilder', 'builder.proposalCreate', \
             {{ {CONSTRUCTION_PROPOSAL} }})"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(previewed(&lua), ["BuildConstruction"]);
    // The bulldozer's removals show nothing new: what showed is hidden.
    lua.load(
        "SCRIPT.guiHandleEvent({}, nil, nil, '', 'bulldozer', 'builder.proposalCreate', \
             { { proposal = { addedNodes = {}, addedSegments = {}, removedNodes = {}, \
                 removedSegments = {}, edgeObjectsToAdd = {} }, toRemove = {}, toAdd = {} } })",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(previewed(&lua), ["BuildConstruction", "none"]);
    // Shown again, then clicked: the room orders the build, and the preview
    // goes.
    lua.load(format!(
        "SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'constructionBuilder', 'builder.proposalCreate', \
             {{ {CONSTRUCTION_PROPOSAL} }}) \
         HOOK.clicks = 1 SCRIPT.guiUpdate({{}}, nil, nil)"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        previewed(&lua),
        ["BuildConstruction", "none", "BuildConstruction", "none"]
    );
    let handed: usize = lua.load("return #HOOK.commands").eval().unwrap();
    assert_eq!(
        handed, 1,
        "the click's build, and no preview, went to the room"
    );
}

#[test]
fn a_preview_hides_once_its_tool_is_no_longer_active() {
    let (lua, _script) = engine();
    lua.load(FAKE_STATION).exec().unwrap();
    // Build 40408 names the active tools by their window, never by the
    // event's id: the list changing is the tool closing.
    lua.load(format!(
        "ACTIVE = {{ 'construction-menu-stations', 'Construction' }} \
         api.gui = {{ contextHelper = {{ getIdsOfActiveTool = function() return ACTIVE end }} }} \
         HOOK.room = true HOOK.clicks = 0 \
         SCRIPT.guiUpdate({{}}, nil, nil) \
         SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'constructionBuilder', 'builder.proposalCreate', \
             {{ {CONSTRUCTION_PROPOSAL} }}) \
         SCRIPT.guiUpdate({{}}, nil, nil)"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(previewed(&lua), ["BuildConstruction"], "still active");
    // The same tools in another order: still the same tool.
    lua.load(
        "ACTIVE = { 'Construction', 'construction-menu-stations' } \
         local t0 = os.clock() \
         while os.clock() - t0 < 0.3 do end \
         SCRIPT.guiUpdate({}, nil, nil)",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(previewed(&lua), ["BuildConstruction"], "still active");
    // The player closed the tool; the next look at the list, a quarter of a
    // second on, hides it.
    lua.load(
        "ACTIVE = {} \
         local t0 = os.clock() \
         while os.clock() - t0 < 0.3 do end \
         SCRIPT.guiUpdate({}, nil, nil)",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(previewed(&lua), ["BuildConstruction", "none"]);
}

#[test]
fn a_preview_the_hook_could_not_draw_is_drawn_once_a_renderer_frees() {
    let (lua, _script) = engine();
    // All the hook's renderers busy at first; one frees later. The member's
    // preview does not change, so it never comes again as a change.
    let (first, later, tries): (usize, usize, usize) = lua
        .load(
            "local previews = ug_require('tpf3mp_1::/scripts/tpf3mp/previews.lua') \
             previews.reset() \
             local incoming = { { from = string.rep('ab', 32), action = { BuildTrack = {} } } } \
             local link = { previews = function() local c = incoming incoming = {} return c end, \
                            log = function() end } \
             local full, tries, drawn = true, 0, 0 \
             local function make() return { track = true }, {}, 1 end \
             local function draw(from, kept) \
                 if kept == nil then return true end \
                 tries = tries + 1 \
                 if full then return nil, 'every renderer is busy' end \
                 drawn = drawn + 1 return true \
             end \
             previews.take(link, make, draw) \
             previews.take(link, make, draw) \
             local first = drawn \
             full = false \
             local t0 = os.clock() while os.clock() - t0 < 0.6 do end \
             previews.take(link, make, draw) \
             previews.take(link, make, draw) \
             return first, drawn, tries",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(first, 0, "nothing drawn while every renderer is busy");
    assert_eq!(later, 1, "drawn once one frees, and only once");
    assert!(tries <= 3, "tried again at most every half second: {tries}");
}

#[test]
fn what_this_game_says_of_a_preview_goes_to_the_log_as_it_changes() {
    let (lua, _script) = engine();
    // The hook drew each one and answered the game's ProposalData: a track
    // the game calls critical, the same again, a depot colliding with a
    // street, then one with nothing to say.
    let logged: Vec<String> = lua
        .load(
            "local previews = ug_require('tpf3mp_1::/scripts/tpf3mp/previews.lua') \
             previews.reset() \
             local logged, incoming = {}, {} \
             local link = { previews = function() local c = incoming incoming = {} return c end, \
                            log = function(_, line) logged[#logged + 1] = line end } \
             local data \
             local function make() return {}, {}, 1 end \
             local function draw(from, kept) return true, data end \
             local from = string.rep('ab', 32) \
             local function show(action, d) \
                 data = d \
                 incoming = { { from = from, action = action } } \
                 previews.take(link, make, draw) \
             end \
             local critical = { errorState = { critical = true, messages = { 'Construction not possible' }, \
                                               warnings = {} } } \
             show({ BuildTrack = { n = 1 } }, critical) \
             show({ BuildTrack = { n = 2 } }, critical) \
             show({ BuildConstruction = {} }, { errorState = { critical = false, messages = { 'Collision' } }, \
                 collisionInfo = { collisionEntities = { { entity = 39949 } } } }) \
             show({ BuildConstruction = { n = 2 } }, {}) \
             local said = {} \
             for _, line in ipairs(logged) do \
                 if line:find('as this game sees it', 1, true) then said[#said + 1] = line end \
             end \
             return said",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        logged,
        [
            "another member's BuildTrack preview, as this game sees it: critical, errors Construction not possible",
            "another member's BuildConstruction preview, as this game sees it: errors Collision, collides with 1 entity (39949)",
            "another member's BuildConstruction preview, as this game sees it: fine",
        ],
        "said when it changes, not again for the same"
    );
}

#[test]
fn a_dry_run_makes_a_builds_proposal_and_sends_nothing() {
    let (lua, _script) = engine();
    lua.load(FAKE_STATION).exec().unwrap();
    lua.load(format!(
        "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua')          local apply = ug_require('tpf3mp_1::/scripts/tpf3mp/apply.lua')          apply.log = function(line) HOOK.logged[#HOOK.logged + 1] = line end          local action = assert(capture.construction({CONSTRUCTION_PROPOSAL}))          P, C = apply.proposalOf(action, {{ company = 31 }})          NOT, WHY = apply.proposalOf({{ Bulldoze = {{}} }}, {{}})"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}
{}", log(&lua)));
    let (file, player, sent, logged): (String, i64, usize, usize) = lua
        .load("return P.constructionsToAdd[1].fileName, C.player, #SENT, #HOOK.logged")
        .eval()
        .unwrap_or_else(|error| {
            panic!(
                "{error}
{}",
                log(&lua)
            )
        });
    assert!(file.ends_with(".con"), "{file}");
    assert_eq!(player, 31, "built for the sender's company");
    assert_eq!((sent, logged), (0, 0), "nothing sent, nothing said");
    let why: String = lua
        .load("return tostring(NOT) .. ' ' .. WHY")
        .eval()
        .unwrap();
    assert_eq!(why, "nil no preview of Bulldoze");
    // The next action applies as before: the dry run left nothing behind.
    lua.load(STATION_REFRESH).exec().unwrap();
    lua.load(format!(
        "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua')          local apply = ug_require('tpf3mp_1::/scripts/tpf3mp/apply.lua')          OK = apply.run(assert(capture.construction({CONSTRUCTION_PROPOSAL})), {{}})"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}
{}", log(&lua)));
    let (ok, sent): (bool, usize) = lua.load("return OK, #SENT").eval().unwrap();
    assert!(ok && sent == 1, "{}", log(&lua));
}

#[test]
fn the_plugin_has_the_hook_draw_each_other_members_preview_and_mounts_nothing() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    // The proposal a preview makes here, as apply.proposalOf would make it,
    // and the game's evaluation of it, which the hook draws while armed.
    lua.load(
        "api.engine = { util = { getPlayer = function() return 25 end, proposal = { \
             makeProposalData = function(proposal, context) \
                 if HOOK.armed then HOOK.evaluated = HOOK.armed:sub(1, 2) .. ' ' \
                     .. tostring(proposal.track) .. ' for ' .. tostring(context.player) end \
                 return {} \
             end } } } \
         package.loaded['tpf3mp.apply'] = { proposalOf = function(action) \
             if action.BuildTrack then return { track = true }, { player = 25 } end \
             return nil, 'this game has no such street' \
         end } \
         HOOK.room = true",
    )
    .exec()
    .unwrap();
    lua.load(
        "M = mount(loadPlugin()) M.step() M.render() \
         HOOK.incoming = { { from = string.rep('ab', 32), action = { BuildTrack = {} } }, \
                           { from = string.rep('cd', 32), action = { BuildRoad = {} } } } \
         M.step() M.render()",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let (kept, mounted): (String, usize) = lua
        .load(
            "local kept = {} \
             for from, p in pairs(require('tpf3mp.previews').remote()) do \
                 kept[#kept + 1] = from:sub(1, 2) .. ' ' .. p.kind .. ' ' .. tostring(p.proposal.track) \
             end \
             local mounted = 0 \
             for _, child in ipairs(M.layout.params.children) do \
                 if child.view == 'ProposalViewer' then mounted = mounted + 1 end \
             end \
             return table.concat(kept, ','), mounted",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        kept, "ab BuildTrack true",
        "the track kept; the road this game cannot make not"
    );
    assert_eq!(
        mounted, 0,
        "no ProposalViewer outside a tool: build 40408 fails fatally (!IsTransformWithContext)"
    );
    let drawing: Vec<String> = lua.load("return HOOK.drawing").eval().unwrap();
    assert_eq!(
        drawing,
        ["drew ab true for 25"],
        "the hook drew the track, as evaluated"
    );
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    for line in [
        "another member's build preview arrived: BuildTrack from abababababababab",
        "drawing another member's build preview: BuildTrack from abababababababab",
        "another member's BuildRoad preview does not show here: this game has no such street",
    ] {
        assert!(logged.iter().any(|l| l == line), "{line}: {logged:?}");
    }
    // The member's tool shows nothing now: the hook clears it.
    lua.load("HOOK.incoming = { { from = string.rep('ab', 32) } } M.step() M.render()")
        .exec()
        .unwrap();
    let drawing: Vec<String> = lua.load("return HOOK.drawing").eval().unwrap();
    assert_eq!(drawing, ["drew ab true for 25", "undrew ab"]);
    let left: usize = lua
        .load("local n = 0 for _ in pairs(require('tpf3mp.previews').remote()) do n = n + 1 end return n")
        .eval()
        .unwrap();
    assert_eq!(left, 0);
}

#[test]
fn a_module_editor_click_goes_to_the_room_as_the_hook_read_it() {
    let (lua, _script) = engine();
    lua.load(FAKE_STATION).exec().unwrap();
    // A construction tool preview before the click, then the module
    // editor's click, whose build only the hook saw: the hook's wins.
    lua.load(format!(
        "HOOK.room = true HOOK.clicks = 0          SCRIPT.guiUpdate({{}}, nil, nil)          SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'constructionBuilder', 'builder.proposalCreate',              {{ {CONSTRUCTION_PROPOSAL} }})          local edit = {EDIT_PROPOSAL} edit.toAdd[1].name = ''          HOOK.built[0] = {{ proposal = edit }}          HOOK.clicks = 1 SCRIPT.guiUpdate({{}}, nil, nil)          HOOK.built[1] = {{ why = 'the matrix does not read' }}          HOOK.clicks = 2 SCRIPT.guiUpdate({{}}, nil, nil)          local nothing = {EDIT_PROPOSAL} nothing.toRemove = {{}}          nothing.proposal.removedSegments = {{}} nothing.proposal.removedNodes = {{}}          HOOK.built[2] = {{ proposal = nothing }}          HOOK.clicks = 3 SCRIPT.guiUpdate({{}}, nil, nil)"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let (handed, carried): (usize, String) = lua
        .load(
            "local b = HOOK.commands[1].BuildConstruction              return #HOOK.commands, table.concat({ b.file, b.name, b.replaces.file, b.replaces.at.x }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        handed, 1,
        "the edit, not the stale preview, and nothing else"
    );
    assert_eq!(
        carried,
        "::/stations/street/modular_street_station/modular_terminal.con|Okehampton Station|::/stations/street/modular_street_station/modular_terminal.con|80"
    );
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    for line in [
        "handed the player's build to the room [module editor]",
        "stopped a build the room cannot carry: the module editor's edit did not read: the matrix does not read [module editor]",
        "stopped a build the room cannot carry: the module editor's edit: an edit that replaces no construction [module editor]",
    ] {
        assert!(logged.iter().any(|l| l == line), "{line}: {logged:?}");
    }
}

#[test]
fn every_game_replaces_the_edited_station_in_one_proposal() {
    let (lua, _script) = engine();
    lua.load(FAKE_STATION).exec().unwrap();
    // This game's station has its own entity, 910, where the action says;
    // the game's verdict is asked first, and a build replaces constructions
    // as the game would.
    lua.load(format!(
        "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         ACTION = capture.construction({EDIT_PROPOSAL}) \
         CONSTRUCTIONS[910] = CONSTRUCTIONS[77] CONSTRUCTIONS[77] = nil \
         api.type.ComponentType.PLAYER_OWNED = 15 OWNERS = {{ [910] = 25 }} \
         local getComponent = api.engine.getComponent \
         api.engine.getComponent = function(e, kind) \
             if kind == 15 then return OWNERS[e] and {{ player = OWNERS[e] }} end \
             return getComponent(e, kind) end \
         ASKED = {{}} \
         api.engine.util.proposal = {{ makeProposalData = function(p, context) \
             ASKED[#ASKED + 1] = {{ sent = #SENT, removes = p.constructionsToRemove[1] }} \
             return {{ errorState = {{ critical = false, messages = {{}} }} }} end, \
             refreshConstruction = function(e) return REFRESH(e) end }} \
         REFRESH = function(e) return {{ refreshed = e, \
             proposal = {{ addedSegments = {{ {{ entity = -2, comp = {{ node0 = -1, node1 = 7777 }} }} }}, \
                          removedSegments = {{ {{ entity = 6000 }} }} }} }} end \
         local send = api.cmd.sendCommand \
         api.cmd.sendCommand = function(cmd, ...) \
             local p = cmd.proposal \
             for _, e in ipairs(p and p.constructionsToRemove or {{}}) do CONSTRUCTIONS[e] = nil end \
             local c = p and p.constructionsToAdd and p.constructionsToAdd[1] \
             if c then CONSTRUCTIONS[911] = {{ fileName = c.fileName, \
                 transf = {{ 1,0,0,0, 0,1,0,0, 0,0,1,0, c.transf[4][1], c.transf[4][2], c.transf[4][3], 1 }} }} \
                 OWNERS[911] = 25 end \
             return send(cmd, ...) \
         end \
         HOOK.batch = {{ ACTION }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let built: String = lua
        .load(
            "local c = SENT[1] local p = c.proposal local e = p.constructionsToAdd[1] \
             return table.concat({ #SENT, #ASKED, ASKED[1].sent, ASKED[1].removes, \
                 #p.constructionsToRemove, p.constructionsToRemove[1], p.old2new[910], \
                 e.fileName, e.name, e.params.length, e.params.modules[12].name, e.playerEntity, \
                 tostring(c.context.player), tostring(c.context.gatherBuildings), \
                 tostring(c.ignoreErrors), tostring(c.playerInitiated), \
                 tostring(HOOK.applied[1].ok), tostring(CONSTRUCTIONS[910]), tostring(CONSTRUCTIONS[911] ~= nil) }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        built,
        "2|1|0|910|1|910|0\
         |::/stations/street/modular_street_station/modular_terminal.con|Okehampton Station|3\
         |station/platform.module|25|25|true|true|true|true|nil|true",
        "the verdict, then one proposal removing this game's station and adding the new one, \
         mapped old to new, as the player's own build"
    );
    // Then the new station's refresh, which snaps its entrance onto the
    // street again, free, as no player's click: a road station edited by
    // the street came loose from it (2026-10-03).
    let refreshed: String = lua
        .load(
            "local r = SENT[2] return table.concat({ r.proposal.refreshed, tostring(r.context), \
                 tostring(r.ignoreErrors), tostring(r.playerInitiated) }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(refreshed, "911|nil|true|false");
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged
            .iter()
            .any(|l| l == "snapping 911 +e-2:-1>7777 -e6000"),
        "{logged:?}"
    );
    // The next edit finds the new station where the old one stood. Its
    // refresh has nothing to snap: nothing more is sent.
    lua.load(
        "SENT = {} REFRESH = function(e) return { refreshed = e, \
             proposal = { addedSegments = {}, removedSegments = {} } } end \
         HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let again: String = lua
        .load("return #SENT .. '|' .. SENT[1].proposal.constructionsToRemove[1] .. '|' .. tostring(HOOK.applied[2].ok)")
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(again, "1|911|true");
    // A refresh the game refuses leaves the edit standing, the same in
    // every game, and says so in the log.
    lua.load(
        "SENT = {} REFRESH = function(e) error('Construction Not Possible') end \
         HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let refused: String = lua
        .load("return #SENT .. '|' .. tostring(HOOK.applied[3].ok) .. '|' .. tostring(CONSTRUCTIONS[911] ~= nil)")
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(refused, "1|true|true");
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged.iter().any(
            |l| l.starts_with("the edited construction stays unsnapped: ")
                && l.contains("Construction Not Possible")
        ),
        "{logged:?}"
    );
    // A station that is not there is refused with why, and nothing is sent.
    lua.load("SENT = {} CONSTRUCTIONS = {} HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let (sent, ok, why): (usize, bool, String) = lua
        .load("return #SENT, HOOK.applied[4].ok, HOOK.applied[4].why")
        .eval()
        .unwrap();
    assert_eq!(sent, 0);
    assert!(!ok);
    assert_eq!(
        why,
        "no ::/stations/street/modular_street_station/modular_terminal.con there"
    );
}

#[test]
fn specialised_rail_platform_edits_use_the_native_replacement_and_validate_it() {
    for local_id in [77, 910] {
        let (lua, _) = engine();
        lua.load(FAKE_STATION).exec().unwrap();
        lua.globals().set("LOCAL_ID", local_id).unwrap();
        lua.load(format!("EDIT = {EDIT_PROPOSAL}")).exec().unwrap();
        lua.load(include_str!("lua/rail_platform_edit.lua"))
            .set_name("@rail_platform_edit.lua")
            .exec()
            .unwrap_or_else(|error| panic!("local entity {local_id}: {error}"));
    }
}

/// The station 77 by FAKE_NETWORK's node 7: its own entrance, edge 6000
/// from its street node 6001 to node 7, frozen in it. As the game, a built
/// construction is listed, and a new one takes entity 911, the acting
/// company's (25).
const STATION_AT_NODE_7: &str = r#"
api.type.ComponentType.CONSTRUCTION = 2
api.type.ComponentType.PLAYER_OWNED = 14
NODES[6001] = { x = 70, y = 0, z = 0 }
EDGES[6000] = { node0 = 6001, node1 = 7, tangent0 = { x = -70, y = 0, z = 0 },
    tangent1 = { x = -70, y = 0, z = 0 }, objects = {},
    roadTemplate = '::/street/town_small.street_template' }
STREETS[7] = { 101, 6000 }
STREETS[6001] = { 6000 }
OWNERS = {}
CONSTRUCTIONS = { [77] = { fileName = '::/stations/street/modular_street_station/modular_terminal.con',
    transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 80,0,0,1 }, townBuildings = {},
    frozenEdges = { 6000 }, frozenNodes = { 6001 }, params = { seed = 7, length = 2 } } }
local get = api.engine.getComponent
api.engine.getComponent = function(e, kind)
    if kind == 2 then return CONSTRUCTIONS[e] end
    if kind == 14 then return OWNERS[e] and { player = OWNERS[e] } end
    return get(e, kind)
end
api.engine.getEntitiesWithComponent = function(kind)
    local l = {}
    if kind == 2 then for e in pairs(CONSTRUCTIONS) do l[#l + 1] = e end end
    table.sort(l)
    return l
end
api.engine.util.getEntityName = function(e) if e == 77 then return 'Okehampton Station' end end
local send = api.cmd.sendCommand
api.cmd.sendCommand = function(cmd, ...)
    local p = cmd.proposal
    for _, e in ipairs(p and p.constructionsToRemove or {}) do CONSTRUCTIONS[e] = nil end
    local c = p and p.constructionsToAdd and p.constructionsToAdd[1]
    if c then CONSTRUCTIONS[911] = { fileName = c.fileName,
        transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, c.transf[4][1], c.transf[4][2], c.transf[4][3], 1 } }
        OWNERS[911] = 25 end
    return send(cmd, ...)
end
REFRESH = function(e) return { refreshed = e,
    proposal = { addedSegments = { { entity = -2, comp = { node0 = -1, node1 = 7 } } },
                 removedSegments = { { entity = 6100 } } } } end
REGENERATED = 0
api.engine.util.proposal = {
    refreshConstruction = function(e) return REFRESH(e) end,
    createProposalReplaceConstruction = function(e, params)
        REGENERATED = REGENERATED + 1
        return FULL(e, params)
    end,
}
"#;

/// Station 77 given a second exit at its other end, onto the road {ROAD}
/// (FAKE_NETWORK's 100, 8-9, or 101, 10-7), as the game proposes it: its
/// own entrance made again (-1 to node 7, 6000 and 6001 removed), its exit
/// node -2, and the road split through a new junction -3 the exit joins
/// (seen on build 40408, 2026-10-03). `{EXTRA}` adds to the lists.
const EDIT_SPLIT: &str = "{ toRemove = { 77 }, \
    toAdd = { { fileName = '::/stations/street/modular_street_station/modular_terminal.con', \
                name = 'Okehampton Station', \
                transf = { 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 80, 0, 0, 1 }, \
                params = { seed = 7, length = 2, modules = { [12] = { name = 'station/exit.module' } } } } }, \
    proposal = { \
    addedNodes = { { entity = -1, comp = { position = { x = 70, y = 0, z = 0 } } }, \
                   { entity = -2, comp = { position = { X2, Y2, z = 0 } } }, \
                   { entity = -3, comp = { position = { X3, Y3, z = 0 } } } }, \
    addedSegments = { \
        { entity = -4, type = 0, comp = { node0 = -1, node1 = 7, type = 0, typeIndex = -1, \
          tangent0 = { x = -70, y = 0, z = 0 }, tangent1 = { x = -70, y = 0, z = 0 }, \
          roadTemplate = '::/street/town_small.street_template', roadStyle = '' } }, \
        { entity = -5, type = 0, comp = { node0 = -2, node1 = -3, type = 0, typeIndex = -1, \
          tangent0 = { x = 0, y = -10, z = 0 }, tangent1 = { x = 0, y = -10, z = 0 }, \
          roadTemplate = '::/street/town_small.street_template', roadStyle = '' } }, \
        { entity = -6, type = 0, comp = { node0 = A, node1 = -3, type = 0, typeIndex = -1, \
          tangent0 = { TA }, tangent1 = { TA }, \
          roadTemplate = '::/street/country.street_template', roadStyle = '' } }, \
        { entity = -7, type = 0, comp = { node0 = -3, node1 = B, type = 0, typeIndex = -1, \
          tangent0 = { TA }, tangent1 = { TA }, \
          roadTemplate = '::/street/country.street_template', roadStyle = '' } } }, \
    removedSegments = { { entity = 6000, type = 0, comp = { node0 = 6001, node1 = 7 } }, \
                        { entity = ROAD, type = 0, comp = { node0 = A, node1 = B } } {EXTRA} }, \
    removedNodes = { { entity = 6001, comp = { position = { x = 70, y = 0, z = 0 } } } }, \
    edgeObjectsToAdd = {}, edgeObjectsToRemove = {} } }";

/// EDIT_SPLIT onto road `road`, with `extra` in its removed segments.
fn edit_split(road: u32, extra: &str) -> String {
    let (a, b, x2, y2, x3, y3, ta) = if road == 100 {
        (
            8,
            9,
            "x = 50",
            "y = 20",
            "x = 50",
            "y = 10",
            "x = 0, y = 40, z = 0",
        )
    } else {
        (
            10,
            7,
            "x = -30",
            "y = 10",
            "x = -30",
            "y = 0",
            "x = 30, y = 0, z = 0",
        )
    };
    EDIT_SPLIT
        .replace("{EXTRA}", extra)
        .replace("ROAD", &road.to_string())
        .replace("X2", x2)
        .replace("Y2", y2)
        .replace("X3", x3)
        .replace("Y3", y3)
        .replace("TA", ta)
        .replace("node0 = A", &format!("node0 = {a}"))
        .replace("node1 = A", &format!("node1 = {a}"))
        .replace("node0 = B", &format!("node0 = {b}"))
        .replace("node1 = B", &format!("node1 = {b}"))
}

/// The module editor's edit as the hook reads it: the construction, and of
/// the street part what it removes, and how many nodes and edges it adds.
const NATIVE_OF: &str = "function NATIVE_OF(full) \
    local s = full.proposal \
    local function blanks(n) local l = {} for i = 1, n do l[i] = {} end return l end \
    local function ids(l) local out = {} for i, x in ipairs(l) do out[i] = { entity = x.entity } end return out end \
    return { toRemove = full.toRemove, toAdd = full.toAdd, proposal = { \
        addedNodes = blanks(#s.addedNodes), addedSegments = blanks(#s.addedSegments), \
        removedNodes = ids(s.removedNodes), removedSegments = ids(s.removedSegments), \
        edgeObjectsToAdd = blanks(#(s.edgeObjectsToAdd or {})), \
        edgeObjectsToRemove = blanks(#(s.edgeObjectsToRemove or {})) } } \
end";

/// A module editor's edit that changes the streets around its station (a
/// new exit splitting a road, 2026-10-03) is asked of the game again, and
/// travels with its connection when the game proposes the same edit as the
/// hook read: else it is refused, with why. An edit of its own streets only
/// is not asked again.
#[test]
fn a_module_edit_that_splits_a_road_travels_with_the_split() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(STATION_AT_NODE_7).exec().unwrap();
    lua.load(NATIVE_OF).exec().unwrap();
    let split = edit_split(100, "");
    lua.load(format!(
        "capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         FULL = function() return {split} end \
         ACTION, WHY = capture.moduleEdit(NATIVE_OF(FULL()))"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let carried: String = lua
        .load(
            "assert(ACTION, WHY) local b = ACTION.BuildConstruction local c = b.connection \
             local out = { REGENERATED, b.replaces.file, b.replaces.at.x, b.name, #c.links, #c.removals, \
                 tostring(schema_check(ACTION)) } \
             for _, r in ipairs(c.removals) do \
                 out[#out + 1] = r.ends.a.x .. ',' .. r.ends.a.y .. '>' .. r.ends.b.x .. ',' .. r.ends.b.y end \
             return table.concat(out, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        carried,
        "1|::/stations/street/modular_street_station/modular_terminal.con|80|Okehampton Station|4|1|true\
         |50,-40>50,40",
        "the road it splits travels, not the old station's own entrance, which goes with the station"
    );
    // The game proposes another edit than the hook read: refused.
    for (full, why) in [
        (
            edit_split(100, "").replace("{ entity = 100,", "{ entity = 101,"),
            "a construction edit the game proposes otherwise: what it removes",
        ),
        (
            edit_split(
                100,
                ", { entity = 100, type = 0, comp = { node0 = 8, node1 = 9 } }",
            ),
            "a construction edit the game proposes otherwise: what it removes",
        ),
        (
            edit_split(100, "").replace("0, 0, 1, 0, 80, 0, 0, 1", "0, 0, 1, 0, 85, 0, 0, 1"),
            "a construction edit the game proposes otherwise: where it stands",
        ),
        (
            edit_split(100, "").replace(
                "edgeObjectsToAdd = {}",
                "edgeObjectsToAdd = { { entity = -9 } }",
            ),
            "a construction edit with a stop or signal",
        ),
    ] {
        let got: String = lua
            .load(format!(
                "local native = NATIVE_OF({split}) \
                 FULL = function() return {full} end \
                 local a, why = capture.moduleEdit(native) \
                 return tostring(a) .. ' ' .. tostring(why)"
            ))
            .eval()
            .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
        assert_eq!(got, format!("nil {why}"));
    }
    // A game that cannot propose it again: refused as before.
    let got: String = lua
        .load(format!(
            "api.engine.util.proposal.createProposalReplaceConstruction = nil \
             local a, why = capture.moduleEdit(NATIVE_OF({split})) \
             return tostring(a) .. ' ' .. tostring(why)"
        ))
        .eval()
        .unwrap();
    assert_eq!(
        got,
        "nil a construction edit that changes the streets around it"
    );
    // An edit of its own streets only travels as the hook read it.
    let (regenerated, connection): (i64, String) = lua
        .load(format!(
            "REGENERATED = 0 \
             local a = assert(capture.moduleEdit(NATIVE_OF({EDIT_PROPOSAL}))) \
             return REGENERATED, tostring(a.BuildConstruction.connection)"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!((regenerated, connection.as_str()), (0, "nil"));
}

/// Every game builds such an edit in one proposal: the old station removed,
/// the new one added, the road split through the junction its exit meets,
/// without the station's own entrances, which it makes itself; then its
/// refresh snaps them. A road of another company it may not split (D21),
/// and a refused refresh leaves the edit and the split standing.
#[test]
fn every_game_builds_an_edit_with_the_road_it_splits() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(STATION_AT_NODE_7).exec().unwrap();
    lua.load(NATIVE_OF).exec().unwrap();
    let split = edit_split(100, "");
    lua.load(format!(
        "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         FULL = function() return {split} end \
         ACTION = assert(capture.moduleEdit(NATIVE_OF(FULL()))) \
         SAVED = {{}} for k, v in pairs(CONSTRUCTIONS) do SAVED[k] = v end \
         HOOK.batch = {{ ACTION }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let built: String = lua
        .load(
            "local p = SENT[1].proposal local s = p.streetProposal \
             local out = { tostring(HOOK.applied[1].ok), #SENT, p.constructionsToRemove[1], p.old2new[77], \
                 p.constructionsToAdd[1].fileName, #s.nodesToAdd, table.concat(s.edgesToRemove, ','), \
                 tostring(s.nodesToRemove) } \
             for _, e in ipairs(s.edgesToAdd) do out[#out + 1] = e.comp.node0 .. '>' .. e.comp.node1 end \
             out[#out + 1] = tostring(SENT[2].proposal.refreshed) \
             return table.concat(out, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        built,
        "true|2|77|0|::/stations/street/modular_street_station/modular_terminal.con|1|100|nil\
         |8>-3|-3>9|911",
        "one proposal: the station replaced and the road split, its entrances left to it; then its refresh"
    );
    // A road of another company: refused in every game, nothing sent.
    lua.load(
        "SENT = {} CONSTRUCTIONS = {} for k, v in pairs(SAVED) do CONSTRUCTIONS[k] = v end \
         OWNERS[100] = 900 HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let (sent, ok, why): (usize, bool, String) = lua
        .load("return #SENT, HOOK.applied[2].ok, HOOK.applied[2].why")
        .eval()
        .unwrap();
    assert_eq!((sent, ok), (0, false));
    assert!(
        why.ends_with("the road or track belongs to another company"),
        "{why}"
    );
    // A refresh the game refuses: the edit and the split stand.
    lua.load(
        "SENT = {} CONSTRUCTIONS = {} for k, v in pairs(SAVED) do CONSTRUCTIONS[k] = v end \
         OWNERS[100] = nil REFRESH = function() error('Construction Not Possible') end \
         HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let (sent, ok): (usize, bool) = lua.load("return #SENT, HOOK.applied[3].ok").eval().unwrap();
    assert_eq!((sent, ok), (1, true));
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged
            .iter()
            .any(|l| l.starts_with("the edited construction stays unsnapped: ")),
        "{logged:?}"
    );
}

/// A road split next to the junction the old station's entrance meets: the
/// settings kept for that junction would name the old entrance, which goes
/// with the old station. So that junction keeps none; the new station and
/// its refresh give it the game's own, the same in every game.
#[test]
fn an_edit_leaves_the_junction_at_its_old_entrance_to_the_station() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(STATION_AT_NODE_7).exec().unwrap();
    lua.load(NATIVE_OF).exec().unwrap();
    let split = edit_split(101, "");
    lua.load(format!(
        "CONFIGS[7] = true \
         local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         FULL = function() return {split} end \
         ACTION = assert(capture.moduleEdit(NATIVE_OF(FULL()))) \
         SAVED = {{}} for k, v in pairs(CONSTRUCTIONS) do SAVED[k] = v end \
         HOOK.batch = {{ ACTION }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let built: String = lua
        .load(
            "local s = SENT[1].proposal.streetProposal \
             local added = {} for _, n in ipairs(s.nodeConfigsToAdd or {}) do added[#added + 1] = n.entity end \
             return table.concat({ tostring(HOOK.applied[1].ok), table.concat(s.edgesToRemove, ','), \
                 table.concat(s.nodeConfigsToRemove or {}, ','), table.concat(added, ',') }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(built, "true|101|7|");
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged.iter().any(
            |l| l == "left to the construction: the settings of 1 junction(s) at its old edges"
        ),
        "{logged:?}"
    );
    // Another company's road at that junction: its settings are not this
    // company's to drop (D21), and nothing is sent.
    lua.load(
        "SENT = {} CONSTRUCTIONS = {} for k, v in pairs(SAVED) do CONSTRUCTIONS[k] = v end \
         NODES[12] = { x = 0, y = -60, z = 0 } \
         EDGES[102] = { node0 = 7, node1 = 12, tangent0 = { x = 0, y = -60, z = 0 }, \
             tangent1 = { x = 0, y = -60, z = 0 }, objects = {}, \
             roadTemplate = '::/street/town_small.street_template' } \
         STREETS[7] = { 101, 6000, 102 } STREETS[12] = { 102 } OWNERS[102] = 900 \
         HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let (sent, ok, why): (usize, bool, String) = lua
        .load("return #SENT, HOOK.applied[2].ok, HOOK.applied[2].why")
        .eval()
        .unwrap();
    assert_eq!((sent, ok), (0, false));
    assert!(
        why.ends_with("the junction edge belongs to another company"),
        "{why}"
    );
}

#[test]
fn a_construction_edited_in_its_window_goes_to_the_room() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load(
        "api.cmd.makeWorldBuildProposalCmd = function(proposal, context, ignoreErrors, playerInitiated) \
             return { kind = 'build', proposal = proposal } end \
         api.type = { ComponentType = {} } api.engine = { util = {} }",
    )
    .exec()
    .unwrap();
    lua.load(FAKE_STATION).exec().unwrap();
    lua.load("M = mount(loadPlugin()) M.step() HOOK.room = true")
        .exec()
        .unwrap();
    // The construction menu's parameters, as it sends them: the game's
    // replacement proposal, no context, playerInitiated.
    lua.load(format!(
        "CALLED = nil \
         api.cmd.sendCommand(api.cmd.makeWorldBuildProposalCmd({EDIT_PROPOSAL}, nil, false, true), \
             function(data, ok) CALLED = ok end) \
         api.cmd.sendCommand(api.cmd.makeWorldBuildProposalCmd({CONSTRUCTION_PROPOSAL}, nil, false, true)) \
         M.step()"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let (sent, handed, replaces): (usize, usize, String) = lua
        .load("return #SENT, #HOOK.commands, HOOK.commands[1].BuildConstruction.replaces.file")
        .eval()
        .unwrap();
    assert_eq!(sent, 0, "neither is built here");
    assert_eq!(handed, 1, "the edit went to the room");
    assert_eq!(
        replaces,
        "::/stations/street/modular_street_station/modular_terminal.con"
    );
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged
            .iter()
            .any(|l| l.contains("makeWorldBuildProposalCmd")
                && l.ends_with("building from this window")),
        "a build that edits nothing stays refused: {logged:?}"
    );
    // This game applied it: the window hears so.
    lua.load("HOOK.results = { { ticket = 1, ok = true } } M.step()")
        .exec()
        .unwrap();
    assert!(lua.load("return CALLED == true").eval::<bool>().unwrap());
}

/// A bus station placed by the street 8-9 of FAKE_NETWORK, as the
/// construction tool proposes it (build 40408's shape): the street rebuilt
/// through a new junction -2, and an entrance edge from the station's own
/// street node -1 to it.
const STATION_BY_ROAD: &str = "{ toRemove = {}, \
    toAdd = { { fileName = '::/stations/street/modular_street_station/modular_terminal.con', \
                name = 'Okehampton Station', playerEntity = 25, \
                transf = { 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 80, 0, 0, 1 }, \
                params = { seed = 7, length = 2 } } }, \
    proposal = { \
    addedNodes = { { entity = -1, comp = { position = { x = 70, y = 0, z = 0 } } }, \
                   { entity = -2, comp = { position = { x = 50, y = 0, z = 0 } } } }, \
    addedSegments = { \
        { entity = -3, type = 0, comp = { node0 = -1, node1 = -2, type = 0, typeIndex = -1, \
          tangent0 = { x = -20, y = 0, z = 0 }, tangent1 = { x = -20, y = 0, z = 0 }, \
          roadTemplate = '::/street/town_small.street_template', roadStyle = '' } }, \
        { entity = -4, type = 0, comp = { node0 = 8, node1 = -2, type = 0, typeIndex = -1, \
          tangent0 = { x = 0, y = 40, z = 0 }, tangent1 = { x = 0, y = 40, z = 0 }, \
          roadTemplate = '::/street/country.street_template', roadStyle = '' } }, \
        { entity = -5, type = 0, comp = { node0 = -2, node1 = 9, type = 0, typeIndex = -1, \
          tangent0 = { x = 0, y = 40, z = 0 }, tangent1 = { x = 0, y = 40, z = 0 }, \
          roadTemplate = '::/street/country.street_template', roadStyle = '' } } }, \
    removedSegments = { { entity = 100, type = 0, comp = { node0 = 8, node1 = 9 } } }, \
    removedNodes = {}, edgeObjectsToAdd = {} } }";

/// A rail station placed on open ground, as the construction tool proposes
/// it on build 40408: the station, and its own platform track as new edges
/// between new nodes, joined to nothing that exists. `{JOIN}` adds an edge
/// or not.
const RAIL_STATION_OPEN: &str = "{ toRemove = {}, \
    toAdd = { { fileName = '::/stations/rail/rail_station.con', \
                name = 'Okehampton Rail', playerEntity = 25, \
                transf = { 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 300, 0, 0, 1 }, \
                params = { seed = 7, length = 2 } } }, \
    proposal = { \
    addedNodes = { { entity = -1, comp = { position = { x = 250, y = 0, z = 0 } } }, \
                   { entity = -2, comp = { position = { x = 300, y = 0, z = 0 } } }, \
                   { entity = -3, comp = { position = { x = 350, y = 0, z = 0 } } } }, \
    addedSegments = { \
        { entity = -4, type = 1, comp = { node0 = -1, node1 = -2, type = 0, typeIndex = -1, \
          tangent0 = { x = 50, y = 0, z = 0 }, tangent1 = { x = 50, y = 0, z = 0 }, \
          roadTemplate = '::/track/standard.track_template', roadStyle = '' } }, \
        { entity = -5, type = 1, comp = { node0 = -2, node1 = -3, type = 0, typeIndex = -1, \
          tangent0 = { x = 50, y = 0, z = 0 }, tangent1 = { x = 50, y = 0, z = 0 }, \
          roadTemplate = '::/track/standard.track_template', roadStyle = '' } } {JOIN} }, \
    removedSegments = {}, removedNodes = {}, edgeObjectsToAdd = {} } }";

/// The native airfield proposal gives each signal row resultEntity=-1, but
/// lists the real, reserved edge-object IDs on their carrier segments.
const AIRFIELD_PROPOSAL: &str = r#"{ toRemove = {},
    toAdd = { { fileName = '::/stations/air/airfield.con', name = 'Okehampton Airfield',
        playerEntity = 25, transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 300,0,0,1 },
        params = { seed = 7 } } },
    proposal = {
        addedNodes = { {NODES} },
        addedSegments = { {SEGMENTS} },
        removedSegments = {}, removedNodes = {},
        edgeObjectsToAdd = { {OBJECT_ROWS} },
        edgeObjectsToRemove = {}
    }
}"#;

#[test]
fn an_airfield_carries_only_its_internal_signals_and_keeps_its_road_connection() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load("api.type.enum.EdgeObjectType = { SIGNAL = 2 }")
        .exec()
        .unwrap();

    let proposal = |external_road: bool, external_signal: bool| {
        let nodes = (1..=10)
            .map(|id| {
                format!(
                    "{{ entity = -{id}, comp = {{ position = {{ x = {id}, y = 0, z = 0 }} }} }}"
                )
            })
            .chain(external_road.then(|| {
                "{ entity = -11, comp = { position = { x = -100, y = 0, z = 0 } } }".to_owned()
            }))
            .collect::<Vec<_>>()
            .join(",");
        let object_ids = [1, 0, 2, 3, 4, 5, 6, 7, 8];
        let mut segments = (0..9)
            .map(|i| {
                let entity = 101 + i;
                let from = i + 1;
                let to = i + 2;
                let object = -400_000_000 - object_ids[i as usize];
                format!(
                    "{{ entity = -{entity}, type = 0, comp = {{ node0 = -{from}, node1 = -{to}, type = 0, typeIndex = -1, \
                     tangent0 = {{ x = 1, y = 0, z = 0 }}, tangent1 = {{ x = 1, y = 0, z = 0 }}, \
                     roadTemplate = '::/street/town_small.street_template', roadStyle = '', objects = {{ {{ {object}, 2 }} }} }} }}"
                )
            })
            .collect::<Vec<_>>();
        let mut rows = (0..9)
            .map(|_| {
                "{ resultEntity = -1, category = 2, left = false, playerEntity = 25, name = 'Okehampton Airfield' }".to_owned()
            })
            .collect::<Vec<_>>();
        if external_road {
            let object = if external_signal {
                "objects = { { -400000009, 2 } }"
            } else {
                "objects = {}"
            };
            segments.push(format!(
                "{{ entity = -120, type = 0, comp = {{ node0 = -11, node1 = 10, type = 0, typeIndex = -1, \
                 tangent0 = {{ x = 40, y = 0, z = 0 }}, tangent1 = {{ x = 40, y = 0, z = 0 }}, \
                 roadTemplate = '::/street/town_small.street_template', roadStyle = '', {object} }} }}"
            ));
            if external_signal {
                rows.push(
                    "{ resultEntity = -1, category = 2, left = false, playerEntity = 25, name = 'Okehampton Airfield' }".to_owned(),
                );
            }
        }
        AIRFIELD_PROPOSAL
            .replace("{NODES}", &nodes)
            .replace("{SEGMENTS}", &segments.join(","))
            .replace("{OBJECT_ROWS}", &rows.join(","))
    };

    // Internal runway/taxiway signals are recreated by the airfield
    // construction itself; the isolated component contributes no road action.
    let isolated = proposal(false, false);
    let (connection, accepted): (String, bool) = lua
        .load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local action, why = capture.construction({isolated}) \
             if not action then error(why) end \
             return tostring(action.BuildConstruction.connection), schema_check(action)"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(connection, "nil");
    assert!(accepted, "the schema takes the construction action");

    // The same airport proposal with an external road edge retains that one
    // edge while the isolated internal signal and runway edges are omitted.
    let mixed = proposal(true, false);
    let (links, accepted): (usize, bool) = lua
        .load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local action, why = capture.construction({mixed}) \
             if not action then error(why) end \
             return #action.BuildConstruction.connection.links, schema_check(action)"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(links, 1, "the external road remains in the action");
    assert!(accepted);

    // A signal attached to the external road is not an airport-internal
    // object and the entire construction action stays refused.
    let with_external_signal = proposal(true, true);
    let refusal: String = lua
        .load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local _, why = capture.construction({with_external_signal}) return why"
        ))
        .eval()
        .unwrap();
    assert_eq!(refusal, "a build with a stop or signal");

    // An airport module edit also replaces its generated runway signals. If
    // the edit changes the surrounding road, carry the split and airport
    // access branch, while omitting the old construction's own removals and
    // the new construction's regenerated signal objects.
    let edit = proposal(true, false);
    let (links, removals, replaces, accepted): (usize, usize, String, bool) = lua
        .load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local p = {edit} \
             api.type.ComponentType.CONSTRUCTION = 2 \
             local frozen = {{}} for edge = 7607, 7627 do frozen[#frozen+1] = edge end \
             local old = {{ fileName = '::/stations/air/airfield.con', \
                 transf = {{ 1,0,0,0, 0,1,0,0, 0,0,1,0, 300,0,0,1 }}, \
                 townBuildings = {{}}, frozenEdges = frozen, frozenNodes = {{}} }} \
             CONSTRUCTIONS = {{ [77] = old }} \
             local get = api.engine.getComponent \
             api.engine.getComponent = function(e, k) \
                 if e == 77 and k == 2 then return old end return get(e, k) end \
             p.toRemove = {{ 77 }} \
             p.proposal.addedNodes[#p.proposal.addedNodes + 1] = \
                 {{ entity = -12, comp = {{ position = {{ x = 0, y = 0, z = 0 }} }} }} \
             p.proposal.addedSegments[#p.proposal.addedSegments + 1] = \
                 {{ entity = -121, type = 0, comp = {{ node0 = 7, node1 = -11, type = 0, typeIndex = -1, \
                    tangent0 = {{ x = 0, y = 10, z = 0 }}, tangent1 = {{ x = 0, y = 10, z = 0 }}, \
                    roadTemplate = '::/street/town_small.street_template', roadStyle = '', objects = {{}} }} }} \
             p.proposal.addedSegments[#p.proposal.addedSegments + 1] = \
                 {{ entity = -122, type = 0, comp = {{ node0 = -11, node1 = -12, type = 0, typeIndex = -1, \
                    tangent0 = {{ x = 0, y = 10, z = 0 }}, tangent1 = {{ x = 0, y = 10, z = 0 }}, \
                    roadTemplate = '::/street/town_small.street_template', roadStyle = '', objects = {{}} }} }} \
             local signalIds = {{ 7091,7011,7109,6229,7296,6230,6948,7583,7229 }} \
             local signalEdges = {{ 7613,7614,7616,7619,7620,7621,7624,7625,7626 }} \
             local signals = {{}} for i, edge in ipairs(signalEdges) do signals[edge] = signalIds[i] end \
             p.proposal.removedSegments = {{}} \
             for edge = 7607, 7627 do \
                 local id = signals[edge] local objects = id and {{ {{ id, 2 }} }} or {{}} \
                 p.proposal.removedSegments[#p.proposal.removedSegments+1] = \
                     {{ entity = edge, type = 0, comp = {{ objects = objects }} }} \
             end \
             p.proposal.removedSegments[#p.proposal.removedSegments+1] = \
                 {{ entity = 101, type = 0, comp = {{ node0 = 10, node1 = 7, objects = {{}} }} }} \
             p.proposal.edgeObjectsToRemove = signalIds \
             local action, why = capture.construction(p) \
             if not action then error(why) end \
             local c = action.BuildConstruction.connection \
             return #c.links, #c.removals, action.BuildConstruction.replaces.file, schema_check(action)"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        links, 3,
        "both sides of the road split and airport access stay linked"
    );
    assert_eq!(removals, 1, "the external road edge is carried once");
    assert_eq!(
        replaces, "::/stations/air/airfield.con",
        "the signal-bearing airfield is still the construction being edited"
    );
    assert!(accepted, "the edited airport action fits the schema");
}

#[test]
fn an_airfield_module_edit_rebuilds_only_its_frozen_runway_signals() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load("api.type.enum.EdgeObjectType = { SIGNAL = 2 }")
        .exec()
        .unwrap();

    let nodes = (1..=10)
        .map(|id| {
            format!("{{ entity = -{id}, comp = {{ position = {{ x = {id}, y = 0, z = 0 }} }} }}")
        })
        .collect::<Vec<_>>()
        .join(",");
    let signal_offsets = [1, 0, 2, 3, 4, 5, 6, 7, 8];
    let added_segments = (0..9)
        .map(|i| {
            let object = -400_000_000 - signal_offsets[i];
            format!(
                "{{ entity = -{}, type = 0, comp = {{ node0 = -{}, node1 = -{}, type = 0, typeIndex = -1, \
                 tangent0 = {{ x = 1, y = 0, z = 0 }}, tangent1 = {{ x = 1, y = 0, z = 0 }}, \
                 roadTemplate = '::/street/town_small.street_template', roadStyle = '', objects = {{ {{ {object}, 2 }} }} }} }}",
                101 + i,
                i + 1,
                i + 2
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let added_objects = (0..9)
        .map(|_| {
            "{ resultEntity = -1, category = 2, left = false, playerEntity = 25, name = 'Okehampton Airfield' }"
        })
        .collect::<Vec<_>>()
        .join(",");
    let removed_ids = [7091, 7011, 7109, 6229, 7296, 6230, 6948, 7583, 7229];
    let removed_edges = [7613, 7614, 7616, 7619, 7620, 7621, 7624, 7625, 7626];
    let removed_segments = removed_edges
        .iter()
        .zip(removed_ids)
        .map(|(edge, object)| {
            format!(
                "{{ entity = {edge}, type = 0, comp = {{ node0 = 500, node1 = 501, objects = {{ {{ {object}, 2 }} }} }} }}"
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let remove_rows = removed_ids
        .iter()
        .map(i32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let full = AIRFIELD_PROPOSAL
        .replace("{NODES}", &nodes)
        .replace("{SEGMENTS}", &added_segments)
        .replace("{OBJECT_ROWS}", &added_objects)
        .replace("toRemove = {}", "toRemove = { 77 }")
        .replace(
            "removedSegments = {}, removedNodes = {}",
            &format!("removedSegments = {{ {removed_segments} }}, removedNodes = {{}}"),
        )
        .replace(
            "edgeObjectsToRemove = {}",
            &format!("edgeObjectsToRemove = {{ {remove_rows} }}"),
        );
    // Keep the fixture easy to inspect if the native proposal shape changes.
    assert!(full.contains("edgeObjectsToRemove = { 7091,7011,7109"));

    lua.load(NATIVE_OF).exec().unwrap();
    let frozen = (7607..=7627)
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let action: (bool, String) = lua
        .load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local full = {full} \
             FULL = function() return full end \
             api.type.ComponentType = {{ CONSTRUCTION = 2 }} \
             local old = {{ fileName = '::/stations/air/airfield.con', \
                 transf = {{ 1,0,0,0, 0,1,0,0, 0,0,1,0, 300,0,0,1 }}, \
                 townBuildings = {{}}, frozenEdges = {{ {frozen} }}, frozenNodes = {{}} }} \
             api.engine.getComponent = function(entity, kind) \
                 if entity == 77 and kind == 2 then return old end return nil end \
             api.engine.util = {{ proposal = {{ createProposalReplaceConstruction = function() return FULL() end }} }} \
             local action, why = capture.moduleEdit(NATIVE_OF(FULL())) \
             if not action then return false, why end \
             return schema_check(action), tostring(action.BuildConstruction.connection)"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(action, (true, "nil".to_owned()));

    // One signal on a non-frozen edge makes the replacement external, even
    // when its ID and type otherwise match the removed-object list.
    let refusal: String = lua
        .load(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local old = api.engine.getComponent(77, 2) \
             old.frozenEdges = { 7607,7608,7609,7610,7611,7612,7613,7614,7616,7619,7620,7621,7624,7625 } \
             local action, why = capture.construction(FULL()) \
             return tostring(action) .. ' ' .. tostring(why)",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(refusal, "nil a build with a removed stop or signal");
}

#[test]
fn a_rail_station_on_open_ground_leaves_its_own_track_to_the_station() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    let open = RAIL_STATION_OPEN.replace("{JOIN}", "");
    let joined = RAIL_STATION_OPEN.replace(
        "{JOIN}",
        ", { entity = -6, type = 1, comp = { node0 = -3, node1 = 8, type = 0, typeIndex = -1, \
           tangent0 = { x = 50, y = 0, z = 0 }, tangent1 = { x = 50, y = 0, z = 0 }, \
           roadTemplate = '::/track/standard.track_template', roadStyle = '' } }",
    );
    let (alone, ok, links): (String, bool, usize) = lua
        .load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local open = capture.construction({open}) \
             local joined = capture.construction({joined}) \
             return tostring(open.BuildConstruction.connection), schema_check(open), \
                 #joined.BuildConstruction.connection.links"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        alone, "nil",
        "the platform track is the station's own: built beside it, it blocks it"
    );
    assert!(ok, "the schema takes it");
    assert_eq!(
        links, 3,
        "joined to an existing track, it travels as before"
    );
}

#[test]
fn a_snapped_stations_preview_leaves_its_junction_settings_out() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    // The station as the tool snapped it to the street, with a junction
    // setting on a node this game does not have (seen on build 40408: "the
    // junction no longer exists").
    let (made, sent): (String, usize) = lua
        .load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local apply = ug_require('tpf3mp_1::/scripts/tpf3mp/apply.lua') \
             local action = assert(capture.construction({STATION_BY_ROAD})) \
             action.BuildConstruction.connection.junctions = {{ {{ \
                 node = {{ network = 'Street', at = {{ x = 999, y = 999, z = 0 }} }} }} }} \
             local p, why = apply.proposalOf(action, {{}}) \
             if not p then return 'none: ' .. tostring(why), #SENT end \
             return p.constructionsToAdd[1].fileName .. ' ' .. #p.streetProposal.edgesToAdd, #SENT"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(made.contains(".con "), "{made}");
    assert!(!made.starts_with("none"), "{made}");
    assert_eq!(sent, 0);
}

#[test]
fn a_station_by_a_road_travels_with_the_junction_that_joins_it() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    let (carried, ok): (String, bool) = lua
        .load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             ACTION = capture.construction({STATION_BY_ROAD}) \
             local c = ACTION.BuildConstruction.connection \
             local out = {{ #c.vertices, #c.links, #c.removals }} \
             for _, v in ipairs(c.vertices) do out[#out + 1] = type(v.resolve) == 'table' and v.resolve.Node or v.resolve end \
             for _, l in ipairs(c.links) do out[#out + 1] = l.from .. '>' .. l.to .. ':' .. l.kind.template end \
             out[#out + 1] = c.removals[1].ends.a.y .. ',' .. c.removals[1].ends.b.y \
             return table.concat(out, '|'), schema_check(ACTION)"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        carried,
        "4|3|1|New|New|Street|Street\
         |0>1:::/street/town_small.street_template\
         |2>1:::/street/country.street_template\
         |1>3:::/street/country.street_template|-40,40",
        "the entrance and the rebuilt street, every link with its kind, and the street it replaces"
    );
    assert!(ok, "the schema takes it");
    // Every game builds the station and the street rebuilt through the
    // junction in one proposal, leaving out the station's own entrance,
    // which the station makes again unsnapped; then the game's refresh of
    // the new station, which snaps its entrance onto the junction. As the
    // game: a built construction is listed, and refreshed on request.
    lua.load(STATION_REFRESH).exec().unwrap();
    lua.load("HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let built: String = lua
        .load(
            "local p = SENT[1].proposal local s = p.streetProposal \
             local out = { #SENT, p.constructionsToAdd[1].fileName, #s.nodesToAdd, #s.edgesToAdd, \
                 table.concat(s.edgesToRemove, ','), table.concat(s.nodeConfigsToRemove or {}, ',') } \
             for _, e in ipairs(s.edgesToAdd) do \
                 out[#out + 1] = e.comp.node0 .. '>' .. e.comp.node1 .. ':' .. e.comp.roadTemplate end \
             local r = SENT[2] \
             out[#out + 1] = r.proposal.refreshed .. ':' .. tostring(r.context) .. ':' .. tostring(r.ignoreErrors) \
                 .. ':' .. tostring(r.playerInitiated) \
             return table.concat(out, '|')",
        )
        .eval()
        .unwrap_or_else(|error| {
            panic!(
                "{error}\n{:?}",
                lua.load("return HOOK.logged").eval::<Vec<String>>()
            )
        });
    assert_eq!(
        built,
        "2|::/stations/street/modular_street_station/modular_terminal.con|1|2|100|8,9\
         |8>-3:::/street/country.street_template\
         |-3>9:::/street/country.street_template\
         |5000:nil:true:false",
        "the station and the rebuilt street, then its refresh, free, as no player's click"
    );
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged
            .iter()
            .any(|l| l == "snapping 5000 +e-2:-1>7777 -e6000"),
        "{logged:?}"
    );
}

/// As the game: a built construction is listed, and refreshed on request,
/// its refresh snapping its entrance onto node 7777.
const STATION_REFRESH: &str = "api.type.ComponentType.CONSTRUCTION = 2 \
    api.type.ComponentType.PLAYER_OWNED = 15 \
    CONSTRUCTIONS, OWNERS = {}, {} \
    local get = api.engine.getComponent \
    api.engine.getComponent = function(e, kind) \
        if kind == 2 then return CONSTRUCTIONS[e] end \
        if kind == 15 then return OWNERS[e] and { player = OWNERS[e] } end \
        return get(e, kind) end \
    api.engine.getEntitiesWithComponent = function(kind) \
        local l = {} if kind == 2 then for e in pairs(CONSTRUCTIONS) do l[#l + 1] = e end end return l end \
    api.cmd.makeEntitySetPlayerCmd = function(entity, player) return { setPlayer = entity, player = player } end \
    local send = api.cmd.sendCommand \
    api.cmd.sendCommand = function(cmd, ...) \
        local c = cmd.proposal and cmd.proposal.constructionsToAdd and cmd.proposal.constructionsToAdd[1] \
        if c then CONSTRUCTIONS[5000] = { fileName = c.fileName, \
            transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, c.transf[4][1], c.transf[4][2], c.transf[4][3], 1 } } \
            OWNERS[5000] = cmd.context and cmd.context.player or api.engine.util.getPlayer() end \
        return send(cmd, ...) \
    end \
    api.engine.util.proposal = { refreshConstruction = function(e) return { refreshed = e, \
        proposal = { addedSegments = { { entity = -2, comp = { node0 = -1, node1 = 7777 } } }, \
                     removedSegments = { { entity = 6000 } } } } end }";

/// Room-built stations stood unnamed (2026-10-02). Every game names the
/// station group a new station's own stations make up by the name the
/// tool gave the construction, where the game left it unnamed; a group
/// that has a name keeps it.
#[test]
fn a_station_the_room_builds_names_its_group_as_the_tool_named_it() {
    for (before, after) in [("nil", "Okehampton Station"), ("'Didcot'", "Didcot")] {
        let (lua, _script) = engine();
        lua.load(FAKE_NETWORK).exec().unwrap();
        lua.load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             ACTION = capture.construction({STATION_BY_ROAD})"
        ))
        .exec()
        .unwrap_or_else(|error| panic!("{error}"));
        lua.load(STATION_REFRESH).exec().unwrap();
        lua.load(format!(
            "api.type.ComponentType.STATION_GROUP = 9 \
             OWNERS[5001], OWNERS[5002] = api.engine.util.getPlayer(), api.engine.util.getPlayer() \
             NAMES = {{ [5002] = {before} }} \
             api.engine.util.getEntityName = function(e) return NAMES[e] end \
             api.engine.system.stationGroupSystem = {{ getStationGroup = function(s) \
                 if s == 5001 then return 5002 end return -1 end }} \
             local get = api.engine.getComponent \
             api.engine.getComponent = function(e, kind) \
                 if kind == 9 and e == 5002 then return {{ stations = {{ 5001 }} }} end \
                 return get(e, kind) \
             end \
             local send = api.cmd.sendCommand \
             api.cmd.sendCommand = function(cmd, ...) \
                 local r = send(cmd, ...) \
                 if CONSTRUCTIONS[5000] then CONSTRUCTIONS[5000].stations = {{ 5001 }} end \
                 if cmd.setName then NAMES[cmd.entity] = cmd.setName end \
                 return r \
             end \
             HOOK.batch = {{ ACTION }} UPDATE({{}}, STATE, 0.2)"
        ))
        .exec()
        .unwrap_or_else(|error| panic!("{error}"));
        let (ok, name): (bool, String) = lua
            .load("return HOOK.applied[1].ok == true, tostring(NAMES[5002])")
            .eval()
            .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
        assert!(ok, "{}", hook_log(&lua));
        assert_eq!(name, after, "{}", hook_log(&lua));
    }
}

/// A street station placed into a street (2026-10-02, live: refused in
/// every game, "the junction no longer exists"). The tool's proposal
/// configures the station's own entrance node -1, the new junction -2 its
/// entrance joins, and the street's existing ends 8 and 9, each added and
/// removed. Every game leaves out the entrance, which the station makes
/// again itself, and the originator the settings that name it: those at
/// -1, and those at -2, whose turns lead into it. The settings at 8 and 9
/// name only the rebuilt street, and travel as the tool made them.
#[test]
fn a_station_by_a_road_leaves_its_own_entrances_junction_settings_to_it() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(CONFIG_WORDS).exec().unwrap();
    lua.load(format!(
        "local function config(turns, walks) \
             local t = {{}} \
             for i, p in ipairs(turns) do t[i] = {{ segment0 = p[1], lane0 = 0, segment1 = p[2], lane1 = 0, \
                 withRoad = true, withTram = false }} end \
             return {{ trafficLightPreference = 0, doubleSlipSwitch = false, userModifiedTrafficLightStates = false, \
                 laneConnections = t, crosswalks = walks, trafficLightConfig = {{ trafficLightType = -1, states = {{}} }} }} \
         end \
         PROPOSAL = {STATION_BY_ROAD} \
         local s = PROPOSAL.proposal \
         s.nodeConfigsToAdd = {{ \
             {{ entity = -1, comp = config({{}}, {{ -3 }}) }}, \
             {{ entity = -2, comp = config({{ {{ -3, -4 }}, {{ -3, -5 }}, {{ -4, -3 }}, {{ -4, -5 }}, {{ -5, -3 }}, {{ -5, -4 }} }}, \
                 {{ -3, -4, -5 }}) }}, \
             {{ entity = 8, comp = config({{}}, {{ -4 }}) }}, \
             {{ entity = 9, comp = config({{}}, {{ -5 }}) }} }} \
         s.nodeConfigsToRemove = {{ 8, 9 }} \
         local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         ACTION = assert(capture.construction(PROPOSAL)) \
         assert(schema_check(ACTION)) \
         assert(#ACTION.BuildConstruction.connection.junctions == 2, 'the street ends; the entrance stays with the station')"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    // Through the schema both ways, as the room hands it to every game.
    let captured: mlua::Value = lua.globals().get("ACTION").unwrap();
    let action = tpf3mp_proto::lua::action_from_lua(&common::tree(&captured))
        .unwrap_or_else(|error| panic!("the schema refuses it: {error}"));
    let back = tpf3mp_proto::lua::action_to_lua(&action).unwrap();
    lua.globals()
        .set("ACTION", common::value(&lua, &back))
        .unwrap();
    lua.load(STATION_REFRESH).exec().unwrap();
    lua.load("HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap_or_else(|error| panic!("{error}"));
    let (ok, sends, sent, removed): (bool, usize, String, String) = lua
        .load(
            "local s = SENT[1] and SENT[1].proposal.streetProposal \
             local removed = {} \
             for i, n in ipairs(s and s.nodeConfigsToRemove or {}) do removed[i] = n end \
             table.sort(removed) \
             return HOOK.applied[1].ok == true, #SENT, s and SENT_WORDS(SENT[1]) or '', \
                 table.concat(removed, ',')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert!(ok, "the station is built: {}", hook_log(&lua));
    assert_eq!(sends, 2, "the station with its street, then its refresh");
    assert_eq!(
        sent,
        "(50,-40) tl0 type-1  dss=false um=false/false turns  walks (50,-40)-(50,0) \
         || (50,40) tl0 type-1  dss=false um=false/false turns  walks (50,0)-(50,40)",
        "the street's ends as the tool configured them; nothing at the entrance or its junction"
    );
    assert_eq!(removed, "8,9", "the settings they replace go");
    assert!(
        !hook_log(&lua).contains("left to the construction"),
        "the originator left them out already: {}",
        hook_log(&lua)
    );
}

/// A construction whose own track the tool snapped onto an existing track
/// node (2026-09-30: a rail depot placed against the end of a track). Every
/// game builds the construction alone, then its refresh snaps its track:
/// built beside it as well, the track collided with the construction's own,
/// the refresh was refused and the depot stood unconnected in every game.
#[test]
fn a_construction_whose_own_track_the_tool_snapped_is_built_alone_then_snapped() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    let joined = RAIL_STATION_OPEN.replace(
        "{JOIN}",
        ", { entity = -6, type = 1, comp = { node0 = -3, node1 = 8, type = 0, typeIndex = -1, \
           tangent0 = { x = 50, y = 0, z = 0 }, tangent1 = { x = 50, y = 0, z = 0 }, \
           roadTemplate = '::/track/standard.track_template', roadStyle = '' } }",
    );
    let links: usize = lua
        .load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             ACTION = capture.construction({joined}) \
             return #ACTION.BuildConstruction.connection.links"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(links, 3, "the tool's proposal, as it was");
    // Node 8 a track node too, and the track's template known.
    lua.load(
        "api.engine.system.streetSystem.getNode2TrackEdgeMap = function() return { [8] = { 100 } } end \
         local find, get = api.res.streetTemplateRep.find, api.res.streetTemplateRep.get \
         api.res.streetTemplateRep.find = function(n) \
             if n == '::/track/standard.track_template' then return 6 end return find(n) end \
         api.res.streetTemplateRep.get = function(id) \
             if id == 6 then return { laneConfigs = { 'track lanes' }, streetStyle = '' } end return get(id) end",
    )
    .exec()
    .unwrap();
    lua.load(STATION_REFRESH).exec().unwrap();
    lua.load("HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let built: String = lua
        .load(
            "local p = SENT[1].proposal local s = p.streetProposal \
             return table.concat({ #SENT, p.constructionsToAdd[1].fileName, #(s.edgesToAdd or {}), \
                 #(s.nodesToAdd or {}), tostring(SENT[2] and SENT[2].proposal.refreshed), \
                 tostring(HOOK.applied[1].ok) }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| {
            panic!(
                "{error}\n{:?}",
                lua.load("return HOOK.logged").eval::<Vec<String>>()
            )
        });
    assert_eq!(
        built,
        "2|::/stations/rail/rail_station.con|0|0|5000|true",
        "the construction alone, then its refresh, which snaps its own track: {:?}",
        lua.load("return HOOK.logged").eval::<Vec<String>>()
    );
}

/// A rail station snapped to a track end, whose tool also configured the
/// switches of its own platform tracks (2026-10-07, live: refused in every
/// game, "the junction no longer exists"). The platform track -10 > -11 >
/// -12 reaches nothing that exists, so the room leaves it to the station;
/// the setting at its switch -11, whose turns name its edges, goes with it.
/// The setting at the snapped track's new internal node -2 stays with the station.
#[test]
fn a_rail_station_leaves_its_own_tracks_junction_settings_to_it() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    let joined = RAIL_STATION_OPEN.replace(
        "{JOIN}",
        ", { entity = -6, type = 1, comp = { node0 = -3, node1 = 8, type = 0, typeIndex = -1, \
           tangent0 = { x = 50, y = 0, z = 0 }, tangent1 = { x = 50, y = 0, z = 0 }, \
           roadTemplate = '::/track/standard.track_template', roadStyle = '' } }, \
         { entity = -13, type = 1, comp = { node0 = -10, node1 = -11, type = 0, typeIndex = -1, \
           tangent0 = { x = 50, y = 0, z = 0 }, tangent1 = { x = 50, y = 0, z = 0 }, \
           roadTemplate = '::/track/standard.track_template', roadStyle = '' } }, \
         { entity = -14, type = 1, comp = { node0 = -11, node1 = -12, type = 0, typeIndex = -1, \
           tangent0 = { x = 50, y = 0, z = 0 }, tangent1 = { x = 50, y = 0, z = 0 }, \
           roadTemplate = '::/track/standard.track_template', roadStyle = '' } }",
    );
    lua.load(format!(
        "local function config(turns) \
             local t = {{}} \
             for i, p in ipairs(turns) do t[i] = {{ segment0 = p[1], lane0 = 0, segment1 = p[2], lane1 = 0, \
                 withRoad = true, withTram = false }} end \
             return {{ trafficLightPreference = 0, doubleSlipSwitch = false, userModifiedTrafficLightStates = false, \
                 laneConnections = t, crosswalks = {{}}, trafficLightConfig = {{ trafficLightType = -1, states = {{}} }} }} \
         end \
         PROPOSAL = {joined} \
         local s = PROPOSAL.proposal \
         for _, n in ipairs({{ {{ -10, 250 }}, {{ -11, 300 }}, {{ -12, 350 }} }}) do \
             s.addedNodes[#s.addedNodes + 1] = {{ entity = n[1], comp = {{ position = {{ x = n[2], y = 20, z = 0 }} }} }} \
         end \
         s.nodeConfigsToAdd = {{ \
             {{ entity = -2, comp = config({{}}) }}, \
             {{ entity = -11, comp = config({{ {{ -13, -14 }}, {{ -14, -13 }} }}) }} }} \
         local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         ACTION = assert(capture.construction(PROPOSAL)) \
         assert(schema_check(ACTION))"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    // Every game builds it, as the station and its refresh.
    lua.load(
        "api.engine.system.streetSystem.getNode2TrackEdgeMap = function() return { [8] = { 100 } } end \
         local find, get = api.res.streetTemplateRep.find, api.res.streetTemplateRep.get \
         api.res.streetTemplateRep.find = function(n) \
             if n == '::/track/standard.track_template' then return 6 end return find(n) end \
         api.res.streetTemplateRep.get = function(id) \
             if id == 6 then return { laneConfigs = { 'track lanes' }, streetStyle = '' } end return get(id) end",
    )
    .exec()
    .unwrap();
    lua.load(STATION_REFRESH).exec().unwrap();
    lua.load("HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let (ok, file): (bool, String) = lua
        .load(
            "return HOOK.applied[1].ok == true, \
                 tostring(SENT[1] and SENT[1].proposal.constructionsToAdd[1].fileName)",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert!(ok, "the station is built: {}", hook_log(&lua));
    assert_eq!(file, "::/stations/rail/rail_station.con");
    // What travels: the snapped track and the setting on it.
    let carried: String = lua
        .load(
            "local c = ACTION.BuildConstruction.connection \
             local out = { #c.links } \
             for _, j in ipairs(c.junctions) do out[#out + 1] = j.node.network .. '(' .. j.node.at.x .. ',' \
                 .. j.node.at.y .. ')' end \
             return table.concat(out, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        carried, "3",
        "the snapped track; no setting on the station's own track, which every game leaves to it"
    );
}

/// A rail station whose own track the tool joined onto the end of a track
/// that exists: track 200 from node 21 (450,0) to its end, node 20 (400,0).
/// As the game (2026-10-10, an underpass mod's tracks onto track ends):
/// the station alone is refused while node 20 stands where its track ends;
/// built, it makes its own track end, node 30, there.
const STATION_ON_TRACK_END: &str = "\
    function TRACK_LANE(electric) \
        local m = {} for i = 0, 15 do m[i] = false end m[4] = electric \
        return { speed = 33, width = 4, height = 0, offset = 0, forward = true, transportModes = m } end \
    NODES[20], NODES[21] = { x = 400, y = 0, z = 0 }, { x = 450, y = 0, z = 0 } \
    EDGES[200] = { node0 = 21, node1 = 20, type = 0, tangent0 = { x = -50, y = 0, z = 0 }, \
        tangent1 = { x = -50, y = 0, z = 0 }, objects = {}, edgeDecorations = {}, \
        roadTemplate = '::/track/standard.track_template', laneConfigs = { TRACK_LANE(true) } } \
    TRACKS = { [20] = { 200 }, [21] = { 200 } } \
    local streets = api.engine.system.streetSystem \
    streets.getNode2TrackEdgeMap = function() local m = {} for n, e in pairs(TRACKS) do m[n] = e end return m end \
    streets.getNodeTrackSegments = function(n) return TRACKS[n] or {} end \
    local find, get = api.res.streetTemplateRep.find, api.res.streetTemplateRep.get \
    api.res.streetTemplateRep.find = function(n) \
        if n == '::/track/standard.track_template' then return 6 end return find(n) end \
    api.res.streetTemplateRep.get = function(id) \
        if id == 6 then return { laneConfigs = { TRACK_LANE(false) }, streetStyle = '' } end return get(id) end";

/// The game for STATION_ON_TRACK_END, once STATION_REFRESH is loaded: its
/// verdict, and what a build does to the tracks.
const TRACK_END_GAME: &str = "\
    api.engine.util.proposal.makeProposalData = function(p, context) \
        local s = p.streetProposal or {} \
        local away = false \
        for _, n in ipairs(s.nodesToRemove or {}) do if n == 20 then away = true end end \
        if #(p.constructionsToAdd or {}) > 0 and NODES[20] and not away then \
            return { errorState = { critical = true, messages = { 'Construction Not Possible' } } } end \
        if POOR and context ~= nil and #(p.constructionsToAdd or {}) == 0 then \
            return { errorState = { critical = true, messages = { 'Not enough money' } } } end \
        return { errorState = { critical = false, messages = {} } } end \
    local send = api.cmd.sendCommand \
    api.cmd.sendCommand = function(cmd, ...) \
        local s = cmd.proposal and cmd.proposal.streetProposal \
        for _, e in ipairs(s and s.edgesToRemove or {}) do \
            for n, list in pairs(TRACKS) do \
                local left = {} for _, x in ipairs(list) do if x ~= e then left[#left + 1] = x end end \
                TRACKS[n] = left end \
            EDGES[e] = nil end \
        for _, n in ipairs(s and s.nodesToRemove or {}) do NODES[n], TRACKS[n] = nil, nil end \
        local c = cmd.proposal and cmd.proposal.constructionsToAdd and cmd.proposal.constructionsToAdd[1] \
        if c then NODES[30], TRACKS[30] = { x = 400, y = 0, z = 0 }, { 301 } end \
        return send(cmd, ...) \
    end";

// Only staged fixtures opt into the non-atomic rejoin path. In ordinary
// rooms the mod fingerprint holds this gate off for every participant.
const ENABLE_TRACK_END_REJOIN: &str =
    "ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').rejoinConstruction = true";

/// The rail station of RAIL_STATION_OPEN with its own track joined onto
/// node 20 of STATION_ON_TRACK_END, as the tool proposes it.
fn station_on_track_end() -> String {
    RAIL_STATION_OPEN.replace(
        "{JOIN}",
        ", { entity = -6, type = 1, comp = { node0 = -3, node1 = 20, type = 0, typeIndex = -1, \
           tangent0 = { x = 50, y = 0, z = 0 }, tangent1 = { x = 50, y = 0, z = 0 }, \
           roadTemplate = '::/track/standard.track_template', roadStyle = '' } }",
    )
}

/// The ordinary room keeps the non-atomic fallback off. Even if the game
/// says the construction would fit after removing the old track, no command
/// may remove that track before its replacement is guaranteed.
#[test]
fn a_construction_refused_on_a_track_end_keeps_the_original_track_by_default() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(STATION_ON_TRACK_END).exec().unwrap();
    lua.load(format!(
        "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         ACTION = assert(capture.construction({}))",
        station_on_track_end()
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    lua.load(STATION_REFRESH).exec().unwrap();
    lua.load(TRACK_END_GAME).exec().unwrap();
    lua.load("HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let (gate, ok, sent, edge, node): (bool, bool, usize, bool, bool) = lua
        .load(
            "return ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').rejoinConstruction, \
             HOOK.applied[1].ok == true, #SENT, EDGES[200] ~= nil, NODES[20] ~= nil",
        )
        .eval()
        .unwrap();
    assert!(!gate, "the release mod must leave the fallback disabled");
    assert!(!ok, "{}", hook_log(&lua));
    assert_eq!(sent, 0, "no command may remove the original track");
    assert!(edge && node, "the original track remains in the world");
    assert!(hook_log(&lua).contains("snapped track-end rejoin awaits a safe build"));
}

#[test]
fn a_construction_refused_on_a_track_end_is_joined_to_it_as_the_tool_joined_it() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(STATION_ON_TRACK_END).exec().unwrap();
    lua.load(format!(
        "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         ACTION = assert(capture.construction({})) \
         assert(schema_check(ACTION))",
        station_on_track_end()
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    lua.load(STATION_REFRESH).exec().unwrap();
    lua.load(TRACK_END_GAME).exec().unwrap();
    lua.load(ENABLE_TRACK_END_REJOIN).exec().unwrap();
    lua.load("HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let (ok, built, again): (bool, String, String) = lua
        .load(
            "local a, b = SENT[1].proposal, SENT[2] and SENT[2].proposal \
             local s = a.streetProposal \
             local first = table.concat({ #SENT, a.constructionsToAdd[1].fileName, \
                 table.concat(s.edgesToRemove, ','), table.concat(s.nodesToRemove, ',') }, '|') \
             local t = b and b.streetProposal or {} \
             local e = t.edgesToAdd and t.edgesToAdd[1] \
             local n = t.nodesToAdd and t.nodesToAdd[1] \
             local second = table.concat({ #(b and b.constructionsToAdd or {}), #(t.edgesToAdd or {}), \
                 tostring(e and e.comp.node0), tostring(e and e.comp.node1), tostring(e and e.type), \
                 tostring(n and n.entity), tostring(n and n.comp.position.x) }, '|') \
             return HOOK.applied[1].ok == true, first, second",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert!(ok, "the station is built: {}", hook_log(&lua));
    assert_eq!(
        built, "3|::/stations/rail/rail_station.con|200|20,21",
        "the station, the track piece at the end and its far end away; then two more"
    );
    assert_eq!(
        again, "0|1|-2|30|1|-2|450",
        "the piece again, from a new node where its far end was onto the station's own track end"
    );
    let electric: bool = lua
        .load("return SENT[2].proposal.streetProposal.edgesToAdd[1].comp.laneConfigs[1].transportModes[5] == true")
        .eval()
        .unwrap();
    assert!(
        electric,
        "its lanes as they were, not its template's: still electrified"
    );
    let paid: String = lua
        .load("return tostring(SENT[2].context) .. '|' .. tostring(SENT[2].playerInitiated)")
        .eval()
        .unwrap();
    assert_eq!(
        paid, "nil|false",
        "laid again for free: the piece was the player's already"
    );
    assert!(
        hook_log(&lua).contains(
            "joining ::/stations/rail/rail_station.con onto track ends 20: \
             its track pieces 200 laid again on it"
        ),
        "{}",
        hook_log(&lua)
    );
}

/// As above, with the track piece's far end, node 21, joined on to track
/// 199 from node 22: node 21 stays, and the piece is laid again from it.
#[test]
fn a_construction_refused_on_a_track_end_lays_the_piece_again_from_its_far_end() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(STATION_ON_TRACK_END).exec().unwrap();
    lua.load(
        "NODES[22] = { x = 500, y = 0, z = 0 }          EDGES[199] = { node0 = 22, node1 = 21, type = 0, tangent0 = { x = -50, y = 0, z = 0 },              tangent1 = { x = -50, y = 0, z = 0 }, objects = {}, edgeDecorations = {},              roadTemplate = '::/track/standard.track_template', laneConfigs = { TRACK_LANE(false) } }          TRACKS[21], TRACKS[22] = { 199, 200 }, { 199 }",
    )
    .exec()
    .unwrap();
    lua.load(format!(
        "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua')          ACTION = assert(capture.construction({}))",
        station_on_track_end()
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    lua.load(STATION_REFRESH).exec().unwrap();
    lua.load(TRACK_END_GAME).exec().unwrap();
    lua.load(ENABLE_TRACK_END_REJOIN).exec().unwrap();
    lua.load("HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let (ok, removed, again): (bool, String, String) = lua
        .load(
            "local s = SENT[1].proposal.streetProposal              local t = SENT[2] and SENT[2].proposal.streetProposal or {}              local e = t.edgesToAdd and t.edgesToAdd[1]              return HOOK.applied[1].ok == true, table.concat(s.nodesToRemove, ','),                  table.concat({ #(t.nodesToAdd or {}), tostring(e and e.comp.node0), tostring(e and e.comp.node1) }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}
{}", hook_log(&lua)));
    assert!(ok, "the station is built: {}", hook_log(&lua));
    assert_eq!(removed, "20", "the far end stays");
    assert_eq!(
        again, "0|21|30",
        "the piece again, from its far end onto the station's track end"
    );
}

/// As above, where the player could not pay for laying the piece again
/// (not enough money left after the station): it is laid again for free,
/// as the game's refresh of a construction is, as it always is: the piece
/// was the player's already.
#[test]
fn a_construction_refused_on_a_track_end_lays_the_piece_again_for_free() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(STATION_ON_TRACK_END).exec().unwrap();
    lua.load(format!(
        "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         ACTION = assert(capture.construction({}))",
        station_on_track_end()
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    lua.load(STATION_REFRESH).exec().unwrap();
    lua.load(TRACK_END_GAME).exec().unwrap();
    lua.load(ENABLE_TRACK_END_REJOIN).exec().unwrap();
    lua.load("POOR = true HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let (ok, free): (bool, String) = lua
        .load(
            "local b = SENT[2] \
             return HOOK.applied[1].ok == true, tostring(b and b.context) .. '|' \
                 .. tostring(b and #b.proposal.streetProposal.edgesToAdd)",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert!(ok, "{}", hook_log(&lua));
    assert_eq!(free, "nil|1", "the piece again, with no player to pay");
}

/// As above, with other tracks' nodes ten metres right above (25) and
/// below (19, an older one) where the piece ended: the piece at its own
/// level is taken, and laid again onto the station's own track end the
/// replay found, by its entity, not onto the node above it.
#[test]
fn a_construction_refused_on_a_track_end_joins_the_piece_at_its_own_level() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(STATION_ON_TRACK_END).exec().unwrap();
    lua.load(
        "NODES[25] = { x = 400, y = 0, z = 10 } TRACKS[25] = { 250 } \
         NODES[19] = { x = 400, y = 0, z = -10 } TRACKS[19] = { 190 }",
    )
    .exec()
    .unwrap();
    lua.load(format!(
        "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         ACTION = assert(capture.construction({}))",
        station_on_track_end()
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    lua.load(STATION_REFRESH).exec().unwrap();
    lua.load(TRACK_END_GAME).exec().unwrap();
    lua.load(ENABLE_TRACK_END_REJOIN).exec().unwrap();
    lua.load("HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let (ok, onto): (bool, String) = lua
        .load(
            "local e = SENT[2] and SENT[2].proposal.streetProposal.edgesToAdd[1] \
             return HOOK.applied[1].ok == true, tostring(e and e.comp.node1)",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert!(ok, "{}", hook_log(&lua));
    assert_eq!(
        onto, "30",
        "the station's own track end, not the node above"
    );
    let removed: String = lua
        .load("return table.concat(SENT[1].proposal.streetProposal.edgesToRemove, ',')")
        .eval()
        .unwrap();
    assert_eq!(
        removed, "200",
        "the piece at its own level, not the one below"
    );
}

/// As above, with the piece's far end, node 21, a junction of track 199
/// whose traffic light the player set: the build stays refused, nothing is
/// taken away, and the setting stays.
#[test]
fn a_construction_refused_on_a_track_end_keeps_a_junction_set_by_hand() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(STATION_ON_TRACK_END).exec().unwrap();
    lua.load(
        "NODES[22] = { x = 500, y = 0, z = 0 } \
         EDGES[199] = { node0 = 22, node1 = 21, type = 0, tangent0 = { x = -50, y = 0, z = 0 }, \
             tangent1 = { x = -50, y = 0, z = 0 }, objects = {}, edgeDecorations = {}, \
             roadTemplate = '::/track/standard.track_template', laneConfigs = { TRACK_LANE(false) } } \
         TRACKS[21], TRACKS[22] = { 199, 200 }, { 199 } \
         local c = api.type.BaseNodeConfig.new() c.trafficLightPreference = 1 CONFIGS[21] = c",
    )
    .exec()
    .unwrap();
    lua.load(format!(
        "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         ACTION = assert(capture.construction({}))",
        station_on_track_end()
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    lua.load(STATION_REFRESH).exec().unwrap();
    lua.load(TRACK_END_GAME).exec().unwrap();
    lua.load(ENABLE_TRACK_END_REJOIN).exec().unwrap();
    lua.load("HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let (ok, sent): (bool, usize) = lua
        .load("return HOOK.applied[1].ok == true, #SENT")
        .eval()
        .unwrap();
    assert!(!ok, "{}", hook_log(&lua));
    assert_eq!(sent, 0, "nothing built, nothing taken away");
    assert!(
        hook_log(&lua).contains("(a junction set by hand where it joins)"),
        "{}",
        hook_log(&lua)
    );
}

/// As above, with a signal on the track piece: it is not taken away, and
/// the build is refused as the game refused it, in every game.
#[test]
fn a_construction_refused_on_a_track_end_with_a_signal_stays_refused() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(STATION_ON_TRACK_END).exec().unwrap();
    lua.load("EDGES[200].objects = { { 900, 2 } }")
        .exec()
        .unwrap();
    lua.load(format!(
        "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         ACTION = assert(capture.construction({}))",
        station_on_track_end()
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    lua.load(STATION_REFRESH).exec().unwrap();
    lua.load(TRACK_END_GAME).exec().unwrap();
    lua.load(ENABLE_TRACK_END_REJOIN).exec().unwrap();
    lua.load("HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let (ok, sent): (bool, usize) = lua
        .load("return HOOK.applied[1].ok == true, #SENT")
        .eval()
        .unwrap();
    assert!(!ok, "{}", hook_log(&lua));
    assert_eq!(sent, 0, "nothing built, nothing taken away");
    assert!(
        hook_log(&lua).contains(
            "the game refuses the build: Construction Not Possible (a stop or signal on a track it joins)"
        ),
        "{}",
        hook_log(&lua)
    );
}

/// A joined station can have more than the 64 junction settings an action
/// carries, mostly for switches on track the station builds itself. Capture
/// leaves those settings with the construction before validating the action.
#[test]
fn a_large_station_does_not_send_its_own_switch_settings() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    let joined = RAIL_STATION_OPEN.replace(
        "{JOIN}",
        ", { entity = -6, type = 1, comp = { node0 = -3, node1 = 8, type = 0, typeIndex = -1, \
           tangent0 = { x = 50, y = 0, z = 0 }, tangent1 = { x = 50, y = 0, z = 0 }, \
           roadTemplate = '::/track/standard.track_template', roadStyle = '' } }",
    );
    lua.load(format!("PROPOSAL = {joined}")).exec().unwrap();
    let (valid, carried, links): (bool, usize, usize) = lua
        .load(
            r#"
            local s = PROPOSAL.proposal
            s.nodeConfigsToAdd = {}
            for i = 1, 70 do
                local node = -100 - i
                s.addedNodes[#s.addedNodes + 1] = {
                    entity = node,
                    comp = { position = { x = 300 + i, y = 10, z = 0 } }
                }
                s.addedSegments[#s.addedSegments + 1] = {
                    entity = -300 - i, type = 1,
                    comp = {
                        node0 = -3, node1 = node, type = 0, typeIndex = -1,
                        tangent0 = { x = 1, y = 10, z = 0 },
                        tangent1 = { x = 1, y = 10, z = 0 },
                        roadTemplate = '::/track/standard.track_template', roadStyle = ''
                    }
                }
                s.nodeConfigsToAdd[i] = {
                    entity = node,
                    comp = {
                        trafficLightPreference = 0, doubleSlipSwitch = false,
                        userModifiedTrafficLightStates = false,
                        laneConnections = {}, crosswalks = {},
                        trafficLightConfig = { trafficLightType = -1, states = {} }
                    }
                }
            end
            local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua')
            local action, why = capture.construction(PROPOSAL)
            if not action then error(why) end
            local connection = action.BuildConstruction.connection
            return schema_check(action), #connection.junctions, #connection.links
            "#,
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(valid, "the action stays within its junction limit");
    assert_eq!(carried, 0, "the station keeps its own switch settings");
    assert!(
        links >= 70,
        "the captured network still represents the placement"
    );
}

#[test]
fn a_depot_placed_on_existing_track_leaves_all_its_internal_branches_to_the_construction() {
    for refuse_snap in [false, true] {
        let (lua, _script) = engine();
        lua.load(FAKE_NETWORK).exec().unwrap();
        lua.load(include_str!("lua/depot_snap.lua")).exec().unwrap();
        lua.load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua')
             ACTION = assert(capture.construction(depot_on_track()))
             assert(schema_check(ACTION))
             assert(#ACTION.BuildConstruction.connection.links == 6)
             REFUSE_SNAP = {refuse_snap}
             HOOK.batch = {{ ACTION }} UPDATE({{}}, STATE, 0.2)"
        ))
        .exec()
        .unwrap();
        let (nodes, edges, sends, connected, applied): (usize, usize, usize, bool, bool) = lua
            .load(
                "local p = SENT[1].proposal.streetProposal
                 return #p.nodesToAdd, #p.edgesToAdd, #SENT, CONNECTED, HOOK.applied[1].ok",
            )
            .eval()
            .unwrap();
        assert_eq!(
            (nodes, edges),
            (0, 0),
            "do not duplicate the depot's own track"
        );
        assert_eq!(sends, 2, "place, then snap to the existing track");
        assert_eq!(connected, !refuse_snap);
        assert_eq!(
            applied, !refuse_snap,
            "never report a refused refresh as applied"
        );
    }
}

#[test]
fn a_station_with_a_long_entrance_keeps_the_external_junction_only() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(include_str!("lua/depot_snap.lua")).exec().unwrap();
    lua.load(format!(
        "local p = {STATION_BY_ROAD}
         local s = p.proposal
         s.addedNodes[#s.addedNodes+1] = {{ entity=-20, comp={{ position={{ x=90,y=0,z=0 }} }} }}
         s.addedSegments[#s.addedSegments+1] = {{ entity=-21, type=0, comp={{
             node0=-20, node1=-1, type=0, typeIndex=-1,
             tangent0={{ x=-20,y=0,z=0 }}, tangent1={{ x=-20,y=0,z=0 }},
             roadTemplate='::/street/town_small.street_template', roadStyle='' }} }}
         local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua')
         ACTION = assert(capture.construction(p))
         assert(schema_check(ACTION))
         HOOK.batch = {{ ACTION }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    let (nodes, edges, removed, first, second): (usize, usize, i64, i64, i64) = lua
        .load(
            "local s = SENT[1].proposal.streetProposal
             return #s.nodesToAdd, #s.edgesToAdd, s.edgesToRemove[1],
                 s.edgesToAdd[1].comp.node0, s.edgesToAdd[2].comp.node1",
        )
        .eval()
        .unwrap();
    assert_eq!((nodes, edges, removed, first, second), (1, 2, 100, 8, 9));
    assert!(
        lua.load("return CONNECTED and HOOK.applied[1].ok")
            .eval::<bool>()
            .unwrap()
    );
}

/// Each click handed over is bound to its ticket, and one that was not to
/// nothing, so the street and track tools continue only from what the room
/// builds for that very click (tpf3mp/toolchain.lua, crates/tpf3mp-hook/src/
/// toolchain.rs).
#[test]
fn each_handed_over_click_tells_the_hook_its_ticket() {
    let (lua, _script) = engine();
    let bound: Vec<(i64, i64)> = lua
        .load(format!(
            "HOOK.room = true HOOK.clicks = 0
             local bound = {{}}
             tpf3mp_native.chainClick = function(click, ticket)
                 bound[#bound + 1] = {{ click, ticket or -1 }}
             end
             tpf3mp_native.chainPoll = function() end
             local function ask(proposal)
                 SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'constructionBuilder',
                     'builder.proposalCreate', {{ proposal }})
             end
             SCRIPT.guiUpdate({{}}, nil, nil)
             ask({CONSTRUCTION_PROPOSAL})
             HOOK.clicks = 1 SCRIPT.guiUpdate({{}}, nil, nil)
             local unnamed = {CONSTRUCTION_PROPOSAL} unnamed.toAdd[1].name = ''
             ask(unnamed)
             HOOK.clicks = 2 SCRIPT.guiUpdate({{}}, nil, nil)
             return bound"
        ))
        .eval::<Vec<Vec<i64>>>()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)))
        .into_iter()
        .map(|b| (b[0], b[1]))
        .collect();
    assert_eq!(
        bound,
        [(0, 1), (1, -1)],
        "click 0 with ticket 1; click 1 not handed over"
    );
}

/// The GUI's half of the tools continuing: the edge the room built is
/// checked in the GUI's own state, and its end node's edges counted.
#[test]
fn the_rooms_edge_is_checked_in_the_guis_state() {
    let (lua, _script) = engine();
    let results: Vec<String> = lua
        .load(
            "local toolchain = ug_require('tpf3mp_1::/scripts/tpf3mp/toolchain.lua')
             local C = { BASE_EDGE = 'edge', BASE_EDGE_STREET = 'street', BASE_NODE = 'node' }
             local comps = {
                 edge = { [11] = { node0 = 2, node1 = 3 }, [12] = { node0 = 3, node1 = 4 },
                          [13] = { node0 = 3, node1 = 5 } },
                 street = { [11] = {}, [12] = {} },
                 node = { [2] = { position = { x = 0, y = 0, z = 0 } },
                          [3] = { position = { x = 1200.25, y = -340.5, z = 81.75 } } } }
             local segments = { [3] = { 11 } }
             local api = { type = { ComponentType = C }, engine = {
                 getComponent = function(e, c) return comps[c][e] end,
                 system = { streetSystem = {
                     getNodeSegments = function(n) return segments[n] or {} end } } } }
             local out = {}
             local function say(...)
                 local t = {}
                 for i = 1, select('#', ...) do t[i] = tostring((select(i, ...))) end
                 out[#out + 1] = table.concat(t, ' ')
             end
             -- The room's end 0.4 mm off; the new street alone at its node.
             say(toolchain.check(api, 11, 2, 3, 1, 1200.2504, -340.5, 81.75))
             -- Two streets and a track there.
             segments[3] = { 11, 12, 13 }
             say(toolchain.check(api, 11, 2, 3, 1, 1200.25, -340.5, 81.75))
             -- Other nodes, no such edge, the node 2 cm away.
             say(toolchain.check(api, 11, 3, 2, 0, 1200.25, -340.5, 81.75))
             say(toolchain.check(api, 99, 2, 3, 1, 1200.25, -340.5, 81.75))
             say(toolchain.check(api, 11, 2, 3, 1, 1200.27, -340.5, 81.75))
             local seen = {}
             local link = {
                 chainPoll = function() return 11, 2, 3, 1, 1200.25, -340.5, 81.75 end,
                 chainSeen = function(_, ...) seen[#seen + 1] = table.concat({ ... }, ' ') end }
             toolchain.frame(link, api)
             api.engine.getComponent = function() error('the game raised') end
             toolchain.frame(link, api)
             link.chainPoll = function() return nil end
             toolchain.frame(link, api)
             out[#out + 1] = table.concat(seen, ';')
             return out",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        results,
        ["1 0", "2 1", "nil", "nil", "nil", "11 2 1"],
        "the edge as the room built it, with its end node's edges counted; another \
         orientation, a missing edge or a node elsewhere is not; a frame that raises \
         tells the hook nothing"
    );
}

#[test]
fn the_build_a_click_saw_goes_to_the_room_and_other_tools_stay_refused() {
    let (lua, _script) = engine();
    let asked: Vec<String> = lua
        .load(format!(
            "HOOK.room = true HOOK.clicks = 0 \
             local out = {{}} \
             local function ask(id, proposal) \
                 local r = SCRIPT.guiHandleEvent({{}}, nil, nil, '', id, 'builder.proposalCreate', {{ proposal }}) \
                 if r == nil then return 'nil' end \
                 for text in pairs(r.errorMessages) do return text end \
             end \
             SCRIPT.guiUpdate({{}}, nil, nil) \
             local elsewhere = {CONSTRUCTION_PROPOSAL} \
             elsewhere.toAdd[1].transf[13] = 99 \
             out[#out + 1] = ask('constructionBuilder', elsewhere) \
             out[#out + 1] = ask('constructionBuilder', {CONSTRUCTION_PROPOSAL}) \
             out[#out + 1] = ask('unsupportedTool', {CONSTRUCTION_PROPOSAL}) \
             local unnamed = {CONSTRUCTION_PROPOSAL} unnamed.toAdd[1].name = '' \
             HOOK.clicks = 1 \
             out[#out + 1] = ask('constructionBuilder', unnamed) \
             return out"
        ))
        .eval()
        .unwrap();
    assert_eq!(
        asked,
        [
            "nil",
            "nil",
            "Not in multiplayer yet: building with this tool",
            "Not in multiplayer yet: an unnamed construction"
        ],
        "the construction tool builds through the room; a proposal it cannot carry says why"
    );
    // The click: the last proposal before it goes to the room.
    lua.load("SCRIPT.guiUpdate({}, nil, nil)").exec().unwrap();
    let (handed, x): (usize, f64) = lua
        .load("return #HOOK.commands, HOOK.commands[1].BuildConstruction.transform.origin.x")
        .eval()
        .unwrap();
    assert_eq!(handed, 1);
    assert!(
        (x + 421.935_729_980_47).abs() < 1e-9,
        "the last one, not the first"
    );
    // A click on a proposal it could not carry hands nothing over.
    lua.load("HOOK.clicks = 2 SCRIPT.guiUpdate({}, nil, nil)")
        .exec()
        .unwrap();
    let handed: usize = lua.load("return #HOOK.commands").eval().unwrap();
    assert_eq!(handed, 1);
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged.contains(
            &"handed the player's build to the room \
              [+c::/depots/road/road_maint_station.con{frozen 0n 0e}]"
                .to_owned()
        ),
        "what was handed over, for the log: {logged:?}"
    );
    assert!(
        logged.contains(
            &"stopped a build the room cannot carry: an unnamed construction \
              [+c::/depots/road/road_maint_station.con{frozen 0n 0e}]"
                .to_owned()
        ),
        "{logged:?}"
    );
    assert!(
        logged.contains(
            &"the room does not carry the unsupportedTool tool yet (?) \
              [+c::/depots/road/road_maint_station.con{frozen 0n 0e}]"
                .to_owned()
        ),
        "a tool the room does not carry logs what it proposed: {logged:?}"
    );
    // Where the hook cannot stop the player's builds, every tool is refused.
    let without: String = lua
        .load(format!(
            "HOOK.clicks = nil \
             local r = SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'constructionBuilder', \
                 'builder.proposalCreate', {{ {CONSTRUCTION_PROPOSAL} }}) \
             for text in pairs(r.errorMessages) do return text end"
        ))
        .eval()
        .unwrap();
    assert_eq!(without, "Not in multiplayer yet: building with this tool");
}

#[test]
fn the_rooms_builds_are_applied_as_replays() {
    let (lua, _script) = engine();
    lua.load(format!(
        "HOOK.batch = {{ {DEPOT} }} UPDATE({{}}, STATE, 0.2) UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    let replaying: Vec<bool> = lua.load("return HOOK.replaying").eval().unwrap();
    assert_eq!(
        replaying,
        [true, false],
        "on around the room's actions only"
    );
}

#[test]
fn in_the_rooms_game_the_build_tools_are_refused() {
    let (lua, _script) = engine();
    let refusals: Vec<String> = lua
        .load(
            "local out = {}
             local function ask(name)
                 local r = SCRIPT.guiHandleEvent({}, nil, nil, '', 'streetBuilder', name, {})
                 if r == nil then return 'nil' end
                 local texts = {}
                 for text in pairs(r.errorMessages or {}) do texts[#texts + 1] = text end
                 return table.concat(texts, ',')
             end
             out[#out + 1] = ask('builder.proposalCreate')
             HOOK.room = true
             out[#out + 1] = ask('builder.proposalCreate')
             out[#out + 1] = ask('builder.proposalPrepareForApply')
             out[#out + 1] = ask('builder.proposalApply')
             out[#out + 1] = ask('select')
             return out",
        )
        .eval()
        .unwrap();
    assert_eq!(
        refusals,
        [
            "nil",
            "Not in multiplayer yet: building with this tool",
            "Not in multiplayer yet: building with this tool",
            "nil",
            "nil"
        ],
        "outside the room's game nothing; in it every proposal a tool makes"
    );
}

#[test]
fn an_action_the_game_script_cannot_apply_is_logged_not_raised() {
    let (lua, _script) = engine();
    lua.load(
        "HOOK.batch = { { Teleport = {} } } UPDATE({}, STATE, 0.2) \
         REFUSE = true",
    )
    .exec()
    .unwrap();
    lua.load(format!(
        "HOOK.batch = {{ {DEPOT} }} UPDATE({{}}, STATE, 0.2) \
         REFUSE, FAILS = false, true \
         HOOK.batch = {{ {DEPOT} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert_eq!(logged.len(), 4, "{logged:?}");
    // One the game ran and answered as failed.
    assert_eq!(
        logged[3],
        "action 1 of this step was not applied: the game refused it"
    );
    assert_eq!(logged[0], "the game script is linked");
    assert_eq!(
        logged[1],
        "action 1 of this step was not applied: this version of the mod does not apply Teleport yet"
    );
    // The game's own refusal, as it raised it.
    assert!(
        logged[2].starts_with("action 1 of this step was not applied: ")
            && logged[2].ends_with("the proposal collides"),
        "{}",
        logged[2]
    );
}

#[test]
fn the_console_event_hands_an_action_to_the_room() {
    let (lua, script) = engine();
    let handle: Function = script.get("handleEvent").unwrap();
    lua.globals().set("HANDLE", handle).unwrap();
    lua.load(format!(
        "HANDLE({{}}, STATE, 'console', 'tpf3mp', 'command', {DEPOT}) \
         HANDLE({{}}, STATE, 'console', 'other', 'command', {DEPOT}) \
         HANDLE({{}}, STATE, 'console', 'tpf3mp', 'command', {{ Nope = 1 }})"
    ))
    .exec()
    .unwrap();
    assert_eq!(
        lua.load("return #HOOK.commands").eval::<usize>().unwrap(),
        1,
        "only its own event, and only an action the schema takes"
    );
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert_eq!(logged[0], "the game script is linked");
    assert_eq!(logged[1], "handed a test action to the room");
    assert!(
        logged[2].starts_with("refused a test action: "),
        "{}",
        logged[2]
    );
}

/// A street network for the road tests, over the stand-in engine state: the
/// street 8-9 (edge 100) running north through (50, 0), and node 7 at the
/// origin with a street of its own (edge 101).
const FAKE_NETWORK: &str = r#"
local CT = { BASE_NODE = 11, BASE_EDGE = 12, BASE_NODE_CONFIG = 13 }
api.type.ComponentType = CT
api.type.enum = { BaseEdgeType = { NORMAL = 0, BRIDGE = 1, TUNNEL = 2 },
                  RoadType = { STREET = 0, TRACK = 1 }, TrafficLightPreference = { AUTO=0, YES=1, NO=2 } }
api.type.Vec3f = { new = function(x, y, z) return { x = x, y = y, z = z } end }
api.type.NodeAndEntity = { new = function() return { comp = {} } end }
api.type.SegmentAndEntity = { new = function() return { comp = {} } end }
api.type.SimpleProposal.new = function() return { constructionsToAdd = {}, streetProposal = {} } end
api.type.BaseNodeConfig = { new = function() return { laneConnections={}, crosswalks={},
    trafficLightPreference=0, trafficLightConfig={ states={}, trafficLightType=-1 },
    doubleSlipSwitch=false, userModifiedTrafficLightStates=false } end }
api.type.BaseNodeLaneConnectionAndEntity = { new = function() return {} end }
api.type.LaneConnection = { new = function() return {} end }
api.type.TrafficLightState = { new = function() return {} end }
local TEMPLATES = { ['::/street/town_small.street_template'] = 4, ['::/street/country.street_template'] = 5 }
api.res = {
    streetTemplateRep = {
        find = function(name) return TEMPLATES[name] or -1 end,
        get = function(id)
            if id == 4 then return { laneConfigs = { 'town lanes' }, streetStyle = '::/style/town.street_style' } end
            if id == 5 then return { laneConfigs = { 'country lanes' }, streetStyle = '::/style/country.street_style' } end
        end,
    },
    bridgeTypeRep = {
        find = function(name) if name == '::/bridge/stone.lua' then return 3 end return -1 end,
        getName = function(id) if id == 3 then return '::/bridge/stone.lua' end end,
    },
    tunnelTypeRep = { find = function() return -1 end, getName = function() end },
}
NODES = { [7] = { x = 0, y = 0, z = 0 }, [8] = { x = 50, y = -40, z = 0 }, [9] = { x = 50, y = 40, z = 0 },
          [10] = { x = -60, y = 0, z = 0 } }
EDGES = {
    [100] = { node0 = 8, node1 = 9, tangent0 = { x = 0, y = 80, z = 0 }, tangent1 = { x = 0, y = 80, z = 0 },
              objects = {}, roadTemplate = '::/street/country.street_template', laneConfigs = { 'country lanes' } },
    [101] = { node0 = 10, node1 = 7, tangent0 = { x = 60, y = 0, z = 0 }, tangent1 = { x = 60, y = 0, z = 0 },
              objects = {}, roadTemplate = '::/street/town_small.street_template' },
}
STREETS = { [7] = { 101 }, [8] = { 100 }, [9] = { 100 }, [10] = { 101 } }
-- The nodes with a lane configuration.
CONFIGS = { [8] = true, [9] = true, [11] = true }
api.engine.getComponent = function(id, kind)
    if kind == CT.BASE_NODE and NODES[id] then return { position = NODES[id] } end
    if kind == CT.BASE_NODE_CONFIG and CONFIGS[id] then return type(CONFIGS[id]) == "table" and CONFIGS[id] or api.type.BaseNodeConfig.new() end
    if kind == CT.BASE_EDGE and EDGES[id] then
        -- A copy, as the game hands out.
        local c = {}
        for k, v in pairs(EDGES[id]) do c[k] = v end
        return c
    end
end
api.engine.system = { lineSystem = { getLines = function() return {} end }, streetSystem = {
    getNode2StreetEdgeMap = function()
        local m = {}
        for node, edges in pairs(STREETS) do m[node] = edges end
        return m
    end,
    getNode2TrackEdgeMap = function() return {} end,
    getNodeStreetSegments = function(node) return STREETS[node] or {} end,
    getNodeTrackSegments = function() return {} end,
} }
"#;

/// A road the room ordered, as the hook hands it (metres): from node 7, onto
/// the middle of the street 8-9, and on over a bridge to open ground.
const ROAD: &str = "{ BuildRoad = { street = '::/street/town_small.street_template', \
    bus_lane = false, tram = 'None', polyline = { \
    vertices = { \
        { pos = { x = 0.0004, y = 0.001, z = 0 }, resolve = { Node = 'Street' } }, \
        { pos = { x = 50, y = 0, z = 0 }, resolve = { Split = { network = 'Street', \
            ends = { a = { x = 50, y = 40, z = 0 }, b = { x = 50, y = -40, z = 0 } } } } }, \
        { pos = { x = 120, y = 0, z = 12 }, resolve = 'New' } }, \
    links = { \
        { from = 0, to = 1, tangent0 = { x = 50, y = 0, z = 0 }, tangent1 = { x = 50, y = 0, z = 0 }, \
          structure = 'Ground' }, \
        { from = 1, to = 2, tangent0 = { x = 70, y = 0, z = 12 }, tangent1 = { x = 70, y = 0, z = 12 }, \
          structure = { Bridge = '::/bridge/stone.lua' } } }, \
    removals = {} } } }";

/// Every game builds a track with its template's distance between track
/// centres (`trackDistance`), as the track tool does: without it the game
/// lays no shared ballast bed or catenary with the tracks beside it, and
/// the ground shows between them (2026-10-02). A street gets none.
#[test]
fn a_track_the_room_ordered_has_its_templates_track_distance() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(
        r#"
        local get = api.res.streetTemplateRep.get
        local find = api.res.streetTemplateRep.find
        api.res.streetTemplateRep.find = function(name)
            if name == '::/track/standard.street_template' then return 6 end
            return find(name)
        end
        api.res.streetTemplateRep.get = function(id)
            if id == 6 then
                return { laneConfigs = { 'track lanes' }, streetStyle = '::/style/track.street_style',
                         trackDistance = 5 }
            end
            return get(id)
        end
        local function across(network, template)
            local polyline = { vertices = {
                    { pos = { x = 200, y = 0, z = 0 }, resolve = 'New' },
                    { pos = { x = 300, y = 0, z = 0 }, resolve = 'New' } },
                links = { { from = 0, to = 1, tangent0 = { x = 100, y = 0, z = 0 },
                    tangent1 = { x = 100, y = 0, z = 0 }, structure = 'Ground' } },
                removals = {} }
            if network == 'Track' then
                return { BuildTrack = { track = template, catenary = true, polyline = polyline } }
            end
            return { BuildRoad = { street = template, bus_lane = false, tram = 'None', polyline = polyline } }
        end
        HOOK.batch = { across('Track', '::/track/standard.street_template'),
                       across('Street', '::/street/country.street_template') }
        UPDATE({}, STATE, 0.2)
        "#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let distances: Vec<String> = lua
        .load(
            "local out = {} \
             for i, c in ipairs(SENT) do \
                 out[i] = tostring(c.proposal.streetProposal.edgesToAdd[1].comp.distance) \
             end \
             return out",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(distances, ["5", "nil"], "{}", log(&lua));
}

/// A track laid onto a street's end starts where the track tool put it: on
/// the street's tram lane, 2 m beside the end node, where alone the game
/// joins the tram lane to the track (build 40420, schema 27). Built from the
/// node's centre instead, the track ended in a buffer stop in every game of
/// the room (2026-10-15). A link that names no end of its own ends on its
/// vertex.
#[test]
fn a_track_onto_a_street_end_starts_on_the_tram_lane() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(
        r#"
        local get = api.res.streetTemplateRep.get
        local find = api.res.streetTemplateRep.find
        api.res.streetTemplateRep.find = function(name)
            if name == '::/track/standard.street_template' then return 6 end
            return find(name)
        end
        api.res.streetTemplateRep.get = function(id)
            if id == 6 then
                return { laneConfigs = { 'track lanes' }, streetStyle = '::/style/track.street_style' }
            end
            return get(id)
        end
        local apply = ug_require('tpf3mp_1::/scripts/tpf3mp/apply.lua')
        local function onto(start)
            local link = { from = 0, to = 1, tangent0 = { x = -50, y = -8, z = 0 },
                tangent1 = { x = -50, y = -8, z = 0 }, structure = 'Ground', position0 = start }
            local p = apply.proposalOf({ BuildTrack = { track = '::/track/standard.street_template',
                catenary = false, polyline = { vertices = {
                    { pos = { x = 0, y = 0, z = 0 }, resolve = { Node = 'Street' } },
                    { pos = { x = -50, y = -10, z = 0 }, resolve = 'New' } },
                links = { link }, removals = {} } } }, {})
            local s = p.streetProposal.edgesToAdd[1].comp
            return string.format("%d>%d %.3f,%.3f,%.3f %.3f,%.3f,%.3f", s.node0, s.node1,
                s.position0.x, s.position0.y, s.position0.z, s.position1.x, s.position1.y, s.position1.z)
        end
        BESIDE = onto({ x = -0.315, y = -1.975, z = 0 })
        CENTRE = onto(nil)
        "#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}
{}", log(&lua)));
    let (beside, centre): (String, String) = lua.load("return BESIDE, CENTRE").eval().unwrap();
    assert_eq!(
        beside, "7>-2 -0.315,-1.975,0.000 -50.000,-10.000,0.000",
        "on the street's node 7, starting on its tram lane"
    );
    assert_eq!(
        centre, "7>-2 0.000,0.000,0.000 -50.000,-10.000,0.000",
        "no end of its own: on the node"
    );
}

#[test]
fn a_roads_preview_is_the_proposal_its_build_would_send() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(format!(
        "local apply = ug_require('tpf3mp_1::/scripts/tpf3mp/apply.lua')          apply.log = function(line) HOOK.logged[#HOOK.logged + 1] = line end          P, C = apply.proposalOf({ROAD}, {{}})"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    let (nodes, edges, removed, sent, logged): (usize, usize, String, usize, usize) = lua
        .load(
            "local p = P.streetProposal              return #p.nodesToAdd, #p.edgesToAdd, table.concat(p.edgesToRemove, ','), #SENT, #HOOK.logged",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        (nodes, edges, removed.as_str(), sent, logged),
        (2, 4, "100", 0, 0),
        "the build's nodes and edges, the split street's edge removed, nothing sent or said"
    );
}

#[test]
fn the_game_script_builds_a_road_as_the_players_tool_would() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(format!(
        "HOOK.batch = {{ {ROAD} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        lua.load("return table.concat(HOOK.logged, '|')")
            .eval::<String>()
            .unwrap(),
        "the game script is linked|building +n-5(50.0,0.0,0.0) +n-6(120.0,0.0,12.0) \
         +e-1/0:7>-5 ::/street/town_small.street_template \
         +e-2/0:-5>-6 ::/street/town_small.street_template \
         +e-3/0:8>-5 ::/street/country.street_template \
         +e-4/0:-5>9 ::/street/country.street_template -e100 -n -c8,9 \
         +j8{lanes 0 tram 0} +j9{lanes 0 tram 0}",
        "applied, and what was sent in the log: the split street's ends lose their lane \
         configurations with it, and get them back"
    );
    let built: String = lua
        .load(
            "local c = SENT[1] local p = c.proposal.streetProposal
             local out = { #p.nodesToAdd, #p.edgesToAdd, table.concat(p.edgesToRemove, ','),
                           tostring(c.context.player), tostring(c.ignoreErrors), tostring(c.playerInitiated) }
             for _, n in ipairs(p.nodesToAdd) do
                 out[#out + 1] = n.entity .. '@' .. n.comp.position.x .. ',' .. n.comp.position.y .. ',' .. n.comp.position.z
             end
             for _, e in ipairs(p.edgesToAdd) do
                 local c = e.comp
                 out[#out + 1] = string.format('%d:%d>%d t%d/%s %s %s %.3f,%.3f %.3f,%.3f', e.entity, c.node0, c.node1,
                     e.type, tostring(c.type), tostring(c.typeIndex), tostring(c.roadTemplate),
                     c.tangent0.x, c.tangent0.y, c.tangent1.x, c.tangent1.y)
             end
             return table.concat(out, ' | ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        built,
        "2 | 4 | 100 | 25 | true | true \
         | -5@50,0,0 | -6@120,0,12 \
         | -1:7>-5 t0/0 -1 ::/street/town_small.street_template 50.000,0.000 50.000,0.000 \
         | -2:-5>-6 t0/1 3 ::/street/town_small.street_template 70.000,0.000 70.000,0.000 \
         | -3:8>-5 t0/nil nil ::/street/country.street_template 0.000,40.000 0.000,40.000 \
         | -4:-5>9 t0/nil nil ::/street/country.street_template 0.000,40.000 0.000,40.000",
        "the links from -1, then the split's halves keeping the street's own template; \
         the new nodes after the edges"
    );
    // The links take the template's lanes and style; the halves keep theirs.
    let lanes: String = lua
        .load(
            "local e = SENT[1].proposal.streetProposal.edgesToAdd
             return e[1].comp.laneConfigs[1] .. '|' .. e[1].comp.roadStyle .. '|' .. e[3].comp.laneConfigs[1]",
        )
        .eval()
        .unwrap();
    assert_eq!(lanes, "town lanes|::/style/town.street_style|country lanes");
}

#[test]
fn a_road_that_resolves_to_nothing_is_built_nowhere() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    // Node 7 has moved 3 m: nothing is within 1.5 m of the vertex.
    lua.load(format!(
        "NODES[7] = {{ x = 3, y = 0, z = 0 }} HOOK.batch = {{ {ROAD} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    // A stop on the street it splits.
    lua.load(format!(
        "NODES[7] = {{ x = 0, y = 0, z = 0 }} EDGES[100].objects = {{ {{ 555, 1 }} }} \
         HOOK.batch = {{ {ROAD} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    assert_eq!(lua.load("return #SENT").eval::<usize>().unwrap(), 0);
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged[1].ends_with("no Street node at vertex 1"),
        "{logged:?}"
    );
    assert!(
        logged[2].ends_with("vertex 2 splits an edge with a stop or signal on it"),
        "{logged:?}"
    );
}

/// The street tool's proposal for the road of ROAD, as build 40408 hands it
/// to game scripts: the split of 8-9 is the removed edge and its two halves.
const STREET_PROPOSAL: &str = "{ toAdd = {}, toRemove = {}, proposal = { \
    addedNodes = { { entity = -1, comp = { position = { x = 50, y = 0, z = 0 } } }, \
                   { entity = -2, comp = { position = { x = 120, y = 0, z = 12 } } } }, \
    addedSegments = { \
        { entity = -3, type = 0, comp = { node0 = 7, node1 = -1, type = 0, typeIndex = -1, \
          tangent0 = { x = 50, y = 0, z = 0 }, tangent1 = { x = 50, y = 0, z = 0 }, \
          roadTemplate = '::/street/town_small.street_template', roadStyle = '' } }, \
        { entity = -4, type = 0, comp = { node0 = 9, node1 = -1, type = 0, typeIndex = -1, \
          tangent0 = { x = 0, y = -40, z = 0 }, tangent1 = { x = 0, y = -40, z = 0 }, \
          roadTemplate = '::/street/country.street_template', roadStyle = '' } }, \
        { entity = -5, type = 0, comp = { node0 = -1, node1 = 8, type = 0, typeIndex = -1, \
          tangent0 = { x = 0, y = -40, z = 0 }, tangent1 = { x = 0, y = -40, z = 0 }, \
          roadTemplate = '::/street/country.street_template', roadStyle = '' } }, \
        { entity = -6, type = 0, comp = { node0 = -1, node1 = -2, type = 1, typeIndex = 3, \
          tangent0 = { x = 70, y = 0, z = 12 }, tangent1 = { x = 70, y = 0, z = 12 }, \
          roadTemplate = '::/street/town_small.street_template', roadStyle = '' } } }, \
    removedSegments = { { entity = 100, type = 0, comp = { node0 = 9, node1 = 8 } } }, \
    removedNodes = {}, edgeObjectsToAdd = {} } }";

#[test]
fn a_road_the_street_tool_proposed_goes_to_the_room() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    let asked: Vec<String> = lua
        .load(format!(
            "HOOK.room = true HOOK.clicks = 0 \
             local out = {{}} \
             local function ask(proposal) \
                 local r = SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'streetBuilder', 'builder.proposalCreate', {{ proposal }}) \
                 if r == nil then return 'nil' end \
                 for text in pairs(r.errorMessages) do return text end \
             end \
             SCRIPT.guiUpdate({{}}, nil, nil) \
             out[#out + 1] = ask({{ toAdd = {{}}, toRemove = {{}}, proposal = {{ addedNodes = {{}}, \
                 addedSegments = {{}}, removedSegments = {{}} }} }}) \
             out[#out + 1] = ask({STREET_PROPOSAL}) \
             return out"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        asked,
        ["nil", "nil"],
        "the tool builds through the room, and a proposal of nothing is not refused"
    );
    lua.load("HOOK.clicks = 1 SCRIPT.guiUpdate({}, nil, nil)")
        .exec()
        .unwrap();
    let handed: String = lua
        .load(
            "local b = HOOK.commands[1].BuildRoad local p = b.polyline
             return table.concat({ #HOOK.commands, b.street, tostring(b.style), #p.vertices, #p.links,
                 #p.removals, #p.removed_nodes, p.vertices[1].resolve.Node, tostring(p.vertices[2].resolve),
                 p.links[2].kind.template, p.links[4].structure.Bridge, p.removals[1].ends.a.y,
                 p.removals[1].ends.b.y }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| {
            panic!(
                "{error}\n{:?}",
                lua.load("return HOOK.logged").eval::<Vec<String>>()
            )
        });
    assert_eq!(
        handed,
        "1|::/street/town_small.street_template|nil|5|4|1|0|Street|New\
         |::/street/country.street_template|::/bridge/stone.lua|40|-40",
        "the proposal as the tool made it: the street it joins rebuilt in its own kind"
    );
    // What the room orders, every game builds.
    lua.load("HOOK.batch = { HOOK.commands[1] } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let (edges, removed): (usize, String) = lua
        .load(
            "local p = SENT[1].proposal.streetProposal \
             return #p.edgesToAdd, table.concat(p.edgesToRemove, ',')",
        )
        .eval()
        .unwrap();
    assert_eq!((edges, removed.as_str()), (4, "100"));
}

#[test]
fn street_precedence_survives_capture_and_room_replay() {
    let (lua, _) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(format!(r#"
        HOOK.room = true HOOK.clicks = 0
        local proposal = {STREET_PROPOSAL}
        proposal.proposal.addedSegments[1].streetEdge = {{ precedenceNode0 = 0, precedenceNode1 = 2 }}
        SCRIPT.guiUpdate({{}}, nil, nil)
        SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'streetBuilder', 'builder.proposalCreate', {{ proposal }})
        HOOK.clicks = 1 SCRIPT.guiUpdate({{}}, nil, nil)
        local p = assert(HOOK.commands[1]).BuildRoad.polyline.links[1].precedence
        assert(p.node0 == 0 and p.node1 == 2)
        HOOK.batch = {{ HOOK.commands[1] }} UPDATE({{}}, STATE, 0.2)
        local e = SENT[1].proposal.streetProposal.edgesToAdd[1].streetEdge
        assert(e.precedenceNode0 == 0 and e.precedenceNode1 == 2)
    "#)).exec().unwrap();
}

/// The level crossing types the game offers here (6 hidden), and the hook's
/// reading of the street and track tools' toolbar type by click (`TOOL[n]`:
/// `{ kind = }`, `{ kind = false, why = }` or `{ why = }`), taken once.
const FAKE_TOOL_CROSSINGS: &str = r#"
TYPES = { [3] = 'railroad_crossing/era_a_eu.rcr', [5] = 'railroad_crossing/era_c_eu.rcr', [6] = 'hidden.rcr' }
api.res.railroadCrossingTypeRep = {
    getName = function(i) return TYPES[i] end,
    find = function(name) for i, n in pairs(TYPES) do if n == name then return i end end return -1 end,
    isVisible = function(i) return i ~= 6 end,
}
TOOL = {}
tpf3mp_native.toolCrossing = function(n)
    local r = TOOL[n]
    TOOL[n] = nil
    if r == nil then return nil end
    if r.why ~= nil then return r.kind, r.why end
    return r.kind
end
"#;

/// The street tool's click carries the level crossing type its toolbar
/// picked, by its file, once `crossingTypes` is on; while it is off, the
/// build goes without it, as before. With it on, a click read as picking
/// none goes as it is; everything else stops the build and the player is
/// told: a hook that cannot tell, and a type that does not read (forced,
/// mixed, hidden, nothing kept).
#[test]
fn a_street_tool_click_carries_its_toolbars_crossing_type() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_TOOL_CROSSINGS).exec().unwrap();
    lua.load(format!(
        r#"HOOK.room = true HOOK.clicks = 0
        SCRIPT.guiUpdate({{}}, nil, nil)
        function CLICK(read)
            SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'streetBuilder', 'builder.proposalCreate', {{ {STREET_PROPOSAL} }})
            TOOL[HOOK.clicks] = read
            HOOK.clicks = HOOK.clicks + 1
            SCRIPT.guiUpdate({{}}, nil, nil)
        end
        ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').crossingTypes = false
        CLICK({{ kind = 5 }})
        ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').crossingTypes = true
        CLICK({{ kind = 5 }})
        CLICK({{ kind = false }})
        CLICK({{ kind = false, why = 'the profile does not find the maps' }})
        CLICK({{ why = "a build that forces a level crossing's type" }})
        CLICK({{ kind = 6 }})
        CLICK(nil)
        NOTICE = HOOK.notes and HOOK.notes['tpf3mp.notice']
        tpf3mp_native.toolCrossing = nil
        CLICK(nil)"#
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let sent: Vec<String> = lua
        .load(
            "local out = {} \
             for i, c in ipairs(HOOK.commands) do out[i] = tostring(c.BuildRoad.crossing_type) end \
             return out",
        )
        .eval()
        .unwrap();
    assert_eq!(
        sent,
        ["nil", "railroad_crossing/era_c_eu.rcr", "nil"],
        "{}",
        log(&lua)
    );
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged.iter().any(|l| l.contains(
            "the build's level crossing type is left out: crossingTypes awaits two-player game acceptance"
        )),
        "{logged:?}"
    );
    for why in [
        "the profile does not find the maps",
        "a build that forces a level crossing's type",
        "the hook kept nothing for the click",
        "the hook does not read the tools' level crossing type",
    ] {
        let line = format!("stopped a build the room cannot carry: {why}");
        assert!(
            logged.iter().any(|l| l.contains(&line)),
            "{line}: {logged:?}"
        );
    }
    assert!(
        logged
            .iter()
            .any(|l| l.contains("crossing type hidden.rcr is not offered here")),
        "{logged:?}"
    );
    assert_eq!(
        lua.load("return NOTICE").eval::<String>().unwrap(),
        "4\tNot in multiplayer yet: a level crossing type picked in the build tool \
         (the hook kept nothing for the click)"
    );
}

/// Every game lays a track across the street 8-9 with the toolbar's level
/// crossing type as the tool maps it, each vertex's node to -2 - type
/// (preferred: a crossing there already keeps its own), reads the crossing
/// laid back and says when the game did not take the type; a type this game
/// does not offer is refused before anything is sent, alike in every game.
#[test]
fn every_game_lays_a_new_crossing_of_the_toolbars_type() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_TOOL_CROSSINGS).exec().unwrap();
    lua.load(
        r#"
        ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').crossingTypes = true
        local find, get = api.res.streetTemplateRep.find, api.res.streetTemplateRep.get
        api.res.streetTemplateRep.find = function(name)
            if name == '::/track/standard.street_template' then return 6 end
            return find(name)
        end
        api.res.streetTemplateRep.get = function(id)
            if id == 6 then return { laneConfigs = { 'track lanes' }, streetStyle = '::/style/track.street_style' } end
            return get(id)
        end
        api.type.ComponentType.RAILROAD_CROSSING = 14
        CROSSINGS = {}
        local component = api.engine.getComponent
        api.engine.getComponent = function(e, kind)
            if kind == 14 then return CROSSINGS[e] end
            return component(e, kind)
        end
        api.engine.system.railRoadCrossingSystem = { getRailroadCrossingForNode = function(node)
            for e, c in pairs(CROSSINGS) do
                for _, n in ipairs(c.nodes) do if n == node then return e end end
            end
            return -1
        end }
        -- The game's build: the street split at (50, 0) by node 50, the
        -- crossing 800 laid there of the type LAID (nil: the map's).
        local send = api.cmd.sendCommand
        api.cmd.sendCommand = function(command, callback)
            local map = command.proposal and command.proposal.streetProposal.node2rcType
            if map then
                NODES[50] = { x = 50, y = 0, z = 0 }
                EDGES[102] = { node0 = 8, node1 = 50, objects = {} }
                EDGES[103] = { node0 = 50, node1 = 9, objects = {} }
                STREETS[8], STREETS[9], STREETS[50] = { 102 }, { 103 }, { 102, 103 }
                CROSSINGS[800] = { nodes = { 50 }, typeIndex = LAID or (-2 - map[-6]) }
            end
            return send(command, callback)
        end
        function TRACK(crossing_type)
            return { BuildTrack = { track = '::/track/standard.street_template', catenary = false,
                crossing_type = crossing_type, polyline = { vertices = {
                    { pos = { x = 20, y = 0, z = 0 }, resolve = 'New' },
                    { pos = { x = 50, y = 0, z = 0 }, resolve = { Split = { network = 'Street',
                        ends = { a = { x = 50, y = -40, z = 0 }, b = { x = 50, y = 40, z = 0 } } } } },
                    { pos = { x = 80, y = 0, z = 0 }, resolve = 'New' } },
                links = {
                    { from = 0, to = 1, tangent0 = { x = 30, y = 0, z = 0 }, tangent1 = { x = 30, y = 0, z = 0 },
                      structure = 'Ground' },
                    { from = 1, to = 2, tangent0 = { x = 30, y = 0, z = 0 }, tangent1 = { x = 30, y = 0, z = 0 },
                      structure = 'Ground' } },
                removals = {} } } }
        end
        HOOK.batch = { TRACK('railroad_crossing/era_c_eu.rcr') }
        UPDATE({}, STATE, 0.2)
        "#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    let map: String = lua
        .load(
            "local m = SENT[1].proposal.streetProposal.node2rcType local keys = {} \
             for k, v in pairs(m) do keys[#keys + 1] = k .. '=' .. v end \
             table.sort(keys) return table.concat(keys, ' ')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert_eq!(
        map, "-5=-7 -6=-7 -7=-7",
        "every vertex's node, the split's included, to -2 - 5"
    );
    let applied: String = lua
        .load("return tostring(HOOK.applied[1].ok) .. ' ' .. tostring(HOOK.applied[1].why)")
        .eval()
        .unwrap();
    assert_eq!(applied, "true nil", "{}", hook_log(&lua));
    assert!(
        !hook_log(&lua).contains("took type"),
        "the crossing laid is of the type: {}",
        hook_log(&lua)
    );

    // A game that did not take the map: said, the build applied alike.
    lua.load(
        "LAID = 3 NODES[50] = nil EDGES[102], EDGES[103] = nil, nil CROSSINGS = {} \
         STREETS[8], STREETS[9], STREETS[50] = { 100 }, { 100 }, nil \
         HOOK.batch = { TRACK('railroad_crossing/era_c_eu.rcr') } UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert!(
        hook_log(&lua).contains(
            "level crossing 800 at vertex 2 took type 3, not 5 (the toolbar's railroad_crossing/era_c_eu.rcr is 5)"
        ),
        "{}",
        hook_log(&lua)
    );

    // A type this game does not offer, and the channel off: refused, nothing sent.
    lua.load(
        "SENT = {} HOOK.applied = {} \
         HOOK.batch = { TRACK('hidden.rcr') } UPDATE({}, STATE, 0.2) \
         ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').crossingTypes = false \
         HOOK.batch = { TRACK('railroad_crossing/era_c_eu.rcr') } UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    let refused: Vec<String> = lua
        .load(
            "local out = {} for i, a in ipairs(HOOK.applied) do \
             out[i] = tostring(a.ok) .. ' ' .. tostring(a.why) end return out",
        )
        .eval()
        .unwrap();
    assert_eq!(
        refused,
        [
            "false crossing type hidden.rcr is not offered here",
            "false crossingTypes awaits two-player game acceptance"
        ]
    );
    assert_eq!(lua.load("return #SENT").eval::<usize>().unwrap(), 0);
}

/// The track tool's proposal for a track from (20, 0) to (80, 0) across the
/// street 8-9, as the tool hands it to game scripts: the crossing's node -1
/// new, the street laid again as its two halves to it, edge 100 removed.
const TRACK_PROPOSAL: &str = "{ toAdd = {}, toRemove = {}, proposal = { \
    addedNodes = { { entity = -1, comp = { position = { x = 50, y = 0, z = 0 } } }, \
                   { entity = -2, comp = { position = { x = 20, y = 0, z = 0 } } }, \
                   { entity = -3, comp = { position = { x = 80, y = 0, z = 0 } } } }, \
    addedSegments = { \
        { entity = -4, type = 1, comp = { node0 = -2, node1 = -1, type = 0, typeIndex = -1, \
          tangent0 = { x = 30, y = 0, z = 0 }, tangent1 = { x = 30, y = 0, z = 0 }, \
          roadTemplate = '::/track/standard.street_template', roadStyle = '' } }, \
        { entity = -5, type = 1, comp = { node0 = -1, node1 = -3, type = 0, typeIndex = -1, \
          tangent0 = { x = 30, y = 0, z = 0 }, tangent1 = { x = 30, y = 0, z = 0 }, \
          roadTemplate = '::/track/standard.street_template', roadStyle = '' } }, \
        { entity = -6, type = 0, comp = { node0 = 9, node1 = -1, type = 0, typeIndex = -1, \
          tangent0 = { x = 0, y = -40, z = 0 }, tangent1 = { x = 0, y = -40, z = 0 }, \
          roadTemplate = '::/street/country.street_template', roadStyle = '' } }, \
        { entity = -7, type = 0, comp = { node0 = -1, node1 = 8, type = 0, typeIndex = -1, \
          tangent0 = { x = 0, y = -40, z = 0 }, tangent1 = { x = 0, y = -40, z = 0 }, \
          roadTemplate = '::/street/country.street_template', roadStyle = '' } } }, \
    removedSegments = { { entity = 100, type = 0, comp = { node0 = 9, node1 = 8 } } }, \
    removedNodes = {}, edgeObjectsToAdd = {} } }";

/// A track the tool laid across a street, from the tool's own proposal
/// through the room to every game's build: the toolbar's type goes with it,
/// every game maps the build's nodes to it, and the crossing laid at the
/// new node where the street's halves meet the track is read back.
#[test]
fn a_track_the_tool_laid_across_a_street_takes_its_toolbars_crossing_type() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_TOOL_CROSSINGS).exec().unwrap();
    lua.load(format!(
        r#"
        ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').crossingTypes = true
        local find, get = api.res.streetTemplateRep.find, api.res.streetTemplateRep.get
        api.res.streetTemplateRep.find = function(name)
            if name == '::/track/standard.street_template' then return 6 end
            return find(name)
        end
        api.res.streetTemplateRep.get = function(id)
            if id == 6 then return {{ laneConfigs = {{ 'track lanes' }}, streetStyle = '::/style/track.street_style' }} end
            return get(id)
        end
        api.type.ComponentType.RAILROAD_CROSSING = 14
        CROSSINGS = {{}}
        local component = api.engine.getComponent
        api.engine.getComponent = function(e, kind)
            if kind == 14 then return CROSSINGS[e] end
            return component(e, kind)
        end
        api.engine.system.railRoadCrossingSystem = {{ getRailroadCrossingForNode = function(node)
            for e, c in pairs(CROSSINGS) do
                for _, n in ipairs(c.nodes) do if n == node then return e end end
            end
            return -1
        end }}
        -- The game's build: the street's halves meet at node 50, a crossing
        -- of the type its node is mapped to.
        local send = api.cmd.sendCommand
        api.cmd.sendCommand = function(command, callback)
            local p = command.proposal and command.proposal.streetProposal
            if p and p.node2rcType then
                MAP = p.node2rcType
                NODES[50] = {{ x = 50, y = 0, z = 0 }}
                EDGES[102] = {{ node0 = 50, node1 = 8, objects = {{}} }}
                EDGES[103] = {{ node0 = 9, node1 = 50, objects = {{}} }}
                STREETS[8], STREETS[9], STREETS[50] = {{ 102 }}, {{ 103 }}, {{ 102, 103 }}
                local laid = -1
                for _, v in pairs(MAP) do laid = -2 - v end
                CROSSINGS[800] = {{ nodes = {{ 50 }}, typeIndex = laid }}
            end
            return send(command, callback)
        end
        HOOK.room = true HOOK.clicks = 0
        SCRIPT.guiUpdate({{}}, nil, nil)
        SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'trackBuilder', 'builder.proposalCreate', {{ {TRACK_PROPOSAL} }})
        TOOL[0] = {{ kind = 5 }}
        HOOK.clicks = 1 SCRIPT.guiUpdate({{}}, nil, nil)
        HOOK.batch = {{ (assert(HOOK.commands[1], 'the click went to the room')) }}
        UPDATE({{}}, STATE, 0.2)
        "#
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    let (handed, map, crossing): (String, String, String) = lua
        .load(
            "local b = HOOK.commands[1].BuildTrack \
             local keys = {} for k, v in pairs(MAP) do keys[#keys + 1] = k .. '=' .. v end \
             table.sort(keys) \
             return tostring(b.crossing_type), table.concat(keys, ' '), \
                 tostring(CROSSINGS[800].typeIndex) .. ' ' .. tostring(HOOK.applied[1].ok)",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert_eq!(handed, "railroad_crossing/era_c_eu.rcr");
    assert_eq!(
        map, "-5=-7 -6=-7 -7=-7 8=-7 9=-7",
        "every vertex's node, the new ones by their ids and the street's ends by theirs"
    );
    assert_eq!(crossing, "5 true", "{}", hook_log(&lua));
    let logged = hook_log(&lua);
    assert!(
        !logged.contains("took type") && !logged.contains("no level crossing"),
        "the crossing at the new node read back of the type: {logged}"
    );
}

/// A mod's build from its game script's GUI half, as Parallel Roads sends
/// one (a SimpleProposal): the country street 8-9 carried on from node 9 to
/// open ground.
const SCRIPT_BUILD: &str = "{ constructionsToAdd = {}, constructionsToRemove = {}, streetProposal = { \
    nodesToAdd = { { entity = -1, comp = { position = { x = 50, y = 100, z = 0 } } } }, \
    edgesToAdd = { { entity = -2, type = 0, comp = { node0 = 9, node1 = -1, type = 0, typeIndex = -1, \
        tangent0 = { x = 0, y = 60, z = 0 }, tangent1 = { x = 0, y = 60, z = 0 }, \
        roadTemplate = '::/street/country.street_template', roadStyle = '' } } }, \
    edgesToRemove = {}, nodesToRemove = {}, edgeObjectsToAdd = {}, edgeObjectsToRemove = {} } }";

/// A script's build in the game scripts' GUI state (tpf3mp/modbuild.lua,
/// proposed D27) goes to the room only as the follow-up of this player's
/// own build, and is always marked playerInitiated, so the hook stops it
/// here; every other one is stopped with why, and outside the room's game it
/// is left alone.
#[test]
fn a_scripts_build_goes_to_the_room_only_as_its_players_follow_up() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(format!(
        "HOOK.room = true HOOK.clicks = 0 HOOK.status = {{ me_id = 'me' }} \
         SCRIPT.guiUpdate({{}}, nil, nil) \
         MADE = {{}} \
         function BUILD(proposal, initiated) \
             MADE[#MADE + 1] = api.cmd.makeWorldBuildProposalCmd(proposal, {{}}, false, initiated) \
             HOOK.clicks = HOOK.clicks + 1 \
             SCRIPT.guiUpdate({{}}, nil, nil) \
         end \
         BUILD({SCRIPT_BUILD}, false) \
         HOOK.batch = {{ {ROAD} }} HOOK.origins = {{ 'other' }} UPDATE({{}}, STATE, 0.2) \
         SCRIPT.guiUpdate({{}}, nil, nil) \
         BUILD({SCRIPT_BUILD}, true) \
         HOOK.batch = {{ {ROAD} }} HOOK.origins = {{ 'me' }} UPDATE({{}}, STATE, 0.2) \
         SCRIPT.guiUpdate({{}}, nil, nil) \
         BUILD({SCRIPT_BUILD}, true) \
         local signal = {SCRIPT_BUILD} signal.streetProposal.edgeObjectsToAdd = {{ {{}} }} \
         BUILD(signal, true) \
         for i = 1, 130 do SCRIPT.guiUpdate({{}}, nil, nil) end \
         BUILD({SCRIPT_BUILD}, true) \
         HOOK.clicks = nil \
         UNCOUNTED = select(2, pcall(api.cmd.makeWorldBuildProposalCmd, {SCRIPT_BUILD}, {{}}, false, false)) \
         HOOK.room = false \
         OUTSIDE = api.cmd.makeWorldBuildProposalCmd({SCRIPT_BUILD}, {{}}, false, false)"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let (handed, from, to, note): (usize, String, String, String) = lua
        .load(
            "local p = HOOK.commands[1].BuildRoad.polyline \
             return #HOOK.commands, p.vertices[1].pos.y .. ' ' .. tostring(p.vertices[1].resolve.Node), \
                 p.vertices[2].pos.y .. ' ' .. tostring(p.vertices[2].resolve), \
                 HOOK.notes['tpf3mp.lastbuild']",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(handed, 1, "only the follow-up of this player's build");
    assert_eq!((from.as_str(), to.as_str()), ("40 Street", "100 New"));
    assert_eq!(note, "2 mine", "two builds applied, the last this player's");
    let initiated: Vec<bool> = lua
        .load(
            "local out = {} for i, c in ipairs(MADE) do out[i] = c.playerInitiated end return out",
        )
        .eval()
        .unwrap();
    assert_eq!(
        initiated, [true; 5],
        "every build is the hook's to stop, whatever the script asked"
    );
    let (uncounted, outside): (String, bool) = lua
        .load("return tostring(UNCOUNTED), OUTSIDE.playerInitiated")
        .eval()
        .unwrap();
    assert_eq!(uncounted, "Not in multiplayer yet: building from a script");
    assert!(!outside, "outside the room's game, as the script asked");
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    let stopped: Vec<&str> = logged
        .iter()
        .filter_map(|l| l.strip_prefix("stopped a build the room cannot carry: "))
        .collect();
    assert_eq!(
        stopped,
        [
            "a script's build with no build of this player's just before it",
            "a script's follow-up of another player's build: that player's game hands it to the room",
            "a script's build with a stop or signal",
            "a script's build with no build of this player's just before it",
        ],
        "{logged:?}"
    );
    assert!(
        logged
            .iter()
            .any(|l| l == "handed the player's build to the room [a script's follow-up build]"),
        "{logged:?}"
    );
    assert!(
        logged.iter().any(|l| l
            == "scripts' builds from the game scripts' GUI state go to the room as their player's follow-ups"),
        "{logged:?}"
    );
}

/// The street tool's build as the room orders it (metres): from node 7 onto
/// the country street 8-11-9, whose node 11 the new junction replaces, the
/// street rebuilt through it in its own kind.
const JUNCTION: &str = "{ BuildRoad = { street = '::/street/town_small.street_template', \
    bus_lane = false, tram = 'None', polyline = { \
    vertices = { \
        { pos = { x = 0, y = 0, z = 0 }, resolve = { Node = 'Street' } }, \
        { pos = { x = 50, y = 2, z = 0 }, resolve = 'New' }, \
        { pos = { x = 50, y = -40, z = 0 }, resolve = { Node = 'Street' } }, \
        { pos = { x = 50, y = 40, z = 0 }, resolve = { Node = 'Street' } } }, \
    links = { \
        { from = 0, to = 1, tangent0 = { x = 50, y = 2, z = 0 }, tangent1 = { x = 50, y = 2, z = 0 }, \
          structure = 'Ground' }, \
        { from = 2, to = 1, tangent0 = { x = 0, y = 42, z = 0 }, tangent1 = { x = 0, y = 42, z = 0 }, \
          structure = 'Ground', kind = { network = 'Street', template = '::/street/country.street_template' } }, \
        { from = 1, to = 3, tangent0 = { x = 0, y = 38, z = 0 }, tangent1 = { x = 0, y = 38, z = 0 }, \
          structure = 'Ground', kind = { network = 'Street', template = '::/street/country.street_template' } } }, \
    removals = { \
        { network = 'Street', ends = { a = { x = 50, y = -40, z = 0 }, b = { x = 50, y = 0, z = 0 } } }, \
        { network = 'Street', ends = { a = { x = 50, y = 0, z = 0 }, b = { x = 50, y = 40, z = 0 } } } }, \
    removed_nodes = { { network = 'Street', at = { x = 50, y = 0, z = 0 } } } } } }";

#[test]
fn the_game_script_rebuilds_a_street_through_a_new_junction() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    // The country street through node 11: edges 100 (8-11) and 102 (11-9).
    lua.load(
        "NODES[11] = { x = 50, y = 0, z = 0 } \
         EDGES[100].node1 = 11 \
         EDGES[102] = { node0 = 11, node1 = 9, tangent0 = { x = 0, y = 40, z = 0 }, \
                        tangent1 = { x = 0, y = 40, z = 0 }, objects = {} } \
         STREETS[8], STREETS[11], STREETS[9] = { 100 }, { 100, 102 }, { 102 }",
    )
    .exec()
    .unwrap();
    lua.load(format!(
        "HOOK.batch = {{ {JUNCTION} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    let built: String = lua
        .load(
            "local p = SENT[1].proposal.streetProposal
             local out = { #p.nodesToAdd, table.concat(p.edgesToRemove, ','), table.concat(p.nodesToRemove, ','),
                           table.concat(p.nodeConfigsToRemove, ',') }
             for _, e in ipairs(p.edgesToAdd) do
                 out[#out + 1] = e.entity .. ':' .. e.comp.node0 .. '>' .. e.comp.node1 .. ' '
                     .. e.comp.roadTemplate .. ' ' .. e.comp.laneConfigs[1] .. ' ' .. e.comp.roadStyle
             end
             return table.concat(out, ' | ')",
        )
        .eval()
        .unwrap_or_else(|error| {
            panic!(
                "{error}\n{:?}",
                lua.load("return HOOK.logged").eval::<Vec<String>>()
            )
        });
    assert_eq!(
        built,
        "1 | 100,102 | 11 | 8,9 \
         | -1:7>-4 ::/street/town_small.street_template town lanes ::/style/town.street_style \
         | -2:8>-4 ::/street/country.street_template country lanes ::/style/country.street_style \
         | -3:-4>9 ::/street/country.street_template country lanes ::/style/country.street_style",
        "the old junction's node and edges removed, the street rebuilt in its own kind"
    );
    // A stop on an edge it removes: built nowhere.
    lua.load(format!(
        "SENT = {{}} EDGES[102].objects = {{ {{ 555, 1 }} }} HOOK.batch = {{ {JUNCTION} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    assert_eq!(lua.load("return #SENT").eval::<usize>().unwrap(), 0);
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged
            .last()
            .unwrap()
            .ends_with("removal 2 has a stop or signal on it and no link rebuilds it"),
        "{logged:?}"
    );
}

/// The engine's octree as the fake network holds it: each edge where its
/// end nodes are, found by any box that reaches its span (twice, as the
/// game may name one more than once). `OCTREE` 'none' leaves the game
/// without one, 'empty' has one that holds nothing yet (a network built
/// earlier in the same update). WHOLE counts the reads of a whole network.
const FAKE_OCTREE: &str = r#"
api.type.Box3 = { new = function(a, b) return { min = a, max = b } end }
QUERIES, WHOLE = 0, 0
if OCTREE ~= 'none' then
    api.engine.system.octreeSystem = { findIntersectingEntities = function(box, visit)
        QUERIES = QUERIES + 1
        if OCTREE == 'empty' then return end
        for e, c in pairs(EDGES) do
            local a, b = NODES[c.node0], NODES[c.node1]
            if a and b and math.max(a.x, b.x) >= box.min.x and math.min(a.x, b.x) <= box.max.x
                and math.max(a.y, b.y) >= box.min.y and math.min(a.y, b.y) <= box.max.y then
                visit(e)
                visit(e)
            end
        end
    end }
end
local street = api.engine.system.streetSystem
local streets, tracks = street.getNode2StreetEdgeMap, street.getNode2TrackEdgeMap
street.getNode2StreetEdgeMap = function() WHOLE = WHOLE + 1 return streets() end
street.getNode2TrackEdgeMap = function() WHOLE = WHOLE + 1 return tracks() end
"#;

/// A street rebuilt through a new junction keeps the settings of the
/// junctions at the ends of the edges it removes, and reads only the edges
/// at the nodes those settings name, never every edge of the world
/// (2026-10-16: ~600 ms of every such build in a late game, a street
/// upgrade included). What it builds is what reading every edge built; an
/// octree changes nothing in a build the room applies.
#[test]
fn a_rebuilt_street_reads_only_the_edges_its_junctions_name() {
    let mut built = Vec::new();
    for octree in ["none", "spatial"] {
        let (lua, _script) = engine();
        lua.load(FAKE_NETWORK).exec().unwrap();
        // The country street through node 11: edges 100 (8-11) and 102
        // (11-9); a town street 103 on from 9 to 12. The junction at 9 turns
        // from the removed 102 onto 103, which stays. A street 200 far away.
        lua.load(
            "NODES[11] = { x = 50, y = 0, z = 0 } NODES[12] = { x = 50, y = 90, z = 0 } \
             NODES[20] = { x = 900, y = 900, z = 0 } NODES[21] = { x = 990, y = 900, z = 0 } \
             EDGES[100].node1 = 11 \
             EDGES[102] = { node0 = 11, node1 = 9, tangent0 = { x = 0, y = 40, z = 0 }, \
                            tangent1 = { x = 0, y = 40, z = 0 }, objects = {}, laneConfigs = { 'country lanes' } } \
             EDGES[103] = { node0 = 9, node1 = 12, tangent0 = { x = 0, y = 50, z = 0 }, \
                            tangent1 = { x = 0, y = 50, z = 0 }, objects = {}, laneConfigs = { 'town lanes' } } \
             EDGES[200] = { node0 = 20, node1 = 21, tangent0 = { x = 90, y = 0, z = 0 }, \
                            tangent1 = { x = 90, y = 0, z = 0 }, objects = {}, laneConfigs = { 'town lanes' } } \
             STREETS[8], STREETS[11], STREETS[9], STREETS[12] = { 100 }, { 100, 102 }, { 102, 103 }, { 103 } \
             STREETS[20], STREETS[21] = { 200 }, { 200 } \
             CONFIGS[9] = { laneConnections = { { segment0 = 102, lane0 = 0, segment1 = 103, lane1 = 0, \
                 withRoad = true, withTram = false } }, crosswalks = { 103 }, trafficLightPreference = 1, \
                 trafficLightConfig = { trafficLightType = -1, states = {} }, doubleSlipSwitch = false, \
                 userModifiedTrafficLightStates = false } \
             FAR_READS = 0 \
             local get = api.engine.getComponent \
             api.engine.getComponent = function(id, kind) \
                 if id == 200 and kind == api.type.ComponentType.BASE_EDGE then FAR_READS = FAR_READS + 1 end \
                 return get(id, kind) \
             end",
        )
        .exec()
        .unwrap();
        lua.globals().set("OCTREE", octree).unwrap();
        lua.load(FAKE_OCTREE).exec().unwrap();
        lua.load(format!(
            "HOOK.batch = {{ {JUNCTION} }} UPDATE({{}}, STATE, 0.2)"
        ))
        .exec()
        .unwrap();
        let (shape, far): (String, u32) = lua
            .load(
                "local p = SENT[1].proposal.streetProposal
                 local out = { #p.nodesToAdd, table.concat(p.edgesToRemove, ','), table.concat(p.nodesToRemove, ','),
                               table.concat(p.nodeConfigsToRemove, ',') }
                 for _, e in ipairs(p.edgesToAdd) do
                     out[#out + 1] = e.entity .. ':' .. e.comp.node0 .. '>' .. e.comp.node1
                 end
                 for _, n in ipairs(p.nodeConfigsToAdd or {}) do
                     local turns = {}
                     for _, t in ipairs(n.comp.laneConnections) do turns[#turns + 1] = t.segment0 .. '>' .. t.segment1 end
                     out[#out + 1] = 'config ' .. n.entity .. ' ' .. table.concat(turns, ',')
                         .. ' walk ' .. table.concat(n.comp.crosswalks, ',')
                 end
                 return table.concat(out, ' | '), FAR_READS",
            )
            .eval()
            .unwrap_or_else(|error| panic!("{octree}: {error}\n{}", hook_log(&lua)));
        assert_eq!(far, 0, "{octree}: the far street's edge was read");
        built.push(shape);
    }
    assert_eq!(
        built[0],
        "1 | 100,102 | 11 | 8,9 | -1:7>-4 | -2:8>-4 | -3:-4>9 | config 8  walk  | config 9 -3>103 walk 103",
        "the junction at 9 turns from the new edge onto the street that stays"
    );
    assert_eq!(built[1], built[0], "an octree changes nothing in a build");
}

/// Two edges a junction could mean by the same ends are still refused,
/// though only the edges near them are read.
#[test]
fn a_junction_edge_named_twice_is_still_refused() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(
        "NODES[11] = { x = 50, y = 0, z = 0 } NODES[12] = { x = 50, y = 90, z = 0 } \
         EDGES[100].node1 = 11 \
         EDGES[102] = { node0 = 11, node1 = 9, tangent0 = { x = 0, y = 40, z = 0 }, \
                        tangent1 = { x = 0, y = 40, z = 0 }, objects = {}, laneConfigs = { 'country lanes' } } \
         EDGES[103] = { node0 = 9, node1 = 12, tangent0 = { x = 0, y = 50, z = 0 }, \
                        tangent1 = { x = 0, y = 50, z = 0 }, objects = {}, laneConfigs = { 'town lanes' } } \
         EDGES[104] = { node0 = 12, node1 = 9, tangent0 = { x = 0, y = -50, z = 0 }, \
                        tangent1 = { x = 0, y = -50, z = 0 }, objects = {}, laneConfigs = { 'town lanes' } } \
         STREETS[8], STREETS[11], STREETS[9], STREETS[12] = { 100 }, { 100, 102 }, { 102, 103, 104 }, { 103, 104 } \
         CONFIGS[9] = { laneConnections = { { segment0 = 102, lane0 = 0, segment1 = 103, lane1 = 0, \
             withRoad = true, withTram = false } }, crosswalks = {}, trafficLightPreference = 1, \
             trafficLightConfig = { trafficLightType = -1, states = {} }, doubleSlipSwitch = false, \
             userModifiedTrafficLightStates = false }",
    )
    .exec()
    .unwrap();
    lua.load(format!(
        "HOOK.batch = {{ {JUNCTION} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    assert_eq!(lua.load("return #SENT").eval::<usize>().unwrap(), 0);
    let log = hook_log(&lua);
    assert!(log.contains("ambiguous junction edge"), "{log}");
}

/// A preview of another player's road finds its nodes near them through
/// the engine's octree, not by reading the whole network, and makes the
/// same proposal (2026-10-16: 130-150 ms a preview in the GUI of a late
/// game, several a second). Where the octree holds nothing near (a road
/// built a moment ago) it reads the whole network, as before.
#[test]
fn a_road_preview_finds_its_nodes_near_them() {
    let mut made = Vec::new();
    for octree in ["none", "spatial", "empty"] {
        let (lua, _script) = engine();
        lua.load(FAKE_NETWORK).exec().unwrap();
        lua.globals().set("OCTREE", octree).unwrap();
        lua.load(FAKE_OCTREE).exec().unwrap();
        let (shape, reads): (String, u32) = lua
            .load(format!(
                "local apply = ug_require('tpf3mp_1::/scripts/tpf3mp/apply.lua')
                 local proposal = assert(apply.proposalOf({ROAD}, {{}}))
                 local p = proposal.streetProposal
                 local out = {{ table.concat(p.edgesToRemove, ',') }}
                 for _, e in ipairs(p.edgesToAdd) do
                     out[#out + 1] = e.entity .. ':' .. e.comp.node0 .. '>' .. e.comp.node1
                 end
                 return table.concat(out, ' | '), WHOLE"
            ))
            .eval()
            .unwrap_or_else(|error| panic!("{octree}: {error}"));
        made.push(shape);
        assert_eq!(
            reads == 0,
            octree == "spatial",
            "{octree}: {reads} whole reads"
        );
    }
    assert_eq!(made[1], made[0]);
    assert_eq!(made[2], made[0]);
}

/// An applied road reads its nodes from the whole network, as it always
/// did, whatever the octree holds: one the octree missed could make one
/// game choose another node than another game.
#[test]
fn an_applied_road_reads_the_whole_network() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.globals().set("OCTREE", "spatial").unwrap();
    lua.load(FAKE_OCTREE).exec().unwrap();
    lua.load(format!(
        "HOOK.batch = {{ {ROAD} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    let (sent, queries, whole): (usize, u32, u32) =
        lua.load("return #SENT, QUERIES, WHOLE").eval().unwrap();
    assert_eq!(sent, 1, "{}", hook_log(&lua));
    assert_eq!(queries, 0, "the octree is not asked");
    assert!(whole > 0);
}

/// An applied action's cost by part is said in the log once it is slow:
/// for finding what makes another player's builds stutter here (2026-10-16).
#[test]
fn a_slow_action_says_what_it_cost() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(format!(
        "local apply = ug_require('tpf3mp_1::/scripts/tpf3mp/apply.lua') \
         apply.SLOW_MS = 0 \
         HOOK.batch = {{ {ROAD} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    let log = hook_log(&lua);
    let line = log
        .lines()
        .find(|l| l.contains("applying BuildRoad took"))
        .unwrap_or_else(|| panic!("{log}"));
    for part in ["nodes Street 1x", "junctions 1x", "commands 1x"] {
        assert!(line.contains(part), "{part}: {line}");
    }
}

#[test]
fn a_street_build_the_room_cannot_carry_says_why() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    let asked: Vec<String> = lua
        .load(format!(
            "HOOK.room = true HOOK.clicks = 0 \
             local out = {{}} \
             local function ask(proposal) \
                 local r = SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'streetBuilder', 'builder.proposalCreate', {{ proposal }}) \
                 if r == nil then return 'nil' end \
                 for text in pairs(r.errorMessages) do return text end \
             end \
             local stop = {STREET_PROPOSAL} stop.proposal.edgeObjectsToAdd = {{ {{}} }} \
             out[#out + 1] = ask(stop) \
             local nowhere = {STREET_PROPOSAL} nowhere.proposal.addedSegments[1].comp.node0 = 12345 \
             out[#out + 1] = ask(nowhere) \
             return out"
        ))
        .eval()
        .unwrap();
    assert_eq!(
        asked,
        [
            "Not in multiplayer yet: a build with a stop or signal",
            "Not in multiplayer yet: node 12345 has no position"
        ]
    );
}

/// Vehicles, lines and station groups for the registry's tests, over the
/// stand-in engine state: VEHICLES, LINES and GROUPS list what exists; a
/// bought vehicle appears as NEXT_VEHICLE, a new line as NEXT_LINE. As the
/// game's line system, LINES lists a new line only from the next update:
/// until then it is in LATE_LINES, and exists all the same.
const FAKE_FLEET: &str = r#"
VEHICLES, LINES, GROUPS = { 401, 402 }, { 301 }, { 91, 90 }
LATE_LINES = {}
NEXT_VEHICLE, NEXT_LINE = 500, 600
BEFORE_UPDATE = function()
    for _, e in ipairs(LATE_LINES) do LINES[#LINES + 1] = e end
    LATE_LINES = {}
end
local function has(list, e)
    for _, x in ipairs(list) do if x == e then return true end end
    return false
end
local CT = { CONSTRUCTION = 2, LINE = 3, TRANSPORT_VEHICLE = 4, STATION_GROUP = 9, GAME_TIME = 10 }
api.type.ComponentType = CT
api.engine.getEntitiesWithComponent = function(kind)
    if kind == CT.TRANSPORT_VEHICLE then return VEHICLES end
    if kind == CT.STATION_GROUP then return GROUPS end
    if kind == CT.CONSTRUCTION then return { 201 } end
    return {}
end
api.engine.system = { lineSystem = { getLines = function() return LINES end } }
api.engine.util.getWorld = function() return 1 end
api.engine.getComponent = function(e, kind)
    if kind == CT.GAME_TIME then return { gameTime = 777000 } end
    if kind == CT.CONSTRUCTION and e == 201 then
        return { fileName = 'depot/bus_depot.con', depots = { 202 },
                 transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 600,10,2,1 } }
    end
    if kind == CT.LINE and (has(LINES, e) or has(LATE_LINES, e)) then return { stops = {} } end
    if kind == CT.TRANSPORT_VEHICLE and has(VEHICLES, e) then return {} end
    if kind == CT.STATION_GROUP and has(GROUPS, e) then return {} end
end
api.res = { modelRep = {
    find = function(name) if name == 'vehicle/bus/city.mdl' then return 41 end return -1 end,
    getName = function(id) if id == 41 then return 'vehicle/bus/city.mdl' end end,
} }
api.type.Vec3f = { new = function(x, y, z) return { x = x, y = y, z = z } end }
api.type.TransportVehiclePart = { new = function() return { part = {} } end }
api.type.TransportVehicleConfig = { new = function() return {} end }
api.type.LoadConfig = { new = function() return {} end }
api.cmd.makeVehicleBuyCmd = function(player, depot, config)
    return { buy = { player = player, depot = depot, config = config } }
end
api.cmd.makeVehicleSetLineCmd = function(vehicle, line, stop)
    return { setLine = { vehicle = vehicle, line = line, stop = stop } }
end
local send = api.cmd.sendCommand
api.cmd.sendCommand = function(command, ...)
    -- As the game: a bought vehicle exists, and is listed, at once; a new
    -- line exists at once, and is listed from the next update. The
    -- command's data says which it made.
    if command.buy then
        VEHICLES[#VEHICLES + 1] = NEXT_VEHICLE
        command.resultVehicleEntity, command.made = NEXT_VEHICLE, NEXT_VEHICLE
    end
    if command.createLine then
        LATE_LINES[#LATE_LINES + 1] = NEXT_LINE
        command.resultEntity, command.made = NEXT_LINE, NEXT_LINE
    end
    send(command, ...)
end
"#;

/// A bus bought at the depot of FAKE_FLEET, as the hook hands it (metres).
const BUY_BUS: &str = "{ BuyVehicle = { \
    depot = { file = 'depot/bus_depot.con', at = { x = 600.4, y = 10, z = 2 } }, \
    consist = { { model = 'vehicle/bus/city.mdl', reversed = false, \
                  loads = { { config = 0, cargo = 3 } }, color = { r = 0.5, g = 0.25, b = 0 } } }, \
    groups = { 1 }, multiple_units = { '' } } }";

#[test]
fn the_registry_names_vehicles_in_the_order_they_came_and_never_again() {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    let named: String = lua
        .load(
            "local registry = ug_require('tpf3mp_1::/scripts/tpf3mp/registry.lua')
             local reg, fresh = registry.sync(nil)
             local out = { #fresh, registry.id(reg, 'vehicles', 401), registry.id(reg, 'vehicles', 402),
                           registry.id(reg, 'groups', 90), registry.id(reg, 'groups', 91),
                           registry.id(reg, 'lines', 301) }
             -- 401 sold, 403 bought: 401's id is retired, 403 gets the next.
             VEHICLES = { 402, 403 }
             reg, fresh = registry.sync(reg)
             out[#out + 1] = tostring(registry.id(reg, 'vehicles', 401))
             out[#out + 1] = registry.id(reg, 'vehicles', 403)
             out[#out + 1] = registry.entity(reg, 'vehicles', 1)
             out[#out + 1] = #fresh .. ':' .. fresh[1][1] .. ':' .. fresh[1][2] .. ':' .. fresh[1][3]
             -- A line the line system leaves out, but which exists, keeps
             -- its id; one gone is retired.
             LINES, LATE_LINES = {}, { 301 }
             reg = registry.sync(reg)
             out[#out + 1] = registry.id(reg, 'lines', 301)
             LATE_LINES = {}
             reg = registry.sync(reg)
             out[#out + 1] = tostring(registry.id(reg, 'lines', 301))
             -- What an action made is bound at once, listed or not.
             LATE_LINES = { 600 }
             reg, fresh = registry.sync(reg, { lines = { 600 } })
             out[#out + 1] = #fresh .. ':' .. registry.id(reg, 'lines', 600)
             LINES, LATE_LINES = { 600 }, {}
             -- A kind it cannot list keeps its names.
             api.engine.system.lineSystem = nil
             local _, _, failed = registry.sync(reg)
             out[#out + 1] = #failed .. ':' .. registry.id(reg, 'lines', 600)
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        named, "5 0 1 0 1 0 nil 2 402 1:vehicles:2:403 0 nil 1:1 1:1",
        "lowest entity first, per kind; a retired id never comes back"
    );
}

#[test]
fn an_unreadable_component_does_not_retire_a_rooms_canonical_id() {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    let named: String = lua
        .load(
            "local registry = ug_require('tpf3mp_1::/scripts/tpf3mp/registry.lua')
             local reg = registry.sync(nil)
             local old = api.engine.getComponent
             LINES = {} -- the line system temporarily omits an existing line
             api.engine.getComponent = function(e, kind)
                 if e == 301 and kind == api.type.ComponentType.LINE then error('component unreadable') end
                 return old(e, kind)
             end
             reg = registry.sync(reg)
             local during = tostring(registry.id(reg, 'lines', 301))
             api.engine.getComponent = old
             LINES = { 301 }
             reg = registry.sync(reg)
             local recovered = tostring(registry.id(reg, 'lines', 301))
             LINES = {}
             reg = registry.sync(reg)
             return table.concat({ during, recovered, tostring(registry.id(reg, 'lines', 301)) }, ',')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        named, "0,0,nil",
        "an unreadable entity keeps its id until absence is established"
    );
}

#[test]
fn the_game_script_buys_the_vehicle_and_tells_the_buyer_which() {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    // The room's first update begins the registry; then the bus, and a
    // vehicle put on line 0.
    lua.load(format!(
        "HOOK.room = true UPDATE({{}}, STATE, 0.2) \
         HOOK.batch = {{ {BUY_BUS}, {{ AssignLine = {{ vehicles = {{ 2 }}, line = 0, first_stop = 1 }} }} }} \
         UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    let bought: String = lua
        .load(
            "local b = SENT[1].buy local p = b.config.vehicles[1] \
             local s = SENT[2].setLine \
             return table.concat({ b.player, b.depot, p.part.modelId, tostring(p.part.reversed), \
                 p.part.compartment2loadConfig[1].cargoTypeId, p.part.color.y, p.purchaseTime, \
                 tostring(p.autoLoadConfig[1]), b.config.vehicleGroups[1], \
                 s.vehicle, s.line, s.stop }, '|')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        bought, "25|202|41|false|3|0.25|777000|true|1|500|301|1",
        "bought at the depot's construction there, as the store configured it; \
        then vehicle-2, the new one, on line-0"
    );
    let assignment_log: String = lua
        .load("return table.concat(HOOK.logged, '\\n')")
        .eval()
        .unwrap();
    assert!(
        assignment_log.contains(
            "assign-vehicle action_index=2 vehicle=2 native=500 line=0 native_line=301 first=1 carrier=none models=none mode=none owner=unavailable line_owner=unavailable outcome=applied"
        ),
        "missing assignment boundary: {assignment_log}"
    );
    assert!(
        assignment_log.contains(
            "buy-vehicle action_index=1 outcome=applied models=vehicle/bus/city.mdl native=500 canonical=2"
        ),
        "missing purchase boundary: {assignment_log}"
    );
    let applied: String = lua
        .load(
            "local out = {} for _, a in ipairs(HOOK.applied) do \
                 out[#out + 1] = a.i .. ':' .. tostring(a.ok) .. ':' .. tostring(a.entity) end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(applied, "1:true:500 2:true:nil", "the buyer hears which");
    // The registry the GUI reads is in the script's state, saved with the
    // world.
    let saved: u32 = lua
        .load(
            "return ug_require('tpf3mp_1::/scripts/tpf3mp/registry.lua').id(STATE.value.registry, 'vehicles', 500)",
        )
        .eval()
        .unwrap();
    assert_eq!(saved, 2);
}

/// The sender's GUI filters another company's depot, but every game's replay
/// must enforce the same rule for old or forged actions. An absent owner is
/// refused too; with one company the existing native behavior remains.
#[test]
fn every_game_buys_only_at_its_companys_owned_depot() {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    lua.load(format!(
        "api.cmd.makeEntitySetColorCmd = function(e, color) return {{ paint = e }} end \
         local CT = api.type.ComponentType CT.PLAYER_OWNED = 13 \
         local base = api.engine.getComponent \
         api.engine.getComponent = function(e, kind) \
             if kind == CT.PLAYER_OWNED then \
                 if e == 202 then return {{ player = 901 }} end \
                 if e == 203 then return {{ player = 25 }} end \
                 return nil \
             end \
             if kind == CT.CONSTRUCTION and e == 201 then \
                 local c = base(e, kind) c.depots = {{ 202, 203, 204 }} return c \
             end \
             return base(e, kind) \
         end \
         ROSTER = {{ next = 2, list = {{ \
             {{ id = 0, entity = 25, name = 'First', color = {{ 0.80, 0.16, 0.12 }} }}, \
             {{ id = 1, entity = 901, name = 'Rival', color = {{ 0.13, 0.42, 0.85 }} }} }}, \
             members = {{ {{ player = 'rival-player', company = 1 }} }} }} \
         STATE.value = {{ companies = ROSTER }} \
         HOOK.room = true UPDATE({{}}, STATE, 0.2) \
         local function buy(index) local action = {BUY_BUS} \
             action.BuyVehicle.depot_index = index return action end \
         HOOK.origins = {{ 'rival-player', 'rival-player', 'rival-player' }} \
         HOOK.batch = {{ buy(0), buy(1), buy(2) }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let result: (usize, i64, i64, bool, bool, String, bool, String) = lua
        .load(
            "local buys = {} for _, c in ipairs(SENT) do if c.buy then buys[#buys + 1] = c.buy end end \
             return #buys, buys[1].player, buys[1].depot, \
                 HOOK.applied[1].ok, HOOK.applied[2].ok, HOOK.applied[2].why, \
                 HOOK.applied[3].ok, HOOK.applied[3].why",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        result,
        (
            1,
            901,
            202,
            true,
            false,
            "the depot belongs to First".to_owned(),
            false,
            "the depot has no company owner".to_owned(),
        ),
        "only the acting company's owned depot is purchased; foreign and ownerless depots fail closed"
    );
}

#[test]
fn a_bought_vehicle_goes_to_the_room_and_the_store_hears_which_it_is() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    // The GUI reads the game script's registry from its state, and the
    // depot's construction, as the game has them.
    lua.load(
        "api.cmd.makeVehicleSetLineCmd = function(vehicle, line, stop) return { kind = 'setLine' } end \
         api.type = { ComponentType = { GAME_SCRIPT = 7, CONSTRUCTION = 2 } } \
         api.engine = { \
             getComponent = function(e, kind) \
                 if kind == 7 and e == 77 then return { state = { registry = { \
                     vehicles = { next = 4, bound = { { 3, 500 } } }, \
                     lines = { next = 2, bound = { { 1, 600 } } }, groups = { next = 0, bound = {} } } } } end \
                 if kind == 2 and e == 201 then return { fileName = 'depot/bus_depot.con', depots = { 202 }, \
                     transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 600,10,2,1 } } end \
             end, \
             system = { \
                 gameScriptSystem = { getEntityForGameScript = function(name) \
                     if name == 'tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs' then return 77 end return -1 end }, \
                 streetConnectorSystem = { getConstructionEntityForDepot = function(d) \
                     if d == 202 then return 201 end end }, \
             }, \
         } \
         api.res = { modelRep = { getName = function(id) if id == 41 then return 'vehicle/bus/city.mdl' end end } } \
         M = mount(loadPlugin()) M.step() HOOK.room = true",
    )
    .exec()
    .unwrap();
    // The store buys a bus at depot 202 and, told which it is, puts it on
    // line 600, as vehicle_react_util.tl does.
    lua.load(
        "CONFIG = { vehicles = { { part = { modelId = 41, reversed = true, \
             compartment2loadConfig = { { loadConfigIndex = 0, cargoTypeId = 3 } }, \
             color = { x = 1, y = 0, z = 0 } } } }, vehicleGroups = { 1 }, muFileNames = { '' } } \
         HEARD = nil \
         api.cmd.sendCommand(api.cmd.makeVehicleBuyCmd(25, 202, CONFIG), function(data, ok, entities) \
             HEARD = { vehicle = data.resultVehicleEntity, ok = ok, entity = entities[1] and entities[1][1] } \
             api.cmd.sendCommand(api.cmd.makeVehicleSetLineCmd(data.resultVehicleEntity, 600, 0)) \
         end) \
         M.step()",
    )
    .exec()
    .unwrap();
    let (handed, heard): (usize, bool) = lua
        .load("return #HOOK.commands, HEARD ~= nil")
        .eval()
        .unwrap();
    assert_eq!(handed, 1);
    assert!(!heard, "not before the room's action ran here");
    let buy: String = lua
        .load(
            "local b = HOOK.commands[1].BuyVehicle local p = b.consist[1] \
             return table.concat({ b.depot.file, b.depot.at.x, p.model, tostring(p.reversed), \
                 p.loads[1].cargo, p.color.r, b.groups[1] }, '|')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        buy,
        "depot/bus_depot.con|600|vehicle/bus/city.mdl|true|3|1|1"
    );
    // This game applied it and bought vehicle 500: the store hears so, and
    // its line assignment goes to the room by canonical ids.
    lua.load("HOOK.results = { { ticket = 1, ok = true, entity = 500 } } M.step()")
        .exec()
        .unwrap();
    let assigned: String = lua
        .load(
            "local a = HOOK.commands[2].AssignLine \
             return table.concat({ HEARD.vehicle, tostring(HEARD.ok), HEARD.entity, \
                 a.vehicles[1], a.line, a.first_stop }, '|')",
        )
        .eval()
        .unwrap();
    assert_eq!(assigned, "500|true|500|3|1|0");
}

/// A ship depot or an aircraft hangar may have no street, so the street
/// connector does not name its construction (INFERRED for build 40408): the
/// store's ship or aircraft is then bought at the construction that lists
/// the depot among its own, by its file and place; a depot no construction
/// lists is refused.
#[test]
fn a_ship_or_aircraft_is_bought_at_the_harbour_or_airport_that_lists_its_depot() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load(
        "api.type = { ComponentType = { GAME_SCRIPT = 7, CONSTRUCTION = 2 } } \
         COMPONENTS = { \
             [201] = { fileName = 'depot/bus_depot.con', depots = { 202 }, \
                       transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 600,10,2,1 } }, \
             [301] = { fileName = 'station/water/harbour.con', depots = { 303, 302 }, \
                       transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 1200,40,0,1 } } } \
         api.engine = { \
             getComponent = function(e, kind) \
                 if kind == 7 and e == 77 then return { state = { registry = {} } } end \
                 if kind == 2 then return COMPONENTS[e] end \
             end, \
             getEntitiesWithComponent = function(kind) if kind == 2 then return { 201, 301 } end return {} end, \
             system = { \
                 gameScriptSystem = { getEntityForGameScript = function(name) \
                     if name == 'tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs' then return 77 end return -1 end }, \
                 streetConnectorSystem = { getConstructionEntityForDepot = function(d) \
                     if d == 202 then return 201 end return -1 end }, \
             }, \
         } \
         api.res = { modelRep = { getName = function(id) if id == 51 then return 'vehicle/ship/ferry.mdl' end end } } \
         M = mount(loadPlugin()) M.step() HOOK.room = true \
         CONFIG = { vehicles = { { part = { modelId = 51, reversed = false, compartment2loadConfig = {}, \
             color = { x = 0, y = 0, z = 1 } } } }, vehicleGroups = { 1 }, muFileNames = { '' } } \
         api.cmd.sendCommand(api.cmd.makeVehicleBuyCmd(25, 302, CONFIG)) \
         api.cmd.sendCommand(api.cmd.makeVehicleBuyCmd(25, 999, CONFIG)) \
         M.step()",
    )
    .exec()
    .unwrap();
    let (handed, depot): (usize, String) = lua
        .load(
            "local b = HOOK.commands[1].BuyVehicle local d = b.depot \
             return #HOOK.commands, d.file .. '|' .. d.at.x .. '|' .. d.at.y .. '|' .. b.depot_index",
        )
        .eval()
        .unwrap();
    assert_eq!(
        handed, 1,
        "the depot no construction lists is not handed over"
    );
    assert_eq!(
        depot, "station/water/harbour.con|1200|40|1",
        "the harbour's second depot"
    );
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged.iter().any(|l| l.contains(
            "refused the player's makeVehicleBuyCmd in the room's game (1 so far): \
             a depot the room cannot name"
        )),
        "{logged:?}"
    );
}

/// Every game buys at the depot of the construction the store bought at,
/// by its index there (an airport's second hangar), not at its first.
#[test]
fn every_game_buys_at_the_constructions_depot_the_store_bought_at() {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    lua.load(format!(
        "local base = api.engine.getComponent \
         api.engine.getComponent = function(e, kind) \
             local c = base(e, kind) \
             if c and e == 201 and kind == api.type.ComponentType.CONSTRUCTION then c.depots = {{ 202, 203 }} end \
             return c \
         end \
         HOOK.room = true UPDATE({{}}, STATE, 0.2) \
         local buy = {BUY_BUS} \
         buy.BuyVehicle.depot_index = 1 \
         local far = {BUY_BUS} \
         far.BuyVehicle.depot_index = 2 \
         HOOK.batch = {{ buy, far }} \
         UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    let (depot, ok, why): (i64, bool, String) = lua
        .load("return SENT[1].buy.depot, HOOK.applied[2].ok, HOOK.applied[2].why")
        .eval()
        .unwrap();
    assert_eq!(depot, 203);
    assert!(!ok);
    assert_eq!(
        why, "the depot/bus_depot.con there has 2 depot(s), and no depot 3",
        "never another of its depots"
    );
}

/// An airfield's or airport's hangar is a subconstruction of it with a
/// depot (build 40408, stations/air/airfield/af_hangar.module.lua), which
/// the store buys at: every game buys the plane at the hangar its
/// construction lists among its subconstructions, by its index there, when
/// `depots` does not list it. An airfield built without its hangar module has
/// no depot, and the purchase is refused in every game, saying so.
#[test]
fn every_game_buys_a_plane_at_the_airfields_hangar_and_refuses_one_without() {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    lua.load(format!(
        "local CT = api.type.ComponentType CT.VEHICLE_DEPOT = 12 \
         local base = api.engine.getComponent \
         local AIRFIELDS = {{ \
             [211] = {{ fileName = '::/stations/air/airfield.con', depots = {{}}, \
                        subconstructions = {{ 710, 711, 712 }}, \
                        transf = {{ 1,0,0,0, 0,1,0,0, 0,0,1,0, 900,50,3,1 }} }}, \
             [221] = {{ fileName = '::/stations/air/airfield.con', depots = {{}}, subconstructions = {{ 720 }}, \
                        transf = {{ 1,0,0,0, 0,1,0,0, 0,0,1,0, 1900,50,3,1 }} }} }} \
         api.engine.getComponent = function(e, kind) \
             if kind == CT.CONSTRUCTION and AIRFIELDS[e] then return AIRFIELDS[e] end \
             if kind == CT.VEHICLE_DEPOT then \
                 if e == 711 or e == 712 then return {{ carrier = 'AIR' }} end \
                 return nil \
             end \
             return base(e, kind) \
         end \
         local list = api.engine.getEntitiesWithComponent \
         api.engine.getEntitiesWithComponent = function(kind) \
             if kind == CT.CONSTRUCTION then return {{ 201, 211, 221 }} end \
             return list(kind) \
         end \
         HOOK.room = true UPDATE({{}}, STATE, 0.2) \
         local function plane(x, index) \
             local buy = {BUY_BUS} \
             buy.BuyVehicle.depot = {{ file = '::/stations/air/airfield.con', at = {{ x = x, y = 50, z = 3 }} }} \
             buy.BuyVehicle.depot_index = index \
             return buy \
         end \
         HOOK.batch = {{ plane(900, 0), plane(900, 1), plane(900, 2), plane(1900, 0) }} \
         UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    let bought: String = lua
        .load(
            "local out = {} \
             for _, s in ipairs(SENT) do if s.buy then out[#out + 1] = s.buy.depot end end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(bought, "711 712", "the first and the second hangar");
    let (ok, why, bare, bare_why): (bool, String, bool, String) = lua
        .load(
            "return HOOK.applied[3].ok, HOOK.applied[3].why, HOOK.applied[4].ok, HOOK.applied[4].why",
        )
        .eval()
        .unwrap();
    assert!(!ok);
    assert_eq!(
        why,
        "the ::/stations/air/airfield.con there has 2 depot(s), and no depot 3"
    );
    assert!(!bare);
    assert_eq!(
        bare_why,
        "the ::/stations/air/airfield.con there has no depot: an airfield or airport has one only \
         with a hangar module, and a harbour never has one (ships are bought at a ship depot)"
    );
}

/// The player's plane, bought at an airfield's hangar: the street connector
/// names no construction for the hangar, the subconstruction lookup names
/// the airfield, and the purchase names the airfield and the hangar's index
/// among its depots (`depots`, then the subconstructions that are depots).
/// Failing closed: a depot two constructions list, and one whose
/// construction does not list it, are refused at the click, never bought
/// at the construction's first depot.
#[test]
fn a_plane_bought_at_an_airfields_hangar_names_the_airfield_and_the_hangar() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load(
        "api.type = { ComponentType = { GAME_SCRIPT = 7, CONSTRUCTION = 2, VEHICLE_DEPOT = 12 } } \
         COMPONENTS = { \
             [201] = { fileName = 'depot/bus_depot.con', depots = { 202 }, \
                       transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 600,10,2,1 } }, \
             [211] = { fileName = '::/stations/air/airfield.con', depots = {}, \
                       subconstructions = { 710, 711, 712 }, \
                       transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 900,50,3,1 } }, \
             [231] = { fileName = 'a.con', depots = { 730 }, transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 1,1,1,1 } }, \
             [232] = { fileName = 'b.con', depots = { 730 }, transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 2,2,2,1 } } } \
         api.engine = { \
             getComponent = function(e, kind) \
                 if kind == 7 and e == 77 then return { state = { registry = {} } } end \
                 if kind == 2 then return COMPONENTS[e] end \
                 if kind == 12 and (e == 711 or e == 712 or e == 202 or e == 730 or e == 740) then return {} end \
             end, \
             getEntitiesWithComponent = function(kind) if kind == 2 then return { 201, 211, 231, 232 } end return {} end, \
             system = { \
                 gameScriptSystem = { getEntityForGameScript = function(name) \
                     if name == 'tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs' then return 77 end return -1 end }, \
                 streetConnectorSystem = { \
                     getConstructionEntityForDepot = function(d) \
                         if d == 202 or d == 740 then return 201 end return -1 end, \
                     getConstructionEntityForSubconstruction = function(e) \
                         if e >= 710 and e <= 712 then return 211 end return -1 end, \
                 }, \
             }, \
         } \
         api.res = { modelRep = { getName = function(id) if id == 51 then return 'vehicle/plane/f13.mdl' end end } } \
         M = mount(loadPlugin()) M.step() HOOK.room = true \
         CONFIG = { vehicles = { { part = { modelId = 51, reversed = false, compartment2loadConfig = {}, \
             color = { x = 0, y = 0, z = 1 } } } }, vehicleGroups = { 1 }, muFileNames = { '' } } \
         for _, depot in ipairs({ 712, 711, 730, 740 }) do \
             api.cmd.sendCommand(api.cmd.makeVehicleBuyCmd(25, depot, CONFIG)) \
         end \
         M.step()",
    )
    .exec()
    .unwrap();
    let handed: String = lua
        .load(
            "local out = {} \
             for _, a in ipairs(HOOK.commands) do \
                 local b = a.BuyVehicle \
                 out[#out + 1] = b.depot.file .. '|' .. b.depot.at.x .. '|' .. b.depot_index \
             end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        handed, "::/stations/air/airfield.con|900|1 ::/stations/air/airfield.con|900|0",
        "the second hangar, then the first; nothing else"
    );
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    for why in [
        "a depot the room cannot name: a depot 2 constructions list",
        "a depot the room cannot name: a depot depot/bus_depot.con does not list among its depots",
    ] {
        assert!(logged.iter().any(|l| l.contains(why)), "{why}: {logged:?}");
    }
}

/// Renaming in an entity window's title, and recolouring a vehicle: a
/// vehicle, a station and a town go to the room by their ids, a depot by its
/// construction; what the room cannot name is refused.
#[test]
fn a_vehicle_station_town_or_depot_renamed_and_a_vehicle_recoloured_go_to_the_room() {
    let lua = gui();
    // Mechanics fixture only: production refuses this channel pending game acceptance.
    lua.load("ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').rename = true")
        .exec()
        .unwrap();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load(
        "api.cmd.makeEntitySetNameCmd = function(e, name) return { kind = 'name' } end \
         api.cmd.makeEntitySetColorCmd = function(e, color) return { kind = 'color' } end \
         api.type = { ComponentType = { GAME_SCRIPT = 7, CONSTRUCTION = 2 } } \
         api.engine = { \
             getComponent = function(e, kind) \
                 if kind == 7 and e == 77 then return { state = { registry = { \
                     vehicles = { bound = { { 3, 500 } } }, groups = { bound = { { 4, 90 } } }, \
                     towns = { bound = { { 1, 7 } } }, lines = { bound = {} } } } } end \
                 if kind == 2 and e == 201 then return { fileName = 'depot/bus_depot.con', \
                     transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 600,10,2,1 } } end \
             end, \
             system = { gameScriptSystem = { getEntityForGameScript = function(name) \
                 if name == 'tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs' then return 77 end return -1 end } }, \
         } \
         M = mount(loadPlugin()) M.step() HOOK.room = true \
         for _, e in ipairs({ 500, 90, 7, 201, 999 }) do \
             api.cmd.sendCommand(api.cmd.makeEntitySetNameCmd(e, 'N' .. e)) \
         end \
         api.cmd.sendCommand(api.cmd.makeEntitySetColorCmd(500, { x = 1, y = 0.5, z = 0 })) \
         api.cmd.sendCommand(api.cmd.makeEntitySetColorCmd(90, { x = 1, y = 0.5, z = 0 })) \
         M.step()",
    )
    .exec()
    .unwrap();
    let handed: String = lua
        .load(
            "local out = {} \
             for _, a in ipairs(HOOK.commands) do \
                 if a.Rename then \
                     local kind, v = next(a.Rename.what) \
                     if type(v) == 'table' then v = v.file .. '@' .. v.at.x end \
                     out[#out + 1] = kind .. ':' .. tostring(v) .. ':' .. a.Rename.name \
                 else \
                     local c = a.VehicleOp.change.Recolor \
                     out[#out + 1] = 'Recolor:' .. a.VehicleOp.vehicle .. ':' .. c.r .. ',' .. c.g .. ',' .. c.b \
                 end \
             end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        handed,
        "Vehicle:3:N500 Station:4:N90 Town:1:N7 Construction:depot/bus_depot.con@600:N201 \
         Recolor:3:1,0.5,0"
    );
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged.iter().any(|l| l.contains(
            "refused the player's makeEntitySetNameCmd in the room's game (1 so far): renaming this"
        )) && logged.iter().any(|l| l.contains(
            "refused the player's makeEntitySetColorCmd in the room's game (1 so far): recolouring this"
        )),
        "{logged:?}"
    );
}

/// Every game renames what the room names, and recolours the vehicle, as
/// the window would; a town this world does not have fails alike.
#[test]
fn every_game_renames_and_recolours_what_the_room_names() {
    let (lua, _script) = engine();
    // Mechanics fixture only: production refuses this channel pending game acceptance.
    lua.load("ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').rename = true")
        .exec()
        .unwrap();
    lua.load(FAKE_FLEET).exec().unwrap();
    lua.load(
        "api.cmd.makeEntitySetColorCmd = function(e, color) return { setColor = color, entity = e } end \
         HOOK.room = true UPDATE({}, STATE, 0.2) \
         HOOK.batch = { \
             { Rename = { what = { Vehicle = 1 }, name = 'Blue Arrow' } }, \
             { Rename = { what = { Station = 0 }, name = 'Central' } }, \
             { Rename = { what = { Construction = { file = 'depot/bus_depot.con', \
                 at = { x = 600, y = 10, z = 2 } } }, name = 'North depot' } }, \
             { VehicleOp = { vehicle = 0, change = { Recolor = { r = 1, g = 0.5, b = 0 } } } }, \
             { Rename = { what = { Town = 9 }, name = 'Nowhere' } } } \
         UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let sent: String = lua
        .load(
            "local out = {} for _, c in ipairs(SENT) do \
                 if c.setName then out[#out + 1] = c.entity .. '=' .. c.setName end \
                 if c.setColor then out[#out + 1] = c.entity .. ':' .. c.setColor.y end end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(sent, "402=Blue Arrow 90=Central 201=North depot 401:0.5");
    let applied: String = lua
        .load(
            "local out = {} for _, a in ipairs(HOOK.applied) do \
                 out[#out + 1] = a.i .. ':' .. tostring(a.ok) .. (a.why and (':' .. a.why) or '') end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        applied,
        "1:true 2:true 3:true 4:true 5:false:no towns 9 in this world"
    );
}

/// A vehicle sent to be sold on arrival crashes build 40408 at the depot, in
/// every game at the same step (2026-10-02): the vehicle window's send is
/// taken kept, the sale refused at the click and, from an older peer, in
/// every game alike, never sent to the game.
#[test]
fn a_vehicle_is_never_sent_to_be_sold_on_arrival() {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    let (taken, why): (bool, String) = lua
        .load(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua')              local ok, why = pcall(capture.vehicleToDepot, {}, 401, true)              return ok, why",
        )
        .eval()
        .unwrap();
    assert!(!taken);
    assert!(why.contains("the game crashes there"), "{why}");
    lua.load(
        "api.cmd.makeVehicleSendToDepotCmd = function(e, sell) return { toDepot = e, sell = sell } end          HOOK.room = true UPDATE({}, STATE, 0.2)          HOOK.batch = {              { VehicleOp = { vehicle = 0, change = { ToDepot = { sell = true } } } },              { VehicleOp = { vehicle = 0, change = { ToDepot = { sell = false } } } } }          UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let sent: String = lua
        .load(
            "local out = {} for _, c in ipairs(SENT) do                  if c.toDepot then out[#out + 1] = c.toDepot .. ':' .. tostring(c.sell) end end              return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(sent, "401:false");
    let applied: String = lua
        .load(
            "local out = {} for _, a in ipairs(HOOK.applied) do                  out[#out + 1] = a.i .. ':' .. tostring(a.ok) .. (a.why and (':' .. a.why) or '') end              return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        applied,
        "1:false:selling a vehicle when it reaches the depot (the game crashes there) 2:true"
    );
}

/// The store's "buy and put on a line" (2026-09-30): the GUI's world has the
/// new vehicle a moment before the game script's state, which names it, so
/// the store hears of it only once its line assignment can name it; heard
/// before, that went nowhere ("a vehicle the room cannot name").
#[test]
fn a_vehicle_bought_onto_a_line_is_put_on_it_once_the_room_can_name_it() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load(
        "api.cmd.makeVehicleSetLineCmd = function(vehicle, line, stop) return { kind = 'setLine' } end \
         api.type = { ComponentType = { GAME_SCRIPT = 7, CONSTRUCTION = 2 } } \
         VEHICLES = { next = 3, bound = {} } \
         api.engine = { \
             entityExists = function(e) return true end, \
             getComponent = function(e, kind) \
                 if kind == 7 and e == 77 then return { state = { registry = { vehicles = VEHICLES, \
                     lines = { next = 2, bound = { { 1, 600 } } }, groups = { next = 0, bound = {} } } } } end \
                 if kind == 2 and e == 201 then return { fileName = 'depot/bus_depot.con', depots = { 202 }, \
                     transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 600,10,2,1 } } end \
             end, \
             system = { \
                 gameScriptSystem = { getEntityForGameScript = function(name) \
                     if name == 'tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs' then return 77 end return -1 end }, \
                 streetConnectorSystem = { getConstructionEntityForDepot = function(d) \
                     if d == 202 then return 201 end end }, \
             }, \
         } \
         api.res = { modelRep = { getName = function(id) if id == 41 then return 'vehicle/bus/city.mdl' end end } } \
         M = mount(loadPlugin()) M.step() HOOK.room = true \
         CONFIG = { vehicles = { { part = { modelId = 41, reversed = false, \
             compartment2loadConfig = { { loadConfigIndex = 0, cargoTypeId = 3 } }, \
             color = { x = 1, y = 0, z = 0 } } } }, vehicleGroups = { 1 }, muFileNames = { '' } } \
         api.cmd.sendCommand(api.cmd.makeVehicleBuyCmd(25, 202, CONFIG), function(data, ok) \
             HEARD = ok \
             api.cmd.sendCommand(api.cmd.makeVehicleSetLineCmd(data.resultVehicleEntity, 600, -1)) \
         end) \
         M.step() \
         HOOK.results = { { ticket = 1, ok = true, entity = 500 } } M.step()",
    )
    .exec()
    .unwrap();
    let (handed, heard): (usize, Option<bool>) =
        lua.load("return #HOOK.commands, HEARD").eval().unwrap();
    assert_eq!(handed, 1, "only the buy: {:?}", log(&lua));
    assert_eq!(heard, None, "held while the room cannot name the vehicle");
    // The game script's state names it now.
    lua.load("VEHICLES.bound = { { 3, 500 } } M.step()")
        .exec()
        .unwrap();
    let assigned: String = lua
        .load(
            "local a = HOOK.commands[2] and HOOK.commands[2].AssignLine \
             return tostring(HEARD) .. '|' .. (a and (a.vehicles[1] .. '|' .. a.line) or 'none')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(assigned, "true|3|1", "{}", log(&lua));
}

/// Five vehicles bought onto a line in a burst (2026-10-01: only the first
/// got the line). The GUI names them only after 300 frames, more than a
/// frame count waits, and the fourth before the third: each store callback
/// is held, in order, until its own vehicle is named, by the clock, so all
/// five go to the room on their line, each by its own vehicle.
#[test]
fn vehicles_bought_onto_a_line_in_a_burst_all_get_the_line() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load(
        "api.cmd.makeVehicleSetLineCmd = function(vehicle, line, stop) return { kind = 'setLine' } end \
         api.type = { ComponentType = { GAME_SCRIPT = 7, CONSTRUCTION = 2 } } \
         VEHICLES = { next = 10, bound = {} } \
         CLOCK = 1000 \
         os.time = function() return CLOCK end \
         api.engine = { \
             entityExists = function(e) return true end, \
             getComponent = function(e, kind) \
                 if kind == 7 and e == 77 then return { state = { registry = { vehicles = VEHICLES, \
                     lines = { next = 2, bound = { { 1, 600 } } }, groups = { next = 0, bound = {} } } } } end \
                 if kind == 2 and e == 201 then return { fileName = 'depot/bus_depot.con', depots = { 202 }, \
                     transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 600,10,2,1 } } end \
             end, \
             system = { \
                 gameScriptSystem = { getEntityForGameScript = function(name) \
                     if name == 'tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs' then return 77 end return -1 end }, \
                 streetConnectorSystem = { getConstructionEntityForDepot = function(d) \
                     if d == 202 then return 201 end end }, \
             }, \
         } \
         api.res = { modelRep = { getName = function(id) if id == 41 then return 'vehicle/bus/city.mdl' end end } } \
         M = mount(loadPlugin()) M.step() HOOK.room = true \
         CONFIG = { vehicles = { { part = { modelId = 41, reversed = false, \
             compartment2loadConfig = { { loadConfigIndex = 0, cargoTypeId = 3 } }, \
             color = { x = 1, y = 0, z = 0 } } } }, vehicleGroups = { 1 }, muFileNames = { '' } } \
         HEARD = {} \
         for i = 1, 5 do \
             api.cmd.sendCommand(api.cmd.makeVehicleBuyCmd(25, 202, CONFIG), function(data, ok) \
                 HEARD[#HEARD + 1] = data.resultVehicleEntity \
                 api.cmd.sendCommand(api.cmd.makeVehicleSetLineCmd(data.resultVehicleEntity, 600, -1)) \
             end) \
         end \
         M.step() \
         HOOK.results = {} \
         for i = 1, 5 do HOOK.results[i] = { ticket = i, ok = true, entity = 500 + i } end \
         M.step() \
         for frame = 1, 300 do M.step() end \
         CLOCK = CLOCK + 2 \
         VEHICLES.bound = { { 10, 501 }, { 11, 502 }, { 13, 504 } } \
         M.step() \
         EARLY = #HEARD \
         CLOCK = CLOCK + 2 \
         VEHICLES.bound = { { 10, 501 }, { 11, 502 }, { 12, 503 }, { 13, 504 }, { 14, 505 } } \
         M.step()",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let (early, heard, assigned): (usize, String, String) = lua
        .load(
            "local out = {} \
             for i = 6, #HOOK.commands do \
                 local a = HOOK.commands[i].AssignLine \
                 out[#out + 1] = a and (a.vehicles[1] .. '>' .. a.line) or '?' \
             end \
             return EARLY, table.concat(HEARD, ','), table.concat(out, ' ')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        early,
        2,
        "the first two once named; the fourth, named before the third, waits behind it: {}",
        log(&lua)
    );
    assert_eq!(
        heard, "501,502,503,504,505",
        "in order, each its own vehicle"
    );
    assert_eq!(
        assigned,
        "10>1 11>1 12>1 13>1 14>1",
        "every vehicle on its line, by its own id: {}",
        log(&lua)
    );
}

#[test]
fn the_next_reachable_stop_travels_as_the_games_choice() {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    // The line manager's "Next Reachable Stop" is stop -1 (build 40408): the
    // action carries no first stop, and every game is given -1 again.
    let (captured, ok): (String, bool) = lua
        .load(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local ctx = { vehicle = function() return 3 end, line = function() return 1 end } \
             ACTION = capture.vehicleSetLine(ctx, 500, 600, -1) \
             return tostring(ACTION.AssignLine.first_stop), schema_check(ACTION)",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(captured, "nil");
    assert!(ok, "the schema takes it");
    lua.load(format!(
        "HOOK.room = true UPDATE({{}}, STATE, 0.2) \
         HOOK.batch = {{ {BUY_BUS}, {{ AssignLine = {{ vehicles = {{ 2 }}, line = 0 }} }} }} \
         UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    let stop: i64 = lua.load("return SENT[2].setLine.stop").eval().unwrap();
    assert_eq!(stop, -1);
}

#[test]
fn a_line_travels_by_its_stations_ids_and_is_made_again_the_same() {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    lua.load(
        "api.type.Line = { new = function() return { vehicleInfo = {} } end, \
             Stop = { new = function() return {} end }, StopConfig = { new = function() return {} end } } \
         api.type.StationTerminal = { new = function(s, t) return { station = s, terminal = t } end } \
         api.cmd.makeLineCreateCmd = function(name, color, player, line) \
             return { createLine = { name = name, color = color, player = player, line = line } } end",
    )
    .exec()
    .unwrap();
    // The line manager's line, as makeLineCreateCmd gets it: two stops at
    // station groups 90 and 91.
    let (ok, why): (bool, Option<String>) = lua
        .load(
            "HOOK.room = true UPDATE({}, STATE, 0.2) \
             local registry = ug_require('tpf3mp_1::/scripts/tpf3mp/registry.lua') \
             local reg = STATE.value.registry \
             local ctx = { group = function(e) return registry.id(reg, 'groups', e) end, \
                           line = function(e) return registry.id(reg, 'lines', e) end } \
             local function stop(group, mode) return { stationGroup = group, station = 0, terminal = 1, \
                 alternativeTerminals = { { station = 0, terminal = 2 } }, loadMode = mode, \
                 minWaitingTime = 0, maxWaitingTime = 180, maxAdditionalWaitingTime = 30.5, waypoints = {}, \
                 stopConfig = { load = { true, false }, maxLoad = { 1, 0.25 }, forceUnload = false, \
                     destroyForConfigChange = true, destroyForRefresh = false } } end \
             LINE = { stops = { stop(90, 0), stop(91, 2) }, customFilters = false, reservationPriority = 0.5, \
                 vehicleInfo = { transportModes = { [3] = true, [0] = true, [5] = false } } } \
             ACTION = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua').lineCreate(ctx, 'Line 1', \
                 { x = 0.8, y = 0.2, z = 0 }, 25, LINE) \
             return schema_check(ACTION)",
        )
        .eval()
        .unwrap();
    assert!(ok, "{why:?}");
    let carried: String = lua
        .load(
            "local l = ACTION.CreateLine.line local s = l.stops[2] \
             return table.concat({ l.stops[1].group, s.group, s.load_mode, s.terminal.terminal, \
                 s.alternatives[1].terminal, s.max_extra_wait, s.rules.max_load[2], \
                 table.concat(l.modes, ','), l.reservation_priority }, '|')",
        )
        .eval()
        .unwrap();
    assert_eq!(carried, "0|1|FullLoadAll|1|2|30.5|0.25|0,3|0.5");
    // Every game makes it again from the action: stations by their groups
    // here, and its new line named.
    lua.load("HOOK.batch = { ACTION } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let made: String = lua
        .load(
            "local c = SENT[1].createLine local s = c.line.stops[2] \
             return table.concat({ c.name, c.color.x, c.player, c.line.stops[1].stationGroup, \
                 s.stationGroup, s.loadMode, s.alternativeTerminals[1].terminal, \
                 s.stopConfig.maxLoad[2], tostring(s.stopConfig.destroyForConfigChange), \
                 tostring(c.line.vehicleInfo.transportModes[3]), \
                 tostring(c.line.vehicleInfo.transportModes[5]), \
                 HOOK.applied[1].entity }, '|')",
        )
        .eval()
        .unwrap();
    assert_eq!(made, "Line 1|0.8|25|90|91|2|2|0.25|true|true|nil|600");
    // The line system lists it only from the next update; the registry
    // names it at once, and the next action finds it by that name.
    let named: String = lua
        .load(
            "local registry = ug_require('tpf3mp_1::/scripts/tpf3mp/registry.lua') \
             local out = { #LINES, registry.id(STATE.value.registry, 'lines', 600) } \
             api.cmd.makeEntitySetNameCmd = function(e, name) return { rename = { entity = e, name = name } } end \
             HOOK.batch = { { EditLine = { line = 1, change = { Rename = 'North' } } } } UPDATE({}, STATE, 0.2) \
             out[#out + 1] = SENT[2].rename.entity \
             out[#out + 1] = registry.id(STATE.value.registry, 'lines', 600) \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(named, "1 1 600 1");
}

/// A line's waypoints travel with it (they were refused before): a train's
/// on a track's lane, by the edge's ends in its own order, and a ship's or
/// aircraft's in the open, each with its tag. Every game puts them back on
/// its own edge; one whose edge runs the other way there is refused, as the
/// lane's index and place would name another.
#[test]
fn a_lines_waypoints_on_track_and_in_the_open_are_made_again_the_same() {
    let (lua, _script) = engine();
    // Mechanics fixture only: production refuses this channel pending game acceptance.
    lua.load("ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').waypoints = true")
        .exec()
        .unwrap();
    lua.load(FAKE_FLEET).exec().unwrap();
    lua.load(
        r#"
        local CT = api.type.ComponentType
        CT.BASE_EDGE, CT.BASE_NODE, CT.BASE_EDGE_STREET = 11, 12, 13
        NODES = { [51] = { 0, 0, 0 }, [52] = { 80, 0, 0 }, [53] = { 120, 0, 0 }, [54] = { 200, 0, 0 } }
        EDGES = { [61] = { node0 = 51, node1 = 52 }, [62] = { node0 = 54, node1 = 53 } }
        local base = api.engine.getComponent
        api.engine.getComponent = function(e, kind)
            if kind == CT.BASE_NODE and NODES[e] then
                local p = NODES[e] return { position = { x = p[1], y = p[2], z = p[3] } }
            end
            if kind == CT.BASE_EDGE and EDGES[e] then
                return { node0 = EDGES[e].node0, node1 = EDGES[e].node1,
                         tangent0 = { 1, 0, 0 }, tangent1 = { 1, 0, 0 } }
            end
            if kind == CT.BASE_EDGE_STREET then return nil end
            return base(e, kind)
        end
        api.engine.system.streetSystem = {
            getNode2TrackEdgeMap = function() return { [51] = { 61 }, [52] = { 61 }, [53] = { 62 }, [54] = { 62 } } end,
            getNode2StreetEdgeMap = function() return {} end,
            getNodeTrackSegments = function(n)
                if n == 51 or n == 52 then return { 61 } end
                if n == 53 or n == 54 then return { 62 } end
                return {}
            end,
        }
        api.type.Line = { new = function() return { vehicleInfo = {} } end,
            Stop = { new = function() return {} end }, StopConfig = { new = function() return {} end } }
        api.type.StationTerminal = { new = function(s, t) return { station = s, terminal = t } end }
        api.type.Waypoint = { new = function() return {} end }
        api.type.EdgePos = { new = function() return {} end }
        api.type.EdgeId = { new = function(entity, index) return { entity = entity, index = index } end }
        api.cmd.makeLineCreateCmd = function(name, color, player, line)
            return { createLine = { name = name, color = color, player = player, line = line } } end
        "#,
    )
    .exec()
    .unwrap();
    let (ok, why): (bool, Option<String>) = lua
        .load(
            "HOOK.room = true UPDATE({}, STATE, 0.2) \
             local registry = ug_require('tpf3mp_1::/scripts/tpf3mp/registry.lua') \
             local reg = STATE.value.registry \
             local ctx = { group = function(e) return registry.id(reg, 'groups', e) end } \
             local function stop(group, waypoints) return { stationGroup = group, station = 0, terminal = 0, \
                 alternativeTerminals = {}, loadMode = 0, minWaitingTime = 0, maxWaitingTime = 180, \
                 maxAdditionalWaitingTime = 0, waypoints = waypoints, \
                 stopConfig = { load = {}, maxLoad = {}, forceUnload = false, \
                     destroyForConfigChange = false, destroyForRefresh = false } } end \
             local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             LINE = { stops = { \
                 stop(90, { { tag = 3, edgePos = { edgeId = { entity = 61, index = 1 }, param = 0.25 } } }), \
                 stop(91, { { tag = 4, edgePos = { edgeId = { entity = -1, index = 0 }, param = 0 }, \
                              pos = { x = -1200, y = 340, z = 0 } } }) }, \
                 customFilters = false, reservationPriority = 0, vehicleInfo = { transportModes = { [3] = true } } } \
             ACTION = capture.lineCreate(ctx, 'Rail and sea', { x = 0, y = 0, z = 1 }, 25, LINE) \
             REVERSED = capture.lineCreate(ctx, 'Backwards', { x = 0, y = 0, z = 1 }, 25, { stops = { \
                 stop(90, { { tag = 1, edgePos = { edgeId = { entity = 62, index = 0 }, param = 0.5 } } }) }, \
                 customFilters = false, reservationPriority = 0, vehicleInfo = { transportModes = { [3] = true } } }) \
             return schema_check(ACTION)",
        )
        .eval()
        .unwrap();
    assert!(ok, "{why:?}");
    let carried: String = lua
        .load(
            "local s1, s2 = ACTION.CreateLine.line.stops[1], ACTION.CreateLine.line.stops[2] \
             local lane, open = s1.waypoints[1].at.Lane, s2.waypoints[1].at.Open \
             return table.concat({ lane.of.Edge.network, lane.of.Edge.ends.a.x, lane.of.Edge.ends.b.x, \
                 lane.index, lane.param, s1.waypoints[1].tag, open.x, open.y, s2.waypoints[1].tag }, '|')",
        )
        .eval()
        .unwrap();
    assert_eq!(carried, "Track|0|80|1|0.25|3|-1200|340|4");
    // In a world where edge 62 runs from 200 to 120, the reversed one's
    // capture still names it 200 first; flipped here, it is refused.
    lua.load(
        "local e = REVERSED.CreateLine.line.stops[1].waypoints[1].at.Lane.of.Edge.ends \
         e.a, e.b = e.b, e.a \
         HOOK.batch = { ACTION, REVERSED } UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let made: String = lua
        .load(
            "local s = SENT[1].createLine.line.stops \
             local w1, w2 = s[1].waypoints[1], s[2].waypoints[1] \
             return table.concat({ w1.edgePos.edgeId.entity, w1.edgePos.edgeId.index, w1.edgePos.param, w1.tag, \
                 w2.pos.x, w2.pos.y, w2.tag, #SENT, tostring(HOOK.applied[2].ok), HOOK.applied[2].why }, '|')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        made,
        "61|1|0.25|3|-1200|340|4|1|false|a waypoint's edge runs the other way here"
    );
}

#[test]
fn the_bulldozer_removes_a_construction_or_edges_in_every_game() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    // A depot, as the game has it; the game's own remove proposals.
    lua.load(
        "api.type.ComponentType.CONSTRUCTION = 2 \
         CONSTRUCTIONS = { [5000] = { fileName = '::/depots/road/road_depot/road_depot.con', \
             transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 80,0,0,1 } } } \
         local get = api.engine.getComponent \
         api.engine.getComponent = function(e, kind) \
             if kind == 2 then return CONSTRUCTIONS[e] end return get(e, kind) end \
         api.engine.getEntitiesWithComponent = function(kind) \
             local l = {} if kind == 2 then for e in pairs(CONSTRUCTIONS) do l[#l + 1] = e end end return l end \
         api.engine.util.proposal = { \
             createProposalRemove = function(e, context) return { removes = e, player = context.player } end, \
             makeSegmentsRemoveProposal = function(ids) return { removesEdges = table.concat(ids, ',') } end }",
    )
    .exec()
    .unwrap();
    // The bulldozer over the depot: the depot, its entrance edge and node;
    // over the street 8-9: that edge.
    let (depot, street): (String, String) = lua
        .load(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local function part(t) t.addedNodes = t.addedNodes or {} t.addedSegments = t.addedSegments or {} \
                 t.removedSegments = t.removedSegments or {} t.removedNodes = t.removedNodes or {} \
                 t.edgeObjectsToAdd = {} return t end \
             DEPOT = capture.bulldoze({ toAdd = {}, toRemove = { 5000 }, proposal = part({ \
                 removedSegments = { { entity = 6967, type = 0, comp = { node0 = 7, node1 = 2066, objects = {} } } }, \
                 removedNodes = { { entity = 2066, comp = { position = { x = 70, y = 0, z = 0 } } } } }) }) \
             STREET = capture.bulldoze({ toAdd = {}, toRemove = {}, proposal = part({ \
                 removedSegments = { { entity = 100, type = 0, comp = { node0 = 8, node1 = 9, objects = {} } } } }) }) \
             local c, e = DEPOT.Bulldoze.Construction, STREET.Bulldoze.Edges \
             return c.file .. '@' .. c.at.x .. ':' .. tostring(schema_check(DEPOT)), \
                 e.network .. ':' .. e.edges[1].a.y .. '>' .. e.edges[1].b.y .. ':' .. tostring(schema_check(STREET))",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(depot, "::/depots/road/road_depot/road_depot.con@80:true");
    assert_eq!(street, "Street:-40>40:true");
    // Every game removes them as the game itself would: the depot with its
    // own entrance, the edge with the nodes it leaves alone. The player pays.
    lua.load("HOOK.batch = { DEPOT, STREET } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let removed: String = lua
        .load(
            "return SENT[1].proposal.removes .. '|' .. SENT[1].proposal.player .. '|' \
                 .. SENT[2].proposal.removesEdges .. '|' .. tostring(SENT[2].context.player) \
                 .. '|' .. tostring(SENT[2].ignoreErrors) .. '|' .. #HOOK.applied",
        )
        .eval()
        .unwrap_or_else(|error| {
            panic!(
                "{error}\n{:?}",
                lua.load("return HOOK.logged").eval::<Vec<String>>()
            )
        });
    assert_eq!(removed, "5000|25|100|25|true|2");
    // A removal the room cannot carry says why.
    let why: String = lua
        .load(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local _, why = capture.bulldoze({ toAdd = {}, toRemove = {}, proposal = { addedNodes = {}, \
                 addedSegments = {}, removedNodes = {}, edgeObjectsToAdd = {}, removedSegments = { \
                 { entity = 101, type = 0, comp = { node0 = 10, node1 = 7, objects = { { 1, 0 } } } } } } }) \
             return why",
        )
        .eval()
        .unwrap();
    assert_eq!(why, "removing an edge with a stop or signal on it");
}

/// Two tracks stacked over each other along the same line, the older one
/// on the ground (nodes 20 and 21, edge 200) and an elevated one 10 m above
/// it (nodes 30 and 31, edge 300); the street 8-9 (edge 100) with a street
/// 10 m above it (nodes 40 and 41, edge 102). The lower ones have the lower
/// entity ids, which a match by horizontal place alone used to choose.
const STACKED: &str = "
NODES[20] = { x = 0, y = 0, z = 0 } NODES[21] = { x = 100, y = 0, z = 0 }
NODES[30] = { x = 0, y = 0, z = 10 } NODES[31] = { x = 100, y = 0, z = 10 }
EDGES[200] = { node0 = 20, node1 = 21, tangent0 = { x = 100, y = 0, z = 0 },
               tangent1 = { x = 100, y = 0, z = 0 }, objects = {} }
EDGES[300] = { node0 = 30, node1 = 31, tangent0 = { x = 100, y = 0, z = 0 },
               tangent1 = { x = 100, y = 0, z = 0 }, objects = {} }
TRACKS = { [20] = { 200 }, [21] = { 200 }, [30] = { 300 }, [31] = { 300 } }
local streets = api.engine.system.streetSystem
streets.getNode2TrackEdgeMap = function()
    local m = {}
    for node, edges in pairs(TRACKS) do m[node] = edges end
    return m
end
streets.getNodeTrackSegments = function(node) return TRACKS[node] or {} end
NODES[40] = { x = 50, y = -40, z = 10 } NODES[41] = { x = 50, y = 40, z = 10 }
EDGES[102] = { node0 = 40, node1 = 41, tangent0 = { x = 0, y = 80, z = 0 }, tangent1 = { x = 0, y = 80, z = 0 },
               objects = {}, roadTemplate = '::/street/country.street_template', laneConfigs = { 'country lanes' } }
STREETS[40] = { 102 } STREETS[41] = { 102 }
api.engine.util.proposal = {
    makeSegmentsRemoveProposal = function(ids) return { removesEdges = table.concat(ids, ',') } end }
";

/// A track or street named by its ends is the one at their height: with an
/// elevated track over an older one along the same line, a bulldoze or a
/// split of either is of that one in every game, never of the one above or
/// below it (2026-10-16: a match by horizontal place alone took the lower,
/// the lower entity id). Ends at neither height name nothing, and two
/// nodes as near refuse the action rather than choosing between them.
#[test]
fn a_track_over_another_is_named_by_its_height() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(STACKED).exec().unwrap();
    // A bulldoze of the ends at `z`, as every game applies it: the edges it
    // removes, or why it removes none.
    let bulldoze = |setup: &str, z: &str| -> String {
        lua.load(format!(
            "{setup} SENT = {{}} HOOK.logged = {{}} \
             HOOK.batch = {{ {{ Bulldoze = {{ Edges = {{ network = 'Track', buildings = {{}}, \
                 edges = {{ {{ a = {{ x = 100, y = 0, z = {z} }}, b = {{ x = 0, y = 0, z = {z} }} }} }} }} }} }} }} \
             UPDATE({{}}, STATE, 0.2)"
        ))
        .exec()
        .unwrap_or_else(|error| panic!("{error}"));
        lua.load(
            "if #SENT > 0 then return SENT[1].proposal.removesEdges end \
             return 'refused: ' .. table.concat(HOOK.logged, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"))
    };
    assert_eq!(bulldoze("", "10"), "300", "the elevated track");
    assert_eq!(bulldoze("", "0"), "200", "the track on the ground");
    let between = bulldoze("", "5");
    assert!(
        between.starts_with("refused") && between.contains("no Track edge to remove (1)"),
        "{between}"
    );
    let twice = bulldoze(
        "NODES[32] = { x = 0, y = 0, z = 10 } TRACKS[32] = { 300 }",
        "10",
    );
    assert!(
        twice.starts_with("refused") && twice.contains("two nodes as near (0.000, 0.000, 10.000)"),
        "{twice}"
    );

    // A road split onto the street 10 m up splits that street, not the one
    // below it; onto the street below, that one.
    let split = |z: &str| -> String {
        let road = ROAD
            .replace(
                "{ pos = { x = 50, y = 0, z = 0 }",
                &format!("{{ pos = {{ x = 50, y = 0, z = {z} }}"),
            )
            .replace(
                "a = { x = 50, y = 40, z = 0 }, b = { x = 50, y = -40, z = 0 }",
                &format!("a = {{ x = 50, y = 40, z = {z} }}, b = {{ x = 50, y = -40, z = {z} }}"),
            );
        lua.load(format!(
            "NODES[32] = nil TRACKS[32] = nil \
             local apply = ug_require('tpf3mp_1::/scripts/tpf3mp/apply.lua') \
             local p, why = apply.proposalOf({road}, {{}}) \
             if p == nil then return 'refused: ' .. tostring(why) end \
             return table.concat(p.streetProposal.edgesToRemove, ',')"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"))
    };
    assert_eq!(split("10"), "102", "the street above");
    assert_eq!(split("0"), "100", "the street below");
}

/// Tracks with signals on them, as the bulldozer removes them (build 40420,
/// 2026-10-16: "1 Signal Will Be Removed", refused in a room before): the
/// edges go to the room, and every game removes them with exactly the
/// signals on them, whether its own proposal lists them or not.
#[test]
fn the_bulldozer_removes_tracks_with_their_signals_in_every_game() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    lua.load(
        "NODES[20] = { x = 200, y = 0, z = 0 } NODES[21] = { x = 300, y = 0, z = 0 } \
         NODES[22] = { x = 400, y = 0, z = 0 } \
         local t = { x = 100, y = 0, z = 0 } \
         EDGES[200] = { node0 = 20, node1 = 21, tangent0 = t, tangent1 = t, objects = { { 700, 2 } } } \
         EDGES[201] = { node0 = 21, node1 = 22, tangent0 = t, tangent1 = t, objects = { { 701, 2 }, { 702, 2 } } } \
         TRACKS = { [20] = { 200 }, [21] = { 200, 201 }, [22] = { 201 } } \
         local streets = api.engine.system.streetSystem \
         streets.getNode2TrackEdgeMap = function() return TRACKS end \
         streets.getNodeTrackSegments = function(n) return TRACKS[n] or {} end \
         api.type.ComponentType.PLAYER_OWNED = 15 \
         OWNERS = {} \
         local get = api.engine.getComponent \
         api.engine.getComponent = function(e, kind) \
             if kind == 15 then return OWNERS[e] and { player = OWNERS[e] } or nil end return get(e, kind) end \
         GAME_LISTS = {} \
         api.engine.util.proposal = { makeSegmentsRemoveProposal = function(ids) \
             return { removesEdges = table.concat(ids, ','), toAdd = {}, toRemove = {}, \
                 proposal = { edgeObjectsToRemove = GAME_LISTS } } end } \
         function SIGNALLED(removed) \
             local function seg(e) local c, objects = EDGES[e], {} \
                 for i, o in ipairs(EDGES[e].objects) do objects[i] = { o[1], o[2] } end \
                 return { entity = e, type = 1, \
                 comp = { node0 = c.node0, node1 = c.node1, objects = objects } } end \
             return { toAdd = {}, toRemove = {}, proposal = { addedNodes = {}, addedSegments = {}, \
                 removedNodes = {}, removedSegments = { seg(200), seg(201) }, \
                 edgeObjectsToAdd = {}, edgeObjectsToRemove = removed } } \
         end",
    )
    .exec()
    .unwrap();
    let captured: String = lua
        .load(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local action, why = capture.bulldoze(SIGNALLED({ 700, 701, 702 })) \
             if not action then error(why) end \
             TRACKS_GONE = action \
             local e = action.Bulldoze.Edges \
             return table.concat({ e.network, #e.edges, e.edges[1].a.x, e.edges[2].b.x, \
                 tostring(schema_check(action)) }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(captured, "Track|2|200|400|true");

    // A signal missing from the removal, one listed twice, one not on the
    // tracks, a stop on them, or a construction removed with them: refused.
    for (mutation, expected) in [
        ("removed = { 700, 701 }", "removing a stop or signal"),
        (
            "removed = { 700, 701, 702, 702 }",
            "removing a stop or signal",
        ),
        (
            "removed = { 700, 701, 702, 703 }",
            "removing a stop or signal",
        ),
        (
            "p.proposal.removedSegments[2].comp.objects[2] = { 702, 0 }",
            "removing a stop or signal",
        ),
        ("p.toRemove = { 5000 }", "removing a stop or signal"),
        (
            "p.proposal.removedSegments[2].type = 0",
            "removing a stop or signal",
        ),
    ] {
        let why: String = lua
            .load(format!(
                "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
                 local removed = {{ 700, 701, 702 }} \
                 {} \
                 local p = SIGNALLED(removed) {} \
                 local _, why = capture.bulldoze(p) return tostring(why)",
                if mutation.starts_with("removed") {
                    mutation
                } else {
                    ""
                },
                if mutation.starts_with("removed") {
                    ""
                } else {
                    mutation
                },
            ))
            .eval()
            .unwrap();
        assert_eq!(why, expected, "{mutation}");
    }
    // Signals on the removed edges but none in the removal: still refused.
    let why: String = lua
        .load(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local _, why = capture.bulldoze(SIGNALLED({})) return tostring(why)",
        )
        .eval()
        .unwrap();
    assert_eq!(why, "removing an edge with a stop or signal on it");

    // Every game removes both tracks and the three signals: given where
    // its own proposal leaves any out, its own list kept where it names
    // them all.
    for (listed, sent) in [
        ("{}", "700,701,702"),
        ("{ 701 }", "700,701,702"),
        ("{ 702, 700, 701 }", "702,700,701"),
    ] {
        lua.load(format!(
            "GAME_LISTS = {listed} SENT = {{}} HOOK.applied = {{}} \
             HOOK.batch = {{ TRACKS_GONE }} UPDATE({{}}, STATE, 0.2)"
        ))
        .exec()
        .unwrap();
        let removed: String = lua
            .load(
                "local p = SENT[1] and SENT[1].proposal \
                 return table.concat({ tostring(HOOK.applied[1].ok), tostring(p and p.removesEdges), \
                     p and table.concat(p.proposal.edgeObjectsToRemove, ',') or 'none', \
                     tostring(SENT[1] and SENT[1].context.player) }, '|')",
            )
            .eval()
            .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
        assert_eq!(
            removed,
            format!("true|200,201|{sent}|25"),
            "game lists {listed}"
        );
    }

    // A game whose removal would take another object, a stop on a track
    // where the room's has a signal, or another company's signal: refused
    // there, nothing sent.
    for (setup, why) in [
        (
            "GAME_LISTS = { 700, 999 }",
            "the game would remove an edge object 999 the bulldozer did not",
        ),
        (
            "EDGES[201].objects = { { 701, 2 }, { 702, 1 } }",
            "edge 2 has a stop on it",
        ),
        ("OWNERS[701] = 30", "the signal belongs to another company"),
    ] {
        lua.load(format!(
            "GAME_LISTS = {{}} OWNERS = {{}} \
             EDGES[201].objects = {{ {{ 701, 2 }}, {{ 702, 2 }} }} \
             {setup} SENT = {{}} HOOK.applied = {{}} \
             HOOK.batch = {{ TRACKS_GONE }} UPDATE({{}}, STATE, 0.2)"
        ))
        .exec()
        .unwrap();
        let refused: String = lua
            .load("return #SENT .. '|' .. tostring(HOOK.applied[1].ok) .. '|' .. tostring(HOOK.applied[1].why)")
            .eval()
            .unwrap();
        assert_eq!(refused, format!("0|false|{why}"), "{setup}");
    }
}

#[test]
fn the_bulldozer_removes_only_a_stock_airports_frozen_runway_signals() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(
        "api.type.enum.EdgeObjectType = { SIGNAL = 2 } \
         api.type.ComponentType.CONSTRUCTION = 2 \
         local ids = { 6454,6229,7011,7012,7091,7109,7165,7229,7286 } \
         local carriers = { [7614] = 6454, [7615] = 6229, [7617] = 7011, \
             [7620] = 7012, [7621] = 7091, [7622] = 7109, [7625] = 7165, \
             [7626] = 7229, [7627] = 7286 } \
         local frozen = {} for edge = 7608, 7628 do frozen[#frozen+1] = edge end \
         CONSTRUCTIONS = { [77] = { fileName = '::/stations/air/airfield.con', \
             transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 300,0,0,1 }, \
             townBuildings = {}, frozenEdges = frozen } } \
         local prior = api.engine.getComponent \
         api.engine.getComponent = function(entity, kind) \
             if kind == 2 then return CONSTRUCTIONS[entity] end return prior(entity, kind) end \
         function AIRPORT_REMOVE() \
             local segments, removed = {}, {} \
             for i, id in ipairs(ids) do removed[i] = id end \
             for edge = 7608, 7628 do \
                 local id = carriers[edge] local objects = id and { { id, 2 } } or {} \
                 segments[#segments+1] = { entity = edge, type = 0, comp = { objects = objects } } \
             end \
             return { toAdd = {}, toRemove = { 77 }, proposal = { \
                 addedNodes = {}, addedSegments = {}, removedNodes = {}, removedSegments = segments, \
                 edgeObjectsToAdd = {}, edgeObjectsToRemove = removed } } \
         end",
    )
    .exec()
    .unwrap();

    let (accepted, file, at, schema): (bool, String, f64, bool) = lua
        .load(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local action, why = capture.bulldoze(AIRPORT_REMOVE()) \
             if not action then error(why) end \
             local c = action.Bulldoze.Construction \
             return true, c.file, c.at.x, schema_check(action)",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert!(accepted);
    assert_eq!(file, "::/stations/air/airfield.con");
    assert_eq!(at, 300.0);
    assert!(schema);

    let airport: bool = lua
        .load(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             CONSTRUCTIONS[77].fileName = '::/stations/air/airport.con' \
             local action = capture.bulldoze(AIRPORT_REMOVE()) return action ~= nil",
        )
        .eval()
        .unwrap();
    assert!(
        airport,
        "the stock large airport uses the same scoped signal batch"
    );

    // The same object rows on a non-stock construction stay refused.
    let refusal: String = lua
        .load(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             CONSTRUCTIONS[77].fileName = '::/stations/rail/rail_station.con' \
             local _, why = capture.bulldoze(AIRPORT_REMOVE()) return why",
        )
        .eval()
        .unwrap();
    assert_eq!(refusal, "removing a stop or signal");

    // Airport signals carried by an external edge are not its runway batch.
    let refusal: String = lua
        .load(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             CONSTRUCTIONS[77].fileName = '::/stations/air/airfield.con' \
             local p = AIRPORT_REMOVE() p.proposal.removedSegments[7].entity = 9000 \
             local _, why = capture.bulldoze(p) return why",
        )
        .eval()
        .unwrap();
    assert_eq!(refusal, "removing a stop or signal");

    // Stops, unmatched objects, and duplicate carrier IDs remain refused.
    for (mutation, expected) in [
        (
            "p.proposal.removedSegments[#p.proposal.removedSegments] = nil",
            "removing a stop or signal",
        ),
        (
            "p.proposal.removedSegments[#p.proposal.removedSegments+1] = { entity = 9000, type = 0, comp = { objects = {} } }",
            "removing a stop or signal",
        ),
        (
            "p.proposal.removedSegments[7].comp.objects[1][2] = 0",
            "removing a stop or signal",
        ),
        (
            "p.proposal.removedSegments[7].comp.objects[1][1] = 9900",
            "removing a stop or signal",
        ),
        (
            "p.proposal.removedSegments[8].comp.objects[1][1] = 6454",
            "removing a stop or signal",
        ),
    ] {
        let refusal: String = lua
            .load(format!(
                "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
                 local p = AIRPORT_REMOVE() {mutation} \
                 local _, why = capture.bulldoze(p) return why"
            ))
            .eval()
            .unwrap();
        assert_eq!(refusal, expected);
    }
}

/// Town buildings over FAKE_NETWORK, as the game has them: constructions
/// that list their town buildings. 5100 stands by the street 8-9, 5200 by
/// the street 10-7; 5300 is a depot. The game's own remove proposals: the
/// street's gathers the town buildings `GATHER` names, as the bulldozer's
/// did. Asset group 6600 (trees) is no construction.
const FAKE_TOWN: &str = r#"
api.type.ComponentType.CONSTRUCTION = 2
api.type.ComponentType.ASSET_GROUP = 30
api.type.ComponentType.MODEL_INSTANCE_LIST = 31
local function at(x, y) return { 1,0,0,0, 0,1,0,0, 0,0,1,0, x,y,0,1 } end
CONSTRUCTIONS = {
    [5100] = { fileName = '::/buildings/a/c1/4x4_02/a_com_l1_4x4_02.con', townBuildings = { 5101 },
               transf = at(60, -10) },
    [5200] = { fileName = '::/buildings/a/r1/2x2_01/a_res_l1_2x2_01.con', townBuildings = { 5201 },
               transf = at(-30, 8) },
    [5300] = { fileName = '::/depots/road/road_depot/road_depot.con', townBuildings = {},
               transf = at(80, 0) },
}
ASSETS = { [6600] = true }
local get = api.engine.getComponent
api.engine.getComponent = function(e, kind)
    if kind == 2 then return CONSTRUCTIONS[e] end
    if (kind == 30 or kind == 31) and ASSETS[e] then return {} end
    return get(e, kind)
end
api.engine.getEntitiesWithComponent = function(kind)
    local l = {} if kind == 2 then for e in pairs(CONSTRUCTIONS) do l[#l + 1] = e end end
    table.sort(l) return l
end
GATHER = {}
api.engine.util.proposal = {
    createProposalRemove = function(e, context) return { removes = e, player = context.player } end,
    makeSegmentsRemoveProposal = function(ids)
        return { removesEdges = table.concat(ids, ','), toRemove = GATHER }
    end }
-- The bulldozer's proposal: `toRemove`, and edges removed, each { entity, node0, node1 }.
function BULLDOZER(toRemove, edges, toAdd)
    local removed = {}
    for i, e in ipairs(edges or {}) do
        removed[i] = { entity = e[1], type = 0, comp = { node0 = e[2], node1 = e[3], objects = {} } }
    end
    return { toAdd = toAdd or {}, toRemove = toRemove, proposal = { addedNodes = {}, addedSegments = {},
        removedNodes = {}, removedSegments = removed, edgeObjectsToAdd = {} } }
end
"#;

/// A town building the bulldozer removes goes through the room: alone, by
/// its file and place, or with the street it stands by, named beside the
/// street's edges. Every game removes the same one, through the game's own
/// removal, booked to the acting player's company and as the player's own
/// build (so the town's reputation follows in every game alike).
#[test]
fn a_town_building_bulldozed_goes_in_every_game_charged_to_the_players_company() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_TOWN).exec().unwrap();
    let eval = |code: &str| -> String {
        lua.load(code).eval::<String>().unwrap_or_else(|error| {
            panic!(
                "{code}: {error}\n{:?}",
                lua.load("return HOOK.logged").eval::<Vec<String>>()
            )
        })
    };
    // The bulldozer over the town building 5100 alone; over the street 8-9,
    // which the game proposes with 5100 beside it.
    let (alone, street) = (
        eval(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             ALONE = capture.bulldoze(BULLDOZER({ 5100 })) \
             local c = ALONE.Bulldoze.Construction \
             return c.file .. '@' .. c.at.x .. ',' .. c.at.y .. ':' .. tostring(schema_check(ALONE))",
        ),
        eval(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             STREET = capture.bulldoze(BULLDOZER({ 5100 }, { { 100, 8, 9 } })) \
             local e = STREET.Bulldoze.Edges local b = e.buildings[1] \
             return e.network .. ':' .. #e.edges .. ':' .. #e.buildings .. ':' .. b.file .. '@' \
                 .. b.at.x .. ',' .. b.at.y .. ':' .. tostring(schema_check(STREET))",
        ),
    );
    assert_eq!(
        alone,
        "::/buildings/a/c1/4x4_02/a_com_l1_4x4_02.con@60,-10:true"
    );
    assert_eq!(
        street,
        "Street:1:1:::/buildings/a/c1/4x4_02/a_com_l1_4x4_02.con@60,-10:true"
    );
    // A player of the company Rival bulldozes; every game removes the town
    // building, and the street with the one beside it, for Rival.
    lua.load(
        "A = string.rep('a', 64) \
         HOOK.batch = { { CompanyOp = { Create = { name = 'Rival' } } } } HOOK.origins = { A } \
         UPDATE({}, STATE, 0.2) \
         SENT = {} HOOK.applied = {} GATHER = { 5100 } \
         HOOK.batch = { ALONE, STREET } HOOK.origins = { A, A } \
         UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let removed = eval(
        "local out = {} for _, a in ipairs(HOOK.applied) do \
             out[#out + 1] = tostring(a.ok) .. (a.why and (':' .. a.why) or '') end \
         local s1, s2 = SENT[1], SENT[2] \
         return table.concat(out, ',') .. '|' .. s1.proposal.removes .. '>' .. s1.proposal.player \
             .. '>' .. s1.context.player .. '>' .. tostring(s1.playerInitiated) .. '|' \
             .. s2.proposal.removesEdges .. '>' .. s2.context.player .. '>' .. tostring(s2.playerInitiated)",
    );
    assert_eq!(removed, "true,true|5100>901>901>true|100>901>true");
    let logged = eval("return table.concat(HOOK.logged, '|')");
    assert!(
        logged.contains("removing Street edges 100 and town buildings 5100"),
        "{logged}"
    );
}

/// What the room cannot name is refused, never removed in one game alone:
/// at the click, trees and other assets (the asset bulldozer rebuilds their
/// group without them) and a street that takes a depot with it; in every
/// game, a street whose removal there would take another town building than
/// the player's did, or not take the one it did.
#[test]
fn a_bulldoze_the_room_cannot_name_is_refused() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_TOWN).exec().unwrap();
    let why = |code: &str| -> String {
        lua.load(format!(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local action, why = capture.bulldoze({code}) \
             return tostring(why)"
        ))
        .eval::<String>()
        .unwrap_or_else(|error| panic!("{code}: {error}"))
    };
    // Trees taken out of their group: the group removed, and built again
    // without them as a construction of no file.
    assert_eq!(
        why("BULLDOZER({ 6600 }, nil, { { fileName = '' } })"),
        "removing trees or other assets (asset group 6600), which the room does not carry yet"
    );
    assert_eq!(
        why("BULLDOZER({ 6601 })"),
        "removing something that is no construction (entity 6601: no component it knows)"
    );
    assert_eq!(
        why("BULLDOZER({ 5100, 5300 }, { { 100, 8, 9 } })"),
        "removing more than one construction at once"
    );
    assert_eq!(
        why("BULLDOZER({ 5100, 5200 })"),
        "removing more than one construction at once"
    );
    // The street 8-9 with 5100 beside it, as the player's bulldozer showed
    // it; in this game the removal would take 5200 too, then 5300, then
    // nothing: each refused, and nothing sent.
    lua.load(
        "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         STREET = capture.bulldoze(BULLDOZER({ 5100 }, { { 100, 8, 9 } })) \
         SENT = {} HOOK.applied = {} \
         GATHER = { 5100, 5200 } HOOK.batch = { STREET } UPDATE({}, STATE, 0.2) \
         GATHER = { 5100, 5300 } HOOK.batch = { STREET } UPDATE({}, STATE, 0.2) \
         GATHER = {} HOOK.batch = { STREET } UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let refused: String = lua
        .load(
            "local out = {} for _, a in ipairs(HOOK.applied) do \
                 out[#out + 1] = tostring(a.ok) .. ':' .. tostring(a.why) end \
             return table.concat(out, '|') .. '|' .. #SENT",
        )
        .eval()
        .unwrap();
    assert_eq!(
        refused,
        "false:the game would also remove the town building \
         ::/buildings/a/r1/2x2_01/a_res_l1_2x2_01.con, which the player's did not|\
         false:the game would remove ::/depots/road/road_depot/road_depot.con with the streets|\
         false:no town building ::/buildings/a/c1/4x4_02/a_com_l1_4x4_02.con there to remove|0"
    );
}

/// Trees over FAKE_NETWORK and FAKE_TOWN: asset group 7000 of four firs
/// (thin instances), 7001 of two firs elsewhere, 7002 of a fir and a
/// boulder (a full instance, with its own matrix), 7003 of one boulder and
/// 7004 of one fir; the game's model files and octree, and the full
/// proposal's types as TF3 (build 40408) binds them: a
/// `Proposal.ConstructionEntity` whose `fileName` only reads (the desc's),
/// whose `desc` and `construction` (a `ConstructionResult`) are written
/// back whole. `TOOL(removed, group)` is the asset bulldozer's proposal
/// taking the assets at the given indices (thin ones first, then full ones)
/// out of `group` (7000 by default), as UI::AssetBulldozerAction builds it:
/// the group in `toRemove`, and, unless every asset went, one construction
/// entity at the origin, its desc autoRemovable, whose models are the assets
/// kept, thin then full, each its file and world matrix.
const FAKE_TREES: &str = r#"
local FILES = { [41] = 'assets/trees/fir.mdl', [42] = 'assets/rocks/boulder.mdl' }
api.res.modelRep = { getName = function(id) return FILES[id] end }
api.type.Vec2f = { new = function(x, y) return { x = x, y = y } end }
local READ_ONLY = { fileName = true, params = true, hasCargoPlatform = true }
api.type.Proposal = {
    new = function() return { kind = 'Proposal' } end,
    TransformedModel = { new = function() return {} end },
    Subconstruction = { new = function() return {} end },
    ConstructionEntity = { new = function()
        local fields = { desc = { fileName = '', autoRemovable = false }, construction = {}, playerEntity = -1 }
        return setmetatable({}, {
            __index = function(_, k)
                if k == 'fileName' then return fields.desc.fileName end
                return fields[k]
            end,
            __newindex = function(_, k, v)
                if READ_ONLY[k] then error("no writable member '" .. k .. "'") end
                fields[k] = v
            end,
        })
    end },
}
local function fir(x, y, rot) return { modelId = 41, pos = { x = x, y = y, z = 3 }, rot = rot, scale = 1.25 } end
local function boulder(x, y)
    return { modelId = 42, transf = { 0, 2, 0, 0, -2, 0, 0, 0, 0, 0, 2, 0, x, y, 2, 1 } }
end
GROUPS = {
    [7000] = { fir(10, 20, 0), fir(14, 21, 0.5), fir(18, 19, 1), fir(22, 20, 2) },
    [7001] = { fir(400, 20, 0), fir(404, 20, 0) },
    [7002] = { fir(600, 50, 0.25) },
    [7003] = {},
    [7004] = { fir(800, 70, 1.5) },
}
FULL = { [7002] = { boulder(602, 50) }, [7003] = { boulder(700, 60) } }
local get = api.engine.getComponent
api.engine.getComponent = function(e, kind)
    if kind == 30 and GROUPS[e] then return {} end
    if kind == 31 and GROUPS[e] then return { fatInstances = FULL[e] or {}, thinInstances = GROUPS[e] } end
    return get(e, kind)
end
api.engine.util.octree = { findEntitiesInCircle = function(at, r, kind)
    local out = {}
    for e in pairs(GROUPS) do out[#out + 1] = e end
    table.sort(out)
    return out
end }
tpf3mp_native.trees = function() return HOOK.trees == true end
function TOOL(removed, group)
    group = group or 7000
    local engine = ug_require('tpf3mp_1::/scripts/tpf3mp/engine.lua')
    local gone, models, n = {}, {}, 0
    for _, i in ipairs(removed) do gone[i] = true end
    for _, t in ipairs(GROUPS[group]) do
        n = n + 1
        if not gone[n] then
            models[#models + 1] = { id = '::/' .. FILES[t.modelId], tag = '', thin = false,
                transf = engine.assetMatrix({ x = t.pos.x, y = t.pos.y, z = t.pos.z, rot = t.rot,
                    scale = t.scale }, false) }
        end
    end
    for _, f in ipairs(FULL[group] or {}) do
        n = n + 1
        if not gone[n] then
            models[#models + 1] = { id = '::/' .. FILES[f.modelId], tag = '', thin = false, transf = f.transf }
        end
    end
    local toAdd = {}
    if #models > 0 then
        toAdd[1] = { fileName = '', playerEntity = -1, desc = { fileName = '', autoRemovable = true },
            transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 0,0,0,1 },
            construction = { subconstructions = { { models = models } } } }
    end
    return { toRemove = { group }, toAdd = toAdd,
        proposal = { addedNodes = {}, addedSegments = {}, removedNodes = {}, removedSegments = {},
            edgeObjectsToAdd = {} } }
end
"#;

/// Trees bulldozed through the room, behind TPF3MP_TREE_BULLDOZE=1: the
/// player's game names the group by its first tree and how many it holds,
/// and the trees taken out by model and place; every game rebuilds its own
/// copy of the group without them, for the acting company, and says so in
/// its log. Without the flag, and wherever the group is not exactly the
/// player's, nothing is removed.
#[test]
fn trees_bulldozed_go_in_every_game_behind_the_flag() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_TOWN).exec().unwrap();
    lua.load(FAKE_TREES).exec().unwrap();
    let eval = |code: &str| -> String {
        lua.load(code).eval::<String>().unwrap_or_else(|error| {
            panic!(
                "{code}: {error}\n{:?}",
                lua.load("return HOOK.logged").eval::<Vec<String>>()
            )
        })
    };
    let capture = "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') ";
    // Without the flag: today's refusal.
    assert_eq!(
        eval(&format!(
            "{capture} local _, why = capture.bulldoze(TOOL({{ 2 }})) return why"
        )),
        "removing trees or other assets (asset group 7000), which the room does not carry yet"
    );
    // With it: the fir at (14, 21) taken out of the group of four.
    let carried = eval(&format!(
        "HOOK.trees = true {capture} \
         TREES = capture.bulldoze(TOOL({{ 2 }})) \
         local a = TREES.Bulldoze.Assets local r = a.removed[1] \
         return table.concat({{ a.first.model, a.first.at.x, a.first.at.y, a.count, #a.removed, r.model, \
             r.at.x, r.at.y, tostring(a.mirrored), tostring(a.owned), tostring(schema_check(TREES)) }}, '|')"
    ));
    assert_eq!(
        carried,
        "::/assets/trees/fir.mdl|10|20|4|1|::/assets/trees/fir.mdl|14|21|false|false|true"
    );
    // A rebuilt group with a tree the group did not hold is refused.
    assert_eq!(
        eval(&format!(
            "{capture} local p = TOOL({{ 2 }}) \
             p.toAdd[1].construction.subconstructions[1].models[1].transf[13] = 99 \
             local _, why = capture.bulldoze(p) return why"
        )),
        "the rebuilt group holds ::/assets/trees/fir.mdl at 99, 20, 3, which the group did not"
    );
    // Every game: a player of Rival bulldozes; the group is rebuilt with
    // the three firs kept, each where it stood and turned as it was.
    lua.load(
        "A = string.rep('a', 64) \
         HOOK.batch = { { CompanyOp = { Create = { name = 'Rival' } } } } HOOK.origins = { A } \
         UPDATE({}, STATE, 0.2) \
         SENT = {} HOOK.applied = {} HOOK.logged = {} \
         HOOK.batch = { TREES } HOOK.origins = { A } UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let rebuilt = eval(
        "local out = {} for _, a in ipairs(HOOK.applied) do \
             out[#out + 1] = tostring(a.ok) .. (a.why and (':' .. a.why) or '') end \
         local s = SENT[1] local ce = s.proposal.toAdd[1] \
         local models = ce.construction.subconstructions[1].models \
         local m = models[2].transf \
         return table.concat({ table.concat(out, ','), s.proposal.kind, s.proposal.toRemove[1], #models, \
             models[1].id, ce.fileName, ce.playerEntity, s.context.player, tostring(s.playerInitiated), \
             string.format('%.4f,%.4f,%.1f,%.1f', m[1][1], m[1][2], m[4][1], m[4][2]), \
             tostring(ce.desc.autoRemovable) }, '|')",
    );
    assert_eq!(
        rebuilt,
        "true|Proposal|7000|3|::/assets/trees/fir.mdl||-1|901|true|0.6754,1.0518,18.0,19.0|true"
    );
    let logged = eval("return table.concat(HOOK.logged, '|')");
    assert!(
        logged.contains(
            "trees: asset group 7000 of 4 assets, 1 removed (::/assets/trees/fir.mdl at 14.00,21.00), \
             rebuilt with 3"
        ),
        "{logged}"
    );
    assert!(
        logged
            .contains("trees: after the rebuild 1 group(s) hold the first tree kept, of 4 assets"),
        "{logged}"
    );
    // A game whose group is not the player's (a tree fewer) refuses, and
    // sends nothing.
    lua.load(
        "SENT = {} HOOK.applied = {} table.remove(GROUPS[7000], 4) \
         HOOK.batch = { TREES } HOOK.origins = { A } UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    assert_eq!(
        eval(
            "local a = HOOK.applied[1] return tostring(a.ok) .. ':' .. tostring(a.why) .. ':' .. #SENT"
        ),
        "false:no asset group of 4 assets with those trees here:0"
    );
}

/// The asset brush through the room: the stroke as the hook read it (two
/// new groups of firs, and the group of two at (400, 20) it replaces)
/// goes to the room as one PlantAssets action, the replaced group named by
/// its first tree and its count; every game removes its own copy of that
/// group and builds the stroke's groups for the acting company, as the tool
/// does. A game without that group refuses the stroke and builds nothing.
#[test]
fn the_asset_brush_plants_the_same_trees_in_every_game() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_TOWN).exec().unwrap();
    lua.load(FAKE_TREES).exec().unwrap();
    let eval = |code: &str| -> String {
        lua.load(code).eval::<String>().unwrap_or_else(|error| {
            panic!(
                "{code}: {error}\n{:?}",
                lua.load("return HOOK.logged").eval::<Vec<String>>()
            )
        })
    };
    lua.load(
        "local function at(x, y) return { 0.6, 0.8, 0, 0, -0.8, 0.6, 0, 0, 0, 0, 1.25, 0, x, y, 3, 1 } end \
         local fir = '::/assets/trees/fir.mdl' \
         HOOK.room = true HOOK.clicks = 0 SCRIPT.guiUpdate({}, nil, nil) \
         HOOK.built[0] = { proposal = { assets = { removed = { 7001 }, groups = { \
             { { model = fir, m = at(400, 20) }, { model = fir, m = at(404, 20) }, { model = fir, m = at(402, 23) } }, \
             { { model = fir, m = at(410, 30) } } } } } } \
         HOOK.clicks = 1 SCRIPT.guiUpdate({}, nil, nil)",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let carried = eval(
        "STROKE = HOOK.commands[1] local t = STROKE.PlantAssets local r = t.removed[1] \
         local a = t.groups[1][3] \
         return table.concat({ #HOOK.commands, #t.models, t.models[1], r.first.model, r.first.at.x, r.first.at.y, \
             r.count, #t.groups, #t.groups[1], a.model, a.transform.origin.x, a.transform.origin.y, \
             a.transform.basis[2], tostring(schema_check(STROKE)) }, '|')",
    );
    assert_eq!(
        carried,
        "1|1|::/assets/trees/fir.mdl|::/assets/trees/fir.mdl|400|20|2|2|3|0|402|23|0.8|true"
    );
    let logged = eval("return table.concat(HOOK.logged, '|')");
    assert!(
        logged.contains(
            "asset brush handed to the room: 2 group(s) of 4 asset(s), replacing 1 group(s)"
        ),
        "{logged}"
    );
    // Every game: group 7001 removed, the two groups built for the company.
    lua.load("SENT = {} HOOK.applied = {} HOOK.batch = { STROKE } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let built = eval(
        "local a = HOOK.applied[1] local s = SENT[1] local ce = s.proposal.toAdd[1] \
         local models = ce.construction.subconstructions[1].models local m = models[3].transf \
         return table.concat({ tostring(a.ok) .. (a.why and (':' .. a.why) or ''), s.proposal.toRemove[1], \
             #s.proposal.toAdd, #models, models[1].id, ce.playerEntity, s.context.player, \
             tostring(ce.desc.autoRemovable), string.format('%.2f,%.2f,%.1f,%.1f', m[1][1], m[1][2], m[4][1], m[4][2]) }, '|')",
    );
    assert_eq!(
        built,
        "true|7001|2|3|::/assets/trees/fir.mdl|25|25|true|0.60,0.80,402.0,23.0"
    );
    let logged = eval("return table.concat(HOOK.logged, '|')");
    assert!(
        logged.contains("asset brush applied: 2 group(s) of 4 asset(s), replacing 1 group(s)"),
        "{logged}"
    );
    // A game whose group differs (a tree fewer) refuses, and sends nothing.
    lua.load(
        "SENT = {} HOOK.applied = {} table.remove(GROUPS[7001], 2) \
         HOOK.batch = { STROKE } UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    assert_eq!(
        eval(
            "local a = HOOK.applied[1] return tostring(a.ok) .. ':' .. tostring(a.why) .. ':' .. #SENT"
        ),
        "false:no asset group of 2 assets with ::/assets/trees/fir.mdl at 400.00, 20.00 here (group 1 the stroke replaces):0"
    );
}

/// Rocks and whole groups bulldozed through the room, behind
/// TPF3MP_TREE_BULLDOZE=1: a boulder (a full model instance) taken out of a
/// group with a fir, the group rebuilt with the fir alone; the fir taken
/// instead, the group rebuilt with the boulder at its own matrix; a group of
/// one boulder, or of one fir, removed whole with nothing rebuilt, as the
/// tool does. Every game logs the group and what stands after. Two assets
/// of one model at one place are refused, as is a tool that moves a full
/// instance: the room could not say which went, or would build it elsewhere.
#[test]
fn rocks_and_whole_asset_groups_bulldozed_go_in_every_game() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_TOWN).exec().unwrap();
    lua.load(FAKE_TREES).exec().unwrap();
    let eval = |code: &str| -> String {
        lua.load(code).eval::<String>().unwrap_or_else(|error| {
            panic!(
                "{code}: {error}\n{:?}",
                lua.load("return HOOK.logged").eval::<Vec<String>>()
            )
        })
    };
    lua.load(
        "HOOK.trees = true A = string.rep('a', 64) \
         CAPTURE = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         function CARRY(removed, group) \
             local action, why = CAPTURE.bulldoze(TOOL(removed, group)) \
             if action == nil then return nil, why end \
             SENT = {} HOOK.applied = {} HOOK.logged = {} \
             HOOK.batch = { action } HOOK.origins = { A } UPDATE({}, STATE, 0.2) \
             return action \
         end \
         function SAID(action) \
             local a = action.Bulldoze.Assets local r = a.removed[1] \
             return table.concat({ a.first.model, a.first.at.x, a.first.at.y, a.count, #a.removed, r.model, \
                 r.at.x, r.at.y, r.at.z, tostring(schema_check(action)) }, '|') \
         end \
         function BUILT() \
             local out = {} for _, a in ipairs(HOOK.applied) do \
                 out[#out + 1] = tostring(a.ok) .. (a.why and (':' .. a.why) or '') end \
             local p = SENT[1].proposal local ce = p.toAdd and p.toAdd[1] \
             local models = ce and ce.construction.subconstructions[1].models or {} \
             local t = models[1] and models[1].transf \
             return table.concat({ table.concat(out, ','), p.toRemove[1], ce and 1 or 0, #models, \
                 models[1] and models[1].id or '-', \
                 t and string.format('%.3f,%.3f,%.1f,%.1f,%.1f', t[1][1], t[1][2], t[4][1], t[4][2], t[4][3]) \
                     or '-', ce and tostring(ce.desc.autoRemovable) or '-' }, '|') \
         end",
    )
    .exec()
    .unwrap();
    // The boulder out of 7002: the fir stays, rebuilt alone.
    assert_eq!(
        eval("return SAID(CARRY({ 2 }, 7002))"),
        "::/assets/trees/fir.mdl|600|50|2|1|::/assets/rocks/boulder.mdl|602|50|2|true"
    );
    assert_eq!(
        eval("return BUILT()"),
        "true|7002|1|1|::/assets/trees/fir.mdl|1.211,0.309,600.0,50.0,3.0|true"
    );
    let logged = eval("return table.concat(HOOK.logged, '|')");
    assert!(
        logged.contains(
            "trees: asset group 7002 of 2 assets, 1 removed (::/assets/rocks/boulder.mdl at 602.00,50.00), \
             rebuilt with 1"
        ),
        "{logged}"
    );
    // The fir out of 7002 instead: the boulder stays, at its own matrix.
    assert_eq!(
        eval("return SAID(CARRY({ 1 }, 7002))"),
        "::/assets/trees/fir.mdl|600|50|2|1|::/assets/trees/fir.mdl|600|50|3|true"
    );
    assert_eq!(
        eval("return BUILT()"),
        "true|7002|1|1|::/assets/rocks/boulder.mdl|0.000,2.000,602.0,50.0,2.0|true"
    );
    // A group of one boulder: removed whole, nothing rebuilt.
    assert_eq!(
        eval("return SAID(CARRY({ 1 }, 7003))"),
        "::/assets/rocks/boulder.mdl|700|60|1|1|::/assets/rocks/boulder.mdl|700|60|2|true"
    );
    assert_eq!(eval("return BUILT()"), "true|7003|0|0|-|-|-");
    let logged = eval("return table.concat(HOOK.logged, '|')");
    assert!(
        logged.contains(
            "trees: asset group 7003 of 1 assets, 1 removed (::/assets/rocks/boulder.mdl at 700.00,60.00), \
             rebuilt with 0"
        ),
        "{logged}"
    );
    assert!(
        logged.contains(
            "trees: after the rebuild 1 group(s) hold the first tree removed, of 1 assets"
        ),
        "{logged}"
    );
    // A group of one fir: the same.
    assert_eq!(
        eval("return SAID(CARRY({ 1 }, 7004))"),
        "::/assets/trees/fir.mdl|800|70|1|1|::/assets/trees/fir.mdl|800|70|3|true"
    );
    assert_eq!(eval("return BUILT()"), "true|7004|0|0|-|-|-");
    // Without the flag, a whole group's removal stays refused.
    assert_eq!(
        eval("HOOK.trees = false local _, why = CARRY({ 1 }, 7004) HOOK.trees = true return why"),
        "removing trees or other assets (asset group 7004), which the room does not carry yet"
    );
    // Two firs of one model at one place, one taken: refused at the click.
    assert_eq!(
        eval(
            "GROUPS[7005] = { GROUPS[7004][1], { modelId = 41, pos = { x = 800, y = 70, z = 3 }, rot = 0, \
                 scale = 1 } } \
             local _, why = CARRY({ 2 }, 7005) GROUPS[7005] = nil return why"
        ),
        "two assets of ::/assets/trees/fir.mdl at 800.000, 70.000, 3.000: which one went is not clear"
    );
    // A tool that moves the boulder it keeps: refused.
    assert_eq!(
        eval(
            "local p = TOOL({ 1 }, 7002) p.toAdd[1].construction.subconstructions[1].models[1].transf = \
                 { 0, 2, 0, 0, -2, 0, 0, 0, 0, 0, 3, 0, 602, 50, 2, 1 } \
             local _, why = CAPTURE.bulldoze(p) return why"
        ),
        "the tool moves the full instance ::/assets/rocks/boulder.mdl (element 11: 3, not 2)"
    );
}

/// Stops over FAKE_NETWORK: the game's edge object types, the stop's model
/// and construction, and the script proposal's edge object record. Edge
/// 100 runs north from node 8 (50, -40) to node 9 (50, 40).
const FAKE_STOPS: &str = r#"
api.type.ComponentType.EDGE_OBJECT = 14
api.type.enum.EdgeObjectType = { STOP_LEFT = 0, STOP_RIGHT = 1, SIGNAL = 2 }
api.type.SimpleStreetProposal = { EdgeObject = { new = function() return {} end } }
api.res.modelRep = { getName = function(id)
    if id == 77 then return '::/stations/street/small_stops/small_new.con' end
end }
-- Edge objects, as their EDGE_OBJECT component has them.
OBJECTS = {}
local get = api.engine.getComponent
api.engine.getComponent = function(id, kind)
    if kind == 14 then return OBJECTS[id] end
    return get(id, kind)
end
"#;

/// Station groups and ownership for paired-stop replay and capture checks.
const FAKE_STOP_GROUPS: &str = r#"
api.type.ComponentType.STATION_GROUP, api.type.ComponentType.PLAYER_OWNED = 9, 15
STOP_GROUPS, STOP_GROUPS_BY_ENTITY, STOP_OWNERS = {}, {}, {}
api.engine.system.stationGroupSystem = { getStationGroup = function(entity)
    return STOP_GROUPS_BY_ENTITY[entity] or -1
end }
local get = api.engine.getComponent
api.engine.getComponent = function(entity, kind)
    if kind == 9 then return STOP_GROUPS[entity] end
    if kind == 15 then return STOP_OWNERS[entity] and { player = STOP_OWNERS[entity] } or nil end
    return get(entity, kind)
end
function STOP_GROUP(group, stations)
    STOP_GROUPS[group] = { stations = stations }
    for _, station in ipairs(stations) do STOP_GROUPS_BY_ENTITY[station] = group end
end
"#;

/// The stop tool's proposal for a stop left of edge 100, 6 m east of its
/// middle, as build 40408 hands it to game scripts: the edge removed and
/// added again between the same nodes, the new stop in its objects and in
/// `edgeObjectsToAdd`. `old` are the objects the edge had, `kept` those the
/// tool keeps on it and `kept_records` their `edgeObjectsToAdd` records,
/// each followed by a comma.
fn stop_proposal(old: &str, kept: &str, kept_records: &str) -> String {
    let edge = |entity: i64, objects: String| {
        format!(
            "{{ entity = {entity}, type = 0, comp = {{ node0 = 8, node1 = 9, \
             tangent0 = {{ x = 0, y = 80, z = 0 }}, tangent1 = {{ x = 0, y = 80, z = 0 }}, \
             objects = {{ {objects} }} }} }}"
        )
    };
    let removed = edge(100, old.to_owned());
    let added = edge(-1, format!("{kept} {{ -400000000, 0 }}"));
    format!(
        "{{ toAdd = {{}}, toRemove = {{}}, proposal = {{ addedNodes = {{}}, removedNodes = {{}}, \
         removedSegments = {{ {removed} }}, addedSegments = {{ {added} }}, \
         edgeObjectsToAdd = {{ {kept_records} {{ category = 0, left = true, \
             modelInstance = {{ modelId = 77, \
                 transf = {{ 1,0,0,0, 0,1,0,0, 0,0,1,0, 56,0,0,1 }} }} }} }} }} }}"
    )
}

#[test]
fn a_stop_the_stop_tool_placed_goes_to_the_room_and_every_game_places_it() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    let proposal = stop_proposal("", "", "");
    let asked: String = lua
        .load(format!(
            "HOOK.room = true HOOK.clicks = 0 SCRIPT.guiUpdate({{}}, nil, nil) \
             local r = SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'streetTerminalBuilder', \
                 'builder.proposalCreate', {{ {proposal} }}) \
             if r == nil then return 'nil' end \
             for text in pairs(r.errorMessages) do return text end"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(asked, "nil", "the stop tool builds through the room");
    lua.load("HOOK.clicks = 1 SCRIPT.guiUpdate({}, nil, nil)")
        .exec()
        .unwrap();
    let handed: String = lua
        .load(
            "local s = HOOK.commands[1].PlaceStop
             local function n(v) return string.format('%.3f', v) end
             return table.concat({ #HOOK.commands, tostring(schema_check(HOOK.commands[1])),
                 s.edge.network, n(s.edge.ends.a.y), n(s.edge.ends.b.y), n(s.at.x), n(s.at.y),
                 tostring(s.left), n(s.direction.x), n(s.direction.y), s.model }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| {
            panic!(
                "{error}\n{:?}",
                lua.load("return HOOK.logged").eval::<Vec<String>>()
            )
        });
    assert_eq!(
        handed,
        "1|true|Street|-40.000|40.000|50.000|0.000|true|0.000|1.000\
         |::/stations/street/small_stops/small_new.con",
        "the edge by its ends, the stop's place on its centreline, the engine's side, \
         the edge's direction there and the stop's construction"
    );

    // What the room orders, every game places: the edge rebuilt with the
    // stop, as the tool does, paid by the player.
    lua.load("HOOK.batch = { HOOK.commands[1] } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let placed: String = lua
        .load(
            "local c = SENT[1] local p = c.proposal.streetProposal local e = p.edgesToAdd[1]
             local o = p.edgeObjectsToAdd[1]
             return table.concat({ #SENT, e.entity, e.type, e.comp.node0, e.comp.node1,
                 e.comp.roadTemplate, #e.comp.objects, e.comp.objects[1][1], e.comp.objects[1][2],
                 table.concat(p.edgesToRemove, ','), table.concat(p.nodeConfigsToRemove, ','),
                 o.edgeEntity, string.format('%.4f', o.param), tostring(o.left), o.model,
                 o.playerEntity, tostring(c.context.player), tostring(c.ignoreErrors),
                 tostring(c.playerInitiated) }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| {
            panic!(
                "{error}\n{:?}",
                lua.load("return HOOK.logged").eval::<Vec<String>>()
            )
        });
    assert_eq!(
        placed,
        "1|-1|0|8|9|::/street/country.street_template|1|-400000000|0|100|8,9|-1|0.5000|true\
         |::/stations/street/small_stops/small_new.con|25|25|true|true"
    );
}

#[test]
fn bridge_signals_follow_the_cursor_ray_instead_of_the_ground_beyond_it() {
    let (lua, _) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    lua.load(format!("P = {}", stop_proposal("", "", "")))
        .exec()
        .unwrap();
    lua.load(r#"
        NODES[8] = {x=50,y=-100,z=50}
        NODES[9] = {x=50,y=100,z=50}
        P.proposal.edgeObjectsToAdd[1].modelInstance = nil
        P.proposal.edgeObjectsToAdd[1].category = 2
        P.proposal.addedSegments[1].comp.objects[1][2] = 2
        api.gui = {
            mouse = { hasTerrainPosition = function() return true end,
                getTerrainPosition = function() return {x=50,y=50,z=0} end },
            camera = {getEye = function() return {x=50,y=-100,z=150} end}
        }
        local engine = ug_require('tpf3mp_1::/scripts/tpf3mp/engine.lua')
        local action = assert(engine.placeStop(P, 'signal.con', true, {})).PlaceStop
        assert(math.abs(action.at.y) < 0.001, 'bridge signal moved towards the ground hit: ' .. action.at.y)
        assert(math.abs(action.at.z - 50) < 0.001)
        assert(action.object == 'Signal' and action.one_way)
        P.proposal.edgeObjectsToAdd[1].param = 0.25
        local explicit = assert(engine.placeStop(P, 'signal.con', false, {})).PlaceStop
        assert(explicit.at.y < -20, 'explicit proposal position must win over the camera')
    "#).exec().unwrap();
}

/// Several stops clicked quickly in a row (the stop tool takes each click
/// at once in a room, crates/tpf3mp-hook/src/stoptool.rs): each click
/// becomes its own PlaceStop, in click order, also when several clicks
/// come between two GUI updates, and every game places them in that order.
#[test]
fn several_quick_stop_clicks_each_go_to_the_room_in_order() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    let at = |y: i32| stop_proposal("", "", "").replace("56,0,0,1", &format!("56,{y},0,1"));
    let (a, b, c, d) = (at(-30), at(-10), at(10), at(30));
    // The tool proposes each stop when it is clicked (MousePressed), with the
    // clicks counted so far; the count goes up as each is queued. Two clicks
    // come in one GUI frame, then two more.
    lua.load(format!(
        "HOOK.room = true HOOK.clicks = 0 SCRIPT.guiUpdate({{}}, nil, nil) \
         local function click(proposal) \
             local r = SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'streetTerminalBuilder', \
                 'builder.proposalCreate', {{ proposal }}) \
             assert(r == nil, 'the stop tool builds through the room') \
             HOOK.clicks = HOOK.clicks + 1 \
         end \
         click({a}) click({b}) SCRIPT.guiUpdate({{}}, nil, nil) \
         click({c}) click({d}) SCRIPT.guiUpdate({{}}, nil, nil)"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    let handed: String = lua
        .load(
            "local out = {}
             for i, c in ipairs(HOOK.commands) do
                 local s = c.PlaceStop
                 out[i] = s and string.format('%.0f', s.at.y) or 'not a stop'
             end
             return #HOOK.commands .. ':' .. table.concat(out, ',')",
        )
        .eval()
        .unwrap_or_else(|error| {
            panic!(
                "{error}\n{:?}",
                lua.load("return HOOK.logged").eval::<Vec<String>>()
            )
        });
    assert_eq!(
        handed, "4:-30,-10,10,30",
        "one PlaceStop a click, in click order"
    );
}

/// As build 40408 proposes a stop to game scripts: no model and no place on
/// its edge objects (seen in a room: `+o{resultEntity=-1 category=0
/// left=false playerEntity=3869}`). The stop is the construction the
/// construction menu gave the tool, which the GUI noted, where the cursor
/// is; a two-sided one is one click on both sides, and every game builds
/// both.
#[test]
fn a_stop_as_the_game_proposes_it_is_the_noted_construction_under_the_cursor() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    lua.load(
        "api.gui = { mouse = { hasTerrainPosition = function() return true end, \
                               getTerrainPosition = function() return { x = 56, y = 0, z = 0 } end } }",
    )
    .exec()
    .unwrap();
    // Both sides, neither object with a model or a place.
    let proposal = stop_proposal("", "{ -400000001, 1 },", "{ category = 0, left = false },")
        .replace(
            ", \
             modelInstance = { modelId = 77, \
                 transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 56,0,0,1 } }",
            "",
        );
    assert!(!proposal.contains("modelInstance"), "{proposal}");
    let ask = |proposal: &str| -> String {
        lua.load(format!(
            "HOOK.room = true HOOK.clicks = 0 SCRIPT.guiUpdate({{}}, nil, nil) \
             local r = SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'streetTerminalBuilder', \
                 'builder.proposalCreate', {{ {proposal} }}) \
             if r == nil then return 'nil' end \
             for text in pairs(r.errorMessages) do return text end"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"))
    };
    // Nothing noted: which stop it is, the room cannot say.
    assert!(
        ask(&proposal).starts_with("Not in multiplayer yet: the stop's construction"),
        "{}",
        ask(&proposal)
    );
    // The menu gave the tool the two-sided stop: noted in the GUI's state.
    lua.load(
        "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         local util = { getActionParams = function(definition) \
             return { constructionActionParams = { edgeObjectBuilder = { resName = definition } } } end } \
         package.loaded['tpf3mp.stopToolWatched'] = nil \
         assert(capture.watchStopTool(util, ug_require('tpf3mp_1::/scripts/tpf3mp/bridge.lua').attach(tpf3mp_native))) \
         util.getActionParams('stations/street/small_stops/small_new_twosided.con')",
    )
    .exec()
    .unwrap();
    assert_eq!(
        ask(&proposal),
        "nil",
        "the stop tool builds through the room"
    );
    lua.load("HOOK.clicks = 1 SCRIPT.guiUpdate({}, nil, nil)")
        .exec()
        .unwrap();
    let handed: String = lua
        .load(
            "local s = HOOK.commands[1].PlaceStop
             local function n(v) return string.format('%.3f', v) end
             return table.concat({ #HOOK.commands, tostring(schema_check(HOOK.commands[1])),
                 n(s.at.x), n(s.at.y), tostring(s.left), s.model, tostring(s.two_sided) }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        handed,
        "1|true|50.000|0.000|false|stations/street/small_stops/small_new_twosided.con|true"
    );
    // Every game builds it on both sides of the edge, in one proposal.
    lua.load("HOOK.batch = { HOOK.commands[1] } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let placed: String = lua
        .load(
            "local p = SENT[1].proposal.streetProposal local e = p.edgesToAdd[1]
             local out = {}
             for _, o in ipairs(e.comp.objects) do out[#out + 1] = o[1] .. ':' .. o[2] end
             for _, o in ipairs(p.edgeObjectsToAdd) do
                 out[#out + 1] = tostring(o.left) .. ':' .. o.model
             end
             return table.concat(out, '|')",
        )
        .eval()
        .unwrap_or_else(|error| {
            panic!(
                "{error}\n{:?}",
                lua.load("return HOOK.logged").eval::<Vec<String>>()
            )
        });
    assert_eq!(
        placed,
        "-400000000:1|-400000001:0|false:stations/street/small_stops/small_new_twosided.con\
         |true:stations/street/small_stops/small_new_twosided.con"
    );
}

#[test]
fn a_stop_the_room_cannot_carry_says_why() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    let capture = |proposal: String| -> String {
        lua.load(format!(
            "local a, why = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua').stop({proposal})
             if a == nil then return why end
             if type(a) == 'table' then return 'table' end
             return tostring(a)"
        ))
        .eval()
        .unwrap()
    };
    // Dropped where a stop stood: the old one is gone from the new edge.
    assert_eq!(
        capture(stop_proposal("{ 555, 0 },", "", "")),
        "a stop that replaces another"
    );
    // A two-sided stop: a new object on each side, one click.
    let two = stop_proposal("", "{ -400000001, 1 },", "{ category = 0, left = false },");
    assert_eq!(capture(two), "table");
    // A signal the engine lists as a stop, and a side the engine lists
    // other than `left` says.
    let signal = stop_proposal("", "", "").replace("category = 0", "category = 2");
    assert_eq!(capture(signal), "a signal the engine lists as no signal");
    // A signal (the engine's SIGNAL, 2): carried as one, one-way as noted.
    let signal = stop_proposal("", "", "")
        .replace("category = 0", "category = 2")
        .replace("{ -400000000, 0 }", "{ -400000000, 2 }");
    // Without the settings the tool builds it with: refused, never built
    // with the construction's defaults (a mod's spacing on it would be lost).
    let unread: String = lua
        .load(format!(
            "local _, why = ug_require('tpf3mp_1::/scripts/tpf3mp/engine.lua').placeStop({signal}, nil, true) \
             return why"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(unread, "a signal whose settings the room cannot read");
    let carried: String = lua
        .load(format!(
            "local a = ug_require('tpf3mp_1::/scripts/tpf3mp/engine.lua').placeStop({signal}, nil, true, \
                 {{ {{ key = 'auto_signals_distance', value = {{ Int = 4 }} }} }}) \
             return a.PlaceStop.object .. ' ' .. tostring(a.PlaceStop.one_way) .. ' ' \
                 .. a.PlaceStop.params[1].key .. '=' .. a.PlaceStop.params[1].value.Int .. ' ' \
                 .. tostring(schema_check(a))"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(carried, "Signal true auto_signals_distance=4 true");
    // On a track with a signal on it already, the signal tool lists the new
    // signal alone in edgeObjectsToAdd (build 40408, 2026-10-06): paired
    // with the one new object, carried, the other kept. Records that match
    // neither every object nor the new ones are refused.
    let beside_signal = signal
        .replace("objects = {  }", "objects = { { 555, 2 } }")
        .replace("objects = {  {", "objects = { { 555, 2 }, {");
    assert!(
        beside_signal.contains("{ 555, 2 }, { -400000000, 2 }"),
        "{beside_signal}"
    );
    let paired: String = lua
        .load(format!(
            "local a, why = ug_require('tpf3mp_1::/scripts/tpf3mp/engine.lua').placeStop({beside_signal}, nil, true, {{}}) \
             if a == nil then return why end \
             return a.PlaceStop.object .. ' ' .. tostring(a.PlaceStop.left) .. ' ' .. tostring(schema_check(a))"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(paired, "Signal true true");
    let unpaired = beside_signal.replace(
        "edgeObjectsToAdd = {  {",
        "edgeObjectsToAdd = { { category = 2 }, { category = 2 }, {",
    );
    assert!(
        unpaired.contains("{ category = 2 }, { category = 2 }"),
        "{unpaired}"
    );
    let refused: String = lua
        .load(format!(
            "local _, why = ug_require('tpf3mp_1::/scripts/tpf3mp/engine.lua').placeStop({unpaired}, nil, true, {{}}) \
             return why"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(refused, "a stop build whose objects it cannot pair");
    let side = stop_proposal("", "", "").replace("left = true", "left = false");
    assert_eq!(capture(side), "a stop whose side the room cannot say");
    // With a stop on the other side, kept: carried.
    let beside = stop_proposal(
        "{ 555, 1 },",
        "{ 555, 1 },",
        "{ category = 0, left = false },",
    );
    assert_eq!(capture(beside), "table");
    // The tool before its first click: nothing.
    assert_eq!(
        capture(
            "{ toAdd = {}, toRemove = {}, proposal = { addedNodes = {}, removedNodes = {}, \
             addedSegments = {}, removedSegments = {}, edgeObjectsToAdd = {} } }"
                .into()
        ),
        "false"
    );
}

#[test]
fn a_stop_is_placed_beside_the_edges_others_and_never_on_a_taken_side() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    // A stop on the right already; the room orders one on the left, from a
    // game whose edge ran the other way (its direction south): here it is
    // the right, taken.
    let stop = "{ PlaceStop = { edge = { network = 'Street', ends = { a = { x = 50, y = -40, z = 0 }, \
        b = { x = 50, y = 40, z = 0 } } }, at = { x = 50, y = 0, z = 0 }, left = true, \
        direction = { x = 0, y = -1, z = 0 }, model = '::/stations/street/small_stops/small_new.con' } }";
    lua.load(format!(
        "EDGES[100].objects = {{ {{ 555, 1 }} }} HOOK.batch = {{ {stop} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    assert_eq!(lua.load("return #SENT").eval::<usize>().unwrap(), 0);
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged
            .last()
            .unwrap()
            .ends_with("the edge has a stop on that side already"),
        "{logged:?}"
    );
    // Facing north it is the left: placed beside the kept one.
    lua.load(format!(
        "HOOK.batch = {{ {} }} UPDATE({{}}, STATE, 0.2)",
        stop.replace("y = -1", "y = 1")
    ))
    .exec()
    .unwrap();
    let objects: String = lua
        .load(
            "local p = SENT[1].proposal.streetProposal
             local out = {}
             for _, o in ipairs(p.edgesToAdd[1].comp.objects) do out[#out + 1] = o[1] .. ':' .. o[2] end
             return table.concat(out, ',') .. '|' .. tostring(p.edgeObjectsToAdd[1].left)",
        )
        .eval()
        .unwrap();
    assert_eq!(
        objects, "555:1,-400000000:0|true",
        "the kept stop under its own entity"
    );
    // A place off the edge, as another world would have it: placed nowhere.
    lua.load(format!(
        "SENT = {{}} EDGES[100].objects = {{}} HOOK.batch = {{ {} }} UPDATE({{}}, STATE, 0.2)",
        stop.replace("at = { x = 50,", "at = { x = 53,")
    ))
    .exec()
    .unwrap();
    assert_eq!(lua.load("return #SENT").eval::<usize>().unwrap(), 0);
}

/// The stop tool names a stop natively (street_util::MakeEdgeObjectName:
/// a street name from the town's name list, else "Stop #n"), and game
/// scripts read it from the proposal's edge object. The capture carries
/// it, and every game builds the stop with it, its group named so where
/// the game left it unnamed. With the kill switch off, every game names it
/// after its town.
#[test]
fn a_stop_keeps_the_name_the_tool_gave_it_in_every_game() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    let proposal = stop_proposal("", "", "").replace(
        "category = 0, left = true,",
        "category = 0, left = true, name = 'High Street',",
    );
    let (name, ok): (String, bool) = lua
        .load(format!(
            "local engine = ug_require('tpf3mp_1::/scripts/tpf3mp/engine.lua') \
             local a = assert(engine.placeStop({proposal}, '::/stations/street/small_stops/small_new.con')) \
             CAPTURED = a \
             return tostring(a.PlaceStop.name), schema_check(a)"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((name.as_str(), ok), ("High Street", true));
    let captured: mlua::Value = lua.globals().get("CAPTURED").unwrap();
    let action = tpf3mp_proto::lua::action_from_lua(&common::tree(&captured)).unwrap();
    match &action {
        tpf3mp_proto::action::Action::PlaceStop(stop) => {
            assert_eq!(stop.name.as_ref().map(|n| n.as_str()), Some("High Street"))
        }
        other => panic!("{other:?}"),
    }

    for (switch, sent, group) in [
        ("true", "High Street>High Street", "High Street"),
        ("false", "Stop>Stop", "Didcot 2"),
    ] {
        let (lua, _script) = engine();
        lua.load(FAKE_NETWORK).exec().unwrap();
        lua.load(FAKE_STOPS).exec().unwrap();
        lua.load(format!(
            "ug_require('tpf3mp_1::/scripts/tpf3mp/apply.lua').NATIVE_STOP_NAMES = {switch}"
        ))
        .exec()
        .unwrap();
        lua.load(
            STOP_OWNERS
                .replace("{SHARED}", "")
                .replace("{NAME}", ", name = 'High Street'"),
        )
        .exec()
        .unwrap_or_else(|error| panic!("{error}"));
        let (names, named): (String, String) = lua
            .load(
                "local s = SENT[1].proposal.streetProposal \
                 return s.edgeObjectsToAdd[1].name .. '>' .. s.edgeObjectsToAdd[2].name, \
                     tostring(NAMES[610])",
            )
            .eval()
            .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
        assert_eq!(names, sent, "switch {switch}");
        assert_eq!(named, group, "switch {switch}: {}", hook_log(&lua));
    }
}

/// The world of a stop placed for company Rival on edge 100, as build
/// 40408 makes one: the stop's two edge objects are its stations
/// themselves (EDGE_OBJECT and STATION), in one station group the engine
/// made no one's; no construction. `{SHARED}` adds another station to that
/// group.
const STOP_OWNERS: &str = "local CT = api.type.ComponentType \
    CT.CONSTRUCTION, CT.STATION_GROUP, CT.PLAYER_OWNED, CT.STATION = 2, 9, 15, 16 \
    api.type.PlayerOwned = { new = function() return {} end } \
    OWNERS = { [100] = 30 } \
    local groups = { [610] = { stations = { 600, 601 {SHARED} } } } \
    local get = api.engine.getComponent \
    api.engine.getComponent = function(id, kind) \
        if kind == 15 then return OWNERS[id] and { player = OWNERS[id] } or nil end \
        if kind == 9 then return groups[id] end \
        if kind == 16 and (id == 600 or id == 601) then return {} end \
        return get(id, kind) \
    end \
    api.engine.util.construction = { getConstructionEntity = function() return -1 end } \
    api.engine.system.stationGroupSystem = { getStationGroup = function(s) \
        if s == 600 or s == 601 then return 610 end if s == 900 then return 620 end return -1 end } \
    NAMES = { [5000] = 'Didcot', [620] = 'Didcot' } \
    api.engine.util.getEntityName = function(e) return NAMES[e] end \
    api.engine.system.stationSystem = { \
        getTown = function(s) if s == 600 or s == 601 then return 5000 end return -1 end, \
        getStations = function(t) if t == 5000 then return { 600, 601, 900 } end return {} end } \
    api.cmd.makeEntitySetPlayerCmd = function(entity, player) \
        return { setPlayer = entity, player = player } end \
    local send = api.cmd.sendCommand \
    api.cmd.sendCommand = function(cmd, ...) \
        if cmd.setPlayer then OWNERS[cmd.setPlayer] = cmd.player \
        elseif cmd.setName then NAMES[cmd.entity] = cmd.setName \
        elseif cmd.proposal and cmd.proposal.streetProposal then \
            EDGES[100].objects = { { 555, 2 }, { 600, 0 }, { 601, 1 } } \
        end \
        return send(cmd, ...) \
    end \
    EDGES[100].objects = { { 555, 2 } } OWNERS[555] = 30 \
    A = string.rep('a', 64) \
    HOOK.batch = { { CompanyOp = { Create = { name = 'Rival' } } } } HOOK.origins = { A } \
    UPDATE({}, STATE, 0.2) \
    SENT = {} HOOK.applied = {} \
    HOOK.batch = { { PlaceStop = { edge = { network = 'Street', ends = { a = { x = 50, y = -40, z = 0 }, \
        b = { x = 50, y = 40, z = 0 } } }, at = { x = 50, y = 0, z = 0 }, left = true, two_sided = true, \
        direction = { x = 0, y = 1, z = 0 }, \
        model = '::/stations/street/small_stops/small_old_twosided.con' {NAME} } } } \
    HOOK.origins = { A } \
    UPDATE({}, STATE, 0.2)";

/// A stop a company's player placed came out another company's, and had no
/// station icon for its player (2026-10-02, and its retest: the hand-over
/// found the two edge objects alone). A street stop's edge objects are its
/// stations, and its station group, which the windows, the icons and the
/// line manager ask, is the station group system's. Once built, every game
/// gives the stop's objects and their group to the acting company, but not
/// a group another stop's station shares; the edge keeps its own owner.
#[test]
fn a_stop_the_room_places_is_the_acting_companys() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    lua.load(STOP_OWNERS.replace("{SHARED}", "").replace("{NAME}", ""))
        .exec()
        .unwrap_or_else(|error| panic!("{error}"));
    let (ok, built, owners, given): (bool, String, String, String) = lua
        .load(
            "local s = SENT[1].proposal.streetProposal \
             local o = {} \
             for _, e in ipairs({ 555, 600, 601, 610 }) do o[#o + 1] = e .. '=' .. tostring(OWNERS[e]) end \
             local g = {} \
             for i = 2, #SENT do if SENT[i].setPlayer then g[#g + 1] = tostring(SENT[i].setPlayer) end end \
             return HOOK.applied[1].ok == true, \
                 s.edgeObjectsToAdd[1].playerEntity .. '>' .. SENT[1].context.player \
                     .. '>' .. s.edgeObjectsToAdd[1].name .. '>' .. s.edgeObjectsToAdd[2].name \
                     .. '>' .. tostring(s.edgesToAdd[1].playerOwned and s.edgesToAdd[1].playerOwned.player), \
                 table.concat(o, ' '), table.concat(g, ',')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert!(ok, "{}", hook_log(&lua));
    assert_eq!(
        built, "901>901>Stop>Stop>30",
        "the stop named for the acting company, paid by it, with a name; its edge keeps its owner"
    );
    assert_eq!(
        owners, "555=30 600=901 601=901 610=901",
        "the stop's stations and their group the company's; the edge's signal as it was"
    );
    assert_eq!(given, "600,601,610");
    let log = hook_log(&lua);
    assert!(
        log.contains(
            "the new ::/stations/street/small_stops/small_old_twosided.con: \
             600 a station in group 610 (owner nil); 601 a station in group 610 (owner nil)"
        ),
        "{log}"
    );
    assert!(
        log.contains("made the acting company's (901): stop 600 (was nil), stop 601 (was nil), station group 610 (was nil)"),
        "{log}"
    );

    // Its group named after its town, after the town's other group of that
    // name; its stations too.
    let names: String = lua
        .load("return NAMES[610] .. '|' .. NAMES[600] .. '|' .. NAMES[601] .. '|' .. NAMES[620]")
        .eval()
        .unwrap();
    assert_eq!(names, "Didcot 2|Didcot 2|Didcot 2|Didcot");
    assert!(
        log.contains("named station group 610 \"Didcot 2\""),
        "{log}"
    );

    // A group that also holds another stop's station is not this stop's.
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    lua.load(
        STOP_OWNERS
            .replace("{SHARED}", ", 800")
            .replace("{NAME}", ""),
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    let owners: String = lua
        .load(
            "local o = {} \
             for _, e in ipairs({ 600, 601, 610 }) do o[#o + 1] = e .. '=' .. tostring(OWNERS[e]) end \
             return table.concat(o, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(owners, "600=901 601=901 610=nil");
    let name: String = lua.load("return tostring(NAMES[610])").eval().unwrap();
    assert_eq!(name, "nil", "nor its name");
}

#[test]
fn the_bulldozer_removes_a_stop_in_every_game() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    lua.load(
        "OBJECTS[555] = { transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 56,0.2,0,1 }, \
             edgeObjectConstruction = '::/stations/street/small_stops/small_new.con' } \
         OBJECTS[556] = { transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 44,0.2,0,1 }, \
             edgeObjectConstruction = '::/stations/street/small_stops/small_new.con' }",
    )
    .exec()
    .unwrap();
    // The bulldozer over stop 555: edge 100 rebuilt with 556 alone.
    let removal: String = lua
        .load(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua')
             local function seg(e, objects) return { entity = e, type = 0, comp = { node0 = 8, node1 = 9,
                 tangent0 = { x = 0, y = 80, z = 0 }, tangent1 = { x = 0, y = 80, z = 0 },
                 objects = objects } } end
             STOP = capture.bulldoze({ toAdd = {}, toRemove = {}, proposal = { addedNodes = {},
                 removedNodes = {}, removedSegments = { seg(100, { { 555, 0 }, { 556, 1 } }) },
                 addedSegments = { seg(-1, { { 556, 1 } }) }, edgeObjectsToAdd = { { category = 0 } } } })
             local b = STOP.Bulldoze.EdgeObject
             return table.concat({ tostring(schema_check(STOP)), b.edge.network, b.edge.ends.a.y,
                 b.at.x, b.model }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        removal,
        "true|Street|-40|56|::/stations/street/small_stops/small_new.con"
    );
    lua.load(
        "EDGES[100].objects = { { 555, 0 }, { 556, 1 } } HOOK.batch = { STOP } \
         UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let removed: String = lua
        .load(
            "local p = SENT[1].proposal.streetProposal local e = p.edgesToAdd[1]
             return table.concat({ #SENT, #e.comp.objects, e.comp.objects[1][1],
                 table.concat(p.edgesToRemove, ','), table.concat(p.edgeObjectsToRemove, ','),
                 tostring(SENT[1].context.player) }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| {
            panic!(
                "{error}\n{:?}",
                lua.load("return HOOK.logged").eval::<Vec<String>>()
            )
        });
    assert_eq!(
        removed, "1|1|556|100|555|25",
        "the other stop kept under its own entity"
    );
}

/// A two-sided stop, as the bulldozer removes it with one click (build
/// 40420, 2026-10-16: "removing more than one stop at once" in a room): both
/// its stops go to the room as one action, and every game rebuilds the edge
/// once without them, its other stop kept.
#[test]
fn the_bulldozer_removes_both_stops_of_a_two_sided_stop_in_every_game() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    lua.load(FAKE_STOP_GROUPS).exec().unwrap();
    lua.load(
        "local two = '::/stations/street/small_stops/small_new_twosided.con' \
         OBJECTS[555] = { transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 56,0.2,0,1 }, edgeObjectConstruction = two } \
         OBJECTS[556] = { transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 44,0.2,0,1 }, edgeObjectConstruction = two } \
         OBJECTS[558] = { transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 56,20,0,1 }, edgeObjectConstruction = two } \
         function TWO_SIDED(objects, kept) \
             local function seg(e, o) return { entity = e, type = 0, comp = { node0 = 8, node1 = 9, \
                 tangent0 = { x = 0, y = 80, z = 0 }, tangent1 = { x = 0, y = 80, z = 0 }, objects = o } } end \
             return { toAdd = {}, toRemove = {}, proposal = { addedNodes = {}, removedNodes = {}, \
                 removedSegments = { seg(100, objects) }, addedSegments = { seg(-1, kept) }, \
                 edgeObjectsToAdd = {} } } \
         end",
    )
    .exec()
    .unwrap();
    lua.load("STOP_GROUP(600, { 555, 556 })").exec().unwrap();
    let removal: String = lua
        .load(
            "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             local action, why = capture.bulldoze(TWO_SIDED({ { 555, 0 }, { 556, 1 }, { 558, 0 } }, { { 558, 0 } })) \
             if not action then error(why) end \
             STOPS = action \
             local b = action.Bulldoze.EdgeObjects \
             return table.concat({ tostring(schema_check(action)), b.edge.network, #b.objects, \
                 b.objects[1].at.x, b.objects[2].at.x, b.objects[1].model }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        removal,
        "true|Street|2|56|44|::/stations/street/small_stops/small_new_twosided.con"
    );
    // Two stops on one side, or of two constructions, are not one stop.
    for (objects, setup) in [
        ("{ { 555, 0 }, { 556, 0 } }", ""),
        (
            "{ { 555, 0 }, { 556, 1 } }",
            "OBJECTS[556].edgeObjectConstruction = '::/stations/street/small_stops/small_new.con'",
        ),
    ] {
        let why: String = lua
            .load(format!(
                "local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
                 local model = OBJECTS[556].edgeObjectConstruction {setup} \
                 local _, why = capture.bulldoze(TWO_SIDED({objects}, {{}})) \
                 OBJECTS[556].edgeObjectConstruction = model \
                 return tostring(why)"
            ))
            .eval()
            .unwrap();
        assert_eq!(
            why, "removing more than one stop at once",
            "{objects} {setup}"
        );
    }

    // Every game rebuilds edge 100 once, without both, stop 558 kept.
    lua.load(
        "EDGES[100].objects = { { 555, 0 }, { 556, 1 }, { 558, 0 } } HOOK.batch = { STOPS } \
         UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let removed: String = lua
        .load(
            "local p = SENT[1].proposal.streetProposal local e = p.edgesToAdd[1] \
             return table.concat({ #SENT, #e.comp.objects, e.comp.objects[1][1], \
                 table.concat(p.edgesToRemove, ','), table.concat(p.edgeObjectsToRemove, ',') }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert_eq!(removed, "1|1|558|100|555,556");

    // A game where one of them is gone removes neither.
    lua.load(
        "EDGES[100].objects = { { 555, 0 }, { 558, 0 } } SENT = {} HOOK.applied = {} \
         HOOK.batch = { STOPS } UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let refused: String = lua
        .load("return #SENT .. '|' .. tostring(HOOK.applied[1].ok) .. '|' .. tostring(HOOK.applied[1].why)")
        .eval()
        .unwrap();
    assert_eq!(
        refused,
        "0|false|no ::/stations/street/small_stops/small_new_twosided.con there"
    );
}

#[test]
fn the_bulldozer_does_not_pair_two_different_stops() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    lua.load(FAKE_STOP_GROUPS).exec().unwrap();
    let result: String = lua
        .load(
        "local model = '::/stations/street/small_stops/small_new_twosided.con' \
         OBJECTS[555] = { transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 56,0.2,0,1 }, edgeObjectConstruction = model } \
         OBJECTS[556] = { transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 44,0.2,0,1 }, edgeObjectConstruction = model } \
         STOP_GROUP(600, { 555 }) STOP_GROUP(601, { 556 }) \
         local function seg(entity, objects) return { entity = entity, type = 0, comp = { node0 = 8, node1 = 9, \
             tangent0 = { x = 0, y = 80, z = 0 }, tangent1 = { x = 0, y = 80, z = 0 }, objects = objects } } end \
         local capture = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         local action, why = capture.bulldoze({ toAdd = {}, toRemove = {}, proposal = { addedNodes = {}, \
             removedNodes = {}, removedSegments = { seg(100, { { 555, 0 }, { 556, 1 } }) }, \
             addedSegments = { seg(-1, {}) }, edgeObjectsToAdd = {} } }) \
         return tostring(action) .. '|' .. tostring(why)",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(result, "nil|the two stops are not one two-sided stop");
}

#[test]
fn replay_refuses_malformed_or_unrelated_two_sided_stop_objects() {
    let model = "::/stations/street/small_stops/small_new_twosided.con";
    let first = format!("{{ at = {{ x = 56, y = 0.2, z = 0 }}, model = '{model}' }}");
    let second = format!("{{ at = {{ x = 44, y = 0.2, z = 0 }}, model = '{model}' }}");
    let cases = [
        (
            "empty removal",
            "{}".to_owned(),
            "",
            "a two-sided stop removal must name both objects",
            "Street",
        ),
        (
            "one-sided removal",
            format!("{{ {first} }}"),
            "",
            "a two-sided stop removal must name both objects",
            "Street",
        ),
        (
            "signals",
            format!("{{ {first}, {second} }}"),
            "EDGES[100].objects = { { 555, 2 }, { 556, 2 } }",
            "resolved to a signal or waypoint",
            "Street",
        ),
        (
            "different stops",
            format!("{{ {first}, {second} }}"),
            "STOP_GROUP(600, { 555 }) STOP_GROUP(601, { 556 })",
            "the two stops are not one two-sided stop",
            "Street",
        ),
        (
            "different places on one edge",
            format!("{{ {first}, {{ at = {{ x = 44, y = 10.2, z = 0 }}, model = '{model}' }} }}"),
            "OBJECTS[556].transf[14] = 10.2",
            "the two stops are at different places on the edge",
            "Street",
        ),
        (
            "other company owns one stop",
            format!("{{ {first}, {second} }}"),
            "STOP_OWNERS[556] = 30",
            "the stop belongs to another company",
            "Street",
        ),
        (
            "edge not in this game",
            format!("{{ {first}, {second} }}"),
            "STOP_GROUP(600, { 555, 556 })",
            "no Street edge for the stop",
            "Street",
        ),
    ];

    for (name, refs, setup, expected, network) in cases {
        let edge_ends = if name == "edge not in this game" {
            "a = { x = 50, y = -400, z = 0 }, b = { x = 50, y = 400, z = 0 }"
        } else {
            "a = { x = 50, y = -40, z = 0 }, b = { x = 50, y = 40, z = 0 }"
        };
        let (lua, _script) = engine();
        lua.load(FAKE_NETWORK).exec().unwrap();
        lua.load(FAKE_STOPS).exec().unwrap();
        lua.load(FAKE_STOP_GROUPS).exec().unwrap();
        lua.load(format!(
            "local model = '{model}' \
             OBJECTS[555] = {{ transf = {{ 1,0,0,0, 0,1,0,0, 0,0,1,0, 56,0.2,0,1 }}, edgeObjectConstruction = model }} \
             OBJECTS[556] = {{ transf = {{ 1,0,0,0, 0,1,0,0, 0,0,1,0, 44,0.2,0,1 }}, edgeObjectConstruction = model }} \
             EDGES[100].objects = {{ {{ 555, 0 }}, {{ 556, 1 }} }} STOP_GROUP(600, {{ 555, 556 }}) {setup} \
             ACTION = {{ Bulldoze = {{ EdgeObjects = {{ edge = {{ network = '{network}', ends = {{ {edge_ends} }} }}, \
                 objects = {refs} }} }} }} \
             HOOK.batch = {{ ACTION }} UPDATE({{}}, STATE, 0.2)"
        ))
        .exec()
        .unwrap_or_else(|error| panic!("{name}: {error}"));
        let outcome: (usize, bool, String) = lua
            .load(
                "return #SENT, HOOK.applied[1] and HOOK.applied[1].ok == true, \
                 HOOK.applied[1] and tostring(HOOK.applied[1].why) or 'missing result'",
            )
            .eval()
            .unwrap();
        assert_eq!(outcome.0, 0, "{name} sent a build: {}", hook_log(&lua));
        assert!(!outcome.1, "{name} applied: {}", hook_log(&lua));
        assert!(
            outcome.2.contains(expected),
            "{name}: expected {expected:?}, got {:?}; {}",
            outcome.2,
            hook_log(&lua)
        );
    }
}

/// Junctions with traffic lights at both ends of edge 100: node 8, where
/// edge 102 from node 11 meets it, with turns, crosswalks and light phases
/// of its own, and node 9, its end.
const TRAFFIC_LIGHT_ENDS: &str = r#"
NODES[11] = { x = 50, y = -100, z = 0 }
EDGES[102] = { node0 = 11, node1 = 8, tangent0 = { x = 0, y = 60, z = 0 }, tangent1 = { x = 0, y = 60, z = 0 },
               objects = {}, roadTemplate = '::/street/country.street_template', laneConfigs = { 'country lanes' } }
STREETS[8] = { 100, 102 } STREETS[11] = { 102 }
api.res.trafficLightTypeRep = {
    find = function(name) if name == '::/traffic_light/standard.lua' then return 3 end return -1 end,
    getName = function(id) if id == 3 then return '::/traffic_light/standard.lua' end end,
}
local function turn(a, b) return { segment0 = a, lane0 = 0, segment1 = b, lane1 = 0, withRoad = true, withTram = false } end
local function phase(locked, duration, minimum, skip)
    return { lockedLanes = locked, duration = duration, minDuration = minimum, canSkip = skip }
end
CONFIGS[8] = { trafficLightPreference = 1, doubleSlipSwitch = false, userModifiedTrafficLightStates = true,
    laneConnections = { turn(100, 102), turn(102, 100), turn(100, 100) }, crosswalks = { 102, 100 },
    trafficLightConfig = { trafficLightType = 3,
        states = { phase({ 0, 3 }, 30, 10, true), phase({ 1, 2, 4 }, 20, 5, false) } } }
CONFIGS[9] = { trafficLightPreference = 0, doubleSlipSwitch = false, userModifiedTrafficLightStates = false,
    laneConnections = { turn(100, 100) }, crosswalks = { 100 },
    trafficLightConfig = { trafficLightType = 3, states = { phase({ 0 }, 25, 25, false) } } }
-- Both, placed, as the world has them now.
function WORLD_WORDS()
    local function nodeAt(e) return PLACE(NODES[e]) end
    local function edgeAt(e) local c = EDGES[e] return ENDS(NODES[c.node0], NODES[c.node1]) end
    return CONFIG_WORDS({ { entity = 8, comp = CONFIGS[8] }, { entity = 9, comp = CONFIGS[9] } }, nodeAt, edgeAt)
end
-- The edges a sent proposal's configurations name.
function NAMED(sent)
    local named = {}
    for _, c in ipairs(sent.proposal.streetProposal.nodeConfigsToAdd or {}) do
        for _, l in ipairs(c.comp.laneConnections) do named[l.segment0], named[l.segment1] = true, true end
        for _, e in ipairs(c.comp.crosswalks) do named[e] = true end
    end
    local out = {}
    for e in pairs(named) do out[#out + 1] = e end
    table.sort(out)
    return table.concat(out, ',')
end
"#;

/// A stop on a town road between two junctions with traffic lights crashed
/// every game of a room (2026-10-04, build 40408: the rebuild of the road
/// removed the lane configurations at its ends and left their traffic
/// lights, ecs::Engine::GetComponentDataIndex asserting BaseNodeConfig in
/// the simulation). Placing a stop, and the bulldozer removing one, now
/// replace those configurations with the same turns, crosswalks and light
/// phases naming the rebuilt road.
#[test]
fn a_stop_keeps_the_junction_settings_at_its_roads_ends() {
    for removal in [false, true] {
        let (lua, _script) = engine();
        lua.load(FAKE_NETWORK).exec().unwrap();
        lua.load(FAKE_STOPS).exec().unwrap();
        lua.load(CONFIG_WORDS).exec().unwrap();
        lua.load(TRAFFIC_LIGHT_ENDS).exec().unwrap();
        let action = if removal {
            "OBJECTS[555] = { transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 50,0.2,0,1 }, \
                 edgeObjectConstruction = '::/stations/street/small_stops/small_new.con' } \
             EDGES[100].objects = { { 555, 0 } } \
             ACTION = { Bulldoze = { EdgeObject = { edge = { network = 'Street', ends = { \
                 a = { x = 50, y = -40, z = 0 }, b = { x = 50, y = 40, z = 0 } } }, \
                 at = { x = 50, y = 0.2, z = 0 }, model = '::/stations/street/small_stops/small_new.con' } } }"
        } else {
            "ACTION = { PlaceStop = { edge = { network = 'Street', ends = { a = { x = 50, y = -40, z = 0 }, \
                 b = { x = 50, y = 40, z = 0 } } }, at = { x = 50, y = 0, z = 0 }, left = true, two_sided = true, \
                 direction = { x = 0, y = 1, z = 0 }, model = '::/stations/street/small_stops/small_new_twosided.con' } }"
        };
        lua.load(format!(
            "{action} assert(schema_check(ACTION)) BEFORE = WORLD_WORDS() HOOK.batch = {{ ACTION }} \
             UPDATE({{}}, STATE, 0.2)"
        ))
        .exec()
        .unwrap_or_else(|error| panic!("{error}"));
        let (sends, removed, named, sent, before): (usize, String, String, String, String) = lua
            .load(
                "local s = SENT[1] and SENT[1].proposal.streetProposal \
                 local removed = {} \
                 for i, n in ipairs(s and s.nodeConfigsToRemove or {}) do removed[i] = n end \
                 table.sort(removed) \
                 if s then s.nodesToAdd = s.nodesToAdd or {} end \
                 return #SENT, table.concat(removed, ','), s and NAMED(SENT[1]) or '', \
                     s and SENT_WORDS(SENT[1]) or '', BEFORE",
            )
            .eval()
            .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
        assert_eq!(sends, 1, "removal {removal}: {}", hook_log(&lua));
        assert_eq!(
            removed, "8,9",
            "removal {removal}: the settings they replace go"
        );
        assert_eq!(
            named, "-1,102",
            "removal {removal}: the rebuilt road and the other one, never the removed road"
        );
        assert_eq!(
            sent, before,
            "removal {removal}: the turns, crosswalks and lights as they were"
        );
        assert!(
            sent.contains("tl1 type3 [0,3 30/10 skip][1,2,4 20/5]"),
            "removal {removal}: {sent}"
        );
    }
}

/// Where the settings at a stop's road cannot be carried over (here a turn
/// onto a lane the road does not have), every game refuses the stop, and
/// sends nothing, rather than reset the junction or leave it broken.
#[test]
fn a_stop_whose_junction_settings_cannot_be_kept_is_refused() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    lua.load(CONFIG_WORDS).exec().unwrap();
    lua.load(TRAFFIC_LIGHT_ENDS).exec().unwrap();
    lua.load(
        "CONFIGS[9].laneConnections[1].lane1 = 1 \
         HOOK.batch = { { PlaceStop = { edge = { network = 'Street', ends = { a = { x = 50, y = -40, z = 0 }, \
             b = { x = 50, y = 40, z = 0 } } }, at = { x = 50, y = 0, z = 0 }, left = true, \
             direction = { x = 0, y = 1, z = 0 }, model = '::/stations/street/small_stops/small_new.con' } } } \
         UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(lua.load("return #SENT").eval::<usize>().unwrap(), 0);
    assert!(
        hook_log(&lua).contains("the junction's lanes changed"),
        "{}",
        hook_log(&lua)
    );
}

#[test]
fn a_stops_loading_flags_reach_the_game_in_order_however_it_copied_them() {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    // As the game: its binding copies a list it is handed in the order
    // `next` walks it, and the actions reach postUpdate as its own copy of
    // what update returned, whose lists `next` walks in hash order.
    lua.load(
        "api.type.Line = { new = function() return { vehicleInfo = {} } end, \
             Stop = { new = function() return {} end }, \
             StopConfig = { new = function() return setmetatable({}, { __newindex = function(t, k, v) \
                 if type(v) == 'table' then \
                     local copy, key = {}, next(v) \
                     while key ~= nil do copy[#copy + 1] = v[key] key = next(v, key) end \
                     v = copy \
                 end \
                 rawset(t, k, v) end }) end } } \
         api.type.StationTerminal = { new = function(s, t) return { station = s, terminal = t } end } \
         api.cmd.makeLineCreateCmd = function(name, color, player, line) \
             return { createLine = { line = line } } end \
         HOOK.room = true UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    // Passengers loaded, the other 36 cargos not, in a table with no array
    // part, as the game's copy has it.
    let load = lua.create_table_with_capacity(0, 40).unwrap();
    let max_load = lua.create_table_with_capacity(0, 40).unwrap();
    for cargo in 1..=37 {
        load.raw_set(cargo, cargo == 1).unwrap();
        max_load.raw_set(cargo, f64::from(cargo) / 100.0).unwrap();
    }
    lua.globals().set("LOAD", load).unwrap();
    lua.globals().set("MAX_LOAD", max_load).unwrap();
    let flags: String = lua
        .load(
            "local stop = { group = 0, terminal = { station = 0, terminal = 1 }, alternatives = {}, \
                 load_mode = 'LoadIfAvailable', min_wait = 0, max_wait = 180, max_extra_wait = 30, \
                 rules = { load = LOAD, max_load = MAX_LOAD, force_unload = false, \
                           destroy_for_config_change = false, destroy_for_refresh = false } } \
             HOOK.batch = { { CreateLine = { name = 'Line 1', color = { r = 1, g = 0, b = 0 }, \
                 line = { stops = { stop }, modes = { 3 }, custom_filters = false, \
                          reservation_priority = 0 } } } } \
             UPDATE({}, STATE, 0.2) \
             local c = SENT[1].createLine.line.stops[1].stopConfig \
             local on = {} for i = 1, #c.load do if c.load[i] then on[#on + 1] = i end end \
             return #c.load .. ' on@' .. table.concat(on, ',') .. ' max[5]=' .. c.maxLoad[5]",
        )
        .eval()
        .unwrap_or_else(|error| {
            panic!(
                "{error}\n{:?}",
                lua.load("return HOOK.logged").eval::<Vec<String>>()
            )
        });
    assert_eq!(
        flags, "37 on@1 max[5]=0.05",
        "passengers, as the player set them"
    );
}

/// The line manager gives a new line the palette colour fewest of the
/// player's lines wear, telling which one a line wears by floor(channel *
/// 255) (line_vehicle_mgmt/line_util.tl). Its first pick, 255/127/0, came
/// back from the room's millionths one step short on green, so it never
/// counted as taken: every line of a room was orange.
#[test]
fn a_new_lines_colour_comes_back_on_the_games_palette_step() {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    lua.load(
        "api.type.Line = { new = function() return { vehicleInfo = {} } end, \
             Stop = { new = function() return {} end }, StopConfig = { new = function() return {} end } } \
         api.type.StationTerminal = { new = function(s, t) return { station = s, terminal = t } end } \
         api.cmd.makeLineCreateCmd = function(name, color, player, line) \
             return { createLine = { color = color } } end \
         HOOK.room = true UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    // The colour as the game hands it over: 127/255 in a float.
    let green = f64::from(127.0_f32 / 255.0);
    lua.globals().set("GREEN", green).unwrap();
    let action: mlua::Value = lua
        .load(
            "local stop = { group = 0, terminal = { station = 0, terminal = 1 }, alternatives = {}, \
                 load_mode = 'LoadIfAvailable', min_wait = 0, max_wait = 180, max_extra_wait = 30, \
                 rules = { load = { true }, max_load = { 1 }, force_unload = false, \
                           destroy_for_config_change = false, destroy_for_refresh = false } } \
             return { CreateLine = { name = 'Line 1', color = { r = 1, g = GREEN, b = 0 }, \
                 line = { stops = { stop }, modes = { 3 }, custom_filters = false, \
                          reservation_priority = 0 } } }",
        )
        .eval()
        .unwrap();
    // As the room carries it: in millionths.
    let carried = match tpf3mp_proto::lua::action_from_lua(&common::tree(&action)).unwrap() {
        tpf3mp_proto::action::Action::CreateLine(create) => create.color,
        other => panic!("{other:?}"),
    };
    assert_eq!((carried.r, carried.g, carried.b), (1_000_000, 498_039, 0));
    lua.globals()
        .set("CARRIED", f64::from(carried.g) / 1_000_000.0)
        .unwrap();
    let made: String = lua
        .load(
            "HOOK.batch = { { CreateLine = { name = 'Line 1', color = { r = 1, g = CARRIED, b = 0 }, \
                 line = { stops = { { group = 0, terminal = { station = 0, terminal = 1 }, alternatives = {}, \
                     load_mode = 'LoadIfAvailable', min_wait = 0, max_wait = 180, max_extra_wait = 30, \
                     rules = { load = { true }, max_load = { 1 }, force_unload = false, \
                               destroy_for_config_change = false, destroy_for_refresh = false } } }, \
                     modes = { 3 }, custom_filters = false, reservation_priority = 0 } } } } \
             UPDATE({}, STATE, 0.2) \
             local c = SENT[1].createLine.color \
             return table.concat({ math.floor(c.x * 255), math.floor(c.y * 255), math.floor(c.z * 255), \
                 tostring(c.y == 127 / 255) }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(made, "255|127|0|true", "the palette's own orange");
}

#[test]
fn without_callbacks_the_registry_alone_finds_what_an_action_made() {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    lua.load(format!(
        "NO_CALLBACKS = true HOOK.room = true UPDATE({{}}, STATE, 0.2) \
         HOOK.batch = {{ {BUY_BUS} }} UPDATE({{}}, STATE, 0.2) \
         HOOK.batch = {{ {BUY_BUS} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    let (sent, applied, logged): (usize, String, Vec<String>) = lua
        .load(
            "local out = {} for _, a in ipairs(HOOK.applied) do \
                 out[#out + 1] = a.i .. ':' .. tostring(a.ok) .. ':' .. tostring(a.entity) end \
             return #SENT, table.concat(out, ' '), HOOK.logged",
        )
        .eval()
        .unwrap();
    assert_eq!(sent, 2, "each bus bought once, without a callback");
    assert_eq!(
        applied, "1:true:500 1:true:nil",
        "the first found as the registry's new vehicle; the second, the same entity \
         again in this fake, is no new one"
    );
    assert_eq!(
        logged
            .iter()
            .filter(|l| l.contains("takes no command callbacks"))
            .count(),
        1,
        "{logged:?}"
    );
    assert!(
        logged
            .iter()
            .any(|l| l == "action 1 of this step made no vehicles this game could name"),
        "{logged:?}"
    );
}

/// A personal timetable mod's game script (docs/MODS.md): its holds and
/// releases of its own company's vehicles go to the room, once each; what
/// the game's own scripts and shared mods send runs as before.
#[test]
fn a_personal_mods_game_script_hands_its_holds_to_the_room() {
    let lua = gui();
    let (sent, handed, wrapped, logged): (String, Vec<String>, u32, Vec<String>) = lua
        .load(
            r#"
            local modguard = ug_require('tpf3mp_1::/scripts/tpf3mp/modguard.lua')
            SENT, HANDED, LOGGED, CALLERS, NOW, ROOM = {}, {}, {}, {}, 0, true
            local cmd = {
                makeVehicleSetManualDepartureCmd = function(v, m) return { kind = 'manual', v = v, m = m } end,
                makeVehicleTryToDepartCmd = function(v) return { kind = 'depart', v = v } end,
                makeScriptingSendEventCmd = function() return { kind = 'event' } end,
                makeVehicleSellCmd = function() return { kind = 'sell' } end,
                sendCommand = function(c) SENT[#SENT + 1] = c.kind end,
            }
            local wrapped = modguard.install(cmd, {
                inRoom = function() return ROOM end,
                personal = function(mod) return mod == 'celmi_timetables' end,
                callers = function() return CALLERS end,
                command = function(a)
                    local ok, why = schema_check(a)
                    if not ok then error(why) end
                    local c = a.VehicleOp.change
                    if type(c) == 'table' then c = 'ManualDeparture=' .. tostring(c.ManualDeparture) end
                    HANDED[#HANDED + 1] = a.VehicleOp.vehicle .. ':' .. c
                    return true
                end,
                context = { vehicle = function(e) if e == 500 then return 7 elseif e == 600 then return 8 end end },
                mayTouch = function(e)
                    if e == 600 then return false, 'the vehicle belongs to Blue Line' end
                    return true
                end,
                now = function() return NOW end,
                log = function(line) LOGGED[#LOGGED + 1] = line end,
            })
            -- The game's own script and a shared mod: run here, as before.
            cmd.sendCommand(cmd.makeVehicleSetManualDepartureCmd(500, true))
            CALLERS = { 'auto_signals_1' }
            cmd.sendCommand(cmd.makeVehicleTryToDepartCmd(500))
            -- The personal mod: a hold, the same hold again at once, a
            -- release, and a hold again later.
            CALLERS = { 'celmi_timetables' }
            cmd.sendCommand(cmd.makeVehicleSetManualDepartureCmd(500, true))
            cmd.sendCommand(cmd.makeVehicleSetManualDepartureCmd(500, true))
            cmd.sendCommand(cmd.makeVehicleSetManualDepartureCmd(500, false))
            NOW = 6000
            cmd.sendCommand(cmd.makeVehicleSetManualDepartureCmd(500, true))
            -- Another company's vehicle; its events; what it may not do.
            cmd.sendCommand(cmd.makeVehicleTryToDepartCmd(600))
            cmd.sendCommand(cmd.makeScriptingSendEventCmd('', 'celmiTT_held', 'celmiTT_held', {}))
            cmd.sendCommand(cmd.makeVehicleSellCmd({ 500 }))
            -- Through a shared mod's helper, still the personal mod's.
            CALLERS, NOW = { 'shared_lib', 'celmi_timetables' }, 20000
            cmd.sendCommand(cmd.makeVehicleTryToDepartCmd(500))
            -- Outside the room's game, as the game would.
            ROOM = false
            cmd.sendCommand(cmd.makeVehicleTryToDepartCmd(500))
            return table.concat(SENT, ','), HANDED, wrapped, LOGGED
            "#,
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(sent, "manual,depart,depart");
    assert_eq!(
        handed,
        [
            "7:ManualDeparture=true",
            "7:ManualDeparture=false",
            "7:ManualDeparture=true",
            "7:Depart"
        ]
    );
    assert_eq!(wrapped, 3, "the factories the fake api.cmd has");
    let refused: Vec<&String> = logged.iter().filter(|l| !l.starts_with("handed")).collect();
    assert_eq!(
        refused,
        [
            "refused makeVehicleTryToDepartCmd for another company's: the vehicle belongs to Blue Line, from the personal mod celmi_timetables",
            "dropped makeScriptingSendEventCmd (heard by this game's scripts only), from the personal mod celmi_timetables",
            "refused a command no factory made, from the personal mod celmi_timetables",
        ]
    );
    assert_eq!(
        logged[0],
        "handed makeVehicleSetManualDepartureCmd from the personal mod celmi_timetables to the room (1 so far)"
    );
}

/// The GUI's guard (guard.lua) and the personal mods' guard (modguard.lua)
/// each keep the sendCommand they found, so on one api.cmd, in either order,
/// neither swallows the other: outside the room's game a command runs once;
/// a personal mod's hold goes to the room once and does not run here; a
/// command the room does not carry is refused once.
#[test]
fn the_gui_guard_and_the_personal_mods_guard_chain_in_either_order() {
    let lua = gui();
    for guard_first in [true, false] {
        let (sent, handed, refused): (String, u32, u32) = lua
            .load(format!(
                r#"
                local guard = ug_require('tpf3mp_1::/scripts/tpf3mp/guard.lua')
                local modguard = ug_require('tpf3mp_1::/scripts/tpf3mp/modguard.lua')
                -- The GUI state's modules, as its script puts them there.
                package.loaded['tpf3mp.capture'] = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua')
                SENT, HANDED, REFUSED, CALLERS, ROOM = {{}}, 0, 0, {{}}, false
                local cmd = {{
                    makeVehicleSetManualDepartureCmd = function(v, m) return {{ kind = 'manual', v = v, m = m }} end,
                    makeVehicleTryToDepartCmd = function(v) return {{ kind = 'depart', v = v }} end,
                    makeScriptingSendEventCmd = function() return {{ kind = 'event' }} end,
                    sendCommand = function(c) SENT[#SENT + 1] = c.kind end,
                }}
                local context = {{ vehicle = function(e) if e == 500 then return 7 end end }}
                local function hand(a)
                    local ok, why = schema_check(a)
                    if not ok then error(why) end
                    HANDED = HANDED + 1
                    return true
                end
                local function onGuard()
                    guard.install(cmd, {{
                        inRoom = function() return ROOM end,
                        command = hand,
                        refused = function() REFUSED = REFUSED + 1 end,
                        later = function(fn) fn() end,
                        context = context,
                        personal = function(mod) return mod == 'celmi_timetables' end,
                        caller = function() return CALLERS[1] end,
                    }})
                end
                local function onModguard()
                    modguard.install(cmd, {{
                        inRoom = function() return ROOM end,
                        personal = function(mod) return mod == 'celmi_timetables' end,
                        callers = function() return CALLERS end,
                        command = hand,
                        context = context,
                        now = function() return 0 end,
                        log = function() end,
                    }})
                end
                if {guard_first} then onGuard() onModguard() else onModguard() onGuard() end
                -- Outside the room's game: runs here, once.
                cmd.sendCommand(cmd.makeVehicleTryToDepartCmd(500))
                ROOM = true
                -- A personal mod's hold: to the room once, not run here.
                CALLERS = {{ 'celmi_timetables' }}
                cmd.sendCommand(cmd.makeVehicleSetManualDepartureCmd(500, true))
                -- A shared mod's event: the room carries none, refused once.
                CALLERS = {{ 'auto_signals_1' }}
                cmd.sendCommand(cmd.makeScriptingSendEventCmd('', 'x', 'y', {{}}))
                return table.concat(SENT, ','), HANDED, REFUSED
                "#
            ))
            .eval()
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            (sent.as_str(), handed, refused),
            ("depart", 1, 1),
            "guard first: {guard_first}"
        );
    }
}

/// The GUI's guard names the mod a refused command came from, and lets a
/// personal mod's event to its own game script through, in this game only.
#[test]
fn the_guard_names_the_mod_and_lets_a_personal_mods_events_reach_its_script() {
    let lua = gui();
    lua.load(FAKE_CMD).exec().unwrap();
    let (callers, refused, sent): (String, Vec<String>, String) = lua
        .load(
            r#"
            local guard = ug_require('tpf3mp_1::/scripts/tpf3mp/guard.lua')
            local stack = { { source = '@::/gui/main/engine_react_util.tl' },
                            { source = 'tpf3mp_1::/scripts/tpf3mp/guard.lua' },
                            { source = '@celmi_timetables::/timetable/plugins/shared/helpers.script.tl' },
                            { source = 'gw_big_city_1::/gui/x.script.tl' },
                            { source = '@celmi_timetables::/timetable/x.tl' } }
            local callers = table.concat(guard.callers(function(level) return stack[level] end), ',')
            REFUSED, FROM = {}, 'celmi_timetables'
            guard.install(api.cmd, {
                inRoom = function() return true end,
                refused = function(kind, why, from)
                    REFUSED[#REFUSED + 1] = tostring(kind) .. ' ' .. tostring(from)
                end,
                later = function() end,
                context = {},
                personal = function(mod) return mod == 'celmi_timetables' end,
                shared = function() return { 'tpf3mp_1' } end,
                caller = function() return FROM end,
            })
            api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'TimetablesEdit', 'setArrDep', {}))
            FROM = 'gw_big_city_1'
            api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'TimetablesEdit', 'setArrDep', {}))
            FROM = nil
            api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'TimetablesEdit', 'setArrDep', {}))
            -- Made by the mod, sent by the game's own helper (a window's
            -- commit): still the mod's.
            FROM = 'celmi_timetables'
            local made = api.cmd.makeScriptingSendEventCmd('', 'TimetablesEdit', 'setMinWait', {})
            FROM = nil
            api.cmd.sendCommand(made)
            local sent = {}
            for _, s in ipairs(SENT) do sent[#sent + 1] = s.command.kind .. ':' .. tostring(s.command.id) end
            return callers, REFUSED, table.concat(sent, ',')
            "#,
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(callers, "celmi_timetables,gw_big_city_1");
    assert_eq!(
        sent, "event:TimetablesEdit,event:TimetablesEdit",
        "the personal mod's own events only"
    );
    assert_eq!(
        refused,
        [
            "makeScriptingSendEventCmd gw_big_city_1",
            "makeScriptingSendEventCmd nil"
        ]
    );
}

/// A personal mod's events to the game's own scripts (a rank, prospecting,
/// a loan) take the room's way, carried or refused, as a click's would; only
/// an event addressed to the mod's own game script passes, here alone.
#[test]
fn a_personal_mods_events_to_the_games_scripts_take_the_rooms_way() {
    let lua = gui();
    lua.load(FAKE_CMD).exec().unwrap();
    let (sent, handed, refused, own): (String, Vec<String>, Vec<String>, Vec<bool>) = lua
        .load(
            r#"
            local guard = ug_require('tpf3mp_1::/scripts/tpf3mp/guard.lua')
            HANDED, REFUSED = {}, {}
            guard.install(api.cmd, {
                inRoom = function() return true end,
                command = function(action)
                    local kind = next(action)
                    HANDED[#HANDED + 1] = kind
                    return true
                end,
                refused = function(kind, why) REFUSED[#REFUSED + 1] = tostring(why) end,
                later = function() end,
                context = { town = function() return nil end, player = function() return 1 end },
                personal = function(mod) return mod == 'celmi_timetables' end,
                shared = function() return { 'tpf3mp_1', 'other_mod_1' } end,
                caller = function() return 'celmi_timetables' end,
            })
            local ev = api.cmd.makeScriptingSendEventCmd
            api.cmd.sendCommand(ev('', 'Companies', 'applyLevel', { level = 2 }))
            api.cmd.sendCommand(ev('', 'Loan', 'Obtain', { { amount = 1 }, { amount = 2 } }))
            api.cmd.sendCommand(ev('', 'Companies', 'spawnIndustry', { companyEntity = 1 }))
            -- Under its own id, but a name the company script hears whatever
            -- the id, and an id of the game's own: not its own.
            api.cmd.sendCommand(ev('', 'TimetablesEdit', 'company.lockPermits', {}))
            api.cmd.sendCommand(ev('', 'tpf3mp', 'command', {}))
            -- Its own script's.
            api.cmd.sendCommand(ev('', 'TimetablesEdit', 'setArrDep', {}))
            local sent = {}
            for _, s in ipairs(SENT) do sent[#sent + 1] = tostring(s.command.id) .. ':' .. tostring(s.command.name) end
            local shared = { 'tpf3mp_1' }
            local own = {
                guard.ownEvent('celmi_timetables', 'TimetablesEdit', 'setArrDep', shared),
                guard.ownEvent('celmi_timetables', 'celmi_timetables', 'x', shared),
                guard.ownEvent('celmi_timetables', 'Notifications', 'add', shared),
                guard.ownEvent('celmi_timetables', 'OtherModChannel', 'x', shared),
                guard.ownEvent('gw_big_city_1', 'big', 'x', shared),
                -- A shared mod hears the same id: not the personal mod's alone.
                guard.ownEvent('timetables_ui_tweak', 'TimetablesEdit', 'x', { 'celmi_timetables' }),
                -- Without the room's shared list, nothing is its own.
                guard.ownEvent('celmi_timetables', 'TimetablesEdit', 'setArrDep', nil),
            }
            return table.concat(sent, ','), HANDED, REFUSED, own
            "#,
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        sent, "TimetablesEdit:setArrDep",
        "only its own event runs here"
    );
    assert_eq!(handed, ["ApplyRank", "Loan"], "carried as a click's are");
    assert_eq!(refused.len(), 3, "{refused:?}");
    assert_eq!(own, [true, true, false, false, false, false, false]);
}

#[test]
fn a_window_hears_of_what_its_command_made_once_its_world_has_it() {
    let lua = gui();
    let heard: String = lua
        .load(
            "local guard = ug_require('tpf3mp_1::/scripts/tpf3mp/guard.lua') \
             guard.CARRY.makeLineCreateCmd = function() return { CreateLine = {} } end \
             local cmd = { makeLineCreateCmd = function(name) return { kind = 'line', name = name } end, \
                           sendCommand = function() end } \
             local tickets = 0 \
             guard.install(cmd, { inRoom = function() return true end, \
                 command = function() tickets = tickets + 1 return true, tickets end, \
                 refused = function() end, later = function(fn) fn() end, context = {} }) \
             HEARD = {} \
             local function hear(data, ok, entities) \
                 HEARD[#HEARD + 1] = tostring(ok) .. ':' .. tostring(entities[1] and entities[1][1]) \
                     .. ':' .. tostring(data.resultEntity) end \
             cmd.sendCommand(cmd.makeLineCreateCmd('Line 1'), hear) \
             cmd.sendCommand(cmd.makeLineCreateCmd('Line 2'), hear) \
             EXISTS = {} \
             local function sees(e) return EXISTS[e] == true end \
             local out = {} \
             out[#out + 1] = guard.deliver(cmd, { { ticket = 1, ok = true, entity = 600 }, \
                                                  { ticket = 2, ok = true } }, sees) \
             out[#out + 1] = #HEARD \
             EXISTS[600] = true \
             out[#out + 1] = guard.deliver(cmd, {}, sees) \
             out[#out + 1] = table.concat(HEARD, ' ') \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    // Line 1 made 600, which this world did not show yet; line 2 made
    // nothing the game could name.
    assert_eq!(
        heard, "0 0 2 true:600:600 false:nil:nil",
        "held, in order, until the world has it; a creation that made nothing \
         is answered as failed"
    );
}

/// The GUI of a room with vehicles 500 and 501 bound to ids 3 and 4, each a
/// locomotive (model 41, bought at game time 1000) and a coach (model 42,
/// bought at 2000), as their TRANSPORT_VEHICLE components have them; 502 is
/// a vehicle the registry has no id for.
const FAKE_TRAINS_GUI: &str = r#"
api.cmd.makeVehicleReplaceCmd = function(vehicle, config)
    return { kind = 'replace', vehicle = vehicle, config = config }
end
api.type = { ComponentType = { GAME_SCRIPT = 7, TRANSPORT_VEHICLE = 4 } }
local function train()
    return { transportVehicleConfig = { vehicles = {
        { part = { modelId = 41 }, purchaseTime = 1000 },
        { part = { modelId = 42 }, purchaseTime = 2000 },
    } } }
end
api.engine = {
    getComponent = function(e, kind)
        if kind == 7 and e == 77 then return { state = { registry = {
            vehicles = { next = 5, bound = { { 3, 500 }, { 4, 501 } } },
            lines = { next = 0, bound = {} }, groups = { next = 0, bound = {} } } } } end
        if kind == 4 and (e == 500 or e == 501 or e == 502) then return train() end
    end,
    system = {
        gameScriptSystem = { getEntityForGameScript = function(name)
            if name == 'tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs' then return 77 end return -1 end },
    },
}
local NAMES = { [41] = 'vehicle/train/loco.mdl', [42] = 'vehicle/waggon/coach.mdl' }
api.res = { modelRep = { getName = function(id) return NAMES[id] end } }
-- A part as the store hands it on: the vehicle's own, or new, bought at
-- the GUI's game time (HandleVehicleChanges sets it before it sends).
function PART(model, purchased, reversed)
    return { part = { modelId = model, reversed = reversed == true,
                      compartment2loadConfig = { { loadConfigIndex = 0, cargoTypeId = 0 } },
                      color = { x = 1, y = 0.5, z = 0 } },
             purchaseTime = purchased, autoLoadConfig = { true } }
end
"#;

/// The GUI with FAKE_TRAINS_GUI, in the room's game.
fn trains_gui() -> Lua {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load(FAKE_TRAINS_GUI).exec().unwrap();
    lua.load("M = mount(loadPlugin()) M.step() HOOK.room = true")
        .exec()
        .unwrap();
    lua
}

#[test]
fn a_group_replacement_goes_to_the_room_vehicle_by_vehicle_by_canonical_ids() {
    let lua = trains_gui();
    // The vehicle window's "modify" on two trains at once, as
    // vehicle_react_util.tl sends it: one command per vehicle, no callback.
    // The first keeps its locomotive (turned round) and coach and gets a
    // new coach; the second has its own two parts the other way round.
    lua.load(
        "local first = { vehicles = { PART(41, 1000, true), PART(42, 2000), PART(42, 5000) }, \
                         vehicleGroups = { 1, 1, 1 }, muFileNames = { '', '', '' } } \
         local second = { vehicles = { PART(42, 2000), PART(41, 1000) }, \
                          vehicleGroups = { 1, 1 }, muFileNames = { '', '' } } \
         api.cmd.sendCommand(api.cmd.makeVehicleReplaceCmd(500, first)) \
         api.cmd.sendCommand(api.cmd.makeVehicleReplaceCmd(501, second)) \
         M.step()",
    )
    .exec()
    .unwrap();
    let (handed, sent): (usize, usize) = lua.load("return #HOOK.commands, #SENT").eval().unwrap();
    assert_eq!(handed, 2, "every vehicle of the group goes to the room");
    assert_eq!(sent, 0, "none is sent here: the room orders it");
    let captured: String = lua
        .load(
            "local out = {} \
             for _, c in ipairs(HOOK.commands) do \
                 local r = c.ReplaceVehicle local parts = {} \
                 for _, p in ipairs(r.consist) do \
                     parts[#parts + 1] = p.part.model .. (p.part.reversed and '<' or '>') \
                         .. tostring(p.kept) end \
                 out[#out + 1] = r.vehicle .. '=' .. table.concat(parts, ',') \
                     .. '/' .. #r.groups .. '/' .. r.consist[1].part.color.g end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        captured,
        "3=vehicle/train/loco.mdl<0,vehicle/waggon/coach.mdl>1,vehicle/waggon/coach.mdl>nil/3/0.5 \
         4=vehicle/waggon/coach.mdl>1,vehicle/train/loco.mdl>0/2/0.5",
        "by canonical id; a part the vehicle has is kept by its index, a bought one is new"
    );
}

#[test]
fn a_replacement_the_room_cannot_name_is_refused_with_why() {
    let lua = trains_gui();
    // A vehicle with no canonical id; a model with no name; no parts.
    lua.load(
        "CALLED = nil \
         api.cmd.sendCommand(api.cmd.makeVehicleReplaceCmd(502, { vehicles = { PART(41, 1000) }, \
             vehicleGroups = { 1 }, muFileNames = { '' } }), function(_, ok) CALLED = ok end) \
         api.cmd.sendCommand(api.cmd.makeVehicleReplaceCmd(500, { vehicles = { PART(99, 5000) }, \
             vehicleGroups = { 1 }, muFileNames = { '' } })) \
         api.cmd.sendCommand(api.cmd.makeVehicleReplaceCmd(500, { vehicles = {}, \
             vehicleGroups = {}, muFileNames = {} })) \
         M.step()",
    )
    .exec()
    .unwrap();
    let (handed, sent, called): (usize, usize, bool) = lua
        .load("return #HOOK.commands, #SENT, CALLED")
        .eval()
        .unwrap();
    assert_eq!((handed, sent), (0, 0), "nothing goes anywhere");
    assert!(!called, "a callback hears it failed");
    assert_eq!(
        shown(&lua).as_deref(),
        Some("Not in multiplayer yet: replacing vehicles")
    );
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    for why in [
        "(1 so far): a vehicle the room cannot name",
        "(2 so far): a vehicle model the room cannot name",
        "(3 so far): a replacement of no vehicles",
    ] {
        let line = format!("refused the player's makeVehicleReplaceCmd in the room's game {why}");
        assert!(logged.contains(&line), "{line} not in {logged:?}");
    }
}

#[test]
fn a_replacement_with_a_callback_hears_the_vehicle_as_it_is_after() {
    let lua = trains_gui();
    lua.load(
        "api.engine.entityExists = function() return true end \
         HEARD = nil \
         api.cmd.sendCommand(api.cmd.makeVehicleReplaceCmd(500, { vehicles = { PART(41, 1000) }, \
             vehicleGroups = { 1 }, muFileNames = { '' } }), function(data, ok, entities) \
             HEARD = data.vehicleEntity .. ':' .. tostring(ok) .. ':' .. entities[1][1] \
                 .. ':' .. #data.config.vehicles end) \
         M.step() \
         HOOK.results = { { ticket = 1, ok = true, entity = 500 } } M.step()",
    )
    .exec()
    .unwrap();
    let heard: String = lua.load("return HEARD").eval().unwrap();
    assert_eq!(heard, "500:true:500:1");
}

/// FAKE_FLEET's vehicle 401 as a train: a locomotive (model 41) bought at
/// 1000 and worn to 0.7, and a coach (42). A replacement is sent as the
/// game's command; REPLACE_AS makes the game give the vehicle a new entity.
const FAKE_TRAINS: &str = r#"
REPLACE_AS = nil
OWN = { [401] = {
    { part = { modelId = 41 }, purchaseTime = 1000, maintenanceState = 0.7, maintenanceChange = 0.01 },
    { part = { modelId = 42 }, purchaseTime = 2000, maintenanceState = 0.9, maintenanceChange = 0.02 },
} }
local component = api.engine.getComponent
api.engine.getComponent = function(e, kind)
    local c = component(e, kind)
    if c and kind == api.type.ComponentType.TRANSPORT_VEHICLE then
        c.transportVehicleConfig = { vehicles = OWN[e] or {} }
    end
    return c
end
local find = api.res.modelRep.find
api.res.modelRep.find = function(name)
    if name == 'vehicle/train/loco.mdl' then return 41 end
    if name == 'vehicle/waggon/coach.mdl' then return 42 end
    return find(name)
end
api.cmd.makeVehicleReplaceCmd = function(vehicle, config)
    return { replace = { vehicle = vehicle, config = config }, vehicleEntity = vehicle }
end
local send = api.cmd.sendCommand
api.cmd.sendCommand = function(command, ...)
    if command.replace and REPLACE_AS then
        for i, e in ipairs(VEHICLES) do
            if e == command.replace.vehicle then table.remove(VEHICLES, i) break end
        end
        VEHICLES[#VEHICLES + 1] = REPLACE_AS
        command.made = REPLACE_AS
    end
    send(command, ...)
end
"#;

/// A replacement of `vehicle`: its locomotive kept (as part `kept`) and
/// turned, as `loco`; its coach left out, and a new coach of model `coach`.
fn replace_train(vehicle: u32, loco: &str, kept: u32, coach: &str) -> String {
    format!(
        "{{ ReplaceVehicle = {{ vehicle = {vehicle}, \
            consist = {{ \
                {{ part = {{ model = '{loco}', reversed = true, loads = {{}}, \
                            color = {{ r = 1, g = 0, b = 0 }} }}, kept = {kept} }}, \
                {{ part = {{ model = '{coach}', reversed = false, \
                            loads = {{ {{ config = 1, cargo = 0 }} }}, color = {{ r = 0, g = 0, b = 1 }} }} }} }}, \
            groups = {{ 1, 1 }}, multiple_units = {{ '', '' }} }} }}"
    )
}

const LOCO: &str = "vehicle/train/loco.mdl";
const COACH: &str = "vehicle/waggon/coach.mdl";

/// The game script in a room whose first update bound 401 and 402 to
/// vehicles 0 and 1, with `setup` run before the room orders `batch`.
fn replay(setup: &str, batch: &[String]) -> Lua {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    lua.load(FAKE_TRAINS).exec().unwrap();
    lua.load(format!(
        "HOOK.room = true UPDATE({{}}, STATE, 0.2) {setup} \
         HOOK.batch = {{ {} }} UPDATE({{}}, STATE, 0.2)",
        batch.join(", ")
    ))
    .exec()
    .unwrap();
    lua
}

#[test]
fn every_game_replaces_the_vehicle_with_its_own_parts_kept_and_new_ones_bought() {
    let lua = replay("", &[replace_train(0, LOCO, 0, COACH)]);
    let sent: String = lua
        .load(
            "local r = SENT[1].replace local out = { r.vehicle } \
             for _, p in ipairs(r.config.vehicles) do \
                 out[#out + 1] = p.part.modelId .. (p.part.reversed and '<' or '>') .. p.purchaseTime \
                     .. '@' .. tostring(p.maintenanceState) .. '/' .. tostring(p.autoLoadConfig[1]) end \
             out[#out + 1] = r.config.vehicleGroups[2] .. r.config.muFileNames[2] \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        sent, "401 41<1000@0.7/nil 42>777000@nil/true 1",
        "the locomotive keeps its purchase time and wear, turned as the player chose; \
         the coach is bought now"
    );
    let (applied, id, next, logged): (String, u32, u32, Vec<String>) = lua
        .load(
            "local registry = ug_require('tpf3mp_1::/scripts/tpf3mp/registry.lua') \
             local reg = STATE.value.registry local a = HOOK.applied[1] \
             return tostring(a.ok) .. ':' .. tostring(a.entity), registry.id(reg, 'vehicles', 401), \
                 reg.vehicles.next, HOOK.logged",
        )
        .eval()
        .unwrap();
    assert_eq!(applied, "true:401", "the vehicle is itself still");
    assert_eq!((id, next), (0, 2), "it keeps its id; no id is used up");
    assert!(
        logged.contains(&"replacing vehicle 0 (entity 401): 2 part(s), 1 kept".to_owned()),
        "{logged:?}"
    );
}

#[test]
fn a_vehicle_the_game_makes_anew_keeps_its_id() {
    let lua = replay("REPLACE_AS = 450", &[replace_train(0, LOCO, 0, COACH)]);
    let named: String = lua
        .load(
            "local registry = ug_require('tpf3mp_1::/scripts/tpf3mp/registry.lua') \
             local reg = STATE.value.registry \
             return table.concat({ tostring(HOOK.applied[1].entity), registry.id(reg, 'vehicles', 450), \
                 tostring(registry.id(reg, 'vehicles', 401)), registry.id(reg, 'vehicles', 402), \
                 reg.vehicles.next }, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        named, "450 0 nil 1 2",
        "vehicle-0 names the new entity; the old is gone, and no new id is made"
    );
}

#[test]
fn an_unreadable_replacement_result_holds_replay_without_reassigning_vehicle_id() {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    lua.load(FAKE_TRAINS).exec().unwrap();
    lua.load(format!(
        "HOOK.room = true UPDATE({{}}, STATE, 0.2) REPLACE_AS = 450 \
         local original = api.engine.getComponent local unread = true \
         api.engine.getComponent = function(e, kind) \
             if unread and e == 450 and kind == api.type.ComponentType.TRANSPORT_VEHICLE then \
                 unread = false error('replacement component unreadable') end \
             return original(e, kind) end \
         HOOK.replayToken = 'replace' HOOK.replayBatch = {{ {}, \
             {{ SellVehicle = {{ vehicles = {{ 0 }} }} }} }} \
         SCRIPT.handleEvent({{}}, STATE, '', 'tpf3mp', 'replay', 'replace')",
        replace_train(0, LOCO, 0, COACH)
    ))
    .exec()
    .unwrap();
    let (held, reason, old_id, new_id, sent): (bool, String, Option<u32>, Option<u32>, usize) = lua
        .load(
            "local reg = STATE.value.registry \
             local registry = ug_require('tpf3mp_1::/scripts/tpf3mp/registry.lua') \
             return not HOOK.replayDone.ok, tostring(HOOK.replayDone.why), \
                 registry.id(reg, 'vehicles', 401), registry.id(reg, 'vehicles', 450), #SENT",
        )
        .eval()
        .unwrap();
    assert!(
        held,
        "an uncertain post-command result must stop room replay"
    );
    assert!(
        reason.contains("replacement component unreadable"),
        "{reason}"
    );
    assert_eq!(
        (old_id, new_id),
        (Some(0), None),
        "the saved id must not be silently rebound"
    );
    assert_eq!(
        sent, 1,
        "no later ordered command runs after the uncertain result"
    );
}

#[test]
fn a_replacement_that_does_not_fit_this_world_is_applied_nowhere() {
    // Keeping a part of another model, a part the vehicle does not have; a
    // model this game lacks; a vehicle with no id.
    let lua = replay(
        "",
        &[
            replace_train(0, COACH, 0, COACH),
            replace_train(0, LOCO, 5, COACH),
            replace_train(0, LOCO, 0, "vehicle/waggon/tender.mdl"),
            replace_train(9, LOCO, 0, COACH),
        ],
    );
    let (sent, why): (usize, Vec<String>) = lua
        .load(
            "local why = {} for _, a in ipairs(HOOK.applied) do \
                 why[#why + 1] = tostring(a.ok) .. ': ' .. tostring(a.why) end \
             return #SENT, why",
        )
        .eval()
        .unwrap();
    assert_eq!(sent, 0, "nothing sent");
    assert_eq!(
        why,
        [
            "false: part 1 keeps a part of another model",
            "false: part 1 keeps a part the vehicle does not have",
            "false: no vehicle model vehicle/waggon/tender.mdl",
            "false: no vehicles 9 in this world",
        ]
    );
}

/// The construction menu's prospection, as it sends it to the company
/// script (`construction_react_util.tl`, `ProspectionActionRecipe`): coal
/// near town 7, the industry types in the menu's order.
const SPAWN_INDUSTRY: &str = "{ companyEntity = 25, townEntity = 7, \
    types = { 'coal_mine_large', 'coal_mine' }, \
    permitKey = 'game_mechanics/company/explorations/exploration_coal.res', \
    cargoType = '::/cargos/coal/coal.cargo' }";

#[test]
fn a_prospection_goes_to_the_room_by_its_towns_id_and_its_types_in_order() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    // The GUI reads the game script's registry from its state: town 7 is
    // town-3.
    lua.load(
        "api.type = { ComponentType = { GAME_SCRIPT = 7 } } \
         api.engine = { \
             util = { getPlayer = function() return 25 end }, \
             getComponent = function(e, kind) \
                 if kind == 7 and e == 77 then return { state = { registry = { \
                     vehicles = { next = 0, bound = {} }, lines = { next = 0, bound = {} }, \
                     groups = { next = 0, bound = {} }, towns = { next = 4, bound = { { 3, 7 } } }, \
                     industries = { next = 0, bound = {} } } } } end \
             end, \
             system = { gameScriptSystem = { getEntityForGameScript = function(name) \
                 if name == 'tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs' then return 77 end return -1 end } }, \
         } \
         M = mount(loadPlugin()) M.step() HOOK.room = true",
    )
    .exec()
    .unwrap();
    lua.load(format!(
        "UNLOCKED = nil \
         api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Companies', 'spawnIndustry', {SPAWN_INDUSTRY}), \
             function() UNLOCKED = true end) \
         M.step()"
    ))
    .exec()
    .unwrap();
    let (sent, handed, unlocked): (usize, usize, bool) = lua
        .load("return #SENT, #HOOK.commands, UNLOCKED ~= nil")
        .eval()
        .unwrap();
    assert_eq!(sent, 0, "not run here: the room orders it for every game");
    assert_eq!(handed, 1, "handed to the room, through the schema");
    assert!(
        !unlocked,
        "the menu's permits stay reserved until it ran here"
    );
    let prospect: String = lua
        .load(
            "local p = HOOK.commands[1].Prospect \
             return table.concat({ p.town, p.cargo, p.industries[1], p.industries[2], p.permit }, '|')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        prospect,
        "3|::/cargos/coal/coal.cargo|coal_mine_large|coal_mine|\
         game_mechanics/company/explorations/exploration_coal.res"
    );
    // This game applied it: the menu hears so and gives back its reservation.
    lua.load("HOOK.results = { { ticket = 1, ok = true } } M.step()")
        .exec()
        .unwrap();
    assert!(lua.load("return UNLOCKED == true").eval::<bool>().unwrap());

    // A town the room cannot name, another company's prospection, or one
    // that can find nothing, is refused, and says why.
    lua.load(format!(
        "local p = {SPAWN_INDUSTRY} p.townEntity = 8 \
         api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Companies', 'spawnIndustry', p)) \
         local q = {SPAWN_INDUSTRY} q.companyEntity = 26 \
         api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Companies', 'spawnIndustry', q)) \
         local r = {SPAWN_INDUSTRY} r.types = {{}} \
         api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Companies', 'spawnIndustry', r)) \
         M.step()"
    ))
    .exec()
    .unwrap();
    let (sent, handed): (usize, usize) = lua.load("return #SENT, #HOOK.commands").eval().unwrap();
    assert_eq!((sent, handed), (0, 1));
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    for why in [
        "a town the room cannot name",
        "prospecting for another company",
        "a prospection that can find no industry",
    ] {
        assert!(
            logged
                .iter()
                .any(|l| l.contains("makeScriptingSendEventCmd") && l.ends_with(why)),
            "{why}: {logged:?}"
        );
    }
    // Taking a rank goes to the room too (tpf3mp/progression.lua); a perk
    // the capture cannot read stays refused.
    lua.load(
        "api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Companies', 'applyLevel', { level = 2 }))          api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Companies', 'applyLevel', { level = 2.5 }))          api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Companies', 'MakeGreen', {}))",
    )
    .exec()
    .unwrap();
    let (handed, level): (usize, u32) = lua
        .load("return #HOOK.commands, HOOK.commands[2].ApplyRank.level")
        .eval()
        .unwrap();
    assert_eq!((handed, level), (2, 2));
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged.iter().any(|l| l.ends_with("a rank of 2.5")),
        "{logged:?}"
    );
}

/// Towns and industries for the prospecting tests, over the stand-in engine
/// state: towns 7 and 5, and a coal mine whose INDUSTRY part 931 is in
/// construction 930.
const FAKE_TOWNS: &str = r#"
local CT = { CONSTRUCTION = 2, TOWN = 12, INDUSTRY = 13, GAME_TIME = 10 }
api.type.ComponentType = CT
TOWNS, PARTS = { 7, 5 }, { 931 }
CONS = { [930] = { fileName = 'industry/coal_mine.con',
                   transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 1234.5, -250.25, 10, 1 } } }
api.engine.getEntitiesWithComponent = function(kind)
    if kind == CT.TOWN then return TOWNS end
    if kind == CT.INDUSTRY then return PARTS end
    return {}
end
api.engine.getComponent = function(e, kind)
    if kind == CT.CONSTRUCTION then return CONS[e] end
    if kind == CT.TOWN then for _, t in ipairs(TOWNS) do if t == e then return {} end end end
end
api.engine.system = {
    lineSystem = { getLines = function() return {} end },
    streetConnectorSystem = { getConstructionEntityForSubconstruction = function(part)
        if part == 931 then return 930 end
        if part == 941 then return 940 end
        return -1
    end },
}
"#;

#[test]
fn every_game_prospects_through_the_company_scripts_own_event() {
    let (lua, _script) = engine();
    lua.load(FAKE_TOWNS).exec().unwrap();
    // The room's first update binds the towns, lowest entity first: 5 is
    // town-0, 7 town-1. Then the room's prospection near town-1, and one
    // near a town this world has not.
    lua.load(
        "HOOK.room = true UPDATE({}, STATE, 0.2) \
         HOOK.batch = { { Prospect = { town = 1, cargo = '::/cargos/coal/coal.cargo', \
             industries = { 'coal_mine_large', 'coal_mine' }, permit = 'coal.res' } }, \
             { Prospect = { town = 9, cargo = 'c', industries = { 'x' } } } } \
         UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let event: String = lua
        .load(
            "local e = SENT[1].event local p = e.param \
             return table.concat({ e.src, e.id, e.name, p.companyEntity, p.townEntity, \
                 p.types[1], p.types[2], #p.types, p.permitKey, p.cargoType }, '|')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        event,
        "|Companies|spawnIndustry|25|7|coal_mine_large|coal_mine|2|coal.res|::/cargos/coal/coal.cargo",
        "the player's company, town 7, the types in order"
    );
    let applied: String = lua
        .load(
            "local out = {} for _, a in ipairs(HOOK.applied) do \
                 out[#out + 1] = a.i .. ':' .. tostring(a.ok) .. ':' .. tostring(a.why) end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        applied, "1:true:nil 2:false:no towns 9 in this world",
        "a town this world has not is refused, the same in every game"
    );
    assert_eq!(lua.load("return #SENT").eval::<usize>().unwrap(), 1);
    // The industry that stood at the start is industry-0.
    let first: u32 = lua
        .load(
            "return ug_require('tpf3mp_1::/scripts/tpf3mp/registry.lua').id(STATE.value.registry, 'industries', 930)",
        )
        .eval()
        .unwrap();
    assert_eq!(first, 0);
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged.contains(
            &"prospecting for ::/cargos/coal/coal.cargo near town-1 (7): coal_mine_large, coal_mine"
                .to_owned()
        ),
        "{logged:?}"
    );
}

#[test]
fn a_prospection_found_is_said_and_its_industry_named_alike_in_every_game() {
    let (lua, script) = engine();
    lua.load(FAKE_TOWNS).exec().unwrap();
    let handle: Function = script.get("handleEvent").unwrap();
    lua.globals().set("HANDLE", handle).unwrap();
    // Subscribed to the company script's two events.
    lua.load("HOOK.room = true UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    assert!(
        lua.load("return STATE.subscribed.startProspection and STATE.subscribed.endProspection")
            .eval::<bool>()
            .unwrap()
    );
    // As the company script sends them: begun, one found nothing, and one
    // found a new industry (part 941 of construction 940).
    lua.load(
        "HANDLE({}, STATE, '', 'Company', 'startProspection', \
             { entity = 7, initiatedTimestamp = 3600000, cargoType = 'coal' }) \
         HANDLE({}, STATE, '', 'Company', 'endProspection', \
             { entity = { entity = 5, index = 0 }, initiatedTimestamp = 100, cargoType = 'grain', success = false }) \
         PARTS = { 931, 941 } \
         CONS[940] = { fileName = 'industry/coal_mine_large.con', \
                       transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, -40, 80.04, 3, 1 } } \
         HANDLE({}, STATE, '', 'Company', 'endProspection', \
             { entity = { entity = 7, index = 0 }, initiatedTimestamp = 3600000, cargoType = 'coal', success = true }) \
         HANDLE({}, STATE, '', 'Loan', 'endProspection', { success = true })",
    )
    .exec()
    .unwrap();
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    let said: Vec<&String> = logged
        .iter()
        .filter(|l| l.starts_with("prospecting"))
        .collect();
    assert_eq!(
        said,
        [
            "prospecting began: coal near town-1 at game time 3600000",
            "prospecting ended: grain near town-0, begun at game time 100, found nothing",
            "prospecting ended: coal near town-1, begun at game time 3600000, \
             found industry-1 industry/coal_mine_large.con at (-40.0, 80.0)",
        ]
    );
    let named: u32 = lua
        .load(
            "return ug_require('tpf3mp_1::/scripts/tpf3mp/registry.lua').id(STATE.value.registry, 'industries', 940)",
        )
        .eval()
        .unwrap();
    assert_eq!(named, 1, "bound in the saved registry at once");
}

#[test]
fn a_registry_from_an_older_mod_gains_the_towns_at_the_rooms_next_update() {
    let (lua, _script) = engine();
    lua.load(FAKE_TOWNS).exec().unwrap();
    lua.load(
        "STATE.value = { registry = { vehicles = { next = 2, bound = {} }, \
             lines = { next = 0, bound = {} }, groups = { next = 0, bound = {} } } } \
         HOOK.room = true UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let (town, vehicles): (u32, u32) = lua
        .load(
            "local reg = STATE.value.registry \
             return ug_require('tpf3mp_1::/scripts/tpf3mp/registry.lua').id(reg, 'towns', 7), reg.vehicles.next",
        )
        .eval()
        .unwrap();
    assert_eq!((town, vehicles), (1, 2), "the towns bound, the rest kept");
    let work: mlua::Value = lua.load("return UPDATE({}, STATE, 0.2)").eval().unwrap();
    assert!(work.is_nil(), "once");
}

#[test]
fn the_game_script_seeds_math_random_with_the_room_steps_seed_each_update() {
    let (lua, _script) = engine();
    // Two games at the same step draw the same numbers; outside the room's
    // steps nothing is seeded.
    let draws: (f64, f64, f64) = lua
        .load(
            "HOOK.seed = 12345 UPDATE({}, STATE, 0.2) local a = math.random()              math.randomseed(999) math.random()              UPDATE({}, STATE, 0.2) local b = math.random()              HOOK.seed = nil math.randomseed(7) UPDATE({}, STATE, 0.2)              local c = math.random() math.randomseed(7)              return a, b, c - math.random()",
        )
        .eval()
        .unwrap();
    assert_eq!(draws.0, draws.1, "the same step's seed, the same draws");
    assert_eq!(draws.2, 0.0, "no seed: the state's own sequence goes on");
}

/// Pure: the roster of companies (tpf3mp/companies.lua).
#[test]
fn companies_are_founded_joined_renamed_recoloured_and_dissolved_alike() {
    let lua = gui();
    lua.load(
        r#"
        COMP = { [2] = { [700] = { player = 901 }, [701] = { player = 25 }, [702] = { player = -1 } } }
        api = {
            engine = {
                util = { getPlayer = function() return 25 end },
                getComponent = function(e, kind) return COMP[kind] and COMP[kind][e] end,
                -- The entity only, as build 40408 calls the function (seen
                -- in its console: the second argument is nil).
                forEachEntityWithComponent = function(fn, kind)
                    for e in pairs(COMP[kind] or {}) do fn(e) end
                end,
            },
            type = { ComponentType = { NAME = 1, PLAYER_OWNED = 2 },
                     Vec3f = { new = function(x, y, z) return { x, y, z } end } },
            cmd = {
                makeGameAddPlayerCmd = function(name, color) return { add = name, color = color } end,
                makeEntitySetNameCmd = function(e, name) return { rename = e, name = name } end,
            },
        }
        SENT, NEXT = {}, 900
        function send(cmd)
            SENT[#SENT + 1] = cmd
            if cmd.add then NEXT = NEXT + 1 return { resultEntity = NEXT } end
        end
        C = ug_require("tpf3mp_1::/scripts/tpf3mp/companies.lua")
        A, B, D = string.rep("a", 64), string.rep("b", 64), string.rep("d", 64)
        R = C.ensure(nil, api)
        "#,
    )
    .exec()
    .unwrap();
    let eval = |code: &str| -> String {
        lua.load(code)
            .eval::<String>()
            .unwrap_or_else(|error| panic!("{code}: {error}"))
    };
    // Everyone plays for the save's own company until they choose.
    assert_eq!(
        eval("return C.of(R, A).id .. ' ' .. C.of(R, A).entity"),
        "0 25"
    );
    // A founds Rival: a new player entity, the next colour, and A plays for it.
    assert_eq!(
        eval(
            "local ok, why, id = C.run(R, A, { Create = { name = ' Rival ' } }, send, api) \
             return tostring(ok) .. ' ' .. tostring(id)"
        ),
        "true 1"
    );
    assert_eq!(
        eval(
            "local c = C.of(R, A) \
             return c.name .. ' ' .. c.entity .. ' ' .. SENT[1].add .. ' ' .. SENT[1].color[3]"
        ),
        "Rival 901 Rival 0.85"
    );
    assert_eq!(
        eval("return tostring(C.of(R, B).id)"),
        "0",
        "B still plays for the first"
    );
    // B joins Rival: two players in one company, D alone in the first.
    assert_eq!(
        eval("return tostring(C.run(R, B, { Join = 1 }, send, api))"),
        "true"
    );
    assert_eq!(eval("return C.of(R, B).id .. ' ' .. C.of(R, D).id"), "1 0");
    // Only its players rename or recolour a company; names stay unique.
    assert_eq!(
        eval(
            "local ok, why = C.run(R, D, { Rename = { company = 1, name = 'Mine' } }, send, api) \
             return why"
        ),
        "only its players rename a company"
    );
    assert_eq!(
        eval("local ok, why = C.run(R, A, { Create = { name = 'rival' } }, send, api) return why"),
        "a company is called rival already"
    );
    assert_eq!(
        eval(
            "C.run(R, B, { Rename = { company = 1, name = 'Blue Line' } }, send, api) \
             return C.find(R, 1).name .. ' ' .. SENT[#SENT].rename"
        ),
        "Blue Line 901"
    );
    assert_eq!(
        eval(
            "C.run(R, A, { Recolor = { company = 1, color = { r = 0.1, g = 0.2, b = 0.3 } } }, send, api) \
             return tostring(C.find(R, 1).color[2])"
        ),
        "0.2"
    );
    // What another company owns is refused, naming it; its own and no
    // one's are not.
    assert_eq!(
        eval(
            "local ok, why = C.mayTouch(R, 25, 700, api, 'vehicle') \
             return tostring(ok) .. ' ' .. why"
        ),
        "false the vehicle belongs to Blue Line"
    );
    assert_eq!(
        eval(
            "return tostring(C.mayTouch(R, 25, 701, api)) .. tostring(C.mayTouch(R, 25, 702, api)) \
             .. tostring(C.mayTouch(R, 25, 703, api))"
        ),
        "truetruetrue"
    );
    // Its last player dissolves a company that owns nothing, and plays for
    // the first again; nobody dissolves the first.
    assert_eq!(
        eval("local ok, why = C.run(R, D, { Delete = 1 }, send, api) return why"),
        "only its players dissolve a company"
    );
    assert_eq!(
        eval("local ok, why = C.run(R, A, { Delete = 1 }, send, api) return why"),
        "others still play for Blue Line"
    );
    assert_eq!(
        eval("local ok, why = C.run(R, D, { Delete = 0 }, send, api) return why"),
        "the room's first company stays"
    );
    assert_eq!(
        eval(
            "C.run(R, B, { Join = 0 }, send, api) \
             local ok, why = C.run(R, A, { Delete = 1 }, send, api) return why"
        ),
        "Blue Line still owns something"
    );
    assert_eq!(
        eval(
            "COMP[2][700] = nil \
             return tostring(C.run(R, A, { Delete = 1 }, send, api)) .. ' ' .. #C.live(R) \
             .. ' ' .. C.of(R, A).id"
        ),
        "true 1 0"
    );
    assert_eq!(
        eval("local ok, why = C.run(R, A, { Join = 1 }, send, api) return why"),
        "there is no company 1"
    );
    // At most MAX companies.
    assert_eq!(
        eval(
            "for i = 1, C.MAX do C.run(R, A, { Create = { name = 'C' .. i } }, send, api) end \
             return #C.live(R) .. ' ' .. select(2, C.run(R, A, { Create = { name = 'X' } }, send, api))"
        ),
        "8 the room has 8 companies already"
    );
}

/// Pure: who may do what to a company (DECISIONS.md, D22, proposed). A
/// password is the room's seal of it; its head alone locks and unlocks it,
/// sends players out and opens or closes its stations; the head's place
/// passes to the longest-standing player when the founder leaves; the room's
/// first company is everyone's.
#[test]
fn a_companys_head_locks_it_and_only_its_password_opens_it() {
    let lua = gui();
    lua.load(
        r#"
        COMP = { [2] = { [800] = { player = 901 }, [801] = { player = 25 }, [802] = {} } }
        api = {
            engine = {
                util = { getPlayer = function() return 25 end },
                getComponent = function(e, kind) return COMP[kind] and COMP[kind][e] end,
                forEachEntityWithComponent = function(fn, kind)
                    for e in pairs(COMP[kind] or {}) do fn(e) end
                end,
            },
            type = { ComponentType = { NAME = 1, PLAYER_OWNED = 2, TRANSPORT_VEHICLE = 4 },
                     Vec3f = { new = function(x, y, z) return { x, y, z } end } },
            cmd = {
                makeGameAddPlayerCmd = function(name, color) return { add = name } end,
                makeEntitySetColorCmd = function(e, color) return { paint = e } end,
            },
        }
        SENT, NEXT = {}, 900
        function send(cmd)
            SENT[#SENT + 1] = cmd
            if cmd.add then NEXT = NEXT + 1 return { resultEntity = NEXT } end
        end
        C = ug_require("tpf3mp_1::/scripts/tpf3mp/companies.lua")
        JAMES, BOB, CAT = string.rep("a", 64), string.rep("b", 64), string.rep("c", 64)
        R = C.ensure(nil, api)
        function seal(scope, byte) return { scope = scope, tag = string.rep(byte, 64) } end
        function why(player, op, s)
            local ok, reason = C.run(R, player, op, send, api, s)
            return ok and "ok" or reason
        end
        -- James founds Rival (company 1, entity 901).
        C.run(R, JAMES, { Create = { name = 'Rival' } }, send, api)
        "#,
    )
    .exec()
    .unwrap();
    let eval = |code: &str| -> String {
        lua.load(code)
            .eval::<String>()
            .unwrap_or_else(|error| panic!("{code}: {error}"))
    };
    assert_eq!(eval("return C.head(R, 1)"), eval("return JAMES"));
    // Only its head locks it, and only with the room's seal for it.
    assert_eq!(
        eval("return why(BOB, { Lock = 1 }, seal(1, 'e'))"),
        "only the head of Rival gives a password to it"
    );
    assert_eq!(
        eval("return why(JAMES, { Lock = 1 })"),
        "a password for Rival comes sealed by the room"
    );
    assert_eq!(
        eval("return why(JAMES, { Lock = 1 }, seal(2, 'e'))"),
        "a password for Rival comes sealed by the room",
        "a seal made for another company"
    );
    assert_eq!(eval("return why(JAMES, { Lock = 1 }, seal(1, 'e'))"), "ok");
    // Joining it needs the password: none, a wrong one, then the right one.
    assert_eq!(
        eval("return why(BOB, { Join = 1 })"),
        "joining Rival needs its password"
    );
    assert_eq!(
        eval("return why(BOB, { Join = 1 }, seal(1, 'f'))"),
        "the password for Rival is not right"
    );
    assert_eq!(eval("return why(BOB, { Join = 1 }, seal(1, 'e'))"), "ok");
    assert_eq!(eval("return why(CAT, { Join = 1 }, seal(1, 'e'))"), "ok");
    // A player is no head: Bob cannot send Cat out or close the stations.
    assert_eq!(
        eval("return why(BOB, { Dismiss = { company = 1, player = CAT } })"),
        "only the head of Rival sends players out of it"
    );
    assert_eq!(
        eval("return why(BOB, { ShareStations = { company = 1, open = false } })"),
        "only the head of Rival closes the stations of it"
    );
    // The head sends Cat out: she plays for the first company again, and
    // gets back in only with the password.
    assert_eq!(
        eval("return why(JAMES, { Dismiss = { company = 1, player = CAT } })"),
        "ok"
    );
    assert_eq!(eval("return tostring(C.of(R, CAT).id)"), "0");
    assert_eq!(
        eval("return why(JAMES, { Dismiss = { company = 1, player = JAMES } })"),
        "the head leaves by joining another company"
    );
    // James leaves: Bob, who has played for Rival longest, is its head now.
    assert_eq!(eval("return why(JAMES, { Join = 0 })"), "ok");
    assert_eq!(eval("return C.head(R, 1)"), eval("return BOB"));
    assert_eq!(
        eval("return why(BOB, { ShareStations = { company = 1, open = false } })"),
        "ok"
    );
    assert_eq!(eval("return tostring(C.open(C.find(R, 1)))"), "false");
    // The founder returns with the password, and heads it again.
    assert_eq!(eval("return why(JAMES, { Join = 1 }, seal(1, 'e'))"), "ok");
    assert_eq!(eval("return C.head(R, 1)"), eval("return JAMES"));
    assert_eq!(eval("return why(JAMES, { Unlock = 1 })"), "ok");
    assert_eq!(eval("return why(CAT, { Join = 1 })"), "ok");
    // The room's first company is everyone's: no head, no password, and its
    // stations stay open.
    assert_eq!(eval("return tostring(C.head(R, 0))"), "nil");
    assert_eq!(
        eval("return why(JAMES, { Lock = 0 }, seal(0, 'e'))"),
        "the room's first company is everyone's: nobody gives a password to it"
    );
    assert_eq!(
        eval("return why(JAMES, { ShareStations = { company = 0, open = false } })"),
        "the room's first company is everyone's: nobody closes the stations of it"
    );
    // Using a station: no one's, one's own and an open company's are fine;
    // a closed company's is refused, naming it. Using is not changing.
    assert_eq!(
        eval(
            "return tostring(C.mayUse(R, 25, 802, api)) .. tostring(C.mayUse(R, 901, 800, api)) \
             .. tostring(C.mayUse(R, 901, 801, api))"
        ),
        "truetruetrue"
    );
    assert_eq!(
        eval("local ok, why = C.mayUse(R, 25, 800, api) return tostring(ok) .. ' ' .. why"),
        "false the station belongs to Rival, which keeps its stations to itself"
    );
    assert_eq!(
        eval(
            "why(JAMES, { ShareStations = { company = 1, open = true } }) return tostring(C.mayUse(R, 25, 800, api))"
        ),
        "true"
    );
    // Its head's choice for one company wins over the default, either way;
    // none leaves it to the default again (D22, proposed).
    let first = eval("return C.find(R, 0).name");
    assert_eq!(
        eval("return why(JAMES, { StationAccess = { company = 1, other = 0, open = false } })"),
        "ok"
    );
    assert_eq!(
        eval("local ok, why = C.mayUse(R, 25, 800, api) return tostring(ok) .. ' ' .. why"),
        format!("false the station belongs to Rival, which keeps its stations from {first}")
    );
    assert_eq!(
        eval(
            "why(JAMES, { ShareStations = { company = 1, open = false } }) \
             why(JAMES, { StationAccess = { company = 1, other = 0, open = true } }) \
             local let = C.mayUse(R, 25, 800, api) \
             why(JAMES, { StationAccess = { company = 1, other = 0 } }) \
             local default = C.mayUse(R, 25, 800, api) \
             why(JAMES, { ShareStations = { company = 1, open = true } }) \
             return tostring(let) .. '|' .. tostring(default) .. '|' .. tostring(C.find(R, 1).access)"
        ),
        "true|false|nil"
    );
    // Only its head chooses, for another company there is.
    assert_eq!(
        eval("return why(CAT, { StationAccess = { company = 1, other = 0, open = true } })"),
        "only the head of Rival decides whose lines stop at the stations of it"
    );
    assert_eq!(
        eval("return why(JAMES, { StationAccess = { company = 1, other = 1, open = false } })"),
        "Rival's stations are always its own"
    );
    assert_eq!(
        eval("return why(JAMES, { StationAccess = { company = 1, other = 7, open = false } })"),
        "there is no company 7"
    );
    assert_eq!(
        eval("return why(JAMES, { StationAccess = { company = 0, other = 1, open = false } })"),
        "the room's first company is everyone's: nobody decides whose lines stop at the stations of it"
    );
    // A colour is fractions from 0 to 1, and no two companies wear one.
    assert_eq!(
        eval("return why(JAMES, { Recolor = { company = 1, color = { r = 2, g = 0, b = 0 } } })"),
        "a colour is { r, g, b }, each from 0 to 1"
    );
    assert_eq!(
        eval(
            "local first = C.find(R, 0).color \
             return why(JAMES, { Recolor = { company = 1, color = { r = first[1], g = first[2], b = first[3] } } })"
        ),
        "Company wears that colour already"
    );
}

/// Through the game script: a line of one company stops at another's
/// station while that one keeps its stations open, and is refused in every
/// game once they are closed; a join travels with its seal.
#[test]
fn a_line_stops_at_another_companys_station_while_it_is_open() {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    lua.load(
        r#"
        api.type.Line = { new = function() return { vehicleInfo = {} } end,
            Stop = { new = function() return {} end }, StopConfig = { new = function() return {} end } }
        api.type.StationTerminal = { new = function(s, t) return { station = s, terminal = t } end }
        api.cmd.makeLineCreateCmd = function(name, color, player, line)
            return { createLine = { name = name, color = color, player = player, line = line } } end
        JAMES, BOB, CAT = string.rep("a", 64), string.rep("b", 64), string.rep("c", 64)
        -- Station group 90 is Rival's (901); 91 is no company's.
        OWNERS = {}
        local get = api.engine.getComponent
        api.type.ComponentType.PLAYER_OWNED = 55
        api.engine.getComponent = function(e, kind)
            if kind == 55 then return OWNERS[e] and { player = OWNERS[e] } or nil end
            return get(e, kind)
        end
        HOOK.room = true
        HOOK.batch = { { CompanyOp = { Create = { name = 'Rival' } } } } HOOK.origins = { JAMES }
        UPDATE({}, STATE, 0.2)
        RIVAL = STATE.value.companies.list[2]
        HOOK.batch = { { CompanyOp = { Lock = RIVAL.id } } } HOOK.origins = { JAMES }
        HOOK.seals = { { scope = RIVAL.id, tag = string.rep("e", 64) } }
        UPDATE({}, STATE, 0.2)
        HOOK.batch = { { CompanyOp = { Join = RIVAL.id } }, { CompanyOp = { Join = RIVAL.id } } }
        HOOK.origins = { BOB, BOB }
        HOOK.seals = { { scope = RIVAL.id, tag = string.rep("f", 64) }, { scope = RIVAL.id, tag = string.rep("e", 64) } }
        UPDATE({}, STATE, 0.2)
        HOOK.seals = nil
        "#,
    )
    .exec()
    .unwrap();
    let applied: String = lua
        .load(
            "local out = {} for _, a in ipairs(HOOK.applied) do \
                 out[#out + 1] = tostring(a.ok) .. (a.why and (':' .. a.why) or '') end \
             return table.concat(out, ',')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        applied, "true,true,false:the password for Rival is not right,true",
        "founded, locked, a wrong password refused, the right one let in"
    );
    let members: String = lua
        .load("local r = STATE.value.companies return #r.members .. ' ' .. r.members[2].company")
        .eval()
        .unwrap();
    assert_eq!(members, "2 1");
    // hook.log says what became of the companies, and never a seal.
    let logged: String = lua
        .load("return table.concat(HOOK.logged, '|')")
        .eval()
        .unwrap();
    assert!(
        logged.contains(
            "company: Lock by aaaaaaaa (with a password's seal): \
             Company #0 (0 chose it); Rival #1 (1 chose it, head aaaaaaaa, password)"
        ),
        "{logged}"
    );
    assert!(
        logged.contains("was not applied: the password for Rival is not right"),
        "{logged}"
    );
    assert!(!logged.contains("eeeeeeee"), "{logged}");
    // Cat, of the room's first company, runs a line from Rival's station 90
    // to 91: allowed while Rival's stations are open, refused once its head
    // closes them, in every game alike.
    lua.load(
        r#"
        OWNERS[90] = RIVAL.entity
        local registry = ug_require('tpf3mp_1::/scripts/tpf3mp/registry.lua')
        local reg = STATE.value.registry
        local ctx = { group = function(e) return registry.id(reg, 'groups', e) end,
                      line = function(e) return registry.id(reg, 'lines', e) end }
        local function stop(group) return { stationGroup = group, station = 0, terminal = 1,
            alternativeTerminals = {}, loadMode = 0, minWaitingTime = 0, maxWaitingTime = 180,
            maxAdditionalWaitingTime = 0, waypoints = {},
            stopConfig = { load = {}, maxLoad = {}, forceUnload = false,
                destroyForConfigChange = false, destroyForRefresh = false } } end
        LINE_ACTION = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua').lineCreate(ctx, 'Shared',
            { x = 0.8, y = 0.2, z = 0 }, 25, { stops = { stop(90), stop(91) }, customFilters = false,
            reservationPriority = 0, vehicleInfo = { transportModes = { [3] = true } } })
        HOOK.applied = {}
        HOOK.batch = { LINE_ACTION } HOOK.origins = { CAT }
        UPDATE({}, STATE, 0.2)
        HOOK.batch = { { CompanyOp = { ShareStations = { company = RIVAL.id, open = false } } }, LINE_ACTION }
        HOOK.origins = { JAMES, CAT }
        UPDATE({}, STATE, 0.2)
        "#,
    )
    .exec()
    .unwrap();
    let lines: String = lua
        .load(
            "local out = {} for _, a in ipairs(HOOK.applied) do \
                 out[#out + 1] = tostring(a.ok) .. (a.why and (':' .. a.why) or '') end \
             return table.concat(out, ',')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        lines,
        "true,true,false:stop 1: the station belongs to Rival, which keeps its stations to itself"
    );
}

/// Through the game script: a player founds a company, and what they do is
/// booked to it; the roster is kept in the script's state.
#[test]
fn what_a_player_does_is_booked_to_their_company() {
    let (lua, _script) = engine();
    lua.load(
        r#"
        -- A month is 1000 ms of game time here.
        GAME_T = 0
        api.type.ComponentType.GAME_TIME = 99
        api.engine.util.getWorld = function() return 1 end
        api.engine.getComponent = function(e, kind)
            if kind == 99 then return { gameTime = GAME_T } end
        end
        api.util = { getDefaultMonthDuration = function() return 1000 end }
        api.type.JournalEntry = { new = function() return { category = {} } end,
                                  Type = { LOAN = 'LOAN', INTEREST = 'INTEREST' } }
        api.cmd.makeJournalBookAssetCmd = function(e, entry) return { journal = entry, entity = e } end
        A, B = string.rep("a", 64), string.rep("b", 64)
        HOOK.batch = { { CompanyOp = { Create = { name = 'Rival' } } } } HOOK.origins = { A }
        UPDATE({}, STATE, 0.2)
        -- A's loan, for Rival: 1200 over 12 months at 12 % total interest; and B's,
        -- who plays for the first company, through the game's loan script.
        OFFER = { type = 'Small', amount = 1200, duration = 12000, percentage = 0.12 }
        STATE.value.companies.loanOffers = { { company = 1, availableLoans = { OFFER } } }
        HOOK.batch = { { Loan = { Take = { next = OFFER, offer = OFFER } } },
                       { Loan = { Take = { next = OFFER, offer = OFFER } } } }
        HOOK.origins = { A, B }
        UPDATE({}, STATE, 0.2)
        -- A month later, Rival pays its first instalment, with no action,
        -- in the room's game.
        HOOK.room = true
        GAME_T = 1000
        UPDATE({}, STATE, 0.2)
        "#,
    )
    .exec()
    .unwrap();
    let roster: String = lua
        .load(
            "local r = STATE.value.companies local c = r.list[2] \
             return #r.list .. ' ' .. c.name .. ' ' .. c.entity .. ' ' .. r.members[1].company",
        )
        .eval()
        .unwrap();
    assert_eq!(
        roster, "2 Rival 901 1",
        "the roster is saved with the world"
    );
    let sent: String = lua
        .load(
            "local out = {} for _, c in ipairs(SENT) do \
                 out[#out + 1] = c.addPlayer or (c.event and c.event.name) \
                     or (c.journal and (c.journal.category.type .. c.journal.amount .. '@' .. c.entity)) or '?' end \
             return table.concat(out, ',')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        sent, "Rival,LOAN1200@901,Obtain,LOAN-100@901,INTEREST-12@901",
        "the company; Rival's loan booked to it, B's through the loan script; \
         a month later Rival's native instalment of 112: 100 principal, 12 interest"
    );
    let loan: String = lua
        .load("local l = STATE.value.companies.loans[1] return l.remaining .. ' ' .. l.paid .. '/' .. l.months")
        .eval()
        .unwrap();
    assert_eq!(loan, "1100 1/12");
}

/// The GUI's "my company" is the player's: api.engine.util.getPlayer answers
/// the company they play for, in the GUI state only, and the game's own
/// answer for the room's first company.
#[test]
fn the_guis_company_is_the_one_the_player_plays_for() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load(
        r#"
        ME = string.rep("b", 64)
        ROSTER = { next = 2, list = { { id = 0, entity = 25, name = "First", color = { 1, 0, 0 } },
                                      { id = 1, entity = 901, name = "Rival", color = { 0, 0, 1 } } },
                   members = {} }
        -- The game's binding is a callable table (build 40408).
        api.engine = api.engine or {}
        api.engine.util = { getPlayer = setmetatable({}, { __call = function() return 25 end }) }
        api.engine.system = api.engine.system or {}
        api.engine.system.gameScriptSystem = { getEntityForGameScript = function(name)
            if name == "tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs" then return 77 end return -1 end }
        api.type = api.type or {}
        api.type.ComponentType = api.type.ComponentType or {}
        api.type.ComponentType.GAME_SCRIPT = 7
        api.engine.getComponent = function(e, kind)
            if e == 77 and kind == 7 then return { state = { companies = ROSTER } } end
        end
        HOOK.status = { room = "r", players = { { name = "b", id = ME, me = true, connected = true } }, me_id = ME }
        "#,
    )
    .exec()
    .unwrap();
    run_frames(&lua, 20);
    let first: i64 = lua
        .load("return api.engine.util.getPlayer()")
        .eval()
        .unwrap();
    assert_eq!(first, 25, "playing for the first company: the game's own");
    // The hook's probe of the native tools' ownership checks reads the
    // company from a note: none for the room's first company.
    let noted: Option<String> = lua
        .load("return (HOOK.notes or {})['tpf3mp.company']")
        .eval()
        .unwrap();
    assert_eq!(noted, None);
    lua.load("ROSTER.members = { { player = ME, company = 1 } }")
        .exec()
        .unwrap();
    run_frames(&lua, 20);
    let mine: i64 = lua
        .load("return api.engine.util.getPlayer()")
        .eval()
        .unwrap();
    assert_eq!(mine, 901, "playing for Rival: Rival");
    let noted: Option<String> = lua
        .load("return (HOOK.notes or {})['tpf3mp.company']")
        .eval()
        .unwrap();
    assert_eq!(
        noted.as_deref(),
        Some("901"),
        "Rival's entity, for the hook"
    );
    let listed: Option<String> = lua
        .load("return (HOOK.notes or {})['tpf3mp.companies']")
        .eval()
        .unwrap();
    assert_eq!(
        listed.as_deref(),
        Some("25,901"),
        "every company of the room, for the map's icons"
    );
    lua.load("ROSTER.members = {}").exec().unwrap();
    run_frames(&lua, 20);
    let noted: Option<String> = lua
        .load("return (HOOK.notes or {})['tpf3mp.company']")
        .eval()
        .unwrap();
    assert_eq!(noted, None, "back to the first company: forgotten");
    let logged: String = lua
        .load("return table.concat(HOOK.logged, '|')")
        .eval()
        .unwrap();
    assert!(
        logged.contains("the GUI's company follows the player's"),
        "{logged}"
    );
}

/// The Multiplayer window's companies (D22, proposed): the head of the
/// player's company sets its password, opens or closes its stations and
/// sends players out; another player joins a company with a password by
/// typing it, and the password goes to the hook beside the action, never
/// into the window's notes or the log. The line manager offers another
/// company's station while that company keeps its stations open.
#[test]
fn the_window_lets_a_head_lock_the_company_and_others_join_with_its_password() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load(
        r#"
        JAMES, BOB = string.rep("a", 64), string.rep("b", 64)
        ROSTER = { next = 2,
            list = { { id = 0, entity = 25, name = "First", color = { 0.8, 0.16, 0.12 } },
                     { id = 1, entity = 901, name = "Rival", color = { 0.13, 0.42, 0.85 }, founder = JAMES } },
            members = { { player = JAMES, company = 1 }, { player = BOB, company = 1 } } }
        api.engine = api.engine or {}
        api.engine.util = { getPlayer = function() return 25 end }
        api.engine.system = api.engine.system or {}
        api.engine.system.gameScriptSystem = { getEntityForGameScript = function(name)
            if name == "tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs" then return 77 end return -1 end }
        api.type = api.type or {}
        api.type.ComponentType = { GAME_SCRIPT = 7, STATION_GROUP = 9, CONSTRUCTION = 2, PLAYER_OWNED = 55 }
        api.type.Vec3f = { new = function(x, y, z) return { x = x, y = y, z = z } end }
        -- Station group 90 is Rival's.
        api.engine.getComponent = function(e, kind)
            if e == 77 and kind == 7 then return { state = { companies = ROSTER } } end
            if e == 90 and kind == 9 then return {} end
            if e == 90 and kind == 55 then return { player = 901 } end
        end
        HOOK.room = true
        function as(me)
            HOOK.status = { room = "r", me_id = me, players = {
                { name = "james", id = JAMES, me = me == JAMES, connected = true, owner = true },
                { name = "bob", id = BOB, me = me == BOB, connected = true } } }
        end
        as(JAMES)
        BAR = mount(loadPlugin())
        for _ = 1, 20 do BAR.step() end
        BAR.render()
        views(BAR.layout)[1].params.onClick()
        function window() WINDOWS.Tpf3mpWindow.step() return WINDOWS.Tpf3mpWindow.render() end
        function find(view, pick)
            for _, v in ipairs(views(window())) do
                if v.view == view and pick(v.params) then return v.params end
            end
        end
        function button(label)
            return find("Button", function(p) return p.content and p.content.params.text == label end)
        end
        function texts()
            local out = {}
            for _, v in ipairs(views(window())) do
                if v.view == 'TextView' then out[#out + 1] = v.params.text end
            end
            return table.concat(out, "\n")
        end
        function last() return HOOK.commands[#HOOK.commands], HOOK.passwords[#HOOK.commands] end
        "#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let eval = |code: &str| -> String {
        lua.load(code)
            .eval::<String>()
            .unwrap_or_else(|error| panic!("{code}: {error}\n{}", log(&lua)))
    };
    assert!(
        eval("return texts()")
            .contains("Rival\nYour company · head: james\nPlayers  james (you), bob"),
        "{}",
        eval("return texts()")
    );
    // Each card shows the money the game's own windows show
    // (getPlayersBalance), not an ACCOUNT that reads 0 for the room's first
    // company; Rival's, where the game gives none, its ACCOUNT's.
    let shown = eval(
        "api.type.ComponentType.ACCOUNT = 6 \
         local get = api.engine.getComponent \
         api.engine.getComponent = function(e, kind) \
             if kind == 6 then return { balance = e == 901 and 5000 or 0, loan = 0 } end \
             return get(e, kind) end \
         api.engine.util.finance = { getPlayersBalance = function(e) \
             if e == 25 then return 1234567 end end } \
         for _ = 1, 20 do BAR.step() end \
         local shown = texts() \
         api.engine.getComponent, api.engine.util.finance = get, nil \
         for _ = 1, 20 do BAR.step() end \
         return shown",
    );
    assert!(shown.contains("$1,234,567"), "{shown}");
    assert!(shown.contains("$5,000"), "{shown}");
    assert!(!shown.contains("$0"), "{shown}");
    // The head types a password: it goes beside the action, and the field
    // hides it.
    assert_eq!(
        eval(
            "local f = find('TextInputField', function(p) return p.passwordMode end) \
             f.onValueChange('s3cret') \
             local action, password = last() \
             return f.placeholderText .. '|' .. action.CompanyOp.Lock .. '|' .. password"
        ),
        "A password to join|1|s3cret"
    );
    assert!(!eval("return texts()").contains("s3cret"));
    // The head chooses who stops at Rival's stations: by default, and for
    // the first company on its own; and sends Bob out.
    // (The label wraps its line.)
    let shown = eval("return texts()").replace('\n', " ");
    assert!(
        shown.contains("Stations, by default and for companies founded later: allowed"),
        "{shown}"
    );
    let first = eval("return ROSTER.list[1].name");
    assert!(
        shown.contains(&format!("{first}: allowed (default)")),
        "{shown}"
    );
    assert_eq!(
        eval(
            "button('Deny by default').onClick() \
             local close = last().CompanyOp.ShareStations \
             button('Deny').onClick() \
             local deny = last().CompanyOp.StationAccess \
             button('Send out').onClick() \
             local out = last().CompanyOp.Dismiss \
             return tostring(close.open) .. '|' .. deny.company .. '>' .. deny.other .. '=' \
                 .. tostring(deny.open) .. '|' .. out.player"
        ),
        format!("false|1>0=false|{}", "b".repeat(64))
    );
    // Once the room has it, the row says so and offers the default back.
    assert_eq!(
        eval(&format!(
            "ROSTER.list[2].access = {{ {{ company = 0, open = false }} }} \
             for _ = 1, 20 do BAR.step() end \
             local shown = texts():find('{first}: denied', 1, true) ~= nil \
             button('Default').onClick() \
             local back = last().CompanyOp.StationAccess \
             ROSTER.list[2].access = nil \
             return tostring(shown) .. '|' .. tostring(back.open) .. '|' .. back.other"
        )),
        "true|nil|0"
    );
    // James plays for Rival: closing it to others must keep its own
    // station selectable. The foreign-company case follows below.
    assert_eq!(
        eval(
            "local util = ug_require('/scripts/entity_util.tl') \
             local open = util.isOwnedByPlayerOrNotOwned(90) \
             ROSTER.list[2].closed = true \
             for _ = 1, 20 do BAR.step() end \
             return tostring(open) .. '|' .. tostring(util.isOwnedByPlayerOrNotOwned(90))"
        ),
        "true|true"
    );
    // With James in the first company: Rival's choice for it wins over
    // its default, either way.
    assert_eq!(
        eval(
            "local util = ug_require('/scripts/entity_util.tl') \
             ROSTER.members = { { player = BOB, company = 1 } } \
             ROSTER.list[2].access = { { company = 0, open = true } } \
             for _ = 1, 20 do BAR.step() end \
             local let = util.isOwnedByPlayerOrNotOwned(90) \
             ROSTER.list[2].closed = nil \
             ROSTER.list[2].access = { { company = 0, open = false } } \
             for _ = 1, 20 do BAR.step() end \
             local kept = util.isOwnedByPlayerOrNotOwned(90) \
             ROSTER.list[2].closed = true ROSTER.list[2].access = nil \
             ROSTER.members = { { player = JAMES, company = 1 }, { player = BOB, company = 1 } } \
             return tostring(let) .. '|' .. tostring(kept)"
        ),
        "true|false"
    );
    // Bob, of the first company now, joins Rival, which has a password.
    assert_eq!(
        eval(
            "ROSTER.list[2].lock = { scope = 1, tag = string.rep('e', 64) } \
             ROSTER.members = { { player = JAMES, company = 1 } } \
             as(BOB) for _ = 1, 20 do BAR.step() end \
             local n = #HOOK.commands \
             button('Join').onClick() \
             local refused = #HOOK.commands == n \
             local f = find('TextInputField', function(p) return p.passwordMode end) \
             f.onTyping('s3cret') \
             button('Join').onClick() \
             local action, password = last() \
             return tostring(refused) .. '|' .. action.CompanyOp.Join .. '|' .. password"
        ),
        "true|1|s3cret"
    );
    let shown = eval("return texts()");
    assert!(
        shown.contains("Rival\nhead: james · password · stations\nclosed\nPlayers  james"),
        "{shown}"
    );
    assert!(shown.contains("Joining Rival..."), "{shown}");
    assert!(!shown.contains("s3cret"), "{shown}");
    let logged = eval("return table.concat(HOOK.logged, '|')");
    assert!(!logged.contains("s3cret"), "{logged}");
    assert!(
        logged.contains("the line manager offers other companies' open stations"),
        "{logged}"
    );
}

/// The game's company window renames the player's company by its player
/// entity: that goes to the room as the company's rename.
#[test]
fn the_company_windows_rename_goes_to_the_room_as_the_companys() {
    let lua = gui();
    lua.load("C = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua')")
        .exec()
        .unwrap();
    let (company, line): (String, String) = lua
        .load(
            "local ctx = { company = function(e) if e == 901 then return 1 end end, \
                           line = function(e) if e == 600 then return 4 end end } \
             local a = C.setName(ctx, 901, 'Blue Line') \
             local b = C.setName(ctx, 600, 'North') \
             return a.CompanyOp.Rename.company .. ' ' .. a.CompanyOp.Rename.name, \
                    b.EditLine.line .. ' ' .. b.EditLine.change.Rename",
        )
        .eval()
        .unwrap();
    assert_eq!(company, "1 Blue Line");
    assert_eq!(line, "4 North");
}

/// With more than one company, vehicles wear their company's colour: a new
/// colour repaints the company's fleet, and only its own.
#[test]
fn a_companys_colour_repaints_its_vehicles() {
    let lua = gui();
    lua.load(
        r#"
        COMP = { [2] = { [500] = { player = 901 }, [501] = { player = 25 }, [502] = { player = 901 } },
                 [4] = { [500] = {}, [501] = {}, [502] = {} } }
        api = {
            engine = {
                util = { getPlayer = function() return 25 end },
                getComponent = function(e, kind) return COMP[kind] and COMP[kind][e] end,
                forEachEntityWithComponent = function(fn, kind)
                    local keys = {}
                    for e in pairs(COMP[kind] or {}) do keys[#keys + 1] = e end
                    table.sort(keys)
                    for _, e in ipairs(keys) do fn(e) end
                end,
            },
            type = { ComponentType = { NAME = 1, PLAYER_OWNED = 2, TRANSPORT_VEHICLE = 4 },
                     Vec3f = { new = function(x, y, z) return { x, y, z } end } },
            cmd = {
                makeGameAddPlayerCmd = function(name, color) return { add = name } end,
                makeEntitySetColorCmd = function(e, color) return { paint = e, color = color } end,
            },
        }
        SENT, NEXT = {}, 900
        function send(cmd)
            SENT[#SENT + 1] = cmd
            if cmd.add then NEXT = NEXT + 1 return { resultEntity = NEXT } end
        end
        C = ug_require("tpf3mp_1::/scripts/tpf3mp/companies.lua")
        A = string.rep("a", 64)
        R = C.ensure(nil, api)
        "#,
    )
    .exec()
    .unwrap();
    let painting: bool = lua.load("return C.painting(R)").eval().unwrap();
    assert!(!painting, "one company: the game's own colours");
    let painted: String = lua
        .load(
            "C.run(R, A, { Create = { name = 'Rival' } }, send, api) \
             SENT = {} \
             C.run(R, A, { Recolor = { company = 1, color = { r = 0, g = 0.5, b = 1 } } }, send, api) \
             local out = {} for _, c in ipairs(SENT) do out[#out + 1] = c.paint .. ':' .. c.color[2] end \
             return tostring(C.painting(R)) .. ' ' .. table.concat(out, ',')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        painted, "true 500:0.5,502:0.5",
        "Rival's two vehicles, not the first company's"
    );
}

/// A vehicle's marker on the map wears its company's colour: the mod
/// replaces the game's marker recipe with one that, while the room has more
/// than one company, puts the marker of a vehicle painted in a company
/// colour in that colour's class (the room paints a company's vehicles so),
/// as TPF2's vehicle icons followed the vehicle's paint, and leaves every
/// other marker as the game made it.
#[test]
fn a_vehicles_marker_wears_its_companys_colour() {
    let lua = gui();
    lua.load(
        r#"
        -- Vehicles by their first part's colour, as build 40408 gives it (a
        -- float's digits; -1 for the model's own colours).
        local function painted(x, y, z)
            return { transportVehicleConfig = { vehicles = { { part = { color = { x = x, y = y, z = z } } } } } }
        end
        COMP = { [4] = {
            [500] = painted(0.12999999523163, 0.41999998688698, 0.85000002384186), -- blue
            [501] = painted(0.80000001192093, 0.15999999642372, 0.11999999731779), -- red
            [502] = painted(0.9, 0.9, 0.9),                                        -- a player's own
            [503] = painted(-1, -1, -1),                                           -- unpainted
        } }
        -- The roster is in the mod's game script's state (entity 77).
        ROSTER = nil
        api = {
            engine = {
                getComponent = function(e, kind)
                    if kind == 7 then return e == 77 and { state = { companies = ROSTER } } or nil end
                    return COMP[kind] and COMP[kind][e]
                end,
                system = { gameScriptSystem = { getEntityForGameScript = function(name)
                    return name == "tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs" and 77 or -1
                end } },
            },
            type = { ComponentType = { TRANSPORT_VEHICLE = 4, GAME_SCRIPT = 7 } },
        }
        CLOCK = 0
        os.clock = function() return CLOCK end
        local script = "gui/tpf3mp/company_markers.script.lua"
        assert(loadstring(mod_source(script), "@" .. script))()
        local exported = data()
        local toolbox = ug_require("::/gui/main/hud_icon_toolbox.tl")
        exported.replace({ ReplaceRecipe = function(original, replacement)
            assert(original == toolbox.HudIconMasterGame, "replaces the game's marker")
            MARKER = replacement
        end })
        C = ug_require("tpf3mp_1::/scripts/tpf3mp/companies.lua")
        -- The game script's companies, a while later (the game gives a new
        -- table on each read).
        function companies(list) ROSTER = { list = list, members = {} } CLOCK = CLOCK + 5 end
        -- The HUD takes only a layout from a marker's recipe (build 40408:
        -- "Recipe child must be a layout"), so the game's marker is always
        -- inside one.
        function marker(entity)
            local node = mount(MARKER, { entity = entity }).layout
            assert(node.layout == "BoxLayout", "a marker's recipe gives a layout")
            local inner = node.params.children[1]
            assert(#node.params.children == 1 and inner.view == "Marker", "around the game's marker")
            local class = node.params.meta and node.params.meta.class
            return (class and (class .. " around ") or "") .. "game's " .. inner.params.entity
        end
        "#,
    )
    .exec()
    .unwrap();
    let marker = |entity: i64| -> String {
        lua.load(format!("return marker({entity})"))
            .eval()
            .unwrap_or_else(|error| panic!("{entity}: {error}"))
    };
    // No roster yet, and one company: the game's markers.
    assert_eq!(marker(500), "game's 500");
    lua.load("companies({ { id = 0, entity = 25, color = C.PALETTE[1] } })")
        .exec()
        .unwrap();
    assert_eq!(
        marker(500),
        "game's 500",
        "one company: as in single player"
    );
    // Two companies: a vehicle in a company colour wears it.
    lua.load(
        "companies({ { id = 0, entity = 25, color = C.PALETTE[1] },                      { id = 1, entity = 901, color = C.PALETTE[2] } })",
    )
    .exec()
    .unwrap();
    assert_eq!(marker(500), "tpf3mp-company-2 around game's 500");
    assert_eq!(marker(501), "tpf3mp-company-1 around game's 501");
    // Any other paint, no paint, and what is no vehicle: the game's.
    assert_eq!(marker(502), "game's 502");
    assert_eq!(marker(503), "game's 503");
    assert_eq!(marker(600), "game's 600");
    // The roster is read again only every two seconds: a company dissolved
    // shows once it is read, back to the game's markers.
    lua.load(
        "ROSTER = { members = {}, list = { ROSTER.list[1],                     { id = 1, entity = 901, color = C.PALETTE[2], gone = true } } }",
    )
    .exec()
    .unwrap();
    assert_eq!(marker(500), "tpf3mp-company-2 around game's 500");
    lua.load("CLOCK = CLOCK + 2").exec().unwrap();
    assert_eq!(marker(500), "game's 500");

    // The style sheet has a class for every colour of the palette.
    let classes: String = lua
        .load(
            r#"
            local rules = {}
            local ssu = { makeAdder = function(result)
                return function(selector, style) result[#result + 1] = selector end
            end }
            local real = require
            require = function(path)
                if path == "::/gui/main/stylesheetutil.lua" then return ssu end
                return ug_require(path)
            end
            local css = "gui/tpf3mp/tpf3mp.css.lua"
            assert(loadstring(mod_source(css), "@" .. css))()
            local result = data()
            require = real
            return table.concat(result, "|")
            "#,
        )
        .eval()
        .unwrap();
    for i in 1..=8 {
        assert!(
            classes.contains(&format!("!tpf3mp-company-{i} VehicleItem::Icon")),
            "{classes}"
        );
        // And for every company's capital, in place of the game's blue.
        assert!(
            classes.contains(&format!("!tpf3mp-capital-{i} R::TownHudIcon!capital-city")),
            "{classes}"
        );
        assert!(
            classes.contains(&format!(
                "!tpf3mp-capital-{i} TextView!tpf3mp-capital-label"
            )),
            "{classes}"
        );
    }
}

/// Every company's capital on the map, for every player: with more than
/// one company, the game's town label crowns each live company's
/// headquarters town (the town closest to the headquarters its PLAYER
/// names), the viewer's own in the game's blue (`capital-city`), another
/// company's in its colour's class, with a line naming whose capital it is;
/// two companies in one town are both named, a company without a
/// headquarters crowns nothing. With one company, the game's own label.
#[test]
fn every_companys_capital_is_crowned_for_every_player() {
    let lua = gui();
    lua.load(
        r#"
        -- Companies' PLAYER components: the headquarters each names, and
        -- each headquarters' closest town.
        PLAYERS = {
            [25] = { headquarters = 701 },  -- Company, in 31
            [901] = { headquarters = 702 }, -- Rival, in 32
            [902] = { headquarters = 703 }, -- Pals, in 32 too
            [903] = { headquarters = -1 },  -- Late: none yet
            [904] = { headquarters = 704 }, -- Odd, in 34, an own colour
        }
        CLOSEST = { [701] = 31, [702] = 32, [703] = 32, [704] = 34 }
        ME = 901
        ROSTER = nil
        api = {
            engine = {
                getComponent = function(e, kind)
                    if kind == 7 then return e == 77 and { state = { companies = ROSTER } } or nil end
                    if kind == 5 then return PLAYERS[e] end
                end,
                util = { getPlayer = function() return ME end },
                system = {
                    gameScriptSystem = { getEntityForGameScript = function(name)
                        return name == "tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs" and 77 or -1
                    end },
                    streetConnectorSystem = { getConstructionClosestTown = function(e)
                        return CLOSEST[e] or -1
                    end },
                },
            },
            type = { ComponentType = { PLAYER = 5, GAME_SCRIPT = 7 } },
        }
        CLOCK = 0
        os.clock = function() return CLOCK end
        -- The game's isCapital (game_mechanics/towns/town_util.tl): the
        -- town closest to getPlayer()'s headquarters. The game loads it by
        -- two names, one table.
        local town_util = {}
        function town_util.isCapital(town)
            local p = api.engine.getComponent(api.engine.util.getPlayer(), 5)
            if p and p.headquarters and p.headquarters >= 0 then
                local t = api.engine.system.streetConnectorSystem.getConstructionClosestTown(p.headquarters)
                if t >= 0 and t == town then return true end
            end
            return false
        end
        -- The game's town label (gui/main/town_hud_react_util.tl): a crown
        -- and `capital-city` for what isCapital says.
        local react = ug_require("::/gui/main/react.lua")
        react.setMouseTransparent = react.setMouseTransparent or function() end
        local town_hud = {}
        town_hud.TownHudIcon = react.RegisterRecipe("TownHudIcon", function(params)
            return { view = "Town", params = { entity = params.entity,
                capital = town_util.isCapital(params.entity) } }
        end)
        GAME_MODULES = {
            ["::/gui/main/town_hud_react_util.tl"] = town_hud,
            ["::/game_mechanics/towns/town_util.tl"] = town_util,
            ["/game_mechanics/towns/town_util.tl"] = town_util,
        }
        local script = "gui/tpf3mp/capitals.script.lua"
        assert(loadstring(mod_source(script), "@" .. script))()
        local exported = data()
        exported.replace({ ReplaceRecipe = function(original, replacement)
            assert(original == town_hud.TownHudIcon, "replaces the game's town label")
            LABEL = replacement
        end })
        C = ug_require("tpf3mp_1::/scripts/tpf3mp/companies.lua")
        function companies(list) ROSTER = { list = list, members = {} } CLOCK = CLOCK + 10 end
        -- A town's label as the HUD gets it: always a layout around the
        -- game's; "<class> | <game's crown?> | <line under it>".
        function town(entity)
            local node = mount(LABEL, { entity = entity }).layout
            assert(node.layout == "BoxLayout", "a town label's recipe gives a layout")
            local children = node.params.children
            local inner = children[1]
            assert(inner.view == "Town" and inner.params.entity == entity, "around the game's label")
            local meta = node.params.meta
            for k in pairs(meta or {}) do assert(k == "class", "a wrapper's meta has a class only") end
            local line = children[2]
            assert(#children <= 2 and (line == nil or line.view == "TextView"))
            return ((meta and meta.class) or "") .. " | " .. (inner.params.capital and "crown" or "-")
                .. " | " .. (line and line.params.text or "")
        end
        "#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let town = |entity: i64| -> String {
        lua.load(format!("return town({entity})"))
            .eval()
            .unwrap_or_else(|error| panic!("{entity}: {error}\n{}", log(&lua)))
    };
    let exec = |code: &str| {
        lua.load(code)
            .exec()
            .unwrap_or_else(|error| panic!("{code}: {error}"))
    };

    // No roster yet: the game's label, its own capital only.
    assert_eq!(town(32), " | crown | ");
    assert_eq!(town(31), " | - | ");
    // One company (co-op): as in single player.
    exec("ME = 25 companies({ { id = 0, entity = 25, name = 'Company', color = C.PALETTE[1] } })");
    assert_eq!(town(31), " | crown | ");
    assert_eq!(town(32), " | - | ");

    // Five companies; the viewer plays for Rival.
    exec(
        "ME = 901 companies({ \
           { id = 0, entity = 25, name = 'Company', color = C.PALETTE[1] }, \
           { id = 1, entity = 901, name = 'Rival', color = C.PALETTE[2] }, \
           { id = 2, entity = 902, name = 'Pals', color = C.PALETTE[3] }, \
           { id = 3, entity = 903, name = 'Late', color = C.PALETTE[4] }, \
           { id = 4, entity = 904, name = 'Odd', color = { 0.6, 0.3, 0.8 } } })",
    );
    // Another company's capital: crowned, in its colour, named.
    assert_eq!(town(31), "tpf3mp-capital-1 | crown | Capital of Company");
    // The viewer's own, shared with Pals: the game's blue, both named.
    assert_eq!(town(32), " | crown | Capital of Rival and Pals");
    // A colour of a company's own choosing: the palette's nearest.
    assert_eq!(town(34), "tpf3mp-capital-5 | crown | Capital of Odd");
    // No company's capital, and Late without a headquarters: nothing.
    assert_eq!(town(33), " | - | ");
    let logged = log(&lua);
    assert!(
        logged.contains(
            "[tpf3mp] capitals: the game's isCapital crowns every company's capital (1 town_util table(s))"
        ),
        "{logged}"
    );

    // Another player, of the first company, sees the same capitals, its
    // own in blue and Rival's in Rival's colour, named first.
    exec("ME = 25");
    assert_eq!(town(31), " | crown | Capital of Company");
    assert_eq!(
        town(32),
        "tpf3mp-capital-2 | crown | Capital of Rival and Pals"
    );

    // Whose capital is where is read again only every ten seconds: Late's
    // new headquarters shows once it is read.
    exec("PLAYERS[903] = { headquarters = 705 } CLOSEST[705] = 33 CLOCK = CLOCK + 5");
    assert_eq!(town(33), " | - | ");
    exec("CLOCK = CLOCK + 5");
    assert_eq!(town(33), "tpf3mp-capital-4 | crown | Capital of Late");

    // Back to one company: the game's own again.
    exec(
        "companies({ ROSTER.list[1], \
           { id = 1, entity = 901, name = 'Rival', color = C.PALETTE[2], gone = true } })",
    );
    assert_eq!(town(31), " | crown | ");
    assert_eq!(town(32), " | - | ");
}

/// A world for the companies' progression (tpf3mp/progression.lua), over the
/// stand-in engine state: towns 5, 7 and 9 of 400, 1000 and 50 people; lines
/// 301 of the save's player (25), 302 and 303 of the company founded next
/// (901), 304 of a player the room has not; the game's delivery and
/// passenger statistics per line, the towns' ratings, and the game's own
/// modules. A game month is 4000 ms of game time: a sample every 1000.
/// REVERSED lists everything the other way round, as another game's hash
/// tables might.
const FAKE_PROGRESSION: &str = r#"
GAME_T = 0
local CT = api.type.ComponentType
CT.GAME_TIME, CT.PLAYER_OWNED, CT.TOWN = 99, 98, 12
api.engine.util.getWorld = function() return 1 end
OWNERS = { [301] = 25, [302] = 901, [303] = 901, [304] = 555 }
api.engine.getComponent = function(e, kind)
    if kind == CT.GAME_TIME then return { gameTime = GAME_T } end
    if kind == CT.PLAYER_OWNED then
        local o = OWNERS[e]
        if o then return { player = o } end
        return nil
    end
    if kind == CT.TOWN and (e == 5 or e == 7 or e == 9) then return {} end
end
api.engine.getEntitiesWithComponent = function(kind)
    if kind == CT.TOWN then return { 5, 7, 9 } end
    return {}
end
api.util = { getDefaultMonthDuration = function() return 4000 end,
             getDefaultYearDuration = function() return 48000 end }
local function keyed(pairsList)
    local t = {}
    local from, to, by = 1, #pairsList, 1
    if REVERSED then from, to, by = #pairsList, 1, -1 end
    for i = from, to, by do t[pairsList[i][1]] = pairsList[i][2] end
    return t
end
local function listed(list)
    if not REVERSED then return list end
    local out = {}
    for i = #list, 1, -1 do out[#out + 1] = list[i] end
    return out
end
api.engine.system.townBuildingSystem = { getTown2personCapacitiesMap = function()
    return keyed({ { 7, { 1000, 0 } }, { 5, { 400, 0 } }, { 9, { 50, 0 } } })
end }
DELIVERIES = function(town)
    if town == 7 then return keyed({ { 301, { [-1] = { 2, 60 } } }, { 302, { [-1] = { 0, 40 } } } }) end
    if town == 5 then return keyed({ { 302, { [-1] = { 0, 10 } } }, { 304, { [-1] = { 0, 99 } } } }) end
    return {}
end
HAPPY = function(town)
    if town == 7 then
        return listed({ { 301, { resident = { 1, 20 }, nonResident = { 0, 0 } } },
                        { 303, { resident = { 0, 10 }, nonResident = { 5, 10 } } } })
    end
    return {}
end
api.engine.util.town = {
    getTownDeliveriesStats = function(town, interval, perLine, perCargo)
        ASKED = { interval, perLine, perCargo }
        return DELIVERIES(town)
    end,
    getTownHappinessStats = function(town, lines) return { byLine = HAPPY(town) } end,
}
TOWN_STATES = { townStates = listed({
    { townEntity = { entity = 7 }, authorityScore = 0.7, cachedRatings = {
        urban_care = { value = 0.9 }, traffic_congestion = { value = 0.8 },
        noise = { value = 1 }, pollution = { value = 1 }, people_happiness = { value = 0.1 } } },
    { townEntity = { entity = 5 }, authorityScore = 0.5 },
    { townEntity = { entity = 9 }, authorityScore = 1 },
}) }
OWN = { [25] = { experience = 1500, level = 1, potentialLevel = 7 } }
GAME_MODULES = {
    ['/game_mechanics/company/company_progression_util.tl'] = {
        getLevelAndFraction = function(base, exp) return math.floor(exp / base), 0 end,
        getCompanyProgressionState = function(e) return OWN[e] end,
    },
    ['/game_mechanics/towns/town_util.tl'] = {
        getRatingSensitivity = function(state, key) return 1 end,
        externalGetTownsState = function() return TOWN_STATES end,
    },
    ['/game_mechanics/company/company_util.tl'] = { getBasePopulation = function() return 200 end },
}
A, B = string.rep('a', 64), string.rep('b', 64)
"#;

/// The progression lines of the hook's log.
fn progression_log(lua: &Lua) -> Vec<String> {
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    logged
        .into_iter()
        .filter(|l| l.starts_with("progression"))
        .collect()
}

/// A room of two companies after its first sample: A founded Rival (901),
/// B plays for the room's first company (25).
fn two_companies(reversed: bool) -> Lua {
    let (lua, _script) = engine();
    lua.load(FAKE_PROGRESSION).exec().unwrap();
    lua.globals().set("REVERSED", reversed).unwrap();
    lua.load(
        "HOOK.room = true \
         HOOK.batch = { { CompanyOp = { Create = { name = 'Rival' } } } } HOOK.origins = { A } \
         UPDATE({}, STATE, 0.2) \
         UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    lua
}

#[test]
fn with_two_companies_each_takes_its_share_of_every_town_by_its_deliveries_and_rating() {
    let lua = two_companies(false);
    let records: String = lua
        .load(
            "local out = {} for _, r in ipairs(STATE.value.progression.records) do \
                 out[#out + 1] = r.company .. ':' .. r.entity .. ':' .. r.experience .. ':' \
                     .. r.potential .. ':' .. r.level end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    // Town 5: Rival carried all its cargo; rating the town's 50 (its own
    // parts are perfect): 400 x 1 x 50 / 100 = 200. Town 7: cargo 60 to 40,
    // passengers 20 to 20, so shares 0.55 and 0.45; the first company's
    // rating is the town's 80 (its passengers' happiness 93, its cargo on
    // time 95): 440; Rival's is its unhappy passengers' 64.29: 289.29.
    // Rival: 489. The first company keeps the game's own 1500, which it
    // earned before the room had two companies. Ranks: experience / 200.
    assert_eq!(records, "0:25:1500:7:1 1:901:489:2:1");
    let log = progression_log(&lua);
    assert_eq!(log.len(), 5, "{log:?}");
    assert_eq!(
        log[0],
        "progression at game time 0: 3 towns, weights cargo 1 passengers 1"
    );
    assert_eq!(
        log[1],
        "progression at game time 0: town-0 population 400: company-1 share 1.0000 \
         rating 50.0000 part 200.0000"
    );
    assert!(
        log[2].starts_with(
            "progression at game time 0: town-1 population 1000: company-0 share 0.5500 \
             rating 80.0000 part 440.0000, company-1 share 0.4500 rating 64.28"
        ),
        "{log:?}"
    );
    assert_eq!(
        log[3],
        "progression at game time 0: company-0 score 440.0000, experience 1500, rank 7 reached, 1 taken, prior experience 1500, sample quarter 0"
    );
    assert!(
        log[4].starts_with("progression at game time 0: company-1 score 489.28")
            && log[4].ends_with(
                ", experience 489, rank 2 reached, 1 taken, prior experience 0, sample quarter 0"
            ),
        "{log:?}"
    );
    let loaded: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        loaded
            .iter()
            .any(|line| line.starts_with("room ranks loaded: quarter=")),
        "{loaded:?}"
    );
    assert!(
        loaded
            .iter()
            .any(|line| line.starts_with("room ranks loaded: company-0")),
        "{loaded:?}"
    );
    // The game's delivery statistics per line, over half a year.
    let asked: String = lua
        .load(
            "return table.concat({ tostring(ASKED[1]), tostring(ASKED[2]), tostring(ASKED[3]) }, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(asked, "24000 true false");
    // The next sample waits for the next quarter of a month.
    lua.load("UPDATE({}, STATE, 0.2) GAME_T = 999 UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    assert_eq!(progression_log(&lua).len(), 5);
    lua.load("GAME_T = 1000 UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    assert_eq!(progression_log(&lua).len(), 10);
}

#[test]
fn every_game_scores_the_companies_alike_however_its_tables_are_ordered() {
    let run = |reversed: bool| {
        let lua = two_companies(reversed);
        lua.load("for _ = 1, 5 do GAME_T = GAME_T + 1000 UPDATE({}, STATE, 0.2) end")
            .exec()
            .unwrap();
        let records: String = lua
            .load(
                "local out = {} for _, r in ipairs(STATE.value.progression.records) do \
                     out[#out + 1] = r.company .. ':' .. r.experience .. ':' .. r.potential end \
                 for _, p in ipairs(STATE.value.progression.passengers) do \
                     out[#out + 1] = p.company .. '@' .. p.town .. '=' .. string.format('%.17g', p.value) end \
                 return table.concat(out, ' ')",
            )
            .eval()
            .unwrap();
        (records, progression_log(&lua))
    };
    let (a, log_a) = run(false);
    let (b, log_b) = run(true);
    assert_eq!(a, b);
    assert_eq!(log_a, log_b);
    assert_eq!(log_a.len(), 30, "six samples of five lines");
}

#[test]
fn with_two_companies_a_company_takes_the_ranks_it_reached() {
    let lua = two_companies(false);
    lua.load(
        "SENT = {} HOOK.applied = {} \
         HOOK.batch = { { ApplyRank = { level = 2 } }, { ApplyRank = { level = 3 } }, \
                        { ApplyRank = { level = 2 } }, { ApplyRank = { level = 5 } } } \
         HOOK.origins = { A, A, B, B } \
         UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let applied: String = lua
        .load(
            "local out = {} for _, a in ipairs(HOOK.applied) do \
                 out[#out + 1] = a.i .. ':' .. tostring(a.ok) .. ':' .. tostring(a.why) end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        applied, "1:true:nil 2:false:Rival has reached rank 2, not 3 3:true:nil 4:true:nil",
        "Rival takes rank 2 and not 3; the first company takes 2, then 5"
    );
    let levels: String = lua
        .load(
            "local out = {} for _, r in ipairs(STATE.value.progression.records) do \
                 out[#out + 1] = r.company .. ':' .. r.level end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(levels, "0:5 1:2");
    // The room's first company takes its ranks in the game's own state too,
    // for when the room is one company again; Rival's are the room's alone.
    let events: String = lua
        .load(
            "local out = {} for _, c in ipairs(SENT) do if c.event then \
                 out[#out + 1] = c.event.id .. '.' .. c.event.name .. '=' .. c.event.param.level end end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(events, "Companies.applyLevel=2 Companies.applyLevel=5");
}

#[test]
fn with_one_company_the_game_keeps_its_own_score_and_takes_its_own_ranks() {
    let (lua, _script) = engine();
    lua.load(FAKE_PROGRESSION).exec().unwrap();
    lua.load(
        "HOOK.room = true UPDATE({}, STATE, 0.2) \
         for _ = 1, 3 do GAME_T = GAME_T + 1000 UPDATE({}, STATE, 0.2) end \
         OWN[25] = { experience = 1500, level = 2, potentialLevel = 4 } \
         SENT = {} HOOK.applied = {} \
         HOOK.batch = { { ApplyRank = { level = 3 } }, { ApplyRank = { level = 5 } }, \
                        { ApplyRank = { level = 2 } } } \
         HOOK.origins = { A, A, B } \
         UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    assert!(
        progression_log(&lua).is_empty(),
        "no score of the room's own"
    );
    let none: bool = lua
        .load("return STATE.value.progression.records[1] == nil")
        .eval()
        .unwrap();
    assert!(none);
    let applied: String = lua
        .load(
            "local out = {} for _, a in ipairs(HOOK.applied) do \
                 out[#out + 1] = a.i .. ':' .. tostring(a.ok) .. ':' .. tostring(a.why) end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        applied,
        "1:true:nil 2:false:the company has reached rank 4, not 5 \
         3:false:the company has rank 2 already"
    );
    let events: String = lua
        .load(
            "local out = {} for _, c in ipairs(SENT) do \
                 out[#out + 1] = c.event.src .. '|' .. c.event.id .. '|' .. c.event.name .. '|' .. c.event.param.level end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        events, "|Companies|applyLevel|3",
        "the growth script's own event, as the company window sends it"
    );
}

#[test]
fn a_sample_the_game_cannot_measure_changes_no_rank() {
    let (lua, _script) = engine();
    lua.load(FAKE_PROGRESSION).exec().unwrap();
    lua.load(
        "SAVED_TOWN_UTIL = GAME_MODULES['/game_mechanics/towns/town_util.tl'] \
         GAME_MODULES['/game_mechanics/towns/town_util.tl'] = nil \
         HOOK.room = true \
         HOOK.batch = { { CompanyOp = { Create = { name = 'Rival' } } } } HOOK.origins = { A } \
         UPDATE({}, STATE, 0.2) UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged.iter().any(|l| l.starts_with(
            "the companies' scores were not sampled: /game_mechanics/towns/town_util.tl did not load"
        )),
        "{logged:?}"
    );
    let none: bool = lua
        .load("return STATE.value.progression.records[1] == nil")
        .eval()
        .unwrap();
    assert!(none, "nothing guessed");
    let quarter: Option<i64> = lua
        .load("return STATE.value.progression.quarter")
        .eval()
        .unwrap();
    assert_eq!(
        quarter, None,
        "a failed sample must remain due this quarter"
    );
    lua.load("GAME_MODULES['/game_mechanics/towns/town_util.tl'] = SAVED_TOWN_UTIL UPDATE({}, STATE, 0.2)")
        .exec().unwrap();
    let recovered: String = lua.load(
        "local p = STATE.value.progression return tostring(p.quarter) .. ':' .. tostring(#p.records)"
    ).eval().unwrap();
    assert_eq!(
        recovered, "0:2",
        "the repaired module lets the same quarter earn XP"
    );
}

#[test]
fn a_failed_tally_keeps_saved_progression_and_retries_the_same_quarter() {
    let lua = two_companies(false);
    let state = "local p = STATE.value.progression \
        return tostring(p.quarter) .. ':' .. tostring(#p.records) .. ':' .. tostring(#p.passengers) \
            .. ':' .. tostring(p.passengers[1].value)";
    let before: String = lua.load(state).eval().unwrap();
    lua.load(
        "GAME_T = 1000 \
         SAVED_SENSITIVITY = GAME_MODULES['/game_mechanics/towns/town_util.tl'].getRatingSensitivity \
         GAME_MODULES['/game_mechanics/towns/town_util.tl'].getRatingSensitivity = function() error('temporary town read failure') end \
         UPDATE({}, STATE, 0.2)",
    ).exec().unwrap();
    let after_failure: String = lua.load(state).eval().unwrap();
    assert_eq!(
        after_failure, before,
        "a partial tally must not change persisted state"
    );
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged.iter().any(
            |line| line.starts_with("the companies' scores were not sampled:")
                && line.contains("temporary town read failure")
        ),
        "{logged:?}"
    );
    lua.load(
        "GAME_MODULES['/game_mechanics/towns/town_util.tl'].getRatingSensitivity = SAVED_SENSITIVITY \
         UPDATE({}, STATE, 0.2)",
    ).exec().unwrap();
    let quarter: i64 = lua
        .load("return STATE.value.progression.quarter")
        .eval()
        .unwrap();
    assert_eq!(
        quarter, 1,
        "the same quarter succeeds after the read recovers"
    );
}

#[test]
fn the_rules_formulas_are_the_games() {
    let lua = gui();
    let out: String = lua
        .load(
            "local p = ug_require('tpf3mp_1::/scripts/tpf3mp/progression.lua') \
             local w = { cargo = 1, passengers = 1 } \
             return table.concat({ \
                 string.format('%.4f', p.happiness(1, 20, 1)), \
                 string.format('%.4f', p.happiness(0, 0, 1)), \
                 string.format('%.4f', p.happiness(9, 10, 0)), \
                 string.format('%.4f', p.onTime(5, 10, 1)), \
                 string.format('%.4f', p.onTime(1, 2, 1)), \
                 string.format('%.4f', p.share(60, 100, 1, 2, w)), \
                 string.format('%.4f', p.share(0, 0, 1, 4, w)), \
                 tostring(p.share(5, 0, 0, 0, w)), \
                 string.format('%.4f', p.share(60, 100, 1, 2, { cargo = 3, passengers = 1 })), \
                 string.format('%.1f', p.part(1000, 0.5, 80)) }, ' ')",
        )
        .eval()
        .unwrap();
    // The game's own: happiness and on-time cargo map 0.3..1 to 0..1 at
    // sensitivity 1, and fewer than 15 people or 10 items count as that many.
    assert_eq!(
        out,
        "0.9286 1.0000 1.0000 0.2857 0.8571 0.5500 0.2500 nil 0.5750 400.0"
    );
}

#[test]
fn the_company_window_reads_each_companys_own_rank_with_two_companies() {
    let lua = gui();
    let out: String = lua
        .load(
            "GAME_MODULES = { ['/game_mechanics/company/company_progression_util.tl'] = { \
                 getCompanyProgressionState = function(e) return { level = 9, potentialLevel = 9, experience = e } end } } \
             local p = ug_require('tpf3mp_1::/scripts/tpf3mp/progression.lua') \
             local roster = { next = 2, members = {}, list = { { id = 0, entity = 25, name = 'First' }, \
                 { id = 1, entity = 901, name = 'Rival' } } } \
             STATEV = { companies = roster, progression = { records = { \
                 { company = 0, entity = 25, experience = 1500, potential = 7, level = 3 }, \
                 { company = 1, entity = 901, experience = 489, potential = 2, level = 1 } } } } \
             assert(p.follow(function() return STATEV end)) \
             local util = GAME_MODULES['/game_mechanics/company/company_progression_util.tl'] \
             local function show(e) local s = util.getCompanyProgressionState(e) \
                 return s.level .. '/' .. s.potentialLevel .. '/' .. s.experience end \
             local two = show(25) .. ' ' .. show(901) .. ' ' .. show(555) \
             roster.list[2].gone = true \
             return two .. ' | ' .. show(25) .. ' ' .. show(901)",
        )
        .eval()
        .unwrap();
    assert_eq!(
        out, "3/7/1500 1/2/489 9/9/555 | 9/9/25 9/9/901",
        "each company's own with two; the game's own with one"
    );
}

/// A notification's popup plays its first sound and tells the game's
/// Notifications script (its `initialSound` event): in the room's game that
/// goes to the room, and every game's script marks the same notification.
#[test]
fn a_notifications_first_sound_is_marked_in_every_game() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load("M = mount(loadPlugin()) M.step() HOOK.room = true")
        .exec()
        .unwrap();
    lua.load(
        "api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Notifications', 'initialSound', \
             { notificationId = 12 })) \
         api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Notifications', 'initialSound', \
             { notificationId = 1.5 })) \
         M.step()",
    )
    .exec()
    .unwrap();
    let (sent, handed, seen): (usize, usize, i64) = lua
        .load("return #SENT, #HOOK.commands, HOOK.commands[1].NotificationSeen.notification")
        .eval()
        .unwrap();
    assert_eq!(sent, 0, "not run here: the room orders it for every game");
    assert_eq!(handed, 1, "the whole-numbered one, through the schema");
    assert_eq!(seen, 12);

    let (lua, _script) = engine();
    lua.load(
        "HOOK.room = true UPDATE({}, STATE, 0.2) \
         HOOK.batch = { { NotificationSeen = { notification = 12 } } } \
         UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let event: String = lua
        .load("local e = SENT[1].event return table.concat({ e.src, e.id, e.name, e.param.notificationId }, '|')")
        .eval()
        .unwrap();
    assert_eq!(event, "|Notifications|initialSound|12");
}

/// Native ownership components are userdata, not Lua tables.
#[test]
fn native_userdata_ownership_keeps_station_and_asset_permissions() {
    let lua = gui();
    lua.load(
        r#"
        local C = ug_require('tpf3mp_1::/scripts/tpf3mp/companies.lua')
        local owner = newproxy(true)
        getmetatable(owner).__index = { player = 901 }
        local api = { type = { ComponentType = { PLAYER_OWNED = 1 } },
            engine = { getComponent = function(e) if e == 100 then return owner end end } }
        local roster = { list = {
            { id = 0, entity = 25, name = 'First' },
            { id = 1, entity = 901, name = 'Rival', closed = true },
        }, members = {} }
        assert(C.ownerOf(api, 100) == 901)
        assert(not C.mayTouch(roster, 25, 100, api))
        assert(not C.mayUse(roster, 25, 100, api))
        assert(C.mayUse(roster, 901, 100, api))
        roster.list[2].access = { { company = 0, open = true } }
        assert(C.mayUse(roster, 25, 100, api))
        assert(not C.mayTouch(roster, 25, 100, api))
    "#,
    )
    .exec()
    .unwrap();
}

#[test]
fn station_selection_recovers_native_edge_details_without_opening_foreign_assets() {
    let lua = gui();
    lua.load(r#"
        local C = ug_require('tpf3mp_1::/scripts/tpf3mp/companies.lua')
        local roster = { list = {
            { id = 0, entity = 25, name = 'First' },
            { id = 1, entity = 901, name = 'Rival', closed = true },
        }, members = {} }
        local active = true
        local CT = { PLAYER_OWNED = 1, STATION_GROUP = 2, STATION = 3, CONSTRUCTION = 4 }
        local api = { type = { ComponentType = CT }, engine = {
            getComponent = function(e, kind)
                if kind == CT.PLAYER_OWNED and (e == 100 or e == 101) then return { player = 901 } end
                if e == 100 and kind == CT.STATION_GROUP then return { stations = {101} } end
                if e == 101 and kind == CT.STATION then return { terminals = {} } end
            end,
            system = { stationGroupSystem = { getStationGroup = function(e) assert(e == 101) return 100 end } },
        } }
        local edge = { transportNetworkEdge = {} }
        local terminal = { station = { stationGroup = 100, terminalIndex1 = 2 } }
        local line = { convertDetails = function(e, details)
            if details then return details end
            return { station = { stationGroup = 100, station = e } }
        end }
        local util = { isOwnedByPlayerOrNotOwned = function() return false end }
        local function require_(path)
            if path == '/gui/line_vehicle_mgmt/line_util.tl' then return line end
            return util
        end
        local guiLoads = 0
        local nativeRequire = require_
        require_ = function(path)
            if path == '/gui/line_vehicle_mgmt/line_util.tl' then guiLoads = guiLoads + 1 end
            return nativeRequire(path)
        end
        C.followStations(api, require_, function() if active then return roster, 'me' end end)
        assert(guiLoads == 0, 'React recipes are HUD-only')
        assert(line.convertDetails(101, edge) == edge, 'game-script GUI must not load or wrap React modules')
        C.followStations(api, require_, function() if active then return roster, 'me' end end, true)
        C.followStations(api, require_, function() if active then return roster, 'me' end end, true)
        assert(line.convertDetails(101, edge) == nil, 'closed foreign station')
        roster.list[2].access = { { company = 0, open = true } }
        assert(line.convertDetails(101, edge).station.stationGroup == 100)
        assert(line.convertDetails(100, edge).station.stationGroup == 100)
        assert(line.convertDetails(101, terminal) == terminal, 'preserve chosen terminal')
        assert(line.convertDetails(200, edge) == edge, 'preserve actual network edge')
        assert(not C.mayTouch(roster, 25, 101, api), 'no editing permission')
        roster.list[2].access = nil
        assert(line.convertDetails(101, edge) == nil, 'reset follows closed default')
        roster.members = { { player = 'me', company = 1 } }
        assert(line.convertDetails(101, edge).station.stationGroup == 100, 'own closed station')
        active = false
        assert(line.convertDetails(101, edge) == edge, 'outside room unchanged')
    "#).exec().unwrap();
}

#[test]
fn the_huds_state_follows_the_players_company() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(
        r#"
        ME = string.rep("b", 64)
        ROSTER = { next = 2, list = { { id = 0, entity = 25, name = "First", color = { 1, 0, 0 } },
                                      { id = 1, entity = 901, name = "Rival", closed = true, color = { 0, 0, 1 } } },
                   members = {} }
        api = api or {}
        api.engine = { util = { getPlayer = setmetatable({}, { __call = function() return 25 end }) },
                       system = { gameScriptSystem = { getEntityForGameScript = function(name)
                           return name == "tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs" and 77 or -1 end } },
                       getComponent = function(e, kind)
                           if e == 77 and kind == 7 then return { state = { companies = ROSTER } } end
                           if (e == 100 or e == 101) and kind == 8 then return {} end
                           if (e == 100 or e == 101 or e == 102) and kind == 9 then return { player = 901 } end
                       end }
        api.type = { ComponentType = { GAME_SCRIPT = 7, STATION_GROUP = 8, PLAYER_OWNED = 9, CONSTRUCTION = 10 } }
        HOOK.status = { room = "r", players = { { name = "b", id = ME, me = true, connected = true } }, me_id = ME }
        CLOCK = 0
        os.clock = function() return CLOCK end
        local script = "gui/tpf3mp/gui_state.script.lua"
        assert(loadstring(mod_source(script), "@" .. script))()
        GAME_UTIL = { getActionParams = function() return {} end }
        ENTITY_UTIL = { isOwnedByPlayerOrNotOwned = function() return false end }
        local real = ug_require
        ug_require = function(path)
            if path == "::/gui/construction/construction_react_util.tl" then return GAME_UTIL end
            if path == "::/scripts/entity_util.tl" or path == "/scripts/entity_util.tl" then return ENTITY_UTIL end
            return real(path)
        end
        data().prepare({})
        ug_require = real
        "#,
    )
    .exec()
    .unwrap();
    let first: i64 = lua
        .load("return api.engine.util.getPlayer()")
        .eval()
        .unwrap();
    assert_eq!(first, 25, "playing for the first company: the game's own");
    lua.load(r#"
        HOOK.room = true
        assert(not ENTITY_UTIL.isOwnedByPlayerOrNotOwned(100), 'foreign closed station must be denied')
        ROSTER.list[2].access = { { company = 0, open = true } }
        assert(ENTITY_UTIL.isOwnedByPlayerOrNotOwned(100), 'explicit permission must reach the HUD')
        assert(not ENTITY_UTIL.isOwnedByPlayerOrNotOwned(102), 'permission must not grant foreign assets')
        ROSTER.list[2].access = {}
        assert(not ENTITY_UTIL.isOwnedByPlayerOrNotOwned(100), 'reset must restore the default')
    "#).exec().unwrap();
    lua.load(
        "ROSTER = { list = ROSTER.list, members = { { player = ME, company = 1 } } } CLOCK = 3",
    )
    .exec()
    .unwrap();
    let mine: i64 = lua
        .load("return api.engine.util.getPlayer()")
        .eval()
        .unwrap();
    assert_eq!(mine, 901, "playing for Rival: Rival");
    lua.load("assert(ENTITY_UTIL.isOwnedByPlayerOrNotOwned(100), 'own closed station must remain selectable'); HOOK.room = false; assert(not ENTITY_UTIL.isOwnedByPlayerOrNotOwned(100), 'outside a room the original predicate must apply')").exec().unwrap();
    let logged: String = lua
        .load("return table.concat(HOOK.logged, '|')")
        .eval()
        .unwrap();
    assert!(
        logged.contains("the GUI's company follows the player's in the HUD's state")
            && logged.contains("the stop tool's stop is noted"),
        "{logged}"
    );
}

/// With the hook's player probe on (its note `tpf3mp.probe`), each GUI
/// state says which lines the game's LineViewer is handed to draw and what
/// getLinesForPlayer answers, each line with its owner, once per answer;
/// with the probe off it says nothing. What is drawn never changes.
#[test]
fn the_map_line_probe_says_what_the_line_viewer_draws() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(
        r#"
        ME = string.rep("b", 64)
        OWNERS = { [700] = 901, [701] = 25 }
        api = api or {}
        api.engine = { util = { getPlayer = setmetatable({}, { __call = function() return 25 end }) },
                       system = { gameScriptSystem = { getEntityForGameScript = function() return -1 end },
                                  lineSystem = { getLinesForPlayer = function(p)
                                      if p == 901 then return { 700 } end return { 701 } end,
                                      getLineStopsForTerminal = function(station, terminal)
                                          if station == 900 and terminal == 1 then return { { 700, 0 } } end return {} end,
                                      getProblemLines = function() return { { 700, 3 } } end } },
                       getComponent = function(e, kind)
                           if kind == 9 then return (OWNERS[e] or e == 901) and { player = OWNERS[e] or 1 } or nil end
                           if kind == 10 and e == 700 then
                               return { stops = { { stationGroup = 800, station = 0, terminal = 1 },
                                                  { stationGroup = 800, station = 1, terminal = 0 } } }
                           end
                           if kind == 11 and e == 800 then return { stations = { 900 } } end
                           if kind == 12 and e == 900 then return { terminals = { {}, {} } } end
                           if kind == 13 and e == 700 then return { color = { x = 0.5, y = 0.25, z = 1 } } end
                       end }
        api.type = { ComponentType = { GAME_SCRIPT = 7, PLAYER_OWNED = 9, LINE = 10, STATION_GROUP = 11, STATION = 12, COLOR = 13 } }
        DRAWN = {}
        local builtin = ug_require("::/gui/main/builtin.lua")
        builtin.LineViewer = function(params) DRAWN[#DRAWN + 1] = params return {} end
        local real = ug_require
        ug_require = function(path)
            if path == "::/gui/construction/construction_react_util.tl" then return { getActionParams = function() return {} end } end
            return real(path)
        end
        HOOK.status = { room = "r", players = { { name = "b", id = ME, me = true, connected = true } }, me_id = ME }
        HOOK.notes = { ["tpf3mp.company"] = "901" }
        CLOCK = 0
        os.clock = function() return CLOCK end
        local script = "gui/tpf3mp/gui_state.script.lua"
        assert(loadstring(mod_source(script), "@" .. script))()
        data().prepare({})
        ug_require = real
        "#,
    )
    .exec()
    .unwrap();
    let draw = "local lines = api.engine.system.lineSystem.getLinesForPlayer(api.engine.util.getPlayer())                 ug_require('::/gui/main/builtin.lua').LineViewer({ showLines = { { entity = lines[1], transparency = 0.5 } } })                 return #DRAWN";
    // The probe off: drawn, nothing said.
    let drawn: i64 = lua.load(draw).eval().unwrap();
    assert_eq!(drawn, 1, "the game's LineViewer still draws");
    let logged: String = lua
        .load("return table.concat(HOOK.logged, '|')")
        .eval()
        .unwrap();
    assert!(!logged.contains("probe: a line viewer"), "{logged}");
    // The probe on: both said, once.
    lua.load("HOOK.notes['tpf3mp.probe'] = '1' CLOCK = 5")
        .exec()
        .unwrap();
    for _ in 0..3 {
        let _: i64 = lua.load(draw).eval().unwrap();
    }
    let logged: String = lua
        .load("return table.concat(HOOK.logged, '|')")
        .eval()
        .unwrap();
    assert!(
        logged.contains(
            "probe: getLinesForPlayer(901) answers 1 line(s): 700 (owned by 901) (the HUD's state)"
        ),
        "{logged}"
    );
    assert!(
        logged.contains(
            "probe: a line viewer is handed 1 line(s) to draw: 700 (owned by 901) (the HUD's state)"
        ),
        "{logged}"
    );
    assert_eq!(
        logged.matches("probe: a line viewer").count(),
        1,
        "once per answer"
    );
    // Each line's stops and the engine's verdict on it.
    assert!(
        logged.contains(
            "probe: line to draw: line 700 owned by 901; colour 0.500 0.250 1.000; 2 stop(s); stop 1: group 800 station 0 terminal 1, group of 1 station(s) owned by nil, station 900 owned by nil with 2 terminal(s), listed at the terminal; stop 2: group 800 station 1 terminal 0, group of 1 station(s) owned by nil, no station 1 in the group; line system problem 3; handed at transparency 0.5 (the HUD's state)"
        ),
        "{logged}"
    );
    // The line's owner: the component types it has, and its colour.
    assert!(
        logged
            .contains("probe: the line's owner 901 has PLAYER_OWNED; no colour (the HUD's state)"),
        "{logged}"
    );
}

/// A purchase names its depot in hook.log: the entity the store passed, its
/// owner, and the construction and index the room names it by, so a
/// vehicle that leaves another depot than the player meant shows which one
/// the store chose.
#[test]
fn a_purchases_depot_is_said() {
    let lua = Lua::new();
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../mod/tpf3mp_1/content/scripts/tpf3mp/capture.lua"
    ))
    .unwrap();
    let capture: Table = lua.load(&source).eval().unwrap();
    let text: Function = capture.get("depotText").unwrap();
    let place = lua.create_table().unwrap();
    place
        .set("file", "::/depots/road/road_depot/road_depot.con")
        .unwrap();
    let at = lua.create_table().unwrap();
    at.set("x", 1360.7).unwrap();
    at.set("y", -8829.4).unwrap();
    at.set("z", 7.2).unwrap();
    place.set("at", at).unwrap();
    let said: String = text
        .call((5001, 372_426, place, 0, mlua::Value::Nil))
        .unwrap();
    assert_eq!(
        said,
        "the store buys at depot entity 5001 (owned by 372426): depot 0 of ::/depots/road/road_depot/road_depot.con at (1360.7, -8829.4, 7.2)"
    );
    let refused: String = text
        .call((
            5002,
            mlua::Value::Nil,
            mlua::Value::Nil,
            mlua::Value::Nil,
            "a depot no construction lists",
        ))
        .unwrap();
    assert_eq!(
        refused,
        "the store buys at depot entity 5002 (owned by no one), which the room cannot name: a depot no construction lists"
    );
}

/// The GUI's views decide "mine" with the game's ownership tests
/// (scripts/entity_util.tl) and getPlayer. When a state's api is made anew
/// after the mod's scripts ran, the tests put the company back in front
/// before they answer; and where the room's roster cannot be read, the
/// company the Multiplayer plugin's state notes for the hook answers.
#[test]
fn the_guis_ownership_tests_follow_the_company_after_a_new_api() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(
        r#"
        ME = string.rep("b", 64)
        -- No roster readable in this state: only the note says the company.
        local function freshApi()
            return { engine = { util = { getPlayer = setmetatable({}, { __call = function() return 25 end }) },
                                system = { gameScriptSystem = { getEntityForGameScript = function() return -1 end } },
                                getComponent = function(e, kind)
                                    if kind == 9 then return { player = OWNERS[e] } end
                                end },
                     type = { ComponentType = { GAME_SCRIPT = 7, PLAYER_OWNED = 9 } } }
        end
        FRESH = freshApi
        OWNERS = { [500] = 901, [501] = 25 }
        api = freshApi()
        -- The game's entity_util, as build 40408 has it.
        ENTITY_UTIL = {}
        function ENTITY_UTIL.getPlayerOwned(e) return api.engine.getComponent(e, api.type.ComponentType.PLAYER_OWNED) end
        function ENTITY_UTIL.isOwnedByPlayer(e)
            local o = ENTITY_UTIL.getPlayerOwned(e) return o ~= nil and api.engine.util.getPlayer() == o.player end
        function ENTITY_UTIL.isOwnedByPlayerOrNotOwned(e)
            local o = ENTITY_UTIL.getPlayerOwned(e) return o == nil or api.engine.util.getPlayer() == o.player end
        local real = ug_require
        ug_require = function(path)
            if path == "/scripts/entity_util.tl" or path == "::/scripts/entity_util.tl" then return ENTITY_UTIL end
            if path == "::/gui/construction/construction_react_util.tl" then return { getActionParams = function() return {} end } end
            return real(path)
        end
        HOOK.status = { room = "r", players = { { name = "b", id = ME, me = true, connected = true } }, me_id = ME }
        HOOK.notes = { ["tpf3mp.company"] = "901" }
        CLOCK = 0
        os.clock = function() return CLOCK end
        local script = "gui/tpf3mp/gui_state.script.lua"
        assert(loadstring(mod_source(script), "@" .. script))()
        data().prepare({})
        "#,
    )
    .exec()
    .unwrap();
    let answer = |lua: &mlua::Lua| -> (i64, bool, bool) {
        lua.load(
            "return api.engine.util.getPlayer(), ENTITY_UTIL.isOwnedByPlayer(500), ENTITY_UTIL.isOwnedByPlayer(501)",
        )
        .eval()
        .unwrap()
    };
    assert_eq!(
        answer(&lua),
        (901, true, false),
        "the note's company: its station is mine, the first company's is not"
    );
    // The state is given a new api: the game's own getPlayer again, until
    // a window asks an ownership test.
    lua.load("api = FRESH()").exec().unwrap();
    let mine: bool = lua
        .load("return ENTITY_UTIL.isOwnedByPlayerOrNotOwned(500)")
        .eval()
        .unwrap();
    assert!(mine, "the test put the company back before it answered");
    assert_eq!(answer(&lua), (901, true, false));
    let logged: String = lua
        .load("return table.concat(HOOK.logged, '|')")
        .eval()
        .unwrap();
    assert!(
        logged.contains("the GUI's getPlayer answers the player's company 901 (the HUD's state)")
            && logged.contains("was the game's own again"),
        "{logged}"
    );
    // The room's first company, or outside the room: the game's own.
    lua.load("HOOK.notes['tpf3mp.company'] = nil")
        .exec()
        .unwrap();
    assert_eq!(answer(&lua), (25, false, true));
}

/// The GUI's other Lua state, where the game renders its React recipes (the
/// vehicle store among them), has an api.cmd of its own (docs/COVERAGE.md,
/// U1): in the room's game the guard is on it too. What the room carries
/// goes to the room; what it does not is refused, the callback told so. The
/// room's answers reach the plugin's state, which passes on those to this
/// state's commands through a note (hudguard.forward): a sale is answered,
/// a new line opens, and a vehicle the store bought is told it and put on
/// its line (2026-10-01: refusing the buy here blocked buying).
#[test]
fn in_the_huds_state_the_guard_carries_or_refuses_every_command() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load(
        r#"
        api.cmd.makeVehicleSellCmd = function(vehicles) return { kind = 'sell' } end
        api.cmd.makeTownCreateCmd = function() return { kind = 'town' } end
        api.cmd.makeVehicleSetLineCmd = function(vehicle, line, stop) return { kind = 'setLine' } end
        STATE = { companies = { next = 1, list = { { id = 0, entity = 25, name = "First" } }, members = {} },
                  registry = { vehicles = { bound = { { 3, 5 } } }, lines = { bound = {} } } }
        api.engine = { util = { getPlayer = setmetatable({}, { __call = function() return 25 end }) },
                       system = {
                           gameScriptSystem = { getEntityForGameScript = function(name)
                               return name == "tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs" and 77 or -1 end },
                           streetConnectorSystem = { getConstructionEntityForDepot = function(d)
                               if d == 202 then return 201 end return -1 end },
                       },
                       getComponent = function(e, kind)
                           if e == 77 and kind == 7 then return { state = STATE } end
                           if e == 201 and kind == 2 then return { fileName = 'depot/bus_depot.con',
                               depots = { 202 }, transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 600,10,2,1 } } end
                       end }
        api.type = { ComponentType = { GAME_SCRIPT = 7, CONSTRUCTION = 2 } }
        api.res = { modelRep = { getName = function(id) if id == 41 then return 'vehicle/bus/city.mdl' end end } }
        HOOK.status = { room = "r", players = {}, me_id = string.rep("b", 64) }
        CLOCK = 0
        os.clock = function() return CLOCK end
        local script = "gui/tpf3mp/gui_state.script.lua"
        assert(loadstring(mod_source(script), "@" .. script))()
        GAME_UTIL = { getActionParams = function() return {} end }
        local real = ug_require
        ug_require = function(path)
            if path == "::/gui/construction/construction_react_util.tl" then return GAME_UTIL end
            return real(path)
        end
        data().prepare({})
        ug_require = real
        -- A frame of the HUD's: the clock moves, the HUD asks whose it is.
        function FRAME() CLOCK = CLOCK + 1; api.engine.util.getPlayer() end
        -- The plugin's state reading the room's answers, as its frame does.
        LINK = ug_require('tpf3mp_1::/scripts/tpf3mp/bridge.lua').attach(tpf3mp_native)
        HUD = ug_require('tpf3mp_1::/scripts/tpf3mp/hudguard.lua')
        function ANSWER(results) return HUD.forward(LINK, results) end
        "#,
    )
    .exec()
    .unwrap();
    let logged: String = lua
        .load("return table.concat(HOOK.logged, '|')")
        .eval()
        .unwrap();
    assert!(
        logged.contains("the guard is on 7 command factories in the HUD's state|"),
        "{logged}"
    );

    // Outside the room's game every command is sent.
    lua.load("api.cmd.sendCommand(api.cmd.makeTownCreateCmd())")
        .exec()
        .unwrap();
    assert_eq!(lua.load("return #SENT").eval::<usize>().unwrap(), 1);

    // In it, a town is refused, its callback told on a later frame.
    lua.load(
        "HOOK.room = true \
         api.cmd.sendCommand(api.cmd.makeTownCreateCmd(), function(_, ok) TOWN = ok end)",
    )
    .exec()
    .unwrap();
    assert_eq!(lua.load("return #SENT").eval::<usize>().unwrap(), 1);
    assert_eq!(
        lua.load("return TOWN").eval::<Option<bool>>().unwrap(),
        None
    );
    lua.load("FRAME() FRAME() FRAME()").exec().unwrap();
    assert_eq!(
        lua.load("return TOWN").eval::<Option<bool>>().unwrap(),
        Some(false)
    );

    // A vehicle sold goes to the room by its id; its ticket is noted, and
    // its callback hears once the plugin's state passed the answer on.
    lua.load(
        "api.cmd.sendCommand(api.cmd.makeVehicleSellCmd({ 5 }), function(_, ok) SOLD = ok end) \
         FRAME() FRAME() FRAME()",
    )
    .exec()
    .unwrap();
    let (commands, sold, noted): (usize, i64, String) = lua
        .load(
            "return #HOOK.commands, HOOK.commands[1].SellVehicle.vehicles[1], \
             HOOK.notes['tpf3mp.hud.tickets']",
        )
        .eval()
        .unwrap();
    assert_eq!((commands, sold, noted.as_str()), (1, 3, "1"));
    assert_eq!(
        lua.load("return SOLD").eval::<Option<bool>>().unwrap(),
        None,
        "not before the room's answer"
    );
    let passed: usize = lua
        .load("return ANSWER({ { ticket = 9, ok = true }, { ticket = 1, ok = true } })")
        .eval()
        .unwrap();
    assert_eq!(passed, 1, "only this state's own");
    lua.load("FRAME() FRAME() FRAME()").exec().unwrap();
    let (sold, noted): (Option<bool>, Option<String>) = lua
        .load("return SOLD, HOOK.notes['tpf3mp.hud.tickets']")
        .eval()
        .unwrap();
    assert_eq!(sold, Some(true));
    assert_eq!(noted.as_deref(), Some("0"), "idle slot stays reserved");

    // The store's "buy onto a line": the buy goes to the room; told which
    // vehicle it bought once the registry names it, the store puts it on
    // its line, which goes to the room by canonical ids.
    lua.load(
        "STATE.registry.lines.bound = { { 1, 600 } } \
         CONFIG = { vehicles = { { part = { modelId = 41, reversed = false, \
             compartment2loadConfig = {}, color = { x = 1, y = 0, z = 0 } } } }, \
             vehicleGroups = { 1 }, muFileNames = { '' } } \
         api.cmd.sendCommand(api.cmd.makeVehicleBuyCmd(25, 202, CONFIG), function(data, ok, entities) \
             HEARD = { vehicle = data.resultVehicleEntity, ok = ok, entity = entities[1] and entities[1][1] } \
             api.cmd.sendCommand(api.cmd.makeVehicleSetLineCmd(data.resultVehicleEntity, 600, 0)) \
         end) \
         FRAME() \
         ANSWER({ { ticket = 2, ok = true, entity = 500 } }) \
         FRAME() FRAME()",
    )
    .exec()
    .unwrap();
    let (handed, kind, heard): (usize, String, bool) = lua
        .load("return #HOOK.commands, next(HOOK.commands[2]), HEARD ~= nil")
        .eval()
        .unwrap();
    assert_eq!((handed, kind.as_str()), (2, "BuyVehicle"));
    assert!(
        !heard,
        "not before the registry names the vehicle: its line could not be named"
    );
    lua.load("STATE.registry.vehicles.bound = { { 3, 5 }, { 4, 500 } } FRAME() FRAME()")
        .exec()
        .unwrap();
    let assigned: String = lua
        .load(
            "local a = HOOK.commands[3].AssignLine \
             return table.concat({ HEARD.vehicle, tostring(HEARD.ok), HEARD.entity, \
                 a.vehicles[1], a.line, a.first_stop }, '|')",
        )
        .eval()
        .unwrap();
    assert_eq!(assigned, "500|true|500|4|1|0");

    // A new line's window hears the line it made.
    lua.load(
        "LINE = { stops = {}, vehicleInfo = { transportModes = {} } } \
         api.cmd.sendCommand(api.cmd.makeLineCreateCmd('L', { x = 1, y = 0, z = 0 }, 25, LINE), \
             function(data, ok) MADE = { ok = ok, line = data.resultEntity } end) \
         STATE.registry.lines.bound = { { 1, 600 }, { 2, 601 } } \
         ANSWER({ { ticket = 4, ok = true, entity = 601 } }) \
         FRAME() FRAME()",
    )
    .exec()
    .unwrap();
    let (kind, made, line): (String, bool, i64) = lua
        .load("return next(HOOK.commands[4]), MADE.ok, MADE.line")
        .eval()
        .unwrap();
    assert_eq!((kind.as_str(), made, line), ("CreateLine", true, 601));
    assert_eq!(lua.load("return #SENT").eval::<usize>().unwrap(), 1);
    let logged: String = lua
        .load("return table.concat(HOOK.logged, '|')")
        .eval()
        .unwrap();
    assert!(
        logged.contains(
            "refused the player's makeTownCreateCmd in the room's game (1 so far) in the HUD's state"
        ) && !logged.contains("a window that waits on what it made"),
        "{logged}"
    );
    // A full queue refuses before sending another action. Every accepted
    // command still receives its answer; none is evicted to make room.
    lua.load(
        r#"
        local old = {}
        for t = 1, #HOOK.commands do old[#old + 1] = { ticket = t, ok = false } end
        ANSWER(old) FRAME() FRAME()
        local before = #HOOK.commands
        DONE = 0
        for i = 1, HUD.MAX_PENDING + 1 do
            api.cmd.sendCommand(api.cmd.makeVehicleSellCmd({ 5 }), function(_, ok)
                assert(not ok); DONE = DONE + 1
            end)
        end
        assert(#HOOK.commands == before + HUD.MAX_PENDING)
        FRAME() FRAME()
        assert(DONE == 1, 'only the refused extra command has answered')
        local answers = {}
        for t = before + 1, #HOOK.commands do answers[#answers + 1] = { ticket = t, ok = false } end
        ANSWER(answers)
        for i = 1, 10 do FRAME() ANSWER({}) end
        assert(DONE == HUD.MAX_PENDING + 1, 'every accepted ticket must finish')
    "#,
    )
    .exec()
    .unwrap();
}

/// A road modifier's build, as the room orders it: the street 8-9 rebuilt in
/// place with the lanes, decoration, lock and owner the tool gave it. Every
/// game gives the lanes their modes as the game takes them, a Lua array
/// from 1 (build 40408: keyed from 0 they land one mode off, and a sidewalk
/// that carries vehicles crashed the simulation), its decoration by the id
/// its name has here, and the acting company as its owner.
#[test]
fn a_road_modifier_is_built_with_its_lanes_decorations_lock_and_owner() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(
        r#"
        -- Each read of a template's lanes gives new copies, as the game's.
        api.res.streetTemplateRep.get = function(id)
            return setmetatable({ streetStyle = '::/style/country.street_style' }, { __index = function(_, k)
                if k == 'laneConfigs' then
                    return { { speed = 1, width = 1, height = 0, offset = 0, forward = true, transportModes = {} },
                             { speed = 1, width = 1, height = 0, offset = 0, forward = true, transportModes = {} } }
                end
            end })
        end
        api.res.edgeDecorationRep = { find = function(name)
            if name == '::/infrastructure/edge_addons/barrier_b.edge' then return 3 end return -1 end }
        api.type.PlayerOwned = { new = function() return {} end }
        "#,
    )
    .exec()
    .unwrap();
    let action = "{ BuildRoad = { street = '::/street/country.street_template', bus_lane = false, tram = 'None', \
        polyline = { vertices = { \
            { pos = { x = 50, y = -40, z = 0 }, resolve = { Node = 'Street' } }, \
            { pos = { x = 50, y = 40, z = 0 }, resolve = { Node = 'Street' } } }, \
          links = { { from = 0, to = 1, tangent0 = { x = 0, y = 80, z = 0 }, tangent1 = { x = 0, y = 80, z = 0 }, \
            structure = 'Ground', \
            lanes = { { speed = 22.22, width = 2, height = 0, offset = -1, forward = false, modes = 3 }, \
                      { speed = 22.22, width = 5, height = 0, offset = -0.5, forward = false, modes = 124 } }, \
            decorations = { { name = '::/infrastructure/edge_addons/barrier_b.edge', flag = false } }, \
            locked = true, owned = true } }, \
          removals = { { network = 'Street', ends = { a = { x = 50, y = -40, z = 0 }, b = { x = 50, y = 40, z = 0 } } } }, \
          removed_nodes = {} } } }";
    lua.load(format!(
        "HOOK.batch = {{ {action} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    let built: String = lua
        .load(
            "local e = SENT[1].proposal.streetProposal.edgesToAdd[1]
             local function modes(l)
                 local on = {}
                 for i = 1, 16 do if l.transportModes[i] then on[#on + 1] = i - 1 end end
                 return table.concat(on, ',') .. (l.transportModes[0] == nil and '' or ' zero-keyed!')
             end
             return table.concat({ modes(e.comp.laneConfigs[1]), modes(e.comp.laneConfigs[2]),
                 e.comp.laneConfigs[2].width, e.comp.edgeDecorations[1][1],
                 tostring(e.comp.edgeDecorations[1][2]), tostring(e.comp.roadDevelopmentLocked),
                 e.playerOwned.player }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| {
            panic!(
                "{error}\n{:?}",
                lua.load("return HOOK.logged").eval::<Vec<String>>()
            )
        });
    assert_eq!(built, "0,1|2,3,4,5,6|5|3|false|true|25");
    // Said in the log once built, the same line in every game.
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    let said = "upgrade applied: street upgrade of 1 edge(s) rebuilt in place; \
                template ::/street/country.street_template; 2 lane(s) carrying PERSON CARGO CAR BUS \
                TRUCK TRAM ELECTRIC_TRAM; lane speeds 22.22 to 22.22; decorations \
                ::/infrastructure/edge_addons/barrier_b.edge; locked 1, owned 1";
    assert!(logged.iter().any(|l| l == said), "{logged:?}");
}

/// With three companies, a loan one founded company takes is its own: it
/// alone gets the money and pays every month's interest and repayment,
/// alike in every game; the other founded company and the room's first are
/// never charged. A loan the finance window lists (the loan script's, the
/// first company's) cannot repay another of the borrower's by its id alone.
#[test]
fn only_the_company_that_borrowed_pays_its_loan() {
    let (lua, _script) = engine();
    lua.load(
        r#"
        GAME_T = 0
        api.type.ComponentType.GAME_TIME = 99
        api.engine.util.getWorld = function() return 1 end
        api.engine.getComponent = function(e, kind)
            if kind == 99 then return { gameTime = GAME_T } end
        end
        api.util = { getDefaultMonthDuration = function() return 1000 end }
        api.type.JournalEntry = { new = function() return { category = {} } end,
                                  Type = { LOAN = 'LOAN', INTEREST = 'INTEREST' } }
        api.cmd.makeJournalBookAssetCmd = function(e, entry) return { journal = entry, entity = e } end
        A, B, C = string.rep("a", 64), string.rep("b", 64), string.rep("c", 64)
        HOOK.room = true
        HOOK.batch = { { CompanyOp = { Create = { name = 'Ann' } } }, { CompanyOp = { Create = { name = 'Bob' } } } }
        HOOK.origins = { A, B }
        UPDATE({}, STATE, 0.2)
        -- Ann (901) borrows 1200 over 12 months at 12 % total interest.
        OFFER = { type = 'Small', amount = 1200, duration = 12000, percentage = 0.12 }
        STATE.value.companies.loanOffers = { { company = 1, availableLoans = { OFFER } } }
        HOOK.batch = { { Loan = { Take = { next = OFFER, offer = OFFER } } } } HOOK.origins = { A }
        UPDATE({}, STATE, 0.2)
        function BOOKED()
            local out = {}
            for _, c in ipairs(SENT) do
                if c.journal then out[#out + 1] = c.journal.category.type .. c.journal.amount .. '@' .. c.entity
                elseif c.event then out[#out + 1] = c.event.name
                elseif c.addPlayer then out[#out + 1] = c.addPlayer end
            end
            SENT = {}
            return table.concat(out, ',')
        end
        "#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let booked: String = lua.load("return BOOKED()").eval().unwrap();
    assert_eq!(booked, "Ann,Bob,LOAN1200@901", "the money to Ann alone");
    // Three months: each one Ann's interest and repayment, nobody else's.
    lua.load(
        "for m = 1, 3 do GAME_T = m * 1000 UPDATE({}, STATE, 0.2) GAME_T = m * 1000 + 1 UPDATE({}, STATE, 0.2) end",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let booked: String = lua.load("return BOOKED()").eval().unwrap();
    assert_eq!(
        booked,
        "LOAN-100@901,INTEREST-12@901,LOAN-100@901,INTEREST-12@901,LOAN-100@901,INTEREST-12@901"
    );
    // The finance window's Repay of the first company's loan 1 (another
    // amount) is refused; Ann's own, from the Multiplayer window, goes.
    lua.load(
        "HOOK.batch = { { Loan = { Repay = { loan = { type = 'Small', amount = 5000000, duration = 12000, \
                                                      percentage = 0.03, id = 1 } } } }, \
                        { Loan = { Repay = { loan = { type = 'Custom', amount = 1200, duration = 1, \
                                                      percentage = 0, id = 1 } } } } } \
         HOOK.origins = { A, A } \
         UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let applied: Vec<String> = lua
        .load(
            "local n = #HOOK.applied \
             return { tostring(HOOK.applied[n - 1].why), tostring(HOOK.applied[n].ok) }",
        )
        .eval()
        .unwrap();
    assert_eq!(
        applied,
        [
            "that loan is not this company's: its own are in the Multiplayer window",
            "true"
        ]
    );
    let booked: String = lua.load("return BOOKED()").eval().unwrap();
    assert_eq!(booked, "LOAN-900@901");
    // Four loans at once, as the game's loan script allows: a fifth is
    // refused.
    let why: String = lua
        .load(
            "for i = 1, 4 do STATE.value.companies.loans[i] = { id = i, company = 1, amount = 1, remaining = 1, months = 1, paid = 0, rate = 0, payment = 1 } end              HOOK.batch = { { Loan = { Take = { next = OFFER, offer = OFFER } } } } HOOK.origins = { A } UPDATE({}, STATE, 0.2)              return tostring(HOOK.applied[#HOOK.applied].why)",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}
{}", log(&lua)));
    assert_eq!(why, "Ann has 4 loans already");
}

/// Founded companies get loan offers from their own persistent copy. The
/// exact displayed terms are required, a taken slot cools down, and neither
/// another company nor the save player's native loan table is changed.
#[test]
fn founded_company_loan_offers_are_checked_consumed_and_company_scoped() {
    let (lua, _script) = engine();
    lua.load(
        r#"
        GAME_T = 0
        -- Lua 5.1's rand/srand are process-global across parallel test VMs.
        -- Exercise independent cooldowns deterministically, at both bounds,
        -- instead of relying on two seeded draws happening to be equal.
        LOAN_COOLDOWNS = { 4000, 8000 }
        math.random = function(first, last)
            assert(first == 4000 and last == 8000, 'native cooldown bounds')
            local delay = table.remove(LOAN_COOLDOWNS, 1)
            assert(delay, 'an unexpected cooldown draw')
            return delay
        end
        api.type.ComponentType.GAME_TIME = 99
        api.type.ComponentType.GAME_SCRIPT = 7
        api.engine.util.getWorld = function() return 1 end
        api.engine.system.gameScriptSystem = {}
        api.engine.system.gameScriptSystem.getEntityForGameScript = function(name)
            if name == '::/game_mechanics/finance/loan.gs' then return 40 end
            return -1
        end
        api.engine.getComponent = function(e, kind)
            if e == 1 and kind == 99 then return { gameTime = GAME_T } end
            if e == 40 and kind == 7 then return { state = LOANS } end
        end
        api.util = { getDefaultMonthDuration = function() return 1000 end }
        api.type.JournalEntry = { new = function() return { category = {} } end,
                                  Type = { LOAN = 'LOAN', INTEREST = 'INTEREST' } }
        api.cmd.makeJournalBookAssetCmd = function(e, entry) return { journal = entry, entity = e } end
        LOANS = { availableLoans = {
            { type = 'Small', amount = 1200, duration = 12000, percentage = 0.12, birthDay = 0 },
            { type = 'Medium', amount = 2400, duration = 24000, percentage = 0.08, birthDay = 0 },
            { type = 'Large', amount = 3600, duration = 36000, percentage = 0.10, birthDay = 0 },
            { type = 'ExtraLarge', amount = 4800, duration = 48000, percentage = 0.12, birthDay = 0 },
        }, obtainedLoans = {}, freeId = 0 }
        GAME_MODULES = { ['::/game_mechanics/finance/loan_util.tl'] = {
            createSmallLoan = function() return { type = 'Small', amount = 1300, duration = 12000, percentage = 0.10, birthDay = GAME_T } end,
            createMediumLoan = function() return { type = 'Medium', amount = 2500, duration = 24000, percentage = 0.07, birthDay = GAME_T } end,
            createLargeLoan = function() return { type = 'Large', amount = 3700, duration = 36000, percentage = 0.09, birthDay = GAME_T } end,
            createExtraLargeLoan = function() return { type = 'ExtraLarge', amount = 4900, duration = 48000, percentage = 0.11, birthDay = GAME_T } end,
        } }
        A, B = string.rep('a', 64), string.rep('b', 64)
        HOOK.room = true
        HOOK.batch = { { CompanyOp = { Create = { name = 'Ann' } } },
                       { CompanyOp = { Create = { name = 'Bob' } } } }
        HOOK.origins = { A, B }
        UPDATE({}, STATE, 0.2)
        ANN, BOB = STATE.value.companies.list[2].id, STATE.value.companies.list[3].id
        assert(#STATE.value.companies.loanOffers == 2)
        OFFER = { type = 'Small', amount = 1200, duration = 12000, percentage = 0.12 }
        NEXT = { type = 'Small', amount = 1300, duration = 12000, percentage = 0.10, birthDay = GAME_T }
        function TAKE(player, offer, next)
            HOOK.batch = { { Loan = { Take = { next = next, offer = offer } } } }
            HOOK.origins = { player }
            UPDATE({}, STATE, 0.2)
            return HOOK.applied[#HOOK.applied].ok, HOOK.applied[#HOOK.applied].why
        end
        function JOURNALS()
            local n = 0
            for _, c in ipairs(SENT) do
                if c.journal and c.journal.category.type == 'LOAN' and c.journal.amount > 0 then n = n + 1 end
            end
            return n
        end
        "#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));

    let forged: (bool, String) = lua
        .load("local ok, why = TAKE(A, { type = 'Small', amount = 999999, duration = 12000, percentage = 0.12 }, NEXT) return ok, why")
        .eval()
        .unwrap();
    assert_eq!(
        forged,
        (false, "that loan offer is no longer available".into())
    );
    let forged_rate: (bool, String) = lua
        .load("local ok, why = TAKE(A, { type = 'Small', amount = 1200, duration = 12000, percentage = 0.01 }, NEXT) return ok, why")
        .eval()
        .unwrap();
    assert_eq!(
        forged_rate,
        (false, "that loan offer is no longer available".into())
    );
    let forged_duration: (bool, String) = lua
        .load("local ok, why = TAKE(A, { type = 'Small', amount = 1200, duration = 24000, percentage = 0.12 }, NEXT) return ok, why")
        .eval()
        .unwrap();
    assert_eq!(
        forged_duration,
        (false, "that loan offer is no longer available".into())
    );
    let wrong_replacement: (bool, String) = lua
        .load("local ok, why = TAKE(A, OFFER, { type = 'Medium', amount = 2500, duration = 24000, percentage = 0.07 }) return ok, why")
        .eval()
        .unwrap();
    assert_eq!(
        wrong_replacement,
        (
            false,
            "the replacement must match the offered loan type".into()
        )
    );
    let accepted: (bool, Option<String>) = lua.load("return TAKE(A, OFFER, NEXT)").eval().unwrap();
    assert_eq!(accepted, (true, None));
    let after_first = lua
        .load(
            "local roster = STATE.value.companies local offers = roster.loanOffers \
             return offers[1].availableLoans[1].cooldownUntil, offers[2].availableLoans[1].amount, \
                 LOANS.availableLoans[1].amount, JOURNALS()",
        )
        .eval::<(i64, i64, i64, i64)>()
        .unwrap();
    assert_eq!(after_first.0, 4_000, "Ann gets the first cooldown draw");
    assert_eq!(after_first.1, 1200, "Bob's same slot remains available");
    assert_eq!(after_first.2, 1200, "the native offer remains untouched");
    assert_eq!(after_first.3, 1, "one loan booking only");
    let reused: (bool, String) = lua
        .load("local ok, why = TAKE(A, OFFER, NEXT) return ok, why")
        .eval()
        .unwrap();
    assert_eq!(
        reused,
        (false, "that loan offer is no longer available".into())
    );
    let bob: (bool, Option<String>) = lua.load("return TAKE(B, OFFER, NEXT)").eval().unwrap();
    assert_eq!(
        bob,
        (true, None),
        "another company's offer slot stays independent"
    );
    let journals: i64 = lua.load("return JOURNALS()").eval().unwrap();
    assert_eq!(journals, 2, "the refused reuse made no duplicate charge");
    let at_boundary: (Option<i64>, Option<i64>, i64, i64, i64) = lua
        .load(
            "GAME_T = 4000 UPDATE({}, STATE, 0.2) local roster = STATE.value.companies \
             return roster.loanOffers[1].availableLoans[1].amount, \
                 roster.loanOffers[2].availableLoans[1].amount, \
                 roster.loanOffers[1].availableLoans[1].cooldownUntil, \
                 roster.loanOffers[2].availableLoans[1].cooldownUntil, JOURNALS()",
        )
        .eval()
        .unwrap();
    assert_eq!(
        at_boundary,
        (None, None, 4000, 8000, 2),
        "neither company's offer refreshes at Ann's exact cooldown boundary"
    );
    let first_refreshed: (i64, Option<i64>, Option<i64>, i64, i64, i64) = lua
        .load(
            "GAME_T = 4001 UPDATE({}, STATE, 0.2) local roster = STATE.value.companies \
             return roster.loanOffers[1].availableLoans[1].amount, \
                 roster.loanOffers[2].availableLoans[1].amount, \
                 roster.loanOffers[1].availableLoans[1].cooldownUntil, \
                 roster.loanOffers[2].availableLoans[1].cooldownUntil, \
                 LOANS.availableLoans[1].amount, JOURNALS()",
        )
        .eval()
        .unwrap();
    assert_eq!(
        first_refreshed,
        (1300, None, None, 8000, 1200, 2),
        "Ann's expiry refreshes only her slot, leaving Bob and native offers untouched"
    );
    let refreshed: (i64, i64, Option<i64>, Option<i64>, i64, i64) = lua
        .load(
            "GAME_T = 8001 UPDATE({}, STATE, 0.2) local roster = STATE.value.companies \
             return roster.loanOffers[1].availableLoans[1].amount, \
                 roster.loanOffers[2].availableLoans[1].amount, \
                 roster.loanOffers[1].availableLoans[1].cooldownUntil, \
                 roster.loanOffers[2].availableLoans[1].cooldownUntil, \
                 JOURNALS(), #LOAN_COOLDOWNS",
        )
        .eval()
        .unwrap();
    assert_eq!(
        refreshed,
        (1300, 1300, None, None, 2, 0),
        "both slots refresh after their own cooldowns without duplicate borrowing"
    );
}

/// A millisecond game clock and two companies, through the actual mod
/// update/postUpdate path. The amounts have exact native installments:
/// loan_util.tl (build 40408), 1200 / 12 principal and 12% of that interest.
const COMPANY_LOAN_CLOCK: &str = r#"
GAME_T = 0
api.type.ComponentType.GAME_TIME = 99
api.engine.util.getWorld = function() return 1 end
api.engine.getComponent = function(e, kind)
    if kind == 99 then return { gameTime = GAME_T } end
end
api.util = { getDefaultMonthDuration = function() return 1000 end }
api.type.JournalEntry = { new = function() return { category = {} } end,
                          Type = { LOAN = 'LOAN', INTEREST = 'INTEREST' } }
api.cmd.makeJournalBookAssetCmd = function(e, entry) return { journal = entry, entity = e } end
A, B = string.rep('a', 64), string.rep('b', 64)
HOOK.room = true
HOOK.batch = { { CompanyOp = { Create = { name = 'Ann' } } },
               { CompanyOp = { Create = { name = 'Bob' } } } }
HOOK.origins = { A, B }
UPDATE({}, STATE, 0.2)
OFFERS = {
    Small = { type = 'Small', amount = 1200, duration = 12000, percentage = 0.12 },
    Medium = { type = 'Medium', amount = 1200, duration = 12000, percentage = 0.12 },
}
STATE.value.companies.loanOffers = {
    { company = 1, availableLoans = { OFFERS.Small, OFFERS.Medium } },
    { company = 2, availableLoans = { OFFERS.Small, OFFERS.Medium } },
}
function TAKE(player, kind)
    local offer = OFFERS[kind]
    HOOK.batch = { { Loan = { Take = { next = offer, offer = offer } } } }
    HOOK.origins = { player }
    UPDATE({}, STATE, 0.2)
    assert(HOOK.applied[#HOOK.applied].ok, HOOK.applied[#HOOK.applied].why)
end
function BOOKED()
    local out = {}
    for _, c in ipairs(SENT) do
        if c.journal then out[#out + 1] = c.journal.category.type .. c.journal.amount .. '@' .. c.entity end
    end
    SENT = {}
    return table.concat(out, ',')
end
C = ug_require('tpf3mp_1::/scripts/tpf3mp/companies.lua')
SENT = {}
"#;

#[test]
fn company_password_seals_survive_same_room_reload_but_reset_on_import() {
    let (lua, _) = engine();
    lua.load(COMPANY_LOAN_CLOCK).exec().unwrap();
    lua.load(
        r#"
        ROOM_A, ROOM_B = 'r-' .. string.rep('1', 32), 'r-' .. string.rep('2', 32)
        local r = STATE.value.companies
        RIVAL = r.list[2]
        r.passwordRoom = ROOM_A
        RIVAL.lock = { scope = 1, tag = string.rep('c', 64) }
        RIVAL.closed = true
        RIVAL.access = { { company = 2, allowed = false } }
        OLD_LIST, OLD_MEMBERS, OLD_ACCESS = r.list, r.members, RIVAL.access
        HOOK.status = { room = 'Same display name', room_id = ROOM_A, players = {} }
        -- An ordinary same-room save/replacement rebuilds the registry.
        STATE.value.registry = nil
        UPDATE({}, STATE, 0.2)
        assert(RIVAL.lock.tag == string.rep('c', 64))
        assert(C.passwordNotice(r) == nil)
        -- Different stable IDs must be detected even with equal names.
        HOOK.status.room_id = ROOM_B
        UPDATE({}, STATE, 0.2)
        assert(RIVAL.lock == nil and r.passwordRoom == ROOM_B)
        assert(r.list == OLD_LIST and r.members == OLD_MEMBERS and RIVAL.access == OLD_ACCESS)
        assert(RIVAL.closed and RIVAL.entity == 901 and C.of(r, A).id == 1 and C.of(r, B).id == 2)
        "#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(lua.load("return BOOKED()").eval::<String>().unwrap(), "");
    let notice: String = lua
        .load("return C.passwordNotice(STATE.value.companies)")
        .eval()
        .unwrap();
    assert!(notice.contains("different room"), "{notice}");
    assert!(notice.contains("must be set again: Ann"), "{notice}");
    let logged: String = lua
        .load("return table.concat(HOOK.logged, ';')")
        .eval()
        .unwrap();
    assert!(logged.contains(&notice), "{logged}");
    lua.load(
        "local r = STATE.value.companies \
         assert(C.run(r, A, { Lock = 1 }, function() error('no asset command expected') end, api, \
             { scope = 1, tag = string.rep('d', 64) })) \
         assert(C.passwordNotice(r) == nil) \
         STATE.value.registry = nil UPDATE({}, STATE, 0.2) \
         assert(RIVAL.lock.tag == string.rep('d', 64)) \
         HOOK.status.room = 'Renamed room' UPDATE({}, STATE, 0.2) \
         assert(RIVAL.lock.tag == string.rep('d', 64))",
    )
    .exec()
    .unwrap();
}

#[test]
fn legacy_company_password_provenance_resets_once_and_requires_a_stable_id() {
    let (lua, _) = engine();
    lua.load(COMPANY_LOAN_CLOCK).exec().unwrap();
    lua.load(
        r#"
        local r = STATE.value.companies
        r.list[2].lock = { scope = 1, tag = string.rep('c', 64) }
        HOOK.status = { room = 'display name', invite = 'K7QM2X', players = {} }
        UPDATE({}, STATE, 0.2)
        assert(r.list[2].lock ~= nil and r.passwordRoom == nil)
        assert(not C.roomNeedsBind(r, HOOK.status.room))
        assert(C.bindRoom(r, HOOK.status.invite) == nil)
        HOOK.status.room_id = 'r-' .. string.rep('3', 32)
        UPDATE({}, STATE, 0.2)
        assert(r.list[2].lock == nil)
        assert(C.passwordNotice(r):find('older save', 1, true))
        assert(C.run(r, A, { Lock = 1 }, function() end, api, { scope = 1, tag = string.rep('e', 64) }))
        assert(C.passwordNotice(r) == nil)
        STATE.value.registry = nil UPDATE({}, STATE, 0.2)
        assert(r.list[2].lock.tag == string.rep('e', 64))
        "#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let logged: String = lua
        .load("return table.concat(HOOK.logged, ';')")
        .eval()
        .unwrap();
    assert_eq!(
        logged
            .matches("Previous company passwords were cleared")
            .count(),
        1
    );
    assert_eq!(lua.load("return BOOKED()").eval::<String>().unwrap(), "");
}

#[test]
fn the_company_window_displays_an_imported_save_password_reset_notice() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load(
        r#"
        local C = ug_require('tpf3mp_1::/scripts/tpf3mp/companies.lua')
        ROSTER = { list = { { id = 0, entity = 25, name = 'First', color = {1,0,0} },
                           { id = 1, entity = 901, name = 'Ann', color = {0,0,1}, founder = 'me' } },
                   members = { { player = 'me', company = 1 } },
                   passwordReset = { legacy = false, companies = { 1 } } }
        api.engine = api.engine or {}
        api.engine.util = { getPlayer = function() return 25 end }
        api.util = { getDefaultMonthDuration = function() return 1000 end }
        api.engine.system = api.engine.system or {}
        api.engine.system.gameScriptSystem = { getEntityForGameScript = function(name)
            if name == 'tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs' then return 77 end return -1 end }
        api.type = api.type or {}
        api.type.ComponentType = api.type.ComponentType or {}
        api.type.ComponentType.GAME_SCRIPT = 7
        api.engine.getComponent = function(e, kind)
            if e == 77 and kind == 7 then return { state = { companies = ROSTER } } end
        end
        api.type.Vec3f = { new = function(x, y, z) return { x = x, y = y, z = z } end }
        HOOK.room = true
        HOOK.status = { room = 'New room', me_id = 'me', players = { { id = 'me', name = 'Ann', me = true } } }
        BAR = mount(loadPlugin())
        for _ = 1, 20 do BAR.step() end
        BAR.render() views(BAR.layout)[1].params.onClick()
        assert(WINDOWS.Tpf3mpWindow, table.concat(HOOK.logged, ';') .. logText())
        WINDOWS.Tpf3mpWindow.step()
        "#,
    )
    .exec()
    .unwrap();
    let visible: String = lua
        .load("local out = {} for _, v in ipairs(views(WINDOWS.Tpf3mpWindow.render())) do \
               if v.view == 'TextView' then out[#out + 1] = v.params.text end end return table.concat(out, ' ')")
        .eval()
        .unwrap();
    let visible = visible.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(visible.contains("Previous company passwords"), "{visible}");
    assert!(visible.contains("must be set again: Ann"), "{visible}");
}

#[test]
fn a_new_loan_does_not_pay_for_an_earlier_debt_free_gap() {
    let (lua, _) = engine();
    lua.load(COMPANY_LOAN_CLOCK).exec().unwrap();
    lua.load(
        "TAKE(A, 'Small') GAME_T = 3000 UPDATE({}, STATE, 0.2) \
         HOOK.batch = { { Loan = { Repay = { loan = { type = 'Small', amount = 1200, duration = 12000, percentage = 0.12, id = 1 } } } } } \
         HOOK.origins = { A } UPDATE({}, STATE, 0.2) \
         assert(#STATE.value.companies.loans == 0) BOOKED() \
         GAME_T = 10000 UPDATE({}, STATE, 0.2) TAKE(A, 'Medium')",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        lua.load("return BOOKED()").eval::<String>().unwrap(),
        "LOAN1200@901",
        "the new principal only, with no payments from the debt-free gap"
    );
    lua.load("GAME_T = 10001 UPDATE({}, STATE, 0.2) GAME_T = 10999 UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    assert_eq!(lua.load("return BOOKED()").eval::<String>().unwrap(), "");
    lua.load("GAME_T = 11000 UPDATE({}, STATE, 0.2) UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    assert_eq!(
        lua.load("return BOOKED()").eval::<String>().unwrap(),
        "LOAN-100@901,INTEREST-12@901",
        "one installment, one full interval after this loan was taken"
    );
    assert_eq!(
        lua.load(
            "local l = STATE.value.companies.loans[1] return l.remaining, l.paid, l.lastPayDay"
        )
        .eval::<(i64, i64, i64)>()
        .unwrap(),
        (1100, 1, 11000)
    );
}

#[test]
fn staggered_loans_keep_their_own_origins_across_calendar_boundaries() {
    let (lua, _) = engine();
    lua.load(COMPANY_LOAN_CLOCK).exec().unwrap();
    lua.load("GAME_T = 990 TAKE(A, 'Small') BOOKED() GAME_T = 1000 UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    assert_eq!(lua.load("return BOOKED()").eval::<String>().unwrap(), "");
    lua.load("GAME_T = 1010 TAKE(B, 'Small') BOOKED() GAME_T = 1989 UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    assert_eq!(lua.load("return BOOKED()").eval::<String>().unwrap(), "");
    lua.load("GAME_T = 1990 UPDATE({}, STATE, 0.2) GAME_T = 2000 UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    assert_eq!(
        lua.load("return BOOKED()").eval::<String>().unwrap(),
        "LOAN-100@901,INTEREST-12@901"
    );
    lua.load("GAME_T = 2010 UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    assert_eq!(
        lua.load("return BOOKED()").eval::<String>().unwrap(),
        "LOAN-100@902,INTEREST-12@902"
    );
    // Native loan.script.tl charges one installment and resets lastPayDay
    // to now after a delayed update; it does not back-charge every month.
    lua.load("GAME_T = 10000 UPDATE({}, STATE, 0.2) UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    assert_eq!(
        lua.load("return BOOKED()").eval::<String>().unwrap(),
        "LOAN-100@901,INTEREST-12@901,LOAN-100@902,INTEREST-12@902"
    );
}

#[test]
fn borrowing_on_another_loans_due_update_does_not_charge_the_new_loan() {
    let (lua, _) = engine();
    lua.load(COMPANY_LOAN_CLOCK).exec().unwrap();
    lua.load("TAKE(A, 'Small') BOOKED() GAME_T = 1000 TAKE(B, 'Small')")
        .exec()
        .unwrap();
    assert_eq!(
        lua.load("return BOOKED()").eval::<String>().unwrap(),
        "LOAN1200@902,LOAN-100@901,INTEREST-12@901"
    );
    assert_eq!(
        lua.load(
            "local l = STATE.value.companies.loans[2] return l.paid, l.remaining, l.lastPayDay"
        )
        .eval::<(i64, i64, i64)>()
        .unwrap(),
        (0, 1200, 1000)
    );
}

#[test]
fn saved_annuity_loans_migrate_once_without_money_or_retroactive_payments() {
    let (lua, _) = engine();
    lua.load(COMPANY_LOAN_CLOCK).exec().unwrap();
    lua.load(
        r#"
        local r = STATE.value.companies
        r.month, r.nextLoan = 3, 2
        r.loans = { { id = 1, company = 1, amount = 1200, remaining = 912,
                      months = 12, paid = 3, rate = 0.01, payment = 107, type = 'Small' } }
        GAME_T = 10000
        -- A GUI must not claim the native payoff for an unconverted annuity.
        assert(not pcall(C.loanTable, r, 1, {}, 1000))
        UPDATE({}, STATE, 0.2)
        "#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        lua.load("return BOOKED()").eval::<String>().unwrap(),
        "",
        "migration never books money or charges the old global month gap"
    );
    assert_eq!(
        lua.load("local l = STATE.value.companies.loans[1] return l.amount, l.remaining, l.months, l.paid, l.lastPayDay, l.legacy.amount, l.legacy.paid")
            .eval::<(i64, i64, i64, i64, i64, i64, i64)>()
            .unwrap(),
        (912, 912, 9, 0, 10000, 1200, 3)
    );
    assert_eq!(
        lua.load("return C.migrateLoans(STATE.value.companies, api)")
            .eval::<i64>()
            .unwrap(),
        0,
        "reloading a converted schedule never migrates it twice"
    );
    lua.load("GAME_T = 11000 UPDATE({}, STATE, 0.2) UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    assert_eq!(
        lua.load("return BOOKED()").eval::<String>().unwrap(),
        "LOAN-101@901,INTEREST-12@901"
    );
    let payoff = lua
        .load(
            "local r = STATE.value.companies local l = C.loanTable(r, 1, {}, 1000).obtainedLoans[1] \
             local n = math.floor(l.duration / 1000 + 0.5) \
             local monthly = math.floor(l.amount / n + 0.5) \
             local nativePayoff = (n - l.timesPaid) * monthly + l.amount - n * monthly \
             return l.amount, nativePayoff, r.loans[1].remaining",
        )
        .eval::<(i64, i64, i64)>()
        .unwrap();
    assert_eq!(payoff, (912, 811, 811));
    // A stale GUI loan's old amount cannot silently repay changed terms.
    let stale = lua
        .load("local ok = C.repay(STATE.value.companies, 1, { id = 1, amount = 1200 }, function(c) SENT[#SENT + 1] = c end, api) return ok")
        .eval::<bool>()
        .unwrap();
    assert!(!stale);
    assert_eq!(lua.load("return BOOKED()").eval::<String>().unwrap(), "");
    lua.load(
        "local l = C.loanTable(STATE.value.companies, 1, {}, 1000).obtainedLoans[1] \
         assert(C.repay(STATE.value.companies, 1, l, function(c) SENT[#SENT + 1] = c end, api))",
    )
    .exec()
    .unwrap();
    assert_eq!(
        lua.load("return BOOKED()").eval::<String>().unwrap(),
        "LOAN-811@901",
        "the facade's native payoff equals the actual debit"
    );
}

#[test]
fn founded_company_loans_use_native_final_rounding_and_payoff() {
    let (lua, _) = engine();
    lua.load(COMPANY_LOAN_CLOCK).exec().unwrap();
    lua.load(
        "OFFERS.Small.amount, OFFERS.Small.duration, OFFERS.Small.percentage = 1000, 3000, 0.10 \
         TAKE(A, 'Small') BOOKED() \
         GAME_T = 1000 UPDATE({}, STATE, 0.2) GAME_T = 2000 UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    // Reference vectors from native loan_util.tl: round(1000/3)=333,
    // round(100/3)=33; the final corrections are one unit each.
    assert_eq!(
        lua.load("return BOOKED()").eval::<String>().unwrap(),
        "LOAN-333@901,INTEREST-33@901,LOAN-333@901,INTEREST-33@901"
    );
    assert_eq!(
        lua.load("local r = STATE.value.companies local l = C.loanTable(r, 1, {}, 1000).obtainedLoans[1] \
                  return l.amount, l.timesPaid, r.loans[1].remaining")
            .eval::<(i64, i64, i64)>()
            .unwrap(),
        (1000, 2, 334),
        "native full payoff includes its principal rounding correction"
    );
    lua.load("GAME_T = 3000 UPDATE({}, STATE, 0.2) UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    assert_eq!(
        lua.load("return BOOKED()").eval::<String>().unwrap(),
        "LOAN-334@901,INTEREST-34@901"
    );
    assert_eq!(
        lua.load("return #STATE.value.companies.loans")
            .eval::<i64>()
            .unwrap(),
        0
    );
    // Native rounding can repay all principal before the final installment,
    // which still carries an interest correction. Migration/cleanup must
    // not drop that pending payment merely because remaining is zero.
    lua.load(
        "OFFERS.Medium.amount, OFFERS.Medium.duration, OFFERS.Medium.percentage = 10, 6000, 0.10 \
         GAME_T = 3001 TAKE(A, 'Medium') BOOKED() \
         for i = 1, 5 do GAME_T = 3001 + i * 1000 UPDATE({}, STATE, 0.2) end BOOKED()",
    )
    .exec()
    .unwrap();
    assert_eq!(
        lua.load("local l = STATE.value.companies.loans[1] return l.remaining, l.paid")
            .eval::<(i64, i64)>()
            .unwrap(),
        (0, 5)
    );
    lua.load("GAME_T = 9001 UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    assert_eq!(
        lua.load("return BOOKED()").eval::<String>().unwrap(),
        "LOAN-0@901,INTEREST-1@901"
    );
}

/// A headquarters as the game's resources declare one
/// (landmarks/hq/headquarter.con: `metadata.company.headquarters`).
const HQ: &str = "{ BuildConstruction = { \
    file = '::/landmarks/hq/headquarter.con', \
    transform = { basis = { 1, 0, 0, 0, 1, 0, 0, 0, 1 }, origin = { x = 100, y = 200, z = 5 } }, \
    params = {}, name = 'HQ' } }";

/// Every company of a room builds its own headquarters, one each: the game
/// counts its headquarters permit over the whole world (company_util.tl),
/// so every game checks the acting company's own (2026-10-01: after one
/// company's, the others could build none).
#[test]
fn each_company_builds_one_headquarters_of_its_own() {
    let (lua, _script) = engine();
    lua.load(
        r#"
        api.type.ComponentType.PLAYER_OWNED = 55
        api.type.ComponentType.PLAYER = 5
        -- Constructions, their owners, and the game's resources.
        CONS, OWNERS, NEXT_CON = {}, {}, 700
        api.res = { constructionRep = {
            find = function(file)
                if file == '::/landmarks/hq/headquarter.con' then return 1 end
                if file == 'depot/road_depot_era_a.con' then return 2 end
                return -1
            end,
            get = function(id)
                if id == 1 then return { metadata = { company = { headquarters = true, companyRank = 1 } } } end
                return { metadata = {} }
            end,
        } }
        api.engine.getComponent = function(e, kind)
            if kind == 2 then return CONS[e] end
            if kind == 55 then return OWNERS[e] and { player = OWNERS[e] } end
        end
        api.engine.getEntitiesWithComponent = function(kind)
            local entities = {}
            if kind == 2 then for e in pairs(CONS) do entities[#entities + 1] = e end end
            table.sort(entities)
            return entities
        end
        api.engine.forEachEntityWithComponent = function(fn, kind)
            if kind == 2 then for e in pairs(CONS) do fn(e) end end
        end
        -- What a build makes: its construction, owned by the company that
        -- pays for it.
        local make = api.cmd.makeWorldBuildProposalCmd
        api.cmd.makeWorldBuildProposalCmd = function(proposal, context, ...)
            for _, e in ipairs(proposal.constructionsToAdd or {}) do
                NEXT_CON = NEXT_CON + 1
                CONS[NEXT_CON] = { fileName = e.fileName,
                    transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0,
                        e.transf[4][1], e.transf[4][2], e.transf[4][3], 1 } }
                OWNERS[NEXT_CON] = e.playerEntity
            end
            return make(proposal, context, ...)
        end
        A, B = string.rep("a", 64), string.rep("b", 64)
        HOOK.batch = { { CompanyOp = { Create = { name = 'Rival' } } } } HOOK.origins = { A }
        UPDATE({}, STATE, 0.2)
        function built(action, who)
            HOOK.applied = {}
            HOOK.batch = { action } HOOK.origins = { who }
            UPDATE({}, STATE, 0.2)
            local a = HOOK.applied[1]
            return tostring(a.ok) .. (a.why and (' ' .. a.why) or '')
        end
        "#,
    )
    .exec()
    .unwrap();
    let eval = |code: &str| -> String {
        lua.load(code)
            .eval::<String>()
            .unwrap_or_else(|error| panic!("{code}: {error}"))
    };
    // Rival builds its headquarters, then the first company its own.
    assert_eq!(eval(&format!("return built({HQ}, A)")), "true");
    let second_hq = HQ.replace(
        "origin = { x = 100, y = 200, z = 5 }",
        "origin = { x = 200, y = 200, z = 5 }",
    );
    assert_eq!(eval(&format!("return built({second_hq}, B)")), "true");
    // A second one is refused, alike in every game; other buildings not.
    assert_eq!(
        eval(&format!("return built({HQ}, A)")),
        "false Rival has its headquarters already"
    );
    assert_eq!(eval(&format!("return built({DEPOT}, A)")), "true");
    let owners = eval(
        "local out = {} \
         for e, c in pairs(CONS) do \
             if c.fileName == '::/landmarks/hq/headquarter.con' then out[#out + 1] = tostring(OWNERS[e]) end \
         end table.sort(out) return table.concat(out, ',')",
    );
    assert_eq!(
        owners, "25,901",
        "one each, the first company's and Rival's"
    );
    // What each company owns, as hook.log says it once a world is up: a
    // world loaded from a save says so whether its owners came back.
    let ownership = eval(
        "local C = ug_require('tpf3mp_1::/scripts/tpf3mp/companies.lua')          return C.ownership(STATE.value.companies, api)",
    );
    assert!(
        ownership.contains("Rival #1 (entity 901): 2 construction(s), headquarters "),
        "{ownership}"
    );
    assert!(
        ownership.contains("#0 (entity 25): 1 construction(s), headquarters "),
        "{ownership}"
    );
    let logged = eval("return table.concat(HOOK.logged, '|')");
    assert!(logged.contains("ownership: "), "{logged}");
}

/// Each company's headquarters gives its own town the game's bonus: the
/// game's town script sums every headquarters' `town_growth` onto its
/// closest town, whoever owns it (towns.script.tl, landmark_util.tl), so
/// the mod adds nothing and says, read only, what each company's
/// headquarters is, what its PLAYER names and the bonus its town gets, at
/// the companies' samples, again only when that changed.
#[test]
fn each_headquarters_bonus_is_logged_for_its_own_town() {
    let (lua, _script) = engine();
    lua.load(
        r#"
        api.type.ComponentType.PLAYER_OWNED = 55
        api.type.ComponentType.PLAYER = 5
        api.type.ComponentType.GAME_SCRIPT = 77
        api.type.ComponentType.GAME_TIME = 99
        GAME_T = 0
        api.engine.util.getWorld = function() return 1 end
        api.util = { getDefaultMonthDuration = function() return 1000 end }
        CONS, OWNERS, PLAYERS, NEXT_CON = {}, {}, {}, 700
        -- Two towns: a headquarters' closest town by its x.
        TOWN_OF, NAMES = {}, { [31] = 'Ashford', [32] = 'Brill' }
        api.engine.util.getEntityName = function(e) return NAMES[e] end
        api.engine.system.streetConnectorSystem = {
            getConstructionClosestTown = function(e) return TOWN_OF[e] or -1 end }
        -- The game's town script's state: what it applies to each town.
        TOWNS = { townStates = {
            { townEntity = { entity = 31 }, constructionBoni = { xpIncrease = 0, reputationRecoveryBoost = 0 } },
            { townEntity = { entity = 32 }, constructionBoni = { xpIncrease = 0, reputationRecoveryBoost = 0 } } } }
        api.engine.system.gameScriptSystem = { getEntityForGameScript = function(name)
            if name == '::/game_mechanics/towns/town.gs' then return 600 end return -1 end }
        api.res = { constructionRep = {
            find = function(file)
                if file == '::/landmarks/hq/headquarter.con' then return 1 end
                return -1
            end,
            get = function(id)
                if id == 1 then return { metadata = { company = { headquarters = true, companyRank = 1 } } } end
                return { metadata = {} }
            end,
        } }
        api.engine.getComponent = function(e, kind)
            if kind == 99 then return { gameTime = GAME_T } end
            if kind == 2 then return CONS[e] end
            if kind == 55 then return OWNERS[e] and { player = OWNERS[e] } end
            if kind == 5 then return PLAYERS[e] end
            if e == 600 and kind == 77 then return { state = TOWNS } end
        end
        api.engine.forEachEntityWithComponent = function(fn, kind)
            if kind == 2 then for e in pairs(CONS) do fn(e) end end
        end
        -- What a build makes, as the engine makes it (apply_proposal.cpp):
        -- the construction, owned by the proposal's playerEntity, whose
        -- PLAYER then names it as its headquarters; the headquarters'
        -- town_growth on it (headquarter.script.tl).
        local make = api.cmd.makeWorldBuildProposalCmd
        api.cmd.makeWorldBuildProposalCmd = function(proposal, context, ...)
            for _, e in ipairs(proposal.constructionsToAdd or {}) do
                NEXT_CON = NEXT_CON + 1
                CONS[NEXT_CON] = { fileName = e.fileName,
                                   persistentMetadata = { town_growth = { xpIncrease = 0.05 } } }
                OWNERS[NEXT_CON] = e.playerEntity
                PLAYERS[e.playerEntity] = { headquarters = NEXT_CON }
                TOWN_OF[NEXT_CON] = e.transf[4][1] > 500 and 32 or 31
            end
            return make(proposal, context, ...)
        end
        A, B = string.rep("a", 64), string.rep("b", 64)
        HOOK.room = true
        HOOK.batch = { { CompanyOp = { Create = { name = 'Rival' } } } } HOOK.origins = { A }
        UPDATE({}, STATE, 0.2)
        function hq(x)
            return { BuildConstruction = { file = '::/landmarks/hq/headquarter.con',
                transform = { basis = { 1, 0, 0, 0, 1, 0, 0, 0, 1 }, origin = { x = x, y = 200, z = 5 } },
                params = {}, name = 'HQ' } }
        end
        HOOK.batch = { hq(100), hq(900) } HOOK.origins = { A, B }
        UPDATE({}, STATE, 0.2)
        -- The game's town script, at its next check of the constructions.
        TOWNS.townStates[1].constructionBoni.xpIncrease = 0.05
        TOWNS.townStates[2].constructionBoni.xpIncrease = 0.05
        function headquartersLogged()
            local out = {}
            for _, l in ipairs(HOOK.logged) do
                if l:sub(1, 14) == 'headquarters: ' then out[#out + 1] = l end
            end
            return out
        end
        "#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    let eval = |code: &str| -> String {
        lua.load(code)
            .eval::<String>()
            .unwrap_or_else(|error| panic!("{code}: {error}"))
    };
    // Both built, each owned by its company, which the PLAYER names.
    assert_eq!(
        eval(
            "local out = {} for e, c in pairs(CONS) do out[#out + 1] = OWNERS[e] .. '>' .. TOWN_OF[e] end \
             table.sort(out) return table.concat(out, ',')"
        ),
        "25>32,901>31"
    );
    let report = eval(
        "local C = ug_require('tpf3mp_1::/scripts/tpf3mp/companies.lua') \
         local lines = C.headquartersReport(STATE.value.companies, api) return table.concat(lines, '|')",
    );
    let lines: Vec<&str> = report.split('|').collect();
    assert_eq!(lines.len(), 2, "{report}");
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("Company #0: headquarters ")
                && l.contains(
                    "closest town 32 (Brill): on it xp +0.05, reputation recovery +0.00; \
                 the game's town script applies xp +0.05, reputation recovery +0.00 there"
                )),
        "{report}"
    );
    let rival = lines
        .iter()
        .find(|l| l.starts_with("Rival #1: headquarters "))
        .unwrap_or_else(|| panic!("{report}"));
    let hq = rival
        .trim_start_matches("Rival #1: headquarters ")
        .split(' ')
        .next()
        .unwrap();
    assert!(
        rival.contains(&format!(
            "headquarters {hq} (a construction), owned by 901;"
        )),
        "the engine names Rival's own: {rival}"
    );
    assert!(
        rival.contains("closest town 31 (Ashford): on it xp +0.05"),
        "{rival}"
    );
    // The companies' sample in the update that built them said both, before
    // the game's town script had checked the constructions; at the next,
    // its bonus applied, both again; then nothing changed and nothing more
    // is said.
    assert_eq!(
        eval(
            "local l = headquartersLogged() return #l .. ' ' .. tostring(l[1]:find('applies xp +0.00', 1, true) ~= nil)"
        ),
        "2 true"
    );
    assert_eq!(
        eval("GAME_T = 1000 UPDATE({}, STATE, 0.2) return tostring(#headquartersLogged())"),
        "4"
    );
    assert_eq!(
        eval("GAME_T = 2000 UPDATE({}, STATE, 0.2) return tostring(#headquartersLogged())"),
        "4"
    );
    // A medium wing on Rival's: its town's bonus changes, and only Rival's
    // line is said again.
    assert_eq!(
        eval(&format!(
            "CONS[{hq}].persistentMetadata.town_growth.xpIncrease = 0.06 \
             TOWNS.townStates[1].constructionBoni.xpIncrease = 0.06 \
             GAME_T = 3000 UPDATE({{}}, STATE, 0.2) \
             local l = headquartersLogged() return #l .. ' ' .. l[#l]"
        )),
        format!(
            "5 headquarters: Rival #1: headquarters {hq} (a construction), owned by 901; closest town 31 (Ashford): \
             on it xp +0.06, reputation recovery +0.00; the game's town script applies xp +0.06, \
             reputation recovery +0.00 there"
        )
    );
}

/// The headquarters report can never hang or slow a game: it never walks
/// the world's constructions, reads nothing more while no company has a
/// headquarters (a room just after a company is founded, 2026-10-01),
/// reads at most `REPORT_MAX` companies, and a read that fails or raises
/// only leaves its part out.
#[test]
fn the_headquarters_report_is_bounded_and_never_raises() {
    let (lua, _script) = engine();
    lua.load(
        r#"
        api.type.ComponentType.PLAYER_OWNED = 55
        api.type.ComponentType.PLAYER = 5
        api.type.ComponentType.GAME_SCRIPT = 77
        C = ug_require('tpf3mp_1::/scripts/tpf3mp/companies.lua')
        READS = 0
        -- Walking the constructions raises here: the report must not walk.
        api.engine.forEachEntityWithComponent = function() error('walked the constructions') end
        api.engine.system.gameScriptSystem = { getEntityForGameScript = function()
            READS = READS + 1 return 600 end }
        PLAYERS = {}
        api.engine.getComponent = function(e, kind)
            READS = READS + 1
            if kind == 5 then return PLAYERS[e] end
            if kind == 2 and e == 777 then error('engine refuses') end
            return nil
        end
        api.engine.system.streetConnectorSystem = {
            getConstructionClosestTown = function() error('not a construction') end }
        ROSTER = { list = {}, members = {} }
        for i = 0, 11 do
            ROSTER.list[#ROSTER.list + 1] = { id = i, entity = 100 + i, name = 'C' .. i }
            PLAYERS[100 + i] = { headquarters = -1 }
        end
        function report()
            READS = 0
            local ok, lines, why = pcall(C.headquartersReport, ROSTER, api)
            return tostring(ok) .. ' ' .. (lines and #lines or tostring(why)) .. ' ' .. READS
        end
        "#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    let eval = |code: &str| -> String {
        lua.load(code)
            .eval::<String>()
            .unwrap_or_else(|error| panic!("{code}: {error}"))
    };
    // No headquarters: one PLAYER read for each of the twelve companies,
    // and nothing else.
    assert_eq!(eval("return report()"), "true 0 12");
    // Headquarters everywhere, the engine refusing the entity: eight lines
    // at most, none raising.
    assert_eq!(
        eval(
            "for i = 0, 11 do PLAYERS[100 + i] = { headquarters = 777 } end \
             local lines = C.headquartersReport(ROSTER, api) return #lines .. '|' .. lines[1]"
        ),
        "8|C0 #0: headquarters 777 (no construction), owned by nil; closest town nil: \
         on it xp +0.00, reputation recovery +0.00; the game's town script has no state for that town"
    );
    // No roster: nil and why, never an error.
    assert_eq!(
        eval("local l, why = C.headquartersReport(nil, api) return tostring(l) .. ' ' .. why"),
        "nil no roster"
    );
}

/// In the GUI, the game's permit counts count the player's company's own
/// constructions while the room has more than one company: the
/// construction menu offers each company its headquarters until it has
/// one. With one company the game's own counts, whole world.
#[test]
fn the_guis_permits_count_the_players_company_own_constructions() {
    let lua = gui();
    lua.load(
        r#"
        ME = 901
        -- 701 is the first company's headquarters, 702 Rival's depot.
        CONS = { [701] = { owner = 25, file = 'hq.con', id = 1 }, [702] = { owner = 901, file = 'depot.con', id = 2 } }
        api = { type = { ComponentType = { PLAYER_OWNED = 55 } },
            engine = { util = { getPlayer = function() return ME end },
                getComponent = function(e, kind) if kind == 55 and CONS[e] then return { player = CONS[e].owner } end end,
                system = { streetConnectorSystem = { forEachConstructionWithMetadata = function(key, _, _, fn)
                    assert(key == 'company')
                    local list = {} for e in pairs(CONS) do list[#list + 1] = e end table.sort(list)
                    -- A headquarters names its permit in its own metadata too
                    -- (headquarter.script.tl, constructionInstance).
                    for _, e in ipairs(list) do
                        fn(e, { fileName = CONS[e].file,
                                persistentMetadata = CONS[e].id == 1 and { company = { permitKey = 'hq.res' } } or {} },
                           CONS[e].id)
                    end
                end } } },
            res = { constructionRep = { get = function(id)
                return { metadata = id == 1 and { company = { permitKey = 'hq.res' } } or {} } end } } }
        -- The game's company_util, as far as the menu reads it: counting the
        -- whole world.
        UTIL = { getActualPermitKey = function(file, meta) return meta.company and meta.company.permitKey end }
        UTIL.countUsedConstructionPermits = function()
            local used = {}
            for _, c in pairs(CONS) do
                if c.id == 1 then used['hq.res'] = (used['hq.res'] or 0) + 1 end
            end
            return used
        end
        UTIL.getConstructionDisableCacheData = function()
            return { companyRank = 1, data = { ['hq.con'] = { numBuilt = 1 }, ['hq.res'] = { numBuilt = 1 } } }
        end
        META = { getKey = function() return 'company' end,
                 constructionInstance = { get = function(m) return m and m.company end } }
        local function require_(path)
            -- The game's two names for one module.
            if path == '/game_mechanics/company/company_util.tl' then return UTIL end
            if path == '::/game_mechanics/company/company_util.tl' then return UTIL end
            if path == '/game_mechanics/company/company_metadata.tl' then return META end
            error('no ' .. path)
        end
        SEVERAL = true
        C = ug_require("tpf3mp_1::/scripts/tpf3mp/companies.lua")
        REQUIRE = require_
        OK, WHY = C.followPermits(api, require_, function() return SEVERAL end)
        function counts()
            local used = UTIL.countUsedConstructionPermits({})
            local data = UTIL.getConstructionDisableCacheData({}).data
            local function n(t, k)
                local v = t[k]
                if type(v) == 'table' then return v.numBuilt end
                return v or 0
            end
            return n(used, 'hq.res') .. ' ' .. n(data, 'hq.con') .. ' ' .. n(data, 'hq.res')
                .. ' ' .. n(data, 'depot.con')
        end
        "#,
    )
    .exec()
    .unwrap();
    let eval = |code: &str| -> String {
        lua.load(code)
            .eval::<String>()
            .unwrap_or_else(|error| panic!("{code}: {error}"))
    };
    assert_eq!(
        eval("return tostring(OK) .. ' ' .. tostring(WHY)"),
        "1 nil",
        "one table, changed once"
    );
    // A second GUI state of the same Lua state finds it changed: counted
    // as done, not as a failure.
    assert_eq!(
        eval(
            "local n, why = C.followPermits(api, REQUIRE, function() return SEVERAL end)              return tostring(n) .. ' ' .. tostring(why)"
        ),
        "1 nil"
    );
    // Rival's player: the first company's headquarters is not Rival's, so
    // Rival has used none; its depot counts.
    assert_eq!(eval("return counts()"), "0 0 0 1");
    // The first company's player: its own headquarters counts.
    assert_eq!(eval("ME = 25 return counts()"), "1 1 1 0");
    // One company in the room: the game's own counts.
    assert_eq!(eval("ME = 901 SEVERAL = false return counts()"), "1 1 1 0");
}

/// The game's window shows the room's invite code with Copy: the hook puts
/// it on the clipboard, and the button says "Copied" for a while.
#[test]
fn the_games_window_copies_the_invite_code() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    let (shown, copied, label, back): (bool, String, String, String) = lua
        .load(
            "HOOK.room = true HOOK.status = { room = 'r', invite = 'eu.example.org K7QM2X', players = {} } \
             BAR = mount(loadPlugin()) BAR.step() BAR.render() \
             views(BAR.layout)[1].params.onClick() \
             local function find(label) \
                 for _, v in ipairs(views(WINDOWS.Tpf3mpWindow.render())) do \
                     if v.view == 'Button' and v.params.content.params.text == label then return v end \
                     if v.view == 'TextView' and v.params.text == label then return v end \
                 end \
             end \
             local shown = find('Invite code  K7QM2X') ~= nil \
             find('Copy').params.onClick() \
             local label = find('Copied') and 'Copied' or 'none' \
             for _ = 1, 130 do BAR.step() end \
             return shown, HOOK.copied, label, find('Copy') and 'Copy' or 'none'",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert!(shown, "the code alone, without the server");
    assert_eq!(copied, "K7QM2X");
    assert_eq!(label, "Copied");
    assert_eq!(back, "Copy");
}

/// The game's subsidy script, as the game keeps it in a game script's
/// state (`game_mechanics/subventions/subventions.script.tl`): its offers,
/// those taken, completed and failed, each by its number and kind, with the
/// money each books (`SubventionBonusMalus`).
const FAKE_SUBSIDIES: &str = r#"
CARGO = '::/game_mechanics/subventions/deliver_cargo/deliver_cargo.res'
PASSENGERS = '::/game_mechanics/subventions/deliver_passengers/deliver_passengers.res'
local function money(amount) return { { type = 'Money', params = { amount = amount } },
                                      { type = 'Reputation', params = { amount = 0.1 } } } end
local function offer(uid, id, upfront, complete, failure)
    return { uid = uid, id = id, data = { upfront = money(upfront), complete = money(complete),
                                          failure = money(failure) } }
end
SUB = { proposedSubventions = { offer(7, CARGO, 100, 2000, 300), offer(8, CARGO, 0, 0, 0),
                                offer(9, PASSENGERS, 0, 0, 0), offer(10, CARGO, 50, 500, 400) },
        activeSubventions = {}, completedSubventions = {}, failedSubventions = {} }
api.type.ComponentType.GAME_SCRIPT = 77
api.type.JournalEntry = { new = function() return { category = {} } end,
                          Type = { LOAN = 'LOAN', INTEREST = 'INTEREST', SUBSIDY = 'SUBSIDY' } }
api.cmd.makeJournalBookAssetCmd = function(e, entry) return { journal = entry, entity = e } end
api.engine.system.gameScriptSystem = { getEntityForGameScript = function(name)
    if name == '::/game_mechanics/subventions/subventions.gs' then return 500 end return -1 end }
GAME_T = 0
api.type.ComponentType.GAME_TIME = 99
api.engine.util.getWorld = function() return 1 end
api.engine.getComponent = function(e, kind)
    if kind == 99 then return { gameTime = GAME_T } end
    if e == 500 and kind == 77 then return { state = SUB } end
end
api.util = { getDefaultMonthDuration = function() return 3000 end,
             getDefaultDayDuration = function() return 100 end }
-- The script's own events, as its handleEvent runs them: accepting moves
-- the offer to the taken and books its money up front to the save's own
-- player; declining drops it.
local function take(list, uid)
    for i, s in ipairs(list) do if s.uid == uid then return table.remove(list, i) end end
end
local send = api.cmd.sendCommand
api.cmd.sendCommand = function(command, callback)
    local e = command.event
    send(command, callback)
    if e and e.id == 'Subvention' then
        local s = take(SUB.proposedSubventions, e.param.uid)
        if s and e.name == 'onAccept' then
            SUB.activeSubventions[#SUB.activeSubventions + 1] = s
            send(api.cmd.makeJournalBookAssetCmd(25, { amount = s.data.upfront[1].params.amount,
                                                       category = { type = 'SUBSIDY' } }))
        end
    end
end
-- The script, months later: a subsidy completed or failed, its money booked
-- to the save's own player.
function FINISH(uid, how)
    local s = take(SUB.activeSubventions, uid)
    local amount = how == 'complete' and s.data.complete[1].params.amount or -s.data.failure[1].params.amount
    local list = how == 'complete' and SUB.completedSubventions or SUB.failedSubventions
    list[#list + 1] = s
    send(api.cmd.makeJournalBookAssetCmd(25, { amount = amount, category = { type = 'SUBSIDY' } }))
end
function BOOKED()
    local out = {}
    for _, c in ipairs(SENT) do
        if c.journal then out[#out + 1] = c.journal.category.type .. c.journal.amount .. '@' .. c.entity
        elseif c.event then out[#out + 1] = c.event.name .. ' ' .. tostring(c.event.param.uid)
        elseif c.addPlayer then out[#out + 1] = c.addPlayer end
    end
    SENT = {}
    return table.concat(out, ',')
end
"#;

/// The subsidy window's Accept and Decline, in the room's game: handed to
/// the room as the offer by its number and kind, never run here; an offer
/// this game no longer has is refused at the click, and says why.
#[test]
fn in_the_rooms_game_a_subsidys_answer_goes_to_the_room() {
    let lua = gui();
    // Mechanics fixture only: production refuses this channel pending game acceptance.
    lua.load("ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').subsidies = true")
        .exec()
        .unwrap();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load(
        "M = mount(loadPlugin()) M.step() HOOK.room = true \
         api.type = api.type or {} api.type.ComponentType = { GAME_SCRIPT = 77 } \
         api.engine = api.engine or {} api.engine.system = api.engine.system or {} \
         api.engine.system.gameScriptSystem = { getEntityForGameScript = function(name) \
             if name == '::/game_mechanics/subventions/subventions.gs' then return 500 end return -1 end } \
         api.engine.getComponent = function(e, kind) \
             if e == 500 and kind == 77 then return { state = { \
                 proposedSubventions = { { uid = 7, id = 'cargo.res' } }, \
                 activeSubventions = { { uid = 3, id = 'cargo.res' } } } } end end \
         api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Subvention', 'onAccept', { uid = 7 })) \
         api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Subvention', 'onDecline', { uid = 7 })) \
         api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Subvention', 'onAccept', { uid = 3 })) \
         M.step()",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let (sent, handed): (usize, usize) = lua.load("return #SENT, #HOOK.commands").eval().unwrap();
    assert_eq!(sent, 0, "not run here: the room orders it for every game");
    assert_eq!(handed, 2, "handed to the room, through the schema");
    let answers: String = lua
        .load(
            "local a, d = HOOK.commands[1].Subsidy.Accept, HOOK.commands[2].Subsidy.Decline \
             return a.uid .. ' ' .. a.kind .. ' | ' .. d.uid .. ' ' .. d.kind",
        )
        .eval()
        .unwrap();
    assert_eq!(answers, "7 cargo.res | 7 cargo.res");
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged
            .iter()
            .any(|l| l.contains("makeScriptingSendEventCmd")
                && l.ends_with("a subsidy no longer offered")),
        "{logged:?}"
    );
    // The room refuses an answer (another company took it first): the
    // player is told why.
    lua.load(
        "HOOK.results = { { ticket = 1, ok = false, why = 'the subsidy was taken already, by Rival' } } \
         M.step() M.render()",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        shown(&lua).unwrap_or_default(),
        "Taking the subsidy: not done, the subsidy was taken already, by Rival"
    );
}

/// Every game answers a subsidy offer alike: the first company in the
/// room's order to accept it takes it through the game's own event, the
/// money goes to that company (moved on from the save's own player, to whom
/// the game's script books it), and every later answer is refused, naming
/// who took it. Months later its reward, or its penalty, follows it.
#[test]
fn every_game_gives_a_subsidy_to_the_first_company_to_accept_it() {
    let (lua, _script) = engine();
    // Mechanics fixture only: production refuses this channel pending game acceptance.
    lua.load("ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').subsidies = true")
        .exec()
        .unwrap();
    lua.load(FAKE_SUBSIDIES).exec().unwrap();
    lua.load(
        r#"
        A, B = string.rep("a", 64), string.rep("b", 64)
        HOOK.batch = { { CompanyOp = { Create = { name = 'Rival' } } } } HOOK.origins = { A }
        UPDATE({}, STATE, 0.2)
        FOUNDED = BOOKED()
        local function ref(uid, kind) return { uid = uid, kind = kind } end
        -- A (Rival) and B (the first company) both accept 7 in one step,
        -- A first; B accepts 8 under the wrong kind, declines 9 and
        -- accepts 10; A then answers 9 too.
        HOOK.batch = { { Subsidy = { Accept = ref(7, CARGO) } }, { Subsidy = { Accept = ref(7, CARGO) } },
                       { Subsidy = { Accept = ref(8, PASSENGERS) } }, { Subsidy = { Decline = ref(9, PASSENGERS) } },
                       { Subsidy = { Accept = ref(10, CARGO) } }, { Subsidy = { Accept = ref(9, PASSENGERS) } } }
        HOOK.origins = { A, B, B, B, B, A }
        UPDATE({}, STATE, 0.2)
        "#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let applied: Vec<String> = lua
        .load(
            "local out = {} for _, a in ipairs(HOOK.applied) do \
                 out[#out + 1] = tostring(a.ok) .. (a.why and (' ' .. a.why) or '') end return out",
        )
        .eval()
        .unwrap();
    assert_eq!(
        &applied[1..],
        [
            "true",
            "false the subsidy was taken already, by Rival",
            "false the subsidy under that number is another one",
            "true",
            "true",
            "false the subsidy is no longer offered",
        ]
    );
    let booked: String = lua.load("return BOOKED()").eval().unwrap();
    assert_eq!(
        booked,
        "onAccept 7,SUBSIDY100@25,SUBSIDY-100@25,SUBSIDY100@901,onDecline 9,onAccept 10,SUBSIDY50@25",
        "Rival's 100 up front moved on to it; the first company's 50 stays its own"
    );
    // A day later nothing has finished: nothing moves. Then the script
    // completes 7, Rival's, and 10, the first company's: Rival gets its
    // reward on the next day, once.
    lua.load(
        "HOOK.room = true \
         GAME_T = 100 UPDATE({}, STATE, 0.2) \
         FINISH(7, 'complete') FINISH(10, 'complete') BOOKED() \
         GAME_T = 150 UPDATE({}, STATE, 0.2) \
         GAME_T = 200 UPDATE({}, STATE, 0.2) \
         GAME_T = 300 UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let booked: String = lua.load("return BOOKED()").eval().unwrap();
    assert_eq!(booked, "SUBSIDY-2000@25,SUBSIDY2000@901");
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged
            .iter()
            .any(|l| l == "subsidy 7 completed for Rival: 2000"),
        "{logged:?}"
    );
    // A completed subsidy is still its taker's: answering it again says so.
    // Rival takes 11, which fails: its penalty, booked to the first company
    // by the script, is Rival's.
    lua.load(
        "SUB.proposedSubventions[#SUB.proposedSubventions + 1] = { uid = 11, id = CARGO, \
             data = { upfront = { { type = 'Money', params = { amount = 0 } } }, \
                      complete = { { type = 'Money', params = { amount = 700 } } }, \
                      failure = { { type = 'Money', params = { amount = 250 } } } } } \
         HOOK.batch = { { Subsidy = { Accept = { uid = 7, kind = CARGO } } }, \
                        { Subsidy = { Accept = { uid = 11, kind = CARGO } } } } \
         HOOK.origins = { B, A } \
         UPDATE({}, STATE, 0.2) BOOKED() \
         FINISH(11, 'fail') BOOKED() \
         GAME_T = 400 UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let last: Vec<String> = lua
        .load(
            "local n = #HOOK.applied \
             return { tostring(HOOK.applied[n - 1].why), tostring(HOOK.applied[n].ok) }",
        )
        .eval()
        .unwrap();
    assert_eq!(last, ["the subsidy is completed already", "true"]);
    let booked: String = lua.load("return BOOKED()").eval().unwrap();
    assert_eq!(booked, "SUBSIDY250@25,SUBSIDY-250@901");
    let kept: usize = lua
        .load("return #STATE.value.companies.subsidies")
        .eval()
        .unwrap();
    assert_eq!(
        kept, 2,
        "7 and 10, completed, kept; 11, failed and settled, forgotten"
    );
    let founded: String = lua.load("return FOUNDED").eval().unwrap();
    assert_eq!(founded, "Rival");
}

/// Stand-ins for the base game's subsidy kinds, as their scripts count
/// progress (game_mechanics/subventions/*/*.script.tl, build 40408):
/// deliver_cargo counts each cargo delivered to its industry by the line
/// that carried it, and once completed doubles those lines' tickets;
/// deliver_passengers completes on a person starting a line between its
/// towns. Lines 31 and 33 are Rival's (901), 32 the first company's (25).
const FAKE_SUBSIDY_KINDS: &str = r#"
CT = { PLAYER_OWNED = 61, SIM_ENTITY_AT_VEHICLE = 62, SIM_ENTITY_AT_TERMINAL = 63 }
OWNERS = { [31] = 901, [32] = 25, [33] = 901 }
AT_VEHICLE = { [701] = 31, [702] = 32 }
FAKE_API = {
    type = { ComponentType = CT },
    engine = { getComponent = function(e, kind)
        if kind == CT.PLAYER_OWNED and OWNERS[e] then return { player = OWNERS[e] } end
        if kind == CT.SIM_ENTITY_AT_VEHICLE and AT_VEHICLE[e] then return { line = AT_VEHICLE[e] } end
    end },
}
local cargo = { handleEvent = function(_src, id, name, param, s)
    if name ~= 'OnCalcTicketPrice' then return end
    local changed, found = {}, false
    for i = 1, #param do
        local p = param[i]
        if s.completedTime then
            if s.data.affectedLines and s.data.affectedLines[p.lineEntity] then changed[i] = 2 found = true end
        elseif s.acceptedTime and p.stockListEntity == s.data.industry then
            s.data.affectedLines = s.data.affectedLines or {}
            s.data.affectedLines[p.lineEntity] = true
            s.data.delivered = (s.data.delivered or 0) + 1
        end
    end
    if found then return changed end
end, isComplete = function(s) return (s.data.delivered or 0) >= s.data.toDeliver end }
local passengers = { handleEvent = function(_src, _id, name, param, s)
    if name ~= 'OnStartedLineUsage' then return end
    for _, entry in ipairs(param.entities) do
        s.data.lineEntity = FAKE_API.engine.getComponent(entry[1], CT.SIM_ENTITY_AT_VEHICLE).line
    end
end }
local workers = { isComplete = function() return true end }
GAME_MODULES = {
    ['::/game_mechanics/subventions/deliver_cargo/deliver_cargo.script.tl'] = { deliver_cargo = cargo },
    ['::/game_mechanics/subventions/deliver_cargo_town/deliver_cargo_town.script.tl'] =
        { deliver_cargo_town = cargo },
    ['::/game_mechanics/subventions/deliver_passengers/deliver_passengers.script.tl'] =
        { deliver_passengers = passengers },
    ['::/game_mechanics/subventions/deliver_workers/deliver_workers.script.tl'] = { deliver_workers = workers },
}
SUBSIDIES = ug_require('tpf3mp_1::/scripts/tpf3mp/subsidies.lua')
KINDS = SUBSIDIES.wrapAll(ug_require, FAKE_API)
-- One cargo delivery at industry 50 by each of `lines`.
function DELIVER(s, ...)
    local params = {}
    for i, line in ipairs({ ... }) do params[i] = { stockListEntity = 50, lineEntity = line } end
    return KINDS.deliver_cargo.handleEvent('', 'TransportVehicleSystem', 'OnCalcTicketPrice', params, s)
end
"#;

/// While subsidies are a room's channel, the mod's run script points the
/// base game's subsidy resources at the mod's wrapper of their scripts, and
/// leaves every other resource, and another mod's subsidy, as it is; while
/// they are not, it changes nothing.
#[test]
fn the_run_script_points_the_base_subsidies_at_the_mods_wrapper() {
    let lua = gui();
    let run = |lua: &Lua| -> Vec<String> {
        lua.load(
            "MODIFIERS = {} \
             function addModifier(kind, fn) MODIFIERS[#MODIFIERS + 1] = { kind = kind, fn = fn } end \
             local source = mod_source('mod.script.lua') \
             assert(loadstring(source, '@mod.script.lua'))() \
             data().runFn({}, {})",
        )
        .exec()
        .unwrap_or_else(|error| panic!("{error}"));
        lua.load(
            "local out = {} for _, m in ipairs(MODIFIERS) do out[#out + 1] = m.kind end return out",
        )
        .eval()
        .unwrap()
    };
    // Subsidies passed two-game acceptance: the mod as shipped points them.
    assert_eq!(run(&lua), ["loadGameRes"]);
    lua.load("ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').subsidies = false")
        .exec()
        .unwrap();
    assert!(
        run(&lua).is_empty(),
        "subsidies are refused in a room: the game's own scripts stay"
    );
    lua.load("ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').subsidies = true")
        .exec()
        .unwrap();
    assert_eq!(run(&lua), ["loadGameRes"]);
    let scripts: Vec<String> = lua
        .load(
            "local redirect = MODIFIERS[1].fn \
             local function res(t, ref) return { type = t, data = { scriptFile = ref, icons = {} } } end \
             local base = '::/game_mechanics/subventions/' \
             return { \
                 redirect('a.res.lua', res('subvention', base .. 'deliver_passengers/deliver_passengers.script@deliver_passengers')).data.scriptFile, \
                 redirect('b.res.lua', res('subvention', base .. 'deliver_workers/deliver_workers.script@deliver_workers')).data.scriptFile, \
                 redirect('c.res.lua', res('subvention', 'other_mod::/mine.script@mine')).data.scriptFile, \
                 redirect('d.res.lua', res('loan', base .. 'deliver_cargo/deliver_cargo.script@deliver_cargo')).data.scriptFile, \
                 tostring(redirect('e.res.lua', nil)) }",
        )
        .eval()
        .unwrap();
    assert_eq!(
        scripts,
        [
            "tpf3mp_1::/tpf3mp_sim/subsidies.script@deliver_passengers",
            "tpf3mp_1::/tpf3mp_sim/subsidies.script@deliver_workers",
            "other_mod::/mine.script@mine",
            "::/game_mechanics/subventions/deliver_cargo/deliver_cargo.script@deliver_cargo",
            "nil",
        ]
    );
    // The wrapper script hands the game each base kind under the name the
    // resource reaches it by, every function the kind's own but its
    // handleEvent.
    lua.load(FAKE_SUBSIDY_KINDS).exec().unwrap();
    let kinds: Vec<String> = lua
        .load(
            "assert(loadstring(mod_source('tpf3mp_sim/subsidies.script.lua'), '@subsidies.script.lua'))() \
             local kinds = data() \
             local out = {} \
             for _, name in ipairs(SUBSIDIES.ORDER) do \
                 local k = kinds[name] \
                 out[#out + 1] = name .. ':' .. tostring(k ~= nil and k.handleEvent ~= nil) \
                     .. ':' .. tostring(k and k.isComplete == GAME_MODULES[SUBSIDIES.KINDS[name].module][name].isComplete) \
             end return out",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        kinds,
        [
            "deliver_cargo:true:true",
            "deliver_cargo_town:true:true",
            "deliver_passengers:true:true",
            "deliver_workers:true:true",
        ]
    );
}

/// A subsidy a company took counts that company's transport only: the
/// room's accept names the taker, the subsidy keeps it in its own data, and
/// its kind hears only the deliveries and passengers of the taker's lines;
/// the double tickets it pays once completed land on the event's own
/// entries. A subsidy with no taker counts everyone's, as the game does.
#[test]
fn a_taken_subsidy_counts_its_takers_transport_only() {
    let lua = gui();
    lua.load(FAKE_SUBSIDY_KINDS).exec().unwrap();
    let counted: Vec<String> = lua
        .load(
            r#"
            local out = {}
            -- Rival (901) takes cargo subsidy 7: the accept the room sends
            -- reaches the subsidy's kind with the taker.
            local s = { uid = 7, acceptedTime = 10, data = { industry = 50, toDeliver = 2 } }
            KINDS.deliver_cargo.handleEvent('', 'Subvention', 'onAccept', { uid = 7, tpf3mpCompany = 901 }, s)
            -- Another subsidy's accept names no taker for this one.
            KINDS.deliver_cargo.handleEvent('', 'Subvention', 'onAccept', { uid = 8, tpf3mpCompany = 25 }, s)
            out[#out + 1] = 'taker ' .. tostring(SUBSIDIES.taker(s))
            -- The first company's line 32 delivers twice, Rival's 31 once.
            DELIVER(s, 32, 31, 32)
            out[#out + 1] = 'delivered ' .. s.data.delivered .. ' complete ' .. tostring(KINDS.deliver_cargo.isComplete(s))
            DELIVER(s, 33)
            out[#out + 1] = 'delivered ' .. s.data.delivered .. ' complete ' .. tostring(KINDS.deliver_cargo.isComplete(s))
            -- Completed: Rival's lines' cargo pays double, by the event's
            -- own entries.
            s.completedTime = 20
            local doubled = DELIVER(s, 32, 31, 32, 33)
            local keys = {}
            for i, m in pairs(doubled) do keys[#keys + 1] = i .. '=' .. m end
            table.sort(keys)
            out[#out + 1] = 'doubled ' .. table.concat(keys, ',')
            -- A subsidy nobody took through the room counts every line.
            local plain = { uid = 9, acceptedTime = 10, data = { industry = 50, toDeliver = 2 } }
            DELIVER(plain, 32, 31)
            out[#out + 1] = 'plain ' .. plain.data.delivered
            -- Passengers: a person on the first company's line does not
            -- connect Rival's towns; one on Rival's does.
            local p = { uid = 11, acceptedTime = 10, data = {} }
            KINDS.deliver_passengers.handleEvent('', 'Subvention', 'onAccept', { uid = 11, tpf3mpCompany = 901 }, p)
            KINDS.deliver_passengers.handleEvent('', 'SimPersonSystem', 'OnStartedLineUsage', { entities = { { 702 } } }, p)
            out[#out + 1] = 'line ' .. tostring(p.data.lineEntity)
            KINDS.deliver_passengers.handleEvent('', 'SimPersonSystem', 'OnStartedLineUsage', { entities = { { 702 }, { 701 } } }, p)
            out[#out + 1] = 'line ' .. tostring(p.data.lineEntity)
            return out
            "#,
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        counted,
        [
            "taker 901",
            "delivered 1 complete false",
            "delivered 2 complete true",
            "doubled 2=2,4=2",
            "plain 2",
            "line nil",
            "line 31",
        ]
    );
}

/// The money of a subsidy whose taker is gone is no one's: the first
/// company, to whom the game's script books it, ends with nothing of it,
/// neither the reward nor the penalty.
#[test]
fn a_gone_takers_subsidy_leaves_the_first_company_with_nothing() {
    let (lua, _script) = engine();
    lua.load("ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').subsidies = true")
        .exec()
        .unwrap();
    lua.load(FAKE_SUBSIDIES).exec().unwrap();
    lua.load(
        r#"
        A = string.rep("a", 64)
        HOOK.batch = { { CompanyOp = { Create = { name = 'Rival' } } } } HOOK.origins = { A }
        UPDATE({}, STATE, 0.2) BOOKED()
        -- Rival takes 7 and 10; the accept names Rival's player entity.
        HOOK.batch = { { Subsidy = { Accept = { uid = 7, kind = CARGO } } },
                       { Subsidy = { Accept = { uid = 10, kind = CARGO } } } }
        HOOK.origins = { A, A }
        ACCEPTED = {}
        for _, c in ipairs(SENT) do end
        UPDATE({}, STATE, 0.2)
        for _, c in ipairs(SENT) do
            if c.event and c.event.name == 'onAccept' then
                ACCEPTED[#ACCEPTED + 1] = c.event.param.uid .. '@' .. tostring(c.event.param.tpf3mpCompany)
            end
        end
        BOOKED()
        -- Then Rival is gone; the script completes 7 and fails 10.
        for _, c in ipairs(STATE.value.companies.list) do if c.name == 'Rival' then c.gone = true end end
        FINISH(7, 'complete') FINISH(10, 'fail')
        "#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let accepted: Vec<String> = lua.load("return ACCEPTED").eval().unwrap();
    assert_eq!(accepted, ["7@901", "10@901"]);
    let script: String = lua.load("return BOOKED()").eval().unwrap();
    assert_eq!(
        script, "SUBSIDY2000@25,SUBSIDY-400@25",
        "the script books both to the first company"
    );
    lua.load("HOOK.room = true GAME_T = 100 UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let settled: String = lua.load("return BOOKED()").eval().unwrap();
    assert_eq!(
        settled, "SUBSIDY-2000@25,SUBSIDY400@25",
        "the first company gives the reward back and gets the penalty back"
    );
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    for said in [
        "subsidy 7 completed for Rival, gone: its reward of 2000 is no one's",
        "subsidy 10 failed for Rival, gone: its penalty of 400 is no one's",
    ] {
        assert!(logged.iter().any(|l| l == said), "{said}: {logged:?}");
    }
    // Settled for good: nothing moves again.
    lua.load("GAME_T = 200 UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let again: String = lua.load("return BOOKED()").eval().unwrap();
    assert_eq!(again, "");
}

/// The economy lane carries every company's balance and the subsidy
/// script's offers with their terms: two games whose offers differ, in a
/// number, a kind or a sum, split there at the next checkpoint, and a dump
/// lists each subsidy.
#[test]
fn the_economy_lane_carries_the_companies_and_the_subsidy_offers() {
    let (lua, _) = engine();
    lua.load(FAKE_WORLD).exec().unwrap();
    let before = read_lanes(&lua);
    lua.load(
        r#"
        local CT = api.type.ComponentType
        CT.GAME_SCRIPT = 77
        WORLD[CT.ACCOUNT][901] = { balance = 500 }
        SUB = { lastSpawnTime = 1200, spawnIntervalModifier = 1,
                proposedSubventions = { { uid = 53396, id = 'deliver_passengers.res', spawnTime = 1200,
                    data = { upfront = { { type = 'Money', params = { amount = 4000 } } },
                             expireDurationProposed = 18000 } } },
                activeSubventions = {}, completedSubventions = {}, failedSubventions = {} }
        MODSTATE = { companies = { list = { { id = 0, entity = 25 }, { id = 1, entity = 901 } } } }
        api.engine.system.gameScriptSystem = { getEntityForGameScript = function(name)
            if name == '::/game_mechanics/subventions/subventions.gs' then return 500 end
            if name == 'tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs' then return 501 end
            return -1 end }
        local get = api.engine.getComponent
        api.engine.getComponent = function(e, kind)
            if kind == 77 and e == 500 then return { state = SUB } end
            if kind == 77 and e == 501 then return { state = MODSTATE } end
            return get(e, kind)
        end
        "#,
    )
    .exec()
    .unwrap();
    let with = read_lanes(&lua);
    for (a, b) in before.iter().zip(&with) {
        assert_eq!(a.0 == 4, a.1 != b.1, "lane {}", a.0);
    }
    assert!(
        with[4]
            .1
            .starts_with("25:1234567 companies 0=25:1234567,1=901:500 subsidies 1:"),
        "{}",
        with[4].1
    );
    // Another game's offer: the same number, another sum.
    lua.load("SUB.proposedSubventions[1].data.upfront[1].params.amount = 4100")
        .exec()
        .unwrap();
    let other = read_lanes(&lua);
    assert_ne!(other[4], with[4]);
    // Another company's balance.
    lua.load("SUB.proposedSubventions[1].data.upfront[1].params.amount = 4000 WORLD[api.type.ComponentType.ACCOUNT][901].balance = 501")
        .exec()
        .unwrap();
    assert_ne!(read_lanes(&lua)[4], with[4]);
    let dump: Vec<String> = lua
        .load("return ug_require('tpf3mp_1::/scripts/tpf3mp/lanes.lua').dump(api, 4, nil)")
        .eval()
        .unwrap();
    assert!(
        dump.iter().any(|l| l.contains(
            "offered 53396 deliver_passengers.res spawn=1200 accepted=- completed=- upfront=4000 \
             complete=0 failure=0 deliver=- delivered=- lapses=18000 taker=-"
        )),
        "{dump:#?}"
    );
    assert!(
        dump.iter()
            .any(|l| l.contains("last=1200 modifier=1 pause=false")),
        "{dump:#?}"
    );
}

/// The game's Lua 5.2 formats `%d` through a 32-bit C `long` on Windows
/// and raises past 2^31 (docs/BIGMAPS.md); the tests' Lua does not.
const GAME_FORMAT: &str = r#"
local format = string.format
string.format = function(f, ...)
    local args, i = { ... }, 0
    for _, conv in f:gmatch('%%([-+ #0-9.]*)([%a%%])') do
        if conv ~= '%' then
            i = i + 1
            local n = tonumber(args[i])
            if (conv == 'd' or conv == 'i') and n and (n >= 2^31 or n < -2^31) then
                error("bad argument #" .. (i + 1) .. " to 'format' (not a number in proper range)")
            end
        end
    end
    return format(f, ...)
end
"#;

/// Offer numbers are drawn past 2^31 in a real game, and balances grow
/// past it: the economy lane still reads them, as the game's own Lua
/// formats numbers, instead of failing the rolling world check, which held
/// the world and refused every action (a playtest on 2026-10-12).
#[test]
fn the_economy_lane_reads_offer_numbers_and_balances_past_2_31() {
    let (lua, _) = engine();
    lua.load(FAKE_WORLD).exec().unwrap();
    lua.load(GAME_FORMAT).exec().unwrap();
    lua.load(
        r#"
        local CT = api.type.ComponentType
        CT.GAME_SCRIPT = 77
        WORLD[CT.ACCOUNT][25] = { balance = 3000000000 }
        WORLD[CT.ACCOUNT][901] = { balance = -2500000000 }
        SUB = { lastSpawnTime = 1200, spawnIntervalModifier = 1,
                proposedSubventions = { { uid = 2999999999, id = 'deliver_passengers.res', spawnTime = 1200,
                    data = { upfront = { { type = 'Money', params = { amount = 4000 } } },
                             expireDurationProposed = 18000 } } },
                activeSubventions = {}, completedSubventions = {}, failedSubventions = {} }
        MODSTATE = { companies = { list = { { id = 0, entity = 25 }, { id = 1, entity = 901 } } } }
        api.engine.system.gameScriptSystem = { getEntityForGameScript = function(name)
            if name == '::/game_mechanics/subventions/subventions.gs' then return 500 end
            if name == 'tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs' then return 501 end
            return -1 end }
        local get = api.engine.getComponent
        api.engine.getComponent = function(e, kind)
            if kind == 77 and e == 500 then return { state = SUB } end
            if kind == 77 and e == 501 then return { state = MODSTATE } end
            return get(e, kind)
        end
        "#,
    )
    .exec()
    .unwrap();
    let lanes = read_lanes(&lua);
    assert!(
        lanes[4]
            .1
            .starts_with("25:3000000000 companies 0=25:3000000000,1=901:-2500000000 subsidies 1:"),
        "{}",
        lanes[4].1
    );
    let dump: Vec<String> = lua
        .load("return ug_require('tpf3mp_1::/scripts/tpf3mp/lanes.lua').dump(api, 4, nil)")
        .eval()
        .unwrap();
    assert!(
        dump.iter().any(|l| l.contains(
            "offered 2999999999 deliver_passengers.res spawn=1200 accepted=- completed=- upfront=4000 \
             complete=0 failure=0 deliver=- delivered=- lapses=18000 taker=-"
        )),
        "{dump:#?}"
    );
}

/// The mod's game script says the subsidy script's offers in the log at a
/// checkpoint whenever they changed, each with its number, kind and terms:
/// two games' logs show where their offers part.
#[test]
fn the_game_script_logs_the_subsidy_offers_when_they_change() {
    let (lua, _script) = engine();
    lua.load(FAKE_SUBSIDIES).exec().unwrap();
    lua.load(
        "HOOK.checkpoint = true UPDATE({}, STATE, 0.2) \
         HOOK.checkpoint = true UPDATE({}, STATE, 0.2) \
         SUB.proposedSubventions[1].spawnTime = 1200 \
         HOOK.checkpoint = true UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    let heads: Vec<&String> = logged
        .iter()
        .filter(|l| l.starts_with("subsidies at game time "))
        .collect();
    assert_eq!(
        heads.len(),
        2,
        "said once, then once more on a change: {logged:?}"
    );
    assert!(
        logged.iter().any(|l| l
            == "subsidy: offered 7 ::/game_mechanics/subventions/deliver_cargo/deliver_cargo.res \
                spawn=1200 accepted=- completed=- upfront=100 complete=2000 failure=300 deliver=- \
                delivered=- lapses=- taker=-"),
        "{logged:?}"
    );
}

/// The construction menu's perk tools, Industry Greenification and the
/// marketing campaign, as they send the company script their events
/// (`industry_greenify_tool.script.tl`, `marketing_campaign_tool.script.tl`).
const MAKE_GREEN: &str = "{ companyEntity = 25, constructionEntity = 931, \
    permitKey = 'ECO_INDUSTRY' }";
const MARKETING: &str = "{ townEntity = 7, companyEntity = 25, \
    marketingParams = { durationMs = 1095000, lineCostFactor = 0.5 }, \
    permitKey = '::/game_mechanics/company/permitKeys/marketing.res' }";

/// Both perk tools were refused in a room ("the Companies script's MakeGreen
/// event"): they go to the room, the industry by its id and the town by
/// its, with the campaign's price, and the tool's own booking of that price
/// after it is neither sent nor refused, since every game books it.
#[test]
fn the_perk_tools_go_to_the_room_by_industry_and_town() {
    let lua = gui();
    // Mechanics fixture only: production refuses this channel pending game acceptance.
    lua.load("ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').perks = true")
        .exec()
        .unwrap();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    // Town 7 is town-3; the coal mine, construction 930 with its industry
    // part 931, is industry-2. Construction 940 has two industries.
    lua.load(
        "api.cmd.makeJournalBookAssetCmd = function(player, entry, at) \
             return { kind = 'journal', player = player, amount = entry.amount } end \
         api.type = { ComponentType = { GAME_SCRIPT = 7, CONSTRUCTION = 2, INDUSTRY = 13 } } \
         api.engine = { \
             util = { getPlayer = function() return 25 end, getYear = function() return 1960 end }, \
             getComponent = function(e, kind) \
                 if kind == 7 and e == 77 then return { state = { registry = { \
                     vehicles = { next = 0, bound = {} }, lines = { next = 0, bound = {} }, \
                     groups = { next = 0, bound = {} }, towns = { next = 4, bound = { { 3, 7 } } }, \
                     industries = { next = 3, bound = { { 2, 930 }, { 1, 940 } } } } } } end \
                 if kind == 2 and e == 930 then return { industries = { 931 } } end \
                 if kind == 2 and e == 940 then return { industries = { 941, 942 } } end \
             end, \
             system = { \
                 gameScriptSystem = { getEntityForGameScript = function(name) \
                     if name == 'tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs' then return 77 end return -1 end }, \
                 streetConnectorSystem = { getConstructionEntityForSubconstruction = function(part) \
                     if part == 931 then return 930 end \
                     if part == 941 then return 940 end \
                     return -1 end }, \
             }, \
         } \
         M = mount(loadPlugin()) M.step() HOOK.room = true",
    )
    .exec()
    .unwrap();
    lua.load(format!(
        "local function ev(name, p, cb) \
             api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Companies', name, p), cb) end \
         ev('MakeGreen', {MAKE_GREEN}) \
         ev('startMarketingCampaign', {MARKETING}, function() \
             UNLOCKED = true \
             api.cmd.sendCommand(api.cmd.makeJournalBookAssetCmd(25, {{ amount = -10000000 }}, nil)) \
         end) \
         M.step()"
    ))
    .exec()
    .unwrap();
    let (sent, handed): (usize, usize) = lua.load("return #SENT, #HOOK.commands").eval().unwrap();
    assert_eq!(
        (sent, handed),
        (0, 2),
        "not run here: the room orders both for every game"
    );
    let perks: String = lua
        .load(
            "local g = HOOK.commands[1].Perk.Greenify local m = HOOK.commands[2].Perk.Marketing \
             return table.concat({ g.industry, g.permit, m.town, m.duration_ms, m.line_cost_factor, \
                 m.permit, string.format('%d', m.cost) }, '|')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        perks,
        "2|ECO_INDUSTRY|3|1095000|0.5|::/game_mechanics/company/permitKeys/marketing.res|10000000"
    );
    // This game ran the campaign: the tool hears so, gives back its
    // permits, and its own booking of the price goes nowhere.
    lua.load("HOOK.results = { { ticket = 2, ok = true } } M.step()")
        .exec()
        .unwrap();
    let (unlocked, sent): (bool, usize) =
        lua.load("return UNLOCKED == true, #SENT").eval().unwrap();
    assert!(unlocked);
    assert_eq!(sent, 0, "the room's action books the price, not the tool");
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        !logged.iter().any(|l| l.contains("makeJournalBookAssetCmd")),
        "{logged:?}"
    );
    // A booking outside the tool's callback is still refused.
    lua.load(
        "api.cmd.sendCommand(api.cmd.makeJournalBookAssetCmd(25, { amount = -1 }, nil)) M.step()",
    )
    .exec()
    .unwrap();
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged.iter().any(|l| l.contains("makeJournalBookAssetCmd")),
        "{logged:?}"
    );

    // Another company's perk, an industry the room cannot tell from the
    // other of its construction, and a town it cannot name are refused.
    lua.load(format!(
        "local function ev(name, p) \
             api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Companies', name, p)) end \
         local a = {MAKE_GREEN} a.companyEntity = 26 ev('MakeGreen', a) \
         local b = {MAKE_GREEN} b.constructionEntity = 941 ev('MakeGreen', b) \
         local c = {MARKETING} c.townEntity = 8 ev('startMarketingCampaign', c) \
         M.step()"
    ))
    .exec()
    .unwrap();
    let handed: usize = lua.load("return #HOOK.commands").eval().unwrap();
    assert_eq!(handed, 2);
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    for why in [
        "greenifying for another company",
        "an industry the room cannot name",
        "a town the room cannot name",
    ] {
        assert!(
            logged
                .iter()
                .any(|l| l.contains("makeScriptingSendEventCmd") && l.ends_with(why)),
            "{why}: {logged:?}"
        );
    }
}

/// The marketing tool's price, as the game's tool computes it.
#[test]
fn a_marketing_campaign_costs_what_the_tool_charges() {
    let lua = gui();
    let costs: Vec<i64> = lua
        .load(
            "local c = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
             return { c.marketingCost(1850), c.marketingCost(1900), c.marketingCost(1960), \
                 c.marketingCost(2020), c.marketingCost(2050) }",
        )
        .eval()
        .unwrap();
    assert_eq!(
        costs,
        [4_000_000, 4_000_000, 10_000_000, 25_000_000, 25_000_000]
    );
}

/// Every game uses a perk through the company script's own event, for the
/// acting company, and books a campaign's price to it, as the tool does;
/// one the company cannot pay for is refused in every game.
#[test]
fn every_game_uses_a_perk_through_the_company_scripts_own_event() {
    let (lua, _script) = engine();
    lua.load("ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').perks = true")
        .exec()
        .unwrap();
    lua.load(FAKE_TOWNS).exec().unwrap();
    lua.load(
        "CONS[930].industries = { 931 } \
         BALANCE = 50000000 \
         api.engine.util.finance = { getPlayersBalance = function(p) return BALANCE end } \
         api.type.JournalEntry = { new = function() return { category = {} } end, Type = { OTHER = 7 } } \
         api.cmd.makeJournalBookAssetCmd = function(player, entry, at) \
             return { journal = { player = player, amount = entry.amount, kind = entry.category.type, \
                 time = entry.time } } end",
    )
    .exec()
    .unwrap();
    // The first update binds town 5 as town-0, 7 as town-1, and the coal
    // mine's construction 930 as industry-0.
    lua.load(
        "HOOK.room = true UPDATE({}, STATE, 0.2) \
         local m = { town = 1, duration_ms = 1095000, line_cost_factor = 0.5, permit = 'm.res', cost = 10000000 } \
         HOOK.batch = { { Perk = { Greenify = { industry = 0, permit = 'ECO_INDUSTRY' } } }, \
             { Perk = { Marketing = m } }, { Perk = { Greenify = { industry = 5 } } } } \
         UPDATE({}, STATE, 0.2) \
         BALANCE = 9999999 \
         HOOK.batch = { { Perk = { Marketing = m } } } \
         UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let sent: String = lua
        .load(
            "local out = {} for _, c in ipairs(SENT) do \
                 if c.event then \
                     local p = c.event.param \
                     out[#out + 1] = table.concat({ c.event.id, c.event.name, p.companyEntity, \
                         tostring(p.constructionEntity or p.townEntity), tostring(p.permitKey), \
                         tostring(p.marketingParams and p.marketingParams.durationMs), \
                         tostring(p.marketingParams and p.marketingParams.lineCostFactor) }, ':') \
                 elseif c.journal then \
                     local j = c.journal \
                     out[#out + 1] = table.concat({ 'journal', j.player, string.format('%d', j.amount), \
                         j.kind, j.time }, ':') \
                 end end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        sent,
        "Companies:MakeGreen:25:931:ECO_INDUSTRY:nil:nil \
         Companies:startMarketingCampaign:25:7:m.res:1095000:0.5 \
         journal:25:-10000000:7:-1"
    );
    let applied: String = lua
        .load(
            "local out = {} for _, a in ipairs(HOOK.applied) do \
                 out[#out + 1] = tostring(a.ok) .. ':' .. tostring(a.why) end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        applied,
        "true:nil true:nil false:no industries 5 in this world \
         false:not enough money for the campaign"
    );
    lua.load(r#"
        local before = #SENT
        local function reject_balance(reader)
            api.engine.util.finance.getPlayersBalance = reader
            HOOK.batch = { { Perk = { Marketing = { town = 1, duration_ms = 1095000,
                line_cost_factor = 0.5, permit = 'm.res', cost = 10000000 } } } }
            UPDATE({}, STATE, 0.2)
            local result = HOOK.applied[#HOOK.applied]
            assert(not result.ok and result.why:find('cannot read the company balance'), tostring(result.why))
            assert(#SENT == before, 'an unreadable balance must not spend a permit or book money')
        end
        reject_balance(function() error('unavailable') end)
        reject_balance(function() return nil end)
        reject_balance(function() return 0/0 end)
        reject_balance(function() return math.huge end)
    "#).exec().unwrap();
}

/// Both the sender and replay refuse new channels with production defaults.
#[test]
fn unaccepted_ports_cannot_be_sent_or_replayed() {
    let (lua, _) = engine();
    lua.load(r#"
        HOOK.room = true
        local bridge = ug_require('tpf3mp_1::/scripts/tpf3mp/bridge.lua')
        local apply = ug_require('tpf3mp_1::/scripts/tpf3mp/apply.lua')
        local link = assert(bridge.attach(bridge.find()))
        local actions = {
            { Preserve = { building = { file = 'b.con', at = { x = 0, y = 0, z = 0 } }, index = 0, preserved = true } },
            { Perk = { Greenify = { industry = 0 } } },
            { Perk = { Marketing = { town = 0, duration_ms = 1, line_cost_factor = 0.5, cost = 1 } } },
            { SetCrossingType = { nodes = { { x = 0, y = 0, z = 0 } }, crossing_type = 'c.rcr' } },
        }
        for _, action in ipairs(actions) do
            local sent, reason = link:command(action)
            assert(not sent and reason:find('awaits two%-player game acceptance'), tostring(reason))
            local applied, why = apply.run(action, {})
            assert(not applied and why:find('awaits two%-player game acceptance'), tostring(why))
        end
        assert(#HOOK.commands == 0)
        -- Rename/recolour, subsidies and line waypoints passed two-player
        -- acceptance; the crossing channel remains gated pending ordinary-
        -- start two-player acceptance.
        local accepted = {
            { CreateLine = { line = { stops = { { waypoints = { {} } } } } } },
            { EditLine = { line = 1, change = { Update = { stops = { { waypoints = { {} } } } } } } },
            { Rename = { what = { Vehicle = 1 }, name = 'x' } },
            { VehicleOp = { vehicle = 1, change = { Recolor = { r = 1, g = 0, b = 0 } } } },
            { Subsidy = { Accept = { uid = 1, kind = 'x' } } },
            { Subsidy = { Decline = { uid = 1, kind = 'x' } } },
        }
        for _, action in ipairs(accepted) do
            local sent, reason = link:command(action)
            assert(sent or not tostring(reason):find('awaits two%-player game acceptance'), tostring(reason))
            local applied, why = apply.run(action, {})
            assert(applied or not tostring(why):find('awaits two%-player game acceptance'), tostring(why))
        end
    "#).exec().unwrap();
}

/// A part whose loads the action leaves out gets the store's own, one for
/// each of the model's compartments: the game throws for a part with fewer.
/// One that names some but not all is refused in every game.
#[test]
fn a_bought_vehicle_loads_every_compartment_of_its_model() {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    lua.load(
        "api.res.modelRep.get = function(id) \
             return { metadata = { transportVehicle = { compartments = { {}, {} } } } } end",
    )
    .exec()
    .unwrap();
    let no_loads = BUY_BUS.replace("loads = { { config = 0, cargo = 3 } }", "loads = { }");
    lua.load(format!(
        "HOOK.room = true UPDATE({{}}, STATE, 0.2) \
         HOOK.batch = {{ {no_loads}, {BUY_BUS} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    let outcome: String = lua
        .load(
            "local p = SENT[1].buy.config.vehicles[1].part \
             local out = { #SENT, #p.compartment2loadConfig, p.compartment2loadConfig[2].loadConfigIndex, \
                 tostring(HOOK.applied[1].ok), tostring(HOOK.applied[2].ok), tostring(HOOK.applied[2].why) } \
             return table.concat(out, '|')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        outcome,
        "1|2|0|true|false|vehicle/bus/city.mdl has 2 compartments, and the part loads 1"
    );
}

#[test]
fn hud_answers_are_chunked_without_overwriting_unread_results() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(
        r#"
        local hud = ug_require('tpf3mp_1::/scripts/tpf3mp/hudguard.lua')
        local link = ug_require('tpf3mp_1::/scripts/tpf3mp/bridge.lua').attach(tpf3mp_native)
        local tickets, results = {}, {}
        for i = 1, hud.MAX_PENDING do
            local t = 8000000000000000 + i
            tickets[i] = string.format('%.0f', t)
            results[i] = { ticket = t, ok = true, entity = 8000000000000000 + i }
        end
        link:note(hud.TICKETS, table.concat(tickets, ','))
        local first = hud.forward(link, results)
        assert(first > 0 and first < #results, 'force multiple batches at the real note limit: ' .. first .. '/' .. #results .. ' tickets=' .. tostring(link:note(hud.TICKETS)))
        local unread = link:note(hud.ANSWERS)
        assert(#unread <= hud.NOTE_MAX)
        assert(hud.forward(link, {}) == 0)
        assert(link:note(hud.ANSWERS) == unread, 'unread answers must not be replaced')
        local seen, count = {}, 0
        while #tickets > 0 do
            local batch = link:note(hud.ANSWERS)
            assert(#batch <= hud.NOTE_MAX)
            local n = 0
            for t in batch:gmatch('(%d+) %d [%-%d]+;') do
                assert(not seen[t], 'delivered twice')
                seen[t] = true; count = count + 1; n = n + 1
            end
            assert(n > 0, 'queued results must continue without new incoming results')
            local remaining = {}
            for _, t in ipairs(tickets) do if not seen[t] then remaining[#remaining + 1] = t end end
            tickets = remaining
            link:note(hud.TICKETS, #tickets > 0 and table.concat(tickets, ',') or '0')
            hud.forward(link, {})
        end
        assert(count == hud.MAX_PENDING)
        assert(link:note(hud.ANSWERS) == '0')
    "#,
    )
    .exec()
    .unwrap();
}

/// In the GUI state the game scripts' GUI half runs in, where the game's
/// company script checks a construction's permits for `getPlayer()`, the
/// player's company answers getPlayer once the room's tools propose a
/// build: a founded company's headquarters was refused there (no preview,
/// nothing placed) by the save's player's rank and the whole world's
/// headquarters (2026-10-01).
#[test]
fn the_game_scripts_gui_state_acts_for_the_players_company() {
    let (lua, _script) = engine();
    lua.load(
        r#"
        JAMES = string.rep("a", 64)
        ROSTER = { next = 2,
            list = { { id = 0, entity = 25, name = "First", color = { 0.8, 0.16, 0.12 } },
                     { id = 1, entity = 901, name = "Rival", color = { 0.13, 0.42, 0.85 }, founder = JAMES } },
            members = { { player = JAMES, company = 1 } } }
        api.type.ComponentType.GAME_SCRIPT = 7
        api.engine.system.gameScriptSystem = { getEntityForGameScript = function(name)
            if name == "tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs" then return 77 end return -1 end }
        local get = api.engine.getComponent
        api.engine.getComponent = function(e, kind)
            if e == 77 and kind == 7 then return { state = { companies = ROSTER } } end
            if get then return get(e, kind) end
        end
        HOOK.room = true
        HOOK.status = { room = "r", me_id = JAMES, players = { { name = "james", id = JAMES, me = true } } }
        BEFORE = api.engine.util.getPlayer()
        SCRIPT.guiHandleEvent({}, nil, nil, '', 'constructionBuilder', 'builder.proposalCreate', {})
        AFTER = api.engine.util.getPlayer()
        SCRIPT.guiHandleEvent({}, nil, nil, '', 'constructionBuilder', 'builder.proposalCreate', {})
        "#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}"));
    let (before, after): (u32, u32) = lua.load("return BEFORE, AFTER").eval().unwrap();
    assert_eq!((before, after), (25, 901));
    let logged: String = lua
        .load("return table.concat(HOOK.logged, '|')")
        .eval()
        .unwrap();
    assert_eq!(
        logged
            .matches("the game scripts' GUI state: getPlayer follows the player's company")
            .count(),
        1,
        "once a state: {logged}"
    );
}

/// The map's height cells, 4 m, and a stroke of the raise tool as the hook
/// reads it at the click: 3 by 2 cells from cell (-10, 7), each `{ 100 + i,
/// 100 }`.
const TERRAIN_STROKE: &str = r#"
api.engine.terrain = { getBaseResolution = function() return { x = 4, y = 4, z = 0.0625 } end }
STROKE = { terrain = { x0 = -10, y0 = 7, width = 3, height = 2,
                       cells = { 100, 100, 101, 100, 102, 100, 103, 100, 104, 100, 105, 100 } } }
"#;

#[test]
fn a_terrain_tools_click_goes_to_the_room_as_terraform_actions() {
    let (lua, _script) = engine();
    lua.load(FAKE_STATION).exec().unwrap();
    lua.load(TERRAIN_STROKE).exec().unwrap();
    // A construction tool's preview before the click, then the raise
    // tool's click, whose stroke only the hook saw: the stroke's wins.
    lua.load(format!(
        "HOOK.room = true HOOK.clicks = 0 SCRIPT.guiUpdate({{}}, nil, nil) \
         SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'constructionBuilder', 'builder.proposalCreate', \
             {{ {CONSTRUCTION_PROPOSAL} }}) \
         HOOK.built[0] = {{ proposal = STROKE }} \
         HOOK.clicks = 1 SCRIPT.guiUpdate({{}}, nil, nil) \
         HOOK.built[1] = {{ why = 'terrain tool: the asset brush: the room does not carry it yet' }} \
         HOOK.clicks = 2 SCRIPT.guiUpdate({{}}, nil, nil)"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let carried: String = lua
        .load(
            "local t = HOOK.commands[1].Terraform \
             return table.concat({ #HOOK.commands, t.origin.x, t.origin.y, t.cell, t.columns, #t.cells, \
                 t.cells[1].target, t.cells[1].before, t.cells[6].target }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        carried, "1|-40|28|4|3|6|100|100|105",
        "the stroke, and nothing else"
    );
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    for line in [
        "terraform handed to the room: 3 by 2 cells of 4 m from cell (-10, 7), 5 changed, heights 100.00 to 105.00 m",
        "stopped a build the room cannot carry: terrain tool: the asset brush: the room does not carry it yet [terrain tool]",
    ] {
        assert!(logged.iter().any(|l| l == line), "{line}: {logged:?}");
    }
}

#[test]
fn a_stroke_larger_than_one_action_goes_in_bands_of_whole_rows() {
    let (lua, _script) = engine();
    lua.load(TERRAIN_STROKE).exec().unwrap();
    // 100 by 50 cells: 40 rows (4,000 cells), then 10.
    lua.load(
        "local cells = {} \
         for i = 1, 100 * 50 do cells[2 * i - 1] = 200 + (i % 7) * 0.25 cells[2 * i] = 200 end \
         HOOK.room = true HOOK.clicks = 0 SCRIPT.guiUpdate({}, nil, nil) \
         HOOK.built[0] = { proposal = { terrain = { x0 = 5, y0 = -20, width = 100, height = 50, \
             cells = cells } } } \
         HOOK.clicks = 1 SCRIPT.guiUpdate({}, nil, nil)",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let bands: String = lua
        .load(
            "local out = {} \
             for _, a in ipairs(HOOK.commands) do \
                 local t = a.Terraform \
                 out[#out + 1] = t.origin.x .. ',' .. t.origin.y .. ',' .. #t.cells \
             end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(bands, "20,-80,4000 20,80,1000");
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged.iter().any(|l| l.ends_with("(part 2 of 2)")
            && l.contains("100 by 10 cells of 4 m from cell (5, 20)")),
        "{logged:?}"
    );
}

/// A painter's stroke as the hook hands it at the click: one Paint action
/// of 4 by 2 texels from (-40, 28), material 3 on five of them, and its
/// mask.
const PAINT: &str = "{ Paint = { materials = { x0 = -40, y0 = 28, columns = 4, rows = 2, runs = { \
    { value = 255, count = 1 }, { value = 3, count = 5 }, { value = 255, count = 2 } } }, \
    mask = { x0 = -40, y0 = 28, columns = 4, rows = 2, runs = { \
    { value = 0, count = 1 }, { value = 1, count = 5 }, { value = 0, count = 2 } } } } }";

#[test]
fn a_terrain_painters_click_goes_to_the_room_as_paint_actions() {
    let (lua, _script) = engine();
    lua.load(format!(
        "HOOK.room = true HOOK.clicks = 0 SCRIPT.guiUpdate({{}}, nil, nil) \
         HOOK.built[0] = {{ proposal = {{ paint = {{ {PAINT}, {PAINT} }} }} }} \
         HOOK.clicks = 1 SCRIPT.guiUpdate({{}}, nil, nil)"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let carried: String = lua
        .load(
            "local p = HOOK.commands[1].Paint \
             return table.concat({ #HOOK.commands, p.materials.x0, p.materials.columns, \
                 #p.materials.runs, p.mask.runs[2].count }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(carried, "2|-40|4|3|5", "both bands, as the hook made them");
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    let line = "paint handed to the room: 4 by 2 texels from (-40, 28), 5 painted, a mask with 5 set (part 2 of 2)";
    assert!(logged.iter().any(|l| l == line), "{line}: {logged:?}");
}

#[test]
fn every_game_applies_paint_through_the_hook_as_the_players_build() {
    let (lua, _script) = engine();
    lua.load(FAKE_TERRAIN_HOOK).exec().unwrap();
    lua.load(format!(
        "HOOK.batch = {{ {PAINT} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let armed: String = lua
        .load(
            "local g = HOOK.grids[1] \
             return table.concat({ #HOOK.grids, g.materials.x0, g.materials.rows, #g.mask.runs, \
                 #SENT, tostring(SENT[1].proposal.carrier), SENT[1].context.player, tostring(HOOK.armed) }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        armed, "1|-40|2|3|1|true|25|nil",
        "the stroke armed, the carrier sent and paid, disarmed"
    );
    let (ok, logged): (bool, Vec<String>) = lua
        .load("return HOOK.applied[1].ok, HOOK.logged")
        .eval()
        .unwrap();
    assert!(ok, "{logged:?}");
    assert!(
        logged
            .iter()
            .any(|l| l
                == "paint applied: 4 by 2 texels from (-40, 28), 5 painted, a mask with 5 set"),
        "{logged:?}"
    );
    // A carrier no build took fails in every game.
    lua.load(format!(
        "NO_FILL = true HOOK.batch = {{ {PAINT} }} UPDATE({{}}, STATE, 0.2) NO_FILL = false"
    ))
    .exec()
    .unwrap();
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    let line = "action 1 of this step was not applied: the hook filled no build with the paint";
    assert!(logged.iter().any(|l| l == line), "{line}: {logged:?}");
}

/// The hook's `terrain`, as the game script uses it: the grid armed, and
/// whether a build was filled with it, which the stand-in game's
/// sendCommand says of the carrier it is sent.
const FAKE_TERRAIN_HOOK: &str = r#"
HOOK.grids = {}
tpf3mp_native.terrain = function(grid)
    if grid == nil then
        local armed, filled = HOOK.armed, HOOK.filled
        HOOK.armed, HOOK.filled = nil, nil
        if armed == nil then return nil end
        return filled == true
    end
    HOOK.armed = grid
    HOOK.grids[#HOOK.grids + 1] = grid
    return true
end
api.type.Proposal = { new = function() return { carrier = true } end }
local send = api.cmd.sendCommand
api.cmd.sendCommand = function(command, callback)
    if HOOK.armed and not NO_FILL and type(command.proposal) == 'table' and command.proposal.carrier then
        HOOK.filled = true
    end
    return send(command, callback)
end
"#;

/// The 3 by 2 stroke as the room orders it.
const TERRAFORM: &str = "{ Terraform = { origin = { x = -40, y = 28 }, cell = 4, columns = 3, cells = { \
    { target = 100, before = 100 }, { target = 101, before = 100 }, { target = 102, before = 100 }, \
    { target = 103, before = 100 }, { target = 104, before = 100 }, { target = 105, before = 100 } } } }";

#[test]
fn every_game_applies_a_terraform_through_the_hook_as_the_players_build() {
    let (lua, _script) = engine();
    lua.load(TERRAIN_STROKE).exec().unwrap();
    lua.load(FAKE_TERRAIN_HOOK).exec().unwrap();
    lua.load(format!(
        "HOOK.batch = {{ {TERRAFORM} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let armed: String = lua
        .load(
            "local g = HOOK.grids[1] \
             return table.concat({ #HOOK.grids, g.x0, g.y0, g.width, g.height, #g.cells, g.cells[1], \
                 g.cells[2], g.cells[11], g.cells[12] }, '|')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(armed, "1|-10|7|3|2|12|100|100|105|100");
    let sent: String = lua
        .load(
            "local c = SENT[1] \
             return table.concat({ #SENT, tostring(c.proposal.carrier), tostring(c.playerInitiated), \
                 tostring(c.ignoreErrors), c.context.player, tostring(HOOK.armed) }, '|')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        sent, "1|true|true|true|25|nil",
        "the carrier, paid by the player, then disarmed"
    );
    let (ok, logged): (bool, Vec<String>) = lua
        .load("return HOOK.applied[1].ok, HOOK.logged")
        .eval()
        .unwrap();
    assert!(ok, "{logged:?}");
    assert!(
        logged.iter().any(|l| l
            == "terraform applied: 3 by 2 cells from cell (-10, 7), heights 100.00 to 105.00 m"),
        "{logged:?}"
    );
}

#[test]
fn a_terraform_no_build_took_or_of_another_grid_fails_in_every_game() {
    let (lua, _script) = engine();
    lua.load(TERRAIN_STROKE).exec().unwrap();
    lua.load(FAKE_TERRAIN_HOOK).exec().unwrap();
    // The hook filled nothing.
    lua.load(format!(
        "NO_FILL = true HOOK.batch = {{ {TERRAFORM} }} UPDATE({{}}, STATE, 0.2) NO_FILL = false"
    ))
    .exec()
    .unwrap();
    // A map of 8 m cells.
    lua.load(format!(
        "api.engine.terrain.getBaseResolution = function() return {{ x = 8, y = 8 }} end \
         HOOK.batch = {{ {TERRAFORM} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    // A hook without `terrain`.
    lua.load(format!(
        "tpf3mp_native.terrain = nil \
         api.engine.terrain.getBaseResolution = function() return {{ x = 4, y = 4 }} end \
         HOOK.batch = {{ {TERRAFORM} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    for line in [
        "action 1 of this step was not applied: the hook filled no build with the grid",
        "action 1 of this step was not applied: a grid of 4 m cells; this map's are 8",
        "action 1 of this step was not applied: the hook would not take the grid: this hook cannot apply a terraform",
    ] {
        assert!(logged.iter().any(|l| l == line), "{line}: {logged:?}");
    }
    let armed: Option<String> = lua.load("return HOOK.armed").eval().unwrap();
    assert_eq!(armed, None, "disarmed after every try");
}

/// The tram track tool on the country street 8-9: the edge rebuilt in place
/// with a tram track in its lane, a noise barrier, locked and owned.
const TRAM_PROPOSAL: &str = "{ toAdd = {}, toRemove = {}, proposal = { addedNodes = {}, \
    addedSegments = { { entity = -1, type = 0, playerOwned = { player = 25 }, comp = { \
        node0 = 8, node1 = 9, type = 0, typeIndex = -1, \
        tangent0 = { x = 0, y = 80, z = 0 }, tangent1 = { x = 0, y = 80, z = 0 }, \
        roadTemplate = '::/street/country.street_template', roadStyle = '', objects = {}, \
        edgeDecorations = { { 3, false } }, roadDevelopmentLocked = true, \
        laneConfigs = { { speed = 22.22, width = 2, height = 0, offset = -1, forward = false, \
            transportModes = { [0] = true, [14] = true } } } } } }, \
    removedSegments = { { entity = 100, type = 0, comp = { node0 = 8, node1 = 9, objects = {} } } }, \
    removedNodes = {}, edgeObjectsToAdd = {} } }";

#[test]
fn an_upgrade_handed_to_the_room_is_said_in_the_log() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(format!(
        "api.res.edgeDecorationRep = {{ getName = function(i) \
             if i == 3 then return '::/infrastructure/edge_addons/barrier_b.edge' end end }} \
         HOOK.room = true HOOK.clicks = 0 SCRIPT.guiUpdate({{}}, nil, nil) \
         local r = SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'streetTrackModifier', 'builder.proposalCreate', \
             {{ {TRAM_PROPOSAL} }}) \
         assert(r == nil, 'refused') \
         HOOK.clicks = 1 SCRIPT.guiUpdate({{}}, nil, nil)"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    let said = "upgrade handed to the room: street upgrade of 1 edge(s) rebuilt in place; \
                template ::/street/country.street_template; 1 lane(s) carrying PERSON TRAM_TRACK; \
                lane speeds 22.22 to 22.22; decorations ::/infrastructure/edge_addons/barrier_b.edge; \
                locked 1, owned 1";
    assert!(logged.iter().any(|l| l == said), "{logged:?}");
    let handed: usize = lua.load("return #HOOK.commands").eval().unwrap();
    assert_eq!(handed, 1);
}

/// Town buildings for the Historic Preservation tests: town building 501
/// stands in construction 500, which the game names for it; 512, the
/// second of construction 510's, only that construction's list names.
const FAKE_TOWN_BUILDINGS: &str = r#"
api.type = api.type or {}
api.type.ComponentType = api.type.ComponentType or {}
api.type.ComponentType.CONSTRUCTION = 2
CONS = {
    [500] = { fileName = 'town/res_1.con', townBuildings = { 501 },
              transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 100.5, -20.25, 3, 1 } },
    [510] = { fileName = 'town/com_2.con', townBuildings = { 511, 512 },
              transf = { 1,0,0,0, 0,1,0,0, 0,0,1,0, 300, 40, 5, 1 } },
}
api.engine = api.engine or {}
api.engine.getEntitiesWithComponent = function(kind)
    if kind == 2 then return { 500, 510 } end
    return {}
end
api.engine.getComponent = function(e, kind)
    if kind == 2 then return CONS[e] end
end
api.engine.system = api.engine.system or {}
api.engine.system.streetConnectorSystem = { getConstructionEntityForSubconstruction = function(part)
    if part == 501 then return 500 end
    return -1
end }
"#;

/// A town building's Historic Preservation checkbox was refused in a room:
/// it goes to the room, the building by its construction and its place
/// there, whether the game names that construction or only its list does.
#[test]
fn historic_preservation_goes_to_the_room_by_its_construction() {
    let lua = gui();
    // Mechanics fixture only: production refuses this channel pending game acceptance.
    lua.load("ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').preservation = true")
        .exec()
        .unwrap();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load(
        "api.cmd.makeTownBuildingSetBlockedDevelopmentCmd = function(e, on) \
             return { kind = 'preserve', entity = e, on = on } end",
    )
    .exec()
    .unwrap();
    lua.load(FAKE_TOWN_BUILDINGS).exec().unwrap();
    lua.load(
        "M = mount(loadPlugin()) M.step() HOOK.room = true \
         local function preserve(e, on) \
             api.cmd.sendCommand(api.cmd.makeTownBuildingSetBlockedDevelopmentCmd(e, on), function() end) end \
         preserve(501, true) preserve(512, false) preserve(999, true) \
         M.step()",
    )
    .exec()
    .unwrap();
    let (sent, handed): (usize, usize) = lua.load("return #SENT, #HOOK.commands").eval().unwrap();
    assert_eq!(
        (sent, handed),
        (0, 2),
        "not run here: the room orders it for every game"
    );
    let carried: String = lua
        .load(
            "local out = {} for _, c in ipairs(HOOK.commands) do local p = c.Preserve \
                 out[#out + 1] = table.concat({ p.building.file, p.building.at.x, p.building.at.y, \
                     p.index, tostring(p.preserved) }, ':') end \
             return table.concat(out, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        carried,
        "town/res_1.con:100.5:-20.25:0:true town/com_2.con:300:40:1:false"
    );
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged
            .iter()
            .any(|l| l.contains("makeTownBuildingSetBlockedDevelopmentCmd")
                && l.ends_with("a town building the room cannot name")),
        "{logged:?}"
    );
}

#[test]
fn calendar_pause_and_pace_go_through_the_room_and_replay_in_both_games() {
    use tpf3mp_proto::{
        action::Action,
        lua::{action_from_lua, action_to_lua},
    };
    let source = gui();
    source.load(FAKE_HOOK).exec().unwrap();
    source.load(FAKE_CMD).exec().unwrap();
    source
        .load(
            r#"
        api.cmd.makeGameSetCalendarSpeedCmd = function(value) return { calendar = value } end
        M = mount(loadPlugin()) M.step() HOOK.room = true
        for _, value in ipairs({ 0, 8000, 4000, 2000, 1000, 500, 0, 2000 }) do
            api.cmd.sendCommand(api.cmd.makeGameSetCalendarSpeedCmd(value))
        end
        for _, value in ipairs({ -1, 0.5, 2147483648, math.huge, 0/0, '2000' }) do
            api.cmd.sendCommand(api.cmd.makeGameSetCalendarSpeedCmd(value))
        end
        M.step()
        assert(#SENT == 0, 'nothing may change locally before room replay')
        assert(#HOOK.commands == 8, 'invalid values must not reach the room')
    "#,
        )
        .exec()
        .unwrap();
    let actions: Vec<mlua::Value> = source.load("return HOOK.commands").eval().unwrap();
    for _ in 0..2 {
        let (replica, _script) = engine();
        replica
            .load(
                r#"
            CALENDAR = {}
            api.cmd.makeGameSetCalendarSpeedCmd = function(value) return { calendar = value } end
            local original = api.cmd.sendCommand
            api.cmd.sendCommand = function(cmd, ...)
                if cmd.calendar ~= nil then CALENDAR[#CALENDAR + 1] = cmd.calendar end
                return original(cmd, ...)
            end
            HOOK.room = true UPDATE({}, STATE, 0.2)
        "#,
            )
            .exec()
            .unwrap();
        for captured in &actions {
            let action = action_from_lua(&common::tree(captured)).unwrap();
            let decoded = Action::from_payload(&action.to_payload().unwrap()).unwrap();
            replica
                .globals()
                .set(
                    "ACTION",
                    common::value(&replica, &action_to_lua(&decoded).unwrap()),
                )
                .unwrap();
            replica
                .load("HOOK.batch = { ACTION }; UPDATE({}, STATE, 0.2)")
                .exec()
                .unwrap();
        }
        replica.load("assert(#HOOK.applied == 8, table.concat(HOOK.logged, '|')); for _, answer in ipairs(HOOK.applied) do assert(answer.ok, answer.why) end").exec().unwrap();
        let values: Vec<u32> = replica.load("return CALENDAR").eval().unwrap();
        assert_eq!(
            values,
            [0, 8000, 4000, 2000, 1000, 500, 0, 2000],
            "{}",
            log(&replica)
        );
        replica
            .load("for _, answer in ipairs(HOOK.applied) do assert(answer.ok, answer.why) end")
            .exec()
            .unwrap();
    }
    source
        .load(
            r#"
        HOOK.room = false
        api.cmd.sendCommand(api.cmd.makeGameSetCalendarSpeedCmd(2000))
        assert(#SENT == 1 and SENT[1].command.calendar == 2000, 'single player stays native')
    "#,
        )
        .exec()
        .unwrap();
}

/// Every game sets the town building at that place in the construction's
/// list, through the game's own command; one no longer there is refused
/// in every game.
#[test]
fn every_game_preserves_the_same_town_building() {
    let (lua, _script) = engine();
    lua.load("ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').preservation = true")
        .exec()
        .unwrap();
    lua.load(FAKE_TOWN_BUILDINGS).exec().unwrap();
    lua.load(
        "api.cmd.makeTownBuildingSetBlockedDevelopmentCmd = function(e, on) \
             return { preserve = { entity = e, on = on } } end \
         HOOK.room = true UPDATE({}, STATE, 0.2) \
         HOOK.batch = { \
             { Preserve = { building = { file = 'town/com_2.con', at = { x = 300.4, y = 40, z = 5 } }, \
                 index = 1, preserved = true } }, \
             { Preserve = { building = { file = 'town/res_1.con', at = { x = 100.5, y = -20.25, z = 3 } }, \
                 index = 0, preserved = false } }, \
             { Preserve = { building = { file = 'town/res_1.con', at = { x = 100.5, y = -20.25, z = 3 } }, \
                 index = 1, preserved = true } }, \
             { Preserve = { building = { file = 'town/res_1.con', at = { x = 900, y = 0, z = 0 } }, \
                 index = 0, preserved = true } } } \
         UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap();
    let done: String = lua
        .load(
            "local out = {} for _, c in ipairs(SENT) do if c.preserve then \
                 out[#out + 1] = c.preserve.entity .. ':' .. tostring(c.preserve.on) end end \
             local why = {} for _, a in ipairs(HOOK.applied) do \
                 why[#why + 1] = tostring(a.ok) .. ':' .. tostring(a.why) end \
             return table.concat(out, ' ') .. ' | ' .. table.concat(why, ' ')",
        )
        .eval()
        .unwrap();
    assert_eq!(
        done,
        "512:true 501:false | true:nil true:nil \
         false:no town building 1 in the town/res_1.con \
         false:no town/res_1.con there"
    );
}

const CONFIG_WORDS: &str = r#"
function CONFIG_WORDS(configs, nodeAt, edgeAt)
    local out = {}
    for _, c in ipairs(configs) do
        local comp = c.comp
        local turns = {}
        for _, l in ipairs(comp.laneConnections) do
            turns[#turns + 1] = edgeAt(l.segment0) .. '#' .. l.lane0 .. '->' .. edgeAt(l.segment1) .. '#'
                .. l.lane1 .. (l.withRoad and 'r' or '') .. (l.withTram and 't' or '')
        end
        local walks = {}
        for _, e in ipairs(comp.crosswalks) do walks[#walks + 1] = edgeAt(e) end
        local phases = {}
        for _, s in ipairs(comp.trafficLightConfig.states) do
            phases[#phases + 1] = '[' .. table.concat(s.lockedLanes, ',') .. string.format(' %g/%g', s.duration,
                s.minDuration) .. (s.canSkip and ' skip' or '') .. ']'
        end
        out[#out + 1] = nodeAt(c.entity) .. ' tl' .. tostring(comp.trafficLightPreference) .. ' type'
            .. tostring(comp.trafficLightConfig.trafficLightType) .. ' ' .. table.concat(phases)
            .. ' dss=' .. tostring(comp.doubleSlipSwitch == true)
            .. ' um=' .. tostring(comp.userModifiedLaneConnections == true)
            .. '/' .. tostring(comp.userModifiedTrafficLightStates == true)
            .. ' turns ' .. table.concat(turns, ' ') .. ' walks ' .. table.concat(walks, ' ')
    end
    return table.concat(out, ' || ')
end
function PLACE(p) return string.format('(%g,%g)', p.x, p.y) end
function ENDS(a, b) a, b = PLACE(a), PLACE(b) if a > b then a, b = b, a end return a .. '-' .. b end
-- The tool's proposal, placed: its own new nodes and edges, else the world's.
function TOOL_WORDS(p)
    local s = p.proposal
    local nodes, edges = {}, {}
    for _, n in ipairs(s.addedNodes) do nodes[n.entity] = n.comp.position end
    local function nodeAt(e) return PLACE(nodes[e] or NODES[e]) end
    for _, e in ipairs(s.addedSegments) do edges[e.entity] = e.comp end
    local function edgeAt(e)
        local c = edges[e] or EDGES[e]
        return ENDS(nodes[c.node0] or NODES[c.node0], nodes[c.node1] or NODES[c.node1])
    end
    return CONFIG_WORDS(s.nodeConfigsToAdd, nodeAt, edgeAt)
end
-- What a replay sent, placed the same way.
function SENT_WORDS(sent)
    local s = sent.proposal.streetProposal
    local nodes, edges = {}, {}
    for _, n in ipairs(s.nodesToAdd) do nodes[n.entity] = n.comp.position end
    local function nodeAt(e) return PLACE(nodes[e] or NODES[e]) end
    for _, e in ipairs(s.edgesToAdd) do edges[e.entity] = e.comp end
    local function edgeAt(e)
        local c = edges[e] or EDGES[e]
        return ENDS(nodes[c.node0] or NODES[c.node0], nodes[c.node1] or NODES[c.node1])
    end
    return CONFIG_WORDS(s.nodeConfigsToAdd or {}, nodeAt, edgeAt)
end
"#;

fn hook_log(lua: &Lua) -> String {
    lua.load("return table.concat(HOOK.logged or {}, '\\n')")
        .eval()
        .unwrap_or_default()
}

#[test]
fn the_simulation_notes_the_save_player_for_native_company_tools() {
    let (lua, _) = engine();
    lua.load("api.engine.util.getPlayer = function() return 214443 end UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let noted: String = lua
        .load("return HOOK.notes['tpf3mp.player']")
        .eval()
        .unwrap();
    assert_eq!(noted, "214443");
}

/// The game's finance window reads the loan script's state, which keeps
/// the room's first company's loans only. In the GUI it shows a player of
/// another company that company's own loans and the offers it can take; a
/// player of the first company sees the loan script's own, as before. The
/// simulation's view of the loan script is never changed.
#[test]
fn the_finance_window_shows_a_founded_companys_own_loans() {
    let lua = gui();
    lua.load(FAKE_HOOK).exec().unwrap();
    lua.load(FAKE_CMD).exec().unwrap();
    lua.load(
        r#"
        ME = string.rep("b", 64)
        ROSTER = { next = 2, nextLoan = 3,
                   list = { { id = 0, entity = 25, name = "First", color = { 1, 0, 0 } },
                            { id = 1, entity = 901, name = "Rival", color = { 0, 0, 1 } } },
                   members = {},
                   loanOffers = { { company = 1, availableLoans = {
                       { type = "Small", amount = 5000000, duration = 3000, percentage = 0.03 },
                       { type = "Medium", amount = 5000, duration = 6000, percentage = 0.05 },
                   } } },
                   loans = { { id = 2, company = 1, amount = 1200, remaining = 1100, months = 12, paid = 1,
                               duration = 12000, percentage = 0.12, lastPayDay = 1000,
                               accounting = "native-v1", payment = 112, type = "Small" },
                             { id = 1, company = 7, amount = 99, remaining = 99, months = 1, paid = 0,
                               rate = 0, payment = 99 } } }
        LOANS = { availableLoans = { { type = "Small", amount = 5000000, duration = 3000, percentage = 0.03 },
                                     { type = "Medium", cooldownUntil = 5000 } },
                  obtainedLoans = { { id = 0 }, { id = 1 }, { id = 2 }, { id = 3 } }, freeId = 4 }
        api.engine = api.engine or {}
        api.engine.util = { getPlayer = function() return 25 end }
        api.engine.system = api.engine.system or {}
        api.engine.system.gameScriptSystem = { getEntityForGameScript = function(name)
            if name == "tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs" then return 77 end
            if name == "::/game_mechanics/finance/loan.gs" then return 40 end
            return -1 end }
        api.type = api.type or {}
        api.type.ComponentType = api.type.ComponentType or {}
        api.type.ComponentType.GAME_SCRIPT = 7
        api.util = api.util or {}
        api.util.getDefaultMonthDuration = function() return 1000 end
        api.engine.getComponent = function(e, kind)
            if e == 77 and kind == 7 then return { state = { companies = ROSTER } } end
            if e == 40 and kind == 7 then return { state = LOANS } end
        end
        local realGetComponent = api.engine.getComponent
        function FRESH_API()
            return { engine = { util = { getPlayer = function() return 25 end },
                                system = api.engine.system, getComponent = realGetComponent },
                     type = api.type, util = api.util, cmd = api.cmd }
        end
        HOOK.status = { room = "r", players = { { name = "b", id = ME, me = true, connected = true } }, me_id = ME }
        function BOARD()
            local s = api.engine.getComponent(api.engine.system.gameScriptSystem.getEntityForGameScript(
                "::/game_mechanics/finance/loan.gs"), api.type.ComponentType.GAME_SCRIPT).state
            local out = {}
            for _, l in ipairs(s.availableLoans) do out[#out + 1] = l.type .. ":" .. tostring(l.amount or l.cooldownUntil) end
            out[#out + 1] = "|"
            for _, l in ipairs(s.obtainedLoans) do
                out[#out + 1] = tostring(l.id) .. ":" .. tostring(l.amount) .. ":" .. tostring(l.duration) .. ":"
                    .. tostring(l.percentage) .. ":" .. tostring(l.timesPaid)
            end
            return table.concat(out, " ")
        end
        "#,
    )
    .exec()
    .unwrap();
    run_frames(&lua, 20);
    let board: String = lua.load("return BOARD()").eval().unwrap();
    assert_eq!(
        board,
        "Small:5000000 Medium:5000 | 0:nil:nil:nil:nil 1:nil:nil:nil:nil 2:nil:nil:nil:nil 3:nil:nil:nil:nil",
        "the first company's player: the loan script's own"
    );
    lua.load("ROSTER.members = { { player = ME, company = 1 } }")
        .exec()
        .unwrap();
    run_frames(&lua, 20);
    let board: String = lua.load("return BOARD()").eval().unwrap();
    assert_eq!(
        board, "Small:5000000 Medium:5000 | 2:1200:12000:0.12:1",
        "Rival's player: Rival's one loan, the offers it can take"
    );
    // The active getComponent override supplies the actual terms to the
    // game's native GUI arithmetic (loan_util.tl, 109-110 and 169-175).
    // Its displayed monthly installment and Repay affordability amount
    // must agree with the stored schedule, rather than an annuity facade.
    let finance_math = lua
        .load(
            "local l = api.engine.getComponent(40, 7).state.obtainedLoans[1] \
             local n = math.floor(l.duration / 1000 + 0.5) \
             local principal = math.floor(l.amount / n + 0.5) \
             local payment = math.floor((l.amount / n) * (1 + l.percentage) + 0.5) \
             local payoff = (n - l.timesPaid) * principal + l.amount - n * principal \
             return payment, payoff, ROSTER.loans[1].payment, ROSTER.loans[1].remaining",
        )
        .eval::<(i64, i64, i64, i64)>()
        .unwrap();
    assert_eq!(finance_math, (112, 1100, 112, 1100));
    let refreshed: String = lua
        .load(
            "api = FRESH_API(); assert(package.loaded['tpf3mp.follow'].ensure(api)); return BOARD()",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        refreshed, "Small:5000000 Medium:5000 | 2:1200:12000:0.12:1",
        "a refreshed GUI api still shows Rival's loans"
    );
    let unchanged: bool = lua
        .load(
            "return LOANS.availableLoans[2].cooldownUntil == 5000 and #LOANS.obtainedLoans == 4 and ROSTER.loans[1].id == 2 and ROSTER.loans[1].paid == 1",
        )
        .eval()
        .unwrap();
    assert!(
        unchanged,
        "reading the finance window does not change simulation state"
    );
    // The window's Repay of it goes to the room as Rival's, by its id and
    // amount, which every game's companies.repay takes.
    lua.load(
        "HOOK.room = true \
         local s = api.engine.getComponent(40, 7).state \
         api.cmd.sendCommand(api.cmd.makeScriptingSendEventCmd('', 'Loan', 'Repay', { nil, s.obtainedLoans[1] }))",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let repay: String = lua
        .load("local l = HOOK.commands[#HOOK.commands].Loan.Repay.loan return l.id .. ':' .. l.amount")
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(repay, "2:1200");
    // Where Rival's loans cannot be read, the window shows none and offers
    // none: never the first company's as Rival's.
    let board: String = lua
        .load(
            "package.loaded['tpf3mp.follow'].LOANS_EVERY = -1              package.loaded['tpf3mp.companies'].loanTable = function() error('unreadable') end              local s = api.engine.getComponent(40, 7).state              return #s.availableLoans .. ' ' .. #s.obtainedLoans",
        )
        .eval()
        .unwrap();
    assert_eq!(board, "0 0");
}

/// Tracks over FAKE_NETWORK and FAKE_STOPS, for signals: track 200 runs east
/// from node 20 (0, 200) to node 21 (100, 200), track 201 west from node 22
/// (200, 200) to node 21. On 200 stand the player's signal 300 (Auto
/// Signals' spacing set, a quarter of the way) and signal 301 (three
/// quarters of the way). Node 21 has a lane configuration.
const FAKE_TRACKS: &str = r#"
api.type.enum.EdgeObjectType = { STOP_LEFT = 0, STOP_RIGHT = 1, SIGNAL = 2 }
NODES[20] = { x = 0, y = 200, z = 0 }
NODES[21] = { x = 100, y = 200, z = 0 }
NODES[22] = { x = 200, y = 200, z = 0 }
EDGES[200] = { node0 = 20, node1 = 21, tangent0 = { x = 100, y = 0, z = 0 }, tangent1 = { x = 100, y = 0, z = 0 },
               type = 0, typeIndex = -1, objects = { { 300, 2 }, { 301, 2 } },
               laneConfigs = { { speed = 30, width = 1, height = 0, offset = 0, forward = true, transportModes = {} } } }
EDGES[201] = { node0 = 22, node1 = 21, tangent0 = { x = -100, y = 0, z = 0 }, tangent1 = { x = -100, y = 0, z = 0 },
               type = 0, typeIndex = -1, objects = {},
               laneConfigs = { { speed = 30, width = 1, height = 0, offset = 0, forward = true, transportModes = {} } } }
TRACKS = { [20] = { 200 }, [21] = { 200, 201 }, [22] = { 201 } }
-- The rebuilt tracks are both owned by the player; the proposal must carry
-- the same owner so a signal-only replay is accepted.
local CT = api.type.ComponentType
CT.PLAYER_OWNED = 14
api.type.PlayerOwned = { new = function() return {} end }
local get = api.engine.getComponent
api.engine.getComponent = function(e, kind)
    if kind == CT.PLAYER_OWNED and (e == 200 or e == 201) then return { player = 25 } end
    return get(e, kind)
end
-- Node 21's turn from track 200 into 201, which the player set by hand.
CONFIGS[21] = api.type.BaseNodeConfig.new()
CONFIGS[21].laneConnections = { { segment0 = 200, lane0 = 0, segment1 = 201, lane1 = 0, withRoad = false, withTram = false } }
CONFIGS[21].userModifiedLaneConnections = true
OBJECTS[300] = { param = 0.25, edgeObjectConstruction = '::/infrastructure/signal/signal_path_c.con',
                 params = { auto_signals_distance = 3 } }
OBJECTS[301] = { param = 0.75, edgeObjectConstruction = '::/infrastructure/signal/signal_path_a.con' }
local streets = api.engine.system.streetSystem
streets.getNode2TrackEdgeMap = function()
    local m = {}
    for node, edges in pairs(TRACKS) do m[node] = edges end
    return m
end
streets.getNodeTrackSegments = function(node) return TRACKS[node] or {} end
"#;

/// What Auto Signals sends after signal 300 with 50 m spacing, replacing
/// (auto_signals.script.lua, submit): both tracks rebuilt in place, a new
/// signal on 200 where 301 stood, which it removes, and two on 201, which
/// runs the other way (so on its other side); the new signals named by
/// their place in edgeObjectsToAdd, across both tracks.
const SIGNALS_BUILD: &str = "{ constructionsToAdd = {}, constructionsToRemove = {}, streetProposal = { \
    nodesToAdd = {}, nodesToRemove = {}, edgesToRemove = { 200, 201 }, \
    edgesToAdd = { \
      { entity = -1, type = 1, comp = { node0 = 20, node1 = 21, type = 0, typeIndex = -1, \
          tangent0 = { x = 100, y = 0, z = 0 }, tangent1 = { x = 100, y = 0, z = 0 }, \
          objects = { { 300, 2 }, { -400000000, 2 } }, \
          laneConfigs = { { speed = 30, width = 1, height = 0, offset = 0, forward = true, transportModes = {} } } }, \
        playerOwned = { player = 25 } }, \
      { entity = -2, type = 1, comp = { node0 = 22, node1 = 21, type = 0, typeIndex = -1, \
          tangent0 = { x = -100, y = 0, z = 0 }, tangent1 = { x = -100, y = 0, z = 0 }, \
          objects = { { -400000001, 2 }, { -400000002, 2 } }, \
          laneConfigs = { { speed = 30, width = 1, height = 0, offset = 0, forward = true, transportModes = {} } } }, \
        playerOwned = { player = 25 } } }, \
    edgeObjectsToAdd = { \
      { edgeEntity = -1, param = 0.75, left = true, oneWay = false, \
        model = '::/infrastructure/signal/signal_path_c.con' }, \
      { edgeEntity = -2, param = 0.75, left = false, oneWay = false, \
        model = '::/infrastructure/signal/signal_path_c.con' }, \
      { edgeEntity = -2, param = 0.25, left = false, oneWay = false, \
        model = '::/infrastructure/signal/signal_path_c.con' } }, \
    edgeObjectsToRemove = { 301 } } }";

/// The signal tool's settings travel with its signal: the GUI notes them
/// with the tool's construction, the capture carries them in PlaceStop,
/// and every game builds the signal with them (Auto Signals reads its
/// spacing off the built signal). A note another construction's, cut short
/// or of settings the room cannot carry refuses the signal, never builds it
/// with the defaults.
#[test]
fn a_signal_keeps_the_settings_the_tool_built_it_with_in_every_game() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    let signal = stop_proposal("", "", "")
        .replace("category = 0", "category = 2")
        .replace("{ -400000000, 0 }", "{ -400000000, 2 }")
        .replace("modelId = 77", "modelId = 78");
    lua.load(
        "CAPTURE = ug_require('tpf3mp_1::/scripts/tpf3mp/capture.lua') \
         local util = { getActionParams = function(definition) \
             return { constructionActionParams = { edgeObjectBuilder = { resName = definition.res, \
                 params = definition.params, oneWay = false } } } end } \
         package.loaded['tpf3mp.stopToolWatched'] = nil \
         assert(CAPTURE.watchStopTool(util, ug_require('tpf3mp_1::/scripts/tpf3mp/bridge.lua').attach(tpf3mp_native))) \
         TOOL = util",
    )
    .exec()
    .unwrap();
    let ask = |definition: &str| -> String {
        lua.load(format!(
            "TOOL.getActionParams({definition}) \
             HOOK.room = true HOOK.clicks = 0 SCRIPT.guiUpdate({{}}, nil, nil) \
             local r = SCRIPT.guiHandleEvent({{}}, nil, nil, '', 'streetTerminalBuilder', \
                 'builder.proposalCreate', {{ {signal} }}) \
             if r == nil then return 'nil' end \
             for text in pairs(r.errorMessages) do return text end"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"))
    };
    let signal_c = "res = 'infrastructure/signal/signal_path_c.con'";
    // A setting the room cannot carry (text), and more settings than a note
    // holds: refused, saying why.
    assert_eq!(
        ask(&format!("{{ {signal_c}, params = {{ label = 'x' }} }}")),
        "Not in multiplayer yet: a signal whose settings the room cannot read: setting label is a string"
    );
    let many: Vec<String> = (0..30)
        .map(|i| format!("a_rather_long_setting_name_{i:02} = {i}"))
        .collect();
    assert_eq!(
        ask(&format!(
            "{{ {signal_c}, params = {{ {} }} }}",
            many.join(", ")
        )),
        "Not in multiplayer yet: a signal whose settings the room cannot read: more settings than a note holds"
    );
    // The note of another construction than the tool's: refused.
    lua.load("HOOK.notes['stop-tool-params'] = '1\\tinfrastructure/signal/signal_path_a.con\\t0'")
        .exec()
        .unwrap();
    lua.load("HOOK.notes['stop-tool'] = 'infrastructure/signal/signal_path_c.con'")
        .exec()
        .unwrap();
    let refused: String = lua
        .load(format!(
            "local _, why = CAPTURE.stop({signal}, ug_require('tpf3mp_1::/scripts/tpf3mp/bridge.lua').attach(tpf3mp_native)) \
             return why"
        ))
        .eval()
        .unwrap();
    assert_eq!(
        refused,
        "a signal whose settings the room cannot read: the tool's settings are another construction's"
    );
    // Cut short (a note is at most 512 bytes, and longer ones are cut):
    // refused.
    let cut: String = lua
        .load(
            "local note = CAPTURE.paramsNote('c.con', { a = 1, b = 2 }) \
             local _, why = CAPTURE.readParamsNote(note:sub(1, #note - 5), 'c.con') return why",
        )
        .eval()
        .unwrap();
    assert_eq!(cut, "the tool's settings were cut short");

    // Auto Signals' settings on the base game's signal: carried, sorted.
    // With them Signal Distance's, a key with a dot in it
    // (signal_distance_1, mod.script.lua: "signal_distance.local", its
    // slider's index), refused in a room until 2026-10-11 ("a setting named
    // signal_distance.local"): carried and built as that one key, never as
    // a path.
    assert_eq!(
        ask(&format!(
            "{{ {signal_c}, params = {{ oneWay = 2, auto_signals_replace = 1, auto_signals_distance = 4, \
             ['signal_distance.local'] = 18 }} }}"
        )),
        "nil",
        "the signal tool builds through the room"
    );
    lua.load("HOOK.clicks = 1 SCRIPT.guiUpdate({}, nil, nil)")
        .exec()
        .unwrap();
    let handed: String = lua
        .load(
            "local s = HOOK.commands[#HOOK.commands].PlaceStop local out = {} \
             for _, p in ipairs(s.params) do out[#out + 1] = p.key .. '=' .. p.value.Int end \
             return s.object .. ' ' .. s.model .. ' ' .. table.concat(out, ',') .. ' ' \
                 .. tostring(schema_check(HOOK.commands[#HOOK.commands]))",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert_eq!(
        handed,
        "Signal infrastructure/signal/signal_path_c.con \
         auto_signals_distance=4,auto_signals_replace=1,oneWay=2,signal_distance.local=18 true"
    );
    // Every game builds it with them.
    lua.load("HOOK.batch = { HOOK.commands[#HOOK.commands] } UPDATE({}, STATE, 0.2)")
        .exec()
        .unwrap();
    let built: String = lua
        .load(
            "local o = SENT[1].proposal.streetProposal.edgeObjectsToAdd[1] \
             return o.params.auto_signals_distance .. ' ' .. o.params.auto_signals_replace .. ' ' .. o.params.oneWay \
                 .. ' ' .. tostring(o.params['signal_distance.local']) .. ' ' .. tostring(o.params.signal_distance)",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert_eq!(built, "4 1 2 18 nil");
}

/// Auto Signals after its player's signal (D27; docs/MODS.md): the script's
/// build, both tracks rebuilt with signals added and one replaced, goes to
/// the room from the player's game as a PlaceSignals, once its acceptance
/// switch is on; every game builds it in one proposal, in a game whose
/// track 201 runs the other way at the other places and side, the junction
/// between the two tracks naming both rebuilt tracks, the new signals the
/// acting company's.
#[test]
fn auto_signals_spacing_goes_to_the_room_and_every_game_builds_it() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    lua.load(FAKE_TRACKS).exec().unwrap();
    let stop = "{ PlaceStop = { edge = { network = 'Track', ends = { a = { x = 0, y = 200, z = 0 }, \
        b = { x = 100, y = 200, z = 0 } } }, at = { x = 25, y = 200, z = 0 }, left = true, \
        direction = { x = 1, y = 0, z = 0 }, model = 'infrastructure/signal/signal_path_c.con', \
        object = 'Signal' } }";
    lua.load(format!(
        "HOOK.room = true HOOK.clicks = 0 HOOK.status = {{ me_id = 'me' }} \
         SCRIPT.guiUpdate({{}}, nil, nil) \
         function BUILD(proposal) \
             api.cmd.makeWorldBuildProposalCmd(proposal, {{}}, false, true) \
             HOOK.clicks = HOOK.clicks + 1 \
             SCRIPT.guiUpdate({{}}, nil, nil) \
         end \
         HOOK.batch = {{ {stop} }} HOOK.origins = {{ 'me' }} UPDATE({{}}, STATE, 0.2) \
         SENT = {{}} \
         SCRIPT.guiUpdate({{}}, nil, nil) \
         local acceptance = ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua') \
         ON = acceptance.signals \
         acceptance.signals = false \
         BUILD({SIGNALS_BUILD}) \
         acceptance.signals = true \
         BUILD({SIGNALS_BUILD})"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    assert!(
        logged
            .iter()
            .any(|l| l.contains("signals awaits two-player game acceptance")),
        "with its switch off, nothing goes: {logged:?}"
    );
    assert!(
        lua.load("return ON").eval::<bool>().unwrap(),
        "on since the two-player game of 2026-10-06"
    );
    let handed: String = lua
        .load(
            "local s = HOOK.commands[#HOOK.commands].PlaceSignals local out = {} \
             local function n(v) return string.format('%.2f', v) end \
             for _, e in ipairs(s.edges) do \
                 local adds, removes = {}, {} \
                 for _, a in ipairs(e.add) do adds[#adds + 1] = n(a.at) .. (a.left and 'L' or 'R') end \
                 for _, r in ipairs(e.remove) do removes[#removes + 1] = n(r.at) .. ' ' .. r.model end \
                 out[#out + 1] = n(e.edge.a.x) .. '>' .. n(e.edge.b.x) .. ' +' .. table.concat(adds, ',') \
                     .. ' -' .. table.concat(removes, ',') \
             end \
             return s.model .. ' ' .. tostring(s.one_way) .. ' ' .. #s.params .. ' | ' \
                 .. table.concat(out, ' | ') .. ' | ' .. tostring(schema_check(HOOK.commands[#HOOK.commands]))",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert_eq!(
        handed,
        "::/infrastructure/signal/signal_path_c.con false 0 \
         | 0.00>100.00 +0.75L -0.75 ::/infrastructure/signal/signal_path_a.con \
         | 200.00>100.00 +0.75R,0.25R - | true"
    );
    assert!(
        logged
            .iter()
            .any(|l| l == "handed the player's build to the room [a script's signals]"),
        "{logged:?}"
    );

    // Another game, whose track 201 runs from node 21 to node 22.
    lua.load(
        "EDGES[201].node0, EDGES[201].node1 = 21, 22 \
         EDGES[201].tangent0, EDGES[201].tangent1 = { x = 100, y = 0, z = 0 }, { x = 100, y = 0, z = 0 } \
         SENT = {} \
         HOOK.batch = { HOOK.commands[#HOOK.commands] } HOOK.origins = { 'other' } UPDATE({}, STATE, 0.2)",
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    let placed: String = lua
        .load(
            "local c = SENT[1] local p = c.proposal.streetProposal local out = {} \
             for _, e in ipairs(p.edgesToAdd) do \
                 local o = {} for _, x in ipairs(e.comp.objects) do o[#o + 1] = x[1] .. ':' .. x[2] end \
                 out[#out + 1] = e.entity .. '[' .. table.concat(o, ',') .. ']' \
             end \
             for _, eo in ipairs(p.edgeObjectsToAdd) do \
                 out[#out + 1] = eo.edgeEntity .. '@' .. string.format('%.2f', eo.param) .. (eo.left and 'L' or 'R') \
                     .. ' ' .. eo.model .. ' ' .. tostring(eo.playerEntity) \
             end \
             return table.concat(out, ' ') .. ' | -' .. table.concat(p.edgesToRemove, ',') \
                 .. ' | -' .. table.concat(p.edgeObjectsToRemove, ',') \
                 .. ' | ' .. table.concat(p.nodeConfigsToRemove, ',') \
                 .. ' | ' .. tostring(c.context.player) .. ' ' .. tostring(c.playerInitiated)",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert_eq!(
        placed,
        "-1[300:2,-400000000:2] -2[-400000001:2,-400000002:2] \
         -1@0.75L ::/infrastructure/signal/signal_path_c.con 25 \
         -2@0.25L ::/infrastructure/signal/signal_path_c.con 25 \
         -2@0.75L ::/infrastructure/signal/signal_path_c.con 25 \
         | -200,201 | -301 | 21 | 25 true"
    );
    // The junction between them names both rebuilt tracks, its turn still
    // marked as set by hand.
    let junction: String = lua
        .load(
            "local p = SENT[1].proposal.streetProposal \
             for _, n in ipairs(p.nodeConfigsToAdd) do if n.entity == 21 then \
                 local t = n.comp.laneConnections[1] \
                 return t.segment0 .. '>' .. t.segment1 .. ' ' .. tostring(n.comp.userModifiedLaneConnections) \
             end end \
             return 'not configured'",
        )
        .eval()
        .unwrap();
    assert_eq!(junction, "-1>-2 true");
}

/// What the capture of a script's signals refuses, saying why: anything
/// beyond tracks rebuilt in place with signals added or removed, and
/// anything it cannot pair one for one. And what a game refuses to apply:
/// a track or a removed signal it cannot find, or finds twice.
#[test]
fn a_signal_build_the_room_cannot_carry_says_why() {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    lua.load(FAKE_TRACKS).exec().unwrap();
    let why = |change: &str| -> String {
        lua.load(format!(
            "local p = {SIGNALS_BUILD} local s = p.streetProposal {change} \
             local a, why = ug_require('tpf3mp_1::/scripts/tpf3mp/engine.lua').placeSignals(p) \
             if a == nil then return why end \
             if a == false then return 'false' end \
             return tostring(schema_check(a))"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}"))
    };
    assert_eq!(why(""), "true");
    let cases = [
        (
            "s.edgesToAdd[1].comp.roadStyle = '::/style/fast.track_style'",
            "a signal build that changes its track",
        ),
        (
            "s.edgesToAdd[1].comp.roadDevelopmentLocked = true",
            "a signal build that changes its track",
        ),
        (
            "s.edgesToAdd[1].playerOwned = { player = 7 }",
            "a signal build that changes its track owner",
        ),
        (
            "s.edgesToAdd[1].playerOwned = nil",
            "a signal build that changes its track owner",
        ),
        (
            "s.edgesToAdd[1].comp.edgeDecorations = { { 42, true } }",
            "a signal build that changes its track",
        ),
        (
            "s.edgesToAdd[1].comp.laneConfigs = { { speed = 31, width = 1, height = 0, offset = 0, forward = true } }",
            "a signal build that changes its track",
        ),
        (
            "s.edgesToAdd[2].comp.tangent0 = { x = -90, y = 0, z = 0 }",
            "a signal build that changes its track",
        ),
        (
            "s.edgesToAdd[2].comp.node1 = 20",
            "a signal build that moves an edge",
        ),
        (
            "s.nodesToAdd = { { entity = -3 } }",
            "a signal build that changes nodes or their junctions",
        ),
        (
            "s.edgeObjectsToRemove = {}",
            "a signal build that drops an object without removing it",
        ),
        (
            "s.edgesToAdd[1].comp.objects = { { 300, 2 }, { 301, 2 }, { -400000000, 2 } }",
            "a signal build that keeps an object it removes",
        ),
        (
            "s.edgesToAdd[2].comp.objects[1] = { 301, 2 }",
            "a signal build that moves an object from another edge",
        ),
        (
            "s.edgeObjectsToAdd[2].edgeEntity = -1",
            "a new signal recorded on another edge",
        ),
        (
            "s.edgesToAdd[2].comp.objects = { { -400000001, 2 } }",
            "a new signal on no edge",
        ),
        (
            "s.edgesToAdd[2].comp.objects[1] = { -400000001, 0 }",
            "a script's build of a new stop",
        ),
        (
            "s.edgeObjectsToAdd[3].model = '::/infrastructure/signal/signal_path_a.con'",
            "signals of more than one kind at once",
        ),
        ("s.edgesToAdd[1].type = 0", "a signal build on a street"),
        (
            "s.edgeObjectsToAdd[1].param = 1.5",
            "a new signal with no place on its edge",
        ),
        (
            "s.edgesToRemove = { 200 }",
            "a signal build that does not rebuild its edges one for one",
        ),
        (
            "s.edgeObjectsToRemove = { 301, 999 }",
            "a signal build that removes an object of no edge it rebuilds",
        ),
    ];
    for (change, expected) in cases {
        assert_eq!(why(change), expected, "{change}");
    }

    // Applying: a second node where a track ends (a track over another
    // within half a metre) refuses it, and so does a removed signal that is
    // not there, or is there twice.
    let action: String = lua
        .load(format!(
            "local a = ug_require('tpf3mp_1::/scripts/tpf3mp/engine.lua').placeSignals({SIGNALS_BUILD}) \
             ACTION = a return 'ok'"
        ))
        .eval()
        .unwrap();
    assert_eq!(action, "ok");
    let apply = |setup: &str| -> String {
        lua.load(format!(
            "{setup} ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').signals = true \
             SENT = {{}} HOOK.batch = {{ ACTION }} HOOK.origins = {{ 'other' }} UPDATE({{}}, STATE, 0.2) \
             return #SENT == 0 and 'refused' or 'built'"
        ))
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)))
    };
    assert_eq!(
        apply("NODES[23] = { x = 100, y = 200, z = 0.3 } TRACKS[23] = { 201 }"),
        "refused"
    );
    assert!(
        hook_log(&lua).contains("two track nodes where a signal's track ends"),
        "{}",
        hook_log(&lua)
    );
    assert_eq!(
        apply("NODES[23] = nil TRACKS[23] = nil OBJECTS[301] = nil"),
        "refused"
    );
    assert!(
        hook_log(&lua)
            .contains("no ::/infrastructure/signal/signal_path_a.con where a signal is removed")
    );
    assert_eq!(
        apply(
            "OBJECTS[301] = { param = 0.75, edgeObjectConstruction = '::/infrastructure/signal/signal_path_a.con' } \
             OBJECTS[302] = { param = 0.751, edgeObjectConstruction = '::/infrastructure/signal/signal_path_a.con' } \
             EDGES[200].objects = { { 300, 2 }, { 301, 2 }, { 302, 2 } }"
        ),
        "refused"
    );
    assert!(hook_log(&lua).contains("two signals where one is removed"));
    assert_eq!(
        apply("OBJECTS[302] = nil EDGES[200].objects = { { 300, 2 }, { 301, 2 } }"),
        "built",
        "{}",
        hook_log(&lua)
    );
}

/// Action-boundary company and assignment diagnostics name only the
/// canonical/native entities needed to compare replicas. Unreadable optional
/// components are markers, and a later vehicle refusal does not erase the
/// successful earlier assignment.
#[test]
fn event_boundary_diagnostics_cover_company_and_partial_line_assignment() {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    lua.load(
        r#"
        PLAYER = string.rep('a', 64)
        HOOK.room = true
        HOOK.batch = { { CompanyOp = { Create = { name = 'Fixture Company' } } } }
        HOOK.origins = { PLAYER }
        HOOK.seals = { { scope = 'SENTINEL_SCOPE', tag = 'SENTINEL_SEAL' } }
        UPDATE({}, STATE, 0.2)

        local CT = api.type.ComponentType
        CT.PLAYER_OWNED = 13
        local base = api.engine.getComponent
        api.engine.getComponent = function(e, kind)
            if kind == CT.PLAYER_OWNED then
                if e == 301 or e == 401 or e == 402 then return { player = 901 } end
                return nil
            end
            if kind == CT.TRANSPORT_VEHICLE then
                if e == 401 then return { transportVehicleConfig = { vehicles = {
                    { part = { modelId = 41 } }, { part = { modelId = 42 } } } } } end
                if e == 402 then error('SENTINEL_COMPONENT_FAILURE') end
            end
            if kind == CT.LINE and e == 301 then
                return { vehicleInfo = { transportModes = { [3] = true, [7] = true } } }
            end
            return base(e, kind)
        end
        api.res.modelRep.getName = function(id)
            if id == 41 then return 'vehicle/bus/city.mdl' end
            if id == 42 then return 'vehicle/bus/ferry.mdl' end
        end
        local send, setLineCalls = api.cmd.sendCommand, 0
        api.cmd.sendCommand = function(command, ...)
            if command.setLine then
                setLineCalls = setLineCalls + 1
                if setLineCalls == 2 then error('second vehicle refused') end
            end
            return send(command, ...)
        end
        SENT, HOOK.applied = {}, {}
        HOOK.batch = { { AssignLine = { vehicles = { 0, 1 }, line = 0, first_stop = 1 } } }
        HOOK.origins = { PLAYER }
        UPDATE({}, STATE, 0.2)
        "#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));

    let logs: String = lua
        .load("return table.concat(HOOK.logged, '\\n')")
        .eval()
        .unwrap();
    assert!(
        logs.contains("company-event action_index=1 op=Create actor_before=company-0/entity-25 actor_after=company-1/entity-901 local_before=company-0/entity-25 local_after=company-0/entity-25 outcome=applied"),
        "{logs}"
    );
    assert!(
        logs.contains("assign-vehicle action_index=1 vehicle=0 native=401 line=0 native_line=301 first=1 carrier=bus models=vehicle/bus/city.mdl,vehicle/bus/ferry.mdl mode=3,7 owner=company-1/entity-901 line_owner=company-1/entity-901 outcome=applied"),
        "{logs}"
    );
    assert!(
        logs.contains("assign-vehicle action_index=1 vehicle=1 native=402 line=0 native_line=301 first=1 carrier=unavailable models=unavailable mode=3,7 owner=company-1/entity-901 line_owner=company-1/entity-901 outcome=refused"),
        "{logs}"
    );
    assert!(
        logs.contains("action-event action_index=1 kind=AssignLine outcome=refused"),
        "{logs}"
    );
    assert!(!logs.contains("SENTINEL_SEAL"), "{logs}");
    assert!(!logs.contains("SENTINEL_SCOPE"), "{logs}");
    assert!(!logs.contains("SENTINEL_COMPONENT_FAILURE"), "{logs}");
    assert!(!logs.contains(&"a".repeat(64)), "{logs}");
    let (sent, applied): (usize, bool) =
        lua.load("return #SENT, HOOK.applied[1].ok").eval().unwrap();
    assert_eq!(
        (sent, applied),
        (1, false),
        "the first command remains applied"
    );
}

/// A buy diagnostic reports ordered model resources and the entity ids after
/// the registry binds the new vehicle. Bulldoze diagnostics reuse the
/// construction already resolved by constructionAt, and distinguish a
/// missing owner from a throwing owner read.
#[test]
fn event_boundary_diagnostics_report_buys_and_resolved_bulldozes() {
    let (lua, _script) = engine();
    lua.load(FAKE_FLEET).exec().unwrap();
    let setup = r#"
        PLAYER = string.rep('b', 64)
        HOOK.room = true
        HOOK.batch = { { CompanyOp = { Create = { name = 'Fixture Company' } } } }
        HOOK.origins = { PLAYER }
        UPDATE({}, STATE, 0.2)

        local CT = api.type.ComponentType
        CT.PLAYER_OWNED = 13
        local base = api.engine.getComponent
        api.engine.getComponent = function(e, kind)
            if kind == CT.PLAYER_OWNED then
                if e == 202 then return { player = 901 } end
                return nil
            end
            return base(e, kind)
        end
        api.res.modelRep.find = function(name)
            if name == 'vehicle/bus/city.mdl' then return 41 end
            if name == 'vehicle/ship/ferry.mdl' then return 42 end
            return -1
        end
        api.cmd.makeEntitySetColorCmd = function(entity, color) return { paint = entity } end
        api.engine.util.proposal = {
            createProposalRemove = function(entity) return { removes = entity } end,
        }
        CONSTRUCTION_SCANS = 0
        local list = api.engine.getEntitiesWithComponent
        api.engine.getEntitiesWithComponent = function(kind)
            if kind == CT.CONSTRUCTION then CONSTRUCTION_SCANS = CONSTRUCTION_SCANS + 1 end
            return list(kind)
        end

        local buy = __BUY_BUS__
        buy.BuyVehicle.consist[2] = {
            model = 'vehicle/ship/ferry.mdl', reversed = false,
            loads = { { config = 0, cargo = 3 } }, color = { r = 0, g = 0, b = 1 },
        }
        buy.BuyVehicle.groups = { 1, 1 }
        buy.BuyVehicle.multiple_units = { '', '' }
        HOOK.batch, HOOK.origins = { buy }, { PLAYER }
        UPDATE({}, STATE, 0.2)

        CONSTRUCTION_SCANS = 0
        SENT = {}
        HOOK.batch = { { Bulldoze = { Construction = {
            file = 'depot/bus_depot.con', at = { x = 600, y = 10, z = 2 },
        } } } }
        HOOK.origins = { PLAYER }
        UPDATE({}, STATE, 0.2)

        local get = api.engine.getComponent
        api.engine.getComponent = function(e, kind)
            if e == 201 and kind == CT.PLAYER_OWNED then error('SENTINEL_OWNER_FAILURE') end
            return get(e, kind)
        end
        REFUSE = true
        HOOK.batch = { { Bulldoze = { Construction = {
            file = 'depot/bus_depot.con', at = { x = 600, y = 10, z = 2 },
        } } } }
        HOOK.origins = { PLAYER }
        UPDATE({}, STATE, 0.2)
        "#
    .replace("__BUY_BUS__", BUY_BUS);
    lua.load(setup)
        .exec()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));

    let logs: String = lua
        .load("return table.concat(HOOK.logged, '\\n')")
        .eval()
        .unwrap();
    assert!(
        logs.contains("buy-vehicle action_index=1 outcome=applied models=vehicle/bus/city.mdl,vehicle/ship/ferry.mdl native=500 canonical=2"),
        "{logs}"
    );
    assert!(
        logs.contains("bulldoze-construction action_index=1 ref=depot/bus_depot.con@(600.0,10.0,2.0) native=201 actual=(600.0,10.0,2.0) owner=none"),
        "{logs}"
    );
    assert!(
        logs.contains("action-event action_index=1 kind=Bulldoze outcome=applied"),
        "{logs}"
    );
    assert!(
        logs.contains("bulldoze-construction action_index=1 ref=depot/bus_depot.con@(600.0,10.0,2.0) native=201 actual=(600.0,10.0,2.0) owner=unavailable"),
        "{logs}"
    );
    assert!(
        logs.contains("action-event action_index=1 kind=Bulldoze outcome=refused"),
        "{logs}"
    );
    assert!(!logs.contains("SENTINEL_OWNER_FAILURE"), "{logs}");
    assert!(!logs.contains(&"b".repeat(64)), "{logs}");
    let scans: usize = lua.load("return CONSTRUCTION_SCANS").eval().unwrap();
    assert_eq!(scans, 2, "one constructionAt traversal per bulldoze action");
}

/// A track the room ordered, started from the middle of track 200 between
/// its signals (FAKE_TRACKS): 200 cut at (50, 200), the new track running
/// south from there, each signal kept on the piece it stood on, by where it
/// stood on 200 and its construction. `KEPT_A` and `KEPT_B` are the kept
/// signals of the western and eastern pieces, `KEPT_C` the new track's.
const CUT_TRACK: &str = "{ BuildTrack = { track = '::/track/standard.street_template', catenary = false, \
    polyline = { \
    vertices = { \
        { pos = { x = 0, y = 200, z = 0 }, resolve = { Node = 'Track' } }, \
        { pos = { x = 50, y = 200, z = 0 }, resolve = 'New' }, \
        { pos = { x = 100, y = 200, z = 0 }, resolve = { Node = 'Track' } }, \
        { pos = { x = 50, y = 150, z = 0 }, resolve = 'New' } }, \
    links = { \
        { from = 0, to = 1, tangent0 = { x = 50, y = 0, z = 0 }, tangent1 = { x = 50, y = 0, z = 0 }, \
          structure = 'Ground', kept = KEPT_A }, \
        { from = 1, to = 2, tangent0 = { x = 50, y = 0, z = 0 }, tangent1 = { x = 50, y = 0, z = 0 }, \
          structure = 'Ground', kept = KEPT_B }, \
        { from = 1, to = 3, tangent0 = { x = 0, y = -50, z = 0 }, tangent1 = { x = 0, y = -50, z = 0 }, \
          structure = 'Ground', kept = KEPT_C } }, \
    removals = { { network = 'Track', ends = { a = { x = 0, y = 200, z = 0 }, b = { x = 100, y = 200, z = 0 } } } } \
    } } }";

const KEPT_300: &str =
    "{ removal = 0, at = 0.25, model = '::/infrastructure/signal/signal_path_c.con' }";
const KEPT_301: &str =
    "{ removal = 0, at = 0.75, model = '::/infrastructure/signal/signal_path_a.con' }";

fn cut_track(a: &str, b: &str) -> String {
    cut_track_onto(a, b, "{}")
}

fn cut_track_onto(a: &str, b: &str, c: &str) -> String {
    CUT_TRACK
        .replace("KEPT_A", a)
        .replace("KEPT_B", b)
        .replace("KEPT_C", c)
}

/// A game with FAKE_TRACKS and the standard track template, its gate for
/// signals kept on a cut track `on`.
fn cut_track_game(on: bool) -> Lua {
    let (lua, _script) = engine();
    lua.load(FAKE_NETWORK).exec().unwrap();
    lua.load(FAKE_STOPS).exec().unwrap();
    lua.load(FAKE_TRACKS).exec().unwrap();
    lua.load(format!(
        r#"
        local get = api.res.streetTemplateRep.get
        local find = api.res.streetTemplateRep.find
        api.res.streetTemplateRep.find = function(name)
            if name == '::/track/standard.street_template' then return 6 end
            return find(name)
        end
        api.res.streetTemplateRep.get = function(id)
            if id == 6 then return {{ laneConfigs = {{ 'track lanes' }}, streetStyle = '::/style/track.street_style' }} end
            return get(id)
        end
        ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').splitSignals = {on}
        -- Each signal's signal list (SIGNAL_LIST): a plain signal unless set.
        api.type.ComponentType.SIGNAL_LIST = 26
        api.type.Signal = {{ Type = {{ SIGNAL = 0, ONE_WAY_SIGNAL = 1, WAYPOINT = 2 }} }}
        SIGNALS = {{}}
        local component = api.engine.getComponent
        api.engine.getComponent = function(id, kind)
            if kind == 26 then
                if OBJECTS[id] == nil then return nil end
                return SIGNALS[id] or {{ signals = {{ {{ type = 0 }} }} }}
            end
            return component(id, kind)
        end
        "#
    ))
    .exec()
    .unwrap();
    lua
}

/// Each new edge's objects, as `entity[object:type,...]`, and the edges
/// removed; or "nothing sent".
fn cut_track_sent(lua: &Lua) -> String {
    lua.load(
        "local c = SENT[1] if c == nil then return 'nothing sent' end \
         local p = c.proposal.streetProposal local out = {} \
         for _, e in ipairs(p.edgesToAdd) do \
             local o = {} for _, x in ipairs(e.comp.objects or {}) do o[#o + 1] = x[1] .. ':' .. x[2] end \
             out[#out + 1] = e.entity .. '[' .. table.concat(o, ',') .. ']' \
         end \
         return table.concat(out, ' ') .. ' | -' .. table.concat(p.edgesToRemove, ',')",
    )
    .eval()
    .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(lua)))
}

/// A track cut between its signals keeps each signal, under its own entity,
/// on the piece it stood on, in every game alike: in one whose track runs
/// the other way too, each signal found by where it stands from the
/// removal's own end. The accepted case is enabled; the gate-off check below
/// confirms that it refuses the build when disabled.
#[test]
fn every_game_keeps_a_cut_tracks_signals_on_their_pieces() {
    let build = cut_track(&format!("{{ {KEPT_300} }}"), &format!("{{ {KEPT_301} }}"));
    let lua = cut_track_game(true);
    lua.load(format!(
        "HOOK.batch = {{ {build} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert_eq!(
        cut_track_sent(&lua),
        "-1[300:2] -2[301:2] -3[] | -200",
        "{}",
        hook_log(&lua)
    );
    assert!(
        hook_log(&lua).contains("kept 2 signal(s) on the track's new pieces"),
        "{}",
        hook_log(&lua)
    );

    // Another game, whose track 200 runs from node 21 to node 20.
    let other = cut_track_game(true);
    other
        .load(format!(
            "EDGES[200].node0, EDGES[200].node1 = 21, 20 \
             EDGES[200].tangent0, EDGES[200].tangent1 = {{ x = -100, y = 0, z = 0 }}, {{ x = -100, y = 0, z = 0 }} \
             OBJECTS[300].param, OBJECTS[301].param = 0.75, 0.25 \
             HOOK.batch = {{ {build} }} UPDATE({{}}, STATE, 0.2)"
        ))
        .exec()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&other)));
    assert_eq!(
        cut_track_sent(&other),
        "-1[300:2] -2[301:2] -3[] | -200",
        "{}",
        hook_log(&other)
    );

    // A mod's signal, which every game of the room has.
    let modded = cut_track_game(true);
    let mod_301 = KEPT_301
        .replace("'::/", "'custom_signals_1::/")
        .replace("signal_path_a", "custom");
    modded
        .load(format!(
            "OBJECTS[301].edgeObjectConstruction = 'custom_signals_1::/infrastructure/signal/custom.con' \
             HOOK.batch = {{ {} }} UPDATE({{}}, STATE, 0.2)",
            cut_track(&format!("{{ {KEPT_300} }}"), &format!("{{ {mod_301} }}"))
        ))
        .exec()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&modded)));
    assert_eq!(
        cut_track_sent(&modded),
        "-1[300:2] -2[301:2] -3[] | -200",
        "{}",
        hook_log(&modded)
    );

    // With its gate off, no game builds it.
    let off = cut_track_game(false);
    off.load(format!(
        "HOOK.batch = {{ {build} }} UPDATE({{}}, STATE, 0.2)"
    ))
    .exec()
    .unwrap();
    assert_eq!(cut_track_sent(&off), "nothing sent");
    assert!(
        hook_log(&off).contains("splitSignals awaits two-player game acceptance"),
        "{}",
        hook_log(&off)
    );
}

/// A track started close to node 21 of track 200-21-201 (FAKE_TRACKS): the
/// tool removes 21 and puts its own node at (70, 200), short of signal 301
/// (at 75), which it keeps on the piece that took over the track beyond
/// 21 (refitSignals). That piece neither starts where 200 started nor ends
/// where it ended, but runs where 301 stood, the same way: every game keeps
/// it there, and none keeps it on a piece that runs the other way.
const ACROSS_NODE: &str = "{ BuildTrack = { track = '::/track/standard.street_template', catenary = false,     polyline = {     vertices = {         { pos = { x = 0, y = 200, z = 0 }, resolve = { Node = 'Track' } },         { pos = { x = 70, y = 200, z = 0 }, resolve = 'New' },         { pos = { x = 200, y = 200, z = 0 }, resolve = { Node = 'Track' } } },     links = {         { from = 0, to = 1, tangent0 = { x = 70, y = 0, z = 0 }, tangent1 = { x = 70, y = 0, z = 0 },           structure = 'Ground', kept = { KEPT_300 } },         { from = FROM, to = TO, tangent0 = TANGENT, tangent1 = TANGENT,           structure = 'Ground', kept = { KEPT_301 } } },     removals = { { network = 'Track', ends = { a = { x = 0, y = 200, z = 0 }, b = { x = 100, y = 200, z = 0 } } },                  { network = 'Track', ends = { a = { x = 200, y = 200, z = 0 }, b = { x = 100, y = 200, z = 0 } } } },     removed_nodes = { { network = 'Track', at = { x = 100, y = 200, z = 0 } } }     } } }";

fn across_node(from: u32, to: u32, tangent: &str) -> String {
    ACROSS_NODE
        .replace("KEPT_300", KEPT_300)
        .replace("KEPT_301", KEPT_301)
        .replace("FROM", &from.to_string())
        .replace("TO", &to.to_string())
        .replace("TANGENT", tangent)
}

#[test]
fn every_game_keeps_a_signal_put_back_across_a_removed_node() {
    let lua = cut_track_game(true);
    lua.load(format!(
        "HOOK.batch = {{ {} }} UPDATE({{}}, STATE, 0.2)",
        across_node(1, 2, "{ x = 130, y = 0, z = 0 }")
    ))
    .exec()
    .unwrap_or_else(|error| {
        panic!(
            "{error}
{}",
            hook_log(&lua)
        )
    });
    assert_eq!(
        cut_track_sent(&lua),
        "-1[300:2] -2[301:2] | -200,201",
        "{}",
        hook_log(&lua)
    );

    let back = cut_track_game(true);
    back.load(format!(
        "HOOK.batch = {{ {} }} UPDATE({{}}, STATE, 0.2)",
        across_node(2, 1, "{ x = -130, y = 0, z = 0 }")
    ))
    .exec()
    .unwrap_or_else(|error| {
        panic!(
            "{error}
{}",
            hook_log(&back)
        )
    });
    assert_eq!(cut_track_sent(&back), "nothing sent");
    assert!(
        hook_log(&back).contains("neither goes on from its track nor runs where it stood"),
        "{}",
        hook_log(&back)
    );
}

/// What a game refuses of a cut track's kept signals, saying why, the same
/// in every game: a signal kept on the piece it did not stand on, one left
/// behind, one it cannot find or finds twice, a waypoint or a mod's signal.
#[test]
fn a_cut_tracks_signal_a_game_cannot_keep_refuses_the_build() {
    let refused = |a: &str, b: &str, setup: &str, why: &str| {
        let lua = cut_track_game(true);
        lua.load(format!(
            "{setup} HOOK.batch = {{ {} }} UPDATE({{}}, STATE, 0.2)",
            cut_track(a, b)
        ))
        .exec()
        .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
        assert_eq!(cut_track_sent(&lua), "nothing sent", "{why}");
        assert!(hook_log(&lua).contains(why), "{why}: {}", hook_log(&lua));
    };
    let both = |a: &str, b: &str| (format!("{{ {a} }}"), format!("{{ {b} }}"));
    // Onto the new track, which does not go on from the cut one.
    let lua = cut_track_game(true);
    lua.load(format!(
        "HOOK.batch = {{ {} }} UPDATE({{}}, STATE, 0.2)",
        cut_track_onto(
            &format!("{{ {KEPT_300} }}"),
            "{}",
            &format!("{{ {KEPT_301} }}")
        )
    ))
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", hook_log(&lua)));
    assert_eq!(cut_track_sent(&lua), "nothing sent");
    assert!(
        hook_log(&lua).contains("neither goes on from its track nor runs where it stood"),
        "{}",
        hook_log(&lua)
    );
    // One left behind on the removed track.
    refused(
        &format!("{{ {KEPT_300} }}"),
        "{}",
        "",
        "an object no link takes",
    );
    // A construction that is not there.
    let wrong = KEPT_301.replace("signal_path_a", "signal_path_c");
    let (a, b) = both(KEPT_300, &wrong);
    refused(&a, &b, "", "where it is kept");
    // Two signals of one construction where one is kept.
    let (a, b) = both(KEPT_300, KEPT_301);
    refused(
        &a,
        &b,
        "OBJECTS[302] = { param = 0.751, edgeObjectConstruction = '::/infrastructure/signal/signal_path_a.con' } \
         EDGES[200].objects[3] = { 302, 2 }",
        "two signals where one is kept",
    );
    // A waypoint, which lines name, whatever its construction.
    let (a, b) = both(KEPT_300, KEPT_301);
    refused(
        &a,
        &b,
        "SIGNALS[301] = { signals = { { type = 2 } } }",
        "a waypoint",
    );
    // The same signal kept twice.
    let (a, b) = both(KEPT_300, &format!("{KEPT_301}, {KEPT_301}"));
    refused(&a, &b, "", "where it is kept");
}

/// Two level crossings, each on two nodes, and a street node above one of
/// them (a bridge): the crossing types the game offers here, the owners of
/// the crossings' roads and tracks, and the hook's carrier of a crossing's
/// type, which the game's build retypes it with.
const FAKE_CROSSINGS: &str = r#"
api.type.ComponentType.BASE_NODE = 30
api.type.ComponentType.RAILROAD_CROSSING = 31
api.type.ComponentType.PLAYER_OWNED = 32
api.type.ComponentType.BASE_EDGE = 33
-- The crossings' segments, by their ends: 900 and 901 meet at the
-- crossing's nodes and reach out to 650 and 651.
EDGE_ENDS = { [900] = { node0 = 650, node1 = 701 }, [901] = { node0 = 700, node1 = 651 },
              [910] = { node0 = 710, node1 = 711 }, [911] = { node0 = 711, node1 = 712 } }
api.type.SimpleProposal.new = function() return { constructionsToAdd = {}, streetProposal = {} } end
NODE_AT = { [700] = { x = 18, y = 20, z = 1 }, [701] = { x = 10, y = 20, z = 1 },
            [710] = { x = 500, y = 0, z = 0 }, [711] = { x = 508, y = 0, z = 0 },
            [720] = { x = 10, y = 20, z = 9 } }
CROSSINGS = { [800] = { nodes = { 700, 701 }, edges = { 900, 901 }, typeIndex = 3 },
              [810] = { nodes = { 710, 711 }, edges = { 910, 911 }, typeIndex = 3 } }
OWNERS = { [911] = 77 }
TYPES = { [3] = 'railroad_crossing/era_a_eu.rcr', [5] = 'railroad_crossing/era_c_eu.rcr', [6] = 'hidden.rcr' }
RETYPES = true
api.res = api.res or {}
api.res.railroadCrossingTypeRep = {
    getName = function(i) return TYPES[i] end,
    find = function(name) for i, n in pairs(TYPES) do if n == name then return i end end return -1 end,
    isVisible = function(i) return i ~= 6 end,
}
local previous = api.engine.getComponent
api.engine.getComponent = function(e, kind)
    if kind == 30 then local p = NODE_AT[e] return p and { position = p } end
    if kind == 31 then return CROSSINGS[e] end
    if kind == 32 then local o = OWNERS[e] return o and { player = o } end
    if kind == 33 then return EDGE_ENDS[e] end
    if previous then return previous(e, kind) end
end
api.engine.system.streetSystem = {
    getNode2StreetEdgeMap = function() return { [700] = {}, [701] = {}, [710] = {}, [711] = {}, [720] = {} } end,
    getNode2TrackEdgeMap = function() return { [700] = {}, [701] = {}, [710] = {}, [711] = {} } end,
}
api.engine.system.railRoadCrossingSystem = { getRailroadCrossingForNode = function(node)
    for e, c in pairs(CROSSINGS) do
        for _, n in ipairs(c.nodes) do if n == node then return e end end
    end
    return -1
end }
api.type.Proposal = api.type.Proposal or { new = function() return { carrier = true } end }
-- The hook's carrier (crates/tpf3mp-hook/src/crossings.rs): armed with this
-- game's crossing and type, filled at the next build's apply, where the
-- game removes the crossing and adds it anew, of the type, under a new
-- entity (RETYPES false: the type stays).
tpf3mp_native.crossing = function(entity, index)
    if entity == nil then
        local filled = CARRIER and CARRIER.filled
        CARRIER = nil
        return filled
    end
    CARRIER = { entity = entity, index = index, filled = false }
    return true
end
local send = api.cmd.sendCommand
api.cmd.sendCommand = function(command, callback)
    if CARRIER and not CARRIER.filled and command.proposal and command.proposal.carrier then
        CARRIER.filled = true
        local old = CROSSINGS[CARRIER.entity]
        if old and RETYPES then
            CROSSINGS[CARRIER.entity] = nil
            CROSSINGS[CARRIER.entity + 1000] = { nodes = old.nodes, edges = old.edges, typeIndex = CARRIER.index }
        end
    end
    return send(command, callback)
end
"#;

/// The level crossing window's change, which the hook read at the click, goes
/// to the room as the crossing's node places, sorted, and the type's file;
/// refused (the channel gated pending ordinary-start acceptance, a crossing
/// gone, the type it has already, or a shape the hook does not carry), the player is told
/// in the game bar's notice, which the window itself never says.
#[test]
fn a_crossing_type_from_its_window_goes_to_the_room_by_its_nodes() {
    let (lua, _script) = engine();
    lua.load(FAKE_CROSSINGS).exec().unwrap();
    // The production gate is off; the fixture turns it on to exercise the
    // type-only mechanics after checking the default refusal.
    lua.load(
        r#"DEFAULT = ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').crossings
        ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').crossings = false
        HOOK.room = true HOOK.clicks = 0 SCRIPT.guiUpdate({}, nil, nil)
        HOOK.built[0] = { proposal = { crossingType = true, entity = 800, type = 5 } }
        HOOK.clicks = 1 SCRIPT.guiUpdate({}, nil, nil)
        FIRST = HOOK.notes and HOOK.notes['tpf3mp.notice']
        ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').crossings = true
        HOOK.built[1] = { proposal = { crossingType = true, entity = 800, type = 5 } }
        HOOK.built[2] = { proposal = { crossingType = true, entity = 800, type = 3 } }
        HOOK.built[3] = { proposal = { crossingType = true, entity = 999, type = 5 } }
        HOOK.built[4] = { why = "crossing window: a crossing change that rebuilds the crossing's tracks" }
        HOOK.clicks = 5 SCRIPT.guiUpdate({}, nil, nil)"#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let on: bool = lua.load("return DEFAULT").eval().unwrap();
    assert!(!on, "crossings is gated off by default");
    let (first, last, sent): (String, String, String) = lua
        .load(
            "local c = HOOK.commands[1].SetCrossingType \
             local nodes = {} for _, p in ipairs(c.nodes) do nodes[#nodes + 1] = p.x .. ',' .. p.y .. ',' .. p.z end \
             return FIRST, HOOK.notes['tpf3mp.notice'], \
                 #HOOK.commands .. ' ' .. table.concat(nodes, ' ') .. ' ' .. c.crossing_type",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        first,
        "1\tNot in multiplayer yet: changing a level crossing's type \
         (crossings awaits two-player game acceptance)"
    );
    assert_eq!(sent, "1 10,20,1 18,20,1 railroad_crossing/era_c_eu.rcr");
    assert_eq!(
        last,
        "4\tNot in multiplayer yet: changing a level crossing's type \
         (a crossing change that rebuilds the crossing's tracks)"
    );
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    for line in [
        "stopped a build the room cannot carry: the crossing is of that type already [crossing window]",
        "stopped a build the room cannot carry: the crossing is gone [crossing window]",
    ] {
        assert!(logged.iter().any(|l| l == line), "{line}: {logged:?}");
    }
}

/// Every game finds the one crossing on exactly the nodes named, checks the
/// acting company may change it, gives it the type through the game's own
/// proposal and reads it back; a crossing elsewhere, another company's, an
/// unknown type or one the game did not take is refused alike.
#[test]
fn every_game_gives_the_same_crossing_its_type() {
    let (lua, _script) = engine();
    lua.load(FAKE_CROSSINGS).exec().unwrap();
    lua.load("ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').crossings = true")
        .exec()
        .unwrap();
    lua.load(
        r#"local function set(nodes, name) return { SetCrossingType = { nodes = nodes, crossing_type = name } } end
        local here = { { x = 10, y = 20.0004, z = 1 }, { x = 18, y = 20, z = 1.2 } }
        local other = { { x = 500, y = 0, z = 0 }, { x = 508, y = 0, z = 0 } }
        HOOK.room = true UPDATE({}, STATE, 0.2)
        HOOK.batch = {
            set(here, 'railroad_crossing/era_c_eu.rcr'),
            set(here, 'railroad_crossing/era_c_eu.rcr'),
            set({ { x = 10, y = 20, z = 1 } }, 'railroad_crossing/era_a_eu.rcr'),
            set(other, 'railroad_crossing/era_c_eu.rcr'),
            set(here, 'railroad_crossing/nowhere.rcr'),
            set(here, 'hidden.rcr') }
        UPDATE({}, STATE, 0.2)
        RETYPES = false
        HOOK.batch = { set(here, 'railroad_crossing/era_a_eu.rcr') }
        UPDATE({}, STATE, 0.2)"#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let done: String = lua
        .load(
            "local why = {} for _, a in ipairs(HOOK.applied) do \
                 why[#why + 1] = tostring(a.ok) .. ':' .. tostring(a.why) end \
             local built = 0 for _, c in ipairs(SENT) do if c.proposal and c.proposal.carrier then built = built + 1 end end \
             return CROSSINGS[1800].typeIndex .. ' ' .. CROSSINGS[810].typeIndex .. ' ' .. built \
                 .. ' | ' .. table.concat(why, ' | ')",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        done,
        "5 3 2 | true:nil | true:nil | false:the crossing is not there \
         | false:the level crossing's road or track belongs to another company \
         | false:no crossing type railroad_crossing/nowhere.rcr \
         | false:crossing type hidden.rcr is not offered here \
         | false:the game did not change the crossing's type"
    );
}

/// What each game reads may differ in the last digits, the owner may not
/// read, and a build may fail: a crossing's nodes match one to one within
/// the tolerance whatever order they sort in; an owner that does not read is
/// refused, not taken for no one's; and the hook is never left armed with a
/// crossing for a later build of the room's.
#[test]
fn a_crossing_is_matched_node_for_node_and_never_left_armed() {
    let (lua, _script) = engine();
    lua.load(FAKE_CROSSINGS).exec().unwrap();
    lua.load("ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').crossings = true")
        .exec()
        .unwrap();
    lua.load(
        r#"-- Named at x 10 for both nodes; this game has one at 10.001, so it
        -- sorts the other first.
        NODE_AT[730] = { x = 10.001, y = 50, z = 1 }
        NODE_AT[731] = { x = 10, y = 58, z = 1 }
        CROSSINGS[820] = { nodes = { 730, 731 }, edges = { 920 }, typeIndex = 3 }
        local streets, tracks = api.engine.system.streetSystem.getNode2StreetEdgeMap,
            api.engine.system.streetSystem.getNode2TrackEdgeMap
        api.engine.system.streetSystem.getNode2StreetEdgeMap = function()
            local m = streets() m[730] = {} m[731] = {} return m end
        api.engine.system.streetSystem.getNode2TrackEdgeMap = function()
            local m = tracks() m[730] = {} m[731] = {} return m end
        -- An owner whose field does not read.
        local component = api.engine.getComponent
        api.engine.getComponent = function(e, kind)
            if kind == 32 and e == 901 and UNREADABLE then
                return setmetatable({}, { __index = function() error('no player here') end })
            end
            return component(e, kind)
        end
        local function set(nodes, name) return { SetCrossingType = { nodes = nodes, crossing_type = name } } end
        local here = { { x = 10, y = 20, z = 1 }, { x = 18, y = 20, z = 1 } }
        local shifted = { { x = 10, y = 50, z = 1 }, { x = 10, y = 58, z = 1 } }
        HOOK.room = true UPDATE({}, STATE, 0.2)
        HOOK.batch = { set(shifted, 'railroad_crossing/era_c_eu.rcr') }
        UPDATE({}, STATE, 0.2)
        UNREADABLE = true
        HOOK.batch = { set(here, 'railroad_crossing/era_c_eu.rcr') }
        UPDATE({}, STATE, 0.2)
        UNREADABLE = false
        -- The context fails before anything is armed; the build fails once
        -- armed: either way nothing stays armed, and the next action builds.
        local new = api.type.Context.new
        api.type.Context.new = function() error('no context') end
        HOOK.batch = { set(here, 'railroad_crossing/era_c_eu.rcr') }
        UPDATE({}, STATE, 0.2)
        api.type.Context.new = new
        ARMED_AFTER_CONTEXT = CARRIER ~= nil
        REFUSE = true
        HOOK.batch = { set(here, 'railroad_crossing/era_c_eu.rcr') }
        UPDATE({}, STATE, 0.2)
        REFUSE = false
        ARMED_AFTER_REFUSAL = CARRIER ~= nil
        HOOK.batch = { set(here, 'railroad_crossing/era_c_eu.rcr') }
        UPDATE({}, STATE, 0.2)"#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let done: String = lua
        .load(
            "local why = {} for _, a in ipairs(HOOK.applied) do \
                 why[#why + 1] = tostring(a.ok) .. ':' .. tostring(a.why) end \
             return CROSSINGS[1820].typeIndex .. ' ' .. CROSSINGS[1800].typeIndex .. ' ' \
                 .. tostring(ARMED_AFTER_CONTEXT) .. ' ' .. tostring(ARMED_AFTER_REFUSAL) \
                 .. ' | ' .. (table.concat(why, ' | '):gsub('[^ |:]+%.rs:%d+:%d+: ', ''))",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        done,
        "5 5 false false | true:nil \
         | false:the owner of the level crossing's road or track does not read \
         | false:no context | false:the proposal collides | true:nil"
    );
}

/// A window rebuild includes segment and possibly junction changes that a
/// type-only replay cannot preserve, so even an otherwise matching crossing
/// is refused and the player is told why.
#[test]
fn the_window_s_rebuild_of_a_crossing_is_refused() {
    let (lua, _script) = engine();
    lua.load(FAKE_CROSSINGS).exec().unwrap();
    lua.load(
        r#"ug_require('tpf3mp_1::/scripts/tpf3mp/acceptance.lua').crossings = true
        HOOK.room = true HOOK.clicks = 0 SCRIPT.guiUpdate({}, nil, nil)
        HOOK.built[0] = { proposal = { crossingType = true, type = 5, nodes = { 701, 700 }, removed = { 900, 901 },
            junctions = { 650, 701 } } }
        HOOK.built[1] = { proposal = { crossingType = true, type = 5, nodes = { 700 }, removed = { 900 } } }
        HOOK.built[2] = { proposal = { crossingType = true, type = 5, nodes = { 700, 701 }, removed = { 911 } } }
        HOOK.built[3] = { proposal = { crossingType = true, type = 5, nodes = { 700, 710 }, removed = { 900 } } }
        HOOK.built[4] = { proposal = { crossingType = true, type = 5, nodes = { 700, 701 }, removed = { 900 },
            junctions = { 651 } } }
        HOOK.clicks = 5 SCRIPT.guiUpdate({}, nil, nil)"#,
    )
    .exec()
    .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    let commands: usize = lua
        .load("return #HOOK.commands")
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(commands, 0);
    let notice: String = lua
        .load("return HOOK.notes['tpf3mp.notice']")
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert!(
        notice.contains("a crossing rebuild also changes segments or junctions"),
        "{notice}"
    );
    let logged: Vec<String> = lua.load("return HOOK.logged").eval().unwrap();
    let reason = "stopped a build the room cannot carry: a crossing rebuild also changes segments or junctions and is not carried [crossing window]";
    assert_eq!(
        logged.iter().filter(|line| line.as_str() == reason).count(),
        5,
        "{logged:?}"
    );
}

/// Level crossings' types are in the network lane, so two games whose
/// crossings differ split at the next checkpoint: each crossing by its
/// nodes' places and its type by its file, whatever the engine numbers them,
/// whether the engine lists the crossings or they are found from the nodes on
/// both a street and a track; a tile reads its own crossings only, and each
/// row is placed in a part, with the junctions.
#[test]
fn level_crossings_types_are_in_the_network_lane() {
    let (lua, _script) = engine();
    lua.load(FAKE_CROSSINGS).exec().unwrap();
    let out: Vec<String> = lua
        .load(
            r#"local L = ug_require('tpf3mp_1::/scripts/tpf3mp/lanes.lua')
            local C = ug_require('tpf3mp_1::/scripts/tpf3mp/crossings.lua')
            -- No edges in this lane but the crossings' rows.
            api.engine.system.streetSystem.getNode2SegmentMap = function() return {} end
            -- The engine lists the crossings.
            api.engine.getEntitiesWithComponent = function(kind)
                if kind == 31 then local out = {} for e in pairs(CROSSINGS) do out[#out + 1] = e end return out end
                return {}
            end
            local rows = C.rows(api)
            local listed = C.listed
            local lane = L.read(api)[L.NETWORK]
            -- It refuses: found from the nodes instead, the same rows.
            api.engine.getEntitiesWithComponent = function(kind)
                if kind == 31 then error('Cannot loop over this component type') end
                return {}
            end
            local byNodes = C.rows(api)
            local byNodesListed = C.listed
            -- Another type: another lane.
            CROSSINGS[800].typeIndex = 5
            local retyped = L.read(api)[L.NETWORK]
            CROSSINGS[800].typeIndex = 3
            -- The same world numbered otherwise: the same lane.
            local renumbered = {}
            for e, c in pairs(CROSSINGS) do
                renumbered[e + 5000] = { nodes = c.nodes, edges = c.edges, typeIndex = c.typeIndex }
            end
            local before = CROSSINGS
            CROSSINGS = renumbered
            local again = L.read(api)[L.NETWORK]
            CROSSINGS = before
            -- A tile's nodes: its crossings only.
            local tile = C.rows(api, { [700] = true, [999] = true })
            -- A lookup that fails, or a type with no name, fails the read: never
            -- a crossing left out, never a type by its index.
            local lookup = api.engine.system.railRoadCrossingSystem.getRailroadCrossingForNode
            api.engine.system.railRoadCrossingSystem.getRailroadCrossingForNode = function() error('busy') end
            local okLookup, whyLookup = pcall(C.rows, api, { [700] = true })
            api.engine.system.railRoadCrossingSystem.getRailroadCrossingForNode = lookup
            TYPES[3] = nil
            local okName, whyName = pcall(C.rows, api, { [700] = true })
            TYPES[3] = 'railroad_crossing/era_a_eu.rcr'
            assert(not okLookup and whyLookup == 'the crossing on node 700 does not read', tostring(whyLookup))
            assert(not okName and whyName == 'the type of level crossing 800 has no name', tostring(whyName))
            assert(lane ~= 'err' and retyped ~= 'err', tostring(lane))
            return { table.concat(rows, ' || '), listed, byNodesListed,
                tostring(table.concat(byNodes, ' || ') == table.concat(rows, ' || ')),
                tostring(lane ~= retyped), tostring(lane == again), table.concat(tile, ' || '),
                L.rowKind(L.NETWORK, rows[1]), tostring(L.rowPart(L.NETWORK, rows[1], 10)),
                tostring(L.rowPart(L.NETWORK, rows[2], 10)) }"#,
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(&lua)));
    assert_eq!(
        out[0],
        "crossing:10000,20000,1000;18000,20000,1000|railroad_crossing/era_a_eu.rcr \
         || crossing:500000,0,0;508000,0,0|railroad_crossing/era_a_eu.rcr"
    );
    assert_eq!(out[1], "listed 2");
    assert_eq!(out[2], "by nodes 2");
    assert_eq!(out[3], "true", "both listings give the same rows");
    assert_eq!(out[4], "true", "a crossing's type is in the lane");
    assert_eq!(out[5], "true", "no entity id is");
    assert_eq!(
        out[6],
        "crossing:10000,20000,1000;18000,20000,1000|railroad_crossing/era_a_eu.rcr"
    );
    assert_eq!(out[7], "junctions");
    // By the first node's millimetres in cells of 256 m: (0, 0) and (1, 0).
    assert_eq!(out[8], "0");
    assert_eq!(out[9], "1");
}

/// Where the hook reads the parts, it reads their level crossings too; this
/// Lua names their types (`part`'s `crossingTypes`, `partTexts`' eighth
/// argument), so two games whose crossing's type differs split there, and a
/// type with no name stops the check rather than going by its index.
#[test]
fn rolling_parts_name_the_level_crossings_the_hook_reads() {
    let game = |name: &str| {
        let lua = rolling_game();
        lua.load(FAKE_PARTS).exec().unwrap();
        lua.load(format!(
            r#"PART_CROSSINGS = {{ 3 }}
            api.res = api.res or {{}}
            api.res.railroadCrossingTypeRep = {{ getName = function(i)
                if i == 3 then return {name:?} end
            end }}"#
        ))
        .exec()
        .unwrap();
        lua
    };
    let read = |lua: &Lua| -> Vec<String> {
        lua.load(
            "for s=1,11 do ROLL_STEP(s,false) end local r=ROLL_STEP(12,true) \
             local out={} for n=0,6 do out[#out+1]=r[n] end return out",
        )
        .eval()
        .unwrap_or_else(|error| panic!("{error}\n{}", log(lua)))
    };
    let (a, b) = (read(&game("crossing/a.rcr")), read(&game("crossing/b.rcr")));
    assert_ne!(a[0], b[0], "the crossing's type is in the network history");
    assert_eq!(a[1..], b[1..], "and in nothing else");
    assert_eq!(
        read(&game("crossing/a.rcr")),
        a,
        "the same world, the same history"
    );
    // No name: the check stops on the junctions turn (the second update).
    let nameless = game("");
    assert!(
        nameless
            .load("ROLL_STEP(1,false) ROLL_STEP(2,false)")
            .exec()
            .is_err()
    );
}
