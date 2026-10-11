# Developing TPF3-MP

What the project is made of, where it stands, and how to build, run and
test it. Players start with the [README](../README.md) and
[PLAYING.md](PLAYING.md); server operators with
[OPERATIONS.md](OPERATIONS.md).

## How it works

- **The server orders everything.** A dedicated server puts every player's
  actions in one order, and every game applies them at the same simulation
  step. The host of each room chooses its rules: the game's own economy, as
  in single player, or canonical rules the server runs itself, with money
  no player can forge.
- **Each player's game is a replica.** It reports what it sees, and the
  server compares the reports and sends a replica that drifted the world
  the room agreed on.
- **Mixed platforms work.** The design never depends on different game builds
  simulating identically.

The full design is in [docs/ARCHITECTURE.md](ARCHITECTURE.md), the
reasoning behind it in [docs/DECISIONS.md](DECISIONS.md), the team's
plan from release day on in [docs/PLAN.md](PLAN.md), and what a
build has to carry to replay on another machine in
[docs/BUILDING.md](BUILDING.md); what a large world costs to load,
hold and save is in [docs/BIGMAPS.md](BIGMAPS.md). Players start
with [docs/PLAYING.md](PLAYING.md); server operators with
[docs/OPERATIONS.md](OPERATIONS.md).

## Status

**Milestone M1: the core netcode, tested without the game.**

- **Protocol.** QUIC with TLS 1.3, per-install Ed25519 identities proven
  against the TLS session, and a version preamble frozen for good. Where a
  network blocks UDP, the same QUIC connection runs through a WebSocket on
  port 443; clients fall back to it on their own.
- **Rooms.** Six-character invite codes, HMAC-tagged, and optional
  passwords, a lobby with
  readiness and content fingerprints, owner hand-over.
- **Sequencer.** Hard lockstep turns. A server-owned clock holds for players
  who are loading or slow, and stops waiting for one that stalls. Pause,
  speed, and exact resume after a reconnect, which survives a server restart.
  Long games' logs are compacted to the canonical state plus the last hour
  of turns, so a restart replays little and no log outgrows its disk.
- **Playout.** Each client plays behind its own jitter buffer, so a player
  feels their own round trip plus a small buffer. Paced bots over 150 ms
  round trips see a median of about 220 ms, and a poor link delays only its
  owner.
- **Protection.** Four adversarial security reviews (the server, snapshot
  transfer, tunnels and log compaction, and the player's side) found no
  critical issue. Every finding is fixed and guarded by a test:
  - per-address limits on sessions, handshakes, rooms and tunnels;
  - QUIC retries under load;
  - budgets for turns, payload bytes, logs and uploads;
  - crash recovery that never damages a log or resumes a client onto
    turns it did not see;
  - checkpoint verdicts one member cannot switch off;
  - bounded memory for a client whose server or game misbehaves;
  - decoders fuzzed with corrupted messages of every kind a peer sends.
- **Economy.** TPF2MP's economy core ported to integer arithmetic. All
  46,048 of TPF2MP's parity vectors replay identically against the original
  Lua.
- **Snapshots.** A deduplicating chunk store for world saves: 100 scattered
  edits to a 120 MiB save transfer 8.9 MiB. Rooms save together, agree on a
  save by its lane digests, and hand it to players who join a running game,
  return too late to resume, or diverge. The saved world survives a server
  restart.
- **Game bridge.** The agent drives the game through a step gate on the
  shared-memory link: events between exactly the right steps, steps on the
  jitter-buffered schedule, saves and loads on the room's word, and a rejoin
  after a lost server that the game sees only as a pause.
- **Hook engine.** Signature resolution with per-build profiles, an x86-64
  detour engine and a shared-memory link to the agent. All five known TPF2
  targets resolve uniquely. The launcher loads the hook into the game it
  starts, as TPF2MP's did, and into no other: nothing is installed into
  the game, and a hook the launcher did not start does nothing.
- **Test kit.** A toy game whose canonical rules run on the server, bots
  that play it through the real client, a fake hook that plays it through
  the real bridge, a lossy-network emulator and a load tester. 8 bots over
  a 150 ms, 2%-loss link agree on every lane, and 400 bots in 50 rooms run
  without a divergence. Fake games join running rooms, get rebased after a
  drift and ride out a server restart, and end in the same world.
- **Launcher.** tearded's TPF2 Multiplayer Launcher's look, as ported to
  Transport Fever 3, drawn natively with egui (`tpf3mp-launcher`, D20), on
  Windows, Linux and macOS, to connect, create or join a room,
  get ready, start, chat, and follow the game: fetching the world, loading,
  playing. It shows what to change when a player's mods differ, and the
  support code the server's log knows the player by. Its log lines go to
  the server by themselves, redacted, so an operator can help from the
  support code alone, with nothing for the player to send; a switch turns
  that off.
  `tpf3mp-agent collect-logs` zips the game's own logs and crash dumps
  when an operator needs them (never keys or tokens). It updates itself
  from the project's releases, installing only what the project signed.
  `tpf3mp-agent launcher` serves
  the same launcher as a page in the browser, on the loopback interface
  only, to the page that holds its secret token.
- **Installer.** Scripts players can read (`INSTALL_TPF3MP.cmd` with
  `tools\install.ps1`, `install.sh`) put the mod in the mods folder and
  take it out again. They refuse while the game runs, undo a failed step
  and delete nothing; CI tests them in Windows PowerShell 5.1 and in
  macOS's bash 3.2.
- **Operations.** Prometheus metrics with alerting rules, a hardened
  container image, a deployment runbook, and measured capacity: a busy room
  costs the server about a three-hundredth of a core. CI builds the release
  packages on all three platforms.
- **Players.** A guide to playing, in [docs/PLAYING.md](PLAYING.md).

**Waiting for the game:** the TPF3-specific hook (build profile, detours,
the real `Game`), the check of received saves, the server's rules for
TPF3's commands, and the release-day measurements in
[docs/DAY_ONE.md](DAY_ONE.md). [HOOKS.md](HOOKS.md) lists what remains.

## Layout

| path | contents |
|---|---|
| `crates/tpf3mp-proto` | Wire messages, framing and limits. |
| `crates/tpf3mp-canon` | Canonical rules. Integer arithmetic only, enforced by lints. |
| `crates/tpf3mp-net` | QUIC endpoints, TLS configuration, identities, framed stream I/O. |
| `crates/tpf3mp-server` | The dedicated server: rooms, sequencer, verdicts, metrics. |
| `crates/tpf3mp-agent` | The client library and CLI that run next to the game. |
| `crates/tpf3mp-launcher` | The launcher: its window (egui, in the page's look), what it shows (`view.rs`), its logs, its updater. |
| `crates/tpf3mp-snapshot` | Deduplicated storage and transfer of world saves. |
| `crates/tpf3mp-hookcore` | Signatures, per-build profiles and the detour engine. |
| `crates/tpf3mp-ipc` | The shared-memory link between the hook and the agent. |
| `crates/tpf3mp-bridge` | The messages and step gate between the agent and the hook. |
| `crates/tpf3mp-hook` | The library the launcher loads into the game it starts. |
| `crates/tpf3mp-launch` | Starts the game with the hook in that one process. |
| `crates/tpf3mp-modscan` | Sorts mods into personal, carried and shared, with the reasons, and finds the mods a player has installed ([MODS.md](MODS.md)). |
| `crates/tpf3mp-nativemods` | Native mods, proposed D29: the signed index, package resolution, the installer and its registry, what the hook enables, and the room's terms ([NATIVE_MODS.md](NATIVE_MODS.md)). |
| `crates/tpf3mp-bigmap` | Big maps, prototype: the size ladder, the ceilings a size hits, the terms a room shares, which features a build can run ([BIGMAPS.md](BIGMAPS.md)). |
| `crates/tpf3mp-testkit` | Toy game, bots, network emulator, load tester, regression harness. |
| `crates/tpf3mp-buildinfo` | The build scripts' helper: the commit, build time and build number built into the binaries, and their Windows version resource ("Which build is this"). |
| `mod/tpf3mp_1` | The game-side Lua mod, in Transport Fever 3's layout: captures builds as actions for the hook, linked to it by `tpf3mp/bridge.lua`. |
| `mod/tpf3mp_bigmap_1` | Big maps' New Game side, prototype: the added size rows. Registers nothing with the game yet. |
| `profiles/` | Exact-build signature profiles and native bundles for Windows Steam builds; `native-build.txt` selects the one compiled into this checkout. |
| `packaging/` | The install scripts and their tests, and the macOS bundle's files. |
| `tools/` | Release-day reverse-engineering and determinism probes. |
| `deploy/` | Container image and compose file. |
| `docs/` | Architecture, protocol, decisions, the team's plan, operations, release-day investigation. |

## Building and testing

How changes move from a feature branch through `dev` and `acceptance` to
`main`, where releases are drafted, is in [AGENTS.md](../AGENTS.md). Read it
before contributing.

Requires Rust. The toolchain is pinned in `rust-toolchain.toml` and installed
automatically by rustup.

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

To test in the real game, with several games in one room on one PC, see
[GAME_TESTING.md](GAME_TESTING.md).

### Contributor credits

Use the [pull request template](../.github/PULL_REQUEST_TEMPLATE.md) to
record request origin, implementation authors, and upstream sources separately.
Check commit author and Co-authored-by metadata rather than treating the PR
opener as the implementation author. Before publishing, review the generated
release notes and add only credits supported by the PR, commit, or project
records. The audit and correction method are in
[CONTRIBUTION_PROVENANCE.md](CONTRIBUTION_PROVENANCE.md).

### Game update builds

`profiles/native-build.txt` selects the reviewed native build directory. The
hook's build script and the update tools read that same selection: `hooks.toml`
and `native.rs` always belong to the selected bundle. For an update, review the
new profile, offsets, layouts and callback ABI, then change this one selection
on the feature branch ([HOOKS.md](HOOKS.md#reviewing-the-native-data-for-a-game-update)).

For the combined process, use `tpfre update`: it snapshots an installed
update or reads an existing archive, collects all archived file changes,
script changes, profiled hook/function changes and independent blockers, then checks/tests and builds only when its
exact reviewed native bundle is selected. Batch the diagnosed changes before
rerunning; use `--check-only` for the initial analysis. See the
[one-run command and reports](../tools/tpfre/README.md#one-update-run).
It never edits the game or automatically approves unknown ABI data.

Build `tools/tpfre` once with Cargo (an installed `quiet-cargo` wrapper is optional).
Then, from the repository root, run the standalone tool against a complete
private source archive:

```powershell
& ./tools/tpfre/target/release/tpfre.exe build --repo . --archive "$env:USERPROFILE/TPF3-MP-builds/25533170-sources" --jobs 2
```

This checks every archived file, the exact SHA-256, size and PE timestamp of
the selected profile, and every target including optional ones. Only after
success does it run the release build of launcher, agent, server and hook.
Missing/incomplete inputs, a different build or failed hooks stop before Cargo
starts. A saved verification report cannot substitute for the live check;
custom profiles cannot override the compiled bundle. An installed Preview is
not needed to verify an archived supported build.
The local build uses the machine's `quiet-cargo` wrapper automatically when
installed, so its build shares the queue and CPU cap with other sessions.

The `release` workflow runs the same `verify-build` check on pushes to
`feat/game-update-*`, `feat/upstream-update-*`, `dev`, `acceptance` and `main`, and on manual runs.
Packages are built on `main` or
manual runs with `verify_only` left off; a failed check blocks every package.
Other feature branches can run it manually with `verify_only` enabled. Set up:

- a dedicated private Windows or Linux GitHub Actions runner, with Git,
  Python 3.12+, bash and the pinned Rust toolchain. Default labels are
  `["self-hosted","Windows","X64","tpf3mp-game-builds"]`; set
  `TPF3MP_GAME_RUNNER_LABELS` to a JSON label array to select a Linux runner.
  `tools/github/verify_game_build.py` runs the live gate without a local wrapper;
- the repository variable `TPF3MP_GAME_ARCHIVE`, an absolute path to the
  complete source archive readable by that runner, outside its checkout.

During the 40408-to-40420 transition, keep `TPF3MP_GAME_ARCHIVE` on the 40408
archive. Set `TPF3MP_UPDATED_GAME_ARCHIVE` to the separate complete public
Steam build ID 25754343 archive on the same private runner before verifying a
40420-selected branch. The workflow chooses the archive from
`profiles/native-build.txt`; `tpfre verify-build` then checks the selected
bundle's SHA-256, size, PE timestamp and every target. An unknown bundle or
missing archive stops verification, while the old release stays verifiable.
The runner-label variable applies to all selected-build verification jobs in
this fork. It changes where static verification runs, not which native bundle
or archive is accepted.

Set up the runner and repository variable in each repository before merging
these release-workflow changes there. A runner registered to a fork and the
fork's variables do not configure upstream. Without the variable the workflow
fails; with an offline runner the verification job waits and packaging stays
blocked. Keep the gate enabled and complete the operator setup before using it.
The workflow requires a clean checkout of the exact release commit and records
it alongside the bundle file hashes and target results. Only
`game-build-verification.json` is
uploaded and attached to the draft release; game files, sources and indexes
remain on the private runner. Missing configuration fails before scheduling
the private check. A failed target check retains its static report as an
artifact for investigation while packaging stays blocked. An unavailable
runner holds packaging until the runner is
available. Ordinary CI compilation/testing needs no private game inputs.
The verification tool's Cargo target directory stays in the runner's tool
cache, outside the clean checkout. Repeated checks reuse compilation artifacts
while still checking the current commit and every private archive file afresh.
The private job is never triggered by pull requests. Register it only in the
trusted repository that holds the release workflow. Run `run.cmd` on Windows
or `run.sh` on Linux under its configured account; it must be online for checks
to finish. On a
non-admin machine it can start hidden at that account's Windows login instead
of being installed as a service. Keep runner credentials and archives outside
Git, and use the Release archive until a reviewed Preview bundle is selected.

For static work on `feat/game-update-*` or `feat/upstream-update-*`, the optional repository variable
`TPF3MP_CANDIDATE_GAME_ARCHIVE` names a second complete private archive.
When configured, the same private job also runs `tpfre verify` against all
candidate signatures, including optional targets, and uploads the separate
`game-candidate-verification.json` artifact. Missing or unsupported candidate
input fails the update check; an unset variable leaves this optional check out.
The active native selection and its mapped archive variable still govern packaging.
For Preview 40418, point the candidate variable at its complete private source
archive while keeping the active variable on Release. Signature success never
selects or approves the candidate's native ABI or runtime. Ordinary CI also
checks the candidate's pinned identity and the native hold without game files.

To verify a pushed feature branch without building or drafting a release:

```powershell
gh workflow run release.yml --repo OWNER/REPO --ref feat/my-update -f verify_only=true
```

Static success certifies profile bytes only. Native ABI review, real-game
acceptance and the feature → dev → acceptance → main gates still apply; no
platform gains game support from this check alone. Ordinary `cargo build`
remains available for development and is not a verified update build.

### Which build is this

Every launcher, agent, server and hook says which build it is. The build
scripts (`crates/tpf3mp-buildinfo`) build in the git commit
(`TPF3MP_COMMIT`, `unknown` outside a checkout), the build time
(`TPF3MP_BUILT`, from `SOURCE_DATE_EPOCH` when set) and a build number, the
count of commits up to the one built (`TPF3MP_BUILD_NUMBER`). Setting any of
them in the build's environment overrides it. The launcher logs them first
thing, with its file, `PROTOCOL_VERSION` and `BRIDGE_VERSION`
(`tpf3mp_agent::about::startup_line`), shows them in its footer and its
About panel, and names its file when a server refuses its protocol.

On Windows the four binaries carry a VERSIONINFO resource: the file
version is `Cargo.toml`'s `major.minor.patch` with the build number as its
fourth part (`0.1.0.442`), the product version is `Cargo.toml`'s, and a
`Commit` string holds the commit. Explorer shows them under
**Properties**, **Details**. The resource compiler comes from the Windows
SDK (embed-resource finds it); a machine without one builds without the
resource and says so in a warning. Linux and macOS builds are unaffected.

Windows Installer replaces a versioned file only with a higher file
version, so an installer built from a later commit replaces the old
binaries by itself. Two builds of the same commit carry the same version;
so does every build of a shallow clone, which has no build number (it
warns): fetch the whole history (`actions/checkout` with `fetch-depth: 0`,
as `release.yml` does) or set `TPF3MP_BUILD_NUMBER`. For a release, bump
`version` under `[workspace.package]` in `Cargo.toml` on a feature branch
as AGENTS.md says; the build number keeps counting under it.

Launchers that share a game link (`--game-link`, the same by default) meet
when the second starts (`tpf3mp_agent::launcher::instance`): the same build
does not start beside the first, another build asks the first to close and
takes its place, and an older build, or one beside a launcher too old to
say its build, refuses with both files named. Playtests that run several
launchers on one PC give each its own `--game-link`, and never meet.

Run a local server with a throwaway certificate, then connect to it:

```sh
cargo run -p tpf3mp-server -- --dev-self-signed runtime/dev-cert.der
cargo run -p tpf3mp-agent -- connect 127.0.0.1:29470 --pin-cert runtime/dev-cert.der
```

In production, the server takes a real certificate (`--cert`, `--key`), and
agents verify it against the public certificate authorities. See
[docs/OPERATIONS.md](OPERATIONS.md) for deployment.

Or use the launcher window:

```sh
cargo run -p tpf3mp-launcher -- --server 127.0.0.1:29470 --pin-cert runtime/dev-cert.der --name ann
```

Without `--server`, every build of the launcher, a developer's too, plays
on the player's server setting (Settings, **Server**), else on its default:
`TPF3MP_DEFAULT_SERVER` when it was built with one, otherwise the project's
relay (`setup::RELAY`). `--server` wins over both for the run, so playtests
stay on the local server whatever the setting says.

A package built with `TPF3MP_SERVERS` (or a launcher given
`--more-servers "US=us.example.org:29470"`) lists more servers besides
its default, and, on its default, plays on all of them: rooms listed from
each, created on the closest (D12's approved regional amendment;
LOBBY.md, "Several servers"). Room lists are merged across server pages,
and a room card carries its listed server into Join. `--server` turns that
off for the run. The release workflow validates its built-in server list
with the launcher's parser before packaging; the list allows at most eight
servers total and rejects invalid addresses, duplicate names or addresses.
`crates/tpf3mp-testkit/tests/regional.rs` plays it through with two local
servers and one that never answers.

(`tpf3mp-agent launcher` with the same options serves it as a page in the
browser instead.)

Try a room by hand with two agents:

```sh
cargo run -p tpf3mp-agent -- host 127.0.0.1:29470 --pin-cert runtime/dev-cert.der --name ann
cargo run -p tpf3mp-agent -- join 127.0.0.1:29470 <invite> --pin-cert runtime/dev-cert.der --name bob
```

Play a room through the whole stack, with a fake game in place of TPF3.
Each game process attaches to its agent over shared memory and runs the toy
game behind the step gate, as the real hook will:

```sh
cargo run -p tpf3mp-agent -- host 127.0.0.1:29470 --pin-cert runtime/dev-cert.der --name ann --game-link ann --start-with 2
cargo run -p tpf3mp-testkit --bin tpf3mp-fakegame -- ann --steps 100
cargo run -p tpf3mp-agent -- join 127.0.0.1:29470 <invite> --pin-cert runtime/dev-cert.der --name bob --game-link bob
cargo run -p tpf3mp-testkit --bin tpf3mp-fakegame -- bob --steps 100
```

Both games print the same lane digests at the end. A server with
`--data-dir` keeps world snapshots, so a third game can join the running
room (`join ... --game-link cat`); its game loads the room's world first.

Or let the multiplayer rig do all of that on one PC: it starts the games,
each with its own agent, data folder (`p1`, `p2`, ... under `--data-root`,
by default `tpf3mp-rig` in the temporary folder) and link name, has the
first create a room and the others join its invite, and starts the game
once everyone is ready. `--server local` runs a throwaway server in the
rig; any other server is given as `host:port`:

```sh
cargo build -p tpf3mp-testkit --bins
cargo run -p tpf3mp-testkit --bin tpf3mp-rig -- --players 3 --server local --steps 300
cargo run -p tpf3mp-testkit --bin tpf3mp-rig -- --players 3 --server 127.0.0.1:29470 --pin-cert runtime/dev-cert.der
```

Each game's output is printed under its player's name. The rig runs until
every game has exited, then checks that they all ended on the same lane
digests (failing if not); Ctrl-C stops everything it started, and so does
the end of `--time-limit <seconds>`, which then fails the run (a real
game still starting is ended once its start returns). `--game` takes the
path of a game executable instead of the fake game, with `--game-arg` for
its arguments. The rig starts each game with the hook in it, as the
launcher does (the hook built next to the rig, or `--hook`). On Windows
each game stays suspended until its hook says it is ready, for at most
`--hook-ready-wait` seconds (30 by default); the rig's tests load a
system library in the hook's place, which never says so, and give 0.
It tells each game its link, data folder and starter through
`TPF3MP_GAME_LINK`, `TPF3MP_DATA_DIR` and `TPF3MP_LAUNCHER_PID`, which the
hook reads (see "Several games on one PC" in [docs/HOOKS.md](HOOKS.md)).
It copies the build profiles in the user's data folder into each game's.
A server started with `tpf3mp-server` lets 8 sessions in from one address
by default: pass `--max-sessions-per-address` for bigger rigs.

For a playtest with the launcher on one PC, the launchers can get into a
room without clicking: the owner's with `--auto-create <room name>
--invite-file <file>` connects, creates the room and writes its invite to
the file; every other one with `--auto-join --invite-file <same file>`
waits for the file and joins. Both need `--server`. With
`--auto-start <players>` the owner's also starts the room's game once that
many players are in it and every one is ready. The dev server makes
a new certificate each time it starts, so start the launchers after it.

Nothing needs a click in the game either. `--auto-play` starts Transport
Fever 3 by itself, as the launcher's button does, once the launcher is in
the room. `--auto-load <save>` names a save in the game's save folder,
such as `mptest`. The owner's game loads it from its main menu, once, and
starts it with no Start Game to press. The launcher passes the name in
`TPF3MP_AUTO_LOAD`, and the hook loads it the way it loads the room's world
for a guest, logging `auto-load: ...` lines. A guest leaves the flag out:
it waits at the menu, is marked ready there, and gets the room's world when
the room starts. Several games run on one PC without a sandbox when each
launcher has its own `--game-link`, `--listen`, `--identity`, `--worlds` and
`--game-data-dir` (the hook's log and profiles, passed in
`TPF3MP_DATA_DIR`): the launcher already starts each game with `SteamAppId`,
so Steam lets it run beside the others. So a whole two-player playtest
starts from two commands:

```sh
tpf3mp-launcher --server 127.0.0.1:29470 --name james --auto-create playtest --auto-start 2 --invite-file invite.txt --auto-play --auto-load mptest
tpf3mp-launcher --server 127.0.0.1:29470 --name bob --auto-join --invite-file invite.txt --auto-play
```

`--start-save <save>` instead of `--auto-load` has every game, the owner's
too, load the save from its main menu at the same moment, with no game
loading it first or saving it for the room. It takes a save's name in the
game's save folder (`<Steam>/userdata/<account>/3493540/local/save/<save>.sav`,
of the account Steam names as playing, or the one account that has it) or
a file's path. The owner's launcher reads the file when it starts, and
when it creates a room hands it to the room in the lobby, as it uploads a
save the room asks for ("The first world" in [PROTOCOL.md](PROTOCOL.md)).
The owner's game then waits at its menu like a guest's, and is marked
ready there once the room has the save; `--auto-start` starts the room
once everyone is. It needs a server that keeps worlds; the old way, above,
still works without one.

```sh
tpf3mp-launcher --server 127.0.0.1:29470 --name james --auto-create playtest --auto-start 3 --invite-file invite.txt --auto-play --start-save twomptest
tpf3mp-launcher --server 127.0.0.1:29470 --name bob --auto-join --invite-file invite.txt --auto-play
tpf3mp-launcher --server 127.0.0.1:29470 --name cat --auto-join --invite-file invite.txt --auto-play
```

With Transport Fever 3 itself (build 40420), two games run on one PC like
this:

```sh
tpf3mp-rig --players 2 --stagger 75 --wait-for-games --no-snapshots --server local     --game "<Steam>/steamapps/common/Transport Fever 3/TransportFever3.exe" --game-build 40420
```

- `--stagger 75` starts each game 75 s after the one before: two started at
  the same moment failed while setting up their graphics. The later games
  start while the room is set up, so every hook finds its link.
- `--wait-for-games` starts the room's game only once every game has
  attached, so each is in it from the start.
- `--no-snapshots` runs the local server without a snapshot store, so every
  player loads the world it starts from itself (`Load` without a file):
  every game must load the same save. The fake games all take seed 0.
  Without it, as on a real server (`--server host:port`, which keeps
  worlds), the owner's game saves its world for the room, and every other
  game loads that save through its GUI ("The room's world" in
  [HOOKS.md](HOOKS.md)). The GUI runs only in a world, so each of those
  games must first be in one of its own, whichever.
- The games share the Steam user's folder, and so its `settings.lua` and
  log. For tests, set `debugMode = true` (the console), `screenMode =
  "WINDOWED"` with `windowSize = { 2560, 1440 }` (smaller, and the
  console's input line falls off the window) and a long
  `autosaveIntervalMinutes`, and put the player's own settings back after.
- Load the world in each game from its console only after the rig says
  `game started`; a game in its world before the room starts runs on its
  own. What ran on release day: a fixture save made with
  `app.startGame` (seed `tpf3mp`, 16 by 16 tiles, the tutorial off:
  `guideSystemConfig.tutorial` 1) and `app.saveGame`, loaded in both with
  `app.loadGame`. Both played the room's world from step 1 at its pace,
  and the determinism probe's samples matched at every step.

Load-test a server with bots:

```sh
cargo run --release -p tpf3mp-testkit --bin tpf3mp-loadtest -- --rooms 50 --bots 8
```

Play the regression scenarios: actors build, buy vehicles, make lines and
assign them, two or more games to a room, checked as they go (see
[docs/REGRESSION.md](REGRESSION.md)):

```sh
cargo run --release -p tpf3mp-testkit --bin tpf3mp-regress
```

### Owner-authorized fast-track releases

A manual `release` workflow dispatch accepts `fast_track=true` to package an
already validated native bundle without the private verification runner.
This is an explicit owner override, not the default: pushes and ordinary
manual runs still require private game verification. Package compilation,
embedded update-key checks and release signing remain enabled. Version 1.2.5
uses this override at the owner's request; its selected native bundle remains
Steam Windows build 40408. The airport probe fixtures now include the active
vehicle component state required by the probe, retaining unreadable-state
refusal and full lane comparison assertions.
