# Mods in a room: shared and personal

*Proposed* ([DECISIONS.md](DECISIONS.md), D25): the players of a room may
differ in their personal mods. Until the owner approves D25, this page
describes what the code on `feat/personal-mods` does; nothing of it changes a
room whose players list no mods.

The user's ask, in their words: "scan a lua mod for what functions it calls
and so long as it doesn't call any ones that could cause a desync, or if it
does changes we cancel and replay them anyways", and for a timetable mod:
"timetables would cancel and replay but only on the player's own vehicles, we
just have to intercept certain lua functions."

Findings are labelled as in [DAY_ONE.md](DAY_ONE.md): **SEEN** (read in the
game's own files or dumps of build 40408), **REPORTED** (a working mod relies
on it), **INFERRED** (not yet checked in the game; each is listed under
"To measure in the game").

Mods that are more than Lua, such as Big Maps, are not Mod Hub mods: they
would come from the project's signed index of native mods
([NATIVE_MODS.md](NATIVE_MODS.md), proposed D29).

## Three kinds of mod

| class | what it is | in a room |
|---|---|---|
| **personal** | Only the player's view: GUI plugins, windows, overlays, styles, GUI resources the game's own GUI uses (rename schemes, menu categories). Everything it does to the world goes through `api.cmd` from the GUI. | May be active in one player's game and not another's. Its commands go through the room's guard like the player's clicks: carried to every game, or refused. |
| **carried** | Decides in a game script (`*.gs.lua`), but acts only through commands the room can carry from a game script: vehicle holds and releases, departures, stops, line renames and updates. A timetable mod or a line namer. | Shared by default. With `--personal-game-scripts` it is personal: its game script runs in its player's game only, and the personal mods' guard hands its commands to the room, for the player's own company's vehicles and lines only. |
| **shared** | Everything else: world content (models, constructions, vehicles, streets, names, economy), run scripts and `addModifier` (resources as they load), `game.config`, game scripts that send what the room does not carry, and anything the scan cannot read. | Every player must run it, in the same version and load order. The room's content check compares these alone. |

The class comes from a static scan (`tpf3mp-modscan`), which is advisory:
Lua can reach anything by a name it builds at run time. What the scan cannot
see, the guards refuse at run time.

### What makes a mod shared

The scan fails closed: a mod is personal only when every file in it is
accounted for as one that cannot touch the simulation. Each of these makes
it shared, with the file and line:

- no `mod.json`, or one that does not parse;
- a `preRunScript`, `runScript` or `postRunScript`;
- `addModifier`;
- a game script (`*.gs.lua`) (a carried mod, if nothing else below applies
  and every command it makes is one the room carries);
- world content: any file with a resource extension (`.mdl`, `.msh`, `.mtl`,
  `.con`, `.module`, `.ani`, `.lod`, `.blob`, `.street`, `.track`, `.bridge`,
  `.tunnel`, `.grp`, `.fbx`, `.zip`), or a script in one of the game's
  resource folders (`vehicle/`, `construction/`, `names/`, `config/`, ...);
- a `.res.lua` whose `type` is not known to be the GUI's alone (the known
  ones: `react-plugin ...`, `react-replacement-config`, `rename_scheme`,
  `rename_scheme_component`, `firstStopToSend_scheme`, `menu_category`,
  `menu_filter_category`, `drag_and_drop`, those of the game's own
  `gui.zip`);
- resource writes (`addAsTable`, `setAsTable`, `removeAsTable`,
  `<x>Rep.add`, `.set`, `.remove`, `.setVisible`);
- `game.interface`, and setting `game.config`;
- the same through a name bound to `game`, `game.config` or `api.res...`
  (`local rep = api.res.modelRep; rep.add(...)`);
- code the scan cannot read: `load`, `loadstring`, `dofile`, `loadfile`,
  `setfenv`, `_ENV`, `_G[...]`, `setmetatable` or `rawset` on `_G`, `api` or
  `game`, the `debug` library's setters, and `api`, `game`, `api.cmd`,
  `api.res`, a repository or `game.interface` indexed by a string or a
  computed name (`api["cmd"]`);
- `sendCommand` kept as a value or replaced (a mod that captures it before
  the guard is on could send past it);
- a file of a kind not known to be inert, under `content/` (outside
  `content/`, the game loads nothing: noted only);
- a file or folder that cannot be read, a link, or too many or too large.

Noted, and leaving a personal mod personal: the commands it makes (named),
GUI resources, style sheets, `setGuiSaveData` (the mod's own data in the
save, not the simulation), `os` and `io`, `app.loadGame` and the like,
`"cosmetic": true`, dependencies, and files outside `content/`.

### "cosmetic" decides nothing

`mod.json`'s `"cosmetic": true` is the author's word. The game reads it into
its mod description (at +0x79, `investigation/TPF3_ACHIEVEMENTS_2026-09-30.md`)
and its API says only that "cosmetic mods still allow achievement from being
earned" (`api/tealdef/api/type/mod.d.tl`, SEEN). Its other readers were not
traced. Timetables, Planning Fever, Alert Fever and the Urban Games legacy
vehicle packs all say it, and all change the world. The scan notes it and
does not trust it.

## The scan

```text
cargo run -p tpf3mp-modscan -- <mod folder or folder of mods>...
cargo run -p tpf3mp-modscan -- --installed [--game <game folder>] [--steam <Steam folder>]
cargo run -p tpf3mp-modscan -- --json ...
```

It prints each mod's class and every reason, `!` for those that make it
shared, and a count at the end. `--installed` scans every mod this player
has, from each place the game keeps them (`tpf3mp_modscan::roots`):

- Mod Hub (mod.io) downloads: `%PUBLIC%\mod.io\10640\mods\<mod.io id>`,
  then `%LOCALAPPDATA%\mod.io\10640\mods\<mod.io id>`, each `mod.json`
  naming the mod (`revyn112_towns_de` in `...\6414521`) (SEEN on Windows:
  58 mods under `%PUBLIC%` on one PC on 2026-10-03, while
  `%LOCALAPPDATA%\mod.io\10640` held only its user's file; on 2026-09-30
  they were found under `%LOCALAPPDATA%`. Linux and macOS to confirm);
- local mods: `<Steam>\userdata\<account>\3493540\local\staging_area\<modId>`
  and `...\local\mods`;
- the game's own: `<game>\mods`, `<game>\mods\release` and `<game>\dlcs`.
  `release` holds the game's built-in mods, which saves list as
  `urbangames_no_costs_1`, `urbangames_sandbox_1`, `urbangames_tycoon_1`
  and so on, and the campaign's (SEEN on Windows, 2026-10-04: 21 mods;
  until then the launcher missed them, and a save listing one had it
  "not found among the installed mods").

A mod is found by its `mod.json`'s `modId`, else its folder's name. A save
lists a Mod Hub mod by its `modId`, the mod.io number only as its hub id
(SEEN, below).

## At run time

### GUI mods: the command guard

A personal GUI mod sends through `api.cmd` in the GUI's Lua state, where the
guard (`mod/tpf3mp_1/content/scripts/tpf3mp/guard.lua`) sits in front of every
factory and `sendCommand` in the room's game: a command the room carries goes
to the room and every game applies it; any other is refused, and the player
told. That is the "cancel and replay". A refusal in hook.log now names the
mod the command came from (the nearest mod file on the stack, `guard.callers`,
or the mod whose function made the command, when the game's own helper sends
it).

What else a GUI mod could reach, from the GUI state's own dump (build 40408,
`investigation/dayone-2026-09-29/probe/script_api_dump_gui.txt`, SEEN):

- `api.res`: each repository has `find`, `get`, `getAll`, `getName`,
  `isVisible` only; no `add`, `setAsTable` or `setVisible` is bound there;
- `game.interface` is absent;
- `api.engine`: `config` (reads), `entityExists`, `forEachEntity`,
  `getComponent`, `getEntitiesWithComponent`, `getRevision`, `mapgen`,
  `system`, `terrain`, `util`: reads, and proposals that do nothing until
  sent as a command;
- `api.cmd.debug` has more factories (`makeGamePerformSimulationStepsCmd`,
  ...) but no `sendCommand` of its own: their commands reach the guard,
  which knows no such kind and refuses them;
- `debug` has `getinfo` and `traceback` only; `loadstring`, `dofile`,
  `loadfile` and `io` are nil; `load` and `rawset` exist (the scan flags
  them).

So the GUI state has no way to change the simulation but commands. Two gaps
remain, and the scan covers them:

- a mod that keeps `api.cmd.sendCommand` in a local before the guard is on
  sends past it (the scan: shared, "command bypass");
- a mod that calls `app.loadGame`, `app.stopGame` or the like leaves the
  room's world in its own game only; the step gate and the lanes catch the
  game that left (not a desync of the others).

A personal mod's event to its own game script (`makeScriptingSendEventCmd`)
is sent as it is: it reaches this game's game scripts only, where the mod's
own game script is, and every other game never hears it. Its own means
(`guard.ownEvent`): the event's id names the mod (contains its id, or one of
its words of four letters or more: Timetables' `TimetablesEdit` for
`celmi_timetables`), the id is none the game's own game scripts or TPF3-MP
listen to (`Companies`, `Loan`, `Notifications`, `Towns`, the empty id of
init events, ...: `guard.RESERVED_IDS`, from build 40408's scripts), and the
name is none they listen to under any id (`company.*`, `builder.*`,
`init*`, `handleLegacy`). Any other event of a personal mod, a rank
(`Companies applyLevel`), prospecting, a loan, goes the way a click's does:
carried through the room, or refused.

### Game-script mods: the personal mods' guard

A game script runs in the simulation's Lua states, in every game that has
the mod, and a command there runs at once (HOOKS.md, "Actions in the game").
A shared game-script mod runs alike in every game. A personal one must not
act in its own game alone. `tpf3mp/modguard.lua`, installed by TPF3-MP's game
script in each simulation state it links in, sits in front of `sendCommand`
there, in the room's game:

- a command with none of the player's personal mods on its stack (the
  game's own scripts, TPF3-MP's, shared mods) runs as before;
- from a personal mod, a vehicle's manual departure, a departure, a stop,
  and a line's rename or update go to the room as actions (`VehicleOp` with
  `ManualDeparture`, `Depart`, `Stop`; `EditLine`), for the player's own
  company's vehicles and lines only; each room game applies it at the same
  update, and `apply.lua` refuses one for another company's in every game
  (`ownOf`, D21);
- the same change to the same thing within 5 s of game time goes once;
- events between game scripts are dropped (another mod's game script,
  shared by every game, must not hear what only one game says);
- anything else is refused.

Only the game of the player who runs the mod decides: its game script reads
the room's world, the same in every game, and its decisions travel as
ordered actions, like a click, a few updates later. That makes it
deterministic across the room, at the cost of the lead: a hold ordered for
update `s + lead` misses a vehicle that leaves within the lead.

A state without `debug.getinfo` cannot tell a personal mod's command from
the game's own. It fails closed: hook.log says "the personal mods' guard is
not on", and the hook loads the room's worlds without this player's
personal mods from then on (the note `personal-mods-unguarded`). What one
did before is this game's alone, which the room's check finds; the resync
loads the world anew, without them. This is also why carried mods stay
shared unless the player asks (`--personal-game-scripts`).

## The room's content check

The server compares content fingerprints (`tpf3mp-proto/src/content.rs`,
`crates/tpf3mp-server/src/room.rs`: joining a running game and starting
refuse `ContentMismatch`, "players have different game versions or mods").
Nothing there changes: the agent declares only the shared mods.

- Without `--mods`, the launcher finds the player's mods itself and the
  player chooses their personal ones ("Choosing mods" below). With
  `--mods <file>`, one mod a line in load order with its version, that list
  overrides it, as before; the picker is then off. A room that tells its
  mods still has its worlds loaded with its list and settings in such a
  game (the content check covers the mods, not their settings), with the
  file's personal mods after them.
- Each listed mod is found among the installed ones and scanned
  (`crates/tpf3mp-agent/src/content.rs`, `split`). A personal one stays out of
  the manifest; a carried one too, with `--personal-game-scripts`; a shared
  one, one not found, and TPF3-MP itself go in (fail closed). Each verdict
  goes to the launcher's log: `mod timetables 8 is carried (...): <path>:
  game script: runs in the simulation of every game that has the mod (...)`.
- Two players who differ only in personal mods therefore declare the same
  manifest, and the room starts.

### TPF3-MP's own mod

Every game of a room runs TPF3-MP's own mod, `tpf3mp_1`, whose Lua applies
the room's actions; the room checks it by its files, not only by its
revision. A player's game once loaded an old copy of the same revision (a
Sandboxie box's own copy, 2026-10-01): nothing noticed, and a road was
built differently in that game.

A room's world must have the mod among its save's mods: a world loads with
its save's mods (or the room's plan of them, which keeps `tpf3mp_1` only
when the save lists it), and without the mod's game script it held paused
for good, without a word (2026-10-01). So:

- a room's list always runs `tpf3mp_1`, and every game loads the room's
  list (`mods::plan`), so a start save without it is taken when the room's
  list is made of it: the owner's pick on the Load Game page, or, with
  the picker, a save whose mods read and fit a room's list. Otherwise
  (`--mods`, too many mods, or no picker) each game loads the save's own
  mods, and the launcher refuses a start save whose mods read and do not
  list `tpf3mp_1` (creating a room, and the owner's pick in the room), saying
  "This save doesn't have the TPF3-MP mod enabled: load it once, turn
  TPF3-MP on in its mods, save it, then pick it again"
  (`crates/tpf3mp-agent/src/save_check.rs`);
- the agent does not load a world from the room that does not list it,
  when it loads the world with the save's own mods: the session ends, and
  both windows say why;
- the hook's log says when a world's plan leaves it out, which a save whose
  mods the agent could not read may still bring.

- **What is compared** (`crates/tpf3mp-agent/src/own_mod.rs`): a SHA-256
  over the files the game loads from the mod, sorted by path, each path with
  its bytes: `mod.json`, `_content.json` and everything under `content/`.
  Left out: what the launcher writes into the mod by itself, the campaign's
  portraits (`content/gui/tpf3mp/portraits/`, "Portraits" in LOBBY.md) and
  their lines in `_content.json`; `_metadata/`, which the game shows but
  does not load; and `desktop.ini`, `Thumbs.db` and dot files.
- **Which copy:** the one the launcher finds first in the game's order of
  mod folders (`tpf3mp_modscan::roots`), the folder it writes the portraits
  to. The launcher runs as the same user as the game, in the same Sandboxie
  box when it is boxed (the game it starts is boxed with it), so it reads
  the files through the game's own view: a box's copy shadows the real one
  for both. The game names a mod's files only as `tpf3mp_1::/...` and says
  nowhere which folder it loaded them from, so the hook could only repeat
  the same search; and the room checks content in the lobby, before the
  game may run.
- **How:** the mod's version in the manifest is its revision, a `+` and the
  first 16 hex digits (`1+0123456789abcdef`), and the agent always declares
  it, last, whatever the room's save lists. A copy that cannot be read gets
  a version no other game has (fail closed), with a warning in the log. The
  existing content check then refuses Start and a join to the running game
  (`ContentMismatch`), and the player with the other copy is told "Your
  TPF3-MP mod differs from the host's (yours fedcba98, host 01234567):
  reinstall the same version". The launcher computes it once, at start: a
  mod reinstalled while it runs counts after a restart.
- With `--mods`, a listed `tpf3mp_1` that is installed gets the same
  version; one not found keeps its listed version, uncompared.

The launcher window's content comparison, its "differ" pill and the
`ContentDiff` messages work on the declared manifests, so they speak of
shared mods only.

## Choosing mods

Without `--mods`, the launcher (`crates/tpf3mp-agent/src/picker.rs`):

- **finds every installed mod** by itself when it starts: Mod Hub's cache
  (`mod.io\10640\mods` under `%PUBLIC%`, then `%LOCALAPPDATA%`), each Steam account's
  `staging_area` and `mods`, the game's `mods`, `mods\release` and `dlcs` (the first of each
  id counts), scans each and keeps its class and first reason, its name
  (`_metadata/modinfo.json`) and its `revision`. Each goes to the launcher's
  log: `mod schbrongx_minimap 1 is personal: only what this player sees`;
- **lets the player choose** their personal mods, and carried ones with
  `--personal-game-scripts` (`LobbyAction::ChooseMod`, the page's
  `choose_mod`). A shared mod is never chosen: every player needs the
  room's. The choice is remembered in the launcher's `launcher.json`
  (`"mods"`), and the room's worlds load with the chosen ones (the lists of
  `Begin`, read when the game begins; a choice made later loads with the
  next world);
- **takes the room's mods as the owner picks them** (bridge version 25,
  protocol 18). The owner picks the room's save on the game's own Load Game
  page (LOBBY.md, "The room's save and mods"), and with it its mods and
  their settings on the page's Mods and Gameplay Settings tabs, as the game
  would load the save. The window sends them with the save
  (`LobbyAction::ChooseRoomMods`: the mods in the game's load order, each
  with its name and source as the owner's game knows it, and the settings
  of the room's mods and the game's own, `GAME_SETTINGS`), and the
  launcher (`Mods::choose_room`) makes them the room's: each in the
  owner's installed version, TPF3-MP's own last, the owner's personal
  mods left out. A mod the owner's game has from Mod Hub is named with its
  Mod Hub number, read from the installed copy, whatever source the save
  recorded (a save made while the mod was a local copy still says
  `StagingArea`). The launcher declares them with the save's upload
  (`DeclareRoom`), and the room tells every member (`RoomMods`). Changing
  them marks every member not ready;
- **makes the owner's start save the room's shared mods** when nothing was
  picked on the Load Game page (a `--start-save`, or the first pick of a
  room made from the Host page before its mods arrive). When the player
  creates a room from a start save, the launcher reads the save's mods
  (`tpf3mp_modscan::save`, below) and takes those that are not the player's
  personal mods (chosen or not) as the room's, each in the player's version
  (a mod the save lists and this player lacks is still the room's, with no
  version: fail closed). It declares them before the room exists. A save
  whose mods do not read leaves the room's mods unknown, and says so:
  worlds then load with their saves' own mods, as without the picker.
  A save with more mods than a room's list holds (256) is still compared
  whole, but not told as the room's list: every game loads the save's
  own mods;
- **adopts them as a guest.** The room tells its mods (`RoomMods`) on
  joining and whenever they change, and the launcher (`Mods::adopt`)
  declares at once those this player has, in their own versions; one
  missing or in another version shows on the lobby's Room's mods tab and
  on the player's card, and holds the start. A guest installs a missing
  Mod Hub mod from that tab (LOBBY.md, "Installing from Mod Hub"), and the
  launcher finds the installed mods again (`LobbyAction::RescanMods`).
  Mods are compared by version, and a Mod Hub download's version names the
  file installed, so two downloads of different Mod Hub files differ;
- **learns them as a guest** from a room that tells none (its owner declared only their
  content, `DeclareContent`). Joining a room, the launcher declares no mods
  but TPF3-MP's own ("TPF3-MP's own mod" above);
  the room answers with what this game lacks (`ContentDiff`: the owner's
  mods in load order, the first 32 named, with the owner's versions), and
  the bridge declares again the room's mods this player has, in its own
  versions (`PickerLink`, `Request::DeclareContent`, which a member may send
  any time). A guest with every one of them, in the same versions, then
  matches the owner, and a missing one or another version stays in the
  "differ" pill and the lobby's list. Joining a running game learns from its
  refusal and tries once more. More than 32 shared mods cannot all be
  learned this way.

The lobby shows both lists (`LobbyView::mods`, `LobbyView::room_mods`,
bridge version 13): every installed mod with its class, reason, whether it
is chosen and whether it may be; and the room's shared mods with whether
this player has each (`yes`, `no`, `other_version`).

### The mods a save lists

A save is a zstd frame; near its start, after the `tf**` magic and a few
settings, is its list of mods as the game writes it (`GameSaveCommandData`'s
`modDescs`): a `u32` count, then per mod five `u32`-length strings (id,
source, hub id, name, url) and an `i32` severity. The hub id is
`<source>,<id>`, or for a mod.io mod its mod.io number (`6414521`). SEEN in
build 40408's saves (the DLCs, `DLC`; TPF3-MP and local mods,
`StagingArea`; `mod.io` mods; lists of over a hundred mods). The reader
tries each offset in the first 64 KiB and takes the first whole list whose
every entry holds together (a mod id, the hub id as above, a severity of 0
to 2); the list may run on for up to 4 MiB. Where the real list does not
hold together, its tail does (an entry's severity of 1 reads as a count of
one): a save listing a mod.io mod once read as the Pre-Order Pack alone, so
the launcher refused it for lacking TPF3-MP (2026-10-03). So a list found
right behind something shaped like an entry, an id and four more strings
matched by their lengths, is taken for such a tail and the save is
refused. `tpf3mp-modscan --save <file>` prints it.

## The save's mod list

A save lists the mods of the game that wrote it: the owner's start save, or
any member's save the room hands out, lists that player's personal mods. The
game's own Load Game page refuses a save whose mods are missing
(`savegame_react_util.tl`, `areModsMissing`), but that is its page's check;
the same page loads a save with other mods when the player changes them,
through `app.loadGame(id, isMapEditor, info)` with `info` the save's details
(`api.type.SaveGameDetails.new(data.info)`) and `info.mods` replaced
(`mod_selector_page.tl`, `updateSaveGame`; `app.d.tl`: `loadGame(saveId,
isMapEditor, info?, isTutorialInit?)`, `info : SaveGameData.SaveGameDetails`
with `mods : {Mod.ModId}`) (SEEN).

So nothing is stripped from a save. Every game loads the room's world with
its own list instead (`tpf3mp_bridge::mods::plan`):

1. the room's mods, in the room's order, TPF3-MP's own among them, whatever
   the save lists: a mod the owner added on the Load Game page is added in
   every game alike, one they left out is left out;
2. then this player's personal mods;
3. the save's other mods, another player's personal ones, are left out.

The settings follow the same way (`tpf3mp_bridge::mods::settings`): the
save's, with each of the room's mods that has settings in the room's list,
and the game's own (`""`) when the room carries them, taking exactly the
room's.

Each is checked with the user profile's `ModRep:exists`; a shared mod not
installed fails the load and holds the world, saying which. The lists reach
the hook with the room's `Begin` (bridge version 12), and every load uses
them: from the main menu (the hook's chunk, `crates/tpf3mp-hook/src/menu.rs`,
which reads the save's details with `app.getSavegameInfo` first and asks
again until the game has them) and from a world's GUI
(`mod/tpf3mp_1/content/scripts/tpf3mp/worldload.lua`). Without the lists (no
`--mods`), a save loads with its own mods, as before.

The owner's game loads the start save the same way (PLAN.md, Test A, #23:
every game, the owner's too, loads the room's save), so the owner's own
personal mods come back through step 2.

## The real mods

What the scan said (2026-09-30) of the mods in `mods extracted` (GameWatcher's
nine, build 40391), `mods web/tf3mod-minimap`, the Mod Hub cache on this PC
(`%LOCALAPPDATA%\mod.io\10640\mods`), and the game's own `mods` and `dlcs`:
37 mods, 9 personal, 2 carried, 26 shared. The verdicts for playing:

| mod | class | why | in a room, when only some players have it |
|---|---|---|---|
| GW Big City, GW Huge City | personal | a game bar plugin; `makeTownCreateCmd`, `makeTownUpdateSizeCmd`, `makeTownUpdateCargoNeedsCmd`, `makeTownConnectWithIndustriesCmd`, `makeEntitySetNameCmd` | personal-safe now: the guard refuses its town commands ("Not in multiplayer yet"), so it builds no city, in any game |
| GW Cheats | personal | three plugins; `makeJournalBookAssetCmd`, `makeStockListSetModifiersCmd`, `makeTownUpdateSizeCmd` | personal-safe now: its money, industry and town cheats are refused |
| GW Startup Fortune | personal | `makeJournalBookAssetCmd` | personal-safe now: refused |
| GW Faster Game Speed | personal | replaces the speed row | personal-safe now: the room's pace rules; a speed asked for goes to the room as the stock row's does |
| GW Quality of Life 1.0.0, 1.1.0 | personal | plugins; 1.1.0 `makeLineCreateCmd` | personal-safe now: a duplicated line is carried |
| Minimap (schbrongx) | personal | plugins, a style sheet, `setGuiSaveData` | personal-safe now |
| urbangames_campaign | personal | nothing loaded but its manifest | (the game's own) |
| **Timetables** (celmi, mod.io 6037864) | carried | its game script (`timetable/game_script/celmi_timetables.gs.lua`); its run scripts only print; commands `makeVehicleSetManualDepartureCmd`, `makeVehicleTryToDepartCmd`, `makeScriptingSendEventCmd` (game script), `makeScriptingSendEventCmd` and `makeLineUpdateCmd` (GUI); says `"cosmetic": true` | personal-safe after the measurements below, with `--personal-game-scripts`: see "Timetables" |
| **Auto Line Namer** (mod.io 6414403) | carried | a game script renaming lines (`aln.script.lua:43`, `makeEntitySetNameCmd`, from `update` on `os.time` timers), and a rename scheme for the line manager (GUI) | personal-safe after the measurements below, with `--personal-game-scripts`: its renames go to the room as `EditLine` renames of the player's own lines. Its line manager button already works as a personal GUI mod (the game sends the rename, which the guard carries) |
| **Automatic Signal Spacing** (mod.io 6414934) | shared | a run script with `addModifier` on signal constructions (it adds two parameters to every signal), and a game script whose `guiUpdate` builds signals (`makeWorldBuildProposalCmd`, removing and re-adding the track edges by entity id with the new signals) | must be shared, and even shared its builds are refused in a room today: see "Automatic Signal Spacing" |
| signal_distance_1, auto_signals_1 (mod.io) | shared | run scripts with `addModifier`; auto_signals a game script building | must be shared. Signal Distance works in a room (2026-10-02); its per-signal setting (`signal_distance.local`) travels with the signal (2026-10-11). Auto Signals works in a room (2026-10-06): its signal carries its spacing, and its spacing goes to the room as `PlaceSignals`: see "Parallel Tracks, Parallel Roads, Auto Signals" |
| parallel_tracks_1, parallel_roads_1 (mod.io) | shared | a game script whose GUI half builds new tracks or roads beside the one drawn (`makeWorldBuildProposalCmd`), and a `construction_tool` resource adding the tool's settings | must be shared; with D27 their builds go to the room: see "Parallel Tracks, Parallel Roads, Auto Signals" |
| Urban Games legacy packs (6, mod.io) | shared | vehicles: 189 to 449 model files each | must be shared |
| GW Bigger Station Range, GW Buy Industries, GW HQ Growth Boost | shared | run scripts with `addModifier` | must be shared |
| DLCs (deluxe, preorder), the campaign missions, sandbox, no costs, tycoon, no end year | shared | archives of content (`.zip`), run scripts, resource writes | must be shared |

### Timetables

Read in full (paths in the mod's folder):

- **Where it decides: its game script.** `OnArriveAtStop` queues an arrival
  (`timetable_gs.script.tl:1182-1198`); `update` drains up to 16 arrivals
  and holds each (`:366`), then sweeps at most 8 lines and 64 vehicles
  (`timetable_scheduler.script.tl:95-131`), and releases a held vehicle when
  `gameTime >= constraint` (`timetable_logic.script.tl:239-241`, `gs:976`).
  It reads `GAME_TIME`, the line system, `TRANSPORT_VEHICLE` and `LINE`.
- **How it acts:** only through `timetable_util.script.tl`:
  `makeVehicleSetManualDepartureCmd(v, true)` to hold (`:93-97`), `(v,
  false)` to release (`:100-104`), `makeVehicleTryToDepartCmd(v)` to force
  (`:107-111`), and `makeScriptingSendEventCmd("", "celmiTT_*", ...)` public
  events (`:124-128`). Its state (`state:set`, `gs:1114`, ...) is its own
  game script's component.
- **What the GUI sends:** `TimetablesEdit` events to its game script
  (`plugins/shared/helpers.script.tl:155`, through the game's
  `useStepStateTimerWithCommit`), and a line's `reservationPriority` with
  `makeLineUpdateCmd` (`plugins/tabs/lines.script.tl:65-81`). No ownership
  check anywhere: it acts on every line (`lineSystem.getLines()`).
- **Nondeterminism:** none that matters when one game decides: `os.clock` in
  disabled probes; `pairs` over vehicle-keyed tables, order-independent in
  outcome (lowest id wins) but not in the order of commands.

With `--personal-game-scripts`, in the room's game:

- its game script runs in its player's game only; its holds, releases and
  forced departures of the player's own company's vehicles go to the room
  as `VehicleOp` actions (`ManualDeparture(true/false)`, `Depart`), and every
  game applies them at the same update; those of another company's are
  refused at capture and again in every game's `apply.lua`;
- its `celmiTT_*` public events are dropped;
- its GUI's `TimetablesEdit` events reach its own game script in this game
  (the mod made them; the game's helper sends them); its line priority goes
  to the room as a line update, for the player's own lines only.

Other players' games never run it; they apply its decisions as actions.

### Automatic Signal Spacing

It must be shared: its run script changes every signal construction
(`content/mod.script.lua:3-53`, `addModifier("loadConstruction", ...)` adding
`assDirection` and `assSpacing`), which changes the construction's
parameters in every game. Shared, it still does not work in a room today,
for three reasons, and all three are needed:

1. **Signals are not carried.** Its builds are `makeWorldBuildProposalCmd`
   from `guiUpdate` (`ass.script.lua:549-554`), which the GUI's guard
   refuses unless a construction window made them (PLAN.md, Part 3,
   "Roadside stops and signals" is open). It needs an action placing
   signals on existing track: each edge by its ends, each signal's place
   along it (`param`), side, one-way and model, as `PlaceStop` does for
   stops.
2. **Its proposal names edges by entity id** and re-adds them under new
   negative ids with the signals in `objects` (`:490-544`). The capture must
   turn that into "these signals on the edges between these points", not
   carry the ids (D8).
3. **Every game would send it.** Its `handleEvent` queues a job in every
   game on the same build (`onPostBuildProposal`, `:570-584`), and each
   game's `guiUpdate` would send the same build. Only the game whose player
   placed the first signal may hand it to the room, as the build tools'
   clicks are (HOOKS.md, "The build tools").

### Parallel Tracks, Parallel Roads, Auto Signals

Three shared mods that build after the player builds, played in a room of
two games on one PC (2026-10-02, build 40408, the save with Parallel
Tracks, Auto Signals, Signal Distance and No Costs; runs `run-1002-205516`
and, with D27, `run-1002-212743`).

Before D27:

- **Parallel Tracks.** The drawn track went to the room and was built in
  both games. Every game whose own toolbar asked for parallels then sent
  them, for whichever player drew (with both set to 3, both games sent 21
  edges for P1's track); the hook stopped each at the apply (`stopped a
  build the room cannot carry: no proposal seen`), and the mod said `build
  failed`. Its settings are each game's own (`api.gui.fireGuiScriptEvent`
  to its game script's GUI half), and the signal tool sets them to 0.
- **Auto Signals.** The first signal went to the room as a `PlaceStop` and
  stood in both games; the mod queued no job. `PlaceStop` does not carry a
  signal's parameters, so the signal the room built has no distance, which
  is what the mod reads (`distanceFromParams`).
- **Signal Distance** works: it only changes resources as they load.
- No divergence in 56 checkpoints; both games applied the same 6 actions.

With D27 (`tpf3mp/modbuild.lua`, HOOKS.md "Scripts' follow-up builds"):

- P1 set 3 parallels and drew a track: P1's game handed the parallels to
  the room (`a script's follow-up build goes to the room`, a `BuildTrack`
  of 3 edges), and both games built them. Both accounts paid the same.
- P2 set 3 parallels as well and drew a track, P1 still at 3: P2's game
  handed its parallels; P1's game stopped its own (`a script's follow-up of
  another player's build: that player's game hands it to the room`). The
  parallels were built once, in both games.
- The parallels the room built did not set the mod off again: it knows its
  own tracks by their middles (`OWN_TOLERANCE`).
- In the game scripts' GUI state the stack named no mod for the call
  (`guard.callers`): the log says `a script's follow-up build` without
  `from parallel_tracks_1`. The rule does not depend on it.

Auto Signals needed, beyond D27, the signal's parameters carried with
`PlaceStop`, and its spacing (edges removed and re-added with signals)
carried as signals on existing edges ("Automatic Signal Spacing" above,
reasons 1 and 2). Parallel Roads takes the same path as Parallel Tracks;
it was not played.

Built (schema 26, branch `feat/auto-signals`): the signal tool's settings
travel in `PlaceStop::params` and every game sets them on the signal it
builds (`SimpleStreetProposal.EdgeObject.params`, a member the API
reference does not list; seen on build 40408, 2026-10-06, in a single
game: a signal built through a script with `auto_signals_distance = 4`
kept it, and Auto Signals queued its job, `job 2: signal 29930, 200 m`).
The mod's spacing build then goes to the room from its player's game as
one `PlaceSignals` (HOOKS.md, "Scripts' follow-up builds"), behind
`acceptance.lua`'s `signals`.

Played on build 40408, 2026-10-06, two games on one PC with the save
`tpf3mp_mods` (runs `run-1006-225251` and `run-1006-230635`): P2, then
P1, placed signals with Auto Signals' spacing on a new track; the player's
game handed the spacing as `PlaceSignals` (`placing 6 and removing 0
signals on 6 tracks`), the other game stopped its own (`a script's
follow-up of another player's build`). A signal with Replace on a track
that had signals on it replaced them in both games (`placing 6 and
removing 6 signals on 9 tracks`), after the fix for the signal tool's
records (HOOKS.md, the stop tool). Each game's signals, listed from the
console (position, construction, settings), were the same in both, the
player's own with its settings; the rolling world checks agreed and no
action failed to apply. `acceptance.lua`'s `signals` is on. Not yet a
regression scenario: the testkit's model keeps no signals along tracks. Its rebuilds touch tracks that lines run on, as
every signal the room places already does; PLAN.md's "never rebuilding an
edge a line runs on" is the owner's to settle for both.

## To measure in the game

INFERRED until seen on build 40408; each is a check before carried mods may
be personal, and the first three before personal GUI mods are relied on:

1. **A save loads with the room's mods.** `app.loadGame(id, false, info)`
   with `info.mods` replaced loads the world with those mods active, from
   the main menu and in a world, and without a Start Game click.
2. **A Mod Hub mod's name in a save.** SEEN (2026-10-03): a save lists the
   `modId` (`revyn112_towns_de`), the mod.io id (`6414521`) only as its hub
   id; the scan finds either.
3. **Leftover `modParams`.** A dropped personal mod's parameters stay in
   `info.modParams`; the game ignores them (expected).
4. **`debug.getinfo` in the simulation's Lua states**, and what a mod file's
   `source` looks like there and in the GUI (`<modId>::/...`, perhaps with
   `@`): the guards read the mod from it.
5. **Entity ids.** A personal game-script mod's game script is an entity in
   its player's game only (`GAME_SCRIPT`). Whether that shifts the ids of
   what is created after, which the depot's placement of a leaving vehicle
   depends on (PLAN.md, Test A; `investigation/TF3_VEHICLE_DETERMINISM_2026-09-30.md`).
6. **A local script event changes nothing shared.** A
   `makeScriptingSendEventCmd` sent in one game only (Timetables'
   `TimetablesEdit`) leaves every lane equal.
7. **The lead against a timetable.** How often a hold ordered a few updates
   late misses a vehicle, at 1x and 4x.

## An in-game test with two players

Two games on one PC (the Sandboxie second player, or the rig), one room,
build 40408. Player A runs the GW Minimap (`schbrongx_minimap`), player B
GW Cheats (`gw_cheats_1`), both TPF3-MP.

1. Each writes a mod list: A `tpf3mp_1 1` and `schbrongx_minimap 1`; B
   `tpf3mp_1 1` and `gw_cheats_1 1`. Start each launcher with `--mods
   <list>`. Each launcher log says:
   `mod schbrongx_minimap 1 is personal: <path>: nothing in it reaches the world`
   (B: `mod gw_cheats_1 1 is personal: ...`) and
   `mod tpf3mp_1 1 is shared: TPF3-MP itself`.
2. A creates the room with a start save that has the minimap active; B
   joins. No "differ" pill; both ready; A starts. Each hook.log:
   `the room's worlds load with the 1 shared mods and this player's 1 personal ones`, then
   `the room's world loads with 2 mods: tpf3mp_1, gw_cheats_1; left out, another player's or in no list: schbrongx_minimap; this player's own added: gw_cheats_1` (B; A's: `2 mods: tpf3mp_1, schbrongx_minimap; left out, ...: none; this player's own added: none`), and
   `the main menu is loading the room's world (tpf3mp_room_<pid>); the game starts it by itself`.
3. In game: A sees the minimap, B does not; B has the cheat buttons, A does
   not. B clicks "add money": B's game bar says "Not in multiplayer yet:
   this action" and B's hook.log says
   `mod: refused the player's makeJournalBookAssetCmd in the room's game (1 so far), from the mod gw_cheats_1`.
   Both balances stay equal.
4. Both build roads, buy buses and make lines for 10 in-game days. No
   `diverged` line in either hook.log; the checkpoints' lanes match.
5. Timetables (with `--personal-game-scripts` on A only): A sets an
   arrival/departure slot on a bus line. A's hook.log:
   `mod: handed makeVehicleSetManualDepartureCmd from the personal mod celmi_timetables to the room (1 so far)`;
   both games hold A's bus at the stop and release it at the slot, lanes
   equal; B's game never runs Timetables. A also runs the first four
   measurements above.
