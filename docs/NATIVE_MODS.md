# Native mods: trusted indexes, installed by the launcher

*ReedME fork policy* ([DECISIONS.md](DECISIONS.md), D31, 2026-10-20)
adopts the external-plugin model with exact source/content consent. D29's
2026-10-15 amendments below describe its upstream origin, not ownership of
this fork. Current integration uses protocol **24**, bridge **30**, action
schema **28** and room-log format **11**. The server registry, Mods browser,
full package identities in room terms (introduced upstream in protocol 22),
Windows x86-64 plugin loading and per-content executable consent are implemented. The Big Maps plugin is supplied as source and
packaging tooling; publishing a package and real-game acceptance remain
separate work. No production registry is configured by this change.

A **native mod** is a mod that is more than Lua: it needs code in the game.
Big Maps ([BIGMAPS.md](BIGMAPS.md)) is the first: hook patches behind
switches, targets in the build's hook profile, a New Game page the hook
serves, settings, and a Lua mod folder. Mod Hub carries Lua mods for the
game's own loader, and a room's Mod Hub mods are handled by D28
([LOBBY.md](LOBBY.md), [MODS.md](MODS.md)); native mods come through a
channel of their own, managed the way CKAN manages Kerbal Space Program
mods. The source is either the room server's authenticated index or the
project's optional signed index, never an arbitrary pasted index URL.

## The index

`native-mods.json` lists packages in the optional project index. Its
`native-mods.json.sig` is an Ed25519 signature of the exact bytes; both are
assets of the repository's `native-mods` release
(`releases/download/native-mods/`), fetched over HTTPS only. A room server
serves the same schema over its authenticated connection and vouches for
its own index, which has no project signature.

```json
{
  "format": 1,
  "serial": 12,
  "packages": [
    {
      "id": "bigmap",
      "version": "0.3.0",
      "name": "Big Maps",
      "description": "Maps up to 128 km a side.",
      "homepage": "https://github.com/<its authors>/<its repository>",
      "simulation": true,
      "builds": ["<SHA-256 of the game's executable>"],
      "features": ["bigmap.octree", "bigmap.street_raster", "bigmap.placement",
                   "bigmap.page", "bigmap.density", "bigmap.memory_gate"],
      "settings": {"octree_depth": 10, "street_raster": true},
      "depends": [{"id": "other", "version": "^1.2"}],
      "conflicts": ["another_big_map_mod"],
      "files": [
        {"path": "mod/tpf3mp_bigmap_1/mod.lua",
         "url": "https://github.com/…/releases/download/native-bigmap-v0.3.0/mod.lua",
         "size": 1234, "sha256": "<lowercase hex>"}
      ],
      "plugins": []
    }
  ]
}
```

| field | meaning |
|---|---|
| `format` | 1. Another format is refused whole: a launcher that does not know a format does not guess. |
| `serial` | Grows with every index published. The launcher remembers the highest it accepted and refuses an older one, so a withdrawn package cannot come back by replaying an old signed index. The registry also binds an accepted serial to the SHA-256 of the exact index bytes; a crash after writing a complete cache pair recovers that serial floor, and different contents cannot reuse it. |
| `id` | 1 to 40 lowercase letters, digits and `_`. |
| `version` | Semantic version, at most 20 characters (it travels in the room's terms). |
| `homepage` | Optional. The mod's own project page, such as its authors' repository: the launcher links to it, as CKAN links to a mod's. An `https` address of at most 200 characters. The files still come only as the trusted index pins them, wherever they are hosted (the mod's own releases included). |
| `simulation` | Whether it changes how the world runs. A feature the hook knows to change the simulation makes the package count as changing it too, whatever this says. |
| `builds` | The game builds it runs on, by the executable's SHA-256, as hook profiles pin them ([HOOKS.md](HOOKS.md)). On any other build it is not installed, and not enabled if it was. |
| `features` | The hook features it enables (below). |
| `settings` | Its settings with their defaults: booleans, integers or text of at most 128 bytes (a room tells them to its members). The launcher has no controls for them: a mod runs with its defaults, or with the room's settings once it joins a room that sets them. |
| `depends` | Packages it needs, with a semver requirement. |
| `conflicts` | Packages that must not be installed beside it. |
| `files` | Data, Lua and settings files: where each goes in the package's folder (plain names joined by `/`), where it comes from, its size and SHA-256. |
| `plugins` | Its own code: native libraries the hook loads into the game (`path`, `url`, `size`, `sha256`; `abi`, the hook's plugin interface they are built against; `platform`, such as `windows-x86_64` or `linux-x86_64`). Only Windows x86-64 loading is currently implemented. A package with any must say `simulation: true`. See "Plugins" below (approved D29 amendment). |
| `hooks` | The places in the game's code it patches, each found by signature: `target` (`<id>::<name>`), `kind` (`detour`, `splice`, `rewrite`, `call` or `slot`), `signature`, `offset`, `expected` (the bytes there), `use` (`chain` or `replace`), and per kind `steal`, `replacement` or `slot`. See "Plugins" below. |
| `game_mods` | Folders of its files that are game mods (`mod/<name>_<n>`): the launcher puts each into the game's `mods` folder while the package is on. See "Plugins" below. |

Unknown fields are refused, and one malformed entry refuses the whole index:
its trusted publisher must fix a mistake; the launcher never guesses around it.

### Signing

The index is signed the way releases are (D7), with the same code: the
launcher's updater and the index call `tpf3mp_nativemods::signed` (Ed25519
through `ring`, public keys as base64 strings, more than one so that a key
can be changed). The key is the **native-mods key**, a key of its own as the
dev key is (D19): the public half is built into the launcher from
`TPF3MP_NATIVE_MODS_PUBLIC_KEY`; without one, the optional project source is
unavailable, but an authenticated server index remains usable. A release
key cannot sign the index, and the native-mods key
cannot sign a release.

To be built with the decision: a `sign-native-mods.yml` workflow that signs
the index in a GitHub environment whose required reviewer approves each
signing, as `sign.yml` does for releases. No key is in the repository; the
tests make a fresh test key each time.

## A server's own index

*Owner-approved amendment to D29 ([DECISIONS.md](DECISIONS.md), 2026-10-15).*
A server can offer native mods without the project's key:
its operator names a **registry** (`--native-mods-registry`,
[OPERATIONS.md](OPERATIONS.md), "Native mods"), and the server makes an
index of it and serves it to its players (protocol 21,
[PROTOCOL.md](PROTOCOL.md)). The launcher trusts it as it trusts the
server's rooms (D4).

The registry is a git repository, as CKAN's metadata repository is for
Kerbal mods, holding one file, `mods.txt`: one address of a mod's own
repository per line, `#` for comments.

```text
# Native mods this server offers
https://github.com/<author>/tpf3mp-bigmap
https://gitlab.com/<author>/trees
```

Each mod's repository holds `tpf3mp-native.json` at its top, read from its
default branch (GitHub's and GitLab's raw files; any other host by naming
the file's own address in the list). It holds the mod's packages in this
page's format. What players see of a mod in the mod browser comes from it:
its name, its description (at most 500 bytes), its `authors` (at most 8,
each at most 48 bytes; the repository's owner if it names none), and its
`preview`, a PNG or JPEG image, either a path in the repository or an
`https` address. The repository is each package's `homepage` unless the
file names another:

```json
{
  "format": 1,
  "packages": [
    {"id": "bigmap", "version": "0.3.0", "name": "Big Maps",
     "description": "Maps up to 128 km a side.",
     "authors": ["<author>"], "preview": "images/preview.png",
     "simulation": true, "builds": ["<SHA-256 of the game's executable>"],
     "features": ["bigmap.octree"], "settings": {"octree_depth": 10},
     "files": [{"path": "mod/tpf3mp_bigmap_1/mod.lua",
                "url": "https://github.com/<author>/tpf3mp-bigmap/releases/download/v0.3.0/mod.lua",
                "size": 1234, "sha256": "<lowercase hex>"}]}
  ]
}
```

The server reads the list and every mod's file again every hour by
default. Each mod stands on its own: one whose file cannot be read, holds a
malformed package or claims an id an earlier mod in the list has is left
out, with the reason in the server's log; the others are served. Its index
is numbered by the time it was made, always past the one before, and the
launcher refuses one older than it kept from that server.

A refresh shares a five-minute budget across the list, mod files, GitHub
listings and proposals, with at most 1,024 requests and 128 MiB of response
allowance (a failed read charges its response cap). Each in-flight request
still has its 60-second timeout. If the shared
budget is exceeded, the server keeps the previous complete index and tries
again at the next refresh; it does not publish a partial replacement.

**Downloads and dates.** For a mod on GitHub the server also asks GitHub
for its releases: how often their files were downloaded, all versions
together, and when the newest was published (with no release, when the
repository was last pushed to). The index carries them as `listings`, by
mod id, and the mod browser shows them; they are never used to choose what
is installed. A mod GitHub does not answer for has none, with the reason in
the log.

**Proposed mods.** A mod joins the registry through a pull request that
adds its repository to `mods.txt`. For a registry on GitHub the server also
reads the open pull requests and serves the mods they would add apart, as
*proposals*, each with its pull request (`registry::Proposals`). In the
launcher's mod browser **Show proposed** lists them, marked
"Proposed in pull request #N: not reviewed" with a link to it; a player
installs one only by choosing it. A room never gets a proposed mod by
itself: its members install it the same way, by choosing it.

**What the launcher does with it.** On connecting, and on **Check the
index**, it reads the server's index (and with proposals shown, the
proposals), keeps them per server (`native-mods/servers/<…>/`), and
installs from the server's index first, then from the project's signed
index when it was built with the key. It gets a room's native mods by
itself only from a server the release lists (or with the project's key);
from a server the player set in Settings it does so when the player presses
**Get the room's native mods**.

## Installing

The launcher keeps native mods in its own data folder, never the game's:

```text
<launcher data>/native-mods/
  native-mods.json, native-mods.json.sig   the last index accepted
  registry.json                            every installed package and its files
  enabled.json                             what the next game runs
  <id>/<version>/…                         a package's files
  <id>/.partial-<version>/                 a download in progress
```

`store::Store` does every step, refusing rather than guessing:

1. **Resolve** (`resolve`): the wanted package's newest version pinned to the
   running build, then its dependencies (an installed version is kept when it
   satisfies the requirement). Refused: a package the index does not list,
   none for this build, one whose plugin this TPF3-MP cannot load, one
   naming a feature this hook does not have, a dependency no version
   satisfies, a dependency cycle, a
   conflict with a package chosen or installed. There is no backtracking: an
   install that does not hold together is refused with the reason.
2. **Download** each file into `.partial-<version>/`, checking its size and
   SHA-256 on the way (`fetch`, the updater's code). More bytes than signed
   stop the download; a short or changed file fails it.
3. **Move into place** only when every file checks out: the partial folder is
   renamed to `<id>/<version>/`. A failure deletes the partial folder; the
   version in use is untouched.
4. **Register**: the version becomes the one in use, the one before it is
   kept for a rollback, and any older one is deleted. The registry is written
   through a temporary file, so it is whole or not written.

- **Upgrade** is an install of a newer version: the old one stays in use
  until the new one has verified, then stays on disk for a rollback.
- **Rollback** goes back to the version before, after checking its files are
  still whole.
- **Uninstall** deletes exactly the files the registry names, then the
  folders that leaves empty; a file someone else put there stays. A package
  another installed package needs is not uninstalled.
- An install cut short after the rename leaves a folder the registry does
  not name: the next install takes it if every file is the package's, and
  refuses (`Occupied`) otherwise.
- A registry that cannot be read is an error, never replaced by an empty
  one.
- Registry package paths are validated before use. Traversal paths and paths
  that cross a symlink, including a symlink above the store root, are refused
  before reads, writes, or deletes.
- **Installing is not enabling.** The player switches a package on
  separately.

## What the game runs

When the launcher starts the game (D11) and the player has enabled a native
mod, it hashes the game's executable, writes the enabled packages for that
build to `enabled.json` with their settings (the defaults with the player's
choices over them) and their folders, and names that file in the game's
environment as `TPF3MP_NATIVE_MODS` (`tpf3mp-agent`'s `native_mods`). A
package that changes only what one player sees and is not pinned to that
build is left out with a log message. An enabled simulation-changing
package that is not pinned to that build stops the start instead of
producing an empty list. Nothing enabled, nothing installed: no variable,
and the executable is not hashed. A registry that cannot be read stops the
start. Before writing `enabled.json` or declaring room terms, the launcher
checks every enabled package's installed file set and bytes against its
recorded signed manifest; changed, missing or extra files stop the start.

The hook makes its plan once, at bootstrap, after matching the build's
profile and before installing anything (`tpf3mp-hook`'s `native_mods`,
`enabled::plan`). A package's features run only when:

- the file names the build the hook runs in;
- this hook has every feature the package names
  (`tpf3mp_nativemods::features::BUILT_IN`), and the matching profiles have
  every target those features patch;
- for a package that changes the simulation, the room compares native mods
  (below);
- its features do not clash with those of a package before it (below,
  "Several mods on one target").

Otherwise a package that changes only what its player sees is left off, with
the reason in `hook.log`; one that changes the simulation **keeps the game
out of rooms**: the hook installs nothing and logs
`multiplayer disabled (fail-closed): native mods that change the simulation
cannot run: …`, as for a build it has no profile for. An `enabled.json` that
cannot be read refuses the same way. No other switch turns a native feature
on: Big Maps' own branches read `TPF3MP_BIGMAP_*` variables by hand; as a
package, it would ask the plan instead (`native_mods::feature(id)`, which
gives the feature's settings and its package's folder).

### Features

The original data-only framework names **features compiled into the
hook**, by id; the hook switches on those the launcher
enabled. Each feature has an id (the package's id, a dot, the feature's), an
effect (only what one player sees, or the simulation) and the profile
targets it patches. `features::BUILT_IN` is empty on `dev`: no native feature
is built in yet. Big Maps would add its own; `features::example::BIG_MAPS`
shows the shape, and the tests use it:

| feature | effect | profile targets, and how it uses each |
|---|---|---|
| `bigmap.octree` | simulation | `bigmap::octree_root`: value, largest |
| `bigmap.street_raster` | simulation | `bigmap::street_raster`: takes it over |
| `bigmap.placement` | simulation | `bigmap::placement_attempts`: value, largest |
| `bigmap.page` | what one player sees | `bigmap::tile_count`: value, largest; `bigmap::size_list`: value, every list's entries |
| `bigmap.density` | simulation | `bigmap::density_levels`: value, every list's entries |
| `bigmap.memory_gate` | what one player sees | none |

The launcher reads the same registry to refuse a package this TPF3-MP cannot
run before downloading it.

### Load order

The launcher writes the enabled packages to `enabled.json` in the **load
order**, and the hook switches them on in that order: each package after
those it depends on (`depends`), the rest by id (`enabled::load_order`;
packages that depend on each other in a circle, which installing already
refuses, come last). It is worked out the same way on every player's game
and is not set by the player: in a room, an order each player chose would be
one more way for their games to differ. The mod manager shows it.

### Several mods on one target

Every patch is the hook's own code, so two packages never patch a game
function apart: each feature says how it uses each target it patches
(`features::Access`), and the hook combines them, much as Harmony combines
patches in modded Unity games:

| how a feature uses a target | example | two features on it |
|---|---|---|
| **value**, with a merge rule | a size limit, a list of map sizes | merged by the rule (`features::Merge`): the largest, the smallest, the sum, or every list's entries |
| **chain** | runs before or after the game's function, which stays | both run, in the load order |
| **replace** | takes the function over | they **clash**: never on together |

Features that share a target must agree on how they combine: both values
with the same rule, or both chains. The tests hold `BUILT_IN` to that
(`features::registry_problems`), so a feature added to the hook that would
combine a target differently from another fails CI rather than a game. A
clash is caught three times over:

- **switching on**: the launcher refuses to switch on a package whose
  features clash with one already on, and the mod manager says which
  (`Store::set_enabled`, "Can't be switched on: it clashes with …");
- **in the game**: the hook's plan leaves a clashing package off, or keeps
  the game out of rooms if it changes the simulation, as for a missing
  target (`enabled::plan`), in case a launcher and hook of different
  builds disagree;
- **in the hook's code**: a feature asks the plan which active features
  share its target, in the load order (`Plan::users`), and merges their
  values (`Merge::numbers`, `Merge::lists`) or runs their chains in turn.

`conflicts` in the index remains for packages that must not be installed
together for reasons the hook cannot see.

## Plugins

*Owner-approved amendment to D29 ([DECISIONS.md](DECISIONS.md), 2026-10-15).*
A package may carry code of its own: **plugins**, native
libraries the hook loads into the game, which patch the game functions the
package declares. Built-in features stay for what the project maintains
itself; a plugin is how a mod's authors ship their own.

```json
{"id": "faster_trains", "version": "0.4.1", "name": "Faster Trains",
 "simulation": true, "builds": ["<SHA-256 of the game's executable>"],
 "plugins": [{"path": "plugin/faster_trains.dll", "platform": "windows-x86_64",
              "abi": 1, "url": "https://github.com/<author>/faster-trains/releases/download/v0.4.1/faster_trains.dll",
              "size": 81920, "sha256": "<lowercase hex>"}],
 "hooks": [{"target": "faster_trains::max_speed",
            "signature": "48 89 5C 24 ? 57 48 83 EC 20 8B FA",
            "offset": 0, "expected": "48895C2408", "use": "chain"}]}
```

**Kinds of hook.** Each hook says how it patches (`kind`), and each is
found by `signature` (one match in the game's code, else refused) and
`offset`, and checked against the bytes it `expected` there before
anything is written. A mod never names an address: addresses change with
every game update and cannot be checked.

| `kind` | what it patches | what the plugin gives (`tpf3mp-plugin-abi`) | several on one site |
|---|---|---|---|
| `detour` (default) | a function's start | a function with the game's signature, through `attach`, which calls on through its slot | chained in the load order |
| `splice` | the middle of a function: `steal` (5 to 32) whole instructions of those `expected` | a callback through `splice`: it gets every register (`Registers`) and may change them; then the stolen instructions run and the function goes on | every callback, in the load order |
| `rewrite` | bytes replaced one for one: an operand, a constant | the bytes through `rewrite`, or none when the package gives them (`replacement`) | only the very same bytes from the package |
| `call` | one `call rel32` (`expected` is `E8` and its 4 bytes) | a function through `attach`, which calls on to the old callee | chained |
| `slot` | one entry of a table of function pointers (a vtable): the `expected` instruction addresses the table (`slot`: its 4-byte displacement `disp_at` bytes in, the instruction `length` long), entry `index` | a function through `attach`, which calls on to the old one | chained |
| `find` | nothing: a function the plugin calls or whose code it checks, found and checked like the others | nothing; `address` gives where it is | any number; it writes nothing, so it overlaps nothing |

`use: replace` makes a hook the only one on its site. Two hooks on
overlapping bytes at different sites refuse, and so does any hook within
32 bytes of a target the hook patches itself for multiplayer.

**What else a plugin gets.** While it starts, a plugin may hand over Lua
(`lua`), which the hook runs in each of the game's Lua states after its
own, the way it serves the main menu: Lua that wraps `resolveutil.loadfile`
can serve a page from the mod's game mod, or wrap a game script. For the
game's whole run it keeps `Services`: guarded reads of the game's memory
(`read`, which refuses memory that cannot be read rather than crash), the
game's update count while its step runs (`update_count`), and the log.

**Game mods.** A package's `game_mods` (`mod/tpf3mp_bigmap_1`, among its
`files`) are put into the game's `mods` folder when the launcher starts the
game with the package on, and taken away when it is off
(`Store::sync_game_mods`). Each holds `.tpf3mp-native-mod`, which names the
package; a folder without it is a player's own, never touched, and a
package whose game mod would take its name is refused.

**How it runs.** At bootstrap, while the game is still suspended and
before the hook's own multiplayer patches (`tpf3mp-hook`'s `plugins` and
`native_mods::load_plugins`), for the enabled packages in the load order:

1. every declared hook is found and checked, a call's old callee and a
   slot's old function read;
2. the hooks are checked against each other and against the code the hook
   patches itself, as above;
3. each plugin file is checked again against its pinned SHA-256, loaded,
   and started: its one export, `tpf3mp_plugin_init`, gets a `Host` and
   asks for its hooks by name;
4. each site is patched once, as the table says. A package of `rewrite`s
   whose bytes it gives itself needs no plugin.

Any step that fails stops before anything is patched (a patch that fails
takes back those made) and keeps the game out of rooms (`hook.log` says
why). Plugins load only into the game on Windows x86-64 yet; elsewhere a
package with one keeps the game out of rooms.

**Big Maps as a plugin.** `crates/tpf3mp-bigmap-plugin` is Big Maps
(`feat/bigmaps-40420`, the hook's `bigmap` module there) moved out of the
hook into a package for build 40420: its code asks the hook for the hooks
its package declares (`manifest::hooks`) instead of patching the game, and
reads its switches from the package's settings (`street_raster`,
`placement`, `emission_every`, `emission_cell`, `memory_gate`) instead of
`TPF3MP_BIGMAP_*` variables, so a room's terms carry them.

| Big Maps' patch | hook | kind |
|---|---|---|
| octree root, new game and load | `bigmap::root_init`, `bigmap::root_load` (after `call Resize`) | `splice` |
| `OctreeSystem::Resize`, which it calls again | `bigmap::resize` | `find` |
| depth 12: the descent's ids, the level decoder | `bigmap::descent`, `bigmap::decoder` | `detour`, `splice` |
| the descent's starter, checked | `bigmap::descent_start` | `find` |
| street raster | `bigmap::obstacle` | `detour` |
| placement spacing | `bigmap::spacing` | `detour`, `replace` |
| emission throttle | `bigmap::grid_slot`, `bigmap::emitter_slot`, each checked to hold `bigmap::grid_update` / `bigmap::emitter_update` | `slot`, `find` |
| emission cells | `bigmap::grid_shift_*`, `bigmap::grid_factor`, in `bigmap::create_grid` | `rewrite`, `find` |
| New Game page, density levels | Lua for the game's Lua states, from its game mod `tpf3mp_bigmap_1` | |

`cargo test -p tpf3mp-bigmap-plugin --test game_40420`, with
`TPF3MP_TF3_BUILD40420_EXE` naming the game's executable, finds every hook
in it with the hook's own `plugins::locate`, arranges them against every
target the hook's 40420 profile patches, and runs the plugin's entry with
a host that answers from the executable's bytes, every switch on: every
check the plugin makes in the game passes there. `cargo build --release -p
tpf3mp-bigmap-plugin` builds the DLL; `cargo run -p tpf3mp-bigmap-plugin
--bin tpf3mp-bigmap-package -- <version> <dll> <base address> <folder>`
writes the package's folder and its `tpf3mp-native.json`. Not yet run in
the game itself. With Big Maps' throttle on, the two vtable entries
`simperf`'s timers wrap are its gates, and those two timers stay out.

**In a room.** A plugin's code, and any patch of the game's code, may
change anything, so a package with either changes the simulation, whatever
it says: it must say `simulation: true`, and its term's digest covers its
plugins' SHA-256s and its hooks as well as its settings
(`terms::package_digest`). A room's members run the same bytes, or are
told they differ.

**In the launcher.** A mod with a plugin is marked **Runs code** in the mod
browser and manager, with "Runs its own code in the game, with your
account's rights on this PC. Install it only from a server you trust." One
whose plugin is for another platform or another plugin interface, or that
needs a hook feature this TPF3-MP lacks, says why and is not offered for
install. Two packages whose declared hooks name the same function, one
taking it over, cannot both be switched on.

**Trust.** Plugins come from the same places as every native mod: the
project's index or the server's own (approved D29 amendment, "A server's
own index"), and the launcher trusts a server's registry for code as it
does for rooms (D4). Executable packages require the consent below, even
when a room needs them. A proposed mod also requires the player to choose
that proposal explicitly. A plugin runs
inside the game with the player's rights: there is no sandbox, and a bad
one can crash the game or worse. Consent and hash verification identify
the approved content; they do not sandbox it.

**Explicit consent.** Before downloading or installing executable plugin
bytes, switching a plugin-bearing package on, rolling it back, or matching a
room that needs it, the launcher resolves the full dependency closure and asks
the player to approve every executable package. The prompt shows each package's
name, version, authors, validated source (the project index or authenticated
server address), and full SHA-256 package identity. That identity includes the
package files, plugin binaries, declared settings, and the effective settings
being approved. A dependency's plugin is shown even when the requested package
itself has no plugin. Approval is remembered for that exact source and identity
across reconnects; a changed source, binary, package, or effective setting gets
a new prompt. The launcher refreshes and resolves the selection again after the
player approves, before it installs or enables anything. Declining or failing
that revalidation leaves the package action unapplied and blocks a room join
that needs it.

Cached data-only packages can still be installed while disconnected. A
server plugin whose exact identity lacks approval requires reconnecting to
that server to refresh its metadata before the consent prompt is offered.

Executable plugins currently load only on Windows x86-64. Linux and macOS
refuse plugin packages during resolve, install, enable, rollback, and launch,
even if a signed index lists a plugin built for those platforms. Data-only
native packages remain available where their platform support allows them.

**Writing one.** Build a library for the platform (`cdylib` in Rust) that
exports `tpf3mp_plugin_init`, an `Init` from `tpf3mp-plugin-abi`:

```rust
use std::ffi::c_void;
use tpf3mp_plugin_abi::{Host, OK};

static mut NEXT: *const c_void = std::ptr::null();

// The game function's own signature and calling convention.
unsafe extern "C" fn max_speed(this: *mut c_void, kind: i32) -> f32 {
    let next: unsafe extern "C" fn(*mut c_void, i32) -> f32 = unsafe { std::mem::transmute(NEXT) };
    unsafe { next(this, kind) * 2.0 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tpf3mp_plugin_init(host: *const Host) -> i32 {
    let host = unsafe { &*host };
    let name = "faster_trains::max_speed";
    let status = unsafe {
        (host.attach)(host.ctx, name.as_ptr(), name.len(), max_speed as *const c_void, &raw mut NEXT)
    };
    // A splice: `(host.splice)(host.ctx, name, len, callback, user)`, the
    // callback `unsafe extern "C" fn(*mut Registers, *mut c_void)`.
    if status != OK { return status; }
    OK
}
```

## In a room

A native mod that changes the simulation must run in every game of a room,
in the same version, with the same settings; one that changes only what a
player sees is that player's own, as personal mods are ([MODS.md](MODS.md),
D25).

The room's **native terms** are, for each simulation-changing package the
game runs, its id, its version and a full SHA-256 package-content identity,
ordered by id. Since protocol 22 they travel in the content manifest's own
list, `ContentManifest::native` ([PROTOCOL.md](PROTOCOL.md), "Native mods"):
each as a mod named `native:<id>` (no mod folder can be named so; `:` is not
allowed in a Windows file name) whose version is `<version>+<10 settings
digest hex digits><64 package-identity hex digits>`, at most 95 characters.
The first ten digits let the server check that `RoomConfig::native` carries
the settings named by the term; they are not the content identity. The full
identity hashes the package id and version, its simulation flag, features
in their declared order, effective settings, sorted dependencies, and the
package files sorted by path with each path, size and declared SHA-256. Download URLs and
display metadata are excluded because they do not change the package's
contents or simulation behavior. A member whose terms differ cannot start
or join, and is told which native mod is missing, which differs and which
the room does not run. Ordinary game mods remain limited to 32-character
versions; protocol 22 rejects the old short native term form for new joins.

What a game declares is what the launcher started it with: the terms of the
`enabled.json` it wrote for that game (`tpf3mp-agent`'s `native_mods`), and
with no game running from the launcher, what the next game will run. The
room's owner also tells the room every setting of each (`RoomConfig::native`,
`RoomMods::native`), so a member's launcher can match them.

**Joining a room** gets the room's native mods by itself:

1. The room tells its native mods on joining its lobby, or before refusing
   a join to its running game.
2. The launcher installs each in the room's exact version from the trusted
   server index or optional project-signed index, for this game's build,
   **never from another player** (`match_room`, `Store::install_exact_from`).
   Before any mutation it checks every full room identity and obtains
   executable consent for the entire dependency closure, using the room's
   settings for each package. Declining leaves the join blocked. It verifies the installed files
   against that index even when the id and package version already match,
   replacing a stale same-version copy or refusing before join; it sets the room's settings over the
   defaults, the player's own choices left out; switches each on; and
   switches off any other simulation-changing native mod. One that changes
   only what the player sees stays as it was. A version the index does not
   have for this build, or settings that do not come out the same, refuse:
   that player cannot join, and the window says why.
3. **The game starts again with them**, since the hook reads the enabled
   list once, as the game starts: when the game running from the launcher
   runs other native mods, the launcher leaves the room's lobby, closes the
   game, starts it again and joins the same room (invite and password as
   the player gave them). A refused join to a running game does the same
   before it tries again. With no game running, the player declares the
   room's terms at once. This happens only on joining a room, and only when
   native mods were installed or switched; nothing else closes the game.
   The room's owner is never asked to: their game's terms are the room's.
4. The lobby lists the room's native mods with whether this game runs each
   alike, and offers **Get the room's native mods** when an attempt failed.

A room restored after a server restart keeps its native mods, their full
settings and its exact scan plan in the fork's format-11 start record, so a
new member's launcher installs and enables the exact room terms. Formats 8,
9 and 10 are refused before any recovery mutation or snapshot collection;
upstream format 9 and fork format 9 are distinct layouts. Keep the complete
old data and snapshot directories with their matching binaries, and use
fresh data and snapshot directories for this upgrade. The fork does not
apply upstream's legacy layouts or invent settings, identities or defaults.

Mods from Mod Hub are not native mods: players get those through their own
game and Mod Hub account (D28).

## In the launcher

Two buttons beside Settings, at the top of the window, open the mods
dialog, with tabs between its two pages. Each is a table of mods, a row
each with its preview image, name and authors, beside the details of the
one picked: its preview, large, its description, its project page
(`homepage`), what it is (version, size, and for the browser downloads and
date) and what can be done with it.

- **Mod manager** (the **Installed** tab): the native mods installed, with
  their version, size, **load order** (where each enabled one comes in the
  order the game loads them, "Load order" above; it is not set by the
  player) and whether each is on, has an update or clashes with one on;
  **Update**, **Switch on**/**Switch off**, **Roll back to** the version
  before, and **Uninstall**.
- **Mod browser** (the **Browse** tab): each package the last accepted
  index has for this game's build, with its version, download size,
  downloads and when it was last updated, to search by name, author or
  description; **Install** or **Update**. With **Show proposed** it lists
  the server's proposed mods too, marked as not reviewed and linking to
  their pull request.

Preview images are fetched as they are shown, over HTTPS only, at most
4 MB, PNG or JPEG, and kept for the session (`tpf3mp-launcher`'s
`previews`); a mod without one shows its initials.

Neither has controls for a mod's settings: a mod runs with its defaults,
or with the settings of the room it was matched to. **Check the index**
fetches the index again. A change takes effect in the next game the
launcher starts; with no game running, the player declares the new terms
at once.

## Framework built, player feature still open

Framework implemented (`crates/tpf3mp-nativemods`, with tests):

- the index format, signature verification primitives and validation, a
  serial floor bound to exact index bytes, and recovery from a complete
  cache written before a crash;
- resolution: builds, dependencies, conflicts, cycles, features, plugins;
- the store: checked downloads, the registry, upgrade, rollback, uninstall,
  enabling and settings, `enabled.json`;
- the hook's plan and its fail-closed refusal (`tpf3mp-hook`), and the
  launcher passing `TPF3MP_NATIVE_MODS` to the game it starts
  (`tpf3mp-agent`);
- helpers for room-term comparison and finding a missing package in a
  signed index;
- the launcher's updater uses the same signature and download code.

Built on it (introduced upstream in protocol 22, carried by fork protocol
24): room terms with full package identities and
settings; the server registry and Mods browser; side-effect-free dependency
resolution; source/settings/content-bound plugin consent; verified installs,
rollback and enablement; and Windows x86-64 plugin loading. The Big Maps
plugin and its package builder are included. Enabled packages' game-mod
folders are installed without overwriting a player's own same-named folder.

Still open before a player release:

- configure a server registry and publish the first reviewed package;
- if the optional project source is wanted, configure its native-mods key,
  signing workflow and first index;
- validate ordinary game startup, room matching and Big Maps gameplay in
  the real game. Automated and static checks do not establish gameplay or
  performance; this integration intentionally defers those tests;
- plugin loading on platforms other than Windows x86-64.
