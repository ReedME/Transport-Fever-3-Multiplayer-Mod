# Testing in the real game

How a coding agent (or a person) drives real Transport Fever 3 games on
the development PC to test a change: two or more games in one room,
clicks and keys, Lua in the game's console, and the logs that say whether
it worked. The scripts are in [`tools/game/`](../tools/game/), for Windows
PowerShell 5.1. An agent with computer use can click the game itself
instead of through `gamewin.ps1`; everything else here still applies.

Read [AGENTS.md](../AGENTS.md) first. The real game is the last check, not
the first one.

## When to use the real game

Test with the cheapest layer that can catch the problem:

1. **`cargo test --workspace`.** This covers the protocol, the server,
   the hook's logic, and the mod's Lua against stand-ins of the game's
   API: `crates/tpf3mp-proto/tests/lua_mod.rs`, `lua_capture.rs`, and the
   fakes in `tests/lua/`. A behaviour change needs a test here anyway.
2. **The fake game.** `tpf3mp-rig --players 2 --server local` (the
   default `--game fake`) runs whole rooms without the game, and
   `tpf3mp-regress` runs the regression scenarios
   ([REGRESSION.md](REGRESSION.md)).
3. **The real game.** Use it for what only the game can show: whether a
   tool's proposal is captured and replayed in every game, whether the
   GUI looks right, and whether the worlds stay equal. It is slow (about
   five minutes to a room) and it can crash. Use it when the owner asks
   for a real-game test, or to prove a fix to something that failed in
   the game.

**The owner can test faster by hand.** Asking them to try something in the
game ("place a bus stop as P2, does P1 see it?") is often quicker than
scripting it. Say exactly what to do and what to look for.

For company or asset incidents, record each player's active company and
which company owns every edited asset. A shared-company test does not clear
an incident reported with players in separate companies; reproduce that
company and ownership arrangement.

## Rules

- **Only the games you started.** Other agents may be testing on the same
  PC. `room.ps1` refuses while any game, rig or server runs. Quit your
  own sessions first by PID; the script never stops existing processes.
- **Never the game install.** The mod goes into the game's per-user
  staging area. The game's own folder and Steam are never changed.
- **Never the production server.** Rooms here use the rig's throwaway
  local server (`--server local`).
- **Capture the game window, not the screen.** `gamewin.ps1 shot` copies
  only the game's own window. Computer-use screenshots show the whole
  desktop. Use them for clicking, and keep what they show out of logs,
  commits and reports.
- **Quit cleanly.** Use `quit.ps1`. A killed game uploads an "abnormal
  termination" report at its next start. If quitting times out, the script
  reports failure and leaves the process alive for diagnosis.
- **Lua in the console can crash the game.** A Lua error inside an engine
  callback is fatal. One example is `api.engine.forEachEntityWithComponent`'s
  function. Wrap queries in `pcall`, read only, and never send commands
  from the console in a room's game. An invalid proposal can corrupt the
  engine without an error, and the crash comes later.

## Setup

Once per machine:

- Steam running, Transport Fever 3 installed, and the game started once
  by hand, so its per-user folder exists.
- Scripts run with `powershell -NoProfile -ExecutionPolicy Bypass -File
  tools\game\<script>.ps1 ...`, because this PC's execution policy blocks
  unsigned scripts.
- The paths are found automatically (`tools/game/env.ps1`). Override them
  with `TPF3MP_GAME_EXE`, `TPF3MP_GAME_LOCAL` (Steam's
  `userdata\<account>\3493540\local`), `TPF3MP_GAME_WORK` (default
  `target\game-runs`) or `TPF3MP_BIN` (default `target\release`, or
  `$CARGO_TARGET_DIR\release`).

Before each test:

```powershell
cargo build --release -p tpf3mp-testkit --bin tpf3mp-rig
cargo build --release -p tpf3mp-hook --lib
```

Build these as two commands: Cargo's `--bin` filter otherwise skips the hook's library and can silently leave an old DLL beside a new rig.
The rig loads `tpf3mp_hook.dll` from next to itself. A running game locks
that DLL, so quit the games before rebuilding it. **Rebuild the hook after
any change to `tpf3mp-proto`'s actions.** The hook checks the mod's
actions against its own copy of the schema, and an old hook refuses new
fields (for example "Link has no field owned"). The mod's Lua is
reinstalled by every `room.ps1` (without `-NoInstall`).

### Fixture saves

Rooms start from a save in the game's `save` folder:

- `tpf3mp_fixture`: a plain map, 16 by 16 tiles, seed `tpf3mp`, the
  tutorial off, with the mod in its mod list. `tools\game\fixture.ps1`
  makes it: it installs the mod as `room.ps1` does, starts one game
  without the hook, has the console start that world with the mod and
  save it, and quits the game. It needs the console (`debugMode = true`),
  refuses while a game runs, and never overwrites a save; `-Name` saves
  under another name.

A fixture has to list the mod: the hook saves the room's world through
the mod's GUI, so a world whose save does not list the mod leaves the
host's save for the room unanswered (`hook.log`: "the game did not save
within 120 s"), and the guests never join. A save made without it is
mended by loading it with the mod added to its mods and saving it again
(`app.loadGame` with the save's details and `mods` extended, as
`tpf3mp/worldload.lua` does for the room).
- `tpf3mp_fixture3`: a loan, a road depot, two bus stations, Line 1, and
  no bus yet (the default).

For tutorial recovery, `fixture.ps1 -Tutorial -Name <fresh-name>` loads
the built-in tutorial with TPF3-MP in single player and saves it before
quitting. Start that save with `room.ps1 -Fixture <fresh-name>`. Verify
the native tutorial cleanup and unlocked construction/finance in both
games, then place a build and take a loan through the ordinary UI.

To make one by hand: set the world up, then run this in its console:

```lua
app.saveGame("tpf3mp_fixture4", function() print("@@saved") end, false, true)
```

## Starting a room

The helper waits for every game's main menu before typing the host's load
command, so a guest opening its window cannot interrupt that input. Console
typing stops without pressing Enter if focus changes or Windows rejects an
input event. Clear any partial console input before retrying; a returned
typing error is not evidence that the command ran.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\game\room.ps1 -Players 2
```

It:

1. installs the mod;
2. starts `tpf3mp-rig` with a local server and one game per player, 25 s
   apart (two games starting together can fail to set up their graphics);
3. waits until every game's hook has attached and the room's game starts;
4. waits until the host's `hook.log` says its main menu is up (the
   mod's `main_page.tl` served and the room begun on a menu frame), then
   loads the fixture in its console. A game that quits first, comes up
   without the mod's page, or is not at its menu after `-MenuWait`
   seconds (180) fails the setup; nothing is typed into it;
5. waits until the room has saved that world and the guests have loaded it
   from their main menus (with `-Players 1`, a game alone in its room, for
   measuring without a second game on the PC: until the host plays the
   room's world from step 1);
6. closes the host's console and zooms in. On a tall window the console's
   close button is elsewhere: `-CloseConsoleAt x,y` names it, in the
   coordinates of a half-size capture (`gamewin.ps1 shot`).

It ends by printing:

```
run: C:\...\target\game-runs\run-1001-1042
games: p1=47320 p2=43256
```

Keep both lines. Every other script takes the run's name (`-Run
run-1001-1042`) and a game's PID (`-GamePid 43256`). p1 hosts the room.
The others joined with its invite, and each plays for its own company once
it founds or joins one.

What is where:

| path | what |
|---|---|
| `<run>\rig.out` | the rig: invite, `(game pid N)`, room started, what each agent heard |
| `<run>\pN\hook.log` | player N's hook and mod: actions handed and applied, refusals, saves, probes |
| `<game local>\crash_dump\stdout.txt` | the game's own log, **shared by every game on the PC**: console output, Lua errors, crashes |
| `<game local>\save\tpf3mp_room_*.sav` | internal snapshots; stale files are cleaned up after their owner process exits |

## Driving a game

`tools/game/gamewin.ps1 <command> ... -GamePid <pid>` brings that game to
the front (it refuses and sends nothing when it cannot), then:

| command | does |
|---|---|
| `shot <name>` | saves the window at half size to `<work>\<name>.png` |
| `click x y`, `rclick x y` | clicks |
| `move x y` | moves the cursor: a tool's preview, a tooltip |
| `drag x1,y1 x2,y2` | press, move, release: a road, a track, an area |
| `scroll x y -Clicks n` | the wheel; n > 0 zooms in |
| `key {ESC}` | keys in SendKeys syntax: not for text fields |
| `vk 0D 1C` | one key as a keyboard sends it (here Enter) |
| `hold 44 20 -Clicks 600` | a key held down (here D, panning right) |
| `text Rival` | typing into a focused text field |
| `console '<lua>' [-Open]` | one line in the developer console |

**Coordinates are a capture's.** A capture is half the window's size,
frame and title bar included. Take a `shot`, read the point off the image,
and click it. The coordinates in the scripts and below were read from a
window of 2582×1496 (captures of 1291×748). With another size, read them
again from a shot. With computer use, click what you see and skip the
conversion.

Things learned the hard way:

- **A loaded world takes the keyboard from the console.** The console
  stays open through a load, but what is typed afterwards no longer
  reaches its input line. Close it and open it again (its key) first;
  `fixture.ps1` does.
- **Hover before clicking a build tool.** The game places at the last
  position it saw the cursor move to, so `move`, wait about a second, then
  `click`. `tools/game/tool.ps1` does this.
- **Enter in a text field needs `vk 0D 1C`.** SendKeys `{ENTER}` does not
  reach the game's text fields.
- **A remote desktop session steals the foreground.** While someone
  watches over Chrome Remote Desktop, its sharing bar keeps the front and
  no click reaches the game. Wait until they leave.
- **Menus differ per company and year.** A fixture from 1900 offers 1900's
  vehicles and tools.

Some places in `tpf3mp_fixture3`, after `room.ps1`'s zoom, in a 1291×748
capture (check with a shot first):

| what | where |
|---|---|
| the road menu's tools tab | road menu `645,708`, then tab `626,578` |
| its tools: tram, bus lane, crosswalk, barrier, trees, lock, lane arrows, traffic lights | `343,652`, `430`, `515`, `600`, `690`, `775`, `860`, `945` (same row) |
| a point on the east road; the main crossing | `850,409`; `606,380` |
| the depot, north and south stations (scenario build spots) | `885,432`; `662,195`; `590,562` |
| the 40420 pause menu's Quit; then Return to Desktop | `314,726`; `1177,726` |

## The console

`tools/game/console.ps1` runs one line of Lua in the GUI's Lua state and
prints the game's output for it. Every game writes to the same
`stdout.txt`, so print a marker: lines with `@@`, and Lua errors, are what
it shows.

```powershell
tools\game\console.ps1 -GamePid 43256 -Open -Lua 'print("@@player", api.engine.util.getPlayer())'
tools\game\console.ps1 -GamePid 43256 -File query.lua   # quotes survive in a file
```

- Use `-Open` only when the console is closed. It toggles the console.
- Multi-line files are joined into one line, and `--` comment lines are
  dropped. Avoid `--` comments at the end of a line.
- Useful reads: `api.engine.util.getPlayer()` (the GUI follows the
  player's company), `api.engine.getComponent(id,
  api.type.ComponentType.X)`, `game.interface.getEntity(id)`, and the
  systems under `api.engine.system`. [HOOKS.md](HOOKS.md) says which Lua
  state sees what.
- The hook's own Lua functions are in `tpf3mp_native` (HOOKS.md). Read
  them; do not drive the room through them.

## Did it work in every game?

Look at the hook logs, not only the screen. For one action, the acting
player's `hook.log` says `handed the player's action N to the room` (or a
refusal, with why), and **every** player's says `the game applied the
room's actions between simulation updates`. Things worth grepping for:

| line | means |
|---|---|
| `handed ... to the room` | captured and sent |
| `refused`, `... in multiplayer yet: ...` | the mod refused it in a room, and says why |
| `not applied: ...` | a game could not replay an ordered action: a bug, as the worlds now differ |
| `Diverged`, `diverged` | the room's checkpoint found the worlds differ |
| `saved the world for the room` | the room took a save; guests and late joiners load it |
| `Lua error`, `attempt to` (in stdout.txt) | a script failed |

`tools/game/tool.ps1 -Run <run> -GamePid <pid> -Item <menu item> -At
<point> [-To <point>] -Shot <name>` uses one tool and prints every
player's new log lines. Then shoot every game (`gamewin.ps1 shot` with
each PID) and compare.

### Do the games agree?

The mod's determinism probe logs a digest of the world every 100
simulation steps to each `hook.log` (`[tpf3mp-probe det] step=...`).
Lanes are vehicles, positions, edges, constructions, town buildings,
money and people.

```powershell
tools\game\probes.ps1 -Run run-1001-1042
```

It compares every step all players sampled, and prints the lanes of each
step that differs (exit code 1). Equal probes after a test, plus every
action applied everywhere, is the evidence that the change works in
multiplayer.

For a company or vehicle incident, keep the event-boundary lines from each
`hook.log`: `company-event`, `assign-vehicle`, `buy-vehicle`,
`bulldoze-construction` and `action-event`. They identify the sender's
company transition, each vehicle/line assignment and its outcome, purchase
model order and resulting IDs, and the construction actually resolved by a
bulldoze. Compare canonical vehicle/line IDs across games; native entity IDs
are local to each game. `owner=none` means no owner component/value was
present, while `owner=unavailable` means the optional read could not be
completed. These records contain no player key, invite, password or seal.

### Next desync trial

Use a fresh local two-player room with the candidate hook and mod, starting
through the ordinary launcher flow. Keep both `hook.log` files from the
beginning of the room; compare the same checkpoint steps, not just the final
world. Each player builds roads and tracks, buys and assigns a vehicle, then
replaces a vehicle and uses it in another ordered action. Keep vehicles and
traffic active at both 1x and 4x rather than waiting in an empty world. A
first 30–45 minute session can expose a failure but is only a bounded first
stage. For the longer acceptance run, cover more than 42,600 room steps
after the shared load (twice the earlier 21,300-step lane symptom), at least
60 in-game days, and a rejoin from the room's save followed by another
vehicle action. Read the actual step numbers in both logs; elapsed wall time
and game days alone do not establish this coverage. The pass condition is
that every ordered action applies in both games, every overlapping checkpoint
digest agrees, and no canonical vehicle or line id changes merely because
its entity was replaced or briefly omitted from a game list. This is the
ordinary-start evidence for PLAN.md's Test A; the offline Lua tests establish
the injected read-error behavior only. Do not inject component-read failures
in the real game to test that failure path.

If a checkpoint differs, preserve both complete logs and run
`tools/lane_diff.py` on the resulting lane dumps. Compare the `free ids: step`
lines at the earliest differing checkpoint and the `free operations:` burst
in both logs. A `complete=false` burst, missing peer interval or omitted log
lines leaves the native operation order unproved; rerun with capture intact
before changing an engine sort. Do not infer the first cause from a later
vehicle position difference alone.

## Investigating without the room

To see what a change would send without letting the games apply it,
patch **only the installed copy** of the mod in the staging area. For
example, log the proposal and `error("dry run")` before it is sent.
Reinstall the repository's mod afterwards (`room.ps1` does). Never commit
such a patch.

## Quitting

```powershell
tools\game\quit.ps1 -GamePids 47320,43256
```

This closes the window, then clicks Quit and Return to Desktop. A game
still running a minute later is reported as a failure and left alive. The rig stops on its own
once its games have exited. If it does not, stop it by the PID in
`<run>\rig.pid`.

## Reporting

Say what you ran (the run's name, the fixture, which players acted) and
what each player's log said. Give the probe result, and attach the shots
that show it. Say plainly what you did not test in the game.

## Checking the helpers without launching a game

Run `powershell -NoProfile -ExecutionPolicy Bypass -File tools/game/test-tools.ps1`.
It parses every helper and exercises probe comparison on synthetic logs. A
missing sample inside the overlap, unreadable lane or conflicting duplicate
fails the comparison. A shorter overlap is reported explicitly; it is not
proof about steps outside that overlap. Room setup succeeds only when every
game reports loading the shared snapshot, and never loads the fixture into a
guest as a fallback. These checks do not replace a fresh end-to-end game run.
