# The native hook

Current ReedME integration: protocol **24**, bridge **30**, action schema
**28** and room-log format **11**. Earlier numbered contracts and dated
proofs below remain their original history. This incompatible refresh
requires fresh data and snapshot directories, preserving old directories
with their matching binaries. Upstream game acceptance reports are not a
new fork playthrough; the Proton competitive/recovery gate remains open.

Acceptance status: line waypoints passed ordinary two-player game
acceptance on 2026-10-16 and are enabled. Entity rename/recolour
([investigation/TPF3_RENAME_2026-10-07.md](../investigation/TPF3_RENAME_2026-10-07.md))
and subsidies
([investigation/TPF3_SUBSIDIES_2026-10-07.md](../investigation/TPF3_SUBSIDIES_2026-10-07.md))
passed it on 2026-10-07.
See [COVERAGE.md](COVERAGE.md) for the selected integration.

The native hook is the small library that runs *inside* the game process. It
captures and cancels player commands, gates the simulation step, controls speed
and save/load, and talks to the [agent](ARCHITECTURE.md#components) over
shared memory. This document specifies the parts built at milestone M0: the
build-signature engine, the per-build profile format, the detour engine, the
shared-memory ABI, and the release-day procedure. Locating and detouring the
actual TPF3 functions comes with the release-day profile (see
[DAY_ONE.md](DAY_ONE.md)); everything the hook needs to do it is here and
tested.

Crates:

| crate | contents |
|---|---|
| `tpf3mp-hookcore` | pattern scanning, per-build profiles + resolution, the x86-64 inline detour engine, a small read-only PE reader |
| `tpf3mp-ipc` | the shared-memory link (this document's ABI) |
| `tpf3mp-hook` | the `cdylib` loaded into the game: platform entry points, profile loading, agent connection |
| `tpf3mp-launch` | starts the game with the hook in that one process, and nowhere else |

## How the hook gets into the game

Only the TPF3-MP launcher puts the hook into the game, into the game it
starts for a room, and for as long as that game runs (D11 in
[DECISIONS.md](DECISIONS.md)). Nothing is installed into the game's folder
and no launch option is set: the game started from Steam is the plain game.

- **Windows.** `tpf3mp-launch` starts the game suspended, writes the hook's
  path into it, and runs `LoadLibraryW` on a thread it creates in the game.
  It checks the thread's result and the game's module list, and only then
  lets the game run. If the hook is not there, it ends the game rather than
  let it run without it. This is how TPF2MP's injector started Transport
  Fever 2 (`--launch`).
- **Linux.** The game starts with `LD_PRELOAD` naming the hook, in its own
  environment only, ahead of anything already preloaded (Steam's overlay).
- **macOS.** Not yet. The game's hardened runtime refuses libraries it did
  not load itself; how to get the hook in is a release-day question
  ([DAY_ONE.md](DAY_ONE.md)).

Before starting the game, the launcher checks four things, and refuses if
any fails:

- the player is in a room;
- the game and the hook are where they should be;
- Steam is running;
- no game it started is still running.

It gives the game `SteamAppId` and `SteamGameId` (3493540), so that the
game does not restart itself through Steam, which would start it without
the hook.

The hook runs only when the launcher started the game. The launcher names
two things in the game's environment:

- its link, `TPF3MP_GAME_LINK`;
- its own process, `TPF3MP_LAUNCHER_PID`.

Without the link the hook returns at once: it writes, hashes and opens
nothing. On Linux and macOS it also returns when its process's parent is
not that launcher. Programs the game starts, a browser opened from the
game for example, inherit `LD_PRELOAD` and the variables; the hook stays
out of them. On Windows nothing the game starts loads the hook.

## Design and the fail-closed rules

TPF3 will be patched often after launch, so the hook never pins raw addresses.
It carries one **profile** per game build. A profile binds a build identity
(executable SHA-256, and optionally file size and PE timestamp) to a set of
named **targets**, each located by a byte **signature** rather than an address.

Resolution is **fail-closed**. `tpf3mp_hookcore::profile::resolve` returns either
a complete, verified target table or a precise refusal, and it installs nothing
on the way to a refusal:

- **Unknown build** - the running executable's identity does not match the
  profile. The hook does not scan at all; multiplayer is disabled.
- **Missing** - a *required* target's signature is not found. Resolution fails
  as a whole, so required hooks are all-or-nothing; a partial install never
  happens.
- **Ambiguous** - a signature matches more than once. Refused, even for an
  optional target: a second, unexpected match is a corruption signal, not
  something to skip.
- **Prologue mismatch** - the signature matched but the exact bytes at the
  target are not the ones the profile expects. Refused.

An *optional* target that is simply absent is recorded and does not fail the
profile. Everything else fails closed. The hook logs the precise reason and
leaves the game untouched.

### Where the resolver scans (production vs. this repo's test)

`resolve` takes a byte slice plus the address its first byte corresponds to, so
it does not care whether those bytes come from a file or from memory.

- **Production**: the hook scans the running process's **mapped, unpacked module
  image** - the bytes the loader (and any DRM stub) produced in memory - passing
  the module's base address as the region base. This is the only correct source
  when a build's code section is packed or encrypted on disk.
- **This repo's static proof** (`tpf3mp-hookcore/tests/tpf2_static_proof.rs`)
  scans the executable **on disk**. That is a development convenience, valid
  only when the build's `.text` is readable on disk (see
  [the TPF2 verification](#what-was-verified-on-the-tpf2-binary)). The call is
  identical; only the byte source differs.

## Signatures

A signature is an IDA-style pattern: two hex digits per fixed byte and `??` (or
`?`) for a byte that may be anything, for example `48 8B ?? ?? E8`. Wildcards
exist so a signature skips the bytes that move between builds - RIP-relative
displacements, call targets, absolute addresses - and matches only the opcodes
and operands that identify the code. A signature must be **unique** across the
scanned region; the scanner reports zero, one, or many matches, and the resolver
treats "many" as a refusal.

Rules of thumb, applied to the TPF2 profile below:

- Prefer register/immediate operands; wildcard every relative or absolute
  displacement.
- Extend the pattern only as far as needed to make it unique. Two functions can
  share a prologue (the two TPF2 menu functions share a seven-`push` opening);
  run the signature to the first distinguishing bytes.
- Keep the **prologue** field free of wildcards: it is the exact code the detour
  engine relocates, and it is re-checked byte-for-byte after the scan.

## The profile format

A profile is TOML. `tpf3mp_hookcore::profile::Profile::from_toml` parses and
validates it.

```toml
name = "Transport Fever 2 Build 35924 (Windows x64)"
image_base = 0x140000000   # informational: what RVAs are relative to
region = ".text"           # informational: the section the resolver scans

[build]
sha256 = "782b904a8f7bbdac1f7a18528f1a5c778691e5aa3087c37c351bf6912585175c"
size = 72843280            # optional; checked when present
pe_timestamp = 0x675ABCC6  # optional; checked when present

[[target]]
name = "GameSim::Step"
signature = "40 53 41 56 48 83 EC 68 48 8B DA 4C 8B F1 48 81 FA E8 03 00 00"
offset = 0                 # bytes from the match to the target (default 0)
prologue = "40 53 41 56 48 83 EC 68 48 8B DA 4C 8B F1 48 81 FA E8 03 00 00"
required = true            # default true
```

- **`signature`** locates the target. **`offset`** (signed, default 0) is added
  to the match position to reach the target address, for the case where a
  signature must begin before or after the function it names.
- **`prologue`** is the exact, wildcard-free bytes expected at the target; the
  resolver verifies them and the detour engine relocates them.
- **`required`** (default `true`): a required target that does not resolve
  refuses the whole profile.

A resolved target's address is `region_base + match_index + offset`.

## The detour engine

Trampolines are allocated within 1 GiB of their target, so a relocated
RIP-relative operand keeps a 32-bit displacement to the data it addresses.
On Windows the free regions around the target are walked; on Unix, mmap
hints step away from it. A trampoline is written while read-write, then
turned read-execute; it is never writable and executable at once. On
Windows, the prologue read stops at the end of the target's memory region.
A relocation that still cannot reach fails the install cleanly
(`DetourError::Encode`); it never produces wrong code.

`tpf3mp_hookcore::detour::InlineDetour` is an x86-64 inline hook. Installing it
overwrites a function's first instructions with a jump to a replacement, after
copying those instructions into a **trampoline** that ends by jumping back into
the function; calling the trampoline therefore runs the original.

- **Relocation.** The stolen prologue is decoded and re-encoded at the
  trampoline's address with iced-x86's block encoder, so a RIP-relative operand
  keeps addressing the same absolute memory from its new home. A prologue that
  cannot be relocated - it branches, returns, or does not decode - is **refused**
  (`DetourError::UnsupportedPrologue`) rather than patched wrong. Only
  straight-line instructions are stolen.
- **Patch form.** A near replacement (within 2 GiB) is reached with a 5-byte
  `jmp rel32`; otherwise a 14-byte `jmp [rip+0]` absolute jump. The trampoline
  always returns with an absolute jump, so it works at any distance.
- **iced-x86, not `retour`.** `retour`'s stable line is 0.3.1 (0.4 is alpha) and
  it owns trampoline allocation and instruction relocation internally - exactly
  the part that must be inspectable and testable on a binary that shifts every
  patch. iced-x86 is a pure-Rust decoder/encoder with no build script; the
  engine drives decode/relocate directly and keeps the trampoline and patch
  bytes in this crate, where tests read them.
- **Architecture.** The engine is x86-64 only. On any other architecture it
  compiles to a stub that returns `DetourError::UnsupportedArchitecture`, so the
  workspace still builds and the caller fails closed (see
  [macOS arm64](#macos-arm64)).

### Thread-safety assumptions

Installing overwrites up to fourteen live code bytes with a non-atomic copy. The
caller must guarantee the target cannot execute during install or uninstall:
**install before the target's first run**, or **park every thread that could
reach it first**. The launcher loads the hook before the game's entry point
runs (into the suspended game on Windows, by `LD_PRELOAD` on Linux). The hook
installs from its bootstrap thread while the game starts, as TPF2MP's injector
let its worker do: the targets run only once a world is loaded, long after.
The engine does not stop threads itself. Detours are removed by
dropping the handle (or `detach`), under the same quiescence rule. The engine's
own tests only ever hook functions inside the test binary, never another
process.

## The `tpf3mp-ipc` ABI

The hook and the agent share one memory mapping: a fixed 64-byte header followed
by two single-producer/single-consumer ring buffers. This section is
byte-exact, because the agent is written separately.

### Object naming and security

The logical link name is mapped to a per-user OS object:

- **Windows**: `CreateFileMappingW`/`MapViewOfFile` in the per-session `Local\`
  namespace, object name `Local\tpf3mp.<hash>` where `<hash>` includes the user
  name. No explicit security descriptor is passed, so the mapping gets the
  process token's default DACL - access for the creating user and SYSTEM only.
- **Linux/macOS**: `shm_open`/`mmap` with mode `0600` (owner only). The name is
  `/tpf3mp.<hash>`, where `<hash>` includes the uid; it is kept within macOS's
  31-character `shm_open` limit.
- **Other users** cannot reach the link. **The same user's processes can.**
  Creation is not exclusive, because the agent re-creates the mapping after
  a restart while the game still holds it (see "Restart"). A process of the
  same user could therefore create the object first, or write into it. That
  process can already debug or inject into the game, so this opens no new
  boundary. Each side still treats the other's bytes as hostile:
  - `open` refuses sizes `create` would refuse;
  - no frame is read beyond its ring;
  - a producer refuses a consumer index claiming more than the ring holds;
  - `next_len` never exceeds `max_message`.

  A hostile peer can garble or stall the link, but cannot make this side
  read or write out of bounds (`tpf3mp-ipc/tests/poc_hostile_peer.rs`).
  Nor can it make the agent take in, and so delete, a file other than a
  save in the directory the agent named (`tests/hook_save_path.rs` in
  `tpf3mp-agent`). A game that stops reading makes the agent stop taking
  the room's turns once about 16 MiB wait for it, rather than hold them
  all.

### Header layout (little-endian)

Total mapping size is `64 + 2 * ring_capacity` bytes.

| offset | size | field | notes |
|---|---|---|---|
| 0  | 4 | `magic` | `T3MP` (bytes `54 33 4D 50`), written **last** as a readiness flag |
| 4  | 4 | `abi_version` | currently `1` |
| 8  | 4 | `header_size` | `64` |
| 12 | 4 | `ring_capacity` | bytes per ring; power of two, `<= 2^31` |
| 16 | 4 | `max_message` | largest payload per message |
| 20 | 4 | `session` | non-zero link generation; changes on re-create |
| 24 | 4 | `hook_pid` | 0 until the hook attaches |
| 28 | 4 | `agent_pid` | 0 until the agent attaches |
| 32 | 8 | `hook_heartbeat` | `u64`, bumped by the hook |
| 40 | 8 | `agent_heartbeat` | `u64`, bumped by the agent |
| 48 | 4 | `h2a_head` | hook->agent read index (consumer: agent) |
| 52 | 4 | `h2a_tail` | hook->agent write index (producer: hook) |
| 56 | 4 | `a2h_head` | agent->hook read index (consumer: hook) |
| 60 | 4 | `a2h_tail` | agent->hook write index (producer: agent) |

Then the data areas: hook->agent at `[64, 64 + ring_capacity)`, agent->hook at
`[64 + ring_capacity, 64 + 2 * ring_capacity)`.

### Rings

Each ring is a byte stream carrying length-prefixed messages: a little-endian
`u32` payload length followed by that many payload bytes. Both the length and
the payload may wrap around the end of the buffer.

- `head` and `tail` are **free-running** `u32` counters (they wrap at `2^32`,
  not at the capacity). Bytes in the ring = `tail - head` with wrapping
  subtraction; this is correct because capacity is a power of two `<= 2^31`. The
  index into the data area is `counter & (capacity - 1)`.
- **Producer**: writes the payload bytes, then stores `tail` with **Release**.
  It reads `head` with **Acquire** to compute free space; it only writes `tail`.
- **Consumer**: reads `tail` with **Acquire**, reads the bytes, then stores
  `head` with **Release**. It only writes `head`.
- A message larger than `max_message` is rejected by the producer; a full ring
  returns "full". Nothing is allocated on either side of a send or receive.

### Startup, heartbeat and restart

- **Startup.** One side (in TPF3-MP, the agent) is the owner: it creates the
  mapping, zeroes the header, writes the ABI version, ring sizes and a fresh
  non-zero `session`, sets its pid and heartbeat, and **publishes `magic` last**
  with a Release store. The other side opens the mapping and reads `magic` with
  an Acquire load; until it appears the open returns "not ready". The opener
  then checks `abi_version` and `header_size`, reads the ring sizes, and sets
  its own pid and heartbeat. The hook fails closed (runs solo) if no mapping is
  present.
- **Heartbeat.** Each side bumps its own counter and reads the peer's. A counter
  that stops advancing means the peer is gone.
  The agent gives a loaded game's hook 60 s (a load 600 s). A game that
  freezes longer (a big map can, while it builds terrain and textures)
  loses its agent. Before any room's game began, the hook then lets the
  game play on alone and logs that it cannot join a room until restarted
  from the launcher; in a room's game it holds the world, as before.
- **Restart.** The owner re-creates the mapping with a new `session`. A peer
  that sees `session` change knows the rings were reset and drops anything in
  flight, then re-syncs from the new generation. The launcher does this for
  every room session (`begin_session`): leaving one room and creating or
  joining another re-creates the link while the game runs on. On Windows
  the new owner re-initialises the very mapping the game still holds; on
  Linux and macOS the old owner unlinks the name when it lets go, and the
  new link is another object, so a peer finds the next generation by
  opening the link again by name. What the hook does about a new
  generation is in "Following the launcher from room to room" below.

### Several games on one PC

The hook opens the link its launcher names, and keeps its log (`hook.log`)
and build profiles (`profiles/*.toml`) in the per-user `TPF3-MP` data
folder. It also carries the release's own profiles, built in from the
repository's `profiles/` folder (Transport Fever 3 Steam build 40408 on
Windows, so far); a profile in the data folder for the same build comes
first, so one can be tried there without a release. Per-build directories
with `hooks.toml` are supported too; flat custom profiles stay supported.
The game's environment says which, so several games on one PC each
reach their own agent:

| variable | effect |
|---|---|
| `TPF3MP_GAME_LINK` | the link name to open, the launcher's `--game-link` (`tpf3mp.default` unless given); without it the hook does nothing |
| `TPF3MP_LAUNCHER_PID` | the process that started the game; on Linux and macOS, the hook does nothing in a process whose parent is another |
| `TPF3MP_DATA_DIR` | the folder for the hook's log and profiles; unset or empty, the per-user one |

`tpf3mp-fakegame` reads `TPF3MP_GAME_LINK` too, when no link is given on its
command line. The multiplayer rig (`tpf3mp-rig`, in
[DEVELOPMENT.md](DEVELOPMENT.md)) sets all three for every game it starts, and starts a real
game with the hook in it as the launcher does.

### Reviewing the native data for a game update

The compiled Windows native data is grouped with its signature profile in
`profiles/tf3_build40420_steam_windows/` (the original 40408 bundle remains
for historical verification): `hooks.toml` records the executable
SHA-256, size and PE timestamp; `native.rs` records the game/Steam build and
exports each subsystem's data file. Those files hold splice bytes, frame and
field offsets, structure layouts and GUI register/site descriptions. The
hook modules reexport the existing names, so their callers and behavioral
tests use the same data. Algorithms, bounds, kill switches and generic x86
instruction decoding remain in the hook modules.

`profiles/native-build.txt` explicitly selects the one compiled native
bundle. The hook build script generates `build_data.rs`'s native module and
built-in profile from this same selection; the release check reads it too.
Bootstrap checks its complete executable identity before
installing the menu or step gate. A TOML profile for another executable is
insufficient: it cannot enable that build with the previous build's native
layouts. Custom profiles for the supported executable keep their priority.
This feature selects Windows Steam build 40420. The released main branch still
selects 40408 until the normal acceptance and promotion gates are passed;
this change does not approve Preview or another platform.

A static signature candidate for Steam Preview 40418 is in
`profiles/tf3_build40418_steam_windows/hooks.toml`. Its original 145 targets matched
the private archive. The additional optional `lua_touserdata` signature awaits
an archive recheck. The 40408 bundle has seven optional performance targets
added after that candidate was prepared. They remain unverified on Preview
while its private archive is unavailable. The candidate directory deliberately
has no `native.rs` and is not selected. Its changed splice bytes and ABI work
are recorded in [the Preview investigation](../investigation/PREVIEW_40418_2026-10-04.md).

For a new build, create a separate bundle directory, investigate the audit's
signature/function/script changes, and review its native data alongside its
`hooks.toml`. Select that reviewed bundle explicitly in `native-build.txt`.
Also review the hook code's ABI assumptions (calling conventions and the
instructions emitted by callbacks); grouping data does not prove those are
unchanged. Use `tpfre build` for the checked local release build; the release
workflow also requires `verify-build` against its private archive before
packaging ([DEVELOPMENT.md](DEVELOPMENT.md#game-update-builds)). Run the normal
tests and the authorized real-game acceptance before promotion. The archive and audit
commands are described in [tpfre](../tools/tpfre/README.md#game-update-workflow-windows-pe-builds).

## The bridge: what travels over the link

`tpf3mp-bridge` defines the messages, postcard-encoded, one per ring frame,
at most 60 KiB each. It has no async runtime or network code, so the hook can
link it. The agent's side is `tpf3mp_agent::bridge`.

- **From the agent (`ToHook`):**
  - `Hello`: always first.
  - `Begin`: a game starts, and saves go in this directory. Bridge 30 also
    carries the stable room ID and exact agreed scan plan before any load;
    the hook preserves same-room company-password provenance and refuses a
    local scan-plan mismatch. It also names the room's rules: with `native`, the game's own economy runs untouched;
    with canonical rules, the Lua mod shows the server's values instead. And
    the local player, as the room's events name the actor: the hook knows
    the player's own commands by it when the room orders them (bridge
    version 5). And the mods the room's worlds load with in this game:
    the room's shared mods and this player's personal ones, or none when
    the agent does not know this player's mods ([MODS.md](MODS.md);
    bridge version 12).
  - `Load { file, next_step, world_epoch }`: load a world, then run
    `next_step`. The producing-world epoch stays attached to its reports.
    Without a file, the game loads the world the player chose to start
    from: the owner's, or everyone's on a server that keeps no snapshots.
    With one, a save the room agreed on: the owner's world at the start
    (the save the owner named for the room, or the one the owner's game
    saved), or the room's latest for a player who joins a running game,
    could no longer resume, or is rebased after diverging. Everything sent before
    a load is void.
  - `Apply(event)`: apply this event before its step. A `Save` event is not
    applied: the session saves the world there (see below).
  - `Release { through }`: steps up to and including this one may run.
  - `Speed`: the room's speed, for display only: sent with the game's
    first turn, whatever the speed, and whenever it changes.
  - `Diverged`, `Refused`: tell the player.
  - `Chat { from, text }`: a member of the room said something. Sent only
    once the game has begun; talk in the lobby stays in the launcher.
  - `Room(RoomInfo)`: the room as it stands (its name, owner, and members
    with their names and whether they are connected), for the game's
    Multiplayer window: sent when the game begins and whenever the room
    changes (bridge version 6).
  - `Lobby(LobbyView)`: the launcher's lobby as it stands (its
    connection, server, the player's name, the room with its invite,
    members, ready marks and owner, the newest 40 chat lines, the last
    error and notice), for the Multiplayer window on the game's main menu
    (D17): sent whenever it changes, before, during and after a room's
    game; only the newest counts (bridge version 9). Since bridge version
    10 it also carries the rules the server offers, the player's saves
    (newest 40, by name; since version 22 the newest 100, the player's
    own only: no `autosave_…` or `tpf3mp_…` copies) and the one offered
    first (`start_save`), where
    the room's world is in this game (`world`: none, fetching with its
    bytes, loading, playing) and how the game differs from the room's.
    Since bridge version 14 it carries the page of the server's public
    rooms last asked for (`rooms`). Since version 16 each member carries the banner they
    picked, and the view the player's own (`banner`); `SetBanner` sets it
    (`set_banner` from the window, empty for the default). Since version 17 the room, the
    room list and create carry the play style (`competitive`). Since version 18 each member
    carries where their game is with the room's world while it comes in (`loading`:
    `fetching` with a `percent`, or `loading`, in the window's state). Since version 19 it
    carries the campaign portraits this game has (`portraits`, LOBBY.md "Portraits"), and a
    banner id of up to 32 bytes may name one; a member's portrait this game lacks is left
    out, for their default. Since bridge version 15 it carries the
    server's address (`server_address`) and the launcher's default server
    (`server_default`), for the server setting. Since version 23 it
    carries the launcher's log session (`log_session`, empty while
    diagnostics are off), which the window shows with a Copy on its first
    page and its Server page (approved D10 amendment). Since version 26
    the room list carries the release's servers when the launcher plays
    on several (`servers`: name, ping, here, reachable) and each room its
    server's name and ping (`server`, `ping_ms`; LOBBY.md, "Several
    servers"). Since bridge version 27 a room-card Join also carries the
    listed server name, so duplicate invite codes still select the card the
    player clicked. Bridge version 28 adds the release's trusted servers to
    the top-level lobby view before room browsing, for region choices on
    typed invites.
  - `End`: the session is over. Sent only once the room's game has begun:
    a room left before that ends nothing in the game, which keeps its link
    for the player's next room.
  - `Preview { from, preview }`: what another member's build tool shows
    now, an action's payload, or `None` once it shows nothing (bridge
    version 24; "Build previews" below). Sent only while the game plays the
    room's world, and only the latest of each member: one still waiting in
    the agent's queue is replaced. The gate drops one that arrives while a
    world loads.
- **From the hook (`ToAgent`):**
  - `Hello`: always first, with the game build.
  - `Loaded { next_step, world_epoch }`: the ordered world is loaded.
    On exact resume the agent re-acknowledges the original load frontier
    before reporting current progress; an offered world is not an ack.
  - `Command { payload, secret }`: the player acted; the room orders it.
    `secret` is a company's password the action needs (joining or locking
    a company), which the agent sends beside the intent and the room seals
    (PROTOCOL.md, "Secrets"); the hook never logs it. Bridge version 11,
    which also carries each ordered `Command`'s seal to the hook.
  - `Ran { step, world_epoch }`: the game ran this step.
  - `Checkpoint { step, lanes, world_epoch }`: digests at a checkpoint.
  - `Saved { event, lanes, file, world_epoch }`: the world as saved at a save event, and
    its digests there; no file if saving failed. The file must be in the
    directory `Begin` named; the agent takes in no other.
  - `Chat { text }`: the player says something to the room
    (`Session::chat`).
  - `Speed { speed }`: the player picked this speed in the game's speed
    row (`Session::request_speed`); the agent asks the room, which takes it
    from the owner only. Bridge version 4.
  - `WorldUp { world }`: before the room begins a game, the game has a
    world up with the mod linked, and steps it (`Session::world_up`).
    `world` counts the worlds whose GUI started (`tpf3mp_native.world`)
    since the hook began, so each is told once. In the room's lobby the
    agent marks the player ready (`Request::SetReady(true)`, as the Ready
    button does), once a world: a player who then presses Not ready stays
    so until another world is up. Nothing is marked once the room's game
    began, while the game's world is being replaced, or before the agent
    knows the room is in its lobby; the step gate sends nothing after
    `Begin`. Bridge version 7.
  - `MenuUp { menu }`: before the room begins a game, the game is at its
    main menu with no world up, and will load the room's world from there
    (`Session::menu_up`; "Loading from the main menu" below). `menu`
    counts the game's arrivals at its menu since the hook began; each is
    told once per room session, so again to the launcher's next room. In
    the room's lobby the agent marks a player other than the room's owner
    ready, once per arrival, if it keeps worlds. The owner only once the
    room has the save the owner named for it to start from
    (`BridgeOptions::start_world`, the launcher's `--start-save`); without
    one never, as the owner's game must have the world everyone plays up
    to save it for the room. Otherwise as `WorldUp`. Bridge version 8.
  - `Lobby(LobbyAction)`: the player pressed a button of the main menu's
    Multiplayer window: connect (a name; the server is the launcher's,
    D12), disconnect, create, join, ready, start, kick, chat or leave. The
    launcher carries it out as if its own window had asked (bridge
    version 9). Since version 10, create also names the rules and the save
    the room starts from: one of the saves the window was offered, by name
    only (never a path); absent for the launcher's own `--start-save`,
    empty for none. Since version 14, create also carries a room's
    listing (its climate and year, for a public room) and `ListRooms {
    page }` asks for a page of the server's public rooms. Since version 15,
    `SetServer { server }` is the player's server setting: a `host:port`,
    or empty for the launcher's default; the launcher checks, remembers
    and reconnects, and refuses it in a room (D12, proposed amendment).
    Connect still names no arbitrary server. On a regional release, a typed
    invite is resolved across the compiled trusted servers; a pasted server
    prefix is not routing authority.
    Since version 20, `ChooseStart { save, map, year }` is the room
    owner's pick of the save the room starts from, in its lobby (empty
    for none; LOBBY.md, "Changing the start save in the room"), and the
    lobby's room carries the save it starts from and the owner's upload
    of it. Since version 25, `ChooseRoomMods { save, map, year, mods,
    params }` is the owner's pick on the game's Load Game page: the save
    (absent to keep the room's), the mods in the game's load order with
    their names and sources, and the settings of the room's mods and the
    game's own (id `""`) (LOBBY.md, "The room's save and mods"); and
    `RescanMods` asks the launcher to find the installed mods again after
    an install from Mod Hub. The lobby carries the room's mods with this
    player's version of each and whether they have it (`LobbyRoomMod`),
    and how each member's game differs (`LobbyMember::differs`); `Begin`'s
    `ModLists` carry the room's settings of its mods.
  - `Log`: a line for the agent's log.
  - `Preview { preview }`: what the player's build tool shows now, for the
    other members, or `None` once it shows nothing (`Session::preview`;
    bridge version 24). The agent sends it on in the room's game only, and
    not one over `MAX_PREVIEW`.
- **The step gate.** The game asks the hook's `Gate` before every step. Until
  the step is released, the hook reads messages and applies each event the
  gate hands over, so an event for step `s` is applied after step `s - 1`
  and before step `s`, never mid-step.
- **Ordering.** The agent sends every event for step `s` after the release of
  step `s - 1` and before the release of step `s`. It only merges releases
  of consecutive steps with no event between them. The hook stops reading
  once its next step is released. The gate refuses anything that breaks
  this: an event for another step, an event after its step's release, or a
  release that goes back. The hook must then stop following and say so.
- **Pacing.** The agent releases steps on its jitter-buffered schedule
  (`Playout`). The game runs released steps at its own call pace, an even
  share of them each call ("Even steps" below), and waits at the gate for
  the next one. It reports each step it ran; the agent reports progress to
  the server from that, at most every 20 ms.
- **Liveness.** The hook must beat its heartbeat from a thread of its own,
  since the game thread blocks while loading. The agent gives up on a hook
  whose heartbeat stands still for 60 s, or 10 minutes while the world
  loads (`BridgeOptions::hook_timeout`, `load_timeout`), and ends the
  session with "the hook stopped responding". A game the launcher started
  is also watched as a process: once its hook attached, the launcher tells
  the bridge within half a second of the process exiting
  (`Control::GameClosed`), and the session ends the same way with
  "Transport Fever 3 closed", without waiting out those limits, so the
  player can start the game again at once. The heartbeat limits remain for
  games the launcher did not start and for a hook whose game still runs
  but hangs.

### The hook's session

`tpf3mp_bridge::Session` is the hook's whole side of the link, run on the
game thread. The game-specific part of the hook implements the `Game` trait
and calls the session from its detours:

- **Startup.** `Session::attach(DEFAULT_LINK, build, patience)`, then
  `wait_for_begin()`. The gate's first answer is `StepGate::Load`.
- **Loading.** Whenever `before_step` or `poll_step` answers
  `StepGate::Load(load)`, replace the world: with the save `load.file`, or
  without one with the world every player starts from. Then call
  `loaded(load.next_step)`. Call `heartbeat()` while loading.
- **Before each simulation step.**
  - `before_step(&mut game)` blocks until the room releases the step,
    calling `Game::apply` for each event on the way. A pause can hold it
    there for as long as the pause lasts.
  - A game whose simulation shares a thread with its rendering, which must
    never block, calls `poll_step` instead. It returns `Wait` until the
    step is released, and the detour skips the step for that frame.
- **After each step.** `after_step(&mut game)` reports it, and at
  checkpoint steps sends `Game::lanes()`.
- **Saving.** At a save event the session calls `Game::save(file)`, then
  `Game::lanes()`, and reports both. The save must hold everything needed
  to continue from that point, because it is what other players load. The
  agent cuts it into its chunk store and deletes the file.
- **When the player acts.** Capture the action before the game applies it
  locally and call `command(payload)`. For a build the payload is the
  action the Lua mod handed over as a table, converted by
  `tpf3mp_proto::lua` and encoded with `Action::to_payload` ("The action
  schema" in [BUILDING.md](BUILDING.md)). The action happens only when the
  room's event comes back through `Game::apply`, on every replica alike.
- **Notices.** `Game::notice` receives speed changes, refusals,
  divergences and the end of the session, for the game's UI.

The session gives up (`AgentGone`) only when the agent's heartbeat stands
still for its patience, never merely because a step is withheld.

#### Following the launcher from room to room

A game started from the launcher outlives the room it was started in: the
player may leave the room and create or join another while the game runs
on. Until the room's game begins (`Begin`), the session follows:

- `End` before `Begin` means the room session is over, not the game: the
  session stops reading that link and does not check that agent's
  heartbeat any more. A link whose `session` changes before `Begin` means
  the same, whether or not the `End` was read first.
- It then opens the link again by name, at most every 100 ms, until it
  finds a new `session`. There it exchanges hellos as `attach` does: its
  own `Hello` first (then a `Log` line saying it followed), and the new
  agent's `Hello` must be the first thing it reads; anything else is
  refused. Then it waits for that room's `Begin`. The agent needs nothing
  new for this: each room session is a fresh `Bridge`, which expects the
  hook's hello first, exactly once.
- Until the next room is created the game runs on its own, as before any
  room. `try_begin`, which the game's step polls, waits for as long as that
  takes; the blocking `wait_for_begin` gives up after the session's
  patience (`AgentGone`).

Everything else still fails closed. A second `Hello` on the same link
generation is refused, and once a game has begun a new generation is
never followed: every read then checks the link's `session`, and a change
is `SessionError::LinkReset`, on which the hook holds the world. The
bridge's messages did not change for this, so `BRIDGE_VERSION` stays.
(`session::tests` in `tpf3mp-bridge` play these through over a real link.)

`tpf3mp_testkit::fake_hook` implements `Game` for the toy game and runs it
through `Session`: the exact code the real hook will run, over the real
link. The `games_behind_the_bridge_and_gate_agree` scenario runs three of
them in one room end to end; others have a player join a running game,
rebase a replica that drifted, and hand a world on across a server restart.
`tpf3mp-fakegame` does the same as a separate process, for trying the stack
by hand (see [DEVELOPMENT.md](DEVELOPMENT.md)). On release day, what remains for TPF3 is:

- the build profile with its signatures;
- the detours that call the session;
- `Game` for the real world: applying an event means executing the player
  command it carries, the lanes are digests of the game state, and saving
  and loading use the game's own save format;
- checking that a save is complete and loads on every platform
  (DAY_ONE.md);
- checking a received save's script data before the game loads it
  (`tpf3mp_agent::save_check`, DAY_ONE.md section 7);
- on the server, a `Ruleset` that validates TPF3's command format and
  applies the canonical economy, with `save` and `restore` so its rooms'
  logs compact (`crates/tpf3mp-server/src/ruleset.rs`). It is added to
  the server's `RulesMenu` next to `native`, which stays offered.

### The main menu's Multiplayer window

The room's lobby is in the game (D17, as amended on 2026-09-30): the
Multiplayer entry on the game's main menu (docs/LOBBY.md) opens a page
that connects, creates or joins a room, shows its players and their ready
marks, chats, gets ready and, for the owner, starts the room's game. It
drives the launcher that started the game, which still holds the
connection (D11): the window is only another front end of the launcher's
[`Action`s](../crates/tpf3mp-agent/src/launcher/api.rs).

- **One link for the game's life.** The launcher opens the game's link
  when it starts, not when a room begins, so the game can start before a
  room is chosen. While no room session holds the link, the launcher
  serves it itself (`launcher::lobby::IdleLink`): it answers the hook's
  hello, sends the lobby whenever it changes and carries out the window's
  actions. A room session's bridge takes the link over already greeted
  (`Bridge::greeted`), passes the lobby both ways (`BridgeOptions::lobby`)
  and gives the link back when it ends (`Bridge::into_link`). Before it
  starts a game or begins a room, the launcher opens the link anew unless
  a game is on it: the one it started, while it runs, or any whose hook
  attached and whose process is still there, such as a game it took over
  (`launcher::renew_unused_link`; a process the system cannot be asked
  about counts as there). A game that closed never read what was sent
  last (the room's end, the lobby's updates); the next game's hook,
  finding that before the launcher's hello, would refuse the link and say
  the game has no link to the launcher. The link is renewed then, not when
  a game closes, because the launcher does not always see that: a game it
  took over may only have fallen silent. Such a game counts as closed once
  its hook fell silent and its process is gone; until then, and while a
  room's session still lets go of a game that closed in it, the launcher
  starts no other game.
- **Reading the link at the menu.** At the main menu no step of the game
  runs, so nothing else reads the link. The window asks the hook for the
  lobby a few times a second; each request exchanges it through the step
  driver (`StepDriver::lobby`): the window's actions go out as
  `ToAgent::Lobby`, and `Session::poll_lobby` reads what came in, keeping
  the lobby, and stops at the first message the game must see at its gate
  (the room's `Begin`), which `try_begin` takes when a world steps. In the
  room's game the gate reads the link and keeps the lobby it finds
  (`Gated::Lobby`); the lobby never holds the world or reaches the
  in-game Multiplayer window's notices.
- **Fail closed.** A hook whose step gate did not install has no driver:
  the window says the game has no link to the launcher and takes no action.
  A menu the hook cannot reach (its targets missing) stays the game's own,
  and the launcher's window still does everything.
- **One room's game per game.** A room left before its game began ends
  nothing: the game follows the player into the next room. After a room's
  game has ended, the window still shows the lobby, but the next room's
  game needs the game started again from the launcher.

### The step gate in the game

`tpf3mp-hook` detours the game's `GameSim::Step` (TF3 build 40408:
`0x159390`, from the built-in profile) and hands every call to
`step::StepDriver` (`crates/tpf3mp-hook/src/step.rs`, `install.rs`).

What one call of `GameSim::Step` does decides the design. In TPF2 and TF3
alike it is one batch of the game's own pacing: the main thread
(`CGame::Step`/`CGame::Sync`) calls it on its own schedule, 5 times a second
at 1x, and it runs as many simulation updates as its one call of the speed
getter answers: none while paused (the game's own paused path), one at 1x,
four at 4x. TF3 also adds a pending count, fed by the debug command
`makeGamePerformSimulationStepsCmd` and capped at 64 a call (the global at
`0x403b8e0`). The renderer interpolates from each batch, so **every call
must run**: a call skipped looks like a batch that ran without the world
moving on, the render clock goes back, and TF3 fails
`data.emitCount >= .0f && data.emitCount < 1.0f` in
`UI::particle_manager_util::UpdateParticleSystemInstance` (a negative
particle time step) within seconds of starting a game. The hook therefore
runs the game's step exactly once per call and chooses the answer to its
call of the speed getter, as TPF2MP's speed hook
(`tpf2-multiplayer/native/src/speedhook.cpp`) patched TPF2's step:

- **Before the room begins a game**, and **after it ends**, the game's own
  speed.
- **In the room's game**, of the steps the room has released
  (`Session::poll_step`, never blocking the game's thread, then
  `Session::read_ahead` and `Session::batch`, which read on through the
  agent's one-step releases), at most 16 a call and never past a
  checkpoint step, whose lanes must be the world's right after it, an even
  share ("Even steps" below); the call then reports each (`after_step`).
  A room faster than the game's own pace catches up that way, up to 16x.
  When the room withholds the next step (paused, or a player behind), 0:
  the game's paused path, and the world stands still.
- **The room's world.** A `Load` without a file (the world every player
  starts from) takes the world the game has loaded. A `Load` with a save
  file, and a `Save` the room orders, go through the mod's GUI ("The
  room's world" below), every call answering 0 until they are done. Any
  error (the agent gone, a malformed message, a failed report, a world
  that did not load) **holds** the world for good: every call answers 0,
  rather than run apart from the room's (fail closed).
- **Only the step's own call is changed.** The profile's target
  `GameSim::Step/GetSpeed call` (`0x1593ee`) is the step's one call of
  `CGameTime::GetSpeed` (`0x2a95a0`); the hook redirects that call
  (`hookcore::detour::CallRedirect`, which checks the call targets the
  getter) to answer the chosen count. The getter's six other callers
  (`CGame::Sync`, the UI, the camera, the particles) read the game's own
  speed: telling them 1 when the speed row says otherwise also trips the
  particle assertion. The game's own speed and pause therefore change
  nothing in the room's game but the display, until the room follows them.
  Nothing may send the debug step command during a room: its pending count
  would add updates to a call.
- **The speed row follows the room.** The Lua replacement
  `gui/tpf3mp/speed_control.script.lua` highlights the accepted speed for guests from
  the hook's room status, including pause. Guests see disabled buttons
  with "Host controls speed" help; their keyboard speed shortcuts are
  disabled through both `game.tl`'s internal `game_react_globals` table
  and the public module that copies its functions. It copies feature
  flags instead of changing the game's table, preserving mission/mod
  restrictions. Becoming host or leaving the room restores the stock
  recipe and shortcuts, including the host's keyboard hints. Updating the display
  never sends `makeGameSetSpeedCmd`: feeding the room's speed back into
  the game's local value would be mistaken for a player's request.
- **The host's speed row asks the room.** The getter's detour only reads: the
  game's own speed, the speed row's value, every time the game asks
  (`CGame::Sync` and the game UI ask every frame, paused or not). When it
  changes in the room's game, the hook sends `ToAgent::Speed` and the agent
  asks the room (`Request::SetSpeed`): the owner's choice sets the room's
  speed for everyone, and anyone else's is refused, shown as a notice. The
  value found on entering the room's game is not sent, so joining never
  resets a room's speed. What the room says back (its speed, a refusal, the
  end) goes to the hook's log.

Measured in TF3 build 40408 on release day (the rig with one player,
`app.startGame()` from the console, the game's speed set with
`makeGameSetSpeedCmd` as the speed row does, `GameTime.updateCount` read
before and after): 4.97 updates a second at 1x, 20.34 at 4x, 10.02 at 2x,
none paused, 4.98 back at 1x, the room confirming each change within a
second, and no assertion in several minutes of play.

The hook installs the detours from its bootstrap thread while the game
starts, before any world is loaded, so no thread is inside the step when
it is patched. It resolves the profile in the game's own mapped image
(not the file on disk), attaches the `Session` to the launcher's link, and
calls the game's step through the detour's trampoline with all four
register arguments passed through unchanged. A panic in the detour sets it
to hold. Tests: `step::tests` drive the driver against a scripted room;
`install::tests` detours a stand-in step in the test binary and checks it
runs exactly once per call with the count chosen and its arguments
untouched; `session::tests` read releases ahead over a real link;
`detour::x86_64::tests` redirect one call in hand-written code.

#### Even steps

The game calls its step on its own clock, about every 200 ms at every
speed, and the agent releases the room's steps on another, its playout's:
one every 200 ms at 1x (5 steps a second), every 50 ms at 4x. Running all
that was released at a call made the count alias against the call clock's
jitter of 10 to 15 ms. Measured on build 40408 with the step trace
(2026-10-07, one player on the deployed server): 0.4 % of the calls ran
another count than speed/100 at 1x, 15 % at 2x, 45 % at 4x (`4 3 5 4`,
`1 7`, `0 9`); in a two-player room 11 %, 19 % and 17 %. The renderer
interpolates from each call, so vehicles moved 75 % and then 125 % of a
normal call, or stood and jumped: they wobbled.

`crate::cadence` instead runs, of the steps released, the room's pace
(steps a second from the room's `Begin`, times its speed from
`ToHook::Speed`) times the game's call period (the mean of the latest 16
call gaps without their two shortest and two longest, so a save's freeze
does not move it and uneven calls average to what they are), with these
rules in order:

- **an event, save, load or end waits** behind the released steps
  (`Session::read_ahead` reads every release up to the first such message
  and never past it, `Session::barrier_waiting`): all of them, so the
  message is read on the next call as before;
- **the player's own action is on its way** through the room (handed over
  in the last 3 s and not yet ordered back): all of them, so it comes back
  as soon as before (another player's action may show a call, about
  200 ms, later);
- **far behind** (more than 1.5 s of the room's pace, at least 16 steps:
  a reconnect, a load; below the server's pacing window, so a game that
  falls behind catches up before it holds the room back): all of them, at
  most 16 a call, as before;
- **the room is paused**, or **nothing new was released for 400 ms** (the
  room waits for a member): all of them, so every game stops at the
  frontier;
- otherwise **the nominal count**, with one released step kept in
  reserve: a call that finds fewer rebuilds the reserve with one update
  less (at 1x one call of 0), a backlog of more than 3 is repaid by a
  quarter a call, at most half the nominal count more, and near a
  checkpoint the steps to it are spread evenly over the calls they take
  (`4 3 3`, not `4 4 2`), over fewer calls while a backlog waits (`5 5`),
  so rooms with checkpoints only a few steps apart keep their pace.

A rate of steps per call within 0.15 of a whole number is taken as that
number; a rate between (7 steps a second against 5 calls) carries its
fraction to an even mix. Which steps run, in which order and with which
actions between them, and every checkpoint's cut, are as before: only the
split of released steps into calls changes. Released steps wait at most
about the band (1 to 3 steps: 200 to 600 ms of simulation at 1x, 50 to
150 ms at 4x) before they run. `TPF3MP_HOOK_EVEN_STEPS=0` runs every
released step a call may, as before (hook.log says which at install). The
step trace notes each choice (`avail`, `cap`, `nom`, `pick`), and
`perf-step:` counts the room calls by rule (`even steps`).

#### The batch interval

Even steps leave unevenness the count cannot remove: a checkpoint every 50
steps cuts a call at 4x (12.5 calls), an action needs a call without
updates, a late turn or a save leaves a backlog. TPF2MP never had it: its
count per call was constant and it paced by the engine's batch interval
(`tpf2-multiplayer/native/src/speedhook.cpp`; its first version varied the
count instead, and vehicles juddered).

TF3 build 40408 does what TPF2 does, every field 0x78 higher (verified in
its disassembly, 2026-10-08). `CGame::Step` (0x11f3b0, main thread, every
frame), `m_data = [CGame+0x1f0]`:

```
totalTime (i64, m_data+0x1a0) += dt
while totalTime >= lastSyncTime (i64, +0x1a8) + guiFrameTime (i32 µs, +0x218):
    next = lastSyncTime + guiFrameTime
    if !CGame::Sync(cgame, fn): break        ; the call at 0x11f406 -> 0x11f650
    lastSyncTime = next
totalTime = min(totalTime, lastSyncTime + guiFrameTime - 1)
alpha = (totalTime - lastSyncTime) / guiFrameTime   -> CGame+0x1f8
```

`CGame::RunGameSimLoop` (0x11e210) runs `GameSim::Step` on
`gameSims[simIdx]` (`m_data+0x88`, `+0x98`) with a fixed 200 ms time step
and waits; a successful Sync publishes that finished state, flips `simIdx`,
hands the other `GameSim` the next batch and sets `guiFrameTime` from
`CalcGuiFrameTime` (0x11d240, 200-400 ms). The renderer interpolates by
`alpha` across the batch Sync exposed. So right after a true Sync the
batch on screen until the next one is known and finished.

`crate::interval` redirects that call (profile targets `CGame::Sync` and
`CGame::Step/Sync call`, both optional) and, after a true Sync, writes
`guiFrameTime = shown × T × trim`: `T` the room's time per step, `shown` the
batch's updates up to half again the nominal count (a catch-up above that
shows as faster motion rather than slowing it); a running room's batch
without updates is shown 50 ms (actions, a reserve being rebuilt: vehicles
stand briefly), or the nominal period when nothing was released at all
(the room waits for a member or a late turn), and a paused room keeps the
game's own interval. A batch of 3 at 4x is shown over 150 ms, one of 5
over 250 ms: vehicles move at the room's speed whatever the count. With it
on, even steps keep a fixed nominal count (the pace over the game's 200 ms)
and do not repay with extra updates; `trim` (0.8..1.25, down to 0.5 —
twice the room's pace — when the backlog is more than twice the band plus
a call behind, as after a load; at most 0.05 a batch) does: after a steady batch it moves against a backlog outside even
steps' 1..3-step band; after a drain, a catch-up or a reserve being
rebuilt (whose backlog says nothing about the pace) it eases back to 1.
Writing anywhere but right after a true Sync moved TPF2's render clock back
(a ShipFoamRenderer assertion); there Step's own clamp keeps `alpha` in
[0, 1) for any interval in bounds (20 ms..2 s).

The step detour publishes each batch, keyed by the `GameSim` it stepped
(`this`), with the updates it really ran (the per-update detour counts
them; the game adds its own pending debug steps to the answer, and such a
batch is not timed), into one of two slots; the wrapper reads the slot of
`gameSims[1 - simIdx]` after Sync flipped it. The next batch runs on the
other `GameSim`, so the slot read is not being rewritten, and the main
thread never waits for the simulation thread (a log line that meets the
lock held is dropped). The wrapper is `extern "C-unwind"`: an exception
Sync throws reaches the game's own handlers as without it. A begin, every load and a hold start a
new epoch: nothing published before times anything after.

Fail closed: only on the profiles whose layout was verified (40408 and
40420: the same offsets), only
with both targets, even steps on and the per-update detour installed
(without it no batch's real count is known, and even steps measure
again); before writing, the wrapper checks
`m_data` (non-null, 8-aligned), `simIdx` (0 or 1), the value Sync wrote
(20 ms..2 s), `totalTime >= lastSyncTime`, that the batch's epoch is still
the current one (a hold, a load, an end or leaving the room start a new
one), and that the field is committed
writable memory (`VirtualQuery`, once per `m_data` and epoch). Anything else
turns the interval off for the process (logged); the step driver then sees
it off and even steps measure the call period again. Outside a running
room's game, paused, or for a batch it cannot match, nothing is written:
the game's own interval stands. `TPF3MP_HOOK_BATCH_INTERVAL=0` turns it off.
`perf-step:` counts intervals written, Syncs skipped and Syncs that
returned false; with the step trace on, each write logs `interval: n=..
us=.. trim=.. backlog=..`.

### The Lua side

The mod runs in two kinds of Lua state: the game's GUI state, started by a
game bar plugin on the first frame of a game
(`mod/tpf3mp_1/content/gui/tpf3mp/`), and the engine states the game runs
game scripts in (`mod/tpf3mp_1/content/tpf3mp_sim/`, "Actions in the
game" below). The hook and the mod meet through one global table,
`tpf3mp_native`, which the hook gives every Lua state that calls `print`:
it detours Lua's `luaB_print` (profile target `luaB_print`) and, after the
game's print, adds the table to that state's globals, once, set raw past
any metatable a strict state gives them. The mod prints before it looks
for the table (`bridge.find`). Its contract is in
`mod/tpf3mp_1/content/scripts/tpf3mp/bridge.lua`; the hook's half is
`crates/tpf3mp-hook/src/lua.rs`:

- `tpf3mp_native.version`: 15. The mod refuses any other.
- `tpf3mp_native.note(key[, value])`: a short string one of the game's Lua
  states notes for the others (at most 16 keys, 512 bytes each; "" forgets
  it); with only a key, what was noted, or nil. The GUI runs in more than
  one Lua state, and a game script's GUI half reads there what the
  construction menu knows (the stop tool's stop, "The build tools").
- `tpf3mp_native.command(action, password)`: an action table, in the game's units.
  The hook reads it into a `tpf3mp_proto::lua::LuaValue`, within
  `MAX_DEPTH` and `MAX_NODES` (a function, userdata or a table as a key is
  refused), converts it with `action_from_lua` and queues
  `Action::to_payload`; the step gate hands it to `Session::command`, in
  the room's game only. It returns `true` and a ticket, or `false` and why.
  `false` or an error means refused, and the mod does not apply the action
  locally either: every game applies it when the room orders it. The
  ticket comes back in `results()`. `password`, optional, is a company's
  password, 1 to 64 bytes of text: only joining or locking a company takes
  one, scoped to that company (`tpf3mp_proto::Secret`); it goes to the room
  with the action and is never logged, and no refusal quotes it (version
  12).
- `tpf3mp_native.take()`: marks a simulation update begun, so its
  checkpoint can be read after the batch's last update. Runtime update
  batches return `nil` actions; ordered actions use the event path below.
- `tpf3mp_native.takeReplay(token)`: once, in the engine's `handleEvent`,
  the actions ordered for the next room step, as `action_to_lua` tables;
  second, each sender (64 hex digits); third, each seal, `{ scope =,
  tag = }` or `false` (version 13). A stale or duplicate token returns
  `nil`. Lists keep their array order; `apply.lua` fills every list passed
  to the game afresh in order (`seq`), because the engine's Lua copies
  can otherwise expose a list in hash order.
- `tpf3mp_native.replayed(token, ok, why)`: completes after every action's
  report and `state:set`; missing reports or a failed script hold the world.
- `tpf3mp_native.log(line)`: a line for `hook.log`, marked `mod:`.
- `tpf3mp_native.poll()`: in the GUI, every frame: what the hook asks of
  it, once, `{ replay = token }`, `{ save = name }` or `{ load = name }`,
  or `nil` ("Actions in the game" and "The room's world" below).
- `tpf3mp_native.saved(name, ok, why)`: the GUI's answer to a save.
- `tpf3mp_native.world()`: a world's GUI started. Before the room begins
  a game, the step gate's next call tells the agent the latest such world
  (`lua::take_world_up`, `ToAgent::WorldUp`), which marks the player ready.
- `tpf3mp_native.room()`: whether the room's game runs (the step gate's
  phase, held included), for the guard ("The player's commands" below).
- `tpf3mp_native.checkpoint()`: in a game script's update, whether it is
  the last of a batch that ends at a checkpoint step ("The world's lanes"
  below).
- `tpf3mp_native.lanes(t)`: the lanes read there, a table from lane
  numbers to strings. Returns `true`, or `false` and why.
- `tpf3mp_native.hash(s)`: the lanes' text hash of `s`, exactly what the
  mod's Lua `hashStr` returns (`crate::lanehash`), without a Lua loop over
  every byte ("What the lanes cost" below); `nil` without a string.
- `tpf3mp_native.network()`: in a game script's `postUpdate` at a
  checkpoint, the network lane read by the hook ("The network lane read
  natively" below): `nil` when `TPF3MP_HOOK_NATIVE_LANES` leaves it off;
  compared, `{ mode = "compare", count, deferred, rows, junctions = {
  heads, preferences, lights, tails } }`; on, `{ mode = "on", count,
  deferred, junctions = { lights } }`, the read kept for `networkSummary`.
  `deferred` lists the junctions' nodes left to the mod. `why` stands for
  all but the mode when the edges did not read, `junctionsWhy` for the
  junctions. Outside a checkpoint's last update, or off the game's step's
  own thread, it reads nothing and says so.
  Optional: the mod reads its own without it.
- `tpf3mp_native.constructions()`: in a game script's `postUpdate`, the
  constructions lane read by the hook: `nil` when off, else `{ mode, text =
  "count:hash", ms }` (compared, with its `rows`), or `{ mode, why }`.
- `tpf3mp_native.networkSummary(preferences, lights, deferred)`: right
  after `network()`, the network lane's text (`count:hash`) from what it
  read, the junctions' names given as `{ [value] = name }` and `{ [type] =
  name }` (`junctions.names`), and the rows the mod made for the junctions
  `network()` left to it (`deferred`, its nodes; `junctions.rowsOf`); `nil`
  and why when it cannot be made.
- `tpf3mp_native.clicks()`: the player's builds queued in the room's game
  so far, or `nil` where the hook cannot take them to the room ("The build
  tools" below).
- `tpf3mp_native.built(n)`: in the GUI: the build the module editor
  queued at click `n`, read by the hook, as game scripts see a proposal,
  once, or a terrain tool's stroke as `{ terrain = grid }`; `nil` and why
  when it did not read (a terrain tool's reason starts `terrain tool: `);
  `nil` when click `n` was neither's ("The module editor" and
  "Terraforming" below). An optional function: the bridge's version stays
  9, and a mod or hook without it keeps both refused.
- `tpf3mp_native.replaying(on)`: the game script begins or ends applying
  the room's actions, whose builds the hook lets through.
- `tpf3mp_native.terrain(t)`: in the game script's `postUpdate`, while the
  room's actions run: arms the next build it sends with the terraform grid
  `t` (`{ x0 =, y0 =, width =, height =, cells = { ... } }`), which the
  hook fills in at that build's apply; `true`, or `nil` and why.
  `terrain()` disarms and answers whether a build was filled (`nil` when
  none was armed). Optional, as `built`: without it no terraform applies
  ("Terraforming" below).
- `tpf3mp_native.applied(index, ok, entity, why)`: in the game script's
  `postUpdate`, after the batch's action `index` (from 1): whether it went,
  the entity it made, if any, and why not.
- `tpf3mp_native.status()`: in the GUI: the room for the Multiplayer
  window, `{ room =, speed =, diverged =, players = { { name =,
  connected =, owner =, me = } } }`, or nil before the room's game. The
  hook keeps what the room tells it (`Room`, `Speed`, `Diverged`), and
  forgets the divergence when a world loads (bridge version 9).
- `tpf3mp_native.chat()`: in the GUI: what the room's members said since
  the last call, `{ { from =, text =, old = } }`, oldest first, 64 lines
  at most. A world's GUI starts with none of the chat so far, so after
  `world()` the next call first gives the last 50 lines taken before
  again, with `old` set.
- `tpf3mp_native.say(text)`: in the GUI: says `text` (280 bytes at most,
  trimmed) to the room for the player, `true` or `false` and why; the step
  driver sends it (`Session::chat`) in the room's game only.
- `tpf3mp_native.results()`: in the GUI: what became of the player's own
  actions since the last call, `{ { ticket =, ok =, entity =, why = } }`,
  oldest first. The step driver knows the player's own actions when the
  room orders them back: the room's event names the player (`Begin`'s
  `player`) and the client sequence number, which `Session::command`
  returned when the driver handed the action over, and which it keeps with
  the action's ticket. An action the room refuses (`Notice::Refused`), or
  one handed over when no room's game runs, answers `ok = false` with why.
- `tpf3mp_native.dump()` and `tpf3mp_native.dumped(lane, entry)`: in a
  game script's `postUpdate` at a checkpoint: the lanes the hook wants
  written to its log entry by entry, and each entry ("Lane dumps" below).
  Optional, as `built`.
- `tpf3mp_native.edgewatch()` and `tpf3mp_native.edgewatched(entity,
  text)`: in a game script's `update`, the entities the edge watch reads in
  this update, or `nil`; in its `postUpdate`, what it read of each ("The
  edge watch" below). Optional, as `built`.
- `tpf3mp_native.mods(list)`: the mods to load a save with, given the
  save's (names, one a line): that list, then those left out and those
  added, the same way, from the room's `Begin` (`tpf3mp_bridge::mods::plan`);
  `nil` without the room's lists. `mods()` alone: `true` when the room gave
  them. Said in the hook's log ("the room's world loads with N mods: ...").
  Optional, as `built` ([MODS.md](MODS.md)).
- `tpf3mp_native.personal()`: this player's personal mods, names one a
  line, or `nil`: the guards tell a personal mod's commands by it
  ("The player's commands" below). Optional.
- `tpf3mp_native.preview(action)` and `tpf3mp_native.previews()`: in the
  GUI, what the player's build tool shows, for the other members, and what
  theirs show ("Build previews" below). Optional.

The table's functions run on whichever thread runs their state (the GUI's
the main thread, the game scripts' a pool of simulation threads) and share
nothing but the hook's queues. They reach Lua through Lua 5.2's C API as
the build profile names it: 18 functions besides `luaB_print`, found from
`luaB_print`'s own calls and their places in lapi.c, which the linker laid
out alphabetically (TPF2's were too; several prologues match TPF2's byte
for byte). They are only called, never detoured. Everything that crosses
into Lua is `C-unwind`: a Lua error, which TF3 raises as a C++ exception,
passes through as the game raised it.

Without the table, the mod logs "no hook in this game" and does nothing.
That is every game Steam started (D11).

### Actions in the game

A player's action happens in no game until the room orders it, and then in
every game between the same two simulation steps, including while paused:

1. The mod hands the action to `tpf3mp_native.command`. The step gate
   sends it to the room.
2. The room orders it as an event for step `s`. After step `s - 1`,
   `StepDriver` gives the ordered actions to `lua::request_replay` and
   holds updates. This applies to running rooms too, so every replica uses
   the same engine phase regardless of when the resume arrives.
3. The GUI polls a `replay` token (Lua contract version 13) and sends only
   that token in the existing `tpf3mp/command` scripting event (a string
   token, rather than an action table, so older saves already subscribe).
   `guard.wakeReplay` can bypass the GUI guard for this wake only; it cannot send an action. The
   native command loop runs outside `GameSim::Step`'s update loop.
4. In the engine's `handleEvent`, `takeReplay(token)` takes the hook's
   actions, origins and seals exactly once. The script uses its existing
   `postUpdate` action processing through `api.cmd` and saves the registry,
   companies and progression state. No update, monthly charge, progression
   sample or checkpoint is requested by the wake. The ordered step scopes
   the RNG seed for this processing and nested script events.
5. Each action is reported with `applied`, including normal refusals. Only
   after `state:set` does `replayed(token, ok, why)` complete the replay.
   The driver then allows later actions, a save, a load or step `s`. A
   missing report, failed wake, script exception or 30-second timeout holds
   the world (fail closed). A hold or closed world invalidates delayed
   wakes; duplicates and stale tokens take nothing.

At that boundary, the mod emits compact records for company changes, line
assignments, vehicle purchases and construction bulldozes. `company-event`
shows the sender's company before and after the operation and the local
game's selected company (`local_before`/`local_after`); it never logs the
sender key, company name, password or seal. Each record's `action_index` is
its 1-based position in that replay batch, not a room simulation step.
`assign-vehicle` is one line per
vehicle and carries its canonical ID, this game's native entity, the line's
canonical/native IDs, ordered model resource names, carrier folder, line mode
set, owner and that vehicle's applied/refused result. `buy-vehicle` carries
only the consist's ordered model resource names and the new native and
canonical vehicle IDs when the registry can name it. `bulldoze-construction`
records the requested construction reference, the already-resolved native
entity and actual origin, and its owner; `action-event` records the action's
applied/refused result. Native entity IDs are local to a game; compare the
canonical IDs and resource names across replicas. Optional diagnostic reads
say `none` when a component or value is absent and `unavailable` when a read
throws or cannot be made. They are protected reads at the action boundary,
not per-tick scans, and never decide whether the action applies.

The world still runs the game's paused path while waiting: neither room
steps nor game time advance, and the paused-tick fix keeps `tickCount`
unchanged. Construction costs are charged normally. Existing update and
checkpoint processing remains in `update`/`postUpdate`. The engine event
path requires two-game acceptance on the supported game build; stand-in
tests alone do not establish that native callbacks are synchronous there.

Measured on build 40408:

- A mod's game script (`*.gs.lua`, discovered by the game) has its
  `update` called once per simulation update: `GameTime.updateCount` goes
  up by one from call to call, and `dt` is 0.2. Its script file defines
  `data()` returning the functions, as the GUI's do.
- The game runs game scripts on a pool of Lua states ("Sim Pool" threads),
  so a script's locals are kept per state, not per game.
- The game's own scripts decide in `update` and change the world in
  `postUpdate`, which the game calls with what `update` returned, and not
  when that is nil: the company script reads its argument unchecked, and
  a lane read from a `postUpdate` after an update that returned nothing
  never reached the hook. The mod's game script does the same, so the
  world changes only in `postUpdate`, not while other scripts' updates may
  run beside it.
- In an engine state a command runs at once (the game's
  `api/tealdef/api/cmd.d.tl`). The game takes no callback in `update`
  ("Callbacks are currently disallowed"), but in `postUpdate` it calls one
  at once, with the command's result: the game's mission scripts read the
  line they made from it right after `sendCommand`
  (`mission_vehicle_util.tl`). `apply.lua` sends every command with one
  there: a command the game answers as failed fails the action in every
  game, and the entity a command made (`resultEntity`,
  `resultVehicleEntity`, else the first of its result entities) is what
  the action made. A state where the game refuses the callback sends
  without one, logs so once, and leaves finding what an action made to
  the registry. A refused command raises. In the GUI and console states
  commands run "in the next simulation step", which is a different update
  on each game: the reason the game script applies them, not the GUI.
- The console has a Lua state of its own. For tests, its
  `api.cmd.makeScriptingSendEventCmd("", "tpf3mp", "command", action)`
  reaches the game script's `handleEvent` in that game only, which hands
  the action to the room.

Seen in two hooked games in one room (the rig, the fixture save): a road
depot sent from the first game's console was handed to the room, and both
games applied it in the same update; the determinism probe's construction
lane changed between steps 500 and 600 in both, to the same value, and
matched at every sample after.

Seen on build 40408, two games through the deployed server: a loan
taken in the guest's finance window went to the room, and both games
booked it in the same update, the account and the loan script's list of
loans alike in both; the loan script's own callback, which books the
money, ran in the game script's `postUpdate`.

`apply.lua` applies, among others:

- `BuildConstruction`: a `SimpleProposal` with one `ConstructionEntity`
  (the file, the matrix from the transform, the parameters from their
  flattened paths, the name, the player) and, in its street proposal, the
  construction's connection, resolved as a road build's polyline, less
  the construction's own entrance (a new vertex at the end of a single
  link), sent with a `Context` naming the
  player, who pays, and gathering the town buildings and fields in its
  way, and `playerInitiated` true, as the player's own build. The game's
  verdict comes first (`makeProposalData`): a critical error refuses the
  build, with its reasons, in every game; warnings (town buildings to
  remove, reputation lost) are logged and built through, as the tool
  builds once the player clicks (`ignoreErrors` true: with it false the
  game dropped such a build without a word, seen on build 40408);
- `Loan`: for the room's first company, the loan script's own event,
  `makeScriptingSendEventCmd("", "Loan", "Obtain", { next, offer })` or
  `"Repay", { nil, loan }`, with the tables the finance window sends. For a
  founded company, Take must match that company's saved active offer slot;
  the room books it to that company and puts only its slot on cooldown;
  Repay names one of its saved loans;
- `Prospect`: the company script's own event,
  `makeScriptingSendEventCmd("", "Companies", "spawnIndustry", {
  companyEntity, townEntity, types, permitKey, cargoType })`, with the
  player's company, the town the registry names, and the industry types in
  the order the action carries them ("Prospecting" below);
- `Perk`: the company script's own event, `MakeGreen` or
  `startMarketingCampaign`, for the acting company, and for a campaign the
  price booked to it after ("Company perks" below);
- `Subsidy`: the subsidy script's own event, `makeScriptingSendEventCmd("",
  "Subvention", "onAccept" | "onDecline", { uid })`, the accept with the
  taking company's player entity (`tpf3mpCompany`), once every game has
  checked the offer against its own script's state ("Subsidies" below).

Every other action is refused with a line in `hook.log`, the same on every
game, so the worlds stay alike. The native build tools come next.

#### What applying an action costs

Applying another player's build runs in this game's step, so a slow one
stutters the game of everyone who watches it. The game script logs, from
Lua's `os.clock` (nothing of it reaches the world; it is the elapsed time
on Windows, but the process's CPU time on Linux and macOS, where the game's
other threads count too):

```
mod: applying BuildRoad took 680.0 ms: nodes Street 1x 14.0 ms (4964), verdict 1x 15.0 ms, commands 1x 28.0 ms, junctions 1x 590.0 ms
mod: actions since the last checkpoint: 2 applied in 160.0 ms (kind n/total/longest ms: BuildConstruction 2/160.0/88.0), registry.sync 0.0 ms over 4 calls
```

The first for each action that took at least 20 ms (`apply.SLOW_MS`), by
part (`tpf3mp/apply.lua`, `apply.costText`): `nodes <network>` a read of
a whole network (its nodes), `near <network>` a preview's octree lookup of
nodes (the edges found; docs/BUILDING.md), `constructionAt` the search of every
construction (how many), `junctions` the junction settings it keeps,
`verdict` the game's `makeProposalData`, `refresh` and `regenerate` the
game's own proposals for a construction, and `commands` every command it
sent. The second at each checkpoint, by kind of action. A preview of
another member's build is made the same way in the GUI (`tpf3mp/previews.lua`),
and every 10 s while any arrive:

```
mod: build previews in the last 10.0 s: made 25 in 1396.0 ms (longest 158.0) [BuildRoad: nodes Street 1x 133.0 ms (4945)], drawn 25 in 405.0 ms (longest 29.0), retried 0 in 0.0 ms (longest 0.0)
```

### The room's world

A room plays its owner's world. On a server that keeps worlds, the owner
names a save before the room starts (the launcher's `--start-save`), which
the owner's agent reads from the game's save folder and hands to the room
in its lobby: no game is involved, every game waits at its main menu, and
all of them, the owner's too, load that save from there when the room
starts. Without one, the room has the owner's game, with its world up,
save it before step 1, and every player's game loads that save. A player
who joins later loads the room's latest one ("The first world" in
[PROTOCOL.md](PROTOCOL.md)). Transport Fever 3 saves and loads only through its GUI's
script API (`app.saveGame`, `app.loadGame`) and only in its own save
folder, so the hook asks the mod's GUI for both
(`crates/tpf3mp-hook/src/worlds.rs`,
`mod/tpf3mp_1/content/gui/tpf3mp/tpf3mp.script.lua`), and the world
stands still meanwhile:

- **Saving.** The session answers `StepGate::Save(order)` until the save
  is reported (`Session::saved`). The driver asks the GUI to save under a
  name of this game's own, `tpf3mp_<pid>_<event>`, as two games on one PC
  share the folder. The GUI calls `app.saveGame(name, callback, false,
  true)` (the last argument leaves the player's own save name alone) and
  answers through `tpf3mp_native.saved` from the callback, once the file
  is written. The hook finds `<name>.sav`, removes the picture the game
  writes beside it, and moves the save to `order.file`, which the agent
  cuts into its store. A save the game refuses, or does not make within
  `SAVE_PATIENCE` (120 s), is reported failed, and the game goes on.
- **Loading.** A `Load` with a file (the room's save, fetched by the
  agent) is copied into the folder as `tpf3mp_room_<pid>.sav`, and the GUI
  asked to load it (`app.loadGame`, with a `SavegameId` in the game's save
  namespace). With the room's mod lists (`Begin`), the GUI first reads the
  save's details (`app.getSavegameInfo`, a few frames) and loads it with
  their `mods` replaced: the save's shared mods, TPF3-MP, and this
  player's personal mods, leaving out another player's
  (`mod/tpf3mp_1/content/scripts/tpf3mp/worldload.lua`; [MODS.md](MODS.md)).
  A shared mod not installed here fails the load, and says which. The GUI tells the hook each time a world's GUI starts
  (`tpf3mp_native.world`); the room's world is the first to start after
  the GUI took the request, never the one it was asked in, whose GUI may
  well report itself in between. Then `Session::loaded(next_step)`, and
  the room's steps run on. A world not up within `LOAD_PATIENCE` (600 s)
  is held.
- **Cleaning up.** Each game's copies carry its process id, so every game
  played would leave a whole world behind (`tpf3mp_room_<pid>.sav`, and a
  `tpf3mp_<pid>_<event>.sav` whose save failed or was never moved). The
  hook removes those of games no longer running
  (`crate::worlds::sweep`): once its save folder is resolved, after it copies a room's
  world in, and after a save it moved out. It fails closed:
  - only files named exactly `tpf3mp_room_<pid>.sav`,
    `tpf3mp_<pid>_<event>.sav` or the `.jpg` beside either (decimal
    numbers as the hook writes them, no leading zero, same case), never
    a folder or a link: every other save, the player's, stays;
  - never this game's own, `<pid>` its own: its `tpf3mp_room_<pid>.sav`
    is the world it is loading or plays, replaced by the next load;
  - never another running game's, two games on one PC sharing the
    folder: a process the hook cannot ask about counts as running.

  Nothing needs an older copy: every load, a rejoin's, a resume's or a
  late join's, copies the world the agent fetched from the room in
  afresh. The Multiplayer window does not offer these copies as a world
  to start a room from.
- **The folder** is Steam's for the account playing,
  `<Steam>/userdata/<account>/3493540/local/save`, found when first
  needed and then kept. First as Steam's API in the game names it
  (`ISteamUser::GetUserDataFolder` through the game's `steam_api64.dll`),
  which works under Proton too, where the registry names Proton's
  stand-in for Steam, without accounts' folders. Else under Steam's
  folders: the registry's (`SteamPath`, `ActiveProcess\ActiveUser`),
  Program Files', and under Proton the Steam client's that Proton names
  (`STEAM_COMPAT_CLIENT_INSTALL_PATH`, through drive `Z:`), for the account
  playing or else the one account with a save folder for the game. The
  hook's log says which. Without one, a load holds the world and a save
  is reported failed, with why for both ways.

#### Loading from the main menu

The GUI runs only in a world, and at the game's main menu no world steps,
so neither the GUI nor the step's detour is there. A game at its menu
(`crates/tpf3mp-hook/src/menu.rs`; the static findings and the in-game
checks are in `investigation/TPF3_MENU_JOIN_2026-09-30.md`):

- **Gets a Lua state that can load.** The game gives a Lua state its `app`
  table in one function, `RegisterAppUsertypes` (profile target, build
  40408 `0xdc5fa0`), for the menu's states and the in-game GUI's. The
  hook detours it and, after the game's registration, runs a short chunk
  there (`lua_load`, `lua_pcallk`): it hands the hook a function that
  loads a save by name, kept in the state's registry (`luaL_ref`), and a
  sentinel whose `__gc` tells the hook when the state closes (Lua 5.2 runs
  every finalizer at `lua_close`), so the hook never calls into a closed
  state. hook.log: `menu: Lua state 0x... has app; the main menu can load
  the room's world from it`.
- **Knows whether a world is loaded.** `DoStep` runs at the menu and in
  a world alike, so the hook reads the engine's own answer:
  `CMenuUI::m_game`, the loaded world, which `CMenuUI::StartGame` asserts
  clear and sets and `CMenuUI::StopGame` clears. `DoStep` tests it before
  it hands its frame to the world's UI; that test is the optional profile
  target `UI::CMenuUI::DoStep/m_game test` (`0x6a01c0`, `cmp [rsi+0x6b0],
  r13`), and its displacement is the field's offset, which the menu's
  frame reads in the `CMenuUI` it is handed. A state given `app` while
  `m_game` is set, or while the existing menu reports a load in progress,
  is the world's GUI's: the menu never loads from it, and
  forgets it when the world closes. Loading is read from the verified
  `CMenuUI::m_loadGameResult` field (`0x6a0c84`, displacement `0x1bd0` on
  build 40408). The menu never queries the progress monitor: its lock
  can be held by the loader while it runs a nested menu frame.
- **Follows the room from the menu's frame.** `UI::CMenuUI::DoStep`
  (`0x6a0160`, the menu's per-frame update on the main thread) is
  detoured. After the game's own frame, the driver runs
  `StepDriver::on_menu` only while the game is at its main menu with no
  world, and a state of the menu's adopted on that thread is open
  (`crates/tpf3mp-hook/src/at_menu.rs`):
  - a game that has had no world up yet (never stepped, no world's GUI
    started, `m_game` never set) is at its menu, but menu work still waits
    until the native load field is known and clear;
  - a world loaded blocks, before its first step and while it stops
    stepping (saving the room's world, held for another player): an
    earlier rule of "no step for 2 s" took the room's session inside the
    owner's world and hung it, and the owner's menu frame once took it
    while saving the room's world before that world's first step (both
    measured 2026-09-30);
  - after a world, the menu is back once `m_game` is clear and the
    native load field stays clear for 2 s with no step between. A load
    blocks: the GUI's load stops the world first and loads after, and a
    moment with a load restarts the 2 s;
  - after a world, a game whose `m_game` the hook cannot read (the target
    missing) or whose menu cannot say whether it loads is never taken for
    the menu (fail closed): as before this rule, only a fresh game follows
    the room from its menu.

  hook.log says where the menu sees the game on each change:
  `menu: a world is loaded (CMenuUI::m_game set)`, `menu: the world closed
  (CMenuUI::m_game cleared); ...`, `menu: no world loaded, but the game is
  loading one`, `menu: back at the main menu after a world (no world loaded
  or loading for 2 s)`; and when the room begins there, `the room began at
  the main menu; the menu sees: <where>`. Before the room begins, `on_menu` reads `Begin` and tells
  the agent `MenuUp` once per arrival, which marks a guest ready (and the
  room's owner, once the room has the save the owner named for it to start
  from: PROTOCOL.md, "The first world") and keeps the hook's heartbeat
  going at the menu; in the room's game it answers a
  `Load` with a file by copying the save into the folder as above and
  loading it from the menu, as the menu's own Load Game page does:
  `api.type.SavegameId.new()` with the name and the `savegame` namespace
  (`app.SaveGameNamespace.getSavegame()`), then `app.loadGame(id, false,
  nil)`. A load the game is busy with already (its
  native load future is set) is tried again on the next frame. Any
  other failure holds the world. A hook-started load also blocks menu work
  until its world GUI arrives or it fails. Nested menu frames do no hook
  work; a missing load-field target refuses menu loading.
- **Starts the world without Start Game.** The menu's pages call
  `app.setWaitForStartReadyGame()` before they load, which is what makes
  the loading screen wait for the player's Start Game
  (`app.startReadyGame()`); the hook does not, and the game then starts
  the loaded world by itself. Confirmed in a two-game playtest on build
  40408: a guest at the main menu loaded the room's world by itself, with
  no Start Game click.
- **Loads it with TPF3-MP active.** Without the room's mod lists `info`
  is nil, so the save keeps its own mod list, and the room's save was
  written by a game whose GUI had TPF3-MP linked (only the mod saves for
  the room). With them (the room's `Begin`, [MODS.md](MODS.md)), the chunk,
  handed the hook's `mods` as a third function, first reads the save's
  details (`app.getSavegameInfo`, answered "busy" until the game has them)
  and passes `info` as the menu's own Load Game page does when a player
  changes a save's mods: `api.type.SaveGameDetails.new(data.info)` with
  `mods` set to the save's shared mods, TPF3-MP and this player's
  personal ones, each `api.type.ModId` checked with the user profile's
  `ModRep:exists` (a shared mod not installed here fails the load and
  holds the world). A world whose mod does not start never says it is up,
  and is held after `LOAD_PATIENCE`.

The loaded world's GUI says it started (`tpf3mp_native.world`), the step's
detour takes the world at its first call (`Session::loaded`), and the
room's steps run on, as for a load from the GUI.

What the menu leaves to a world up: a `Load` without a file (the owner's
world, or everyone's on a server that keeps no worlds), and a `Save` the
room orders. Without a save named to start from, the owner's game
therefore still needs its world up to start the room: the menu logs `at the main menu: the room plays the world this
game starts from ...` once, and takes nothing. Without the menu's five
function targets (all optional in the profile) or an adopted state,
nothing of this runs: hook.log says `the main menu cannot load the room's
world (fail closed): ...`, and a game needs a world up, any one, before it
can load the room's. Without the `m_game` test, the install line says so
and only a game that has had no world up follows the room from its menu.

Tried on build 40408 through the deployed server, with two games on one PC
(the rig, the fixture save): the owner's game saved its world for the room
in 215 ms and reported it within a second. The guest fetched the save,
its GUI took the load in the world it was in, and the room's world was up
4 s later and played from step 1. A road depot sent from the owner's
console was applied by both games, and the determinism probe's 15 samples
from step 500 to 1900 matched in every lane it reads, the construction and
money lanes changing alike after the build
(`investigation/dayone-2026-09-29/6-determinism.md`; the edge lane is
still unread there).

#### A world the room did not load

A room's world is replaced only by a load the room orders: its first
world (the start save, or the owner's world saved before step 1 and
loaded by every game, the owner's too: "The first world" in
[PROTOCOL.md](PROTOCOL.md)), a rebase after a divergence, a rejoin. Each
one is the same save, at the same room step, in every game. A new world
generated in the room's lobby (**New world: choose map and settings**, the
stock New Game page) takes that path too: the owner's game has it up when
the room starts, the room has it saved before step 1, and every game
loads that save.

A world the player starts on their own after the room's world closed
does not. On 2026-10-02 both games of a running room came to play a
freshly generated world started from the game's own menus: neither
launcher fetched a world from the room between the start save and the
divergence. Each game's step gate,
still following the room, ran the new world from whatever room step the
room had reached when it came up, with no load and no
`playing the room's world from its save, from step N` line. At room step
450 one game's world had run 58 updates and the other's 75. The same
`CompanyOp` applied at game time 9800 in one game and 13200 in the
other, and created different entities (47426 and 47334). The subsidy
offers parted first, and the room saw the split at step 2250. The owner's
game then saved its new world for the room as if it were the room's.

So the driver marks the world it takes for the room
(`crate::step::WorldMark`, taken when a load of the room's is done or the
owner's world is taken at a `Load` without a file):

- `closed`: the worlds that closed in this game (`CMenuUI::m_game`
  cleared, seen by the menu's frame), other than for a load the hook
  started (`lua::load_started`), which closes the world it was asked in;
- `started`: the worlds whose GUI started (`tpf3mp_native.world`), which
  decides only where the hook cannot see a close (no `m_game` test in the
  profile).

In the room's game, with no load of the room's under way, a world up
with another mark than the room's is none the room loaded. The step's
detour holds it for good before it runs a step or answers a `Save`
(`holding the world (fail closed): this game has a world up the room did
not load ...`). At the main menu after the room's world closed, the menu
says once that a world started now is held (`at the main menu: the
room's world closed in this game ...`). A load the room orders there (a
rejoin, a rebase) takes a new mark, and the room's steps run on. To play
the room's world again, the player leaves the room and joins it again,
which loads its latest save.

### The player's commands

In the room's game a player's command runs in every game at the same
update, or in none. Transport Fever 3's GUI sends most of what a player
does from its own Lua state, through `api.cmd.sendCommand` (build 40408's
scripts): buying, selling, replacing and assigning vehicles and making and
editing lines (`gui/line_vehicle_mgmt/`, `gui/entity_window/`), a
construction's parameters and bridges and tunnels
(`makeWorldBuildProposalCmd`, from `gui/construction/construction.tl` and
the entity windows), loans and other game mechanics as script events
(`makeScriptingSendEventCmd`), and the speed row (`makeGameSetSpeedCmd`).
The street and track tools, and placing a construction, are native
builders that reach the command queue (`CommandList::Add`) without Lua.

So the stock windows do send through `api.cmd.sendCommand`, as the mod's
replays do (the question in PLAN.md, Part 2). What tells them apart is the
Lua state, not a caller's address: the player's commands come from the
GUI's state, the room's replays from the game script's states, whose
`api.cmd` the mod leaves alone, but for one thing: what a player's
personal mods send there (below, and [MODS.md](MODS.md)).

A GUI mod only one player runs (a personal mod, [MODS.md](MODS.md)) sends
through the same guard as the player's clicks: carried or refused alike.
A refusal names the mod it came from, the nearest function on the stack
whose source is a mod's file (`<modId>::/...`, `guard.callers`), so
hook.log says `refused the player's makeTownCreateCmd in the room's game
(1 so far), from the mod gw_big_city_1`. A personal mod's event to its own
game script (`makeScriptingSendEventCmd` with an id that names the mod and
neither an id nor a name the game's scripts or TPF3-MP listen to,
`guard.ownEvent`) is sent here as it is: it reaches this game's game scripts
only, where the mod's own game script runs. Any other of its events is
carried or refused as the player's own would be.

A personal mod's game script runs in its player's game only, in the
simulation's states, where a command runs at once. There
`tpf3mp/modguard.lua`, installed by the mod's game script in each state it
links in, sits in front of `sendCommand` in the room's game: a command with
a personal mod anywhere on its stack is not run; a vehicle's manual
departure (`VehicleChange::ManualDeparture`), departure or stop, or a
line's rename or update, for the player's own company's, goes to the room
as an action, once per change within 5 s of game time; an event between
game scripts is dropped; anything else is refused, each once in hook.log
(`handed makeVehicleSetManualDepartureCmd from the personal mod
celmi_timetables to the room (1 so far)`). Commands from the game's own
scripts, TPF3-MP's and shared mods run as before. A state without
`debug.getinfo` cannot tell them apart: it notes `personal-mods-unguarded`,
after which the hook loads the room's worlds without the player's personal
mods (docs/MODS.md).

The mod guards the build tools in its game script, whose `guiHandleEvent`
runs in the GUI's state. The street, track, station and depot, stop and
bulldozer tools tell game scripts of every proposal they make
(`builder.proposalCreate`), and they honour an error a script returns, as
the game's company script refuses constructions without a permit
(`game_mechanics/company/company.script.tl`). In the room's game the mod's
script returns "Not in multiplayer yet: building with this tool" for every
proposal of a tool the room does not carry yet, and the tool shows it in
red and builds nothing (seen on build 40408, with the street tool). A tool
the room carries builds through it instead ("The build tools" below). The
script subscribes to those events by name, as a save may carry an older
version's subscriptions.

The mod guards the GUI's commands
(`mod/tpf3mp_1/content/scripts/tpf3mp/guard.lua`). `api.cmd` is a plain
table whose factories and `sendCommand` (a callable table) can be replaced,
as the console's state showed, and none of the game's 6,041 scripts keeps a
reference of its own to either. Once linked, the GUI wraps every
`make*Cmd` factory, to note which one made each command, and
`sendCommand`. While the room's game runs (`tpf3mp_native.room()`):

- a command of a kind in `guard.PASS` is sent, its arguments untouched. So
  far that is the speed row's `makeGameSetSpeedCmd`, which the step gate
  reads as the player's request to the room;
- a command `guard.CARRY` makes an action of goes to the room instead
  (`tpf3mp_native.command`), which orders it for every game, this one
  included ("Actions in the game"). Its callback hears what became of it
  once this game has applied it (`results()`, `guard.deliver`), as the
  game's own command would answer: `(data, ok, {{ entity, 0 }})`, with the
  entity the action made. The game's windows chain on that: the store
  puts the vehicle it bought on a line by the entity its callback hears
  (`resultVehicleEntity`), the line manager opens the new line
  (`resultEntities[1][1]`); told nothing, or told before its world had the
  line, the line manager made a second line at the next stop clicked (seen
  on build 40408). So an answer waits until the GUI's world has the entity
  it names (`api.engine.entityExists`; the game script made it in the
  simulation) and, for a bought vehicle or a new line, until the game
  script's state the GUI reads names it by its id (`guard.NAMED`): that
  state reaches the GUI after the entity, and the store's "buy and put on
  a line" sent its line assignment in between, which no game could name
  ("a vehicle the room cannot name", 2026-09-30). Each answer waits on its
  own entity for up to 20 seconds by the clock, counted from when it is the
  one waited on (`guard.HOLD_SECONDS`; a frame count ran out before the GUI
  named a burst of five vehicles, 2026-10-01), and answers keep their
  order; a
  command that should have made something and made nothing the game could
  name is answered as failed, which the windows handle. With both, a new
  line took its stops one by one as in single player (build 40408). So
  far:
  - loans, the finance window's `makeScriptingSendEventCmd("", "Loan",
    "Obtain" | "Repay", …)`, as a `Loan` action carrying the loans' terms.
    The first company uses the loan script's event; founded-company offers
    and loans are checked and booked by the room's companies module;
  - prospecting, the construction menu's `makeScriptingSendEventCmd("",
    "Companies", "spawnIndustry", …)`, as a `Prospect` action ("Prospecting"
    below);
  - taking a rank, the company window's `makeScriptingSendEventCmd("",
    "Companies", "applyLevel", { level })`, as an `ApplyRank` action
    ("Company ranks" below);
  - the construction menu's perk tools, `makeScriptingSendEventCmd("",
    "Companies", "MakeGreen" | "startMarketingCampaign", …)`, as a `Perk`
    action ("Company perks" below), refused in the sender and in every
    game's replay until `acceptance.lua`'s `perks` is turned on after a
    two-player game (COVERAGE.md);
  - a town building's Historic Preservation checkbox
    (`makeTownBuildingSetBlockedDevelopmentCmd`, the town building window's
    `HistoricBuildingCard`), as a `Preserve` action: the construction the
    building stands in, by its file and position, and the building's index
    in that construction's town buildings (`capture.townBuildingOf`: the
    construction the game names for it, else the one whose list has it;
    INFERRED that the window's entity is the listed one). Every game sets
    the building at that index through the same command. Refused in the
    sender and in every game's replay until `acceptance.lua`'s
    `preservation` is turned on after a two-player game (COVERAGE.md);
  - answering a subsidy offer, the subsidy window's
    `makeScriptingSendEventCmd("", "Subvention", "onAccept" | "onDecline",
    { uid })`, as a `Subsidy` action naming the offer by its number and
    its kind, read from the subsidy script's offers as this game has them
    (`capture.subsidy`); an offer this game no longer has is refused at
    the click ("a subsidy no longer offered"). A refusal by the room is
    told the player in the game bar;
  - vehicles: buying (`makeVehicleBuyCmd`: the depot by its construction's
    file and position and its index among that construction's depots, an
    airport's second hangar say. A construction's depots are its
    `depots`, then its subconstructions that are depots: an airfield's or
    airport's hangar is its hangar module's subconstruction, which the
    store buys at (build 40408). The construction is the one the street
    connector names for the depot, else for the depot as a subconstruction,
    else the one construction listing it; a depot two constructions list,
    or one its construction does not list, is refused at the click, never
    bought at the first depot. Every game refuses a purchase naming a depot
    the construction does not have, saying how many it has: an airfield or
    airport built without its hangar module has none, and a harbour never
    has one, ships being bought at a ship depot. With more than one company,
    every game's replay also requires that the depot's `PLAYER_OWNED` is the
    acting company; another company's depot and a depot with no readable owner
    are refused. With one company, the game's native purchase behavior stays.
    The consist part by part,
    as the store configured it), selling, putting on a line, and the vehicle window's stop, start,
    to the depot (kept: sell-on-arrival is refused because build 40408 crashes
    at arrival), reverse and depart; replacing
    (`makeVehicleReplaceCmd`, the vehicle window's "modify" and the store's
    "replace", `ReplaceVehicle`). The store sends one command per vehicle,
    a group's vehicles one by one, and none with a callback
    (`vehicle_react_util.tl`, `HandleVehicleChanges`, build 40408); a
    vehicle whose new consist is empty it sells instead. A part the player
    left in the consist is the vehicle's own, its purchase time kept; the
    store bought the rest (purchase time 0, set to the GUI's game time
    before it sends). So the capture marks a part kept when it is one of
    the vehicle's own of the same model and purchase time, each matched
    once, and every game gives a kept part the purchase time and wear it
    has there, a new one the game time of the update it applies in. The
    vehicle keeps its canonical id: the game script binds the id to the
    entity the vehicle is after the command (the command's result
    entities, its data's `vehicleEntity`, else the vehicle itself; TF3's
    API says the vehicle is replaced and its command data has no result
    field, so the same entity is INFERRED), before the registry's sync
    would retire it;
  - lines: creating, changing (the line whole, as the line manager built
    it: stops, terminals, loading rules, waypoints), deleting, renaming and
    recolouring. A waypoint on a street or track names its lane by the
    edge's ends, node 0 first, or by the construction whose network it is
    in, with the lane's index and the place along it; a game whose edge
    runs the other way refuses the line rather than guess. A ship's or
    aircraft's waypoint in the open goes by its position;
  - renaming a vehicle, a station, a town or another construction in its
    window's title or the line manager, and a vehicle's colour, as `Rename`
    (by canonical id, a construction by its file and place) and `VehicleOp`
    `Recolor`; every game checks the acting company may;
  - a construction's edit sent from its window
    (`makeWorldBuildProposalCmd` with the game's replacement proposal), as
    a `BuildConstruction` that replaces it ("The build tools" below).

  Vehicles, lines, station groups, towns and industries have no place to
  name them by, so actions name them by canonical id
  (`tpf3mp/registry.lua`). Towns and the industries standing when the room
  began get theirs at the room's first update (or, in a room begun by an
  older mod, at its next update); an industry is named by its construction. Every game
  gives them the same ids without telling another: every game runs the
  same world, so after the same action the same things exist, and the
  mod's game script binds each new one to the next id of its kind, lowest
  entity first, after every action the room orders and at the room's
  first update, and retires the ids of those gone; an id never comes back.
  What an action made, as the game answered its command, is bound at
  once, whatever the game's lists say, and an id is retired only when its
  entity no longer exists, never because a list left it out. If checking an
  omitted entity's component raises an error, its binding stays until the
  game can establish whether it still exists; this also keeps one replica's
  transient read failure from changing later action references.
  A vehicle replacement whose returned entity cannot be read holds ordered
  replay before another room update runs; it cannot silently give the
  replacement a new canonical id.
  It keeps the registry in its state, which the game saves with the world,
  so a player who joins or reloads from the room's save has it as the
  others do. The GUI reads it from the script's state
  (`gameScriptSystem.getEntityForGameScript`, the `GAME_SCRIPT`
  component), as the loan window reads the loan script's. A command
  naming something the registry has no id for is refused, and says so;
- any other kind, and a command no wrapped factory made, is refused, as
  PLAN.md (Part 3) says of every action whose strict flag is off. It is
  not sent. Its callback, if it has one, is called on the next frame with
  `(command, false, {})`, as the game answers a command that failed. The
  game bar shows "Not in multiplayer yet: …" for a few seconds, and
  `hook.log` gets a line for the first refusal of each kind and every
  hundredth after.

The GUI runs in more than one Lua state, each with an `api.cmd` of its
own ("Companies" below). The one the game renders its React recipes in
gets the same guard (`tpf3mp/hudguard.lua`, installed from
`gui/tpf3mp/gui_state.script.lua`), so no window sends a command past the
room from either state ([COVERAGE.md](COVERAGE.md)). The room's
answers (`results()`) have one reader, the plugin's state, so the HUD's
state notes the tickets of its commands it waits on
(`tpf3mp_native.note("tpf3mp.hud.tickets", …)`), the plugin's state passes
on the answers to those (`hudguard.forward`, a note
`tpf3mp.hud.answers` of `ticket ok entity;`), and the HUD's state hears
them through the same `guard.deliver` as the plugin's: the vehicle store,
which renders there (2026-10-01), is told the vehicle it bought once the
registry names it, and puts it on its line; a new line's window opens the
line. Only a hook without `note` leaves the answers unrouted: a command is
then answered as sent, and one whose window waits on what it made
(`guard.RESULT`) is refused. That state has no frame the mod runs in: the
deferred callbacks and the answers run from the HUD's next reads of the
player's company, two clock ticks on. hook.log: `the guard is on N command
factories in the HUD's state`, and each refusal there `... in the HUD's
state: <why>`. Which windows render in that state is not known on build
40408 beyond the store; the log says if any sends a command.

The forwarding notes reserve their slots while idle. At most 24 commands
wait for routing at once; another is refused before submission and can be
retried. Answers are sent in batches of at most 512 bytes. Only the plugin
writes the answer note; the HUD acknowledges a batch by removing its tickets.
The plugin retains remaining answers and retries each frame, even without
new results, instead of overwriting unread answers or dropping overflow.
Vehicle parts with no load configuration use the store's first configuration
for every model compartment. A partially specified set is refused before
the engine buy command, with the expected and supplied compartment counts.

Before the room begins, and after it ends, every command is sent as it
would be, and every tool builds. A kind the room comes to carry is
captured into an action instead of refused, and applied by every game
("Actions in the game").

### Companies

A room holds up to eight companies (DECISIONS.md, D21). It starts as one,
the save's own player, which every player plays for; a player founds a
company of their own, joins another, renames or recolours theirs, or
dissolves it as its last player once it owns nothing, through the room
(`CompanyOp`), and every game applies it at the same update, as any
action. `tpf3mp/companies.lua` keeps the roster in the mod's game script's
state, which the game saves with the world:

- *A company is a TF3 player entity*, `makeGameAddPlayerCmd(name,
  colour)`: every game makes it at the same update of the same world, so it
  is the same entity in every game (seen on build 40408: entity 7595 in all
  three games of a room). Its money is that entity's `ACCOUNT`, and what it
  builds and buys is owned by it (`PLAYER_OWNED`), as the game keeps
  ownership.
- *Who acted.* The hook hands each ordered action to the game script with
  the player who sent it (the Lua link's version 10, `tpf3mp_native.version`:
  `takeReplay(token)` answers the actions
  and, second, each one's sender as 64 hex digits, and
  third, each one's seal; `status()` names each
  player's `id` and the local one's `me_id`). The game script books the
  action to that player's company: `apply.lua` puts the company's player
  entity where it put the save's player before (a build's `Context.player`
  and its constructions' and stops' `playerEntity`, `makeVehicleBuyCmd`'s
  and `makeLineCreateCmd`'s player, prospecting's `companyEntity`).
- *What a company builds* is its own: after a construction (a station,
  depot, airport, harbour) is built, every game hands the construction,
  its depots, its stations, the station groups they alone make up and
  its own (frozen) edges with what stands on them to the acting company
  with `makeEntitySetPlayerCmd` wherever anyone else owns them, or no one,
  as the game's missions hand one over (`setPlayerForConstruction`), and
  hook.log names each (`the new <file> made the acting company's`). An
  owner is read through the component's binding (`PLAYER_OWNED` is
  userdata on build 40408; read as a table only, every owner came back
  nil until 2026-10-02, so nothing counted as any company's). If the game
  cannot find the built construction, read an owner, or complete an
  ownership command, the action is reported as not applied with the failure.
  The engine may already have built the construction before this check, so
  a failed settlement can leave that partial result in this game.
- *What another company owns* is refused, the same in every game, naming
  its owner: an edited, bulldozed or removed construction, road or track
  edge, or stop, and the vehicles and lines an action names, when their
  `PLAYER_OWNED` player is another company's. What no company owns (the
  towns' roads) stays everyone's.
- *Loans.* The game's loan script (`::/game_mechanics/finance/loan.gs`)
  keeps the save's own player's loans only. Each founded company has its
  own copy of the available loan slots in the room's saved roster. It starts
  from the native offers, replacing a slot on the first company's cooldown
  with a fresh offer of that kind. A Take must match the exact type, amount,
  duration and rate in one of that company's active slots; forged terms and
  reused or cooling-down offers are refused before any journal entry is
  booked. On a valid Take, that slot enters its own 4-to-8-month cooldown,
  as the native loan script does, and `loan_util` draws its replacement
  when the cooldown expires. The room's update seed makes those draws the
  same in every game. The native loan table is never changed for a founded
  company's Take. The room books the money to that company as the game books
  a loan (a `LOAN` journal entry, `makeJournalBookAssetCmd`, which raises the
  account's balance and loan alike, seen on build 40408), and pays it back
  each month of the game's calendar as an annuity, the interest as
  `INTEREST` and the rest as `LOAN`, or all at once. Each company pays its
  own loans only. Paying one back names it by its id and amount. A company
  has four loans at most, as the loan script allows. In GUI states the
  finance window shows a player of another company that company's persisted
  offers and loans: `tpf3mp/follow.lua` answers its loan-script
  `GAME_SCRIPT` component with `companies.loanTable`, so Obtain and Repay go
  to the room as that company. The room's first company continues using the
  native loan state and finance window. The game script books the months
  since the last payment on the first update of a new month, in every game
  alike.
- *Subsidies.* The game's subsidy script
  (`::/game_mechanics/subventions/subventions.gs`, `subventions.script.tl`
  on build 40408) draws its offers in its `update`: once the last offer is
  older than an interval that follows the first company's rank
  (`subvention_util.getSpawnIntervalDuration`), it reseeds `math.random`
  from the game time (`math.randomseed(gameTime + math.random(0, 1000))`,
  after the hook's own reseed of the call, "Seeds, as built"), picks a kind
  and lets the kind draw the offer from the world (towns and industries in
  the engine's order, `gridCollision` weights, the first company's
  balance). So two games draw the same offers only while their worlds and
  **game times agree at every room step**. They did not in a room seen on
  2026-10-02 (two games): its world was replaced while the room ran (a new
  world from the lobby, not the room's load "from its save, from step N"),
  each game closed the old world and stepped the new one from another room
  step, so at room step 450 one game's new world had run 58 updates
  (`ticks: step 450: tickCount=58`) and the other's 75, the same action
  applied at game time 9800 in one and 13200 in the other, and even the
  company the room founded got another entity in each (47426 and 47334).
  The offers then differed, an accept applied in one game and was refused
  in the other ("the subsidy is no longer offered"), and the room split,
  noticed only at step 2250 (lane 3): the economy lane read the save's own
  player alone, whose subsidy money nets to nothing. Two changes make such
  a split show at once and say where: the economy lane carries every
  company's balance and the subsidy script's offers, taken, completed and
  failed subsidies with their terms (`tpf3mp/subsidies.lua`, `rows`), so
  the first checkpoint after the offers part diverges in lane 4, and its
  dump lists each subsidy; and the mod's game script says the offers in
  `hook.log` at a checkpoint whenever they changed (`subsidies at game
  time T: last=... modifier=... pause=...; N subsidies`, then one
  `subsidy: <offered|taken|completed|failed> <uid> <kind> spawn= accepted=
  completed= upfront= complete= failure= deliver= delivered= lapses=
  taker=` line each), so two games' logs show where they part. A world
  stepped from different room steps is the step gate's to hold, not the
  subsidy script's. A world loaded from the room's save starts every game
  at the same step and game time, and its offers agreed there: two games
  that loaded a room's save listed the same offers, and drew the same new
  ones afterwards, at every checkpoint of a run
  ([investigation/TPF3_SUBSIDIES_2026-10-07.md](../investigation/TPF3_SUBSIDIES_2026-10-07.md)).
  Offers belong to no company. Accepting one (`Subsidy::Accept`) is
  checked by every game against its script's state first: the offer under
  that number must still be offered, of the kind the action names, and the
  only offer under that number; else it is refused, alike in every game
  whose world agrees, naming who took it ("the subsidy was taken already,
  by Rival"). So when two companies accept one offer in the same step, the
  first in the room's order gets it, and the offer leaves every company's
  list. A game cannot refuse alike what only it lacks: its world already
  differs from the others', and the economy lane reports that at the next
  checkpoint. Then the script's own `onAccept` runs, with the taking
  company's player entity (`{ uid, tpf3mpCompany }`), and books the money
  up front. The script books every amount, up front, the reward for
  completing and the penalty for failing, to `getPlayer()`, the room's
  first company in a game script's state (`subvention_util.tl`,
  `applyBonusMalus`). For a subsidy another company took, every game moves
  each amount on to that company as `SUBSIDY` journal entries (out of the
  first company's account and into the taker's): the money up front at
  once, and the reward or penalty on the first update of the game day
  after the script completed or failed it, while the room keeps a record
  of who took which (`roster.subsidies`), saved with the world. When the
  taker is gone (dissolved) by then, its reward and its penalty are no
  one's: the first company gives back the reward, or gets back the
  penalty, the script booked to it, so it ends with nothing of a subsidy
  it did not take. Companies do not merge in a room. Declining
  (`Subsidy::Decline`) runs the script's `onDecline`: the offer is gone
  for every company, as in single player.
  *Progress is the taker's.* Each kind counts progress in its own
  `handleEvent`, for anyone's transport: `deliver_passengers` completes
  when a person starts a direct line between its towns
  (`OnStartedLineUsage`, SimPersonSystem), then doubles that line's
  tickets; `deliver_cargo` and `deliver_cargo_town` count each cargo
  delivered to the industry, or to the town's buildings, by the line that
  carried it (`OnCalcTicketPrice`, TransportVehicleSystem), then double
  those lines' cargo. The subsidy script reaches a kind's functions
  through the kind's resource at every call (`util.useFn(scriptFile ..
  ".handleEvent")`). The mod's run script (`mod.script.lua`, `runScript`
  in `mod.json`) adds a resource modifier (`addModifier("loadGameRes",
  subsidies.redirect)`) that points the four base kinds' resources at
  `tpf3mp_sim/subsidies.script.lua`, which wraps each kind's own table:
  on the room's `onAccept` the wrapper keeps the taker in the subsidy's own
  data (`tpf3mpTaker`, in the script's state, the same in every game), and
  from then on hands the kind only the entries whose line the taker owns
  (`PLAYER_OWNED`), mapping the ticket multipliers it answers back to the
  event's own entries. Resources are the whole game's, so every Lua state
  runs the same wrapper. A subsidy with no taker kept (single player, a
  save from before) counts everyone's, as the game does. Only while
  `acceptance.subsidies` is on. The modifier's name is read from the
  binary (`loadGameRes`, beside `loadConstruction` and `loadGameScript`),
  and seen working in a two-game run: a passenger subsidy another company
  took stayed open while the first company's line between its two towns
  completed the first company's own subsidy for the same towns, in both
  games ([investigation/TPF3_SUBSIDIES_2026-10-07.md](../investigation/TPF3_SUBSIDIES_2026-10-07.md)).
  The base game's subsidy cards know no companies: every player sees every
  taken subsidy as accepted, another company's too. Not carried:
  `deliver_workers` completes when the industry's workers are boosted (`industry_util.isPersonCapacityBoosted`),
  a state of the industry the engine keeps for no company, so anyone's
  commuters complete it; the reputation and town growth bonuses are the
  towns', as the game has them; the pace of new offers follows the first
  company's rank (`subventions.script.tl`).
- *Colours.* With more than one company, a vehicle bought is painted in its
  company's colour (`makeEntitySetColorCmd`), and a new colour repaints the
  company's vehicles, in the engine's own order. With one company the
  game's colours stay, as in single player.
- *Markers.* Painting colours a vehicle's body and its pictures in the
  game's windows, not its marker on the map: build 40408 draws every
  vehicle's marker alike, a white glyph on a dark tile
  (`HudIconManager.cpp`, `gui/main/internal_hud.css.lua`), where TPF2's
  followed the vehicle's paint. `gui/tpf3mp/company_markers.res.lua`, a
  `react-replacement-config`, replaces the game's marker recipe
  (`hud_icon_toolbox.HudIconMasterGame`) with one that calls it and, while
  the room has more than one company, puts the marker of a vehicle painted
  in a company colour in that colour's class; `gui/tpf3mp/tpf3mp.css.lua`
  colours the tile of each class. Seen on build 40408: the HUD renders
  markers on its worker threads ("Main Pool" in the log), each with a Lua
  state of its own where the Multiplayer plugin does not run and a
  vehicle's `PLAYER_OWNED` reads empty; so each reads the roster from the
  game script's state (every 2 seconds) and the vehicle's first part's
  colour, and the HUD takes only a layout from the recipe ("Recipe child
  must be a layout"). The game's log says what became of the markers
  (`[tpf3mp] company markers: ...`).
- *Capitals.* Build 40408 crowns one town on the map, the player's
  capital: the town label's recipe (`town_hud_react_util.TownHudIcon`)
  asks `town_util.isCapital(town)`, true for the town closest to
  `getPlayer()`'s PLAYER `headquarters`, and gives that label the class
  `capital-city` (blue, `hud_icon_master.css.lua`) and a crown; nothing
  else in the GUI asks it. `gui/tpf3mp/capitals.res.lua`, a
  `react-replacement-config`, in each GUI Lua state that renders recipes
  and never in the simulation's, while the room has more than one company:
  has `isCapital` answer true for every live company's capital as well
  (each company's PLAYER `headquarters` and its closest town, read again
  every 10 seconds, `tpf3mp/capitals.lua`), and replaces `TownHudIcon`
  with a recipe that calls the game's inside a layout of its own, with a
  line under it, "Capital of Rival" ("... and Pals" when two share a
  town), and, for a capital not the viewer's company's, the class
  `tpf3mp-capital-<n>` of that company's palette colour (the nearest for
  a colour of its own); `gui/tpf3mp/tpf3mp.css.lua` colours the label's
  tile and line in it in place of the game's blue. The viewer's own
  capital keeps the game's blue, as it is the company `getPlayer()`
  answers there (*The GUI's company*). The game's log says what became of
  it (`[tpf3mp] capitals: ...`), including how many capitals each state
  read: none where a state cannot read the companies' PLAYER.
- *The GUI's company.* TF3's windows ask `api.engine.util.getPlayer()`
  whose money to show and what is the player's own ("Foreign" otherwise).
  In the GUI state the mod replaces it (a callable table on build 40408,
  which takes the assignment) with one that answers the player entity of
  the company this player plays for, or the game's own answer for the
  room's first; `hook.log` says `the GUI's company follows the player's`.
  The game scripts' states keep the game's own answer, so the simulation
  is the same in every game. Seen on build 40408: the game bar's account
  showed the new company's money, and another company's depot opened
  without its vehicle management. The GUI runs in more than one Lua state:
  the line manager's HUD, drawn in another, showed the room's first
  company's depots to a Rival player and not Rival's own. So
  `gui/tpf3mp/gui_state.res.lua`, a `react-replacement-config` that
  replaces no recipe, has the game run `gui_state.script.lua` in the state
  it renders its recipes in, before any renders; there
  `tpf3mp/follow.lua` gives getPlayer the same answer, read from the hook
  (`status().me_id`) and the game script's roster every 2 seconds
  (`hook.log`: `the GUI's company follows the player's in the HUD's
  state`).

  Seen again on 2026-10-02 (build 9ad8837, competitive room): with every
  state saying it followed, the line manager showed the first company's
  stations, not Rival's own, and Rival's stops were "another company's".
  Every one of those views decides "mine" in Lua at call time
  (`scripts/entity_util.tl`'s `isOwnedByPlayer` and
  `isOwnedByPlayerOrNotOwned`, and `getLinesForPlayer(getPlayer())`,
  `requireOwnedByPlayer = getPlayer()` in `line_vehicle_mgmt/manager_window.tl`,
  `station_group.tl`, the vehicle and depot lists, the finance window's
  `getPlayersBalance(getPlayer())`), so the state's getPlayer was the
  game's own when they ran. No native player stands behind them that the
  hook could write instead: the GUI's `GameState` (`CGame+0x1e0`, where
  getPlayer's answer and the bulldozer's list come from, `+0x20c`) is one
  of the simulation's two buffers (the probe, SEEN: `the GUI's is buffer
  [0]`, then `[1]`), so writing it would change the simulation.
  INFERRED: the state's api was made anew after the mod's script ran (a
  React root reloading its interfaces), and the install, once per Lua
  state (`package.loaded["tpf3mp.followed"]`), never came back. Now
  `follow.ensure` puts the company in front of the current api's getPlayer
  whenever it is not there, and the
  game's two ownership tests in each state's `entity_util` call it before
  they answer, so the first window that asks after a new api gets the
  company. Where a state cannot read the roster, the company the
  Multiplayer plugin's state notes for the hook (`tpf3mp.company`, which
  the native tools already act on) answers (`follow.noteSource`). Each
  state says what its getPlayer answers, and when it had to put the
  company back:

  ```
  the GUI's getPlayer answers the player's company 372630 (the HUD's state)
  the GUI's getPlayer was the game's own again (a new api in this state); it follows the player's company again (the HUD's state)
  ```

  No state saying it answers the company while a window still shows the
  first company's things would mean a Lua state the mod's scripts never
  run in.

  The notes live as long as the game's process, the entities they name
  as long as their world. When a world closes the hook forgets
  `tpf3mp.company` and `tpf3mp.companies` (`lua::forget_world_notes`,
  from the menu frame that sees `CMenuUI::m_game` cleared), and says so:

  ```
  menu: the closed world's company note(s) forgotten (2): the next world's views follow its own room's roster
  ```

  Kept, the next world's GUI answered getPlayer with the last world's
  company, natively and through `follow.noteSource`, and the game's own
  game bar asked `getPlayersBalance` of an entity that world does not
  have: the engine's `Account` lookup is unchecked, so a brand-new world
  started from the Multiplayer lobby crashed in its first frame
  (2026-10-02: an access violation reported as a hang, entity 372610;
  then `Engine.h:323 GetComponentDataIndex: Assertion 'it !=
  components.end()'` for entity 63030, an animal there, followed by the
  shutdown's "Buffer has not been destroyed" render errors).
- *In a competitive room* the GUI founds the player a company of their
  own (`tpf3mp.script.lua`, `foundOwnCompany`): the same `CompanyOp`
  `Create` **Found** sends, named `<name>'s company`, sent by the
  player's own game, so the room orders it for every game like any other
  action (D8: the server never writes an action). Only while
  `status().competitive` is true, the roster is read and says the player
  plays for the room's first company, and no company of the room, dissolved
  ones included, was founded by them; and not before four readings of the
  room since the world's GUI linked (`OWN_SETTLE`, about a second), so a
  world that is still catching up has applied what the room ordered
  before. A reading that lacks the roster or the play style delays it no
  further: the readings need not agree in a row. At most once a room and
  player in this Lua state. Each reason not to found is said once in
  `hook.log`: `not founding the player's own company: <why>` (the
  launcher has not said whether the room is competitive, the room is
  co-op, the room's companies are not read yet, the player plays for
  <company>, the player founded a company in this room before, ...).
  The player's name is their entry's by id in `status().players`, else
  the first eight hex digits of their id. The name is the same each
  time, so a second one sent before the first arrives is refused alike in
  every game; with another player of the same name in the room, the first
  four hex digits of the player's id follow it. `hook.log`: `a competitive
  room: founding the player's own company`.
- *The Multiplayer window* lists the companies with their money and
  players, the one the player plays for first with its colour to choose
  (the game's colour chooser, `ColorChooserButton`, with the companies'
  colours first and then the game's line colours and greys, as its vehicle
  window offers them), a name to change, loans to take and pay back, and
  each other company to join, with a password field (`passwordMode`, as
  the game's own login form) for one that has a password. Each company
  shows its head, and whether it has a password or has closed its
  stations. The head of the player's company also sees its password to
  set, change or remove, its stations to open or close, and a button to
  send each other player out.
- *The game's company window* renames the company by its title
  (`game_mechanics/company/company.tl` sends `makeEntitySetNameCmd` on the
  player entity `getPlayer()` answers, the player's company): the guard
  captures a name or colour set on a company's player entity as that
  company's `Rename` or `Recolor` (`capture.setName`, `setColor`, which
  know companies by the GUI's roster), and every game checks it is the
  player's own. The window has no extension point for more; the rest
  stays in the Multiplayer window.
- *Who may do what* (DECISIONS.md, D22, proposed), checked by every game
  alike when the room orders it (`companies.run`): a company's players
  build, buy, run lines, borrow, rename and recolour it (a colour of
  fractions from 0 to 1 that no other company wears); its head, the
  founder while they play for it and else the player who has played for
  it longest (the roster's members are kept in the order they joined),
  alone gives it a password (`CompanyOp::Lock`), takes it away
  (`Unlock`), sends a player out (`Dismiss`: they play for the room's
  first company again) and opens or closes its stations by default
  (`ShareStations`), or for one other company over the default
  (`StationAccess`, action schema 23; `open` nil puts it back to the
  default; the roster keeps it as the company's `access` list). The
  line manager offers a station, and every game refuses a line's stop,
  by the same rule (`companies.lets`, `mayUse`). The room's first company
  is everyone's: no head, no
  password, and its stations stay open. `hook.log` names why a refused
  action was refused.
- *Passwords.* The window hands the password to `command` beside the
  action (`Join` or `Lock`), the hook sends it to the room beside the
  intent (`tpf3mp_proto::Secret`, scoped to the company), and the server
  orders the action with the password's seal, never the password
  (PROTOCOL.md, "Secrets"). `take()` hands each action's seal third; a
  `Lock` keeps it as the company's `lock`, and a `Join` of a company with a
  lock needs a seal for that company equal to it ("joining X needs its
  password", "the password for X is not right"). The roster keeps the seal
  only, which is safe in a save: it cannot be checked against a guess
  without the server's key. The window reads only whether a company has
  one.
- *Using another company's stations.* The game stops a line at any
  station: build 40408's line and stop commands check that a stop names a
  station and terminal the station group has, not who owns it (no owner
  refusal among the exe's line errors; a stop is a station group, a
  station index and a terminal, `api/tealdef/api/engine.d.tl`). Its line
  manager offers only the player's own stations and those no one owns
  (`gui/line_vehicle_mgmt/manager_window.tl`, at each click that adds or
  moves a stop, asks `scripts/entity_util.tl`'s
  `isOwnedByPlayerOrNotOwned`). In the GUI state the mod wraps that test so
  it also takes a station group or station construction of a company that
  keeps its stations open; `hook.log` says `the line manager offers other
  companies' open stations`. The wrapper is installed in each GUI Lua
  state, including the HUD's separate state. Two-game GUI acceptance on
  2026-10-02 confirmed own closed stations, explicit grants and default
  grants for another company's full bus station and roadside stop.
  Build 40408 also reports some station clicks as TransportNetworkEdge
  details: the HUD's `line_util.convertDetails` wrapper recovers ordinary
  station details after checking access. Genuine edges and explicit terminal
  selections retain their native interpretation. Only the HUD loads this
  module, because its React builtin recipes are unavailable in the
  game-script GUI state. Every game
  checks each stop of a new or changed line (`apply.lua`, `lineComponent`,
  `companies.mayUse`) and refuses one at a closed company's station, naming
  it. TPF2 had to patch a native station filter for this (TPF2MP's shared
  stations); TF3's ordinary picker was exercised without a native ownership
  patch. Native PLAYER_OWNED components are userdata; `ownerOf` reads their
  player field instead of discarding them as non-tables. A company's vehicles still use its own
  depots, as TPF2MP left `FindPathToDepot`'s owner check alone.

### Prospecting

Prospecting for an industry near a town
([investigation/TPF3_PROSPECTING_2026-09-30.md](../investigation/TPF3_PROSPECTING_2026-09-30.md))
is one command from the player's GUI, and everything after it happens in
the simulation:

1. The construction menu's prospection, on the town the player picks,
   sends the company script `Companies` `spawnIndustry`. In the room's
   game the guard captures it (`capture.prospect`) as a `Prospect` action:
   the town by its canonical id, the cargo, the industry types in the
   menu's order, the permit. It is refused, and says why in `hook.log`,
   for a town the registry cannot name, another company, or no industry
   type. The menu keeps its permit reserved until this game has applied
   the action, as it does until the game answers its own command.
2. Every game applies it at the same update, through the company script's
   own event (`apply.lua`). The company script checks that the company
   does not prospect there already, keeps the prospection with the game
   time as its start, uses the permit, and tells the prospecting plane to
   fly.
3. Months later (six at most, trying each month with a growing chance),
   every game's company script draws the outcome, from the game time
   (`math.randomseed(gameTime * 1000 + index)`), shuffles the industry
   types by those draws in the order the action carried them, and asks the
   game for a place (`makeIndustrySpawnProposal`, seeded from the game
   time too). It builds the industry, with the construction's seed the
   game chose, itself: not player-initiated, so the hook's build stop lets
   it through, and in the engine state, where the guard is not. A
   prospection that finds nothing gives the permit back.
4. The mod's game script hears the company script's `startProspection`
   and `endProspection` and says each in `hook.log`; an industry found is
   bound in the registry at once, so every game names it by the same id.

What the room carries is the request, not the outcome: the outcome is a
function of the world and the game time, which every game has alike
(INFERRED from the scripts and the binary; the investigation says what is
seen and what is not). The permit check is the menu's alone, as in single
player: two players prospecting in the same moment could each pass their
own menu's check and use one permit more than the company has, the same in
every game.

In `hook.log`, prospecting for coal near a town shows first, in the game
of the player who picked the town:

```
handed the player's action 42 to the room
```

then in every game of the room, at the same step:

```
prospecting for ::/cargos/coal/coal.cargo near town-3 (1234): coal_mine
prospecting began: ::/cargos/coal/coal.cargo near town-3 at game time 5400000
the game applied the room's actions between simulation updates
```

and, one to six game months later, again in every game at the same step:

```
prospecting ended: ::/cargos/coal/coal.cargo near town-3, begun at game time 5400000, found industry-17 <the coal mine's construction file> at (1234.5, -250.3)
```

or `..., found nothing`. The entity in brackets on the first line is each
game's own; the rest, the industry's id, file and place included, is the
same in every game.

The game's company script runs the prospections of the save's own player
alone (`company.script.tl`, its update looks at `getPlayer()` only, in the
engine state): a prospection of another company is kept and its permit
used, but its outcome is never drawn (seen in the scripts; not carried yet,
docs/PLAN.md).

### Company perks

The construction menu's perk tools (`gui/construction/tools/`, build
40408) each send the company script one event, which spends the perk's
permit for the company and hands the perk on:

- **Industry Greenification** (`industry_greenify_tool.script.tl`) sends
  `Companies` `MakeGreen` with the industry part the player picked; the
  company script tells the emissions script to cut its emissions. The
  guard captures it (`capture.greenify`) as `Perk::Greenify`, the industry
  by its canonical id: the registry binds industries by their
  constructions (`tpf3mp/registry.lua`), and every game takes that
  construction's one industry part (`capture.industryPart`). A
  construction with more than one industry is refused, as no id tells its
  parts apart.
- **Marketing campaign** (`marketing_campaign_tool.script.tl`) sends
  `Companies` `startMarketingCampaign` with the town and the campaign's
  terms (`durationMs`, `lineCostFactor`); the company script starts it in
  the towns script. The tool books its price in the event's callback with
  `makeJournalBookAssetCmd`. The guard captures the event
  (`capture.marketing`) as `Perk::Marketing`, with the price the tool
  charges in that year (`capture.marketingCost`, the tool's own formula);
  every game checks the company can pay it, starts the campaign through
  the company script and books the price itself. The tool's own booking,
  sent from its callback, is neither sent nor refused (`guard.FOLLOWS`),
  so the price is paid once, in every game.

Both are refused for another company, an industry or town the registry
cannot name, and, until a two-player game shows matching permits, town
reputations, emissions and money, by `acceptance.lua`'s `perks` gate in
the sender and in every game's replay.

### Company ranks

A company's rank gives it the game's permits: headquarters, marketing,
prospecting and the rest
([investigation/TPF3_PROGRESSION_2026-09-30.md](../investigation/TPF3_PROGRESSION_2026-09-30.md)).
The game's growth script keeps one company, the save's player: its
experience is the highest world population it has seen, the rank that
reaches is its potential, and the company window takes a rank reached with
`Companies` `applyLevel`.

- **One company in the room**: the game's own score. The company window's
  `applyLevel` goes to the room as an `ApplyRank` action, and every game
  applies it at the same update through the growth script's own event. A
  rank not reached, or taken already, is refused with why in `hook.log`.
- **More than one** (D23, proposed): the mod's game script scores every
  company four times a game month, at the same game time in every game
  (`tpf3mp/progression.lua`): for each town, the town's population times
  the company's share of what was carried for it (cargo delivered to it in
  the last half year, passengers travelling to and from it on lines, from
  the game's statistics per line and the lines' owners) times its rating
  there over 100 (the town's rating, with the happiness of its own
  passengers and the punctuality of its own cargo, by the game's formulas).
  Experience is the highest score, the rank it reaches the game's own
  thresholds. The records are the mod's game script's state, saved with
  the world. `ApplyRank` takes a rank reached in that record; the room's
  first company also takes it in the game's own state. In the GUI the
  mod answers the game's windows' `getCompanyProgressionState` from the
  record, so the company window and the permits show the player's
  company's own rank (`the company window shows each company's own rank`
  in `hook.log`; INFERRED that the game's windows share the module the mod
  changes, to check in the game).

Nothing is guessed: a sample whose statistics or game modules do not read
is left out (`the companies' scores were not sampled: ...`) and remains due
for retry in the same game quarter. Previously such a failure persisted the
quarter before reading it, losing that sample's chance to earn XP. A town
with no rating counts for nobody.

In `hook.log`, every game of a room of two companies writes at each
sample, at the same game time and with the same numbers:

```
room ranks loaded: quarter=431 companies=2
room ranks loaded: company-0 entity=25 experience=20514 potential=3 level=2
room ranks loaded: company-1 entity=901 experience=800 potential=0 level=1
progression at game time 5400000: 12 towns, weights cargo 1 passengers 1
progression at game time 5400000: town-3 population 1240: company-0 share 0.5500 rating 80.0000 part 545.6000, company-1 share 0.4500 rating 64.2859 part 358.7153
progression at game time 5400000: company-0 score 3120.4000, experience 20514, rank 3 reached, 2 taken, prior experience 20514, sample quarter 432
progression at game time 5400000: company-1 score 812.9000, experience 812, rank 0 reached, 1 taken, prior experience 800, sample quarter 432
```

The `room ranks loaded` lines report the saved records on the first
multicompany update of this game-script state, before sampling; at most 16
companies are listed, with an explicit omitted count beyond that. Compare
them with the last sample before a rebase and the first sample after it to
see whether the offered world restored or lost experience. The sample logs
one line per town someone carried for, then one per company, including the
experience before that sample and the quarter persisted with it. Compare
lines at the same game time across games: any difference is a divergence.

### Headquarters

What the game does (build 40408, its scripts): a headquarters is the
construction `landmarks/hq/headquarter.con`, whose company metadata says
`headquarters = true` and names the permit `permitKeys/hq.res`, one at
rank 1. The engine keeps one headquarters a player entity, its `PLAYER`
component's `headquarters` (`api/tealdef/api/engine.d.tl`), which the
game's capital town reads (`town_util.isCapital`, for `getPlayer()`).
But the game counts a permit's constructions over the whole world,
whoever owns them: the construction menu's
`company_util.getConstructionDisableCacheData` (its `numBuilt`, "Already
Built") and the tool's `company_util.countUsedConstructionPermits`
("All 1 Permits Used Up"), both through
`streetConnectorSystem.forEachConstructionWithMetadata`. So once one
company had its headquarters, no other company could build one
(2026-10-01, three players).

What the mod does, with more than one company in the room:

- **Every game** refuses a second headquarters of the same company when
  the room orders it (`companies.mayBuild`, from `apply.lua`'s
  `BuildConstruction`): "<company> has its headquarters already". What
  counts is a construction whose resource's metadata says headquarters
  and which the acting company owns (`PLAYER_OWNED`). An edit of the
  headquarters (its modules, `replaces`) is no second one. A construction
  this game cannot tell is refused with why.
- **In both GUI states** (the plugin's and the HUD's), the two
  `company_util` functions count the player's company's constructions
  only (`companies.followPermits`; the module as the game loads it under
  both `/game_mechanics/...` and `::/game_mechanics/...`): the menu
  offers each company its own headquarters, and its upgrades by its own
  rank. With one company, the game's own counts. `hook.log`: `the game's
  permits count each company's own constructions (N company_util
  table(s))`, and the same `in the HUD's state`.
- **In the game scripts' GUI state** too, where the game's company script
  checks a construction's permits as the tool proposes it
  (`company.script.tl`, `builder.proposalCreate`: an error and
  `skipRender`, so the tool shows no preview and builds nothing): the mod's
  game script's `guiHandleEvent` puts on, once a state, getPlayer as the
  player's company (`follow.lua`), the company's own rank
  (`progression.follow`) and its own permit counts (`followPermits`), each
  a no-op where another GUI state sharing the tables did first. Without
  them a founded company's headquarters showed no preview (2026-10-01): the
  script read the save's player's rank and counted every company's
  headquarters. `hook.log`: `the game scripts' GUI state: getPlayer follows
  the player's company; ranks are each company's; permits count each
  company's own constructions`. The executable has no headquarters check
  of its own in the construction tool: `"headquarters"` is read only by the
  proposal's apply (`sub_9f96e0`, which sets the paying player's
  headquarters) and the Lua bindings' setup.
- After a headquarters is built, every game logs what the engine made of
  it: `headquarters for company entity <e>: its PLAYER names <entity>`.

What a headquarters gives, and to whom (build 40408, its scripts and its
executable, read 2026-10-01):

- **Its town's growth.** The construction carries `town_growth` metadata:
  experience +5% (`landmarks/hq/headquarter.script.tl`), +1% more for each
  medium wing, and reputation recovery +1% for each large wing (the
  modules' metadata, summed by `headquarter_addon.script.tl`). The game's
  town script (`game_mechanics/towns/towns.script.tl`,
  `updateConstructions`, every 20 updates) sums that metadata of every
  construction in the world, **whoever owns it**, onto the town closest to
  it (`landmarks/landmark_util.tl`, `collectTownGrowthMetadata`, without
  `playerOwnedOnly`); the town's experience then grows by
  `1 + xpIncrease` (`town_util.getXpFactor`, `town_growth.script.tl`), its
  reputation recovers faster (`applyEventDecay`), and its "Bonuses" rating
  in the town window shows Excellent instead of Good
  (`town_util.getRatingBonuses`). So each company's headquarters gives
  its town what a single player's gives, in every game alike, at the same
  step: the mod adds nothing to it. Two headquarters closest to one town
  add up, as the game adds up any landmarks there.
- **Workplaces**: 24 industrial places (`personCapacity`), for any owner.
- **Nothing per company.** No rank, experience, permit or company value
  comes from having one: the company's experience is the world's
  population (D23), the headquarters is itself a permit (rank 1) and its
  wings are permits by rank (`rankAndPermits`, the company window's
  "Headquarters Upgrades"), and `getCompaniesValue()` has no headquarters
  term (`api/tealdef/api/type.d.tl`, `CompanyValue`).
- **Its PLAYER `headquarters`.** The engine sets it in the build's apply
  (`apply_proposal.cpp`, `sub_9f96e0`, rva 0x9fd78a-0x9fd9b0): for each
  construction added whose description's company metadata says
  `headquarters`, for the construction's own `playerEntity` (its assert
  "ce.playerEntity != ecs::Entity()"), and a removed one clears it for its
  owner. The room's builds name the acting company as `playerEntity`
  (`apply.lua`), so each company's PLAYER names its own (read statically;
  the log line above shows it in a real game). Only the GUI reads it: the
  town's capital badge (`town_util.isCapital`), the "Headquarters"
  tooltip and selection (`game_tooltips.tl`, `view_manager_util.tl`),
  all through `getPlayer()`, which the GUI's states answer with the
  player's company (`tpf3mp/follow.lua`): each player sees their own
  company's town as the capital. The native selector filter
  (`UI::CreateSelectorFilter`) reads the local player's, but selects any
  owned construction anyway.

With more than one company, at each of the companies' samples (four times
a game month, `tpf3mp/progression.lua`), each game logs every company's
headquarters and its town's bonus, read only, a line again only when it
changed:

```
headquarters: Rival #1: headquarters 701 (a construction), owned by 901; closest town 31 (Ashford): on it xp +0.05, reputation recovery +0.00; the game's town script applies xp +0.05, reputation recovery +0.00 there
```

The headquarters is the one the company's PLAYER names; "owned by" another
entity than the company's, "no construction", or a town script that
applies less than what is on it, is a fault to report. The report is
bounded and never waits: a few engine reads for each of at most eight
companies, no pass over the world's constructions, nothing more while no
company has a headquarters, every read in a `pcall`.

Once a world is up, with more than one company, each game logs what each
company owns as the engine records it, read only: `ownership: <company>
#<id> (entity <e>): N construction(s), headquarters <entity>; ...`. A world
loaded from a save that shows a company owning nothing it built has lost
its owners in the save.

The game's own tools act as the save's player, not the player's company:
its street, track and construction tools put the engine's player
(`playerEntity`, the room's first company) in their proposals whatever
company the player plays for, so a company's own stops and stations are
another player's to them (no snapping), and the HQ's Configure opens the
native module builder under the same player (its button shows for every
headquarters: `perk.tl` gates it on `isHeadquarters` alone, and its click
on `entity_util.isOwnedByPlayer`, which the mod answers). The street,
track and modifier tools and the bulldozer now act as the company ("The
tools' player" below); the other tools not yet: see the investigation of the engine's player in
[investigation/TF3_LOCAL_PLAYER_2026-10-01.md](../investigation/TF3_LOCAL_PLAYER_2026-10-01.md).

**The probe of the engine's player** (`crates/tpf3mp-hook/src/probe.rs`),
off unless the game's environment has `TPF3MP_PROBE_PLAYER=1`. Read only:
it takes field offsets from two optional profile targets (`probe: GUI
GameState getter`, `probe: engine GameState getter`), checks every pointer
readable before it reads it, and writes nothing. In `CMenuUI::DoStep`'s
detour, after the game's frame, for five frames whenever `m_game` changes
and then every 3 s, `hook.log` says:

```
probe: reading the engine's player, read only: the GUI's GameState at [[menu+0x6b0]+0x1e0], the engine's at [[game+0x1f0]+0x78+8*i], i at +0x98; 8192 bytes of each scanned every 3 s
probe: at the menu, no CGame (m_game is 0)
probe: CGame 0x...: GUI GameState [+0x1e0] 0x...; engine buffers [+0x1f0] 0x...: [0] 0x..., [1] 0x..., i 0; the GUI's is neither buffer
probe: player 118368 in GUI 0x...: +0x40q +0x1a0d
probe: player 118368 in engine [0] 0x...: +0x40q
probe: player 118368 in engine [1] 0x...: none
```

The player is the save's, as the mod's game script notes it once linked
(`note("tpf3mp.player")`); until then `probe: the save's player is not known
yet`. Without the variable: `probe: the engine's player is not probed`.

With the same variable it also counts who asks the proposal street graph
for an entity's owner (`probe: ProposalStreetGraph::GetPlayerOwnedPtr`,
rva 0xa46cd0): a pass-through detour that always calls the original and
returns its answer, counting each caller's return address in a fixed table
of 64 rows by side (inside `GameSim::Step`, on a simulation pool thread,
or on the main thread outside the step: the GUI and its tools), with no
lock and no allocation. Every 3 s, from the menu's frame:

```
probe: counting the callers of probe: ProposalStreetGraph::GetPlayerOwnedPtr at 0x...; its answer unchanged; flushed every 3 s
probe: owner read from rva 0x...: in the step 0, sim pool 0, GUI 412
probe: owner reads: none in 3 s
```

A caller counted under GUI only is a tool's or the GUI's own; one under
the step or the pool is the simulation's.

With the same variable it also logs what the native tools' ownership test
takes for another player's (`probe: street_util IsOwnedByOtherPlayer`, rva
0x610ea0; investigation/TF3_LOCAL_PLAYER_2026-10-01.md, "Why the street
tool will not split a road the room built"): a pass-through detour that
always returns the original's answer. When it answers true it counts the
entity, under the tool's player, in a fixed table of 64 rows (no lock, no
allocation), and the first time asks the original again with the company
this player plays for (the GUI's `note("tpf3mp.company")`,
`tpf3mp/follow.lua`) and with the save's player (`note("tpf3mp.player")`)
to say whose the entity is. Every 3 s, from the menu's frame, nothing when
nothing was answered:

```
probe: logging what probe: street_util IsOwnedByOtherPlayer at 0x... takes for another player's, its answer unchanged; flushed every 3 s
probe: a native tool took entity 380001 for another player's: the tool acts as player 214443, the entity is owned by this player's company 372363; 41 time(s), first from rva 0x5fc027
```

A line like that one, for a road the room built for the player's own
company, is the street tool refusing to snap into it; rva 0x5fc027 is the
street builder's snap.

With the same variable it also logs the street bulldozer's answers: its
edge test (`probe: StreetBulldozerAction edge test`, rva 0x5f2a00, a lambda
of `UI::StreetBulldozerAction::vf2`) and the owner test every bulldozer
action calls (`probe: bulldozer owner test`, `sub_5f7db0`), each with the
entity, the owner list it had and the answer, merged by equal answers,
merged by equal answers and callers (the return address, as an RVA), every 3 s:

```
probe: the street bulldozer's edge test on entity 380001: refused with owner list [214443]; 4 time(s), from rva 0x5f2f4b
probe: the bulldozer's owner test on entity 380001: allowed with owner list [372363]; 4 time(s), from rva 0x5f2ae6
```

An edge refused by the edge test with the company in its list, and no
owner-test line for it, was refused before ownership (a construction's
edge, another network, the underground mode).

**The tools' player** (`crates/tpf3mp-hook/src/toolplayer.rs`), on unless
`TPF3MP_HOOK_TOOL_COMPANY=0`. `UI::CGameUI`'s constructor reads the save's
player once and hands each native tool a copy; the room builds roads,
tracks and constructions as the acting company's (`PlayerOwned`), so for a
player of any company but the room's first the tools took the company's
own edges for another player's: the street tool snapped only to their ends
(`sub_610ea0` from its snap marks another player's edge fixed), the
bulldozer would not offer them, and the tram track tool would not join the
company's rail. In a room, at the start of each tool's `Step`, on the main
thread, the hook writes the company the GUI notes
(`note("tpf3mp.company")`, `tpf3mp/follow.lua`) into three tools' copies:

| tool | field | its `Step` |
|---|---|---|
| `UI::StreetBuilder` (the street and the track builder) | `+0xc0` | `0x585e50` |
| `UI::TrackModifier` (tram track, bus lane, electrification and the other modifiers) | `+0xa0` | `0x5cbf80` |
| `UI::ConstructionBuilder` (stations, depots, every construction the menu places) | `+0xa0` | `0x51cd60` |
| `UI::StreetTerminalBuilder` (the stop builder, and the signal and waypoint builder, a second instance) | `+0xa0` | `0x595f30` |
| `UI::ModuleBuilder` (a station's modules) | `+0xa8` | `0x545b50` |
| `UI::Bulldozer` (every bulldozer action) | its own player (`+0x28`), its owner list (`+0xa8`, the one player every query it makes is built from) and the one player of its `BulldozerFilter`'s copy (`[[+0xc0]+0x10]`) | `0x4d6340`, and its list setter `0x4d6220` |

Each field's offset is read from its constructor's code (profile targets
`... ctor/player store`, `UI::Bulldozer ctor/filter`, which also gives the
filter's vtable, checked before every write, and its one-player list);
code that is not exactly the expected shape leaves that tool alone. Each is a UI
object's field read only by its own class's code (the profile lists every
reader), so nothing the simulation runs sees it. A field is written only
where it holds the save's player (`note("tpf3mp.player")`) or the company
this wrote; any other value is left and said once. Outside a room, for the
room's first company, or when either note is missing, the game's value
stays, and one this wrote goes back. The builds go through the room as
before: capture reads a proposal's ownership as owned or not, the room
builds for the acting company, and every game refuses an edit of another
company's edge (`companies.mayTouch`). So the tools now also refuse a split
or bulldoze of another company's road (the room's first company's
included), as the room does. hook.log:

```
tool-company: the street and track builder acts as the player's company in a room (UI::StreetBuilder::Step at 0x...; TPF3MP_HOOK_TOOL_COMPANY=0 turns it off)
tool-company: the street and track builder at 0x... acts as the player's company 372363 (was player 214443)
tool-company: the street and track builder at 0x... acts as the save's player 214443 again
```

In the first game test (2026-10-02, build aaa331c) the street tool split
the company's road mid-span; stations and stops were still refused before
capture (the construction and stop builders kept the save's player), and so
was bulldozing the company's road: the filter's player was written, but
the bulldozer's own player (`+0x28`), which its proposals are made for
(`Step` 0x4d687e, 0x4d46b0, 0x4d2b70, its lambda 0x4d2650), was not. Both
are written now. The second test (95127b2) showed the bulldozer's edge
test allowing the company's road with the company in its list and refusing
it with the save's player in its list, from the same frames: the menu's
step (`CMenuUI::DoStep`'s lambda through `sub_6a1410`) sets the bulldozer's
owner list (`+0xa8`, and the filter's copy) again to the GUI's player
(`[[game+0x1e0]+0x20c]`, the save's) through `sub_4d6220`, between the
tool's frames, and the click's query is built from that list. So the
setter is detoured too (`UI::Bulldozer set owner list`): the game's own
assignment, then the bulldozer's fields brought to the company at once. An
empty list (the setter's other case, which lets every owner through) is
left alone. A field the game set back and this wrote again is said once,
then at each power of two (`... was set back to player 214443; the company
372630 written again (N time(s) so far, all tools)`). Not covered: the town, terrain and other tools CGameUI
hands the player to (`sub_59b890`, `sub_5a2480` and the rest), which build
nothing a company owns.

**The views' player** (`crates/tpf3mp-hook/src/guiplayer.rs`), on unless
`TPF3MP_HOOK_GUI_COMPANY=0`. With the windows following the company (bd3c695),
the map still showed the first company's station icons and lines and not
the player's company's: those are drawn natively, and each decides "the
player's own" by asking the GUI's `IGameStateProvider` for the `GameState`
and reading its player (`+0x20c`) inline. There is no shared helper to
hook, and that `GameState` is one of the simulation's two buffers (the
probe), so its player is never written. Each read in a UI function is
spliced instead (`Splice`), right after it: in a room, the register that
holds the save's player gets the company the GUI notes
(`note("tpf3mp.company")`). Two reads compare the player with an owner at
once (`mov eax,[player]; cmp [reg],eax`); those splices take the read and
the compare, and point the owner's pointer at a copy of the save's player
where the owner is the company, and at no one's where it is the save's
player, the register being dead after on both paths. The sites:

| what it draws or picks | function | splices |
|---|---|---|
| the icons above the map's stations | `UI::HudIconManager::PreemptiveOctreeTraversal` (`0x67b610`) | `0x67b7db` |
| the station viewer | `UI::StationViewer::vf4` (`0x83a570`) | `0x83a608` |
| what the selector picks, and its filter | `sub_839c50` (from `UI::CSelector`), `UI::ViewCreator::vf1` (from `CreateSelectorFilter`) | `0x839cd9`, `0x86712a` |
| the catchment overlay | `UI::layers::CatchmentAreaHelper` (`sub_8764c0`, `sub_8779c0`) | `0x8765f5`, `0x876f36`, `0x8770ba`, `0x8770fd`, `0x877b39` (owner test) |
| the map layers' colours: lines and stations | `LayerManagerColorMap` (`sub_87b7f0`), `UI::layers::LayerManager` (`0x883020`, `sub_885b10`) | `0x87b840`, `0x87b919`, `0x87b9ef`, `0x88307b`, `0x885c82` (owner test) |
| two React components | `RendererComponentDelegate` (`sub_29f66d0`), `RailroadCrossingComp` (`sub_289e060`) | `0x29f689a`, `0x289e116` |

Each splice takes whole instructions with no branch, call or RIP-relative
operand, and no jump of the function lands inside it (tpfre). The
profile's targets give each with a unique signature, checked again before
it is spliced. What a site answers is read from two atomics the menu's
frame refreshes from the notes, so a site on a worker thread (the HUD's
octree traversal) reads no lock. Outside a room, for the room's first
company, or while either note is missing, every read answers as the
game's. Not covered: the scripting bindings that read the same player
(`sub_24d6f40` and the others under `0x24f…`), which the game scripts call
too; the GUI's Lua answers those (`tpf3mp/follow.lua`). hook.log:

```
view-company: 16 of 16 of the views' player reads see the player's company in a room (TPF3MP_HOOK_GUI_COMPANY=0 turns it off)
view-company: the icons above the map's stations see the player's company 372631 (view: HudIconManager::PreemptiveOctreeTraversal/player)
```

**The map with every company** (2026-10-02, after eba8614). A room's map
shows every company's icons and lines, not only the player's own: the
game's own rule shows only the GUI player's, which was the save's player,
and after the views took the company it was the company's alone. Two of
the views' tests are widened, display only, with
`TPF3MP_HOOK_GUI_ALL_COMPANIES=0` to keep them to the player's own:

- the HUD's icon pass (`sub_674430`, a lambda of
  `HudIconManager::PreemptiveOctreeTraversal`), which skips an entity whose
  `PlayerOwned` owner is not the pass's player (`lea rdx,[rax+rcx*4]; test
  rdx,rdx; je; cmp [rdx],r12d; jne`), spliced at the `lea`: for an owner
  that is a company of the room, the pointer is formed at a copy of the
  pass's player, so the icon of every company's station, vehicle and line
  shows. An entity no one owns passes as the game's;
- the map layers' colour test (`sub_885b10`, `view: LayerManager
  colour/owner test`): a line or station of any company of the room takes
  its own colour (its `Color` component) as the player's do.

The room's companies are the GUI's note `tpf3mp.companies` (their player
entities, comma separated, `tpf3mp/follow.lua`, `noteCompanies`), read
with the company once a frame into atomics. Each entity keeps its own
colour: a line its own (each new line its own colour), a vehicle
the company's paint (`companies.paintVehicle`). INFERRED, not seen: that
the station icons' colour is per entity too; where they are one colour for
every company, a per-company tint needs the renderer's colour read found.
What a player may select, edit or plan stays their own company's: the
selector, the station viewer and the catchment overlay keep the company
alone.

**The GUI's Lua getPlayer, natively.** The map's line overlay is a React
`LineViewer` whose lines a Lua state lists (`params::LineViewer`,
`LineVisualization`); a GUI state whose api was made anew (the React roots
reload their interfaces, `ScriptComponentRoot::ReloadInterfaces`) kept the
game's getPlayer until a window asked an ownership test. So getPlayer's own
binding answers the company in a room, in every GUI Lua state: its closure
(`sub_24ed220`, registered as `getPlayer` by `SetupUtilInterface`) reads
the player of the `GameState` its state's getter gives and pushes it
(`call sub_2fbe300`, the Lua integer push); that call is redirected
(`view: getPlayer binding/push`). The closure's getter is a `std::function`
(`+0x38`); where its call (vtable slot 2) is one of the GUI's getters,
`CMenuUI::SwitchToGameUI`'s or `ScriptComponentRoot::ReloadInterfaces`'s
(both `mov rax,[...+0x1e0]`, the GUI's slot), the answer is the company; the
game scripts' states, whose getter reads the engine's buffers (`+0x1f0`),
keep the game's. The getter is told by its code's shape, checked at every
call. hook.log: `view-company: the GUI's Lua getPlayer answers the player's
company 372426 natively (view: getPlayer binding/push)`.

**The map's lines, probed.** After 55f81ed the player's own line was still
not drawn on the map. Every line the game draws over the map is a React
`LineViewer` (`gui/main/builtin.lua`; the native `UI::LineViewer`,
`game/ui/util/lineviewer.cpp`) handed `showLines`, LineVisualizations by
line entity, by a window: the line manager (its map mode hands it
`getLinesForPlayer(getPlayer())`, `manager_window.tl` 427), the line,
station, vehicle and town windows, the statistics. The native viewer has
no owner test (tpfre: its functions read only the `Line` component); it
draws what it is handed. So with `TPF3MP_PROBE_PLAYER=1` each GUI state
says what every `LineViewer` is handed and what
`lineSystem.getLinesForPlayer` answers, each line with its owner, once per
answer and 40 lines at most (the hook notes `tpf3mp.probe` for the GUI's
Lua, `follow.watchLines`); what is drawn never changes:

```
probe: getLinesForPlayer(372553) answers 1 line(s): 373300 (owned by 372553) (the HUD's state)
probe: a line viewer is handed 1 line(s) to draw: 373300 (owned by 372553) (the HUD's state)
```

A company's line in that list but not drawn is the viewer's path (the
line's own route through its stops); one missing from it is the list's.

The 3d91e83 try (p0, company #2, 372671): the company's line 338852 was
listed by `getLinesForPlayer(372671)` and handed to a viewer, owned by
372671, and still not drawn. The viewer draws from the simulation's
per-line data (`ecs::LineSystem::GetData`, `sub_ad2050`, its `line2data`),
and draws nothing unless that data's revision (`+0x18`) and its per-stop
segment count match the line's own (`sub_7f01d0`, 0x7f03e7 and 0x7f0444);
nothing in its call tree reads a player (tpfre: no `+0x20c` read, no
`PlayerOwned`, to depth 3 from `LineViewer::vf1`, `vf4` and `sub_7f53c0`).
So the probe also says, for every line a viewer is handed (the first
company's as well, to compare): each stop's station group, station and
terminal as the line names them, whether that station and terminal exist,
their owners, whether the line system lists the line at that terminal, and
the engine's own verdict (`lineSystem.getProblemLines`,
`util.line.getLineProblems`, `getDetailedLineProblems`; the values below
illustrative, not seen):

```
probe: line to draw: line 338852 owned by 372671; 3 stop(s); stop 1: group 373231 station 0 terminal 1, group of 2 station(s) owned by 372671, station 180929 owned by 372671 with 1 terminal(s), no terminal 1, not listed at the terminal; ...; line system problem 3
```

The 34a2edf try (p1, company #1, 372571): line 342589, its three stops'
groups, stations and terminals all there, all the company's, the line
listed at every terminal, was not drawn; a line made the same way while
playing for the room's first company (329152, the save's player 214443)
was. So drawing depends on the owner, but not in the viewer: tpfre finds
no player read and no `PlayerOwned` six calls deep from
`UI::LineViewer::vf1`, `vf4`, `sub_7f53c0` and `sub_7f01d0`. The viewer
draws a line only when the line system's data for it
(`LineSystem::GetData`) has the revision it expects (`[data+0x18]` against
`[state+0x80]`) and one segment list per stop (`sub_7f01d0`, 0x7f03e7 and
0x7f0444). With `TPF3MP_PROBE_PLAYER=1` that test is now spliced and said
per line, read only (`probe: LineViewer route data test`):

```
probe: the line viewer's route data for line 342589: revision 0 (it expects 7), 0 segment list(s) (it expects 3); it skips the line
```

(illustrative). A company's line skipped there and the first company's
drawn says the line system keeps route data for the save's player's lines
only, which is the simulation's own and is not changed; the next step is
then to find what fills `line2data` and its player.

**The line viewers draw every company's lines** (`view: LineViewer lines
of the player/call`, guiplayer.rs; found 2026-10-02 with the probes of
4415a50). A line viewer builds a line's whole route (`sub_7f10a0` with
stop filter -1) only for its candidate lines, and `sub_7f3ea0` (called by
`LineViewer::Update` and `UI::MetroViewer::vf4`) makes them the lines of
one player from the line system's player-to-lines index (`sub_ad2620`,
the index `getLinesForPlayer` reads), that player being the one the viewer
stored when it was made (`[viewer+0x28]`, from the GUI player read of
`sub_29f66d0`, `RendererComponentDelegate`). So only the save's player's
lines were drawn while the GUI was the room's first company, and none
otherwise (seen: a founded company's line and the first company's both
got only stop filter 0 geometry, and only the first company's got -1, and
only while the player played for it). That one call is redirected: in a
room it answers the lines of every company the GUI notes
(`tpf3mp.companies`), or with `TPF3MP_HOOK_GUI_ALL_COMPANIES=0` the
player's company's, in a vector the hook keeps until the next call (the
caller copies it at once). Outside a room it answers as the game. The
index function and its other callers, the simulation's among them, are
untouched: the change is to which lines a viewer draws. hook.log, once:
`view-company: the line viewers draw the lines of 214443, 372609 (2
line(s)) in place of player 372609's (view: LineViewer lines of the
player/call)`.

**The store's depot follows the company** (`view: findBestDepot depot owner
test`, `view: findBestDepot owner test`, guiplayer.rs; 2026-10-02, build
45b8ed5: the line window bought company #2's vehicles at depot 317114,
owned by 214443, the first company's). Opened from a line, the store asks
`api.engine.util.vehicle.findBestDepotForLine` (`line_util.tl`), and the
line manager and the store `findBestLineAndDepotForVehicle`. Both reach
`sub_2689fd0` and through it `sub_2689dd0`, which keep only what the
`GameState`'s player owns: `mov reg,[GameState+0x20c]; cmp [rax],reg; jne`,
rax the depot's (or line's) `PlayerOwned`. Nothing but those two bindings
calls the four functions on the way (tpfre), and only the game's GUI scripts
call the bindings on build 40408. Each test is spliced as the other owner
tests are: an owner that is the player's company passes, the save's
player's does not. Because a game script could call the bindings too,
these two answer the company only on the GUI's thread (the menu's frame's,
noted at each refresh) outside the simulation's step; anywhere else they
answer as the game.

**A purchase's depot**, in hook.log when the store buys (the GUI's
capture, `capture.depotText`): `the store buys at depot entity 5001 (owned
by 372426): depot 0 of ::/depots/road/road_depot/road_depot.con at (1360.7,
-8829.4, 7.2)`, or why the room cannot name it. Opened from a line, the
store asks the engine for the line's depot
(`api.engine.util.vehicle.findBestDepotForLine`, `line_util.tl`), which
takes no player from Lua; the line says which depot it chose and whose.

Not per company, as the game has no way to ask for another company's:
`api.engine.util.headquarters.getTransportedData()` and
`getCompaniesValue()` take no company and answer for the engine's local
player, the room's first company; the windows that show them (the game
bar's transported figures, the finance window's company value) show that
company's for everyone.

### The build tools

The street, track and construction tools are native: a click queues a
`WorldBuildProposal` command, which the simulation applies at its next
step, in that game alone. Nothing a script does stops one on build 40408:
the street tool sends no `builder.proposalPrepareForApply` (a click is
`proposalCreate` twice, then the simulation's `onPreBuildProposal` and
`onPostBuildProposal`, then the GUI's `proposalApply`), an error raised in
`onPreBuildProposal` is logged and the build goes on, and emptying the
proposal's lists there crashes the game (found with
`tools/probe/tf3/tpf3mp_buildprobe_1`). So the hook stops the player's
builds natively (`crates/tpf3mp-hook/src/builds.rs`, profile targets
`CommandList::Add` and `WorldBuildProposal apply`):

- **At the click**, `CommandList::Add`, on the main thread: a command whose
  payload is a `WorldBuildProposal` (its variant index, at payload +
  0x9b8, is 52) with `playerInitiated` set (payload + 0x3d2) is counted, in
  the room's game only, and added as the game would.
- **At the apply**, the simulation's apply of a `WorldBuildProposal` (the
  command dispatcher's case 53, `0x9e1160`): a player-initiated build in
  the room's game answers false, as a build the game refused, and the game
  tells the tool so through its own path. The room's own builds go
  through: the mod's game script applies them with
  `tpf3mp_native.replaying(true)`. Towns' growth and the game's scripts,
  not player-initiated, go through as ever.

The mod's game script, in the GUI (`guiHandleEvent`), keeps the action each
proposal of a tool the room carries makes
(`mod/tpf3mp_1/content/scripts/tpf3mp/capture.lua`), marked with the clicks
counted when it saw it (`tpf3mp_native.clicks()`). Its `guiUpdate` hands
the room the one each click saw last: the proposal and the click both run
on the main thread, in order, so that is the proposal clicked. The room
orders it for every game, this one included, and each game's `postUpdate`
builds it, paid by the player (`Context.player`) and clearing town
buildings in its way (`gatherBuildings`), as the tool builds; without a
context the game builds for free.

Six tools build through the room so far, the module editor and the
terrain tools through the hook ("Terraforming" below, gated off), and a
construction's window its edits:

- **The construction tool** (`constructionBuilder`): a proposal of one
  construction (a station, a depot, anything the tool places) becomes a
  `BuildConstruction`, with the file, the transform, the parameters
  flattened and the game's name for it. The streets in its proposal are
  what the tool built around the construction, and travel with it as its
  connection: a bus station placed by a road rebuilds the road through a
  new junction and adds an entrance edge from the junction to the
  station's own street node (seen on build 40408, the construction's
  frozen nodes and edges empty in the proposal). Built without them, the
  station stood beside the road, its entrance a dead end, and the line
  manager could not connect it ("Could Not Connect Stations"). The
  entrance edge in the tool's proposal is the construction's own, snapped
  onto the road: every station and depot has one frozen node and one
  frozen edge, its entrance, and a scripted build makes that edge again
  unsnapped, ending about 2 m short of the road (build 40408, read from
  the console). The game's refresh of the construction
  (`api.engine.util.proposal.refreshConstruction`) snaps it as the tool
  does: its proposal's entrance edge ends at the road's node. So the
  replay builds, in one action, the construction with the rest of the
  connection (the road rebuilt through the junction), then the refresh of
  the new construction, free (no context) and not as a click of the
  player's; the refresh finds the junction the tool chose. The town
  buildings in its way the replay clears again (`gatherBuildings`, and
  `gatherFields` for fields). Several
  constructions at once, or one replacing more than one construction that
  is not a town building, is refused. Before sending, the replay asks the
  game's verdict (`makeProposalData`) and refuses what it calls critical,
  with its reasons.

  A proposal that replaces one construction of the player's with a new
  one (an edit of its modules or parameters, an upgrade) becomes a
  `BuildConstruction` with `replaces`: the old construction by its file and
  where it stands (a `ConstructionRef`, as a depot is named; entity ids are
  no name, docs/BUILDING.md), the new one's file, transform, parameters and
  name (the old one's, where the proposal leaves it out). Its street part
  is mostly the construction's own entrance, made again with it, and that
  is not carried: what it removes of the old construction's own (its
  `frozenEdges`, `frozenNodes`, and track ends only its frozen edges
  touch) goes with the old construction. What it changes around it travels
  as its connection, as a new construction's does, without those: a new
  exit onto a road the station did not join splits that road through a new
  junction (seen 2026-10-03, build 40408). One replacing a construction the
  room cannot name is refused. Every game finds the old
  construction by file and place (within 2 m), then asks the game's
  verdict and builds, as the player's own build (`ignoreErrors`,
  `playerInitiated`), paid by the player and clearing town buildings in
  its way, one `SimpleProposal` that removes it (`constructionsToRemove`,
  this game's own entity), builds the connection as a new construction's
  (its own entrances peeled off) and adds the new one, mapped old to new
  (`old2new = { [old] = 0 }`), as the game's own upgrade makes one
  (`mission_framework_util_entity.tl`, `upgradeConstruction`). The new
  construction makes its entrances again unsnapped, as a scripted build
  does, so every game then refreshes it as it refreshes a build's, free
  and not as a click of the player's: the refresh snaps its entrances onto
  the streets beside them (a road station edited by the street came loose
  from it in both games, 2026-10-03). A refresh with no street change is
  not sent; one the game refuses leaves the edit standing, unsnapped, the
  same in every game, and is logged. A connection may not remove or split the old construction's
  own edges, and no junction's settings may name them: a junction the
  connection rebuilds next to the old entrance keeps no settings, which the
  construction and its refresh give it, the game's own (logged `left to
  the construction: the settings of N junction(s) at its old edges`),
  only where the acting company may change every edge at it.
  Every edge a construction's connection removes or splits, a new one's or
  an edit's, must be the acting company's or no company's (D21), as a
  bulldozed one. Seen in two games on 2026-10-03: a plain edit's refresh
  (`snapping 72194 +e-2:-1>57114 -e71473`), and an edit adding an exit
  onto another road, replayed with the road split and both entrances
  snapped alike in both games (docs/BUILDING.md). The new
  construction stands where the old one stood, so the next edit, a depot
  bought at it or a line finds it by the same reference; what stood on it
  passes to it through `old2new`, and the registry binds, after the
  action as after every other, any station group the game made anew, the
  same in every game. An old construction not there fails the action in
  every game, and nothing is sent. INFERRED, not yet seen in the game:
  that an edit's proposal names the old construction in `toRemove` and
  the new one in `toAdd` (TPF2's shape), that its street part removes
  only the construction's own entrance, and that `old2new` keeps its
  stations' station groups.

  Where edits come from on build 40408 (read from the binary and the
  game's Lua, not yet seen in the game):
  - the game tells game scripts of the proposals of six tools only, under
    the ids `UI::CGameUI`'s constructor names them by:
    `constructionBuilder`, `streetTerminalBuilder`, `streetBuilder`,
    `trackBuilder`, `streetTrackModifier` (the road and track modifiers,
    "The road and track modifiers" below) and `bulldozer`. The **module editor**
    (`UI::ModuleBuilder`, opened from a station's window) is not among
    them: it queues its `WorldBuildProposal` itself and game scripts hear
    nothing of it (its click was stopped with "no proposal seen" in the
    game, 2026-09-30). So the hook reads it natively ("The module editor"
    below), and the GUI takes that for the click. The mod's `CAPTURE` also
    names `moduleBuilder` and `moduleBulldozer` (the construction menu's
    names for those tools, `ConstructionActionParam`) in case a later
    build sends their proposals; INFERRED;
  - a construction's parameters changed in the construction menu, and the
    cargo buttons of a station's window, send the game's replacement
    proposal from Lua (`api.engine.util.proposal
    .createProposalReplaceConstruction`, then `makeWorldBuildProposalCmd(proposal,
    nil, false, true)`, `gui/construction/construction.tl`,
    `gui/entity_window/entity_window_util.tl`). The guard carries such a
    command as the same edit (`guard.CARRY.makeWorldBuildProposalCmd`,
    `capture.windowBuild`). A window's build that only rebuilds edges in
    place (no construction, no node added or removed, every new edge
    between the ends of one it replaces), as the bridge and tunnel window's
    type change makes one (`gui/entity_window/bridge_and_tunnel.tl`,
    `createBridgeOrTunnelProposal`), is read as the road and track
    modifiers' rebuild is (`capture.inPlace`, `capture.modify`) and travels
    as a `BuildRoad` or `BuildTrack`, its edges' bridge or tunnel type
    included; a stop or signal on those edges must be kept in place, as
    for the modifiers. With `acceptance.lua`'s `bridges` off it is
    refused, saying it awaits two-player game acceptance: it travels as an
    ordinary build, so the gate is at the capture. `bridges` is on since a
    two-player game on build 40420 (2026-10-11) carried track and road
    bridges, a road tunnel and a road bridge with a stop this way
    ([record](../investigation/TPF3_BRIDGE_TUNNEL_WINDOW_2026-10-11.md)).
    Any other build a window sends stays refused
    ("building from this window");
  - a bulldozer proposal that removes a construction of the player's and
    adds one (a module removed, if the module bulldozer reaches game
    scripts as the bulldozer) is carried as the same edit; INFERRED.

  In the room's game `guiHandleEvent` also logs each event it does not
  handle by id and name, once each and 40 at most (`an event the mod does
  not handle: id …, name …`), so a test in the game shows what a tool
  that builds "with no proposal seen" sends, if anything.
- **The module editor** (`UI::ModuleBuilder`), read natively
  (`crates/tpf3mp-hook/src/modules.rs`). It queues its build with its own
  call of `CommandList::Add` (profile target `ModuleBuilder::MousePressed/Add
  call`, 0x543b25 in `UI::ModuleBuilder::MousePressed`; `Add` returns to
  0x543b2a; the factory before it sets `playerInitiated`). The add's
  detour is entered through a thunk that notes where `Add` returns to; a
  player's build counted there whose call returns into the module editor
  has its `Proposal` read before `Add` runs: `toRemove`, the first
  `toAdd` entity's file (`ResName`, printed `mod::/path`), parameters (an
  abseil b-tree of variants, walked whole), matrix and name, and the
  street part's removed nodes and segments by entity (the layouts of
  `feat/capture-all`'s `conscap.rs` and
  `investigation/TPF3_CONSTRUCTION_CAPTURE_2026-09-29.md` there, re-checked
  with `tools/tpfre` on build 40408). Every read is checked readable,
  every count and text capped, every table walked to exactly its size. It
  is kept, as a Lua table of the shape game scripts see a proposal in,
  under the click count before it (the 16 newest kept), and logged
  (`module editor: click N queued …`, or `module editor: click N does not
  read: …`). The GUI's `guiUpdate`, handing on click N, asks
  `tpf3mp_native.built(N)` first, ahead of any preview another tool
  showed: the proposal is made an action by `capture.moduleEdit`, as
  the construction tool's, and must replace a construction (else
  refused: `an edit that replaces no construction`); one that did not
  read is refused with why. The hook reads only how many nodes and edges
  the street part adds, so an edit that changes the streets around its
  construction (a new exit splitting a road) is asked of the game again:
  `api.engine.util.proposal.createProposalReplaceConstruction(old,
  params)` with the editor's parameters, as the construction menu asks
  for a construction's new parameters (`gui/construction/construction.tl`),
  proposed the editor's street part exactly in the game (2026-10-03, build
  40408: the same three nodes and four edges added, the same node and two
  edges removed). Its street part travels only if it is the same edit as
  far as the hook read it: the same construction replaced by the same
  file, standing where the editor put it (within 0.01), as many nodes and
  edges added, the same nodes and edges removed (none twice), and no stop
  or signal on either side; else it is refused with why (`a construction
  edit the game proposes otherwise: …`). The construction itself (file,
  parameters, matrix, name) is the one the hook read. The click's apply is stopped as every
  player's build is, and the room orders the edit for every game, which
  replaces the construction as above. `built` is optional in the bridge:
  a mod or hook without it keeps the module editor refused. Without the
  profile target the hook logs so at install and the module editor stays
  refused. INFERRED, not yet seen in the game: every layout read (static
  only), that the proposal's parameters are the ones the replay needs
  (`seed` among them), and that the module bulldozer (a
  `UI::Bulldozer` action) is not this call.
- **The street and track tools** (`streetBuilder`, `trackBuilder`): the
  proposal becomes a `BuildRoad` or `BuildTrack`, as the tool made it, by
  positions (docs/BUILDING.md, "The action schema"): the nodes and edges it
  adds, each edge in its own kind (the street it joins is rebuilt through
  the new junction in that street's template), and the edges and nodes it
  removes. The replay builds it as the game's own scripted track builder
  does, `nodesToRemove` included. Each new track edge gets its template's
  distance between track centres (`StreetTemplate.trackDistance`, the
  edge's `distance`): without it the game laid no shared ballast bed or
  catenary with the tracks beside it, and the ground showed between them
  (seen 2026-10-02, tracks laid side by side in a room). Each edge ends
  where the tool ended it (`BaseEdge.position0`, `position1`), which need
  not be its node: a track laid onto the free end of a street with tram
  tracks starts on the street's tram lane, 2 m beside the node, and only
  so does the game join them for trams (build 40420; docs/BUILDING.md,
  "Edge ends"). Built from the node's centre, the track ended in a buffer
  stop in every game of the room (2026-10-15); the schema-27 edge ends fix
  passed two-game acceptance on build 40420. A build that moves or removes
  an edge with a stop or signal on it, or that places stops, signals or
  constructions, is refused, except a track cut between its signals where
  every existing non-waypoint signal is declared and kept on its continuing
  piece (`Link::kept`, BUILDING.md; `splitSignals`), and a signal the tool
  puts back where it stood on a track it redraws around a node it moves,
  kept as the old signal on that piece (`refitSignals`).
- **The bulldozer** (`bulldozer`): its proposal removes one construction
  (with the construction's own entrance edge and node) or edges of one
  network (with the nodes they leave on their own). It becomes a
  `Bulldoze`: the construction by its file and position, or the edges by
  their ends. The replay removes them as the game makes such a removal
  itself, `createProposalRemove` for the construction and
  `makeSegmentsRemoveProposal` for the edges (on build 40408 the first
  gave exactly the bulldozer's proposal), and the player's company pays.
  A street's branch removed, or a track's stub ending on a street (a
  level crossing), takes the junction or crossing node with it, and the
  bulldozer joins the two street edges left there into one new edge
  between their far ends (seen in a room on build 40420, 2026-10-16; until
  then refused as "a stop build that adds or removes nodes"). The game's
  own `makeSegmentsRemoveProposal` of the stub alone joins nothing, and
  the game refused that removal ("Construction Not Possible"). So such a
  proposal, every edge it adds such a join, travels as the build it is: a
  `BuildRoad` or `BuildTrack` of the joining edge between the existing
  far nodes, with the removed edges (each in its own network) and nodes
  as its removals, as a road modifier's build travels. Capture accounts for
  every removed edge: it must pair with a join or be the known track stub
  attached to the joined node, with its removed far node a degree-one track
  endpoint. An unrelated extra removal is refused. A stop or signal on a
  joined edge, or town buildings the bulldozer takes with it, are refused.
  A town building is a construction like any (one that lists its
  `townBuildings`) and goes the same way. A town street's proposal lists
  the town buildings along it in `toRemove` too; they travel beside the
  edges (`Bulldoze::Edges::buildings`, by file and position), and every
  game checks that its own `makeSegmentsRemoveProposal`, which gathers
  them through the same `street_util::FinishProposal`, removes exactly
  those, else refuses the bulldoze. The replay is the player's own build
  (`playerInitiated`), so the game's towns script charges the town's
  reputation from it in every game alike (`onPreBuildProposal`,
  `town_util.getProposalStats`). The asset bulldozer's proposal (trees
  and other assets: the asset group removed and rebuilt without them as
  a construction of no file) and anything else that is no construction
  are refused, naming what was hit (an asset group, or the components
  the entity has). The log line of such a refusal also says what the
  rebuilt group holds (its desc's type, its models, the thin ones, and
  the first one's model and place) and what the group removed holds
  (full and thin instances): what a replay of it would have to build.
  On build 13090a8 the rebuilt group read as plain models: each a model's
  file (`::/assets/...`) and its world matrix, none thin, one construction
  of no file and no desc type, for a group of thin instances. Unless
  `TPF3MP_TREE_BULLDOZE=0` is set in the player's game (the hook's
  `trees()`), that proposal travels as a `Bulldoze::Assets`: the group by its first
  asset and how many it holds, the assets taken out by model and position,
  and which way the tool turned them (read off its matrices; a rebuilt
  group that holds any asset the group did not, turns a thin one
  otherwise or moves a full one, or a removed asset with another of its
  model at its place, is refused). The tool's own shape, decompiled on
  build 40408 (`UI::AssetBulldozerAction`,
  `construction_builder_util::CreateProposalAddAsset`): the group in
  `toRemove` and, unless every asset of it went, one
  `Proposal.ConstructionEntity` at the origin whose desc is
  `autoRemovable` and whose one subconstruction's `models` are the assets
  kept, the thin instances first (matrix from position, turn and scale),
  then the full ones (their own matrix), none `thin`. When the last
  assets of a group go (a lone tree or rock), nothing is added. TF3 binds
  these types otherwise than TF2 and its own tealdef say:
  `Proposal.ConstructionEntity` has `desc`, `construction`, `transf`,
  `frozenNodes`, `segmentsBefore`, `name`, `playerEntity` and
  `setAsHeadquarterHack` as data, and `fileName`, `params` and
  `hasCargoPlatform` read-only (writing `fileName` raises "no writable
  member"); its `construction` is a `Proposal.ConstructionResult`
  (`subconstructions`, `metadata`, `cost`, `maintenanceCost`,
  `maintenanceCarrier`, `streetTerminal`, `params`), not the
  `Construction` component; a `Proposal.Subconstruction` has `models`,
  `station`, `depot`, `industry`, `metadata`, `laneLists` and
  `colliders`; a `Proposal.TransformedModel` `id`, `tag`, `transf` and
  `thin` (the binding's registration, `RegisterUsertypesTransport`). The
  tool also gives its subconstruction one empty terrain alignment list,
  which no binding reaches; it aligns no terrain. Every game finds the
  one group of that size holding them all, builds the same full proposal
  (`api.type.Proposal`) from its own copy less those assets, sends it as
  the player's company's build, and logs the group, its assets before,
  and the group holding the first asset kept (or, with none kept, the
  first removed) after (`trees:` lines). The flag keeps it to trials. A
  stop it
  removes is carried as the stop tool's builds are (below): its edge
  rebuilt without it, the stop named by its edge, where it stands and its
  construction (the `EDGE_OBJECT` component's `transf` and
  `edgeObjectConstruction`), a `Bulldoze::EdgeObject`; the replay puts it
  in `edgeObjectsToRemove`. Both stops of a two-sided stop, which one
  click removes, go as one `Bulldoze::EdgeObjects` (schema 27): two stops
  of one construction, one on each side, else refused. A track removed
  with signals on it goes as `Bulldoze::Edges` when the proposal's
  `edgeObjectsToRemove` are exactly the signals on its removed tracks
  (each type `SIGNAL`, listed once, no construction removed with them);
  the replay removes the tracks with those signals and refuses a stop or
  another company's signal on them. An edge with a stop on it is
  refused.
- **The stop tool** (`streetTerminalBuilder`): a stop on a street. The
  tool queues a `WorldBuildProposal` (command 52 from
  `UI::StreetTerminalBuilder`, found statically), which the hook's gate
  stops like the others. The tool waits for each click's answer before it
  takes another (build 40408, read with tpfre: `MousePressed` 0x594f50
  returns at once while its busy byte `+0x2c8` is set, sets it before it
  queues the click, and only the command's callback clears it, once the
  simulation applied the command; `Step` shows no preview meanwhile). In
  a room that answer is the refused apply, and the stop comes later from
  the room, so a row of stops clicked quickly lost every click after the
  first (2026-10-02). So in the room's game the add's detour finds the tool
  from the click's own callback before `Add` (a `std::function` on
  `MousePressed`'s stack whose impl pointer, `+0x38`, is itself, the tool
  at `+8`) and clears the byte once the click is queued, where it reads 1
  as `MousePressed` set it (`crates/tpf3mp-hook/src/stoptool.rs`, profile
  targets `StreetTerminalBuilder::MousePressed/Add call` and `/busy set`,
  whose signature holds the offset). Each click is then its own
  `PlaceStop`, handed on in click order; the late callback, with the
  refused build's empty result, clears the byte again. Without the
  targets, or on anything that does not read so, the tool waits as the
  game has it, and hook.log says why once (`stop tool: ...`). Its
  proposal has the shape the game's own mission scripts check a stop by (`checkStop`,
  `mission_task_build_construction_util.tl`): one edge removed and the same
  edge added again between the same nodes, whose `objects` list its stops
  as `{ entity, EdgeObjectType }`, as many as `edgeObjectsToAdd`. The new
  stop is the one entity the old edge did not list. It becomes a
  `PlaceStop`: the edge by its ends, the point of its centreline where the
  stop stands, the engine's `left`, the edge's direction there, and the
  stop's construction. Seen on build 40408 (a room, 2026-09-30): the
  proposal game scripts get has no model and no place on its edge objects
  (`+o{resultEntity=-1 category=0 left=false playerEntity=3869}`), and a
  stop is a construction (`stations/street/small_stops/small_new.con`,
  whose update script places `small_new.mdl` on the edge). So the GUI
  notes the construction the construction menu gives the tool (its
  `EdgeObjectBuilder.resName`, from `construction_react_util
  .getActionParams`, which `capture.watchStopTool` wraps in each GUI Lua
  state) through the hook (`tpf3mp_native.note`), and the capture reads it;
  a proposal whose edge object has a model
  (`api.res.modelRep.getName(modelInstance.modelId)`) names it itself. The
  place is the proposal's parameter or model position where it has one,
  else the point of the centreline nearest the ground under the cursor
  (`api.gui.mouse.getTerrainPosition`), which every game then uses. A
  two-sided stop is one click that adds an object on each side: a
  `PlaceStop` with `two_sided`, which every game builds on both sides in
  one proposal (objects `-1` and `-2`). Every game's
  `postUpdate` rebuilds the edge as the game's electrify task rebuilds one
  (`electrify.tl`: the edge's own component read afresh, entity -1), its
  other stops kept under their own entities, the new stop
  `edgeObjectsToAdd[1]` (edge -1, the parameter where it stands, `left`,
  the model, the player), named in the edge's objects as `{ -1, side }`,
  the lane configurations at the edge's ends replaced by the same turns,
  crosswalks and light phases naming the rebuilt edge
  (`junctions.renamed`: only references to the old edge change, the other
  edges keep their entities, nothing is searched for by position; one
  that no longer fits its lanes refuses the stop in every game). Removed
  alone, a junction with traffic lights kept its lights with no
  configuration, a fatal assert
  (`ecs::Engine::GetComponentDataIndex`, `BaseNodeConfig`) that crashed
  every game of a room on 2026-10-04 (build 40408). Then the game's
  verdict, and the build as the player's
  own (`ignoreErrors`, `playerInitiated`), paid by the player. The
  rebuilt edge keeps its own `PlayerOwned` (a company's road stays the
  company's). Once built, the stop is settled as the acting company's
  (2026-10-02: a company's stops came out another company's, and its
  player could not open them, nor see its station icon): each new
  object on the edge, which for a street stop is its station itself
  (`EDGE_OBJECT` and `STATION`, `mission/name_util.tl`), its station
  group (`stationGroupSystem.getStationGroup` of the object), and for a
  stop built as a construction that construction and its stations, are
  handed over with `makeEntitySetPlayerCmd` where anyone else owns them,
  as the game's own missions hand a stop over
  (`transfer_ownership_util.tl`); the windows, the icons and the line
  manager ask the station group's owner (`station_group.tl`). A group
  that also holds another stop's station is left as it is. hook.log says
  what each new object is and which group holds it (`the new <stop>: 600
  a station in group 610 (owner nil); ...`), and each one handed over
  with its owner before (`the new <stop> made the acting company's`). The
  stop is named as the game's tool named it. Build 40408's stop tool
  (`UI::StreetTerminalBuilder`, through `street_util::MakeEdgeObjectName`
  in `construction_util_terminal.cpp`, `sub_2647e60`) gives a new stop
  the name a station already there has, else the first street name of the
  town's name list (the `streetNamesScript` of `names/*.names.lua`) that
  no station has yet, else `Stop #n` with the first free n
  (`sub_25d5a80`); signals and waypoints get `{townName} Signal #n` and
  `Waypoint #n` (`sub_25d4540`). The choice reads the originator's world
  and runs the name script, so it is made once: the capture reads it
  from the proposal's edge object (`Proposal.EdgeObject.name`, which the
  game binds for scripts), `PlaceStop::name` carries it (schema 25), and
  every game builds the stop with it. A name longer than the schema's 64
  bytes is left out. Where none is carried, or with
  `apply.NATIVE_STOP_NAMES` false (the kill switch), the stop's edge
  objects carry `Stop` (never empty: a script build with an empty name
  leaves the stop with no `NAME` and no owner, docs/BUILDING.md), and once
  built its own group and stations are named after the town the game
  counts the stop in (`stationSystem.getTown`), with a number after it
  where another station group of that town has that name (`Didcot`,
  `Didcot 2`), the same in every game. A construction (a station, depot,
  airport, harbour or truck station) is named natively the same way at
  the originator, by `UI::ConstructionBuilder`'s
  `CreateProposalAddConstruction` (`sub_a34a50`: `{townName}
  {constructionName}`, or for a station `sub_25d4800`: `{townName}
  Station`, a direction from the town centre, or one of nine suffixes,
  `Annex` to `Upper`, in an order shuffled by a hash of the position, no
  RNG, then `{stationName} #{number}`), and `BuildConstruction::name`
  already carries that name to every game.
  A construction the room builds (a station, an airport, a harbour) names
  its own stations' group by the name the tool gave the construction
  where the game left it unnamed (`named station group N "..."` in
  hook.log). A receiver
  whose edge runs the other way flips `left`; a side already taken is
  refused (two stops on one side is a fatal assert in the game's lane
  creation on TPF2). Refused: a stop dropped where one stood (the game
  moves its lines to the new one, which a replay cannot say), more than
  two new objects, or two on one side, more than one signal or waypoint at
  once, a stop whose engine side (`STOP_LEFT`, `STOP_RIGHT`) is not what
  its `left` says, and a stop whose construction no GUI state noted.
  A signal or waypoint carries the settings the tool builds it with
  (`PlaceStop::params`, schema 26): `construction_react_util.getActionParams`
  gives the tool `EdgeObjectBuilder.params` (the construction's own keys,
  values as the tool's controls hold them: `oneWay`, and a mod's, Auto
  Signals' `auto_signals_distance`, `_replace`, `_direction`, Signal
  Distance's `signal_distance.local`: a key may hold a dot, and is one
  key, never a path as a construction's parameters' are), and
  `capture.watchStopTool` notes them with the tool's construction
  (`stop-tool-params`: `1`, the construction, how many, each
  `key=<i|f|b>value`, tab-separated, keys sorted). A note is at most 512
  bytes and a longer one is cut short, so one that would not fit, settings
  that are not plain numbers or booleans, a note another construction's or
  one whose count does not match refuse the signal (`a signal whose
  settings the room cannot read: ...`), never build it with the
  construction's defaults. Every game sets them on the signal it builds
  (`SimpleStreetProposal.EdgeObject.params`: not in the game's API
  reference, but a member on build 40408, and kept on the signal it
  builds, seen 2026-10-06 in a single-player game: Auto Signals then
  spaced signals from it). `year`, which the game's own tool adds, is not
  among them: no base signal construction reads it.
  Seen on build 40408 (2026-10-06): on a track with a signal on it
  already, the signal tool's `edgeObjectsToAdd` lists the new signal
  alone, not the kept ones; the capture pairs the records with every
  object of the edge where there is one for each, else with the new ones
  where there is one for each of those, and refuses any other count (`a
  stop build whose objects it cannot pair`).
  INFERRED, not yet seen in the game: that the stop tool's proposal lists
  `objects` in the order of `edgeObjectsToAdd`, that a kept stop keeps its
  entity there, that `STOP_LEFT` goes with `left`, that a script proposal
  names a new object `-1` in the edge's objects (TPF2's convention), that
  the model name `modelRep` gives is the one `EdgeObject.model` takes, and
  that the tool's click is `playerInitiated`.

**The street and track tools' continuation is test-only**
(`TPF3MP_TOOL_CONTINUE=1`) until an ordinary-start real-game check covers
input cancellation and native track direction. With the default setting,
the game leaves the tool at its normal start-point picker after the room
builds a segment. With the opt-in, the tools continue from that segment,
as they do from the one just built in single player
(`crates/tpf3mp-hook/src/toolchain.rs`, `scripts/tpf3mp/toolchain.lua`).
Read on 40420 with tpfre (the same code, at the same places, on 40408 and
40418): one click of either tool, `UI::StreetBuilder::UpdateEngine`
(0x588c60), sets the tool's busy byte (`+0x9e0`), queues the build with a
callback (`UpdateEngine::<lambda_1>`, inline in its `std::function`: the
tool at `+8`, the start point the next segment takes at `+0x10`, the
clicked end at `+0x1c`, and two settings bytes at `+0x28` and `+0x29`),
then resets the tool's proposal, start point and all (`ResetProposal`,
0x575650). The callback (0x56d7f0) clears the busy byte, and continues
only when the command's result (`+0x30`, the apply's bool, 0x9e3473) is
set: it finds, in the payload's vector of built edges (`+0x18`, 0x350
bytes each: the edge, its two nodes, both ends and both tangents, real ids
once applied: 0x25fc980, 0x25fe200), the edge whose end equals the clicked
end, and calls the tool's "start here" (0x570a30) with a
`SnapPointBaseEdge` there and, when the end node has exactly one edge of
the tool's network, the edge's direction. In a room the click's build is
refused at its apply, so the result is unset and the player picked a start
point after every segment (reported 2026-10-15).

The hook does the callback's part later, from what the room built, and
never hands the callback a result of its own:
1. At the click, the add's detour reads the callback as `UpdateEngine`
   built it (busy byte 1, functor inline) and keeps that one click; a
   later click of the street or track tool replaces it. A script's build
   (a mod's follow-up, `playerInitiated` too) leaves it, and picking
   another tool closes this one.
2. The mod's `guiUpdate` tells the hook the ticket it handed the click to
   the room with (`chainClick`), or that it handed nothing over.
3. While the room's actions are applied, each build's edges are copied out
   of its payload after the apply, and kept once that ticket's action is
   reported applied (`applied`); a refused or failed one drops the click.
4. Each GUI frame (`chainPoll`), the built edge that ends within 5 mm of
   the clicked end (the room carries millimetres) is checked in the GUI's
   own state: that edge, with those nodes, its end where the room put it,
   which also shows that state has the room's build. Its end node's
   street and other edges are counted (`getNodeSegments`, `chainSeen`).
5. At the tool's next frame (the `UI::StreetBuilder::Step` pre-hook of
   the tools' player), if the tool is not busy, has no start point of the
   player's, keeps its settings and their continue byte, the edge exists in
   the tool's engine (`ecs::Engine::EntityExists`, which `StartAt`'s
   revision read asserts) and the GUI saw it a frame ago at most, the hook
   calls the tool's "start here" with the start point and direction the
   callback would have built.

The click is dropped, and the tool left as the game left it, when the
player starts a segment (even one let go again before the room built),
the native builder resets its proposal after the click was captured,
the tool misses frames (closed), the room
refuses or does not apply the build or applies it without such an edge,
the game leaves the room, or 15 s pass. The direction rule is the
game's, counted in the GUI's state as the game counts it, with one
difference: the game counts the other network's edges that also have
`BaseEdge`'s field `+0x90` clear, the hook all of them. (Whether the
click's end snapped to something does not matter: a track snapped to a
street beside it, as a guide, still goes on in its direction.)
A right-click while the click waits passes through the tool's own input
handler with no start point (`UI::StreetBuilder` vf2, 0x581479..0x5814d7).
The native proposal-reset detour now drops a kept click if that path resets
the builder; continuation remains off even with test opt-in if this detour
fails to install. Closing the tool also drops it after missed frames. Whether
the parent handler can leave the tool open without resetting remains to be
checked in game, so default continuation stays off. "Closed" is three GUI frames without the
tool's frame, counted from the click's own; a tool closed and opened
again faster than that is not seen.
Profile targets, all optional: `StreetBuilder::UpdateEngine/Add call`,
`/capture` (whose signature holds every offset of the tool and the
callback read), `StreetBuilder::UpdateEngine callback/result scan` and
`/snap` (the result's and the start point's layout),
`UI::StreetBuilder::StartAt` and `ecs::Engine::EntityExists`; install
checks each call between them. Without any of them, or without the
`Step` pre-hook (`TPF3MP_HOOK_TOOL_COMPANY=0`), the tools wait for a
start point after each segment, and hook.log says why once
(`street tool: ...`).

A refusal shows its reason in the tool, and the log has each new reason
with the proposal's shape (`the room cannot carry this ... build`); every
build handed to the room is logged with its shape too. The shape
(`engine.describe`) names each edge's stops and signals as its component
lists them (`[entity:EdgeObjectType]`), each new edge object with what of
its record reads (`+o{...}`, and its binding's member names where its
fields do not), and each edge object removed with what its `EDGE_OBJECT`
says (`-o<entity>{param edgeObjectConstruction at ...}`). The upgrade, bus
lane and tram track tools stay refused until their builds are captured. Where the profile lacks the
two targets, `clicks()` is nil and every tool stays refused.

Seen on build 40408, through the deployed server with two games on one PC:
a maintenance building placed with the construction tool in the guest's
game was stopped there, handed to the room, and built in both games in the
same update; both accounts paid its $180,336, and the room found no
divergence. The same for a street across open ground ($71,820), a street
onto an existing junction ($94,231), a street onto another's middle, a
track across open ground ($22,054), a track across a street, and a bus
depot snapped onto a town street, clearing three town buildings
($825,816): identical in both games, towns included.

### Scripts' follow-up builds

*Proposed (D27).* A mod's game script that builds after the player builds
(Parallel Tracks, Parallel Roads) hears the build in `onPostBuildProposal`,
in every game, and sends its own build from its GUI half (`guiUpdate`)
with `api.cmd.makeWorldBuildProposalCmd`. Every game that runs the mod
sends it, from its own player's settings, for whichever player built. The
hook counts each as a click and stops it at the apply, so none of them
builds (seen on build 40408, 2026-10-02: Parallel Tracks' tracks for one
player's track, sent and stopped in both games, `[parallel_tracks] ...
build failed`, the worlds equal).

The mod's game script wraps `makeWorldBuildProposalCmd` in the game
scripts' GUI state, from its first `guiUpdate`
(`mod/tpf3mp_1/content/scripts/tpf3mp/modbuild.lua`). hook.log: `scripts'
builds from the game scripts' GUI state go to the room as their player's
follow-ups`. In the room's game:

- every build made through it is marked `playerInitiated`, whatever the
  script asked (`makeWorldBuildProposalCmd`'s fourth argument): the hook
  counts it and stops it at the apply. One the script marked `false` would
  otherwise build in this game alone, as the hook lets builds that are not
  player-initiated through. Where `clicks()` is nil the factory raises
  instead (`Not in multiplayer yet: building from a script`);
- the game script's `postUpdate` notes, after the room's builds of an
  update, whether the last of them was this player's own
  (`tpf3mp_native.note("tpf3mp.lastbuild", "<n> mine|other")`, the sender
  against `status().me_id`). A script's build within `FOLLOW_FRAMES` (120)
  of the GUI's frames after this player's build is its follow-up: its
  SimpleProposal is read in the shape the build tools hand game scripts
  (`nodesToAdd`, `edgesToAdd` as `addedNodes`, `addedSegments`), made the
  street or track tool's action (`engine.captureBuild`) and kept for the
  click its command counts, which `guiUpdate` hands the room as a tool's
  (`handed the player's build to the room [a script's follow-up build from
  <mod>]`);
- a follow-up that rebuilds existing tracks in place with signals added
  or removed, and nothing else (Auto Signals after its player's signal,
  `modbuild.isSignals`), is read by `engine.placeSignals` instead: each
  added segment pairs with the one removed edge between the same nodes,
  its tangents, type, template, owner, style, lock, decorations and lane
  configurations unchanged; a mixed signal-and-track modification is
  refused rather than replayed as signals alone. Each object it keeps is the
  removed edge's own, each it drops is in `edgeObjectsToRemove` and a
  signal, and each new one a signal whose record is the
  `edgeObjectsToAdd` entry its entity names (-400000000, then down,
  counted across all the edges) on its own edge; all new signals of one
  construction, one-way and settings. It becomes one `PlaceSignals`
  (`[a script's signals]`): each track by its ends (node 0 first), each
  new signal's place along it and side, each removed one's place and
  construction. Any other shape refuses it with why. Every game finds
  every track first, by one node within 0.5 m of each end in three
  dimensions and one track between them, and each removed signal as the
  one of its construction within 0.25 m of its place, all one for one;
  anything missing or found twice refuses the whole build. It rebuilds all
  the tracks in one proposal, a place read as `1 - place` and the side
  flipped where its track runs the other way, the lane configurations at
  their ends naming the rebuilt tracks at once (`junctions.renamedAll`,
  which keeps a junction's turns set by hand marked so), and gives the new
  signals to the acting company. Behind `acceptance.lua`'s `signals`, on
  since a two-player game on 2026-10-06 ([MODS.md](MODS.md)). The mod then hears the room's build in
  every game, finds the first signal among the removed tracks' objects and
  its new signals with no spacing set, and builds nothing more;
- any other is stopped with why: `a script's follow-up of another player's
  build: that player's game hands it to the room`, `a script's build with
  no build of this player's just before it`, or what it holds that is not
  carried from a script yet (constructions, removals, stops, and signals
  in any other shape).

Seen on build 40408 (2026-10-02, two games on one PC, Parallel Tracks):
the mod's `guiUpdate` runs in the state the wrapper is on, its
SimpleProposal reads back as lists, and the hook counts its build as one
click. P1's parallels went to the room from P1's game alone, P2's from
P2's alone, the other game's stopped, each built once in both games, and
the room found no divergence ([MODS.md](MODS.md), "Parallel Tracks,
Parallel Roads, Auto Signals"). The stack there named no mod
(`guard.callers`), so the log says no mod's name.


### The road and track modifiers

The tools of the road menu's tools tab and the track menu's (tram tracks,
bus lanes, noise barriers, alleys, the towns' lock, electrification, a
track type) tell game scripts their builds as `streetTrackModifier`. Seen
on build 40408 (a room, 2026-09-30; `hook.log` names the tool, e.g.
`ACTION_TRAM_TRACK_TOOL ::/gui/construction/tools/tram_track_tool.res`,
and what it changes): each rebuilds the stretch of road it is used on,
edge by edge, between the same places (a node between two edges may be
removed and added again at the same place), with its new template (a
tram track or bus lane is the road's template on TF3), decorations
(`edgeDecorations`: `barrier_b.edge` as `{ 3, false }`, `alley.edge` as `{
0, false }`), `roadDevelopmentLocked` and owner (the player-owned tool:
`false>true`, `nil>` the player), new node configurations at its ends, and
the town buildings along it cleared and put back.

The mod carries such a build as the `BuildRoad` or `BuildTrack` of the
network of its first edge (`capture.modify`, `engine.captureModify`): each
link names its template (`kind`), its decorations by name
(`edgeDecorationRep.getName`; every game finds its own id with `find`), and
whether it is locked and owned by the acting company. The town buildings it
clears every game's build clears again (the build is the player's own,
`ignoreErrors`). Junction changes in the tool's proposal now travel with
the polyline; otherwise the replay preserves the endpoint configurations
and remaps their edge references (BUILDING.md, "Junction edits").
Stops and signals on the stretch stay: a new edge between the same
places in the same direction as a removed one, listing exactly its objects,
is the edge rebuilt in place, and every game's build gives it the objects
of the edge it replaces under their own entities (`engine.keptInPlace`,
`networkInto`); a stop moved onto another edge is refused. The same rule
lets the road and track tools build through an edge with a stop on it. A
track signal (not a waypoint) may also move onto the piece of a track
the build cuts that goes on from one of its old track's ends, or that
runs where it stood, the same way, where the tool put it (`Link::kept`,
BUILDING.md).

A track's catenary and type travel the same way, as far as the game's
scripts say (build 40408's `construction_react_util.tl`: the
electrification tool, the track upgrade and the track decorations are
`TrackEdgeModifier`s, as the street ones are `StreetEdgeNodeModifier`s,
and every such tool is the GUI's `streetTrackModifier`): the track
template is the type, and catenary is the lanes' `ELECTRIC_TRAIN` mode, as
a tram track is the road lanes' `TRAM_TRACK` (`TransportMode`, 0 to 15).
INFERRED, not yet seen in the game: that the track tools' proposals have
the street tools' shape and reach game scripts.

Each upgrade is said in `hook.log`: in the player's game when it is handed
to the room, and in every game, the player's included, once built, the
same text everywhere (`roads.upgradeSummary`: what every rebuilt edge has
after it, not what it had):

```
handed the player's build to the room [+e-1/0:8(...)>9(...) -e100:8(...)>9(...)]
upgrade handed to the room: street upgrade of 1 edge(s) rebuilt in place; template ::/street/standard/town_medium_new.lua; 4 lane(s) carrying PERSON CAR BUS TRUCK TRAM ELECTRIC_TRAM TRAM_TRACK ELECTRIC_TRAM_TRACK; lane speeds 13.89 to 13.89; decorations ::/infrastructure/edge_addons/barrier_b.edge; locked 0, owned 0
building +e-1/0:8>9 ::/street/standard/town_medium_new.lua -e100 -n -c8,9
upgrade applied: street upgrade of 1 edge(s) rebuilt in place; template ...; decorations ::/infrastructure/edge_addons/barrier_b.edge; locked 0, owned 0
```

(the template, modes and speeds illustrative), and for an electrified
track `track upgrade of N edge(s) rebuilt in place; template ...; M
lane(s) carrying TRAIN ELECTRIC_TRAIN; ...`. A game whose build fails says
`action 1 of this step was not applied: ...` instead of `upgrade
applied`, and the room's check of the lanes (the network lane reads every
edge's template) finds it.

### Junction tools

Build 40408's native lane/crosswalk tools do not emit a Lua proposal preview.
At the existing `CommandList::Add` hook, `junctions.rs` reads junction-only
WorldBuildProposals and stores them against the click number through the
same `tpf3mp_native.built(n)` queue as the module editor. That snapshot
overrides an older road/construction preview. The Lua command guard also
captures a window's `createTrafficLightProposal` via `capture.windowBuild`.
The original player command remains cancelled; each replica uses the
ordinary ordered replay path. Mixed native geometry proposals keep their
existing capture path, never a partial junction replacement.
The reader classifies mixed geometry/construction proposals before imposing
the standalone junction limit. A live eight-track station attempt generated
392 node configs (another attempt generated 200); checking the 64-junction
limit first incorrectly rejected the station before construction capture.
The regression test covers both sizes and every mixed-proposal vector,
while oversized standalone junction edits remain refused.
The corrected reader was tested in two real games on the local server on
2026-10-01: an eight-track, 320 m station placed and applied on both replicas.
The same run exercised two module edits after correcting the ownership
check for unfrozen station track ends (BUILDING.md, "Module edits and upgrades").

Read-only binary evidence (Steam Windows 40408): StreetProposal config
vectors at +0x60/+0x78; BaseNodeLaneConnectionAndEntity has the component
at +0 and node at +0x78, stride 0x80. BaseNodeConfig connections/crosswalks
are at +0/+0x18, double-slip at +0x48, preference at +0x4c, light states at
+0x50, light type at +0x68 and custom phases at +0x70. Connections have
stride 0x14; phases have stride 0x28. Binding-registration signatures
at RVAs 0x1768337 and 0x22c395f gate this reader in the profile; the
static proof resolves both against the installed executable. Crosswalks
are a `phmap::flat_hash_set<int>` occupying +0x18 through +0x47: control
bytes and slot pointers, size, capacity and internal bookkeeping. The
constructor at RVA 0xa4990d is a third profile anchor; the move/copy at
0x1eda20/0x1fe1e0 and iteration at 0xa49f1a establish this layout. The
reader checks the sentinel, occupied-slot count, bounds and unique edge
IDs, skipping empty/deleted slots. The first real crosswalk click on
2026-10-01 exposed and now regression-covers the earlier incorrect vector
assumption. Every read, vector count and boolean is checked.
The API declaration's `userModifiedLaneConnections` is absent from this
build's binding registration; the adapter does not invent an offset.

Two-game test on 2026-10-01 (Steam Windows 40408, local server, disposable
copy of `tpf3mp_fixture2.sav`): the host removed a crosswalk, the guest
restored it, then the guest changed a lane connection and enabled traffic
lights at the same crossing. Both replicas logged each ordered replay;
network checkpoint hashes agreed at steps 500, 750, 1000 and 1350. The
crosswalk and traffic lights also changed visibly in the other game.
Evidence is in the local `runtime/junction-live-20261001-01/` logs. This
does not yet cover custom phases/reset, adjacent geometry changes,
proposal field comparison or rejoining after an edit. The initial load
also hit a native crash/Lua UI error; a later host-then-guest restart
worked. That startup failure has not been diagnosed. A rejoined game's
speed row showed 1x while the room was paused; pause then play resumed it.

Full acceptance is still pending. Keep both `junctions.strict_junctions`
and `junctions.double_slip_only` false in the normal fork mod: standalone
edits, including double-slip changes, remain refused. U-D30 is upstream
history, not a ReedME approval. Narrow mechanics fixtures explicitly set
`double_slip_only = true` in their own copies. For an authorized full
acceptance run, set `strict_junctions = true` in matching disposable test
mod copies only, then:

1. Start two games through the launcher into one room and the same save.
   Use the actual tools to toggle crosswalks, lane turns and traffic lights;
   edit phase timings and reset settings. Repeat from the other player.
2. Confirm the native capture log names the clicked junction edit, and
   the emitted action has positions/resource names, with no engine IDs.
   Compare the engine's proposal against the replay's proposal field by
   field before treating matching synthetic tests as gameplay evidence.
3. Check both games' visible crossings, arrows and light settings, then
   upgrade/split an adjacent road and check that the settings survive.
   A change that removes a referenced lane must refuse without changing
   either game, rather than silently replace the configuration.
4. Compare network checkpoint dumps through subsequent updates and after
   save/load or rejoin. One-sided edits to a crosswalk, turn or phase in a
   disposable diagnostic test must cause a network-lane mismatch.

`lua_mod.rs` runs portable capture → wire → replay against two different
ID spaces and checks the native-click precedence, refusal cases and
curved-road preservation. `hook/tests/junctions.rs` checks native layouts
and malformed memory. These tests do not launch the game and do not
complete the playtest above. AGENTS.md currently prohibits automated game
launches/modifications; a human must run this check or explicitly override
that restriction before an agent runs it.

### Level crossing types

Clicking a level crossing opens `RailroadCrossingContent`, whose only child
is the native `builtin.RailroadCrossingComp` (`gui/entity_window/
make_entity_window.tl`): no Lua runs when the player picks another type.
Its selection handler (build 40420: the `<lambda_5>` thunk 0x28a4ca0 ->
`sub_28a35a0`) calls `UpdateRailRoadCrossingProposal` (0x28a41c0), the
CanAfford check ("Not Enough Money") for the window's player, the
`WorldBuildProposal` factory 0x9f0000 (withCostRep 0, ignoreErrors 0,
**playerInitiated 1**) and `CommandList::Add` (0x28a386c). So in a room the
hook's apply detour stops it like any player build (`builds.rs`), and game
scripts hear nothing: until this reader the change was dropped in the
clicker's game with no word to the player, and never reached the others
(two games, 2026-10-16: equal rolling checks, the window back at the old
type).

The proposal (`construction_builder_util::Proposal`, the StreetProposal
head `junctions.rs` reads) takes one of two shapes, chosen by whether the
crossing's edge is found in the street graph's index (`sub_b4e690`):

- the crossing entity mapped to the new type index in an
  `std::unordered_map<int,int>` at +0xd0, nothing else. No Lua binding
  reaches this map;
- a rebuild of the crossing's segments, and each of its nodes mapped to
  the type at +0x90: the map Lua's `SimpleStreetProposal.node2rcType`
  converts to (`sub_10c6750`).

The ordinary window may use the rebuild shape even for a plain type swap.
That UI flow is unimplemented and refused unless the proposal has the
separate type-only map shape. Seeing the type-only shape in a fixture does
not establish that the normal click uses it.

`crates/tpf3mp-hook/src/crossings.rs` tells the window's `Add` call by its
return address (`RailroadCrossingComp/type selection Add call`) and reads
its proposal there, where the profile also finds both maps where they were
(`StreetProposal/level crossing maps`, the constructor's `add rcx,0x90;
call; lea rcx,[rdi+0xd0]`). The map is MSVC's: a sentinel list node at +8
and the size at +0x10; each node next, previous, key (+0x10), value
(+0x14). The reader walks the list to exactly its size, every link
answered by its back link, no key twice. The only carried shape is one
crossing in +0xd0, with no segments, nodes, edge objects, constructions or
junction changes. A window rebuild maps nodes to the type in +0x90 and lays
segments again; it may also replace junction settings. The hook refuses
every such rebuild, even when the segment counts match, because a type-only
action cannot reproduce those changes and the captured data does not prove
equivalence. Other shapes are refused too. A recognised click always keeps
an answer (the crossing and type, or `crossing window: ` and why), so the
GUI never hands another tool's preview for it.

The GUI half of the game script (`tpf3mp_sim`) makes the action
(`tpf3mp/crossings.lua`, `capture`): the crossing by the places of all its
nodes, sorted by whole millimetres, and the type by its resource file
(`railroadCrossingTypeRep.getName`; an index differs between games). A
crossing gone, a type not offered here, or the type it has already is
refused. `crossings.capture` refuses any native proposal with `nodes`,
`removed` or `junctions`: the window's rebuild changes more than the type,
and the type-only action cannot preserve its segments or junction settings.
The player is told in the
game bar ("Not in multiplayer yet: changing a level crossing's type",
why in brackets), through the note `tpf3mp.notice`, which the game bar
plugin shows.

Every game's replay (`apply.lua`, `SetCrossingType`) finds the crossings
on a node within 0.5 m (any direction) of a named place, among the nodes
on both a street and a track, by `railRoadCrossingSystem.
getRailroadCrossingForNode`, and keeps the one whose nodes are exactly
the named places, refusing none or two. The acting company must own the
crossing and each of its edges or find them unowned (`mine`); an owner
that does not read is refused.

The script API cannot retype a crossing. `node2rcType` (+0x90) is read
only where the proposal lays a crossing's streets anew (`sub_2644c80`,
from `street_util::CreateShapes`, 0x263f4a0), so a proposal with nothing
but it changes nothing (static reading). The window's map (+0xd0) is read
in `CreateShapes` too (0x264012a): for each crossing the old entity goes to
`RailroadCrossingProposalData`'s removals and a copy of it, nodes, edges
and state kept, of the new type, to its additions; `street_util::
UpdateEngine` removes and adds them. So the replay builds as the window
does, through a carrier like the terraform's: the game script arms the
hook (`tpf3mp_native.crossing(entity, type)`, this game's crossing and
index), sends an empty `api.type.Proposal`, and the apply detour, while
the room's actions run, fills it: the crossing into the carrier's
crossing map through the game's own insert (`sub_567540`,
`unordered_map<int,int>::try_emplace`, found through the window's call of
it at 0x28a42cf, `UpdateRailRoadCrossingProposal/crossing map insert`,
since it has eleven byte-identical twins; its head is checked), the type
into the node it answers, the map read back to hold exactly that. A
carrier with anything else in it is refused, and so is the build. The
crossing is then read back by its first node: of the old type, the action
is refused (alike in every game); of another type, or gone, the replay
stops (`replayFatal`). The crossing gets a new entity, which the room
never names: the next action finds it by its nodes again. This type-only
path is preparatory and mechanics-tested with fixtures; production acceptance
keeps it disabled by default.

Refused in the sender and every replay while `acceptance.lua`'s `crossings`
is false. The PR author's report names staged Japan_2 two-player runs from
2026-10-17, but ordinary-start acceptance is not established here; keep the
gate off until that evidence is verified. Not covered: the cost (the window
checks the player can afford the change; the carrier is built with the game's
cost as any build of the room's, unchecked against the window's; each game
logs what the company paid, `the company paid N`).

**The tools' toolbar type.** The road and track tools (one native class,
`UI::StreetBuilder`) take the toolbar's `rcType` (a concrete type: the
list has no automatic entry) and, building their proposal
(`CreateProposalAndUpdate`, build 40420 `sub_5771f0`, insert at
0x5780d6), map each crossing node of their own polyline, a node with both
a street and a track edge in the proposal's graph, to `-2 - rcType` in
`node2rcType` (+0x90); an automatic choice (-1) maps nothing.
`CreateShapes`' reader (`sub_2644c80`) looks up every node of each
crossing it lays: `-1` is ignored, a value `>= 0` forces that type (the
window's rebuild writes those), one below -1 gives `-2 - value` only where
no crossing is on the group's nodes already. Without an entry the crossing
gets `GetBestRailroadCrossingType` (`sub_8b5bf0`), the era's newest
visible type, not the tool's default by lane speed. The Lua binding of the
tool's proposal does not reach +0x90, so until this the capture dropped
the choice and every game's replay laid the era's type (seen 2026-10-17).

So the hook reads it at the click (`crossings::record_tool`, from
`add_detour` for every player build that is not the module editor's, a
terrain tool's or the window's): the map walked whole as above, `-1`
entries left out, nothing mapped read as none, every other entry the
tool's preferred encoding and all one type read as that type; a forced
value or two types are refused. Each click's reading is kept in a queue of
its own (16 clicks), so the GUI's routing of the other native records is
untouched, and handed to the GUI once (`tpf3mp_native.toolCrossing(n)`).
A profile that does not find the maps reads as unknown. The GUI gives a
street or track click's `BuildRoad` or `BuildTrack` the type by its
resource file (`crossing_type`, action schema 28), the type checked
visible here. Only a click read as picking no type goes as it is: a
reading that is refused, a type not offered, a click with nothing kept, a
reading the profile cannot make, and a hook without `toolCrossing` all
stop the build while `crossingTypes` is on, and the player is told ("Not
in multiplayer yet: a level crossing type picked in the build tool"),
since every game would otherwise lay another type than theirs.

Every game's replay (`apply.lua`, `buildNetwork`) resolves the type by
name first (one not offered refuses the build before anything is sent,
alike in every game), then maps each vertex's node of its proposal
(`node2rcType` of the `SimpleProposal`: new nodes by their proposal ids,
snapped ones by their entities) to `-2 - type`, as the tool does. The tool
maps only its crossing nodes, but a vertex that joins no crossing is never
looked up, and the preferred encoding leaves a crossing already there its
own type, so the crossings laid are the tool's. The crossings at the
build's vertices on both networks (a vertex with a street and a track
edge: a captured track across a street brings the crossing's node as a
new vertex, the street's halves as links to it) are read before the build
and after it: on the vertex's own node when it is an existing one, and on
the nodes its existing neighbours reach within 10 m of it through an edge
of their network (the game lays a crossing as half, connector, half). A
crossing laid anew of another type than the toolbar's, or one extended
that lost its own, is logged ("took type"), not refused: the build went,
and the network lane of the room's world checks reads every crossing's
type, so games that laid different types are found there. Behind
`acceptance.lua`'s `crossingTypes`, off: while it is, the GUI sends such a
build without the type, as before the channel, and a replay refuses one
that carries it. Staged two-game runs on Japan_2 passed on 2026-10-17
(`crossing-tool-1017b`: a track across a road, a road across a track and
one of the preselected type, each of the picked type in both games, 20
paired rolling world checks equal; `crossing-tool-1017c`: a parallel
track keeping the existing crossing's type, a bridge carrying none, 19
equal); ordinary-start acceptance is not yet recorded.
Not covered: a construction's or another tool's click whose proposal maps crossing
nodes (read, never carried; none is known); the preview the other members
see (sent before the click, without the type); the cost of a type (the
replay builds at the game's cost for its proposal).
### Build previews

What a player's build tool shows before the click, the road, track,
station or building it would build, the other members see in their own
games while it shows. Advisory: a preview is never applied, ordered,
logged by the room or saved, and nothing of it reaches the world.
Ported from TpF2 Multiplayer's shared build previews (`mp/previews.lua`
and `native/src/preview_plugin.cpp` in tpf2-multiplayer).

- **What is shown.** The action the tool's proposal would build, as the
  capture makes it for the click (`tpf3mp/capture.lua`): the room's own
  action schema, in millimetres and resource names (D8), so no engine id
  travels. Only the tools that build something new: constructions
  (stations, depots, buildings, a station's edit), streets, tracks and
  stops (`tpf3mp/previews.lua`, `SHOWN`). Not the bulldozer, the
  modifiers or the junction tools; not the module editor or the terrain
  tools, which tell game scripts nothing of their proposals.
- **Out.** The game script's GUI half (`guiHandleEvent`) hands each such
  proposal's action to `tpf3mp_native.preview(action)`, and `nil` when the
  tool shows nothing: an empty proposal, one the room cannot carry, one of
  a tool not shown, a click (the room then orders the real build), or the
  game's active tools changed since the tool's last proposal
  (`api.gui.contextHelper.getIdsOfActiveTool()`, looked at four times a
  second: build 40408 lists its tools by their window, "Construction" or
  "variant-tracks", never by the event's id, so the list changing is the
  tool closing or another opening; where the game gives no list, the
  preview hides on its next proposal or click only).
  A right-click abort can keep that tool list unchanged. On build 40408,
  the hook also withdraws the street/track preview when the native
  `StreetBuilder::ResetProposal` runs, before calling the game's original
  reset. Both road and track tools use that builder. The target is checked
  by the profile; without it this additional withdrawal is unavailable and
  said in the hook log. A new proposal can publish a fresh preview.
  The hook converts it with the schema, refuses one over
  `tpf3mp_proto::MAX_PREVIEW` (16 KiB) and keeps the latest
  (`crate::previews`); the step driver sends it (`Session::preview`) at
  most five times a second, and again every two seconds while it shows,
  in the room's game only.
- **The room.** The server relays it (`GameMessage::Preview`,
  `ServerMessage::Preview`, protocol 17) to the other members of the
  running game, on the control stream, in a queue of its own behind
  everything else and dropped when full (PROTOCOL.md, "Game messages from
  the client"). Not over QUIC datagrams: a road's or a station's action is
  several kilobytes, more than one datagram carries.
- **In.** The hook keeps each member's latest (`crate::previews`); one not
  heard of again for six seconds is gone. `tpf3mp_native.previews()` gives
  the GUI what changed, `{ { from =, action = } }`, without `action` for
  one gone; the room's end tells each one still shown as gone. The
  Multiplayer plugin (`gui/tpf3mp/tpf3mp.script.lua`), which stays mounted
  in the game bar, takes them every frame, in a room or not
  (`tpf3mp/previews.lua`, `take`), and says each
  member's first in the log ("another member's build preview arrived:
  ...").
- **Showing them.** Each preview is made into the proposal its action
  would build in this game, for the sender's company:
  `apply.proposalOf(action, ctx)` runs the build's handler dry, stopping
  it at the proposal it would send, so nothing is sent, built or logged
  (construction, road, track and stop builds only). One this game cannot
  make (a street type it lacks, an edge it has not) does not show, and the
  log says why. A road or construction build's junction settings are left
  out (`junctions.into` is skipped in the dry run; a stop's rebuild keeps
  the settings at its own road's ends, which this game reads itself):
  they name nodes of the sender's game, and
  checking them failed every station snapped to a street ("the junction
  no longer exists").
- **Drawing them** (`crate::drawing`), as TpF2 Multiplayer did
  (`native/src/preview_plugin.cpp`): the hook keeps a `UI::BuilderRenderer`
  of its own for each other member, made by the game's own
  `RendererFactory` (`CGameUI+0x588`) and registered once with the main
  `CRendererComponent` ("mainView", `CGameUI+0xbf0`), at most 16. To draw
  one, the plugin arms the GUI thread for the member
  (`tpf3mp_native.draw(from)`), has the game evaluate the proposal with
  `api.engine.util.proposal.makeProposalData(proposal, context)`, and
  disarms (`drawn()`). The hook redirects that binding's one call of
  `CreateProposalData`: after the game's own call, on the armed thread
  only, it clears the member's renderer and fills it with
  `builder_renderer_util::AddToRenderer` from the toolkit, the converted
  proposal and the `ProposalData` just made, as the game's own
  ProposalViewer does (`ModelData` from `CGameUI+0x538`, no offset, an
  empty entity map, no catchment-area job). Every other call of the
  binding, the mod's own game script's included, is the game's alone. A
  proposal the game calls critical ("Construction Not Possible",
  `errorState.critical`, `ProposalData+0x570`) is drawn too, as the
  game's own street, track and construction builders draw theirs:
  `AddToRenderer` draws that state itself (`0x5e3357`); only the
  ProposalViewer skips it (`0x2aa39d5`), and the hook once did, so a track
  dragged through a road showed nothing to the others.
  `undraw(from)` clears a member's renderer when their tool shows
  nothing. `~CGameUI` is detoured: its renderers are cleared, leave its main
  component and are destroyed before the game's own destructor runs.
  The tint is this game's verdict, blue, or red where this game finds
  errors in the build (a collision) or calls it critical. `drawPreview`
  answers the game's `ProposalData`, and the log says what this game says
  of each member's preview whenever that changes ("another member's
  BuildTrack preview, as this game sees it: critical, errors Construction
  Not Possible"; the entities it collides with by kind, edge, node,
  construction or town building, and their ids).
- **Their terrain**, composed as TpF2 Multiplayer composed it. A preview's
  cuts and embankments are terrain heights its renderer uploads into the
  one view terrain every renderer shares: `EndHeightMod` (`0x7bbae0`)
  calls `terrain::ViewTerrain::ApplyBlocks(*(r+0x50), *(r+0x1b8)+0x1af8,
  *(r+0xf3))` while the renderer's flag at `+0xf0` is set. A renderer's
  `Clear` with its second flag resets that view terrain whole (`0x398600`
  walks every changed block of it, not the renderer's own), so a reset by
  the player's own tool takes a member's embankments away, and a member's
  preview gone would leave its embankments behind. The hook detours
  `Clear`, `EndHeightMod` and the renderer's destructor (`vf0`): after
  every `Clear` that reset the view terrain, whoever's, each renderer's
  heights are applied again, the members' first and this player's own
  tools last, so where both change the same ground the player's tool
  shows. The game's renderers that upload heights are noted in
  `EndHeightMod` (at most 64) and forgotten in their destructor and with
  the world's GUI; the hook clears its own as the ProposalViewer does
  (`Clear(r, 1, 1, 1)`, through the trampoline) and composes after. Every
  offset is read from the upload's own instructions (`0x7bbb6a`), and its
  call must be the profile's `ApplyBlocks`. Without every part, the hook's
  renderers upload no heights (their flag cleared before and after each
  fill): the preview then shows no cut or embankment, and is drawn paler,
  partly under the ground, but never leaves terrain behind; the log says
  which ("drawn, with their terrain, composed" or "their terrain is not
  shown"). Composing runs on the GUI thread only, never while a world's
  GUI is destroyed.
- **Every target and offset** is in the
  profile, each offset read from the game's own instruction that uses it;
  a build without all of them draws nothing (fail closed), and the log
  says so ("the others' build previews are (not) drawn").
  The game's own `builtin.ProposalViewer` cannot draw them: build 40408
  allows it only inside a tool's `ActionDescriptor`, and mounted anywhere
  else (the plugin's layout, tried 2026-10-03) every game that received a
  preview stopped with the fatal assertion `!IsTransformWithContext`
  (`react_transform.cpp:97`). Inside the action slot it would take the
  player's own tool's place.

### Terraforming

The terrain tools (raise, lower, smooth, flatten and the heightmap brush,
`UI::TerrainModifier`), the terrain painter and the asset brush tell game
scripts nothing on build 40408. All three are `UI::ProposalAction`s
(their `vf17` tail-calls `ProposalAction::DoApply`, 0x549ac0), which makes
a `WorldBuildProposal` with `playerInitiated` 1 (the factory 0x9ee860's
seventh argument, stored at payload + 0x3d2) and queues it with its one
call of `CommandList::Add` (0x549be5, profile target
`ProposalAction::DoApply/Add call`). So the click was counted and its
apply stopped as every player's build is, and a stroke in the room's game
changed nothing: "no proposal seen".

The add's detour reads a click whose call returns to 0x549bea
(`crates/tpf3mp-hook/src/terrain.rs`): the payload's `Proposal`, which
must change nothing but its height grid. `Proposal.terrain` is at 0x2d8
(`RegisterUsertypesTransport`, 0x22c1ab0, binds `terrain` there and
`ProposalTerrain.baseHeightMod` at its start); the `Proposal`'s
destructor (0x48ee90) frees three grids there, `{ x0, y0, width, height;
data }` of 0x28 bytes each: the heights (`Vec2f` cells), the paint's
materials (bytes) and its mask (words). A grid whose cell count is not
width times height, a cell that is not finite, more than 65,536 cells, a
street part, a construction (the asset brush), or paint (the painter) is
refused with why. The grid is kept for the click like the module
editor's, as `{ terrain = { x0, y0, width, height, cells = { v1, w1, ...
} } }`, a reason as `terrain tool: ...`. The GUI takes it with `built(n)`
and hands the room `Terraform` actions (`capture.terraform`): the first
cell's index times the cell size (`api.engine.terrain.getBaseResolution`,
4 m) as the corner, the columns, every cell's two values in millimetres,
in bands of whole rows of at most 4,096 cells (the largest fits the
48 KiB payload at any heights).

Every game, the player's own included, applies a `Terraform` in its game
script (`tpf3mp/apply.lua`): it checks the map's cell size, arms the hook
with the grid (`tpf3mp_native.terrain(t)`, optional in the bridge
contract), sends an empty `api.type.Proposal` as the player's build
(`makeWorldBuildProposalCmd(proposal, context, true, true)`, paid by the
acting company), and disarms (`terrain()`, which answers whether a build
was filled). While the room's actions run, the apply's detour fills the
next build with the armed grid: it must be an empty proposal (every list
and the three grids zero), the cells go into a vector of the game's own
heap (the UCRT's `malloc`, which the game's `operator new` calls, with
MSVC's 32-byte alignment and the block's address before the data for
4 KiB or more, as its `operator delete` expects), and the grid's header is
set. A script cannot fill it: Lua's `GridVec2f` has `width`, `height`,
`x0`, `y0` and `at`, no setter. A carrier that is not empty is answered
false; a grid nobody filled fails the action in every game.

INFERRED, not yet seen in the game: the order of a grid's four integers
(TPF2's), that the cells go row by row, that the apply sets the heights
from the grid alone, so every game gets the same ground, that an empty
`api.type.Proposal` reaches the apply with its grids empty, and that the
tool's click is one `DoApply` per stroke part. The lanes do not read the
terrain (`tpf3mp/lanes.lua`).

What `hook.log` says, in the player's game:

```
terraform: click 12 queued 18 by 17 cells from cell (-212, 455), 241 changed, heights 104.20 to 109.85 m
terraform handed to the room: 18 by 17 cells of 4 m from cell (-212, 455), 241 changed, heights 104.20 to 109.85 m
```

and in every game, the player's included, when it applies:

```
terraform: filled the room's carrier with 18 by 17 cells from cell (-212, 455), 241 changed, heights 104.20 to 109.85 m
terraform applied: 18 by 17 cells from cell (-212, 455), heights 104.20 to 109.85 m
```

(the numbers illustrative; a stroke cut in bands says `(part i of n)`).
The `Terraform` action is enabled in `tpf3mp/acceptance.lua` after the
2026-10-02 local two-game validation of all five height brushes on build
40408. Native base/surface heights matched, including after save/reload;
see `investigation/STATION_TERRAIN_2026-10-02.md` for measurements and limits.
The gate remains available: with it off, the hook still reads a stroke at
its click, but the GUI's sender
refuses the actions (`terraform awaits two-player game acceptance`), and
so would every game's replay; nothing is armed and no carrier is filled.

The painter's click is stopped the same way, and its stroke read at
the same call (`paint: click N queued ...`): the materials grid (a byte a
texel, 255 where the stroke leaves one as it is) and the mask grid, a
`vector<bool>` of 32-bit words with its count of bits just past it
(`MASK_BITS`, 0x28 into the grid, as TPF2's `Grid<bool>`). The hook cuts
it into `Paint` actions of whole rows, each within 4,096 runs (materials
and mask together), run-length packed; every game arms the hook with one
(`tpf3mp_native.terrain(t)`, the action's body) and sends the empty
carrier, whose materials and mask grids the hook fills.

The asset brush's click is stopped too, and its stroke read there
(`asset brush: click N queued ...`): the groups it removes (`toRemove`)
and each group it adds, a `ConstructionEntity` of no file whose
subconstruction's models (the vector at 0x140 of the first element of the
vector at 0xa30, each 0xa8 bytes: the model's resource name, its world
matrix at 0x60) are its trees (seen on build 40420 with the hook's asset
probe, 2026-10-16). The GUI names each removed group by its first asset
and how many it holds, as the asset bulldozer does, and hands the room
`PlantAssets` actions; every game finds its own copy of each removed group
and builds the stroke's groups through the game's own Lua, as the
bulldozer's replay rebuilds one (`api.type.Proposal.ConstructionEntity`,
an autoRemovable desc, one subconstruction of the trees), for the acting
company. A group missing or ambiguous in a game refuses the stroke there.

### The world's lanes

A room finds a game that drifted from the others by comparing the world's
lanes at every checkpoint step (`checkpoint_interval`, 50 steps by
default): digests of parts of the world, which the session reports with
the step (`Session::after_step`, `Game::lanes`). The lanes are read by the
mod's game script, which sees the world between updates:

- The session ends every batch at a checkpoint step, so a checkpoint is
  always a batch's last update. The driver knows the step a batch starts
  at (`RoomGate::next_step`) and so whether it ends at one, and tells the
  hook's Lua side (`lua::begin_batch`), which counts the batch's updates by
  their `take()`.
- `tpf3mp_native.checkpoint()` returns whether a report is due and the
  absolute simulation step (Lua bridge version 14). Every `postUpdate`
  reads a small part of the world at that step. The last update hands over
  the accumulated observations through `tpf3mp_native.lanes`.
- After the batch the driver takes them (`lua::end_batch`), makes a
  SHA-256 digest of each lane's text (`step::lane_digests`), and the
  session reports them for the checkpoint step. A batch that ended at a
  checkpoint without them holds the world: the room could not tell whether
  it is still its own (fail closed).

The lanes, numbered as the regression harness's model numbers its own
(`crates/tpf3mp-testkit/src/regress/model.rs`, `lane`), each a count and a
hash of sorted rows, so the order the engine lists things in does not
matter:

| lane | reads |
|---|---|
| 0 network | street/track endpoints (0.1 m), template and per-lane modes/dimensions/direction; junction positions (1 mm), portable turn/crosswalk references, light preference/resource/phases and flags; each level crossing's node positions (1 mm) and type file |
| 1 constructions | every construction by its file and position (0.1 m) |
| 2 lines | every line's number of stops |
| 3 vehicles | each vehicle's state, stop and place on its path: the path edge, the distance along it (1 cm) and the speed (1 cm/s), the simulation's own (`MOVE_PATH.dyn`) |
| 4 economy | the save's own player's balance; with more than one company, each company's balance by its roster id; the subsidy script's offers, taken, completed and failed subsidies with their terms (`tpf3mp/subsidies.lua`, `rows`) |
| 5 towns | each town's number of buildings |
| 6 people | the number of people |

Nothing is read by an entity id that two games agreeing on the world could
number differently, except where the save carries it (towns, the player).
On build 40408 `getEntitiesWithComponent`
refuses `BASE_EDGE`, `LINE` and `PLAYER` ("Cannot loop over this component
type"), hence the street and line systems.

#### Rolling observations

Normal checkpoints no longer scan the whole network and construction list
in one update. `lanes.rolling` visits 1 km squares of the map through the
engine's octree, collecting IDs in its callback and reading components only
after it returns. Intersecting edges include their endpoint junctions,
including endpoints outside the square. Border squares extend outside the
terrain bounds. All component references and per-read caches die in the
same update.

A square with more than 32 edges and constructions subdivides into four
before canonicalization. The split's static counts are observations too:
different inventories cannot silently select different read schedules.
Subdivision stops at 32 m cells, where all remaining objects are read;
extreme concentrations can therefore still exceed the usual per-update
cost. A wall-clock deadline never changes which objects or steps are read.

One of lanes 2–6 is also read each update, rotating every five steps (one
second at the room's normal five steps/s). Lanes 0–1 cover a full spatial
sweep, whose duration depends on map area and density. This trades immediate
whole-map comparison for bounded-area observations: a persistent static
change is detected when its area is visited and the next checkpoint is
reported. It is not a snapshot of the entire world at the report step.

Each observed lane hashes its sorted rows together with the simulation
step and area. The window hashes, pending subdivision queue and cursor are
saved in `worldCheck` in the mod's game-script state after every update.
Joining or rebasing from a mid-window save resumes that history; starting
a room at step 1 starts a new history. Missing history, skipped/repeated
steps or a failed lane read hold the game through `scanned(false, why)`.
`lua::end_batch` also refuses requested reads that were not acknowledged.
Matching `err` strings are not accepted as successful rolling checks.

The old full reader remains for diagnostic dumps and stand-in engines.
Where the hook reads the static lanes natively, the rolling checks read
them in parts instead of tiles ("The network lane read natively", below).
`rolling world check` logs the window and cursor; `rolling world sweep`
logs completed coverage. `rolling-check-cost` reports the mean and maximum
per-update read cost, including saving the scan state. These diagnostic
times are never saved or hashed. The older measurements below concern
the synchronous full reader, not the rolling implementation.

On the large Silver fixture, two real games completed the same first sweep
in 2,412 updates (8 minutes 2.4 seconds at 1x). Across 7,400 retained reads,
the mean was 3.73 ms and the maximum 21 ms, including saving the rolling
state. There was no reported divergence. This removes the recurring full
scan; game saves and industry generation can still pause independently.
Methodology, raw data and limits are in
[the rolling-check investigation](../investigation/TF3_ROLLING_CHECKS_2026-10-05.md).

#### What the lanes cost

Build 40408 also has optional bulk readers, `tpf3mp_native.laneRows` and
`junctionConfig`. The mod first copies a borrowed component with
`api.type.BaseEdge.new(component)` or `BaseNodeConfig.new(component)`.
Rust verifies the owned userdata's exact class and inline payload before
reading its bounded vectors. Lane settings become the existing sorted lane
text; junction settings become a plain Lua table for the existing canonical
row builder. Unsupported userdata uses the Lua reader. Older profiles without
`lua_touserdata` do not advertise these functions, avoiding needless copies.
The layouts and live ownership check are recorded in
`investigation/TF3_CHECKPOINT_SNAPSHOTS_2026-10-05.md`.

Junction checks also reuse the complete street/track adjacency maps that
enumerate their nodes. Each map is fetched once per checkpoint, instead of
asking the engine for each node's street and track segments again. Capture and
replay retain their live queries. Nothing is cached across checkpoints.

These changes still run synchronously at the agreed simulation step. They
retain the full network, crosswalk, lane and traffic-light checks and the
existing checkpoint frequency; they do not introduce a rolling/background
scan. A future background reader needs an immutable snapshot or complete
change tracking so that worlds are compared at the same step.

The read runs inside the game's step, on its main thread, so the whole
game stands still while it runs, and the step after it catches up with
several updates at once: players see a stutter, and vehicles jump, at every
checkpoint (every 10 s at 1x, every 2.5 s at 4x). The mod's game script
logs what each read cost:

```
mod: lanes read in 201.0 ms: network 173.0, constructions 26.0, lines 0.0, vehicles 0.0, economy 1.0, towns 0.0, people 1.0; of it sort+concat 5.0 ms, hash 6.0 ms over 1905931 bytes; network: map 2.0 ms, 2212 edges' getComponent 3.0 ms, their 8836 lane configs 47.0 ms, junctions 85.0 ms
```

The network lane is most of it, and it grows with the map. Measured on
build 40408 in a room of two games on a save with 2212 edges and 2126
junction rows (`MP_crash_1004`), the checkpoint's step took about 950 ms
(20 ms otherwise): the junction rows 290-460 ms, the edges' lane configs
260-400 ms, the Lua hash over the lanes' 1.9 MB of text about 150 ms. What
made them cheaper, with the lanes' text unchanged (checked in the game
against the old read at every checkpoint):

- `junctions.rows` reads each node, edge and position once per read
  (`remembered` in `tpf3mp/junctions.lua`), makes each edge's key once and
  names each light preference and light resource once: every junction at
  an edge's ends named it again, each time from the engine.
- An edge's lane configs and each config's modes are read once: every
  field read of a component asks the engine again.
- The hash runs in the hook (`tpf3mp_native.hash`), with the Lua `hashStr`
  where a state has no hook: 5-12 ms instead of about 150.

Together the read took 200-275 ms there, the checkpoint's step 240-300 ms:
still a visible stutter, about a quarter of it. Reading the network natively
takes most of the rest (below).

#### The network lane read natively

`TPF3MP_HOOK_NATIVE_LANES` in the game's environment lets the hook read the
rolling checks' static lanes, network and constructions, from the engine's
memory, one part at a time. The reader is
`crates/tpf3mp-hook/src/netread.rs`; the build's offsets are in its native
bundle, `profiles/<build>/netread.rs`; the layouts are in
`investigation/TF3_NATIVE_NETWORK_2026-10-04.md`.

This is a prototype. The schedule and the contract are proposals for the
owner of the rolling checks (see below).

| value | the mod |
|---|---|
| unset, `off` | rolling tiles through the octree, as above (scan version 1) |
| `on` | a room that starts at step 1 keeps a history of native parts (scan version 2) |
| `compare` | the same, and each part is also read in Lua; Lua's text counts, and a difference is logged with its rows |

**Parts.** The map is split into parts. An object's part follows from its
canonical row alone, so the hook, Lua and every game place it alike, and
entity ids play no part:
- an edge goes by the lower of the two ends its row names, to 0.1 m, by x,
  then y, then z;
- a junction goes by its node key's millimetres (`pointKey`);
- a construction goes by its row's x and y, to 0.1 m.

Cells are 256 m. Part = `(floor(x / 256 m) + 3 floor(y / 256 m)) mod N`,
in whole tenths of a metre or millimetres, so every part has cells all over
the map (`netread::part_of`, `lanes.partOf`, `lanes.rowPart`).

**Schedule.** One batch every S updates reads one kind of objects of one
part: the edges, the junctions or the constructions. The kinds take turns
before the next part, and after 3·N batches the whole map is read.
`TPF3MP_HOOK_PARTS=NxS` (default `10x1`) sets N and S for the prototype; a
room keeps what its first update found, in its saved scan state. Each
update still reads one rotating dynamic lane and is acknowledged once. Each
batch's text is hashed into its lane's window hash with the step, the kind
and the part (`parts|kind|k/N`). As with the tiles, a sweep samples objects
when their part is read: an object moved between parts within a sweep is
read once, twice or not at all in that sweep.

**Plan agreement and work limits.** The ordinary launcher and CLI create,
join and reconnect paths parse `TPF3MP_HOOK_NATIVE_LANES` and
`TPF3MP_HOOK_PARTS` through the shared protocol contract, including
`--game-env` overrides given to the launcher. The bridge caches that effective
plan for room updates and reconnects. Malformed mode or
plan values refuse multiplayer. Off mode always advertises rolling-v1,
`10x1`; a valid prototype part setting has no effect in off mode. Native
modes allow 1..1024 parts and strides 1..1000. The room fixes and persists
that exact mode/parts/stride at creation; joins and reconnects must match
before they replace a connection or change membership. The agent waits for
both the room view and mod declaration before processing a turn stream,
including on reconnect, and forwards the room plan in bridge-30 `Begin`.
The hook uses one cached parsed configuration for its native mode and plan,
and holds a mismatched or malformed configuration before loading a world or
running room steps. Default-off behavior is unchanged; `on` and `compare`
remain experimental opt-ins.

A saved rolling-v2 history is refused before its next hash if this game's
mode or part plan changed. The rolling-v1 history used when mode is `off`
also saves and checks the mode, part count and stride. A rolling-v1 save
from before these fields existed is adopted only when all three are absent
and the current mode is `off`; partial metadata and native-mode resumes fail
closed. A room recovered from the fork's format 11 retains its exact
configured plan. Earlier formats
are refused before recovery; this fork does not apply upstream's legacy
format readers.

Each native part scans every entity slot's component bits before it selects
rows, including when its cell is empty or one cell contains all rows. The
read log reports the number of component-bit entity slots scanned. The
4,096-slot dense-cell fixture verifies the same full-table count for both a
hot part and an empty part. A second fixture finds a sparse construction at
entity 131,072 in a 131,073-slot table, so IDs above 115k are not refused by
an artificial cap. These establish the scan's work shape and high-ID
coverage, not a performance bound. `MAX_ENTITIES` (8,388,608) is an
input-safety ceiling, not a safe per-update CPU bound; each part still scans
the complete entity-slot table. Compare mode also runs the Lua reference
walk for that part and has no hard per-update work bound. There is no hard
latency bound for this prototype, and the one local measurement below does
not establish frame-time safety on other maps or machines.

**Bindings.** In the game script's `postUpdate`, `part(n, k, kind)` reads
one batch and keeps it, and hands the Lua its light and crossing types
(`lights`, `crossingTypes`) and the junctions left to it (`deferred`).
`partTexts(n, k, kind, preferences, lights, deferred, rows,
crossingTypes)` (Lua bridge version 15), called in the same update,
answers its lanes' texts:
`count:hash`, as `summary` makes them, and `nil` for a lane the kind does
not read. It answers only for the batch asked for, read at the room's
current step, and only once. `part()` without arguments reports the
configured plan (`mode`, `parts`, `stride`) even when mode is `off`, and
reads and replaces nothing.

A history of parts is held, through `scanned(false, why)`, whenever the
game reads no parts or a part did not read. This is checked on every
update, not only on a batch's. No other reader takes over.

**Rows.** The hook makes the same text the mod's Lua makes:
- each edge's row as `lanes.edgeRow` makes it, numbers printed as the
  game's Lua 5.2 prints them (`%.14g`, `%.3f`, `%.0f`, checked against a
  real Lua in the tests);
- each junction's row as `junctions.rows` makes it, but for its traffic
  light preference and its light's resource name, which only the game's
  Lua names: the mod hands those names in `preferences` and `lights`;
- each construction's file (its `ResName`, +0) and the translation of its
  `transf` (16 float32 at +0x58; x and y at +0x88 and +0x8c), as
  `file@x,y` to 0.1 m.

On a part's junctions turn the hook reads its level crossings too: each
`RailroadCrossing` (its pool by `CompVec<RailroadCrossing>`'s vtable,
0x40 bytes: its nodes, a `std::vector` of 4-byte entities at +0, at most
8, aligned, its capacity past its end; its type's index at +0x30), its
nodes' places from their `BaseNode` in whole millimetres rounded half up
(`floor(v * 1000 + 0.5)`, as `crossings.lua`'s `mm`; a junction's key rounds
ties to even), sorted, none twice; the crossing in the part of its first
node's millimetres, as a junction. `part` hands the game's Lua the
crossings' type indices (`crossingTypes`), which it names
(`crossings.names`, a type with no name failing the read) and hands back
as `partTexts`' eighth argument; a part that hands no `crossingTypes`
(a hook from before version 15) stops the check. The rows are `crossing:x,y,z;x,y,z|file`,
as `crossings.rows` makes them, and the network text is the hook's own
followed by theirs, `<count>:<hash>|crossings <count>:<hash>`; with `rows`
the crossing rows are among the network's, so compare mode checks them
row by row against this Lua's. A world that never had a crossing may have
no pool of them: that one absence is no crossing, but a pool whose head
does not read fails the read. A tile (version 1) reads, in Lua, the
crossings on its edges' nodes.

Rows are sorted, joined and hashed in the hook; no row comes into the
mod's Lua unless compared.

Compare mode's reference reads every row in Lua without the hook's
decoders (`lanes.decoders` and `junctions.decoders` off). It reads the same
owned copies of `BaseNodeConfig` as every other reader, because copying
can lay a crosswalk set out anew. It keeps the rows of the batch's part and
kind (`lanes.partRows`) and compares the rows themselves, not only their
hash.

What it reads, only while the game's step runs (the engine is
`[CGameTime+8]` of the `CGameTime` the step called its speed getter on, so
only inside the game script's `postUpdate`):

- the pools of `BaseEdge`, `BaseNode`, `BaseNodeConfig`, `Construction` and
  (on a junctions turn, where the engine has one) `RailroadCrossing`,
  each the one of the engine's pools (`engine+0x78`) whose vtable is its
  `CompVec`'s; its index there is its type id;
- every entity whose component bits (`engine+0xc0`, 16 bytes an entity)
  hold that type id, its data index from its component list
  (`engine+0x90`, which must list the type id), its component from the
  pool's dense vector or its pages; a removed entity is skipped. Entities
  are looked up 64 at a time, and the processor is asked to fetch their
  records, lists and elements before they are read (a prefetch hint, which
  reads nothing and cannot fault), so that the waits for memory overlap;
- each edge's ends, lane configs, road template, nodes and network; each
  node's position; each junction's turns, crosswalk set, phases, flags,
  preference and light type; each level crossing's nodes and type index.

Anything that does not read as the layout says fails the read with why:
a pointer out of order, a count past its bound, an entity whose bits and
list disagree, a flag that is not 0 or 1, a number that is not finite, a
turn naming an edge that is not there. The history of parts then holds.
Nothing is written, and nothing of the game's is called.

It reads only in an update whose world check is due and not yet
acknowledged, and only on the thread running the game's step, inside it:
there nothing changes the engine beside the step's own work. On build 40408
the game script's `postUpdate` runs there (thread 3 in the hook's
numbering, every time); its `update` runs on the game's pool of threads,
one or another, and so does no reads. A call on any other thread (the
GUI's state, the console) or outside a due check reads nothing and says
so. These two guards are what is checked; which Lua state calls is not.

A junction whose phases name its crosswalks in an order that matters is
left to the mod: one with two crosswalks or more, one of whose phases
locks some of them but not all, or locks one of them twice. A phase names
its lanes by index, the crosswalks' part in the order the game's Lua lists
the crosswalk set, and the Lua lists a copy of the component, whose hash
set a copy may lay out in another order than the engine's own (phmap's
copy inserts anew). The hook hands those junctions' nodes over
(`deferred`), and the mod makes their rows itself (`junctions.rowsOf`) and
hands them back to `partTexts`.

Measured on build 40408, Sep's `tpf3mp_silver_ab_20261001` (5,852 edges,
5,049 junctions, 6,187 constructions):
- One game alone, the same build for both, 109 checkpoint windows each.
  The rolling tiles read the whole map once in 2,412 updates (482 s at
  1x), at 1.71 ms an update on average, 6 ms at most. Native parts by kind,
  8 parts every 2 updates, read it in 47 updates (9.4 s), at 3.50 ms on
  average, 11 ms at most, 9 ms the median of each window's highest.
- One batch cost, on average: edges 3.4 ms, junctions 5.3 ms,
  constructions 3.3 ms.
- The game's step calls (median, 95th and 99th percentile) were within the
  noise between runs on that PC.
- Compare mode found no difference, row for row, in several hundred
  compared batches.

The finding of a batch is about 2 to 3 ms, mostly memory latency: one pass
over the component bits and three or four scattered reads an object. The
rest is the batch's rows.
An edges-only or constructions-only batch now resolves only its own component
pool. In the four-pool, empty-world test this removes three repeated pool
table walks (29 memory reads before, 8 after). This is a read-count result,
not a measured frame-time improvement.

Earlier, a full read of both lanes at the checkpoint (the first form of
this reader) took 26-30 ms on `MP_crash_1004` and moved a 40 to 90 ms
step onto every checkpoint on Silver.

The follow-up shares the network lane's already-read edge components with
junction rows and memoizes each node's street/track adjacency within that
read. Counting lane configs also uses the already-read collection. No cache
survives a checkpoint, and capture/replay continue to read fresh components.
In the four-arm junction fixture, this reduces edge component reads from
8 to 4, adjacency queries from 9 to 2, and lane-config field reads from
8 to 4. These are API call counts, not a measured frame-time improvement.
A comparison with PR #102's original head (`2039a96`) produced identical
digests in 48 road/track cases with shifted entity IDs and successive
geometry, lane-speed and traffic-light edits. Real-game timings above
precede this follow-up. A missing diagnostic clock no longer prevents a
checkpoint from being read.

Measured follow-up (2026-10-05): two local build-40408 games, the shared
`tpf3mp_silver_ab_20261001` save, roughly 5,850 edges and 19,660 lane
configs. Baseline `2039a96` and candidate `24d4827` read the same checkpoint
back to back, with the same native hash. After discarding each game's first
four observations, the first ten baseline-first and ten candidate-first
pairs per game were retained: 40 pairs total. The game's Lua pool makes a
module-local counter non-global, so balancing uses the logged execution
order, not that counter. Raw observations and selection flags are in
`investigation/checkpoint_ab_2026-10-05.csv`.

| Reader cost | Original PR median | Follow-up median |
|---|---:|---:|
| Full checkpoint read | 637 ms | 620 ms |
| Network lane | 569 ms | 547 ms |
| Junction portion | 259.5 ms | 265 ms |

Mean full-read cost was 642.525 -> 626.875 ms; median paired saving was
14 ms (2.2%), and the candidate was faster in 31/40 retained pairs. The
incremental gain is modest: junction-only time did not improve in this run.
All 64 observations collected before cleanup had identical baseline and
candidate digests and zero failed lane reads; no room divergence was
reported during the measured interval. These are paired reader timings
(`os.clock` on Windows), not an FPS benchmark or a normal checkpoint-step
duration: benchmark steps run both readers. The games were quit normally
and the staging copy restored without instrumentation afterwards.

Measured native follow-up (same save, 2026-10-05): baseline `6af232e`
versus the owned-snapshot readers and complete adjacency-map reuse. In each
game, the first four pairs were warm-up; the next ten alternate execution
order using a hook note shared across Lua states. Twenty retained pairs:

| Reader cost | Same-run baseline median | Native follow-up median |
|---|---:|---:|
| Full checkpoint read | 809.5 ms | 653.5 ms |
| Network lane | 718.5 ms | 543 ms |
| Junction portion | 330.5 ms | 316.5 ms |

Median paired saving is 155.5 ms (19.96%). All 28 pairs, including warm-up,
had identical digests, zero failed lanes and zero native fallbacks. Each read
used the native paths for roughly 5,850 edges and 5,050 configured nodes.
After the paired phase, each game ran six checkpoints with only the candidate:
12 reads had a 645.5 ms median; their whole checkpoint steps had a 713.5 ms
median. Step traces are buffered: match the 20 checkpoint-marked frames to
the 20 reads in order per game, not to adjacent log lines. The hitch remains
significant. No room divergence was reported during this run.

The PC ran two games at once; compare paired timings within this run, not its
absolute numbers against the earlier 620 ms run. Builds and automated checks
were finished before the retained observations. This measures checkpoint
latency, not average FPS. Raw observations are in
`investigation/checkpoint_native_ab_2026-10-05.csv`,
`checkpoint_native_only_2026-10-05.csv` and
`checkpoint_native_steps_2026-10-05.csv` in the same directory.
Both games were quit normally and the staged mod restored. Earlier diagnostic
runs (a sandbox-global failure and rejected borrowed userdata) are excluded;
they established no valid speedup.

The Windows rig's successful injection tests use a test-only DLL that
signals the normal hook-ready event. A system DLL is still used to test
startup timeout cleanup; production readiness checks are unchanged.

`TPF3MP_HOOK_STEP_TRACE=1` in the game's environment writes a
`step-trace:` line for every call of the game's step
(`crates/tpf3mp-hook/src/steptrace.rs`): when it came, how many updates it
ran and why (`run`; `wait` for the room's next step, `reserve` for
released steps kept back ("Even steps"), `actions`/`replaying` for the
room's actions, `save`, `load` for the room's world, `hold`; `own` outside
the room's game), the game's step and the whole call in milliseconds (timed
for the trace even with `TPF3MP_HOOK_PERF=0`), `lanes` on a checkpoint's
batch, and, on a call that could run the room's steps, how its count was
chosen: the steps released and not run (`avail`), the batch's cap (`cap`),
the nominal count (`nom`) and the rule (`pick`: `nominal`, `reserve`,
`underrun`, `repay`, `barrier`, `command`, `paused`, `stale`, `far`, or
`off`).

Vehicles are compared by their place on their paths, not in the world. On
build 40408, with a bus running a line in two games in one room, the bus's
world position (`api.engine.util.vehicle.getPosition`) and its path state
as the frame began (`MOVE_PATH.dyn0`) differed between the games in the
same simulation update, by millimetres to metres: they follow each game's
own frames. Its path state (`MOVE_PATH.dyn`) was the same in every sample.
Read to 1 m, the world position flipped at a rounding edge now and then,
and the room resynced a game whose simulation had not diverged.

Seen on build 40408, two games through the deployed server: every lane
read, and the room found no divergence over several checkpoints.

#### Lane dumps

Vehicle entries also include `arrival=<station-index>/<terminal-index>`
and `arrival_locked=<bool>` from `TransportVehicle`, so a comparison can
detect a different terminal choice before movement diverges. These are
diagnostic fields only; they do not change the vehicle lane digest or the
wire format. An unavailable field is printed as `nil`. When a `Path` is
available, the dump also includes its physical edge count, an ordered hash
of up to 4,096 edges (with the sampled count), nearby edge identities and
directions, end and terminal-decision offsets, and movement/acceleration
state. Different path lengths or decision offsets can expose route
differences before movement diverges. Native edge indices are not portable:
an industry rebuilt in a different order can give identical geometry
different indices, so a path-hash mismatch alone is not proof of different
physical routes. These fields are local diagnostics and never sent as actions.

`TPF3MP_HOOK_TRACE_LINE=<native line entity>` additionally traces changes to
that line's native route cache through `ecs::LineSystem::GetData/return`.
It records terminals, edge counts, hashes of edge identity and direction
(excluding padding), decision indices and invalid flags. The trace includes
temporary engine contexts; compare simulation vehicle checkpoints before
concluding that a cache read changed gameplay. It is disabled by default.

A digest says *that* a lane differs, not which vehicle, line or edge, nor
how. So after a divergence every game in the room writes the full text of
the diverged lanes to its `hook.log`, entry by entry, at the same two
checkpoints, and `tools/lane_diff.py` diffs two or three games' logs
(`crates/tpf3mp-hook/src/lanedump.rs`). Seen in a three-player playtest on
build 40408: the room said `Diverged { step: 250, lanes: [3] }` to two games
and nothing more.

- **Who dumps.** The room tells only the games whose world differed from
  its verdict (`Notice::Diverged`), and the others are needed to diff
  against. So a game told it diverged says so in the room's chat, a line
  every member's hook reads (the room passes chat to its sender too):

  ```
  [tpf3mp] lane dump lanes=3 steps=300,350 diverged=250: this game's world differed from the room's; every game writes these lanes to its hook.log
  ```

  Its steps are the first two checkpoints at least 20 steps
  (`lanedump::MARGIN_STEPS`) past the step the game runs next, time for the
  line to reach every game. Every game that hears it, that one included,
  dumps those lanes at those steps; the line shows in the Multiplayer
  window like any other. No protocol change: the chat carries it.
- **Bounds.** One dump asked for a minute at most, here or heard
  (`lanedump::GAP`); a line naming steps already planned only adds its
  lanes (both diverged games of a room of three say one). A line heard is
  checked: at most 16 lanes and two steps, each a checkpoint step, not past,
  and no more than 20 checkpoints ahead. One checkpoint writes at most
  `lua::MAX_DUMP_LINES` (20,000: room for the whole network lane of
  `twomptest`, about 10,800 entries) entries, all its lanes together, each cut to
  2000 bytes; the rest are counted.
- **By hand.** `TPF3MP_HOOK_LANE_DUMP` in the game's environment dumps
  lanes at every checkpoint, for chasing a desync on purpose: `all`, or
  lane numbers (`3`, `0,3`). `off` dumps nothing, not even after a
  divergence. The hook says in its log what it read.
- **Vehicle diagnostic detail.** Long vehicle entries are split into
  `vehicle-N/detail-NNN` records below the relay's 1024-byte line limit;
  long capacity fields become numbered `free_partN` fields. The diagnostic
  row uses `~` instead of `@` so a numeric position is not redacted as an
  email address. The lane hash input is unchanged.
  `vehicle-N/path-NNNN` records show route order, raw edge entity, lane
  index, direction, and edge endpoints/tangents where available. The old
  `path_hash` explicitly says `path_hash_scope=local_ids`: different raw
  IDs alone do not prove route divergence. Compare geometry and lane index
  too. It samples the first two route edges, up to twelve around the
  vehicle's current edge, and the last two of the first 4096: at most 16
  per vehicle and 128 per dump, with at most 1024 distinct geometry reads, cached within
  that dump. The full path hash still covers up to 4096 edges; a changed
  unsampled edge can change that hash without a differing sampled row.
  `route_omitted` and `geometry=budget` make these limits visible;
  `geometry=unavailable` means the native edge could not be read. Vehicle
  base entries and compact `vehicle-N/core` entries reach the hook before
  route records so optional detail cannot crowd them out at its line
  limit. A core names the vehicle's state, stop, line, edge, full-precision
  position and speed, and its path count and hash. The hook writes accepted
  route records first, then verbose base entries, then every vehicle's
  core and the lane summaries. Thus a newest-byte tail of an oversized
  dump can still locate the differing vehicle even if verbose rows are lost.
  It cannot guarantee a complete upload for a large world or busy log;
  use `collect-logs` for the local full log if diagnostics report skipped
  bytes. These extra reads happen only during dumps,
  not ordinary rolling checks. They describe the sampled state, not every
  earlier routing decision; a root cause can still require reproduction.
  Line assignments also log one `assign-vehicle` line per named vehicle at
  the action boundary, with canonical/native vehicle and line IDs, the
  vehicle's ordered model names and carrier, the line's mode set, owners and
  the command's per-vehicle result. A refused later command stays visible
  beside an earlier vehicle command that applied; these reads do not scan
  vehicles every tick.
- **A box of the network.** `TPF3MP_HOOK_LANE_DUMP_BOX=x0,y0,x1,y1` (the
  world's x and y, metres, any two opposite corners) dumps the network lane
  at every checkpoint in `TPF3MP_HOOK_LANE_DUMP_BOX_STEPS=from-to` (steps,
  both included; unset, every checkpoint), only the edges with an end
  inside the box, and no junctions; the lane's `summary` line is still the
  whole lane's. It works with `TPF3MP_HOOK_LANE_DUMP=off`, and gives way to
  a whole dump of the lane asked for at the same checkpoint (a divergence,
  or `TPF3MP_HOOK_LANE_DUMP` naming lane 0). The order carries the box to
  the mod (`dump()` answers `{ step =, lanes =, box = { x0, y0, x1, y1 }
  }`). A value that does not read is refused in `hook.log`, and nothing is
  cut.
- **In the game.** The driver passes the dump with the batch that ends at
  the checkpoint (`step::Batch::dump`, `lua::begin_batch`). In that
  batch's last update `tpf3mp_native.dump()` answers `{ step =, lanes = {
  ... } }` once; the game script, right after handing over the lanes, reads
  each lane asked for again with `lanes.dump`, and hands each entry to
  `tpf3mp_native.dumped(lane, entry)`, which the hook writes to its log
  after the batch as `lane <n> step <step> <entry>`, then a line
  `lane dump at step <step>: ... entries written`. Both functions are
  optional in the bridge contract (a hook or mod without them dumps
  nothing, as `built`), so `VERSION` stays 9.
- **The entries.** `lanes.dump` runs the very reader that sums the lane
  up, so it reads the same values the digest hashes: each entry is
  `<key> <field>=<value> ... entity=<e> row=<row>`, the fields the raw
  values the row was made from at full precision (`%.17g`), `row` the text
  hashed. The key is the registry's canonical id where there is one
  (`vehicle-N`, `line-N`, `town-N`, `industry-N`), else `row:<row>`
  (edges, other constructions) or `player` and `people`. The Lua entries
  are sorted by key, ids first; the hook puts optional vehicle route rows
  before every lane's base rows and compact cores in its log. The same world still dumps the
  same lines in the same order whatever order the engine lists it in.
  Each lane ends with
  `summary <text>`, the lane's text as hashed; a lane that cannot be read
  is one line `err <why>`.

  | lane | fields |
  |---|---|
  | 0 network | `p0`, `p1` (the edge's ends, `x,y,z`, read from the game's `Vec3f` userdata), `template` |
  | 1 constructions | `file`, `x`, `y`, `z` |
  | 2 lines | `stops`, then `stop<i>=<group>/<station>/<terminal>` |
  | 3 vehicles | `state`, `stop` (index), `line`, `edge`, `pos`, `speed` (`MOVE_PATH.dyn`); dump only: `arrival`, `arrival_locked`, `load` (`loadState`), `pending` (`unloadPendingIncome.amount`), `free` (`lineStop2cargo2available`: free capacity per cargo type, stops apart by `\|`), the path fields |
  | 4 economy | `balance`; dump only: `loan`, `time` (`GAME_TIME`), `income` (`finance.calcIncomeSince(0)`), `last_income` (`finance.getLastIncomeTime`), the finance window's table (`finance.computeFinanceTable`, four periods: `transport<carrier>.<kind>`, `investment<kind>`, `other<kind>`, `loan`, `interest`, `total`, ..., sorted), and one entry per vehicle and per line, `takings=` (`finance.calculateBalance({e}, 0, now, true)`: its income and maintenance since the game began, as the game's vehicle and line windows sum them) |
  | 5 towns | `buildings`, `size` (the three size factors), `experience`, `level` (the town growth script's state) |
  | 6 people | `count` |

  A vehicle's line, a line's stops and their station groups are read for
  the dump only; the lanes hash what the table above says. A finance read
  the engine refuses dumps as `nil`, the finance table as `err`; the
  balance is dumped all the same.

  Why lane 4 names vehicles: in the round of 2026-10-02 on `twomptest`
  (three games, no input, `136d775`), cat's game alone said `Diverged {
  step: 36400, lanes: [4] }` after 36,350 steps in step. The other lanes,
  vehicles and people among them, agreed. Its balance was 310 above the
  others' at steps 36450 and 36500 alike (37969976 against 37969666;
  37992005 against 37991695): one booking of 310 between the checkpoints
  after steps 36350 and 36400, the balance alike before and after it. The
  vehicles lane hashes where the vehicles are, not what they carry, and
  income is the game's `cargo_income.script.lua` over the distance a
  unit or passenger travelled; maintenance is booked per vehicle and
  construction too. The balance alone could not say whose booking split;
  each vehicle's and line's `takings` can. Every checkpoint's
  (`TPF3MP_HOOK_LANE_DUMP=4`) names the first checkpoint and the vehicle
  and line whose takings differ, and its `free` and `pending` (lane 3)
  what it carried.

After a divergence, gather each game's `hook.log` (a second player in a
Sandboxie box has its own under the box's copy of the data folder) and run:

```
grep 'lane 3 step 300' hook.log          # one game's vehicles at step 300
python tools/lane_diff.py james=james/hook.log bob=bob/hook.log cat=cat/hook.log
python tools/lane_diff.py --lane 3 --step 300 --max 50 --ignore entity a.log b.log
```

For each lane and step it pairs captures by occurrence and entries by key,
prints observed differences, and reports whether the lane's hashed summary
agrees. Missing summaries, rows or route records, a requested lane with no
rows, and a truncated checkpoint are marked incomplete; a missing entry then
does not prove an absent entity.
Repeated steps with unequal capture counts are left unpaired, since a world
reload could put the same step in a different epoch. Exit 0 means complete
aligned captures agree, 1 means an observed difference, and 2 means the
comparison is incomplete or ambiguous. `python tools/test_lane_diff.py`
tests it.

#### The town street field

On save `twomptest`, three games diverged at step 12800 in two of four
soaks. Traced down, every input, seed and try of town growth's street
developer at step 12771 was alike in all three games but one: node
261290's open pass (`TownDeveloper::Develop`, then
`StreetDeveloper::TryCandidate`) built street 325514 at 27.03 degrees in
two games and 19.72 in the third, from the same node and so the same
position-seeded turn. The turn is not where they part. After it the open
pass bends the street's end by the town's street field (`TryCandidate`,
`0x967dc8..0x967ebc`, when the town has one):

- `StreetField::At` (`0x2b76a90`, our name; `rcx` the field, `r8` the
  street's end) answers the field's two axes there: a sum over the field's
  sources (16 bytes each, `[field]..[field+8]`) of each source's axis,
  weighted by `exp(-d/100)` within the field's radius (`0x2b769a0`), and the
  axis at right angles to it. `0x2b76820` then snaps the street's direction
  to whichever axis lies within its angle, or leaves it.
- `At` caches its answer in a `std::map` at `field+0x18`, keyed by the
  point's 50 m cell (`round(x/50 + 0.5)`, likewise `y`), and answers any
  later point in that cell from the cache: the answer computed at the
  first point that asked there, not at this one.
- The field hangs off the town developer's context (`GameState+0x200`,
  `[[ctx+0x1f0]+8]+0x30`, read in `0x95a080`), one per `GameState`, and no
  lane reads the cache. Which points asked in a cell earlier, in this game
  and on whichever buffer ran those updates, decides the answer, and with
  it whether the street snaps.

**The fix** (`town-field-cache`, `crates/tpf3mp-hook/src/townfield.rs`; on
unless `TPF3MP_HOOK_TOWN_FIELD_CACHE=0`) splices `At`'s lookup at its end
test (`0x2b76b4a`, `cmp byte [r9+0x19], 0`, 5 bytes stolen; the only branch
to it is the lookup loop's `jne` at `0x2b76b1c`, to its first byte) and
points `r9`, the node found, at the map's head (`r10`, whose nil flag is
set). Every call then takes the miss path (`0x2b76b86`): the answer is
computed at the point asked, from the sources, and the engine's own insert
(`0x2b76660`) runs as before. The answer is a function of the point and the
sources alone. A head whose nil flag does not read leaves that lookup to
the game, said once in `hook.log`; a panic switches the fix off. Before
splicing, the site must lie at `At+0xba` and its `jne` reach `At+0xf6`; the
static proof checks that the open pass calls `At` (`0x967e0e`) and that the
miss path inserts through `0x2b76660`. `hook.log` says

```
order fix town-field-cache: installed (at 0x..., the town street field is computed at every point asked, never answered from its per-cell cache)
order fix town-field-cache: alive, calls=<n> cache-entries-passed=<n> refused=<n>
```

the second at the first call and every 4,096th; `cache-entries-passed`
counts the lookups that found a cached answer and were made to compute
instead. Every game of a room must run it alike: a game with it off builds
some town streets at other angles than one with it on.
#### The town trace

`TPF3MP_HOOK_TOWN_TRACE=1` (or `on`; off unless set; logging only,
`crates/tpf3mp-hook/src/towntrace.rs`). Round of 2026-10-02 on
`twomptest`: one town street (entity 325514, a `town_old_small` dead end)
was built at another angle in one of three games between steps 12750 and
12800, with `tickCount`, `updateCount` and so the town developer's seed
equal in all three. The trace tells apart three readings: the size
factors the `town_growth` script sends differ; the developer's context at
`GameState+0x200`, which `GameState::Replicate` (`0x255de0`) does not
copy, differs with the frames' batching; or the developer read world
input the lanes do not hash.

`TownUpdateSize::Apply` (`0x9dfb10`, our name) applies the script's
`makeTownUpdateSizeCmd`: it writes the size factors into the town, seeds
a `minstd_rand` from an FNV-1a of `updateCount` and calls
`TownDeveloper::Develop` (`0x8dc240`) with `GameState+0x200` and the
generator. Two splices in the applier (`+0x17a`, right before the call
is set up, the seed in `ecx`; `+0x1c7`, at the return, the generator after
`Develop` still in the frame) say one line a call; a splice at `Develop`'s
entry says one line a call from any caller:

```
town: step <s> update <n> town <e> size <hex>,<hex>,<hex> (<f>,<f>,<f>) flag <0|1> seed <n> (expected <n>) gamestate <ptr> buffer <0|1> engine <ptr> developer <ptr> developer-engine <own|other(buffer n)|ptr> gen-after <n> entity-ids <before>-><after>
town: step <s> develop from +<call rva> town <e> flag <0|1> gen <n> developer <ptr> engine <ptr>
```

The size factors are the command's raw float bits, then their values.
`expected` is the seed recomputed from `updateCount` (`towntrace::
town_seed`). `buffer` numbers the `GameState`s in the order this game first
saw them; `developer-engine` names the engine the developer's context holds
at `+0xb0` (where `Develop` reads the town): this update's, the other
buffer's, or another. `gen-after` is the generator after `Develop`: equal
seeds and different `gen-after` mean `Develop` drew a different number of
times, so its input differed. `entity-ids` is the length of the engine's
entity table before and after. `step` is `-` outside a released update,
`update` `?` when unread. Pointers differ between games by nature; the
other fields of one step's lines must agree.

Before splicing, the two sites must lie at their offsets from the
applier's start, the call between them must reach `Develop`, and each
site's bytes must be the expected ones (the profile states them; the static
proof checks the call and that every target resolves uniquely); otherwise
nothing is spliced and `hook.log` says why (`town trace: ... not traced,
...`). A panic switches the trace off.

The switch also dumps the towns lane at every checkpoint (lane dumps
above, even with `TPF3MP_HOOK_LANE_DUMP=off`): one line a town,

```
lane 5 step <s> town-<id> buildings=<n> size=<f1>,<f2>,<f3> experience=<n> level=<n> entity=<e> row=<row>
```

the size factors at full precision (`%.17g`) from the `TOWN` component,
experience and level from the base game's town growth script's state, read
as its own `town_cargo_util.getTownCargoState` reads it (`nil` when it
cannot be read). `tools/lane_diff.py` diffs them like any lane.

#### The edge watch

`TPF3MP_HOOK_EDGE_WATCH=<entities>` (`325514,220468`, edges or nodes, at
most 32) with `TPF3MP_HOOK_EDGE_WATCH_STEPS=<from>-<to>` (both included;
unset, every step). Off unless the first is set; logging only
(`crates/tpf3mp-hook/src/edgewatch.rs`). The round of 2026-10-02 on
`twomptest` found the town trace's town updates alike in every game while
an existing town street (entity 325514, a `town_old_small` dead end) had its
free end moved between steps 12750 and 12800, to (-2356.4, -20690.8) in some
runs and games and (-2360.9, -20680.5) in others. The watch finds the update
that moves it and what was applied then.

**Each update** in the window, the mod's game script asks the hook which
entities to read (`edgewatch()` in its `update`, which then always hands
`postUpdate` its work), and at the end of its `postUpdate` reads each with
`lanes.watch` (`tpf3mp/lanes.lua`) and hands the text over
(`edgewatched(entity, text)`). The hook logs it the first time and whenever
it differs from the last:

```
edge watch: step <s> update <n> entity <e> first|changed edge node0=<n> node1=<n> p0=<x,y,z> p1=<x,y,z> t0=<x,y,z> t1=<x,y,z> n0=<x,y,z> n1=<x,y,z> type=<t> template=<t>
edge watch: step <s> update <n> entity <e> first|changed node pos=<x,y,z>
edge watch: step <s> update <n> entity <e> first|changed absent
```

`p0`/`p1` and `t0`/`t1` are the `BASE_EDGE`'s ends and tangents, `n0`/`n1`
its nodes' `BASE_NODE` positions, all at full precision (`%.17g`); `update`
is the game's `updateCount`. A change in update `n` happened after the
watch's read in update `n-1` (after every system and every game script's
`postUpdate` the mod's ran after) and before its read in `n`.

**Every command applied** in the window, from any path, one line, by two
logging-only splices in `CommandApply::One` (`0x9e1c10`, our name;
"Simulation Thread: Apply Command", `apply_command.cpp`; `rcx` the
`GameState`, `rdx` the `Command`): at its entry, whose return address names
the path, and at its epilogue after the cookie check (`+0x352`), which both
ways out reach:

```
apply: step <s>|after <s> update <n> from +<rva> (<path>) kind <k> result <r> entities <n> [<e>,...] -> <n> [<e>,...] entity-ids <before>-><after>[ watched]
```

- `step <s>`: applied inside the room's update of step `s`; `after <s>`:
  between updates, after the batch that ended at step `s`.
- `from` and the path: `queue` (`+0x11eb96`), the commands
  `CGame::RunGameSimLoop` drains from `CommandList` between updates, which
  is where everything the GUI queues lands (the street builder's
  proposals among them), at whatever step the drain falls on; `script`
  (`+0x1204bf`), a script's `sendCommand` applied at once by the send
  lambda `0x120410`; `direct` (`+0x120334`), CGame's other send lambda;
  `other`, any other site. In a game script's `update` the send lambda
  does not apply a command: it appends it to that script's buffer
  (`GameScriptSystem::Update` points a thread-local at it, `0xaad5f0`),
  and the buffers are applied later through `GameState`'s command
  function (`0x268ed0`), a tail jump into `One`, so those lines name the
  site that called that function.
- `kind`: the payload's variant index, the byte at `payload+0x9b8` that
  `One` hands the dispatcher (`0x9d7350`), which switches on `kind + 1`
  (its jump table at `0x9d8bbc`; entry 0 is the empty variant). The kinds
  the round of 2026-10-02 saw, named by each case's scope string: 9
  `EntitySetEmissions`, 16 `GameSetCloudCoverage`, 17 `GameSetSpeed`, 23
  `LogBookValue`, 27 `ScriptingSendEvent`, 40 `TownUpdateSize` (the
  applier `0x9dfb10` the town trace splices), 52 `WorldBuildProposal`.
- `result`: the byte `One` leaves at `Command+0x30`.
- `entities`: the command's entity list (`Command+8`, 16-byte entries, the
  id in the first four bytes), its length and up to 8 ids, before and after
  the apply. `watched` marks a line that lists a watched entity.
- `entity-ids`: the length of the engine's entity table before and after
  (more after: the command made entities).

`step`, `update` and `from` of a line are `-`, `?` and `?` when unread.
Before splicing, the epilogue site must lie at `One+0x352`, `One+0xaf` must
be the kind's read (`movsx rcx, byte [r8+0x9b8]`), and each site's bytes the
expected ones (the profile states them; the static proof checks the four
paths reach `One` and the dispatcher's call); otherwise nothing is spliced,
`hook.log` says why, and the edges are still watched. A panic switches the
watch off. Calls nest (a command's apply can apply another): each entry is
paired with its own epilogue by the stack pointer.

**The street builder's pool.** `StreetBuilderPool` is the GUI tool's: it
is made in `UI::StreetBuilder`'s constructor (`0x56a740`, with
`action-streetbuilder`), as `TrackModifierPool` is in the track modifier's,
and computes the player's proposals. Their results reach the world only as
commands the GUI queues with `CommandList::Add`, which `One` applies from the
`queue` path above, so the `apply:` lines are where such a job lands. The
simulation's own pool work (`TownSystem::Update2`, `TransportNetworkSystem::
Update2`, the street shape factories, ...) is `ThreadPool::Enqueue` returning
a `JoiningFuture`, joined within the call that made it (read from the
lambdas' RTTI names, not traced), so none of it lands on a later step; no
separate hook is added for it.

For the round after 2026-10-02 (the edge, its two neighbours, and a box
around them):

```
TPF3MP_HOOK_EDGE_WATCH=325514,220468,261291
TPF3MP_HOOK_EDGE_WATCH_STEPS=12700-12850
TPF3MP_HOOK_LANE_DUMP_BOX=-2460,-20790,-2260,-20580
TPF3MP_HOOK_LANE_DUMP_BOX_STEPS=12700-12850
```

then, from each game's `hook.log`, `grep -E '^\[[0-9]+\] (edge watch|apply):'`
and compare the games line by line by `step` and `update`.

`GameSetSpeed` (kind 17) writes the `GameSpeed` component's speed on the
game's entity (`0xbacc90`, `+4`) and nothing else. In the third round only
the host applied it, 1,654 times from the `queue` path between steps 12700
and 12850 (about eleven an update), each with no entities and result 1;
the guests none. The simulation does not read that speed in the room's
game: `GameSim::Step` asks it through `CGameTime::GetSpeed`, which the step
gate redirects, and the other readers are `CGame::Sync`, the UI, the
camera, the rail vehicles' sounds and their transformators. So it changes
no lane; it does leave the host's saved `GameSpeed` unlike the guests'.

#### The street trace

`TPF3MP_HOOK_STREET_TRACE=1` (or `on`; off unless set; logging only,
`crates/tpf3mp-hook/src/streettrace.rs`), with
`TPF3MP_HOOK_STREET_TRACE_STEPS=<from>-<to>` (both included; unset, every
step) and `TPF3MP_HOOK_STREET_TRACE_BOX=x0,y0,x1,y1` (the tried node's x
and y, metres; unset, anywhere). The rounds of 2026-10-02 found the street
the edge watch watches (325514, the id reused) built new at step 12771
(update 105104) by town 214465's `TownUpdateSize`, from node 261290 out of a
straight street through it, 88 m long in every game, but at a right angle
to that street in some games ((-2360.9, -20680.5)) and 7.3 degrees off in
the others ((-2356.4, -20690.8)), with the update's inputs, the developer's
generator after it and every lane before it alike.

What decides the angle, read with `tools/tpfre`:

- `TownDeveloper::Develop` (`0x8dc240`) calls the street step (`0x967720`,
  our name) until it builds nothing. The step lists the town's street
  nodes (an octree query, `0x965fa0`, sorted by distance to the town's
  centre with `std::sort`, `0x964930`), and tries each with
  `StreetDeveloper::TryCandidate` (`0x967920`, our name) in two passes:
  **block**, then **open**. The first street built ends the step.
- `TryCandidate` expands the node into directions (`Expand`, `0x968130`:
  along and at right angles to each street at the node, 88 m, the two
  right angles in an order a `minstd_rand` seeded from the node's position
  picks). The block pass takes a direction only if it points into a closed
  street loop around the node (`StreetLoopFactory::Extract`, `0x8d9180`, at
  most 50 streets a loop, cached per step), and builds it exactly; the open
  pass takes only the others, turned by a random angle from the same
  position-seeded generator (`0x967d23`). Neither pass draws from the
  developer's generator, so `gen-after` cannot see this choice.
- So the right-angle street is the block pass's, and the turned one the
  open pass's: where the street came out turned, the block pass refused the
  node's right-angle direction, or never reached it.

Every refusal goes into a `std::set` (`0x963320`; key: the node, its
streets, the pass, its loops, the direction's index) from one of six slots
of `TryCandidate`'s frame, and the slot names the reason. The trace logs,
for the tries its filter covers:

```
street: step <s> try <k> node <e> at <x>,<y> pass block|open -> built dir <dx>,<dy> end <x>,<y>
street: step <s> try <k> node <e> at <x>,<y> pass block|open -> no
street: step <s> try <k> reject node <e> edges <n> loops <n> dir <i> pass block|open reason <reason>
street: step <s> try <k> proposal errors <bytes>/<bytes> [<hex>] [<hex>]
```

| reason | refused by |
|---|---|
| `branches` | `0x966470` (the branches' angles, `CheckBranchesRec`) |
| `not-in-block` | the block pass: the direction leaves every closed loop |
| `in-block` | the open pass: the direction enters a closed loop |
| `off-map` | `0x388e60`: the end is off the terrain |
| `snapped` | `0x9689c0`, for an end the block pass snapped to a street |
| `build` | `0x9692c0`: water at the end, the proposal's checks (`0x2638c00`, `0x2639860`) or its errors |

`try <k>` numbers the tries within a step, so two games' lines pair up by
step and number; the first line that differs is the decision that split.
A `proposal` line (from a splice in `0x9657c0` right after
`CreateProposalData`, `0xa1fd10`) gives the proposal's two error vectors'
lengths in bytes and first 32 bytes: both empty builds the street. A
`build` refusal without a `proposal` line before it failed before the
proposal was made. Differing `loops` say the street loops around the node
were found differently; a `build` refusal in one game only, with errors,
says the proposal's collision or terrain checks answered differently,
which points at the thread pool's collision jobs
(`street_util::CheckCollision`'s `ThreadPool::LoopImpl`, `0x2631ba0`).

Before splicing, the try's return site must lie at `TryCandidate+0x7bc`,
`TryCandidate`'s three calls of the reject function must reach it, and
each site's bytes must be the expected ones (the profile states them; the
static proof checks the call chain from `Develop` to every site); otherwise
nothing is spliced and `hook.log` says why. A panic switches the trace off.

For the next round, with the town trace and the edge watch as before; no
box, so every try of the developments around the split is logged (an
order of tries that differs elsewhere in the town shows too):

```
TPF3MP_HOOK_TOWN_TRACE=1
TPF3MP_HOOK_STREET_TRACE=1
TPF3MP_HOOK_STREET_TRACE_STEPS=12740-12775
```

then `grep -E '^\[[0-9]+\] (street|town):'` in each game's `hook.log` and
compare by step and try number.

#### The road entry trace

Soak 7 of 2026-10-02 on `twomptest` (three games, no input, `51e1e4d`, each
with `TPF3MP_HOOK_LANE_DUMP=4`): cat's game alone said `Diverged { step:
25950, lanes: [3] }`, the economy equal at every checkpoint to the end.
At step 26000 one horse bus, vehicle-11 (entity 198452, line-10, en route to
stop 2, a road vehicle: never in the land claim loop, whose six contenders
are the save's trains), was 9.83 m along its path edge in cat's game and
10.45 m in the others', at the same 4.276 m/s; at 26050 53.25 against 54.12
m, and a second bus, vehicle-40 (282046, line-12), which stood at stop 5 in
every game at 26000, 28.00 m from it against 30.83 m at 4.722 m/s. At 200 ms
an update (`GAME_TIME` 10,000 ms per 50 steps) the first lag is about 0.15 s,
less than an update: the bus was held back for part of one (braking behind
something, or its stop time ended later), not started a whole update late;
the second left its stop about 0.6 s, three updates, later. Every order fix
said `refused=0` throughout; the land-vehicle samples and the `ticks:`
lines were equal in all three games. The `vehicles-at-stop-order` count of
lists it reordered differs from cat's second `alive` line on (65,536 calls
in), with no split for 25,000 steps: those lists' engine order is each
buffer's history, which the fix sorts away, so that count is not one games
must agree on. The road fix's in-step milestone line is: it was equal in
all three games for 77 milestones and differed in cat's at the one reached
near step 25850 (`in-step appends=5111808 reordered=1383817`, the others
`1383824`), while every vehicle's place still agreed to the centimetre at
the checkpoint of step 25900. So the simulation's own appends to
`EdgeUseManager`'s edge lists went another way somewhere in steps
25520..25850, before any vehicle's place did. (Earlier rounds' logs show
the same: the step-3300 splits' games parted at the milestone near step
3300. Soak 6's lane-4 split at step 36400 shows no such difference through
the milestones after it.)

A milestone comes every 65,536 appends, about 330 steps. The trace
(`crates/tpf3mp-hook/src/roadtrace.rs`, logging only, on the road fix's two
detours, so nothing new is hooked) narrows it:

- **At every checkpoint**, when `TPF3MP_HOOK_ROAD_ENTRY_DIGEST=1`, a trace window or recorder is enabled, after the `ticks:`
  line, the appends inside the game's step since the last checkpoint line:

  ```
  road-entry: step <s>: in-step appends=<n> persons=<n> vehicles=<n> reordered=<n> set=<fnv64> sequence=<fnv64>
  ```

  `persons` are `Add`'s (`PersonMoveSystem`), `vehicles` `AddRange`'s
  (`LandVehicleMoveSystem`). Each append is hashed over its kind, entity,
  component, current path index, range, `{back, front}` bits and each
  edge's entity, index and direction byte; `set` sums the hashes (equal
  whatever their order), `sequence` chains them in order. Two games' lines
  for one step must be equal; the first that differs bounds the first
  append that differs to 50 steps, and `set` against `sequence` says
  whether different appends were made or the same ones in another order.
  The first line after a load also holds the load's own appends.
- **In a window**, `TPF3MP_HOOK_ROAD_ENTRY_TRACE=<from>-<to>` (room steps,
  both included), one line per append inside the step:

  ```
  road: step <s> #<k> person|vehicle <entity> comp <c> cur <i>|- range <from>..<to>|- bounds <back>,<front> edges <n> <entity>/<index>/<dir>,... reordered <n>[ on <edge>: <count> <entity>:<component>:<back>:<front> ...]
  ```

  `#k` numbers the appends of a step, so two games' lines pair by step and
  number; at most 8 edges are listed (`+n` more). `step` is the room's step
  of the update running; an append made before an update's systems run
  (applying a command) carries the step before, or `-` at a batch's first
  update. About 200 appends a step on `twomptest`.
- **A recorder**, `TPF3MP_HOOK_ROAD_ENTRY_RECORD=<n>` (1 to 4000 steps):
  the same lines for the last `n` steps are kept in memory (at most 100,000
  lines and 64 MiB of text; oldest lines are dropped with an explicit
  truncation marker) and written when
  the game takes a lane dump asked for in the room's chat (every game hears
  the ask a diverged game makes, itself included), after a line
  `road-entry record: <n> in-step append(s) of steps <a> to <b> (...)`. A
  split no one can predict then leaves the appends before it in every
  game's log. A game with `TPF3MP_HOOK_LANE_DUMP=off` takes no ask and
  writes nothing.
- For the entities `TPF3MP_HOOK_WATCH_ENTITIES` lists (comma-separated
  entity ids), a traced or
  recorded append also lists, for each edge it touched, the edge's entries
  after the sort (`on <edge>:`, at most 16): the vehicles and persons ahead
  of and behind a watched vehicle, and their places.

All road tracing is off by default; ordinary play does not allocate or hash
these diagnostic records. hook.log says what it read at start-up (`road-entry trace: ...`).

For the round after soak 7, in every game:

```
TPF3MP_HOOK_LANE_DUMP=3,4
TPF3MP_HOOK_ROAD_ENTRY_RECORD=600
TPF3MP_HOOK_ROAD_ENTRY_TRACE=25500-25960
TPF3MP_HOOK_WATCH_ENTITIES=198452,282046
```

Runs of this save are repeatable: bob's soak-6 milestones and every game's
soak-7 milestones are equal through step 25500. If cat's game splits at
the same place, the window holds it; if elsewhere, the checkpoint lines
name the 50 steps and the recorder holds them. Then
`grep -E '^\[[0-9]+\] road-entry: ' hook.log` in each game, and from the
first step whose line differs, `grep -E '^\[[0-9]+\] road: step <s> '`,
compared by step and number: the first append that differs names the
person or vehicle, its edges and range, and with the watched vehicles'
`on` lists what held the bus back.

### Seeds, as built

Ported onto dev from `feat/steam-hook-on-dev` (971c48c, as merged in
87f6b05). The targets are resolved by dev's profile resolution and handed
over at their addresses in the process. Piece 1 has run in the game
(2026-09-30, three games of one room, below) and was rebuilt from what that
showed; what follows marks what is tested only off the game.

**The game scripts' `math.random` is never reseeded through a Lua state the
hook remembers** (changed twice on 2026-09-30). The first cut called
`math.randomseed` from `ecs::Engine::Update` in every state its registrar
detour had marked, assuming two per world. A world load makes and frees
many, one registration group per loader thread; after a rebase the roster
held two freed states, and the next reseed crashed the game in
`lua_getfield`. The second had the mod's game script seed its own state at
the start of its `update` (`tpf3mp_native.seed`, `seeds::current_seed`):
safe, but the game runs its game scripts on a pool of states (piece 1), so
that seeded only the state the mod's script ran in that update, not the one
`reforestation.script.tl` ran in. Now the hook reseeds each script call in
the state the engine hands that call, on the thread about to run it (piece
1). The registrar detour is gone; the mod's own call stays and adds
nothing.

`crates/tpf3mp-hook/src/seeds.rs` is TF3's counterpart for the seeds the
survey (`investigation/TPF3_RNG_2026-09-29.md`, items 5 to 7) found are
not functions of the room's state. Three independent pieces, each failing
closed on its own with its reason in `hook.log` (every line starts with
`seeds:`), installed by `install.rs::install_inner` after the step gate,
the build tools and the main menu's load. A fourth, `ticks.rs`, keeps the
frame counter one of the engine's own seeds reads equal in every game
(piece 4 below).
All three are Windows x64 only, like the step gate.

**1. The game scripts' `math.random`, reseeded per call (live).** The
engine's `math.random` is a `boost::mt19937` per `lua::State`, seeded 5489
at creation and never saved (`reforestation.script.tl:54` draws its
proposal seed from it every update, `exploration_plane.script.tl` its
targets). The game runs its game scripts on a **pool** of such states, one
per worker thread of its job pool: `GameScriptSystem` takes the engine's
shared pool when the machine has eight or more hardware threads, and makes
its own "GameScriptSystem Pool" of half of them only below eight
(`0xaaeb80`). So which state, and so which stream, a script's `update`
draws from depends on how that update's jobs were scheduled. Measured on
2026-09-30, three games of one room that loaded one save (`real67`, the
entity trace): every allocation matched for the first 392, then
`reforestation` planted its first trees at steps 360, 360 and 364, and
every build after that at other steps in each game. The first cut of this
piece reseeded the states `CGame::CGame::lambda_3` registers, from the
simulation thread before each update; in the game that registrar runs 16
to 38 times per load, from the loader's and the pool's threads, and again
while the world runs, so the two states it kept were a different pair in
each game, and the reseed called into states other threads could be
running. It is gone. The hook now reseeds per **call**, in the state that
runs it:

- *Where.* The engine calls a script's callback through a functor that is
  handed the state it runs in, on the thread that runs it, right before
  the Lua function is called in that state
  (`game_script_util::UseFunctionAndCallScriptCreateParamFn`: the functor
  builds the `GameScriptStateNotificationHelper` the script gets as its
  `state`). Three profile targets:
  `game_script_util::Update/lambda_1::_Do_call` (`0xf45450`, `(this,
  lua::State*& rdx, GameScriptData& r8)`, the entity at `this+0x20`),
  `game_script_util::PostUpdate/lambda_1::_Do_call` (`0xf449b0`, the same
  shape, the entity at `this+0x18`; the same code serves
  `HandleApplyCommandBuildProposal`'s functor) and
  `game_script_util::HandleEvent/lambda_1/lambda_1::operator()`
  (`0xf41770`, `(captures, lua::State* rdx, GameScriptData& r8)`, the
  entity at `captures+0x20`). Each offset is where the engine's own code
  reads the entity in that function. The detour reads the `lua_State*` at
  `lua::State+0` and the entity, both through readable checks, and calls
  `math.randomseed(seed)` in that state, then the original runs.
- *Events with a seed.* `HandleEvent` takes an `int const*` seed; when it
  is set (`captures+0x28`), the engine itself calls `lua::State::
  RandomSeed` (`0x2fb17c0`: `math.randomseed(n)` in the state) before the
  script's `handleEvent`. Such an event keeps the engine's seed.
- *The step.* `GameSim::Step`'s update loop applies the update's commands
  and then calls `ecs::Engine::Update(engine, float dt)` (`0x2bb8a50`, its
  only caller, at `0x15955c`), which runs every system's `Update`,
  `ecs::GameScriptSystem::Update` among them. The hook detours
  `ecs::Engine::Update` and makes that update's room step the current one
  (`seeds::current_step`), which the worker threads read when the
  update's scripts run. It is **per update, not per call of the step**:
  one call runs a batch of released steps, batch sizes differ per
  machine, and a seed per batch would itself diverge the replicas. The
  driver arms the batch (`step::StepDriver::on_step` calls
  `seeds::before_updates(next_step, updates)` right before running the
  game's step, which also clears the current step), and the detour takes
  one step per call while the batch lasts. An update beyond the batch, or
  one before any batch is armed (before the room's world, at the game's own
  speed, on the paused path), has no current step, and its script calls
  are not reseeded: the game's own randomness, as outside a room. The next
  arm logs a mismatch between the calls seen and the updates released,
  once, as the sign the "one `Engine::Update` per update" reading is wrong.
- *The seed.* `seeds::script_seed(step, call, entity)` =
  `seed_for(step, script_salt(call, entity))`; `seed_for(step, salt)` is
  `splitmix64(step ^ (salt << 32))` folded to `1..=0x7fff_ffff`, and the
  salt mixes the call's kind (`"updt"`, `"post"`, `"evnt"`) and the
  script's entity, which every replica shares (one save, one numbering);
  never the state or the thread, which differ per machine. So a script
  draws the same numbers at the same step on every game, whichever state
  runs it and whatever ran in that state before. Pinned in tests: step 1
  gives 1216681719 for salt 0, and the update of entity 5023 at step 360
  gets 1108749812.
- *The call.* `math.randomseed(seed)` through Lua C API pointers the
  seeds module resolves from the profile itself (`seeds::SeedApi`: the
  link to the mod calls no Lua code, so its API has no `pcall`):
  `lua_rawgeti(REGISTRY, LUA_RIDX_GLOBALS)` for the globals table in 5.2,
  `lua_getfield` for `math` and `randomseed`, `lua_pushnumber`,
  `lua_pcallk`, stack restored after; a profile without one of them leaves
  the reseed off, and `hook.log` names it. Not the engine's `lua::State::
  RandomSeed`, which calls without protection. A call where
  `math.randomseed` is missing or raises runs unreseeded, said once in the
  log (the same failure on every replica, from the same game code).
- *The detours* are assembly thunks (`#[unsafe(naked)]`): they save the
  four argument registers and `xmm0`-`xmm3`, call the Rust side, restore
  and jump to the trampoline with the stack untouched, so no target's
  signature is assumed; before the trampoline is published a thunk returns
  to the caller without running the original.

**2. `TownDevelopAt` (gated off).** The applier (`TownDevelopAt::Apply`,
`0x9dedf0`) seeds its town developer's `minstd_rand` from the CRT `rand()`,
which the game seeds from the wall clock. The player's
`makeTownDevelopAtCmd` is refused in a room already (the mod's `guard.lua`,
"The player's commands"). The alternative for
later is a thunk on the applier that calls the CRT's `srand` (resolved
with `GetProcAddress` from `api-ms-win-crt-utility-l1-1-0.dll`, then
`ucrtbase.dll`: the UCRT keeps `rand()`'s state per thread, and the
applier's thread is the caller's) with `seed_for(step, "town" + n)`, `n`
the command's number within the step, right before the original. It is
behind `seeds::TOWN_DEVELOP_RESEED = false`: flip it once a room has
shown in the `t` lane that a reseeded `TownDevelopAt` develops the same
town everywhere. Whether `srand` resolved is logged either way.

**3. The CRT math dispatch (measurement).** The UCRT picks FMA3 or SSE2
bodies for `sinf`, `cosf`, `tanf`, `asinf`, `acosf`, `atan2f`, `expf`,
`logf`, `powf`, `fmodf` and the double `sin`, `cos`, `tan`, `exp`, `log`,
`pow`, `atan2`, `fmod` at start-up from `__isa_available` (`sqrtf`,
`floorf`, `ceilf` are exact either way). At bootstrap the hook logs one
`cpu:` line: vendor, family/model/stepping, the `cpuid` bits the CRT's rule
reads (SSE4.2, FMA, MOVBE, AVX, F16C, BMI1, AVX2, BMI2, AVX-512 F/CD/BW/DQ/
VL), `XCR0`, the `__isa_available` level those bits imply by the published
rule (an estimate, not a read of the variable: it is not exported and the
survey named no target for it), and whether the FMA3 bodies are expected
(level 4, AVX2, and up). **The plan:** run the two-replica determinism
probe (DAY_ONE section 4, `tools/probe/tf3`) once on an Intel and an AMD
machine whose `cpu:` lines differ in the FMA3 answer, and once on two
machines whose lines agree. A split that appears only in the first pair,
in the `p` (vehicle positions) and `e` (edge geometry) lanes first (the
movement and path code is where `sinf`/`cosf`/`atan2f` run every step)
and later in `n` and `t`, is the CRT; the same split in the second pair is
not. If it is the CRT, the fix is a detour of those imports to one body
(the survey's known technique, not built).

**4. The paused frames' `tickCount` (`paused-tick`, live; not run in the
game yet).** `crates/tpf3mp-hook/src/ticks.rs`; the findings are in
[investigation/TPF3_TRAIN_PRIORITY_2026-09-30.md](../investigation/TPF3_TRAIN_PRIORITY_2026-09-30.md).
`GameTime.tickCount` (`+0x3c`) counts every update and also every call of
`GameSim::Step` on its paused path; `updateCount` (`+0x40`) counts updates
only (the API's own words, `engine.d.tl:434`). The paused path (the speed
call answered 0) calls the GameTime advance (`0xbace10`, `CGameTime::Advance`,
name ours) with `r8b = 0` at `0x159412`; the advance runs `inc [+0x3c]`
always and `inc [+0x40]` only when `r8b` is set (`0xbace99`). The room's
game takes that path whenever the room holds the world, and during every
room `Load` and `Save`, a machine-dependent number of times, so without
the fix every hold leaves the games' `tickCount` apart for good. The
simulation reads it in the land-vehicle reservation shuffle's seed
(`0xac1b23`: which train or road vehicle gets contested track first), in
`AccountSystem::Update2` (`tickCount % n`), in the town developer's and
street proposals' stamps, and in the base game's notifications game
script (`tickCount % 30` picks which notifications' sim scripts update).
TPF2 Multiplayer made the same call a NOP (`156824d`, `pausedtick`).

- *The patch.* The profile target `GameSim::Step/paused GameTime advance`
  (the call's `E8`, found by the eleven instructions around it, one match)
  is redirected with `CallRedirect`, which refuses unless the call reaches
  the target `CGameTime::Advance`; the fix also refuses unless
  `CGameTime::Advance/tick` (the two increments' eleven bytes) sits `0x89`
  into the advance and reads back as expected. Any miss leaves the call
  alone and says why (`paused-tick fix: off, ...`). The redirect's
  function holds the advance only when the step detour said this call is
  the room's game's (`step::Batch::room`: the driver follows a room's game
  or holds it) and the call is a paused one (`r8b` clear); otherwise it
  calls the game's own advance with the registers the step passed. So a
  game outside a room pauses exactly as before, and the running loop's
  call (`0x15954b`, `r8b = 1`) is never touched. A redirect rather than
  TPF2's NOP, for that reason: the same process can play alone and in a
  room.
- *What skipping drops* (read in the binary): the advance opens an engine
  modification scope (`0x2bbbba0`/`0x2bbc060`) around the increments and
  records a `ComponentChanged` for `GameTime` into each observer's change
  log (`0x2bb6b50`); a held frame records none, which is true. The UI
  readers of `tickCount` stand still while the room holds: the
  notifications script refreshes one of its four type groups per paused
  frame by `tickCount % 4`, and the industry window rate-limits its
  expansion preview by `tickCount`. The town and street builder tools stamp
  their proposals with it; those proposals travel to the room in the
  action's bytes. The horn-sound choice (`0x26948f0`) is presentation.
- *Kill switch.* `TPF3MP_HOOK_PAUSED_TICK=0` (or `off`) in the launcher's
  environment leaves the call alone.
- *The counters in hook.log.* At every checkpoint, and at the first batch
  after a world is loaded, the step detour logs
  `ticks: step <room step>: tickCount=<n> updateCount=<n>`, read through the
  game's getters (`CGameTime::GetTickCount` `0x2a95c0`,
  `CGameTime::GetUpdateCount` `0x2a9680`, names ours) with the `CGameTime`
  the step's own speed call was made on. Two games of one room must log
  equal lines at equal steps. The redirect itself logs
  `paused-tick: holding tickCount on the room's paused frames (held n,
  passed m)` at the first hold and every 16384th: those counts differ per
  machine by design.

**Tested without the game** (`ticks::tests`, `step::tests`): the decision
(only the room's paused frames), the kill switch, the layout check, the
checkpoint line, that nothing installs without the targets, that the
profile states the site's bytes, the driver's `room` and `first_step` on
each batch, and the redirect through a real `call` on a hand-written
function (the advance runs outside a room, is held in it, runs again
after). The static proof (`tf3_static_proof.rs`) checks on the installed
game that both calls reach the advance, with `r8b` 0 and 1, and the
increments' bytes.

**Tested without the game** (`seeds::tests`): the
seed derivation (range, purity, the pinned values, a script call's seed
by step, kind and entity), the batch arithmetic (one step per update, the
mismatch report, disarming), the current step (the released update
running, none between batches, beyond a batch or on the paused path),
which calls are reseeded (none without a step, none over an event's own
seed), the CRT level rule on synthetic CPUs and the running CPU's report,
the driver's step counter, and the reseed's call sequence against the
embedded Lua 5.1: the exact seed reaches `math.randomseed`, and a missing
`math` or a raising `randomseed` refuses with the stack restored. The
static proof checks that the three call sites resolve uniquely at their
addresses in the installed game. **Not tested without the game:** the
thunks against the real targets (the functors' layouts, that one
`ecs::Engine::Update` is one released update), the 5.2 globals path, and
whether `srand` resolves in the game's process. A room's `hook.log` says:
`seeds: detour installed on ...` for the three call sites and
`ecs::Engine::Update`, `seeds: step n: math.randomseed(...) before the
update of script entity e` for the first three calls of each kind and at
every thousandth step, and no mismatch line.

### The order fixes, as built

Industry construction inputs also require stable ordering. Build 40408's
base `industries/industryutil.lua` enumerates two string-keyed maps with
`pairs`: `generatedData` for implicit terminals, and `generatedData.lanes.curves`
for internal lanes. Different Lua hash orders can give a lane or terminal
index a different physical meaning after an industry rebuild.

The native Lua bytecode-cache lookup supplies a deferred wrapper for this
base resource, including a construction worker's first load and cache hits.
Intercepting only the loader body is insufficient: cache hits bypass it.
The wrapper uses
the game's original loader under a per-state recursion guard, then wraps
`makeIndustryUpdateFn` with `industry_order.lua`. At each update the wrapper
orders the input and then creates the original callback from that input,
avoiding reliance on metatables surviving a worker transfer. Only the two input maps
receive a sorted `__pairs` iterator; explicit terminal arrays and completed
construction results are untouched. It neither changes global `pairs` nor
writes to the game installation. The guard is cleared on load errors.
Unknown map key types or pre-existing metatables are refused. Native
validation and its current limits are recorded in
`investigation/TPF3_ROAD_TERMINALS_2026-10-01.md`.

### Automatic industry spawn probe

Steam build 40408 has a separate native demand-spawn path. For a local
diagnostic run only, set `TPF3MP_HOOK_TRACE_INDUSTRIES=1` in the launcher's
environment. The exact-build hook then records each demand callback's native
GameTime and FNV-1a seed, both target-capacity key strings and the unchanged
capacity result, candidate/resource loop order and its `industry_closed_`
gate result, and each returned industry entity ID (`-1` means no entity was
emitted). A `?` marks an input that failed the bounded memory/layout check.
The entity ID can be matched to the industry/construction lanes to inspect
the resulting placement.

This is a read-only diagnostic: it never changes native values or sends the
records to the room. The game-thread callbacks copy fixed-size records into a
64-entry queue; a separate logger thread formats and writes them. The probe
records at most 32 loop candidates per callback and 512 callbacks per game;
queue overflow is reported explicitly. Its six signatures live in a separate
compiled 40408-only diagnostic profile, leaving the release targets and the
40418 Preview coverage contract unchanged. The probe enables only on an exact
`TPF3MP_HOOK_TRACE_INDUSTRIES=1` opt-in and when every site resolves before
the ordinary hooks patch the image. Any splice mismatch rolls back the detour
and earlier splices. The capacity
detour also checks its compiled 40408 entry bytes before installation, so a
shadow profile cannot redirect that name to a function with another ABI. With
the switch unset, the probe installs nothing.

The native collection fixes below were ported with the seeds, from the
same commits. Their original static analysis follows; later real-game
results are recorded in `investigation/TPF3_ROAD_TERMINALS_2026-10-01.md`.

`crates/tpf3mp-hook/src/order.rs` ports the four order dependencies to
TF3 Steam build 40408, from the static survey
[investigation/TPF3_RNG_2026-09-29.md](../investigation/TPF3_RNG_2026-09-29.md)
("What must change for lockstep"). `install.rs::install_inner` calls
`order::install` once the step gate is in; each site is its own fix,
installed on its own and failing closed on its own: its profile targets
must resolve (all `required = false`, so a build that lacks one loses the
fix, not the profile), the bytes at the site must be exactly what the fix
expects (the resolver checks the target's `prologue`, and the splice
reads and compares them again before it writes), and every read the hook
makes on the game's thread goes through `image::readable`. A shape the
hook does not recognise is refused for that step, said once per reason in
hook.log, and the engine's own order stands; a panic on the game's thread
switches that fix off for the rest of the game. hook.log carries one
`order fix <name>: installed (...)` or `order fix <name>: off, <why>`
line per fix. Nothing here has run in the game yet: the sites were read
statically with `tools/tpfre`, the sort and stub logic is unit-tested, and
the two mid-function hooks are exercised through their real stolen bytes
on hand-written functions in the test binary.

| survey item | fix | site (RVA) | what it does |
|---|---|---|---|
| 1, land-vehicle reservation order | `land-vehicle-order` | `ecs::LandVehicleMoveSystem::Update2/shuffle` (`0xac1b70`) | sorts the vector of vehicles that want track by entity id before the engine's seeded shuffle (kill switch `TPF3MP_HOOK_LAND_VEHICLE_ORDER=0`) |
| 2, ship and aircraft claim order | `order-measure` | `EdgeReservationManager::Reserve` (`0x255c2e0`), `Reserve_simple` (`0x255c160`) | measured only, when `TPF3MP_HOOK_MEASURE_ORDER` is set |
| 3, road edge entries | `road-entry-order` | `EdgeUseManager::Add` (`0x255e940`), `AddRange` (`0x255cc70`) | keeps each edge's entries in entity order after every append (kill switch `TPF3MP_HOOK_ROAD_ENTRY_ORDER=0`) |
| 4, vehicles at a stop | `vehicles-at-stop-order` | `ecs::SimEntityAtTerminalSystem::Update/vehicles at stop` (`0xb0e35c`) | sorts the vehicles at a line stop by entity id before the boarding loop (kill switch `TPF3MP_HOOK_VEHICLES_AT_STOP_ORDER=0`) |
| 5, platform choice | `platform-order` | `ecs::TransportVehicleSystem::Update2/visit` (`0xb8bccb`), `FindNextFreeTerminal/candidate sort` (`0xb85430`) | asks the vehicles for a free platform in entity order, and puts the candidate terminals in one order before their cost sort (kill switch `TPF3MP_HOOK_PLATFORM_ORDER=0`) |
| (soak 4 of 2026-10-02), town street angles | `town-field-cache` | `StreetField::At/cache found` (`0x2b76b4a`) | the town street field is computed at every point asked, not answered from its per-50-m-cell cache ("The town street field"; kill switch `TPF3MP_HOOK_TOWN_FIELD_CACHE=0`) |
| TF2 `candidates` (person-order) | `person-candidates-order` | `destination_util::GetTargetsByLandUse/candidates` (`0x8e3d65`) | sorts the PersonCapacity candidates by entity id before their capacities are summed for the destination draw ("The person-order fixes"; kill switch `TPF3MP_HOOK_PERSON_CANDIDATES_ORDER=0`) |
| TF2 `departures` (person-order) | `person-departures-order` | `ecs::SimEntityAtBuildingSystem::Update2/leave batches` (`0xb05f92`) | sorts the persons and cargo leaving buildings by entity id before they are signalled (kill switch `TPF3MP_HOOK_PERSON_DEPARTURES_ORDER=0`) |
| TF2 `arrivals` (person-order) | `person-arrivals-order` | `ecs::PersonMoveSystem::Update2/arrival batch` (`0xaecfcd`) | sorts the walk arrivals by entity id before they are signalled (kill switch `TPF3MP_HOOK_PERSON_ARRIVALS_ORDER=0`) |
| TF2 `idle` (person-order) | `person-needs-path-order` | `ecs::SimEntityNeedsPathSystem::Update/list` (`0xb18213`) | sorts the persons waiting for a path by entity id before `PathFactory::Compute` seeds by their place (kill switch `TPF3MP_HOOK_PERSON_NEEDS_PATH_ORDER=0`) |
| TF2 `freed ids` (person-order) | `freed-id-order` | `ecs::Engine::EndModification/free-id append` (`0x2bb4fd1`) | sorts each modification's freed ids before they join the free-id queue (kill switch `TPF3MP_HOOK_FREED_ID_ORDER=0`; all five off with `TPF3MP_HOOK_PERSON_ORDER=0`, alike in every game of a room) |

**The mid-function splice** (`tpf3mp_hookcore::detour::Splice`) is what
the two fixes hook with. A whole-function detour cannot reach a point in
the middle of `Update2`, so the splice replaces `steal` bytes at the site
(whole instructions, at least five) with a `jmp rel32` into a stub
allocated near it (`alloc_near`, as `CallRedirect`'s). The stub is
hand-assembled and pure (`splice_stub`, checked byte for byte in a test):
it pushes every general-purpose register and the flags, which form a
`SavedRegs` block on the game's stack, aligns the stack, saves the
volatile `xmm0`-`xmm5`, calls the fix's `extern "system" fn(*mut
SavedRegs)` with the block, restores everything, runs the stolen bytes
verbatim and jumps back to the instruction after them with an absolute
jump. The register contract is therefore: the hook sees the site's every
register, may change one through the block (the tests do), and the
function sees nothing else changed but memory. The stolen bytes are
decoded with iced-x86 and refused if they branch, call, return, end inside
an instruction or address memory relative to `rip` (they run at another
address). The caller states the bytes it expects at the site, and nothing
may branch into the stolen bytes past their first (established with
`tpfre q xrefs` and noted in the profile). `xmm6`-`xmm15` are callee-saved,
so the hook keeps them as any function would; the upper `ymm` halves are
volatile at every call, and both sites follow a `call` with no vector
instruction between (the disassembly), so nothing lives in them there.

**The in-place rewrite** (`tpf3mp_hookcore::detour::Rewrite`) replaces
bytes one for one after checking the site holds exactly the bytes expected
(a site holding anything else is refused unwritten), and restores them when
detached or dropped. The simulation timers use it to swap a vtable slot's
pointer for the timer's ("The game's own systems: the `perf: sim` line").

**Land-vehicle reservation order** (the survey's item 1, CONFIRMED). In
`ecs::LandVehicleMoveSystem::Update2` (`0xac0f90`) the engine walks its
family's node list (20-byte records: the entity id, then four component
indices) and pushes an 8-byte `{int32 nodeIndex, float priority}` entry
for each vehicle that wants track into a vector at `[rbp-0x20]..[rbp-0x18]`
(`0xac1a50..0xac1abe`), reads `GameTime+0x3c` and folds it into a
`minstd_rand` seed (`0xac1b15..0xac1b4c`), Fisher-Yates-shuffles the
vector (`0xac1b70..0xac1c63`), **stable-sorts it by the float**
(`0xac1c6c..0xac1d62`: an insertion sort under 33 entries, otherwise
`std::stable_sort` with a temporary buffer; the survey had not seen this
step), and then reserves track in that order (`Reserve` at `0xac1f20`).
So the shuffle only decides the order among equal priorities, and it is
applied to positions in node-list order, which two replicas can hold
differently. The fix splices in at `0xac1b70`, the first instruction of
the shuffle (`mov r13, [rbp-0x20]; mov rsi, [rbp-0x18]; cmp r13, rsi`;
the first two, 8 bytes, are stolen), reads the vector through `rbp`,
reads each entry's entity id the way the engine's own reservation loop
does at `0xac1d72` (`this` at `[rbp-0x80]`, the node-list holder at
`this+8`, the records at `[holder]..[holder+8]`, the record at
`records + nodeIndex*20`, the id at its first dword; that walk is the
profile target `.../records`, and the fix refuses unless it resolves a
few hundred bytes after the site), and sorts the entries in place by that
id, each entry whole. The engine's seed, shuffle and priority sort then
run unchanged on a vector whose order is a pure function of the entity set,
so every replica shuffles the same sequence with the same seed. TPF2's
name key and seeded jitter (`trainorder.h`) are left out: the engine's own
shuffle already keeps a fixed priority from starving anyone, and TF3's ids
are lockstep state (the free-id queue is saved, HOTJOIN_ORDER.md). A
duplicate id, an index past the records, or bounds that are not whole
entries is a refusal. With `TPF3MP_HOOK_MEASURE_ORDER` set the ids in the
engine's order are hashed into the `land` lane, so two replicas' logs
show whether their node lists agreed before the sort (`reordered` counts
how often they were out of order), each call's seed into `seeds`, and
the family's whole node list, entity by entity, into `nodes` (on updates
where at least two vehicles want track): the survey's item 3 for this
family.

The sort makes the shuffle's input a function of the entity set; the
shuffle's **seed** is `GameTime.tickCount` (`0xac1b23`, then `% (2^31-1)`,
0 becomes 1), which the `paused-tick` fix above keeps equal in every game.
The splice reads it from `r8d` at the site (the seed's fix-up leaves it
there, `cmove r8d, r12d`, and the mask loop after it does not touch `r8`).
Whatever the measurement, for one seed value in 256 (about one update in
256, since the seed is the frame counter) the fix logs

```
order fix land-vehicle-order: sample seed=<seed> n=<vehicles that want track> ids=<fnv64 of their ids, sorted>
```

sampled by the seed's value, not by a count of calls, so two games that
agree write the same lines. The engine's shuffle and its priority sort
(each vehicle's line's `reservationPriority`, descending, stable) are a
function of exactly the seed, the ids and the priorities, so equal lines
at equal seeds mean an equal claim order for vehicles of equal priority.
`n` is 0 when fewer than two vehicles want track.

**Vehicles at a stop** (item 4, TPF2's `vehstop`, TERMINAL_WAIT_ORDER.md).
`ecs::SimEntityAtTerminalSystem::Update` (`0xb0db00`) asks
`TransportVehicleSystem` for the vector of vehicles standing at each
`(line, stopIndex)` (`0xb86510`, name ours: it hashes the pair into the
system data's phmap at `+0x90` and returns `&slot.vector`, or a static
empty vector) and hands the waiting cargo and people to those vehicles in
the vector's order with one running index, exactly TPF2's shape. The
vector is append order while the game runs (its owner's `EntityAdded`
adds after a `std::find`, the assert string at `0xb8433a`) and load order
after a load, so a retained and a loaded replica load two trucks at one
stop differently. The fix splices in at `0xb0e35c`, the `mov
[rsp+0x248], rax` right after the getter's call (the profile target
resolves the site by the call and the two instructions after it; the fix
also checks that the `call rel32` before the site reaches the getter
target), and sorts the `std::vector<Entity>` `rax` names by id in place.
Its `vehstop` lane hashes the order found. What is **not** ported: the
unload deques (TPF2's `unload` in `SimEntityAtVehicleSystem`), which were
not located in TF3 in this pass; measure the `vehstop` lane first.

**Ship and aircraft claim order** (item 2) is measured, not changed, for
the reason TPF2's `moveorder.inl` gives: `ShipMoveSystem::Update2`
(`0xaf6120`) and `AircraftMoveSystem::Update2` (`0xa83a40`) index the
family's node vector directly, and permuting an engine-owned list blind is
not safe. With `TPF3MP_HOOK_MEASURE_ORDER` set, both overloads of
`transport::EdgeReservationManager::Reserve(this, engine, typeIndex,
entity, &path, from, to)` (assert-named; every land, ship and aircraft
claim goes through them) are detoured whole, and each claim's entity and
the twelve bytes of each edge of `path[from..to)` go into the `claims`
lane. That lane is the direct proof for items 1 and 2 together: two
replicas whose `claims` hashes agree at every line let the same vehicles
through in the same order. If it splits with the land-vehicle fix on,
the ships and aircraft are next, and the fix is the node-list canon
(TPF2's `step` site, `family_canon.h`: every family's node list in entity
order at each `ecs::Engine::Update`, `0x2bb8a50`).

**Road edge entries** (item 3, `road-entry-order`; TPF2's `roadentries`).
TF3's `EdgeUseManager` keeps 20-byte entries per edge (`{int32 entity,
int32 component, float back, float front, bool forward}`), consumed by
the nearest-occupant searches (`0x255f340`, `0x255ef60`: `vcomiss; jbe`,
the first entry wins an exact tie), `GetNext` (`0x255f760`) and the claim
loop's first-blocked search (`0x255afd0`). The fix is **sort on add**,
because every writer of an edge's list was read and none reorders it:

| writer | what it does to the list | label |
|---|---|---|
| `Add` `0x255e940` (persons; from `PersonMoveSystem`'s node-added callback, `0xaec8e6`) | asserts the entity is not there, then `push_back` (`0x255ea6d`, or the grow path `0x1cf220`) | SEEN |
| `AddRange` `0x255cc70` (vehicles; from `LandVehicleMoveSystem`'s node-added callback via `0x255edc0`) | per path edge `from..=to`: updates the vehicle's entry in place if it has one, else `push_back` (`0x255cde9`, `0x255cedb`, `0x255cffe`) | SEEN |
| `Remove` `0x2561440` (`PersonMoveSystem` node-removed) | find, `memmove` the tail down one entry | SEEN |
| `RemoveRange` `0x2561690` (`LandVehicleMoveSystem` node-removed) | the same per path edge | SEEN |
| `RemoveEntity` `0x2561510` (`TransportNetworkSystem` node-removed) | drops a deleted edge's data whole | SEEN |
| `GetOrAddEdgeData` `0x255d5a0`/`0x255d740`, `VectorMap::Add` `0x255e860` | grow the slot and edge vectors, moving each entries vector whole | SEEN |
| the copy-on-write copy (`0x255f0b0` -> `0x255e2d0`) | copies the data, order kept | INFERRED |
| the readers (`0x255ef60`, `0x255f340`, `0x255f760`, `0x255ff70`, `0x2560270`, `0x2560ef0`, `0x2561160`, `GetPos01sDEBUG`) | read only (their callees: position getters) | SEEN |

So a list is append order (history while running, registration order
after a load, since the lists are rebuilt through the node-added
callbacks) and nothing else moves an entry; a list sorted after each
append stays sorted, with no cost per update, which a per-update sort
could not beat. The fix detours `Add` and `AddRange` whole (both profile
targets already), runs the engine's, and then sorts, by entity id, the
entries of the edges it touched: `Add`'s one edge, `AddRange`'s path
edges `from..=to` (its seventh and eighth arguments). An edge's list is
found as `GetOrAddEdgeData` finds it: from the manager's data (an
`EdgeUseManagerData`), its entity-to-slot index `[+0]..[+8]` (`int32`s),
its slots `[+0x18]..[+0x20]` (72 bytes each), the slot's edges
`[slot]..[slot+8]` (32 bytes each), the entries vector at `edge+8`; every
bound is checked and a shape that does not fit is a refusal for that edge.
The two appenders are handed the data differently: `Add`'s `this` is the
manager, whose data is at `[this+0x18]` (`Add` reads it through the
copy-on-write getter `0x255f0b0`, which answers `[this+0x18]`), while
`AddRange`'s `this` *is* the data (its one caller, `0x255edc0`, calls the
getter and passes the answer on; the manager is its ninth argument, and
the fix checks that `[manager+0x18]` names the data it was given).
Until 2026-09-30 the fix read `[this+0x18]` in `AddRange` too, which is
the data's slot vector, so every vehicle append was refused with "the
edge's entity has no slot" (the only refusal in the three-player
playtest's hook.log, about 70% of the appends) and the vehicles' lists
were never sorted; the persons' (`Add`) were.

Every read on the hot paths (this walk, the order fixes' vectors, the
game scripts' reseed) goes through `image::Readable`, which reads through
guarded reads ("Reading the game's memory: guarded reads" below): no
system call, and a misread address is refused by the hook's vectored
handler instead of crashing the game. Before those, `Readable` was a
per-thread cache of the regions `VirtualQuery` found committed and
readable, dropped (`image::invalidate`) before every simulation update,
before every call of the game's step, when a world's GUI starts and when
a load is asked for, so a region was asked of the system about once per
update, not once per word (13 times per edge before). The system call
costs about 1.7 µs on the development PC and tens of microseconds inside
Sandboxie, which hooks system calls: in the three-player playtest
(`f622599`) the boxed games spent 37 to 42% of their step time in the
hook, most of it in these checks, and even with the cache a big map's
road fix still cost about 6 µs per append (about 1.5 `VirtualQuery` calls
an append: 11 ms per update, 19% of the game's step). That cache is
still the path with guarded reads off. Every address the hook reads
comes from a structure the engine keeps live; the checks guard against a
layout the hook misreads, and dropping the cache wherever the engine
frees keeps a freed region from answering. The sort is `road::place`, in
place: one scan
finds how far the list is strictly ascending; a list kept sorted is then
either whole (nothing written) or out of order only in the entry just
appended, which is moved into place by binary search; anything else is
sorted whole through a reused buffer. It gives exactly the order of the
reference `road::entry_order` (checked on random lists in the tests),
refuses the same lists, and writes nothing when it refuses. On the
development PC one append to an edge of 2 to 32 entries went from about
5 to 8 µs to about 0.1 µs through the cache and 33 to 47 ns with guarded
reads (`order::splice_tests::road_append_bench`, which hits the cache
every time; in a game the misses were the cost; a check from the cache
is about 6 ns, a guarded one 1 to 2 ns, against 0.5 to 1.7 µs asking the
system, `image::tests::readable_bench`), the sort alone
from 94 to 31 ns at 2 entries and 508 to 117 ns at 128
(`order::tests::road_sort_bench`). Both appenders must be detoured, or none sorts. The callbacks run
serially at the end of a modification (INFERRED from their callers, the
engine's node-added dispatch), so the sort writes nothing a reader is
walking. The `AddRange` detour takes all nine arguments (the ninth at the
caller's `[rsp+0x48]`); the measurement's earlier detour forwarded eight.

**Platform choice** (`platform-order`; investigation/TPF3_TRAIN_PRIORITY_2026-09-30.md,
"Platforms"). `ecs::TransportVehicleSystem::Update2` (`0xb8bae0`) walks its
node list (8-byte `{entity, TransportVehicle index}` records at
`[[this+8]]`, as many as the update's `int` argument, spilled at
`[rbp+0x5b0]`) and, for each vehicle en route with its decision flag set,
calls `FindNextFreeTerminal` (`0xb84e20`, its one caller, `0xb8bea1`); a
choice is stored for the rest of the update in a copy of the allocation
map (`StoreTerminalAllocation` `0xb8b960`), so a vehicle visited later
sees what earlier ones took, and the path changes are applied later in
visit order (deferred lambdas). Two sites, each on its own:

- *visit* (`0xb8bccb`): the loop reloads the list's begin into `rdi`
  every iteration (`mov rax,[r13+8]; mov rdi,[rax]`) and reads only
  `[rsi+rdi]` and `[rsi+rdi+4]`; `rdi` is set anew after the loop
  (`0xb8c22f`). At the first iteration (`rsi` 0) the hook copies the
  list, checks that its length is the count, and sorts the copy by
  entity id; at every iteration it points `rdi` at the copy. The engine's
  list is never written (its index into it stays valid), and the same
  vehicles are visited, in entity order. A list already sorted is left
  alone; a list that does not match the count, or that moves during the
  loop, is refused and the engine's order stands.
- *candidates* (`0xb85430`): `FindNextFreeTerminal` gathers 12-byte
  candidates from `LineSystem`'s per-stop table and `std::sort`s them by a
  cost it looks up per candidate (`0xb85453`, a float-only comparator), so
  equal costs keep an introsort order that depends on the input's. The
  hook puts `[r13, r14)` in one canonical order first (by the
  station and terminal words, then the first), so the sort, and the
  search that starts at the current terminal and wraps around, break
  equal costs the same way in every game.

Both sites allocate nothing in the common case: the visit site scans the
list in place and copies it only when it is out of entity order, into a
buffer kept from update to update (`platform::sort_records`); the
candidates are checked and, if need be, sorted in place
(`platform::sort_candidates_in_place`). Each gives exactly the order of
the reference functions (`visit_order`, `candidate_order`), checked on
random inputs in the tests.

With `TPF3MP_HOOK_MEASURE_ORDER` set, the `visits` lane hashes the
engine's visit order before the fix each update, `candidates` counts the
candidate sorts that changed something, and `road` hashes each checked
edge's id and its entities in the order kept.

**The measurement** (`order::measure`). Off, nothing is hooked. With
`TPF3MP_HOOK_MEASURE_ORDER=1` in the launcher's environment (the game
inherits it; a number above 1 is the interval, default 100 updates),
`ecs::Engine::Update` (`0x2bb8a50`, the per-step engine advance) counts
updates and closes each one's lanes through the existing seeds hook.
If that hook is absent, a measurement detour forwards the update's `dt`
in `xmm1`. Sharing the installed hook avoids trying to detour an already
modified prologue, which previously left measurements unclosed. The two `Reserve`
overloads feed the `claims` lane. `EdgeUseManager::Add` and `AddRange`
feed `appends` from the road entry fix's detours, which install while
measuring even with that fix switched off. The fixes' sites feed the
other lanes. Every `interval` updates one line goes to hook.log:

```
order measure: updates 201..=300: claims=<fnv64>/<n> appends=<fnv64>/<n> land=<fnv64>/<calls> reordered <n> seeds=<fnv64> nodes=<fnv64>/<n> vehstop=<fnv64>/<calls> reordered <n> visits=<fnv64>/<updates> reordered <n> candidates=<reordered>/<sorts> road=<fnv64>/<edges> reordered <n>
```

Updates are numbered from the room's step once the step driver has
loaded the room's world (`step.rs` tells `measure::room_step` the next
step; before that, from the hook's start), on the survey's reading that
one `Engine::Update` call is one update (INFERRED, to be confirmed by
the numbers agreeing with the room's). Two replicas' lines can be diffed
directly; a lane that differs names the container. The hashes are FNV-1a
64 over the raw values, so an entity id that legitimately differs shows
too; TF3's ids are expected equal (the survey), and the `reordered`
counts say whether the sorts changed anything.

### Reading the game's memory: guarded reads

Every native read the hook makes on the game's threads must fail closed:
an address the hook got wrong (a layout it misread, a build that moved a
field) is refused, never a crash. `crates/tpf3mp-hook/src/image/guarded.rs`
does this without asking the system first:

- **Two routines** in assembly do every hot-path read: `touch(first,
  last)` reads one byte of each page a range spans (the check behind
  `Readable::readable` and `image::readable_cached`), `copy(dst, src,
  len)` copies bytes out (behind `Readable::read`). They are leaf
  functions that use only volatile registers and never move `rsp`, so at
  any instruction in them the return address is at `[rsp]`, and they
  share one recovery stub that answers 0.
- **A vectored exception handler**, added first
  (`AddVectoredExceptionHandler(1, …)`) when the hook installs
  (`image: guarded reads on (…)` in hook.log), acts on exactly one kind
  of exception: `EXCEPTION_ACCESS_VIOLATION`,
  `STATUS_GUARD_PAGE_VIOLATION` or `EXCEPTION_IN_PAGE_ERROR` whose
  instruction pointer is inside the two routines **and** whose first
  parameter says it was a read. It moves the thread to the recovery stub
  (`EXCEPTION_CONTINUE_EXECUTION`), and the caller sees a refusal.
  Everything else, a write fault in the routines included, returns
  `EXCEPTION_CONTINUE_SEARCH` untouched: the game's own handlers and its
  crash reporter see every other fault exactly as without the hook. (The
  game's own vectored handler, also added first, at startup, acts only on
  heap corruption, `0xC0000374`; its crash reporter is a last-chance
  filter, `SetUnhandledExceptionFilter`, which a child process in the
  tests stands in for.)
- **Addresses that cannot be user memory** (below 64 KiB, which Windows
  never maps, past `0x7FFF_FFFE_FFFF`, or wrapping) are refused without
  a read, so a null pointer plus an offset costs no fault.
- **Guard pages.** Reading a `PAGE_GUARD` page clears the guard and
  raises `STATUS_GUARD_PAGE_VIOLATION` (a thread's own stack guard page
  is the kernel's to handle: it grows the stack and the read succeeds, as
  any stack use would). Another thread's stack, or a guarded heap, relies
  on that guard, so the handler puts it back (`VirtualProtect` with the
  page's protection plus `PAGE_GUARD`) before refusing: the same answer
  the `VirtualQuery` check gave, with the guard kept. Between the system
  clearing it and the handler re-arming it are a few microseconds, in
  which a second read of the same page would succeed (it is committed
  memory, so that read is safe too); only a misread pointer reaches a
  guard page at all.
- **Writes.** A read proves nothing about writing, and neither did the
  `VirtualQuery` check (it refused only no-access and guard pages, not
  read-only ones). The fixes that sort in place (the road fix, the order
  and person-order fixes) write through memory a check found readable,
  as before: a vector the engine itself just wrote, on the engine's
  thread, so the guarantee is the same as it was.
- **Cost.** A check of readable memory, or a check and an 8-byte read,
  is 1 to 2 ns, against 6 to 8 ns from the old cache when it hits and
  0.5 to 1.7 µs (tens in Sandboxie) when it asks `VirtualQuery`
  (`image::tests::readable_bench`, release build, development PC). A
  refused read costs one exception dispatch, 1.2 to 1.8 µs; refusals mean
  a misread layout, so they are rare, and the `perf:` line counts them.
- **The kill switch** `TPF3MP_HOOK_GUARDED_READS=0` (or `off`) in the
  game's environment, a handler the system refuses, or a build that is
  not x86-64 Windows: the hook checks with `VirtualQuery` through the
  region caches, as before (`image: guarded reads off (…)` or
  `… unavailable …` in hook.log). Elsewhere than Windows nothing native
  is read, as before.
- The handler is never removed: the hook stays loaded for the life of
  the game. The cold paths (code checks before a patch, install-time
  reads) still ask `VirtualQuery` through `image::readable`.

The tests (`image::guarded::windows_tests`) read committed, reserved,
freed, no-access and read-only pages, ranges that run from a committed
page into an uncommitted one and a no-access page in the middle of a
long range, a guard page (refused, and its guard still there
afterwards), sixteen threads faulting at once, a later vectored handler
that still receives a fault outside the routines and a write fault
inside them, and a child process whose stray fault still ends in its
last-chance filter.

### The person-order fixes

`crates/tpf3mp-hook/src/persons.rs` ports silver2127's TPF2 Multiplayer
person-order sorts (`tpf2-multiplayer`: `docs/re/HOTJOIN_ORDER.md`,
"Mechanism" and "The fix"; `native/src/slice/hotjoin_order.inl`; the Linux
reference `native/linux/src/order_canon_linux.cpp`) to TF3 Steam build
40408. In TF2 the person simulation read its batches in ECS node-list
order, which is add/swap-remove history in a running world and
registration order in a loaded one, and drew one random stream across a
batch in that order. The same draw then picked another building, or gave
another person the stay. The fix sorts each batch ascending by entity id
right before the engine reads it, so each decision depends on the batch's
contents only.

**Why now.** On `twomptest` three games split at step 25950. One bus ran
about 0.15 s behind in one game. The first evidence was the road edge
appends between steps 25520 and 25850: persons' `Add` and vehicles'
`AddRange` came in another order, inside the `road-entry-order` fix's
counters. A later split, at step 36400, differed only by a fare (+310).
Both look like the person simulation drifting, and the TF3 port had none of
TF2's person-order sorts.

**Every game of a room must run the same person-order settings.** The
sorts change what the simulation decides. A game with a batch sorted and a
game with it in the engine's order pick different buildings for the same
draw. All five fixes are on by default. `TPF3MP_HOOK_PERSON_ORDER=0` (or
`off`, `false`, `no`) turns all of them off, and each fix's own switch
turns it off alone (table below). Either kind of switch must be set alike
on every machine of the room. hook.log's first line of the group says so:

```
person-order: on (TPF3MP_HOOK_PERSON_ORDER=0 turns every person-order fix off); every game of a room must run the same person-order settings
```

The sites were read statically with `tools/tpfre` (build 40408). Every
batch is a `std::vector<Entity>` (4-byte ids) that the engine reads next,
and nothing indexes it by position across the sort. The hook splices in
(`tpf3mp_hookcore::detour::Splice`, as the order fixes do) and sorts the
ids in place.

| TF2 batch (TF2 Windows site) | TF3 fix | TF3 site (RVA) | what reads the batch in order | status |
|---|---|---|---|---|
| 1, candidates (`0x927df6`) | `person-candidates-order` | `destination_util::GetTargetsByLandUse/candidates` (`0x8e3d65`) | `BinarySearchIndex` (`0x8e4400`) sums free capacity in entry order and binary-searches one draw | ported |
| 2, departures (`0xa7c9fd`) | `person-departures-order` | `ecs::SimEntityAtBuildingSystem::Update2/leave batches` (`0xb05f92`) | `SimPersonSystem::NoteAtBuildingPersonsLeave` (`0xb2e950`): one generator seeded from `updateCount`, drawn in batch order | ported |
| 3, arrivals (`0xa59928`) | `person-arrivals-order` | `ecs::PersonMoveSystem::Update2/arrival batch` (`0xaecfcd`) | `SimPersonSystem::NoteWalkPersonsArrived` (`0xb33a30`): stay draws in batch order | ported |
| 4, idle (`0xa867ce`) | `person-needs-path-order` | `ecs::SimEntityNeedsPathSystem::Update/list` (`0xb18213`) | `PathFactory::Compute` (`0x8d11e0`): one generator per chunk, seeded from `updateCount` and the chunk's start index | ported |
| 5, capacity maps (`0x21234de`) | none | (`~SimEntityUpdateHelper` `0x256b0c0`, `0x256b349`) | `ApplySimPersonData` (`0x256da20`) walks five phmap flat maps in slot order with one shared stream | skipped (below) |
| 6, freed ids (`0x23de385`) | `freed-id-order` | `ecs::Engine::EndModification/free-id append` (`0x2bb4fd1`) | the FIFO free-id deque `AddEntity` (`0x2bb37b0`) pops | ported |
| person target-set, network person and network index order (Linux only) | none | none | none | not needed (below) |

| switch (`0`, `off`, `false` or `no`) | turns off |
|---|---|
| `TPF3MP_HOOK_PERSON_ORDER` | all five fixes below |
| `TPF3MP_HOOK_PERSON_CANDIDATES_ORDER` | `person-candidates-order` |
| `TPF3MP_HOOK_PERSON_DEPARTURES_ORDER` | `person-departures-order` |
| `TPF3MP_HOOK_PERSON_ARRIVALS_ORDER` | `person-arrivals-order` |
| `TPF3MP_HOOK_PERSON_NEEDS_PATH_ORDER` | `person-needs-path-order` |
| `TPF3MP_HOOK_FREED_ID_ORDER` | `freed-id-order` |
| `TPF3MP_HOOK_FREED_ID_TRACE` | the free-id trace (logging only; it never needs to match between games) |

Each fix installs on its own and fails closed on its own:

- Its profile target must resolve (`required = false`, so a build without
  the site loses that fix, not the profile).
- The splice compares the site's bytes with the fix's: the stolen
  instructions and every instruction after them up to the call that hands
  the vector on, so the frame slot it sorts is the one the engine passes.
- Each fix also checks the code around its site (below), and refuses
  with the reason in hook.log on any mismatch.
- Every read on the game's thread goes through `image::Readable`.
- A vector of another shape (pointers out of order, not whole aligned ids,
  more than 2^24 of them) is refused for that call. The refusal is said
  once per reason and the engine's order stands.
- A panic switches the fix off for the rest of the game.

hook.log carries one line per fix: `order fix <name>: installed (at <rva>,
...)` or `order fix <name>: off, <why>`.

**Candidates** (`person-candidates-order`). `GetTargetsByLandUse`
(`0x8e3ac0`, our name) gets the `PersonCapacity` family and copies its
node list (8-byte `{entity, index}` records) into a local
`vector<Entity>` at `[rsp+0x70]` (`0x8e3d10..0x8e3d3f`). It then builds
one 0x24-byte `TargetDataEntry` per entity, in that order, on the thread
pool. The chunks are joined in index order, not in completion order, so
threads add nothing but the list's order.

`BinarySearchIndex` later sums the entries' free capacity in entry order
and binary-searches one draw. The entries also index a sibling vector, so
the fix sorts the entity list before the build, not the entries after it.

- *Site:* `mov dword [rsp+0x60], 1` (`0x8e3d65`, 8 bytes stolen), after
  the copy loop and before the build. Nothing branches into its bytes.
- *Checks:* `lea rbx, [rsp+0x70]` before the copy loop (`site-0x8c`), and
  the build's size computation from `[rsp+0x78] - [rsp+0x70]` after the
  site. At run time the hook refuses unless `rbx` names `rsp+0x70`.
- *Reach:* the leave handler reaches it through the destination
  assignment (`0xb2b040`) and `GetRandomTargets` (`0x8e2e10`). The town
  system's new-person path and the Lua `getTargetsByLandUse` reach it too.
  It is called synchronously from all of them.

**Departures** (`person-departures-order`).
`SimEntityAtBuildingSystem::Update2` (vf12, `0xb05ea0`) gathers the
entities whose stay ran out on the thread pool, in node order. The pool
loop is `0xb052c0`, chunks of 256 joined in index order. The result is
two `vector<Entity>`, at `[rbp+7]` and `[rbp+0x1f]`, and each is emitted to
its signal (`0xaeaeb0`). `NoteAtBuildingPersonsLeave` seeds one generator
from an FNV hash of `updateCount` and draws, person by person in batch
order, whether the person recomputes its destination.

- *Site:* the test of the first vector after the pool loop (`0xb05f92`,
  `mov rcx, [rbp+7]; cmp rcx, [rbp+0xf]`, 8 bytes stolen). The only branch
  to it is the `je` at `0xb05f51`, to its first byte.
- *Sorting:* both vectors are sorted (persons and cargo).
- *Checks:* the second emit's bytes, and both emits must call one function.

**Arrivals** (`person-arrivals-order`). `PersonMoveSystem::Update2` (vf12,
`0xaecaa0`) gathers the walk arrivals on the pool, in node order (chunks
of 128 joined in index order), into a `vector<Entity>` at `[rbp-0x78]`. It
emits that vector (`0xaecfe8`) to `NoteWalkPersonsArrived`, which seeds
from `updateCount` and draws stay durations in batch order.

- *Site:* `0xaecfcd` (`mov rax, [rbp-0x70]; cmp [rbp-0x78], rax`, 8 bytes
  stolen), reached only by the `je` at `0xaecf11`, to its first byte.
- *Checks:* its emit must call the signal that the departures' two emits
  call, so it needs the departures target to resolve.

**Needs-path, TF2's idle** (`person-needs-path-order`). TF3 has no
`SimEntityIdleSystem`. Its counterpart is
`ecs::SimEntityNeedsPathSystem::Update` (vf11, `0xb181b0`, on odd
`updateCount`s), the only caller of `PathFactory::Compute`. It builds one
`PathFactoryInput` per entry of its own list `m_systemData->add`
(`[[this+0x10]]`). That list is a `vector<Entity>` that `EntityAdded`
appends to and `EntityToBeRemoved` erases from in place, so it is in
insertion history.

Compute runs three items per person (one per mode) in chunks. Each chunk
seeds one generator from `updateCount` and the chunk's start index and
draws for every item in it, so each trip's path and mode follow the
person's place in the list. The results are matched back to `add` by
position in the same call (`0xb183c0`, which the fix checks). Sorting the
list in place before its first read therefore keeps them paired.

- *Site:* that first read (`0xb18213`, `mov rax, [r13+0x10]; mov rdx,
  [rax+8]`, 8 bytes stolen).
- *Shared data:* the list's data is copy-on-write, a `shared_ptr` that
  the replicated engine's copy may share. The system's own writers take a
  private copy through a getter (`0xb186c0`) before they write.
- *What the fix does when it is shared:* if the use count in the control
  block (`[[this+0x18]+8]`) is above 1, the fix calls the same getter, the
  way `EntityToBeRemoved` does (`0xb18121`, the profile target
  `.../data getter call`; the getter's head is checked at install). Only
  then does it sort.
- *Why not refuse instead:* that would apply the sort in some games and
  not in others, depending on the frame pacing.

**Freed ids** (`freed-id-order`). `ecs::Engine::EndModification`
(`0x2bb4d90`) appends the modification's removed ids to the FIFO free-id
deque at `engine+0xd8`, through the deque's insert `0x2bb1110` at its end.
The removed ids are `m_betweenChanges` (`[[engine+0x1f0]]`), a
`vector<Entity>` that `RemoveEntity` pushes to in call order.
`AddEntity` (`0x2bb37b0`) pops the deque's front, and the deque is saved
(`Engine::Load` asserts `m_freeIds.empty()` before it reads it).

Sorting each batch before the append makes the deque depend only on which
ids each modification removed. The order matters because the 40 callers
of `RemoveEntity` include walks of node lists and hash maps.

- *Site:* `lea rcx, [r13+0xd8]` (`0x2bb4fd1`, 7 bytes stolen), reached only
  by the `je` at `0x2bb4fbf`. `r12` there is `engine+0x1f0`.
- *The second engine:* `Replicator::Apply` (`0x2bb4430`) replays the sim's
  modifications one by one, Begin, the removals, then End through the same
  `EndModification`. So it sorts the same batches, and its `AddEntity`
  check (`entity == c.entity`) still holds.

**What the sort cannot reach.** Read with `tools/tpfre` on build 40408,
`EndModification` is the only code that appends to the free-id queue (its
insert `0x2bb1110` has one caller, `0x2bb4ff3`), `AddEntity` the only code
that pops it or grows the entity table, and `RemoveEntity` (`0x2bb75f0`)
only pushes to the modification's removed ids. `Engine::Load` and
`Engine::Clone` replace the queue whole. So with the sort, the ids a game
hands out are a function of one thing: the sequence of the simulation
engine's modifications, each with the ids it took and freed. Two games can
only part on ids when that sequence differs, for example when a
modification lands at a moment the frames choose rather than the room's
steps (the paused path runs the game scripts' system, and a batch's first
update drains the command list before `ecs::Engine::Update` begins).

Seen once in production (release 1.2.8, 2026-10-06, two games from the
start world, no reload): the signals placed by the room had equal ids in
both games up to room step 34900, and from step 35100 on other ids. The
only actions in between were a bulldozed signal and nine line edits.
Vehicles diverged in lane 3 about 1,300 steps later, at step 36400.

#### The free-id trace

Logging only, on with `freed-id-order` (`TPF3MP_HOOK_FREED_ID_TRACE=0`
turns it off). It rides on the fix's own splice and reads, through
`image::Readable`, only the simulation's engine: the one `GameSim::Step`
hands `ecs::Engine::Update` (`[[this+8]+0x18]`), noted at every call of
the room's game's step. The replicated engine is left out.

- At every checkpoint (and the first batch after a load), right after the
  `ticks:` line:

  ```
  free ids: step <n>: queued=<ids> front=<id> hash=<16 hex>; updates: <m> modification(s) took <t> and freed <f> (<16 hex>); outside updates: <m> took <t> and freed <f>
  ```

  The queue's size, front and a hash of every id in pop order, then what
  the modifications in the room's updates took (new and reused ids) and
  freed since the last checkpoint, with a hash of the freed ids in order.
  Two games of a room must log equal lines at equal steps. The first line
  that differs bounds where their ids parted to one checkpoint interval,
  long before a placed object's id or a vehicle shows it.
- Each modification of the simulation's engine that took or freed ids
  outside the room's updates (the first 16, then every 1024th):

  ```
  freed-id trace: the simulation's engine took <t> and freed <f> id(s) <where> (returning to <rva>); <n> such modification(s) since the world loaded
  ```

  `<where>` is `in the game's step outside the room's updates` (the paused
  path, or a batch's first update before the engine's update began) or
  `outside the game's step` (another frame or thread). `<rva>` is the
  return address of `EndModification` (its head is checked: the return
  address is `0x128` above the site's `rsp`), the code to read with
  `tpfre q <db> func <rva>`. In a sound game there are none; a room
  action is applied for its step and counts as an update.

- The trace also keeps the first 96 modifications, including ones that
  take or free no IDs, of each
  checkpoint interval in memory, across the last 128 checkpoints. When a
  divergent checkpoint or its room-wide lane-dump request arrives, each
  game writes the latest 64 retained intervals up to that checkpoint to
  `hook.log`.
  This includes an allocator split well before the first lane mismatch,
  without continuously filling the hook's 192 KiB/minute diagnostic upload
  budget. Native-operation trace bursts, whether triggered by a divergence or
  an accepted room-chat ask, are limited to one per minute and at most 128
  KiB including logger prefixes, the header and a short suppression notice.
  Other hook lines share the 192 KiB/minute upload tail, so this trace bound
  does not guarantee that every diagnostic line reaches the relay; collect
  the local full log if the upload reports skipped bytes. Further distinct
  divergence steps in that minute
  produce one short rate-limit line; the next burst header reports how many
  notices were suppressed and the latest suppressed step, then includes the
  latest retained interval through the eligible divergence. The header gives
  the oldest and newest retained checkpoints, expired checkpoints,
  recorded/retained operations, omitted operations, unfit counts and
  unreadable native observations.
  `complete=true` means none of those gaps occurred within the selected
  incident window. Older expired checkpoints are reported separately and do
  not make an intact recent window incomplete. The launcher's source
  and run budgets can still skip old log
  lines, which its own diagnostics report.

  `free operations: step <checkpoint> part <n>` lines list compact records
  in modification order, each with `step:place:taken/freed:table/queued:
  first_freed/last_freed/freed_hash:return_rva`. `U` means a room update,
  `S` the game's step outside an update, and `X` outside its step. `-` is
  unavailable; `?` for taken means its count did not fit the preceding
  observation. The IDs and hash describe the freed batch at
  `EndModification`; taken is a count inferred between modification ends,
  not the identities of IDs allocated by `AddEntity`. If native worker
  modifications overlap, callback observation order need not be causal
  modification order. A world load clears
  this memory. Turning `TPF3MP_HOOK_FREED_ID_TRACE=0` off also disables
  these lines, and **Send diagnostics** Off keeps hook lines local.

The checks: `EndModification`'s head before the site, and the step's
`mov rcx, [rbp+8]; ...; mov rcx, [rcx+0x18]; call ecs::Engine::Update`
at `GameSim::Step + 0x1c0`. A mismatch leaves the trace off and says why
(`freed-id trace: off, ...`); the fix itself stays on. `freed-id-order`'s
own `in-step` counts are not comparable between games: the paused path's
modifications run inside the step too, as many as there are frames.

**Not ported**:

- *Capacity maps* (TF2's `capacity`). TF3's `SimEntityUpdateHelper` keeps
  its affected persons and cargo in nine phmap flat maps and sets at its
  data block (`[helper+0x130]`), not in MSVC `unordered_map`s with a list.
  `~SimEntityUpdateHelper` seeds one generator from `updateCount * 3 mod
  (2^31-1)`, not mt19937 with 5489. `ApplySimPersonData` walks the five
  person maps in slot order and hands the shared seed to every apply
  function. There is no list to relink.
- *Why the slots cannot just be permuted:* the walks also look entries up
  in the same maps.
- *When slot order can differ:* it is a function of the key set, the
  capacity and the insertion order among colliding keys, and phmap's hash
  has no per-process seed. Two games part here only if the builder
  (`PrepareSimPersonData`, `0x2574d70`) inserts in history order, which was
  not traced.
- *The possible fix, not built:* rebuild each map in ascending key order
  with phmap's exact insert at `0x256b349` (`mov rdi, [r13+0x130]`, the
  twin of TF2's site). It is risky enough to need its own measurement
  first.
- *The Linux-only orders* (`target_order_linux.cpp`,
  `network_person_order_linux.cpp`, `network_index_order_linux.cpp`,
  `person_map_order_linux.cpp`; `RESUME_STATUS.md`,
  `LINE_COST_DESYNC.md`). These make a libstdc++ build walk its sets, and
  hash its costs, the way the Windows build does. Every TF3 game in a room
  runs the same Windows executable, so they are not needed. A native Linux
  TF3 game in a room would need their counterparts.
- *The step canon* (TF2's `step`, every family's node list in entity order
  at each `Engine::Update`). It is a different fix, and still the
  candidate the order fixes above name for the ship and aircraft families.

**PathFactory's chunks do not depend on the core count.**
`PathFactory::Compute`'s chunk size is `max(1, ceil(3n / [pool+0xc4]))`
(`0x8ceec1..0x8ceeef`), and each chunk's seed comes from its start index.
`[pool+0xc4]` is a constant cap of 100, written only in
`ThreadPool::ThreadPool` (`0x3055a9f`), not the pool's thread count. So the
chunk boundaries, and with the list sorted the path seeds, are the same on
every machine whatever its number of cores.

The one machine-dependent case is a pool with exactly one worker thread.
`LoopImpl`'s shortcut (`0x8cf502..0x8cf517`) then runs the whole batch as
one chunk with one seed. That happens on a 1-vCPU machine, or with the
debug "Num. Sim. threads" slider at 1. Such a game would draw other paths
than a game with a bigger pool.

**What the log says.**

- At the first call of each fix and every `2^12` calls (`2^14` for
  candidates, `2^16` for freed ids), an alive line:

  ```
  order fix <name>: alive, calls=<n> reordered=<n> refused=<n> (in the step <calls>/<reordered>)
  ```

- At every such multiple of calls inside the game's step, the counts two
  games of a room must agree on:

  ```
  order fix <name>: in-step calls=<n> reordered=<n>
  ```

  `freed-id-order` also counts the second engine's modifications, which
  run outside the step as often as frames come, and the paused path's,
  which run inside it as often as frames come. Its in-step counts can
  therefore differ between games of a room; the free-id trace's `free
  ids:` lines are the ones to compare (above).

The time goes to the `person-order` piece of the `perf:` line.

**Tested without the game:**

- `persons::tests`: the sort, the vector checks, each site's frame
  slots, the departures, freed-id and needs-path hooks on vectors in
  memory, the switches and the off lines, the master line, and that the
  profile states each site's bytes.
- `persons::splice_tests`: the freed-id hook through its real 35 site
  bytes on a hand-written function.
- The static proof (`tf3_static_proof.rs`): the six targets resolve
  uniquely at their RVAs in the installed game, plus the calls the checks
  rely on. Those are the emits to `0xaeaeb0`, the pool loop, the
  `updateCount` seeds, Compute's caller, the results loop's re-read, the
  getter and the deque insert.

**In the game:** soak 9 on `twomptest` (three games on one PC, all five
fixes on) passed step 26100 in sync, past the step-25950 split. Whether it
also passes the step-36400 fare split was not yet known when this was
written.

### The fast emission grid

`crates/tpf3mp-hook/src/emission` replaces the work of
`ecs::EmissionGridSystem::Update` (`0xaa9230`, profile target
`emission::EmissionGridSystem::Update`) with a fused step whose result is
the game's to the bit (investigation/TF3_SIM_COST_2026-10-05.md §1).

**What the game does.** Each update moves the noise and the pollution grid
(16 m cells: 1,602 x 16,002 floats each on a 100 x 1000-tile map, 1,794 x
1,794 on Gigantomaniac) one step per 0.2 s of `dt`. A step is up to three
full-grid passes on the game's thread pool, each reading one buffer and
writing the system's shared temporary, then a swap of the two vectors:
Diffuse (5-point stencil, `+1e-15`), Wind (pollution only: bilinear shift
and a divide per cell) and Average (`(1-c)*conc + c*avg`). The kernels never
write the border ring, so borders travel with their buffers. About 68 bytes
of memory traffic per cell and update: 1.75 GB on a 100 x 1000-tile map,
0.22 GB on Gigantomaniac. The grid is simulation state (town noise and
pollution ratings, eco levels, scripts) and is saved, so every bit must
match in every game of a room.

**What the hook does.** It redirects `Update`'s three dispatcher calls
(`+0x1f2` Diffuse, `+0x29e` Wind, `+0x31c` Average). The Diffuse and Wind
calls only note their arguments; the Average call runs the whole step at
once: one pass over row bands on up to 16 threads of the hook's own (the
game's pool is idle while `Update` waits), eight cells at a time with AVX.
Every lane does the game's scalar operations in the game's order (no FMA,
no reassociation; the same `+1e-15`, the same divide), and the step writes
the same cells into the same buffers that the game's passes and swaps
would, the temporary included. Each band first copies the up to four rows
outside it that it reads, so the result does not depend on the number of
bands or threads. `Update` itself (component lookups, the temporary's
resize, the swaps, the border checks, the step count) stays the game's.
Measured offline on the 1,602 x 16,002 grids of a 100 x 1000-tile map
(both grids, one update, 16 threads, Ryzen 9 9950X3D): the game's kernels
split over threads as its pool splits them took 42 ms, the fused step 20 ms
(2.1x). Both are bound
by memory bandwidth; the fused step moves 32 bytes per cell instead of 68,
the least a step that reads two buffers and writes two can move.

**Fail-closed.** `TPF3MP_HOOK_FAST_EMISSION=0` in the game's environment
leaves the game's update (on by default: the result is bit-identical, so a
game with it and one without agree, and the room sets nothing). It also
stays off without the profile target, on a CPU without AVX, or when any
byte of `Update`, the three dispatchers or the three kernels differs from
the FNV-1a hashes in `profiles/.../emission.rs`. A step the model does not
cover runs the game's own dispatchers, in the game's order: grids of
different sizes, a weight or wind the game's own code asserts on, a
non-default MXCSR on the simulation thread or a worker, calls out of order.
The first three fused steps of each grid, and one in 1,024 after, first run
three 12-row windows of the real grid through the game's own kernels and
the fused step side by side; a difference turns the fused step off for the
rest of the game.

**In `hook.log`:**

```
emission grid: fused update installed (at 0x...: one pass, AVX, up to 16 threads, bit-identical to the game's three passes; TPF3MP_HOOK_FAST_EMISSION=0 turns it off)
emission grid: noise 1602x16002: 3 windows of the real grid bit-identical to the game's kernels; fused from now on
emission grid: pollution 1602x16002: 3 windows of the real grid bit-identical to the game's kernels; fused from now on
emission grid: self-check <n> passed                    (at n = 8, 16, 32, ...)
emission grid: 4096 grid steps fused, <t> ms each on average; 0 run the game's way
```

Lines to worry about: `emission grid: the game's own update, <why>` at
install (the fix is not active), `this step runs the game's way: <why>`
(one step fell back; harmless, the game's code ran), and `OFF for the rest
of this game: self-check failed: <name> differs at (x, y) ...`, which means
the model and the game disagree: report it with the log.

**Tested without the game** (`emission::original_tests`, which relocate the
game's code from the executable with the harness in
`crates/tpf3mp-hook/src/original.rs`):

- the three kernels against the fused step on random grids from 3 x 3 to
  258 x 129 (zeros, negative zeros, denormals, huge values, infinities,
  emitter input between steps; winds of every sign and zero), 12 steps
  each, with 1 to 64 bands, with and without threads and AVX lanes: every
  bit of every buffer equal;
- the game's whole `Update` (relocated with stubs for the ECS lookups and a
  one-thread pool) with and without the redirected calls, on worlds from
  3 x 3 to 260 x 300 and 34 x 600, eight updates each including a `dt` of
  0.4 (two steps): the same buffer in every vector and the same bits, with
  every step fused, and with every step forced down the fall-back path;
- a non-default MXCSR runs the game's passes; the self-check passes on the
  game's kernels and catches a one-ulp change of a weight; the recorded
  hashes, call sites and kernel calls match the executable;
- `emission_speed` (ignored, a benchmark): `cargo test -p tpf3mp-hook
  --release --lib -- --ignored emission_speed --nocapture`.

**Not done: skipping still blocks.** A block whose inputs did not change
would give the same outputs, but the emitters write into the grid outside
`Update` (`EmissionEmitterSystem::Update2`, three insert paths), and so do
loading and scripts; proving that no write was missed needs either hooks
on every writer or a full comparison against a copy, which costs most of
what skipping saves. The fused step leaves the grid exact without that
proof.

### What the hook costs: the `perf:` lines

`crates/tpf3mp-hook/src/perf.rs` times the hook's per-update work where
it runs, and the game's own `GameSim::Step` around the step detour's call
of the original, so the one can be set against the other. Each timed call
reads the clock twice (`Instant`, which is `QueryPerformanceCounter` on
Windows) and adds its nanoseconds and one call to two atomics; no call is
sampled. A timed call costs about 56 ns with the timing on and under 1 ns
off (`perf::tests::a_timed_call_costs_two_clock_reads`, release build, on
the development PC); at a few thousand timed calls a second that is a few
tenths of a millisecond a second, so the timing is **on by default**.
`TPF3MP_HOOK_PERF=0` (or `off`) in the game's environment turns it and its
lines off; hook.log says which at install (`perf: timing the hook's
work, ...` or `perf: timing off (...)`).

Every 10 seconds of wall time, after a call of the step, the two existing
`perf:` lines go to hook.log with the game's own costliest systems in a
separate `perf: sim` line ("The game's own systems: the `perf: sim` line"
below). A `perf-step:` line reports the selected update count for each
`GameSim::Step` call in the same window:

```
perf: 10.0s: game step 2000.0 ms (200.0 ms/s) in 600 batches, 600 updates (3.333 ms/update); hook 42.5 ms (4.25 ms/s, 2.12% of the game's step); readable cache 0 hits, 0 misses; guarded reads 90000, 3 faults
perf: road-entry 19000/9.50ms/0.50us, platform-visit 0/0.00ms/0.00us, platform-candidates 0/0.00ms/0.00us, land-vehicle 0/0.00ms/0.00us, vehicles-at-stop 0/0.00ms/0.00us, person-order 0/0.00ms/0.00us, reseed 6000/30.00ms/5.00us, paused-tick 0/0.00ms/0.00us, lanes 0/0.00ms/0.00us, lane-dump 0/0.00ms/0.00us, gate 600/3.00ms/5.00us; road-entry refused 12 (12 the edge's entity has no slot)
perf-step: 10.0s: room updates/call 0:0 1:600 2:0 3:0 4+:0; outside-room exact calls 0:0 1:0 2:0 3:0 4+:0; own-speed calls 0 (updates unknown); max consecutive zero-update room calls 0; max call-start gap 50.0 ms; even steps off:0 nominal:600 reserve:0 underrun:0 repay:0 barrier:0 command:0 paused:0 stale:0 far:0; batch interval written 50 skipped 0 false syncs 0
```

The `perf-step:` buckets count the selected `Updates::Exactly(n)` answer,
not a guess from elapsed time or the update timer: `room` calls are the
room's cadence, exact calls outside a room are listed separately, and
`Updates::Own` calls have an unknown update count. The zero-update maximum
counts adjacent room calls selected as `Exactly(0)` and is clipped to each
10-second window; any non-room or nonzero/unknown call breaks the run. The
call-start gap is the largest interval between consecutive detour-entry
timestamps, with each gap recorded after its later step returns, using the
`Instant` already read for `perf:` timing. Window reporting occurs after a
step returns, so a call that spans the 10-second boundary is included in the
window it closes. The gap is `n/a` until two calls have been observed.
`even steps` counts the room calls by the rule that chose their count
("Even steps"): in a steady room nearly all are `nominal`. These are
simulation-call cadence and timing measurements, not render-frame timings
or evidence of vehicle motion or visual stutter.

The first line: the window's length; the game's step, its total time,
that time per second of wall time, its calls (batches) and the
simulation updates run (`ecs::Engine::Update` calls, counted by the seeds'
per-update detour, so `0` without it), and the step's time per update;
then the hook: the sum of every piece below, per second, and as a share
of the game's step time; then the readability checks answered from
`image::Readable`'s cache and those that asked the system (each miss is
one `VirtualQuery` or more; both 0 with guarded reads on), and the
guarded checks and reads made and how many of them a fault refused (0
with `TPF3MP_HOOK_GUARDED_READS=0`; each thread adds its reads in
batches of 256, so a window's count may lag by that much a thread). Each piece of the second line is
`<name> <calls>/<total ms>/<mean µs>` over the window:

| piece | what is timed | calls are |
|---|---|---|
| `road-entry` | `road-entry-order`'s sort after `EdgeUseManager::Add` or `AddRange` (the engine's append is not counted) | appends |
| `platform-visit` | `platform-order`'s visit site: the copy and sort at the loop's first iteration, the redirect at every one | iterations of the chooser's loop (one per vehicle per update) |
| `platform-candidates` | `platform-order`'s candidate sort | `FindNextFreeTerminal` sorts |
| `land-vehicle` | `land-vehicle-order`'s sort | reservation updates |
| `vehicles-at-stop` | `vehicles-at-stop-order`'s sort | stop lookups |
| `person-order` | the person-order fixes' sorts ("The person-order fixes") | batches: candidate lists, leave and arrival batches, needs-path lists, freed-id batches |
| `reseed` | the per-call reseed of the game scripts (update, postUpdate, handleEvent), its checks and `math.randomseed` | script calls |
| `paused-tick` | the paused-tick redirect's decision (the game's own advance, when passed on, is not counted) | paused frames |
| `lanes` | `tpf3mp_native.lanes(t)`: the lanes' text read off the Lua stack at a checkpoint (the mod's own reading of the world is Lua, inside the game's step) | checkpoints |
| `lane-dump` | `dump()` and `dumped(lane, entry)` | calls |
| `gate` | the step detour's own work around the game's step: the room's session, the driver, the lane digests, the log | step calls |

The line ends with the road fix's refusals in the window, by reason.

Reading them: every piece but `gate` runs inside the game's step, so the
game's step time includes it; `gate` runs around it. `reseed` runs on the
threads that run the game scripts, which may run side by side, so its
total is time spent on those threads, not wall time; the share is then an
upper bound of what the hook adds to a frame. A share of a few percent is
the hook's; a game step that takes more per update with the same world is
the game's.

**A/B the fixes.** Every piece has a kill switch in the game's
environment (the launcher's environment reaches the game). Run the same
save with and without one, a minute or more each, and compare the first
lines' `ms/update` and the piece's total:

| switch (`0`, `off`, `false` or `no`) | turns off |
|---|---|
| `TPF3MP_HOOK_ROAD_ENTRY_ORDER` | `road-entry-order` |
| `TPF3MP_HOOK_PLATFORM_ORDER` | `platform-order`, both sites |
| `TPF3MP_HOOK_LAND_VEHICLE_ORDER` | `land-vehicle-order` |
| `TPF3MP_HOOK_VEHICLES_AT_STOP_ORDER` | `vehicles-at-stop-order` |
| `TPF3MP_HOOK_PERSON_ORDER` | every person-order fix; each also has its own ("The person-order fixes") |
| `TPF3MP_HOOK_PAUSED_TICK` | the paused-tick redirect |
| `TPF3MP_HOOK_SCRIPT_RESEED` | the game scripts' per-call reseed (the per-update detour stays, so the mod's own `tpf3mp_native.seed` still works) |
| `TPF3MP_HOOK_LANE_DUMP=off` | lane dumps, even after a divergence |
| `TPF3MP_HOOK_MEASURE_ORDER` | (unset by default) the order measurement, which adds its own detours and hashing when set |
| `TPF3MP_HOOK_GUARDED_READS` | guarded reads: the hot paths check with `VirtualQuery` through the region caches instead (same answers, slower) |
| `TPF3MP_HOOK_PERF` | the timing and these lines, the `perf: sim` line's timers with them (`full` keeps them on and counts the component lookups too) |

Each switch changes what the game computes, so a game with one off
diverges from a room whose other games have it on: A/B in a room where
every game has the same switches, or alone.

The desync diagnostics are off unless set, and logging only: they change
nothing the game computes, so one game of a room may run them alone.

| switch | turns on |
|---|---|
| `TPF3MP_HOOK_LANE_DUMP_BOX=x0,y0,x1,y1` (with `TPF3MP_HOOK_LANE_DUMP_BOX_STEPS=from-to`) | the network lane (0) dumped at every checkpoint of those steps, only its edges with an end in the box, even with lane dumps off ("Lane dumps") |
| `TPF3MP_HOOK_TOWN_TRACE` (`1` or `on`) | the `town:` lines and the towns lane's dump at every checkpoint ("The town trace") |
| `TPF3MP_HOOK_STEP_TRACE` (`1` or `on`) | a `step-trace:` line for every call of the game's step ("What the lanes cost") |
| `TPF3MP_HOOK_BATCH_INTERVAL` (`0` or `off`) | the hook never writes the game's batch interval ("The batch interval") |
| `TPF3MP_HOOK_EVEN_STEPS` (`0` or `off`) | each call of the game's step runs every released step it may, instead of an even share ("Even steps") |
| `TPF3MP_HOOK_EDGE_WATCH=<e>,...` (with `TPF3MP_HOOK_EDGE_WATCH_STEPS=from-to`) | the `edge watch:` lines for those entities and an `apply:` line for every command applied ("The edge watch") |
| `TPF3MP_HOOK_STREET_TRACE` (`1` or `on`; narrowed by `TPF3MP_HOOK_STREET_TRACE_STEPS` and `TPF3MP_HOOK_STREET_TRACE_BOX`) | the `street:` lines ("The street trace") |
| `TPF3MP_HOOK_ROAD_ENTRY_TRACE=from-to` | a `road:` line for every in-step road edge append in those steps ("The road entry trace") |
| `TPF3MP_HOOK_ROAD_ENTRY_RECORD=<n>` | the last `n` steps' `road:` lines kept in memory and written when the room asks for a lane dump |
| `TPF3MP_HOOK_WATCH_ENTITIES=<e>,...` | each traced or recorded append of those entities lists the entries of the edges it touched |

The `road-entry:` digest at every checkpoint needs no switch: it is on
while `road-entry-order` sorts.

### The game's own systems: the `perf: sim` line

`crates/tpf3mp-hook/src/simperf.rs` times the game systems whose cost
grows most with the map (investigation/TF3_SIM_COST_2026-10-05.md): timing
only, each timer calls the game's function with the arguments it was
given. They are in while the timing is (`TPF3MP_HOOK_PERF` not `0`), at
two clock reads a call (about 56 ns) against milliseconds of work, and
each one fails closed on its own: hook.log says at install, for each,
`perf: sim timer <name>: in (...)` or `perf: sim timer <name>: absent,
<why>`.

| name | the game's function | how it is reached |
|---|---|---|
| `emission-grid` | `ecs::EmissionGridSystem::Update` (`0xaa9230`): the noise and pollution grids' diffusion, wind and averaging | its vtable slot (`0x1436fcb10`) |
| `emission-emitters` | `ecs::EmissionEmitterSystem::Update2` (`0xaa51c0`): the emitters' splat into the grids | its vtable slot (`0x1436fc490`) |
| `towns` | `ecs::TownSystem`'s update (`0xb61cc0`), which sums the grids per district | its vtable slot (`0x143708bb8`) |
| `parcel-collision` | `parcel_util::UpdateParcelCollision` (`0x9312e0`): each applied proposal's octree walk over the union of its boxes plus 50 m | its only call (`0x1425fcb9a`), redirected |

The three systems have no direct caller (the static proof checks it), so
their timers replace the vtable slot's pointer after checking it holds the
function the profile resolved: a detour of the function itself, by any
other feature, still runs, inside the timer. Once a window:

```
perf: sim emission-grid 600/24000.00ms/40000.00us, emission-emitters 600/3000.00ms/5000.00us, towns 600/1200.00ms/2000.00us, parcel-collision 40/2000.00ms/50000.00us (50.333 ms/update together); parcel boxes 120, union mean 1.000 km², max 25.500 km²; component-index fast, not counted (TPF3MP_HOOK_PERF=full counts it)
```

Each timer is `<name> <calls>/<total ms>/<mean µs>` (`<name> absent` when
it is not in); then their sum per simulation update; the parcel walk's
boxes and the mean and largest area of its query box (a few large unions
or many small ones decide which of the investigation's option 6 helps);
then the component lookup below. With `TPF3MP_HOOK_PERF=full` the lookup's
calls are counted too (`component-index <n> calls`), one `lock add` a call
on a counter picked by the thread's stack: cheap, but on millions of calls
a second, so not by default. Set the time against the first line's `game
step ... ms/update`. The emitters' and grid's passes run on the game's
thread pool and are waited for, so their time is wall time on the
simulation's thread.

### The faster component lookup

`crates/tpf3mp-hook/src/fastindex.rs` (`fast-component-index`, on unless
`TPF3MP_HOOK_FAST_COMPONENT_INDEX=0` or `off`). The game's
`ecs::Engine::GetComponentDataIndex(engine, entity, type)` (`0xa4b90`,
about 1,600 direct calls) is a linear search: the entity's list of
`{type, index}` pairs at `[engine+0x90] + entity·24`, the first pair of
the type, its index; a miss asserts (`Engine.h:0x143`). It keeps a 0xE0
byte frame and a `/GS` cookie on every call for the assert's formatting.
The hook jumps from its entry to a frameless copy of the hit path: the
same reads in the same order, the same first match. A miss goes, with
every argument register as it came, to the original's trampoline, which
searches again and asserts as the game does.

- **Exact, so not a room setting.** Nothing is cached, so nothing must be
  kept coherent with the engine, and a call on any thread reads what the
  original read. The tests run the game's own function, relocated from
  the executable, and the hook's on 320,000 random queries over random
  worlds (duplicate types in a list, empty and 200-pair lists, negative
  entities, `INT_MIN`/`INT_MAX` types, misses): the same answer every time,
  and every miss reaches the game's assert, every hit never does. A third
  test runs both with every register set to a known value: a hit changes
  `rax` and `r9` only, all of which the original changes too.
- **Fails closed**: the profile's signature is the whole hit path, entry
  to `ret`, so a build whose lookup differs anywhere leaves the game's.
- **What it gains**, measured on the development PC
  (`fastindex::original_tests::a_lookup_costs`, release): about 1.5 to 2
  ns of 10 to 14 ns when the lists are in the cache (about 15%), and
  nothing measurable when they are not (100 ns and more, two dependent
  cache misses: the entity's list header, then its list). So it trims the
  per-call overhead only; the parcel walk's cost on a large map is mostly
  the misses, which only fewer lookups remove (the investigation's
  option 6a).

To A/B it, run the same save with and without
`TPF3MP_HOOK_FAST_COMPONENT_INDEX=0` and compare the first line's `ms/update`
and the `perf: sim` line's `parcel-collision`.

## Release-day procedure: adding a target for a new build

The first TF3 build's targets are already located (RVAs, RTTI/source
names) in
[investigation/TPF3_RECON_2026-09-29.md](../investigation/TPF3_RECON_2026-09-29.md):
the command queue (`CommandList::Add`), the sim step (`GameSim::Step`,
`CGame::RunGameSimLoop`), `CGameTime`, the two-`GameState` swap, the
player/company commands, and a lockstep step-budget global. This procedure
turns each into a verified profile target; the recon page also lists the
reconciliations to settle in-game first (e.g. `CommandList` vs
`DeferredCommandBuffer`).

1. **Archive the build.** Record the executable SHA-256, file size and PE
   timestamp (`BuildIdentity::of_file`), plus the Steam build/manifest ids. Keep
   a private copy (see [DAY_ONE.md](DAY_ONE.md)).
2. **Find the function** with the RE pipeline, and note its RVA and the bytes at
   its start. `tools/tpfre` indexes the executable in seconds and answers
   `func`, `callers`, `xrefs`, `str`, `dis` and `whois` queries on it
   ([its README](../tools/tpfre/README.md)).
3. **Write a signature.** Take the opening bytes; replace every relative or
   absolute displacement with `??`; extend only until the pattern is unique
   across the scanned section. Record the exact, wildcard-free `prologue` (at
   least the number of bytes the detour must steal - 5 for a near hook, 14 for a
   far one, on an instruction boundary).
4. **Add a `[[target]]`** to the build's profile with `name`, `signature`,
   `offset`, `prologue` and `required`.

   `tools/re/make_profile.py` does steps 3 and 4 for x86-64 builds, from the
   binary and the symbol map `name_functions.py` wrote:

   ```
   python tools/re/make_profile.py TransportFever3.exe out/TransportFever3.symbols.json \
       GameSim::Step CGame::Step -o profile.toml
   ```

   It writes the `[build]` identity (SHA-256, size, and the PE timestamp on a
   PE) and one target per function, with `offset = 0`. Displacements it
   wildcards: branch and call targets (rel8 and rel32), RIP-relative operands,
   and immediates or absolute displacements that point into the image. The
   signature starts as the prologue's instructions and grows one instruction at
   a time until it matches once in the function's on-disk section, never past
   the function's end or `--max-length` (128) bytes. The prologue covers
   `--steal` bytes, 14 by default: a far jump, since how far the detour lands is
   only known at install. Like the engine, it refuses a prologue holding a
   branch, call, return or interrupt, and keeps RIP-relative data operands,
   which the engine relocates. Every refusal names the target and the reason: a
   name shared by several functions (pick one with `NAME@0xRVA`), a function
   byte-identical to another (never unique), a branch too early to steal around.
   `tpfre q <db> sig NAME --toml` applies the same rules to one function and
   prints its `[[target]]` block (identical to make_profile's on TPF2's
   targets), to try a target before writing the profile.
   `tools/re/test_make_profile.py` checks the tool on a synthetic PE and keeps
   `tpf3mp-hookcore/tests/data/make_profile_fixture.{pe,toml}` current, which
   `tests/make_profile_fixture.rs` resolves with hookcore itself.
5. **Verify.** Resolve the profile against the **in-memory module image** of the
   running build and confirm the target resolves uniquely to the expected
   address and that the prologue matches. Keep a static check against an
   archived copy where the code section is readable on disk.
6. **Never widen a signature to force a match** on a build you have not archived.
   An unknown build must stay unknown, so the hook fails closed.

## What was verified on the TPF2 binary

Against `TransportFever2.exe`, Steam build 35924 (SHA-256
`782b904a...585175c`, size 72,843,280, PE timestamp `0x675ABCC6`, image base
`0x140000000`), the profile in `tpf3mp-hookcore/tests/data/tpf2_build35924.toml`
resolves all five targets, each **matching exactly once** across `.text`:

| target | RVA |
|---|---|
| `GameSim::Step` | `0x15aa00` |
| `CGame::Step` | `0x118e90` |
| `CGameTime::GetSpeed` | `0x2877a0` |
| `UI::CMenuUI::StartSavegame` | `0x6785c0` |
| `UI::CMenuUI::CreatePage` | `0x663370` |

`CGameTime::GetSpeed` sits next to two near-identical siblings, so its signature
runs past the (wildcarded) call to the distinguishing `mov eax,[rax+4]`;
`StartSavegame` and `CreatePage` share a seven-`push` prologue, so each signature
runs to its distinct `lea`/frame bytes (and `CreatePage` to the `mov
[rsp+0x330],rbx` store that separates it from a twin at `0x215c480`). The test
also confirms the resolver refuses a modified copy (corrupting one target's
bytes yields a `Missing` refusal) and refuses a mismatched build identity.

**DRM note.** This build carries a SteamStub section (`.bind`, high entropy),
which can decrypt code at load time. For build 35924 the code section is
nonetheless **readable on disk**: all five prologues match the on-disk `.text`
exactly, consistent with the RE survey's ~88,000 assert-string references found
in the same on-disk section. On-disk verification is therefore valid *for this
build*. It is not guaranteed in general - a future build could encrypt `.text` -
which is why the production resolver scans the in-memory, unpacked module image,
and why on-disk scanning is documented as a development convenience only.

## What a shipped mod hooks on the same build

Build 35924 has a shipped lockstep mod hooking it, [TpF2 Multiplayer](https://github.com/silver2127/tpf2-multiplayer)
(0.6.1.12, 2026-09-20), so every target in the profile and everything in this
section runs in players' games rather than in a test. RVAs are from image base
`0x140000000`. Names are the ones the binary carries in its `__FUNCSIG__` assert
strings where it has one (`tools/re/name_functions.py` recovers those); the
rest are the mod's own names for functions it identified by decompiling or by
differential capture. Steal sizes are the bytes that mod's detour engine
overwrites; its engine refuses RIP-relative instructions in a prologue rather
than relocating them, so its steals are a conservative bound for one that does.

### The five profile targets, as the mod uses them

| target | RVA | how the mod uses it |
|---|---|---|
| `GameSim::Step` | `0x15aa00` | Not detoured whole. Two sites inside it are patched: the calls to `CGameTime::GetSpeed` at `0x15aa30` and `0x15aae4` (a fractional speed scales the batch interval) and the paused branch's `call 0xaea970` (GameTime advance) at `0x15aa4a`, so a paused game advances `GameTime+0x30` per simulation step and not per render batch. Both siblings of `GetSpeed` are real: the profile's extended signature is the right call. |
| `CGame::Step` | `0x118e90` | Detoured, 16-byte steal, for pacing (the leader is the clock; joiners pace to it). |
| `CGameTime::GetSpeed` | `0x2877a0` | Read through its call sites rather than hooked; `GameTime::get` at `0x2877c0` returns the counter at `+0x30`. |
| `UI::CMenuUI::StartSavegame` | `0x6785c0` | Detoured (the share observer: a host that loads another world pushes it), and called directly to load a shared save in-process: build a `SaveGameId` `{wstring path; string name; string namespace}`, get its `SavegameInfo` from the save manager (`0x2e6ca0`; the manager is `+200` on the app object from `0xbb23c0`), default-construct `LoadGameParams` (`0x553b70`, 0x138 bytes) and call from `CMenuUI`'s own per-frame update, vftable `0x301dc38` slot 33 (`0x672b10`), on the main thread, where the game starts its own queued loads. Guards on the menu object: `+0x4e8` non-zero while a game runs, `+0x1988` "initialization already active", `+0x19a0` a queued load. |
| `UI::CMenuUI::CreatePage` | `0x663370` | Detoured, 20-byte steal (the Multiplayer panel on the title menu). Two more menu entries go with it: the list-add at `0x22d99e0` (15) and the main-page builder at `0x667bc0` (14). |

### The command pipeline: two hooks, not one

TF3's command surface is now documented, not guessed: Urban Games'
reference lists **61 `api.cmd.make*Cmd` factories** with their argument
types, recorded in
[investigation/TF3_OFFICIAL_API_2026-09-29.md](../investigation/TF3_OFFICIAL_API_2026-09-29.md).
A TF3 profile's factory targets are found for that list, not ported name
for name from TPF2's. Two entries change the design directly:
`makeWorldBuildProposalCmd` takes a fifth `playerInitiated` argument (a
possible player-vs-replay signal, see below), and companies are commands
(`makeGameAddPlayerCmd`, `makeEntitySetPlayerCmd`), so ownership changes go
through this same pipeline rather than the native, assert-bypassed
`setPlayer` binding TPF2 patched.

Every player action becomes a `Command` built by a `make_cmd::*` factory and
handed to `CommandList::Add(list, OUT handle, cmd, ..., callback)`. The mod
hooks both, and the reason is worth carrying into a TPF3 profile:

- The **factory** hook sees *what* the command is, while its arguments are still
  the caller's typed structures (a proposal, a `component::Line`, a vehicle
  configuration), which is the only moment they are cheap to decode.
- The **`Add`** hook is the only place a command can be *cancelled*: it zeroes
  the result handle and returns without queueing. The factory cannot cancel;
  its caller still holds the command.
- The mod's own replays go through the same factories (the script's
  `api.cmd.*` path), so the **return address of the factory call** is the only
  thing that tells a player's command from the mod's replay of one. That
  caller-RVA filter is load-bearing, not tidiness: without it every replay is
  captured again. On TF3 it may not hold: the GUI is script, and if the
  stock tools build their commands through `api.cmd` as our replays do,
  both arrive from the same caller. Check this before porting the filter
  ([investigation/TF3_MODS_2026-09-27.md](../investigation/TF3_MODS_2026-09-27.md)).

| factory | RVA | steal | |
|---|---|---|---|
| `BuildProposal` | `0x9dc750` | 19 | roads, track, constructions, terrain, assets, the bulldozer: one command, told apart by the proposal's shape ([BUILDING.md](BUILDING.md)) |
| `CommandList::Add` | `0x9d2a00` | 18 | the cancel point |
| `BuyVehicle` | `0x9dca00` | 15 | its UI waits on the result entity |
| `SellVehicle` | `0x9de380` | 20 | |
| `ReplaceVehicle` | `0x9dddb0` | 15 | its UI waits on the result entity |
| `SendToDepot` | `0x9de6f0` | 20 | |
| `SetLine` | `0x9dea10` | 18 | |
| `CreateLine` | `0x9dcde0` | 19 | its UI asserts on an empty result |
| `UpdateLine` | `0x9df4e0` | 19 | |
| `DeleteLine` | `0x9dd190` | 20 | |
| `Reverse` | `0x9ddfe0` | 20 | a toggle: replaying an uncancelled one applies it twice |
| `SetColor` | `0x9de8a0` | 20 | `r9 -> CVec3f*` |
| `SetName` | `0x9deb70` | 15 | `r9 -> std::string*` (MSVC SSO) |
| `SetGameSpeed` | `0x9de9e0` | 21 | the clock buttons |
| `SetDate`, `SetCalendarSpeed` | `0x9de9b0`, `0x9de870` | 21 | the editor's date controls |

Three rules the cancel point taught, each after a crash or a wedged tool:

1. **A cancelled command's completion callback is a contract.** Commands whose
   UI waits on the result (the build tools, `BuyVehicle`, `ReplaceVehicle`)
   must have their callback fired with a zeroed result at `Add`, or the tool
   hangs for the rest of the session. The callback is a `std::function` whose
   impl the game builds on the stack (`{vftable, captured this}`; `_Do_call`
   is vftable slot 2) or on the heap (impl pointer at `r9+0x38`).
2. **Fire-and-forget commands must not have it fired.** `SetLine` and
   `Reverse` fired with the success byte still 0 make the UI take its failure
   branch ("unable to find a path to a stop"). Suppress without firing.
3. **A callback that asserts on an empty result is moved, not fired.**
   `CreateLine`'s callers (`UI::LineList` `0x610490`, `UI::LineManager`
   `0x6154a0`) assert `resultEntity != ecs::Entity()`. The mod moves the
   callback object into a stash and fires it later, from a later `Add` on the
   same thread, with a stand-in result naming the entity the replay created
   (a 16-byte entry `{int32 entity; double gen; int32}` whose generation must
   match the registry's, `[reg+0xb8]+id*12`).

And one that holds everywhere: **never cancel when the decode failed.** A
command the mod cannot ship in full runs natively and is read back afterwards;
cancelling it would lose the player's action.

### Layouts the capture depends on

Every vector is read at the game's own length (`{begin, end, cap}`), with a
sanity bound on the span and a readability check on every page it touches,
under SEH: a misread pointer fails the decode loudly, and a failed decode is
never cancelled.

- `component::Line`: `vector<Stop>` at `+0x00`, `int waitingTime` `+0x18`,
  `VehicleInfo` `+0x1c` (8 bytes: a `std::bitset<16>` of transport modes plus
  4). `VehicleInfo` is **engine-maintained**: the sim-side `UpdateLine`
  handler (`0x9d9fd0`) restores its own copy, and a command cannot set it.
- `Line::Stop`, 0xa8 bytes: `Entity stationGroup` `+0x00`, `int station`
  `+0x04`, `int terminal` `+0x08`, `vector<StationTerminal{int,int}>
  alternativeTerminals` `+0x10`, `int loadMode` `+0x28` (0..3), two `float`
  waits `+0x2c`/`+0x30`, `vector waypoints` `+0x38`.
- A proposal's edge record, 120 bytes: node ids `+0x00`/`+0x04`, tangents
  `+0x10`/`+0x1c` (3 floats each), `BaseEdge` type and type index
  `+0x28`/`+0x2c` (1 bridge, 2 tunnel), and an optional `PlayerOwned` as
  `{int32 player +0x70; uint8 present +0x74}`.
- `TransportNetwork` and the other components are reached through the engine's
  type index. *Corrected (2026-10-04, from the disassembly):* the type index
  comes from `ComponentManager::GetComponentTypeIndex` (`0xa4cc0`, at
  `engine+0x48`) with the component's `RTTI_Type_Descriptor`, the data
  index from `Engine::GetComponentDataIndex` (`0xa4b90`; `0xd0920` is inside
  a phmap rehash), the pool is `engine+0x78[typeIndex]` (not `+0x88`), and
  its entries are `sizeof(T)` bytes (0x48 is `GameTime`'s), data at `+0x68`
  (indices below `0x40000000`) or paged at `+0x80`. The details, with the
  street network's layouts, are in
  `investigation/TF3_NATIVE_NETWORK_2026-10-04.md`.

### The game has two engines

`CGame::RunGameSimLoop` (`0x1184d0`) keeps two `GameState` objects at
`CGame+0x168 -> { GameState*[2], ..., int current at +0x20 }` and copies one
into the other every frame with `GameState::Replicate` (`0x241630`);
`CGame+0x158` is whichever is current this frame, and that is what the UI's
`GameStateProvider` returns (`0x8badf0`: `mov rax,[rcx+8]; mov rax,[rax+0x158]`).
`GameState+0x28` is that state's `ecs::Engine`, and each engine owns its own
system objects. A command carries a specific engine pointer, so anything
computed on its behalf (the mod re-runs the line editor's platform assignment
at the replay) has to take the state whose `+0x28` is that engine, never
"this frame's".

### What lockstep needed beyond command capture

Identical commands at identical steps were not enough; the mod patches four
places where the engine's order depended on memory layout or on a seed:

| | RVA | steal | |
|---|---|---|---|
| train reservation order | `0xabe02d` | 16 | the engine shuffles the order trains claim track with a `minstd_rand` seeded from `GameTime+0x30` over node-list positions, which differ per machine; the detour orders by train name with a seeded jitter |
| free space on a road edge | `0x2117350`, `0x2117140` | 5 | the sum is taken in ascending order in `double`, so every peer gets the same float |
| road edge use entries | `0xa64473` | | kept in name order after `EdgeUseManager::Add` |
| ship and aircraft claim order | `0xa6c1e0`, `0xa2bc60` | 5 | measured only: the family node vector is engine-owned |

The world comparison that finds the remaining divergences hashes geometry and
state, never entity ids: ids, seeds and town growth differ legitimately
between machines that agree on the world.

### UI patches for companies mode

Small in-place patches, each verified against the exact bytes at the site
before it is applied: the line editor's station owner gate (`0x609631`, a
5-byte `cmp eax,[rbx+0x28]; je` with accept `0x609605` and reject `0x609636`),
three owner gates that hide other players' icons, the icon draw call
(`0x80b613`), the station label background (`0x80a0ee`), a foreign entity's
window opening read-only (`jne` at `0x8b3060`), the window bind (`0x8b2390`),
and the HUD station and depot icons (`0x5e38e1`, `0x5e45d0`, depot ctor
`0x5e2b70`). The `setPlayer` binding's ownership assert is bypassed at the
`je` `0x11677a1`. Each is a separate patch with its own byte check, so a build
change disables one feature rather than the mod.

### Detour rules that held up

- **Verify, then steal.** Every target's expected bytes are compared before
  the patch; a mismatch logs and leaves that feature off. Steals stop on an
  instruction boundary at or past 14 bytes and cover only plain,
  position-independent instructions; a `call`, a jump or a RIP-relative
  operand in the prologue is a refusal. Where only five bytes are safe to
  take, a page within ±2 GiB of the site is allocated for the detour and the
  five bytes become a `jmp rel32` into it.
- **Install before the target's first run.** The mod's proxy `alut.dll` loads
  every DLL from `DllMain`, before the game's entry point, so no thread can
  be inside a target when it is patched.
- **The cancel is gated on evidence.** Cancelling is only safe because
  something replays, so it is switched on by fresh evidence from the script
  half on disk (its per-tick status file). With the mod's Lua side absent, the
  hooks capture nothing and cancel nothing, and the base game is unchanged.

The GUI hook exposes `copy(text)` for the room invite. The lobby uses the
local `copy` action; clipboard errors are reported and never sent to the server.

### Upgrade diagnostics

Road and track modifiers log a bounded summary when handed to the room and
again when applied in each game. The summary identifies the modifier and
edge count without changing its payload. Track proposal tests use the Lua
API stand-in; they do not establish native track-tool acceptance. Action
handoff logs also include the action kind, without a wire format change.

## Company tool integration (2026-10-03)

The newer company tool, stop ownership/naming, station junction and asset
bulldozer changes from `local/combined-dev` through `34a2edf` are ported onto
current dev. The snapshot exporter and duplicate contributor guidelines
were already present. Existing loading behavior, automatic-company policy,
HUD callback bounds and acceptance gates are retained. The older branch's
finance-window wrapper is not added: it depends on a separate loan-table
implementation that is absent here.

The first company also gets the map's all-companies display; its selection
and editing checks still use its own player. The line-viewer probe diagnoses
unrendered route data and does not fix it. Tree/rock bulldozing, terrain
paint and the asset brush go through the room (2026-10-16). Action schema 25 adds
`PlaceStop::name` and `Bulldoze::Assets` to dev's existing schema 24, without
renumbering its perk or preservation actions. Native playtest observations
above are the contributor's evidence; this integration has no new live-game
acceptance run.

### Startup readiness and stalled menu recovery

On Windows the launcher keeps the main thread suspended until hook bootstrap
finishes, including the main-menu `RegisterAppUsertypes` and world-loading
detours. Installing only the lobby entry before resuming can miss the menu's
Lua state: the save downloads, but the game never loads it. The launcher waits
up to 120 seconds; a missing readiness event or timeout stops the suspended
process with an error instead of running with incomplete hooks.

Menu room control continues even if its Lua loading state is unavailable,
provided the native checks establish that no world is loaded or loading.
This keeps Start and Leave processing alive; entering menu Lua remains guarded
by the loading API's state checks. No game simulation runs from this path.

The launcher must continue servicing the shared-memory Hello handshake while
Windows waits for readiness. Launch runs on a blocking worker; the async
launcher pumps the idle link through Hello and keeps its heartbeat alive until
startup completes. Otherwise hook attachment waits for a launcher reply while
the launcher waits for hook attachment, causing multiplayer to disable itself.
Menu actions after Hello stay queued for the normal launcher loop. An existing
room session continues servicing its own link on the async runtime.
