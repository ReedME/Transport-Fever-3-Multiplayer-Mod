# The team's plan, from release day on

## ReedME review work (2026-10-11)

This fork's owner authorized the review implementation (D30). The upstream
plan below is retained as context; this checklist tracks our validation.

- [x] R01: verified Proton portable installation and game-closed checks.
- [x] R02: producing-world report epochs and stale completion rejection.
- [x] R03: per-loan timing without retroactive charges.
- [x] R04: native finance accounting and facade match the game's model.
- [x] R05: Linux Steam library paths resolved by the Windows Proton client.
- [x] R06: cleanup cannot remove newly retained or in-flight worlds.
- [x] R07: ordered durable snapshot pointers and recovery retention.
- [x] R08: failed demanded saves retry while paused with backoff.
- [x] Rehosting password policy and its save/reload regressions.
- [x] ReedME branch protections and protected update-signing environment.
- [x] Fork server/image/update defaults and portable private verifier tested.
- [x] Local source checks, Windows crosscheck and synthetic room acceptance.
- Cross-platform CI and acceptance must pass on the exact pushed candidate;
  current run results are tracked on [the review PR](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/pull/1).
- [ ] Exact supported Windows archive and private verifier runner available.
- [ ] Real two-player Proton competitive/recovery session.

Detailed setup and outstanding environmental inputs are in `REEDME_SETUP.md`.

## Upstream plan

What the team works on from Transport Fever 3's release (2026-09-29), in
order, and who takes what. It is the team's plan as agreed on 2026-09-27,
with the review's changes worked in; each change says why. Tick an item
(`[x]`) in the same change that finishes it.

**For agents.** Before starting a task, check it against this page and
[DECISIONS.md](DECISIONS.md). If the task conflicts with either, or with
"Asks that conflict with a decision" below, do not quietly do it and do
not quietly refuse it: say so to the person who asked, name the decision,
and ask how to go on. A decision changes only by a new entry in
DECISIONS.md, never by a task.

## Asks that conflict with a decision

Flag each of these when a task asks for it:

| an ask like | conflicts with | instead |
|---|---|---|
| Steam networking, peer-to-peer, a player hosting, a player's game as the truth | D2, D4: the server orders every turn; no player is the host | Rooms on the project's server |
| Joining a room from the game without the launcher, a Multiplayer entry that works in a game Steam started | D11: the hook runs only in a game the launcher started | *Changed:* the main menu's Multiplayer entry in a game the launcher started, which drives that launcher (D17, amended 2026-09-30) |
| Connecting to an unlisted server, or treating a pasted address as invite routing authority | D12: a regional release routes only to its compiled trusted list. A six-character code is checked on every trusted region; only a unique credential match permits automatic join, and a room card or explicit region choice names its trusted target | `--server` for a one-server playtest; a release-listed region for explicit selection; bare typed invites resolve across all listed regions |
| A proxy DLL (`alut.dll`), files in the game's folder, an installer `.bat` that patches the game | D9, D11 | The readable install scripts put in the mod alone; the launcher injects the hook |
| An action sent to other games before its channel was checked (a "strict" flag off meaning "send it unchecked") | Fail closed (AGENTS.md) | Off means the action is refused in a multiplayer game; see Part 3 |
| A speed control in the launcher | Part 2, Dev A: the game's own speed buttons, synced by the server | |
| Long invites or support IDs, or one code for both | D13 | Six-character codes, separate for a room and a session |
| Engine entity IDs on the wire | D8: positions in millimetres, resource names, canonical IDs | |
| A web view for the launcher (Tauri, WebView2, WebKitGTK), or another launcher | D20: a native egui window, in the look of tearded's launcher | Change the look in `theme.rs` and `app.rs` |
| A Dev track, choosing or going back to versions | Held until after launch by the owner (D20; D18, D19). *Changed:* the room moved into the game is no longer held (D17, amended 2026-09-30) | Ask the owner first |
| Writing or changing a decision, or settling a question left open for the owner | The owner decides (AGENTS.md) | A pull request the owner approves |
| Logging an invite code bare | D13: codes cannot be spotted in a log line | `invite=<code>`, which redaction hides |
| Sending crash dumps, whole files, or players' logs to a server other than the one they play on | D10 (including its approved amendment): redacted lines to the server played on, over the game's connection | The game's dumps through `collect-logs`, on the player's say |

## Before release (done)

- [x] `Action` enum in `tpf3mp-proto`, one variant per player action,
  positions in fixed point, resource names and canonical IDs, no engine
  IDs; every variant round-trips through postcard (D8).
- [x] `tools/re/make_profile.py` writes a hook profile from a symbol map
  and function names, tested on a synthetic PE.
- [x] The rig: N games on one PC, each with its own data folder and game
  link.
- [x] `tpf3mp-agent collect-logs` zips the game's own logs and crash dumps.
  The launcher's log goes to the server by itself (D10).
  *Added (owner-approved D10 amendment, 2026-10-02):* the hook's and the
  game's logs and the game's error reports go too, as redacted lines
  within budgets, every line under the launcher's log session; minidumps
  stay with `collect-logs`.
- [x] Packaging. *Changed:* no proxy DLL and no install `.bat`. The
  packages hold the launcher, agent, hook and mod, with readable install
  scripts (D9), and the launcher injects the hook (D11).
  *Added (owner, 2026-10-01):* Windows v1.1's standalone launcher handles
  first install, signed package downloads, shortcuts, mod updates, repair
  and uninstall. It invokes the readable scripts underneath the setup UI;
  the ZIP remains the portable option. D9 and D11 still apply.
- [x] The road and track capture ported from TPF2's Lua into
  `mod/tpf3mp_1`, producing `Action::BuildRoad`.
- [x] `tools/probe/check_lua.py` on both probe mods, ready to install.

## Part 1: release day, before anything else

**Go or no-go first.** *Added:* these can end the plan, so they come
before the rest ([DAY_ONE.md](DAY_ONE.md) §0).

- [ ] **The launcher's start.** Started by the launcher (suspended, hook
  loaded, `SteamAppId` set, Steam running), the game runs signed in, with
  the Workshop, and does not restart itself through Steam (which would
  lose the hook). DAY_ONE.md §5.
- [x] **Anti-tamper.** `binary_survey.py`: packer sections, entropy, TLS
  callbacks. TPF2 had SteamStub only; Denuvo or a VM protector changes
  the native plan. *Done 2026-09-29, Steam build 40408 on Windows:*
  SteamStub only, `.text` unencrypted, 3 TLS callbacks
  (`investigation/dayone-2026-09-29/1-gonogo.md`). Epic/GOG's 40393 and
  the other platforms are still to check.
- [x] **Archive the build.** Steam build ID, depot manifests, executable
  hashes, and a private copy of each executable (DAY_ONE.md §1). *Done
  for Steam build 40408 (Steam build ID 25533170).*
- [ ] **Patch duty.** Name one person who, on every game patch, reruns the
  naming, the build diff and the profile, and holds the release until the
  hook matches. Expect a day-one patch.

*Added (2026-10-04, patch tooling; ownership remains open):*

- [x] Private source archives with executable/libraries, script/API sources,
  Steam metadata when available, hashes and refusal of incomplete snapshots
  (`tpfre archive`).
- [x] Automated per-target signature/prologue and containing-function audit,
  script diff and strict exact-build profile verification (`tpfre audit`,
  `tpfre verify`; D14). Static results never replace real-game acceptance.
- [x] Bundle reviewed profiles and other build-specific native data so a new
  build's changes can be reviewed together.
- [x] Integrate exact-build verification with private build inputs into the
  update/release procedure (`tpfre build`, `verify-build`, and the release
  workflow's private runner gate); retain every existing promotion gate.
  Runner registration and `TPF3MP_GAME_ARCHIVE` configuration are operator
  setup, not evidence that a GitHub verification run has passed.

*Added* (2026-09-27, from third-party mods made for build 40391,
[investigation/TF3_MODS_2026-09-27.md](../investigation/TF3_MODS_2026-09-27.md)):
our mod and both probes are TPF2 mods and will not load in TF3 as they
are.

- [x] Before release day: `mod/tpf3mp_1` in TF3's layout (`mod.json`,
  `_content.json`, `_metadata/modinfo.json`, `content/`), loaded by a
  game bar plugin, with the Lua side of the link to the hook
  (`tpf3mp/bridge.lua`, HOOKS.md "The Lua side"). How the rest of TpF2
  Multiplayer's mod comes over: [PORTING_TPF2MP.md](PORTING_TPF2MP.md).
- [x] Before release day: `script_api_dump` and `determinism_probe` in
  the same layout (`tools/probe/tf3`), run from a GUI `onStep` plugin and
  logging through `debugPrint` where `io` is missing; the run script's
  state has a dump of its own. The TPF2 probes stay as the fallback.
- [x] Before release day: a tool for each release-day check
  (`tools/dayone/dayone.py`, DAY_ONE.md "Release day, step by step").
- [x] Read the game's `.tl` sources and `.d.tl` API declarations before
  running the probes; list every `api.cmd.make*Cmd` and the tool that
  sends it. *Done:* 61 factories, 311 sending places
  (`investigation/dayone-2026-09-29/5-scripts.md`).

Then:

- [x] `binary_survey.py`: the Lua version, whether RTTI and `__FUNCSIG__`
  strings are present. (*Changed:* no proxy DLL to find, D11.) *Done:*
  Lua 5.2; RTTI and `__FILE__` kept, `__FUNCSIG__` gone.
- [x] `tools/tpfre` (D14; `name_functions.py` to cross-check): TF3's
  equivalents of TPF2's `GameSim::Step`, `CGame::Step`,
  `CommandList::Add`, save and load, into the recon log. TF3's names may
  differ from TPF2's. *Done* with `tpfre match` (TPF2's names carried
  over); the targets are in `profiles/tf3_build40408_steam_windows/hooks.toml`,
  proven against the installed game
  (`crates/tpf3mp-hookcore/tests/tf3_static_proof.rs`).
- [x] `script_api_dump`: both state dumps (game script and GUI); every
  `api.cmd.make.*` factory; whether `io`, `os`, `require` and `load`
  exist. *Done* for the GUI and run-script states
  (`investigation/dayone-2026-09-29/probe/`); no `io`, `load` present.
- [ ] `determinism_probe` on two games from one save: 60 in-game days
  without input, then with scripted input; `compare_runs.py`, the first
  differing step per lane. *Changed (owner, 2026-09-29):* deferred.
  Determinism is assumed to be TPF2's until two hooked games measure it
  at their checkpoints, which is exact at any speed; the GUI-frame probe
  needs 1x (at 4x it skips most samples).
- [ ] A Windows save loaded on Linux and the reverse: does it load, do the
  lanes match.
- [ ] Where the game writes its log and crash dumps, and where it loads
  mods from (DAY_ONE.md, release-day order 6 and 7). *Windows done:* the
  log is `local/crash_dump/stdout.txt` (not TPF2's `local/stdout.txt`),
  mods load from `local/staging_area`; Linux and macOS to check.

## Part 2: the first playable room

Dev A (the hook; on everyone's critical path, so Part 3 work moves off
Dev A where it can):

- [ ] Find `GameSim::Step`, `CGame::Step`, `CGameTime::GetSpeed`,
  `UI::CMenuUI::StartSavegame`, `UI::CMenuUI::CreatePage` (or TF3's
  names for them).
- [ ] Detour the step: `Session::before_step` before each step,
  `after_step` after. Done when the game holds while the agent withholds
  a turn and goes on when it releases it. *Changed:* built and tested
  against a stand-in step (`crates/tpf3mp-hook`, HOOKS.md "The step gate
  in the game"): a `GameSim::Step` call runs one update per step at the
  game's 1x, so the detour runs it once per released step and not at all
  while one is withheld. Still to see in the real game, and to add: the
  mod holding the game at 1x, and loading a room's save.
- [ ] Detour the command queue's add: a road build is cancelled locally
  and its payload goes to `Session::command`. Done when a click builds
  nothing locally and the command shows in the agent's log.
- [ ] `Game::save(file)` calls the native save; the file loads in a fresh
  game.
- [ ] `StepGate::Load`: load the named save from native code, then
  `loaded(next_step)`.
- [ ] Speed and pause follow the room: the game's own buttons go through
  `Session::command` (`Control::Speed`). The launcher has no speed
  control. *Added:* the speed row is a script recipe a mod can replace,
  so this may need no native code; the pause-or-cycle key must be caught
  too. *Changed:* built and seen in the real game (build 40408, one
  player): in a room's game the simulation step runs the updates the room
  released, whatever the game's own speed says (the hook answers the
  step's own call of `CGameTime::GetSpeed`; its other callers keep the
  game's value, HOOKS.md), so no button or key runs the world at another
  pace, and the speed row's value, pause included, goes to the room as a
  speed request (`ToAgent::Speed`); the owner's sets the room's speed
  (measured 1x, 2x, 4x and pause), anyone else's is refused as a notice.
  *Changed:* guests' speed rows now highlight the room's accepted speed,
  including pause; guests' buttons and keyboard speed shortcuts are
  disabled with "Host controls speed" help. The host still requests changes
  through the game's speed helper. Two-game visual acceptance is pending.
  *Added (2026-10-03):* ordered actions apply between simulation updates,
  including while paused, through a token-only wake of the game script.
  The driver holds updates and room saves until replay and script-state
  storage finish. Duplicate/stale wakes and failed or timed-out replay are
  covered by automated tests. *Tried 2026-10-03 on two local games, build
  40408, from the plain fixture:* with the host paused at Apr 16, the guest
  built a 72 m road. Both games showed it before resuming, the guest paid
  $11,129 and the host paid nothing, and the date held. A guest loan take
  and repayment also applied while paused; after resuming, both games had
  the same step-2200 world probe digest. The probe helper could not compare
  every step because the host omitted its earlier step-1100 sample.
- [ ] *Added:* whether the stock tools send their commands through
  `api.cmd.sendCommand`. If they do, the caller-RVA filter cannot tell a
  click from our replay (HOOKS.md), and the hook needs another way to
  tell them apart.

Dev B (the game's side of the bridge):

- [ ] `Tf3Game` in `tpf3mp-hook`, implementing `Game`:
  - `apply(event)`: decode to an `Action` and hand it to the Lua mod,
    which runs it with `api.cmd`;
  - `lanes()`: vehicle count, vehicle positions to 1 m, edge geometry to
    0.1 m, the construction list, town building counts, money per player,
    people count;
  - `save(file)`: Dev A's native save; `notice()`: to the Lua mod.
- [ ] Lanes for TF3's new systems, each added after measuring what it
  costs to read at every checkpoint (*changed:* these were mixed into the
  method list): warehouse stock and spoilage; vehicle wear; town
  happiness, pollution, noise and reputation; company rank, subsidies,
  contracts and built landmarks; time of day, if it affects the
  simulation.
- [ ] Report the game build and active mods (`ContentManifest`), so
  `--game-build` and `--mods` are no longer needed.
- [ ] `save_check::check_lua_data` on a fetched save at `Done::Fetched`
  (`crates/tpf3mp-agent/src/bridge.rs`); refuse to load one that fails.
- [ ] Two games through the real server with the rig, two players.
- [x] *Added, the owner's ask:* a player is marked ready by the agent once
  their game has a world up with the mod linked, in the lobby
  (`ToAgent::WorldUp`, bridge version 7); Not ready holds for that world.
  The owner still presses Start game; starting by itself once everyone
  is ready would be next, if wanted.

Dev C (building):

- [ ] Roads and track: `Action::BuildRoad`/`BuildTrack` into a proposal on
  the receiving game, snapping to nodes within 0.5 m, else new ones
  (BUILDING.md, "Resolving a vertex").
- [ ] Level crossings take the rail's height; a bridge over a road never
  becomes a crossing (both were TPF2 bugs).
- [ ] Bulldoze: find the edge by its endpoints plus a 14 m search along
  the centreline.
- [ ] *Changed:* an in-game Multiplayer panel for a game the launcher
  started: the room, its players, chat and whether the worlds match. Not
  a way to join without the launcher, and no Steam networking (D2, D11).
  *Changed:* moving the lobby itself into the game (D17) is no longer
  held: the owner lifted the hold on 2026-09-30 (the item under Part 4,
  "the room in the game").

**Test A** (the gate to Part 3):

- [ ] Two players each build 20 roads and tracks, crossing each other's;
  lanes match at every checkpoint.
- [ ] 60 in-game days without divergence.
- [ ] *Added:* vehicles leaving a depot a few millimetres to 30 cm apart
  in each game. *Cause found:* the depot sets a leaving vehicle back by its
  entity id, and a game that kept its world numbers entities differently
  from one that loaded its save
  ([TF3_VEHICLE_DETERMINISM_2026-09-30.md](../investigation/TF3_VEHICLE_DETERMINISM_2026-09-30.md)).
  *Fix* (#23): every game, the owner's too, loads the same save whenever
  the room hands one out. Tick once merged and deployed.
- [ ] *Added:* a third player joins mid-game, and a player rejoins after
  killing their game; lanes still match. Late join, rejoin and repair
  all load a save and then apply turns. If a loaded game walks its lists
  in another order than the running one (TPF2's hot-join desync), all
  three break, so this is found here, not in Part 3.
- [ ] *Added:* if a list's order differs, sort every engine list walked
  during a step by entity ID (moved here from Part 3).

*Added:* **Test A, automated** ([REGRESSION.md](REGRESSION.md)): scripted
scenarios that build, buy, make lines and assign vehicles, two or more
games to a room, checked at every step of the script.

- [x] The harness, `tpf3mp-regress`, playing its scenarios on a model of
  the game through the whole stack, in `ci` and `acceptance`.
- [ ] The hook answers the harness's `Observation` from the game.
- [ ] The hook's test mode walks a scenario file, and scenarios get a file
  format.
- [ ] Rooms start from a fixture save everyone has.
- [ ] Measure how fast the game steps at top speed, minimized, and with
  drawing skipped; the budget is 10 minutes a platform.

## Part 3: every action

For each action below:

1. Dump the proposal the game makes for a click and the one the replay
   makes for the same build, and diff them field by field.
2. Add a `strict_<action>` flag, off by default. **Off means the action is
   refused in a multiplayer game**: the hook cancels it and the player is
   told it is not available yet. It is never sent unchecked (fail closed).
3. A 2-player sandbox playtest without divergence. *Added:* and a
   regression scenario for the action ([REGRESSION.md](REGRESSION.md)).
4. Turn the flag on.

Dev A (moves to whoever finishes Part 2 first where Dev A is still on
the hook):

- [ ] Traffic light phases: the intersection by position, the full phase
  table. *Built 2026-10-01:* action schema 11, native junction-only
  capture on Windows build 40408, Lua capture/replay and checkpoint
  coverage for crosswalks, lane connections and light settings. The
  `strict_junctions` switch remains off. The 2026-10-01 two-player test
  demonstrated crosswalk toggles, a lane-connection change and traffic
  lights with matching network checkpoints. Custom phases/reset,
  geometry preservation and the remaining HOOKS.md acceptance checks
  are still required before enabling it.
- [ ] Line priority per line; loading rules per station or line.
- [ ] The new click-to-assign line creation: a new UI flow, captured from
  scratch.
- [ ] Buying a vehicle: the same key on every game, bound in creation
  order, not by entity ID. Selling, replacing, cloning (joining the
  original's line on the same step).
- [ ] Lines: create, edit stops and platforms, delete; assign a vehicle.
- [x] Vehicle colour and name, with no echo between games (TPF2 froze on
  100,000 colour commands). *Done (2026-10-07):* a vehicle, a station, a
  town and a depot renamed and the vehicle recoloured in a two-game room
  on build 40408, applied alike in both, no echo; the `rename` gate is on.
  See `investigation/TPF3_RENAME_2026-10-07.md`.

Dev B:

- [ ] Subsidies, contracts and loans, tied to a company; the server
  decides when two companies want the same subsidy on the same turn (the
  first in order gets it). A loan is never replayed twice for its taker.
- [ ] Headquarters upgrades; prospecting (the industry at the same place,
  with the same ID, on every game); boosting industry.
- [x] Terraforming: raise, lower, smooth, flatten and heightmap brushes.
  Two local games on build 40408 produced identical native heights on
  2026-10-02; see `investigation/STATION_TERRAIN_2026-10-02.md` for limits.
- [ ] Greening and other new brushes, terrain paint, the asset brush.
- [x] Companies: create, switch, dissolve; owners move with the company,
  and no money is created in the switch. *Added (D21):* any split of the
  room's players, loans for every company, colours, and the GUI showing
  the player's own company.
- [ ] *Added, proposed (D22, not decided; the owner approves):* who may do
  what to a company: its head (founder, then the longest-standing player)
  sets its password, sends players out and opens or closes its stations;
  joining a company with a password needs it, sealed by the server and
  never held by a game; the game's company window renames the company;
  the colour chooser offers the game's colours too. Built on
  `feat/company-play`; to see in a real game with three players.
- [x] *Changed, approved station-access portion of D22 (2026-10-02):*
  heads can allow or deny each other company over an open/closed default;
  new companies inherit the default, and existing services are not removed.
  A company's lines stop at
  another company's open stations: the line manager offers them, and
  every game refuses a line that stops at a closed company's station.
  Two local games demonstrated native station selection, grants and reset,
  foreign-edit refusal, pathing, passenger carriage and matching fares after
  reload. The HUD has its own module state and needs a station-details
  conversion fix. See `investigation/STATION_TERRAIN_2026-10-02.md`.
- [ ] *Proposed (D23), for the owner to approve:* company ranks. With one
  company the game's own, a rank the company window takes carried to every
  game (`ApplyRank`); with more, each company's score its share of each
  town's population by what it carried there, times its rating there over
  100, at the same game time in every game. Built on `feat/progression`;
  open: whether the game's thresholds scale with the number of companies,
  and prospecting's outcome for companies other than the room's first,
  which the game's company script never runs (it looks at the save's
  player alone).
- [ ] Roadside stops and signals, the side included. *Changed (owner-approved
  D27 amendment, 2026-10-08):* signal-only in-place rebuilds may touch a
  track a line runs on when nodes, geometry, type, template, owner, style,
  lock, decorations and lanes stay unchanged; every other mixed rebuild
  is refused. Roadside stops and other lined-edge rebuilds remain open.
- [ ] The room's required mods from Mod Hub IDs; a missing mod is
  installed from Mod Hub, never received from another player. *Added (D28,
  proposed):* built on `feat/lobby-mods`: the owner picks the room's save,
  mods and settings on the game's Load Game page, the room tells every
  member its mods (protocol 18), and a member installs a missing Mod Hub
  mod from the lobby with their own game ([MODS.md](MODS.md),
  [LOBBY.md](LOBBY.md)). Tick once the owner approves D28; the new world
  path is still open.
- [ ] *Added, open for the team:* a rule for mods that send commands from
  the GUI (GW Big City and Startup Fortune do, once per save). Every
  player's game sends them: forwarded, the room gets one city per player;
  dropped, the worlds differ. *Proposed (D25, for the owner):* such a mod is
  personal; the guard carries what the room carries and refuses the rest,
  in every game alike ([MODS.md](MODS.md)). *Added (owner-approved D27), for builds a shared mod sends after the player builds (Parallel
  Tracks, Parallel Roads):* the follow-up of a player's build goes to the
  room from that player's game alone; every other game's is stopped
  (`tpf3mp/modbuild.lua`). Built for new streets and tracks; other removals
  stay stopped. *Added (owner-approved D27 amendment, 2026-10-08):* signals a script places along existing
  tracks (Auto Signals) go as one `PlaceSignals`, the signal's own
  settings in `PlaceStop`, behind `acceptance.lua`'s `signals`, on after
  a two-player game (2026-10-06, [MODS.md](MODS.md)); no regression
  scenario yet. Its tracks are rebuilt in place, lines on them too,
  only when their other properties are unchanged: see the item above.
- [ ] *Added (D25, proposed):* personal mods ([MODS.md](MODS.md)). Built:
  the scan (`tpf3mp-modscan`), the content check on shared mods only, the
  room's world loaded with the room's mods and the player's own, and the
  personal mods' guard for game-script mods. Tick once the two-player test
  in MODS.md passes in the real game, and its measurements are made.
- [ ] *Added (D29, owner-approved 2026-10-08):* native mods (more than Lua,
  such as Big Maps) installed by the launcher from the project's signed
  index, enabled in the hook only for their pinned build, and in a room
  part of its terms ([NATIVE_MODS.md](NATIVE_MODS.md)). PR #117 and its
  hardening implement framework only: index validation and signature
  primitives, the store and registry APIs, the enabled-list handoff, and
  the hook's fail-closed plan. No production key or index, player-facing
  installer, built-in native feature, or room terms on the wire exists, so
  players cannot install or use a native mod yet. Open: the key and signing
  workflow, the launcher's page, Big Maps as the first package, and the
  room's terms on the wire.

Dev C:

- [ ] Roads with lane connections, tram lanes and ramps; track types and
  underground segments.
- [ ] Landmarks, built once: the second on the same turn is refused and
  pays nothing.
- [ ] Warehouses, specialised terminals, maintenance facilities, noise
  barriers, pollution plants through the construction replay.
- [ ] Stations and depots: first place one on flat ground on two games
  and diff every edge's height. Module edits and upgrades.
- [x] Bulldozing a construction, charged on every game, with no demolition
  beyond what was asked (the TPF2 "heal" bug). *Done* (#15): a road depot
  and a street removed through the room on build 40408; each game removed
  the depot, its entrance edge and that edge's loose node, nothing more,
  and both accounts read the same.
- [x] The in-game Multiplayer window: players, speed, chat, and whether
  the worlds match. *Done* (#16), tried in two games through the deployed
  server; the lobby stays in the launcher (D17, D20).

## Part 4: after launch

- [ ] Linux: the hook and profile, passing Test A.
- [ ] Mixed-platform rooms, only for the pairs whose determinism matched.
- [ ] macOS: *added:* not at launch. The launcher refuses to start the
  game there (`tpf3mp-launch`), though mixed platforms remain a
  requirement (D2).
- [ ] A `Tf3Canonical` rule set on the server from `tpf3mp-canon`:
  validate, apply, save and restore (D6).
- [ ] Big maps: *open:* say what is wanted; [BIGMAPS.md](BIGMAPS.md) is
  what is known.
- [x] The auto-updater (D7; needs the owner's update key, OPERATIONS.md
  "Before the first release").
- [x] The server: deployed beside tf2mp-relay (OPERATIONS.md).
- [x] Building into the TF2 launcher: *decided by the owner* (D20): our
  own native window, drawn in the exact look of tearded's launcher as
  ported to TF3, with its release notes. No web view.
- [ ] *Added:* the minimap ([MINIMAP.md](MINIMAP.md)): TPF2 Big Maps'
  minimap for TF3, as a game bar plugin in the mod. First script only
  (towns, industries, network, stations, camera, click to move, companies
  and industry types); then the terrain picture rendered by the hook; in a
  room, other players' cameras and builds.
- [ ] *Added* (proposed by tearded, 2026-10-02, for the owner): other
  players' build previews in the game, as TpF2 Multiplayer showed them:
  what a player's road, track, station or building tool shows before the
  click, the others see in 3D, in the game's own blue or red, while it
  shows (HOOKS.md, "Build previews"). Advisory, never part of the world.
  The transport (protocol 17, bridge 24) and the hook's
  `UI::BuilderRenderer` for each other member on build 40408 are built
  (the game's own `ProposalViewer` fails fatally outside a tool's action)
  (investigation/TPF3_BUILD_PREVIEWS_2026-10-02.md). Tick once seen in the
  real game.
- [ ] *Changed:* (D17, the hold lifted by the owner on 2026-09-30): the
  room in the game. The main menu's Multiplayer window connects, creates
  and joins rooms, shows the players and their ready marks, chats and
  starts the room's game, through the launcher that started the game
  (docs/LOBBY.md). Built; tick once seen working in the real game
  (investigation/TPF3_INGAME_MENU_2026-09-30.md, section 6).
  *Proposed* (D24, for the owner): the launcher's window opens with the
  lobby in the game, starting the game and showing where things stand,
  with its own lobby one click away; the window picks the save a room
  starts from. *Proposed revision* (D24, 2026-10-02, for the owner): every
  lobby action in the game's Multiplayer window only; the launcher's window
  starts the game and shows status, its lobby kept hidden as a rescue for
  a menu the hook cannot reach.
- [ ] *Held* (D18, D19, D20): choosing versions and tracks, and a Dev
  track of untested builds. The owner decides after launch, once `dev`
  takes reviewed pull requests only.
