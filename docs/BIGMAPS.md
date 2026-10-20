# Big maps: world layout, loading and memory, from tpf2-bigmap

What [tpf2-bigmap](https://github.com/silver2127/tpf2-bigmap) (0.5.2,
September 2026) found while pushing Transport Fever 2 build 35924 past its
map-size ceilings: how the world is laid out, which structures scale with map
area, what a load actually does, where the time and the memory go, which
optimisations held and which did not. The plugin's own account is its README
and the `docs/` folder of that repository (`runtime-memory-audit.md`,
`generation-peak.md`, `terrain-compression.md`, `alignment-batch.md`,
`load-speed-todo.md` and the per-feature notes); this page is the part that
carries into a TPF3 room where several machines have to load, hold and save
the same world.

Tagging follows the source: **measured** means read from a running game, a
process inspection or the game's own log; **derived** means computed from
decompiled code and not yet observed live. Sizes are for a 256 by 256 tile
world (65,536 tiles, 65.5 km on a side) unless stated; a 512 by 512 world is
four times the tiles. RVAs and offsets are TPF2's and will not survive into
TPF3; the shapes and the ratios are the point.

## How the world is laid out

- **A tile is 256 m.** Measured: a 224-tile map reports a 57,344 m bounding
  box. The New Game menu turns two dropdown indices into a tile count, and
  the heightmap is `tiles * 64 + 1` pixels a side, so the base heightmap is
  4 m per sample. The shipped presets are 18 by 54 up to 96 by 96 tiles
  (Megalomaniac, 24.6 km square, 604 km²); every Megalomaniac variant
  conserves that area as the ratio stretches. Tile counts are in the save
  header (two ints after the `tf**` magic, zstd-compressed).
- **Terrain has three resolutions.** The 4 m base heightmap (65 by 65 uint16
  per tile, persisted); a 1 m height cache per tile (257 by 257 uint16,
  132,098 bytes, rebuilt on every load by a bicubic refine and then cut by
  every road, track and construction alignment); and per-tile render data
  (a material-index grid of 260 by 260 bytes per tile, LOD tessellation
  patches). The save holds the base heightmap and the alignments, never the
  finished 1 m cache.
- **Every entity lives in one octree** whose root is a two-tier constant, not
  derived from the map: ±16,384 m at depth 9 for up to 128 tiles, ±32,768 m
  at depth 10 above that, 128 m leaves. Inserts never test containment; an
  entity past the root walks to a boundary leaf, and every query prunes on
  node boxes first, so anything beyond the box is invisible to lookups.
- **There are two complete game states**, swapped every simulation step
  (`GameState::Replicate`), so everything area-scaled below exists twice:
  collision rasters, emission grids, tree and asset instance lists, the
  octree. Terrain tile caches are refcount-shared between the two, not
  copied, in steady state.

## The ceilings, in the order they are hit

| ceiling | cause | symptom |
|---|---|---|
| 224 tiles | the menu clamps both axes (`GetNumTilesNew`); `settings.lua`'s undocumented `worldDimensionsOverride = {224, 224}` bypasses the preset table but not the clamp (both values must be even) | none: it is the menu |
| **180 tiles** (46.1 km) | "Creating streets" sizes a `vector<bool>` with one bit per square metre over the whole map through a **32-bit multiply**; at 224 tiles `57,345²` wraps negative, sign-extends to about 1.8e19 and `resize` throws `length_error`. Uncaught, so it is `abort` with no message; the thrown object's RTTI in the minidump is the only evidence | silent SIGABRT during generation |
| **32,768 m** (256 tiles) | the octree root. Beyond it the street builder finds nothing under a position and stacks a second node on the first. Measured on a 320-tile map: 21 duplicate positions, all with `max(|x|,|y|)` between 34,175 and 40,082 m, none inside 32,768, on all four edges; 168 repair failures in half an hour; towns in the band generate with zero population because they never get streets | `Duplicate base nodes found`, printed by a load-time repair pass, so the save is already damaged |
| 185 km pairwise | town and industry placement squares candidate separations in signed int32 pixels; above `sqrt(INT_MAX)` pixels distances go negative or wrap to a small positive value | towns clustered along the middle of a long axis (seen on a 58 by 292 km preview) |
| `(tilesX*64+1)*(tilesY*64+1) <= INT_MAX` | the heightmap element count | no 1,024-tile square; long maps must be narrow |
| Lua `%d` | Lua 5.2's `%d` goes through a 32-bit C `long` on Windows | any id above 2^31 formatted from a script fails; use `%.0f` |

Two lessons from the 180-tile wall that transfer directly. Widening the
multiply would have been worse: the two dimensions are stored as int32 and
every access computes `y*nx + x`, so a correctly sized vector would still be
addressed with wrapped indices past 2^31, silent corruption instead of a clean
abort. The plugin instead scales the raster's cell size (an argument to the
constructor) until the cell count fits, so every downstream index stays in
range untouched. And the octree fix is a 13-byte in-place rewrite (a wider root
constant inline, one deeper level), because the 32,768.0f in `.rdata` sits in a
run of constants with about a hundred readers; a shared constant is never the
thing to patch. Depth 11 is the cap of the original node-id scheme (ids are
`8*parent+1+octant` in 32 bits); going deeper needs compact id ranges for the
new levels and a patched renderer decoder, which is built and tested offline
but not yet validated in a running game.

## What a big map costs

### At rest

Per-tile structures (derived from code, the material grid measured):

| structure | per tile | copies | 256² |
|---|---|---|---|
| 1 m height cache | 132,098 B | 1, COW-shared by both game states | 8.06 GiB |
| material-index grid (render, CPU copy) | 67,601 B | 1 | 4.13 GiB |
| base heightmap | 8,450 B | 1, COW | 0.52 GiB |
| emission grids (16 by 16 floats, two layers plus scratch) | 3 KB | 2 engines, copied every step | 0.38 GiB |
| collision rasters at 16, 32 and 64 m plus passable tiles at 10 m | ~2.8 KB | 2 engines | 0.34 GiB |
| trees and scenery (24 B thin, 192 B fat instances, 12 B octree refs) | content | 2 engines | ~1.6 GiB per copy on the measured desert map, about 35-45 million trees |

The material grid was the surprise: it was believed to be touched only on
terrain edits, and a live read of the process (25,992 tiles, every cell
exactly 67,601 bytes, 18% of private memory) proved it retained for every
tile. Its bytes are dithered, not flat: 34 distinct values, a median of 8 per
tile, so it compresses to about 20-25%, not the 8:1 first guessed. The render
vertices, by contrast, were believed retained (1 GiB) and are not. Measure
retention before sizing a cache for it.

Whole-process, measured: a freshly generated large stock map was 15 GiB
private at the end of generation before the plugin held a byte; a 44 MB save
of a mid-sized world loaded to about 6.6 GiB; a loaded 256² save settled at
about 18 GB working set with the first pager alone and 10.5 GB once the
material grid was paged too. A 16 GiB machine cannot hold a 128 by 128 world
however the caches are set. Their world-entry stage log (private GiB): 3.1 before allocation,
14.9 after terrain generation, 6.45 at the start of trees, 9.8 after trees,
11.6 after scenery, 13.9 at the end of `InitNewGame`.

### Generating

Terrain generation is the binding constraint when creating a map, and it
follows a printed law. The game's own teardown line, `Terrain toolkit used N
maps and X MB`, is `X = (64*tiles+1)² * N * 4 / 1,000,000` (decimal MB,
truncated), verified exactly on seven logs from 96 to 512 tiles. Each map is
one full-resolution `vector<float>`; **all N are alive at once**, because the
name-to-map container has no erase path and is cleared wholesale at teardown,
and the peak is held through asset placement, the slowest stage. Desert uses
18 maps, temperate and tropical 10. At 256² that is 1.07 GB per map, 19.3 GB
for desert; at 512² 4.3 GB per map, 77 GB. Two layer ops allocate a further one
to two maps of scratch that the print does not count.

A Lua pass over the generator's layer list (splitting temporary names into
values at full overwrites and letting values with disjoint lifetimes share a
buffer, from a verified per-op table of which ops read the old output and
which write in place) took desert from 18 to 15 buffers and temperate from 10
to 9, the lower bound for those semantics. Sharing a name serialises layers
that used to run in parallel. A compressing pager is the wrong tool here: every
layer op is a full sweep over its map, so the working set is the whole map.

### Saving

Autosaves of a 1.4 GB save took 20-22 s (zstd level 3). Level 1 with a 64 KiB
input buffer instead of 128 bytes compressed 2.79 times faster for 9.2%
larger output (a 2.56 GB payload, identical after round trip); manual saves of
a 65,000-tile world then measured 13.7-14.4 s. The room-level consequence for
a TPF3 design: the stall timeout in [PROTOCOL.md](PROTOCOL.md) is sized as
"long enough for an autosave" from a 113 MB TPF2 autosave; a big-map autosave
is an order of magnitude larger and pauses the game for 15-20 s on a fast
machine, and a snapshot of that world is 1.4 GB on the wire.

**On TF3 (build 40408)** the save writer is the same: `PushCompressor`
(`0x32d450`) reads zstd level 3 at `0x32d464` and hands its stream a
128-byte buffer at `0x32d5c4`. Stage 0 measured saves of 3.3 s at
Gigantomaniac 1:1 (164 MB) and 7.7 s for a 45 km world (438 MB). The hook
rewrites both instructions in place, as tpf2-bigmap's `save_fast` did:
`mov eax,1` for the level and `mov r8d,0x10000` for the buffer
(`crates/tpf3mp-hook/src/savefast.rs`, profile targets "save:
PushCompressor …"). Each checks its bytes first and installs alone; the
loader is untouched (`PushDecompressor` keeps reading the constant), and a
save stays a standard zstd frame any game loads. Rooms judge lane digests,
not save files, so games with and without it agree. Off by default;
`TPF3MP_HOOK_SAVE_FAST=1`, `on`, `true` or `yes` opts in (case-insensitive,
whitespace trimmed); unset or any other value keeps the game's own settings.
hook.log says what was applied
(`faster saves: zstd level 1, a 64 KiB buffer`) or that the feature is off.
The TF3 speed-up is not measured yet. Only the Steam Windows 40408 profile
has these verified targets. The 40420 bundle has no save targets, so opting
in logs that faster saves are unavailable for the build without resolving or
rewriting any site. The Preview 40418 candidate also omits them because its
signatures have not been checked. On a supported profile, a missing or
byte-mismatched site keeps that part of the game's original save settings.

### Simulating (TF3)

TF3's noise and pollution grids have 16 m cells, so their cost grows with
the area: 1,602 x 16,002 floats each on 100 x 1000 tiles, eight times
Gigantomaniac's, moved one step every update in up to three full-grid
passes (about 1.75 GB of memory traffic per update; analysis in
investigation/TF3_SIM_COST_2026-10-05.md). On every map size, the hook
runs each step as one fused pass that leaves every bit as the game's passes
would (2.1x faster offline on those grids), looks up components faster, and
times the costliest systems in the `perf: sim` line; see
[HOOKS.md](HOOKS.md), "The fast emission grid", "The game's own systems:
the `perf: sim` line" and "The faster component lookup".

## What a load actually does

The save holds the 4 m heightmap and every alignment. A load therefore
recomputes the whole 1 m terrain: the bicubic refine of every tile, then the
alignment pass, which hands the set of terrain blocks dirtied since the last
frame to `UpdateSubterrains`. In play that set is a few entries; on a load it
is the whole map, and the pass computes every block's result before
publishing any of them. Measured on a 207,360-tile save: 1,658,880 dirty
entries, about 9.9 million work blocks, 31.6 GiB live at the peak (25.8 of
them these blocks), and an "Out of memory" assert on a 94 GiB machine. Feeding
the same pass its own set in batches of 512 entries (a detour that walks the
game's `std::set` read-only and calls the original with a degenerate tree per
batch) took the peak from 34-36 GiB to 8.3 GiB. Compute and publication then
alternate, which is what the engine does per frame anyway. Load time against
stock was not measured.

A load also holds **two terrain versions**: both game states build their own
tile grid through `AddTile` (131,072 live tiles at 256²), and once filled
every tile has exactly one byte-identical twin in the other version (64,922
pairs of 65,536; measured by hashing every live tile every 10 s). The lever is
content dedup at eviction (a hash lookup instead of an encode for the twin),
not copy-on-write: the copy hook fired zero times during a load, and in play
every shared tile was written (710 of 710), so sharing was pure overhead.

Where the time goes, from a 20 ms instruction-pointer profile of a 256² save
load (about 70 s from "Loading from file" to "Initial material index
generation"), per 20 s window:

| window | top game work |
|---|---|
| t+120 | bicubic refine 11.2%, terrain alignment blend 5.4% plus its six per-call vector fills 5.3%, a 4 by 4 matrix product 3.5% |
| t+140 | the plugin's pager 24.5% (41% of the loading thread), the per-tile height block copy 10.1%, min/max scan 2.4% |
| t+160 | pager 28.5%, LOD tessellation 16.2% |
| t+180 | LOD tessellation 46.3%, pager 12.9% |

Two traps in reading such a profile. The tool's busiest-thread percentage is
that thread's share of **its own** samples: "one thread at 86% in
`UpdateLodTess`" looked like a serial loop, but 5,241 samples against 704 ticks
means at least 8 threads were inside it at once. It already runs on the
engine's pool (three quarters of the logical cores, 100 chunks), and a
plugin-side split would have gained nothing; nothing was built. And kernel
first-touch and pager-restore time is attributed to the faulting user
instruction, so a "hot" copy loop may be page faults: a 257² block copy is
1.1 µs warm and 25.7 µs into never-touched pages, and no user-mode copy
removes the second number.

New-map entry is different: on a new 256² map, town and industry **road
connections** took 127.5 s of a 230 s entry, terrain generation 54 s, trees
10.7 s, scenery 5.1 s. The connection stage mutates the network between
attempts and re-checks connectivity afterwards, so nothing in it can be reused
across attempts without changing which roads get built. A 287 s entry was
measured in another session; a room's load timeout (5 min in PROTOCOL.md) is
close to that on a big map.

## What held, and what did not

Every change below is behind its own configuration key, default off,
byte-verifies its site before patching, falls back to the original on any
mismatch, and has an offline test that runs the **original machine code**
(the exe mapped at its preferred base, or the function copied into the test
process with its constants relocated) against the replacement and compares
complete output buffers.

**Bit-identical fast paths.** Terrain heights are simulation data that every
peer must agree on, so approximate results were never an option. Each
replacement reproduces the same IEEE single-precision operations on the same
operands in the same association, only swapping operands where IEEE is
commutative, dropping only multiplications by exactly 0 or 1 (which can change
the sign of a zero and nothing else), no FMA, no reassociation, tested under
all four rounding modes:

| function | what it does | stock | fast | proof |
|---|---|---|---|---|
| bicubic refine (4 m to 1 m) | hoists the per-pixel divisions, inlines the two 4 by 4 products with zero terms dropped, packs four lanes | 245 µs per tile | 72 µs | 4,315 comparisons, 159 M samples identical |
| alignment blend | six 132 KB allocate-and-fill cycles per call become two `memset`s over a pooled buffer; an 8-wide "any weight here" test skips the dense pass | 126-800 µs | 27-532 µs | 1,374 comparisons, 12.8 M samples |
| tile min/max scan | SSE2 with a 0x8000 bias so signed min/max orders unsigned values; a 69-byte mid-function patch with a mechanical liveness proof over every exit path | 36.7 µs | 1.5 µs | 62,634 register-level comparisons |
| height block copy | one `memcpy` per row when the spans are disjoint, stock loop otherwise | 13.4 µs warm | 1.1 µs | 3,011 geometries, every aliasing shape |

**The compressed terrain pager.** Engine code keeps raw pointers into the
tile vectors, so a cache cannot move them. The pager reserves a placeholder
arena with a fixed address per tile version, backs resident tiles with
pagefile sections, and evicts by protecting the view, snapshotting through a
private read alias (no thread suspension, concurrent writers fault and wait),
encoding, and unmapping back to the placeholder; a fault decodes into a fresh
section at the same address and retries the instruction. The codec is planar
prediction, zigzag residuals and a static per-tile rANS coder with four
neighbour contexts, 6.95% of raw on real tiles (heights are in 5 cm steps, so
99.3% of residuals are 0 or ±1), with a 64-bit content hash checked on every
decode. Decode was bound by cache latency, not the coder: a 12-bit table
missed L1, and 10 bits (16 KiB) cost 0.01 points of ratio. Measured on a
256² map: 18 GB working set down to 10.5 GB.

The policy around it took as long as the mechanism. Once a second each pager
sets its resident target from four rules: while loading, everything may stay
resident down to a headroom (a seventh of RAM, 2-12 GiB), because the loader
re-reads what was just evicted; in steady state it ramps toward a hot budget
but grows when the engine faults 300 or more evicted tiles back per second and
keeps the level the stutter drove it to as a floor that decays over minutes;
a cap of a quarter of RAM; and a commit-pressure throttle that is **sticky**
for at least 30 s and until free commit clears the threshold by more than the
pager itself gave back. Without stickiness, on a machine with no page file
sitting 1 GiB under the threshold, the pager's own 3 GiB release cleared it,
it re-expanded, and the flag set again: 74 flips in one session, each
re-inflating the terrain in front of the camera. A flat 12 GiB reserve tuned
on a 94 GiB box starved a 32 GiB machine down to a 1 GiB target and every
tile faulted through a decode; everything is now a fraction of RAM.

**The material grid** got the same pager once static analysis found exactly
one allocation site and one free site for the 67,601-byte cells and no copy or
move of the cell triples; a private codec (dense alphabet, order-1 rANS)
stores it at about 20%.

**What did not work**, each with the measurement that killed it:

- A **2 m height cache** (129 by 129) halved the cache and broke the game:
  alignment regions are metre-indexed and were read as 2 m coordinates
  (features repeated across cells, cliffs at cell edges), the construction
  worker combines metre rectangles with the cache's levels, and the renderer's
  upload contract is a fixed 259 by 259 samples. Lower memory and a successful
  load proved nothing about correctness.
- **Copy-on-write sharing** of resident tiles between the two versions: the
  load never reached the copy hook, and in play every share was privatised.
- A **page-granular small pager** for the alignment pass's work blocks: it
  restored 4.9 million blocks correctly and could not keep pace with 50,000
  allocations a second from 24 engine threads on one lock; the peak stayed.
- Routing the **generation float maps** through the pager: every layer op
  sweeps its whole map, so it would thrash.
- Growing the pager's budget during a load to speed it up: the load took the
  same 73 s and the peak rose to 36.5 GB; reverted.

## Towns and industries on a big map

Counts are a **fixed density per km²**, so they scale with area: a 57 km map
is 5.4 times Megalomaniac and generates about 200 towns and 1,600 industries.
Two multipliers sit in front of the visible density and neither defaults to
1.0: towns are 0.2 per km² times a dropdown factor of {0.2, 0.3, 0.4, 0.5}
applied engine-side (not in the shipped Lua), industries 0.8 per km² times
{0.4, 0.6, 0.8, 1.0}. Missing those overstates every count 1.7 to 3.3 times;
a 57 km map at a hand-patched 0.0367 per km² gave 36 towns, which is exactly
0.0367 times 0.3 times 3,288 km². A fixed multiplier does not hold a count as
the map grows (the scale that keeps Megalomaniac's 36 towns is `604 / area`),
so the plugin's added density levels are named for the map size at which they
reproduce those counts.

Placement never touches Lua. Both kinds go through `RandomLocationFactory`:
N uniform samples, then a four-worker perturbation search on a score that is
the sum of a spacing term (`minDist / nearestDist`, 99,999 when closer), a
slope term and 1,000 times a water or obstacle term; candidates are kept while
every part is below 1. The inner search budget is a constant 200 attempts
(the plugin offers 50 as an experiment). **Runtime industry founding reuses
the same path**, so anything a mod does to placement runs in lockstep on
every peer and must be a pure function of the seed and the heightmap. The
spawner's target is `round(area_km² * targetMaxNumberPerArea)`, it counts
every construction with sim buildings regardless of road connection, and both
of its timers are gated strictly on `N < T`; "Industry density target:
Disabled" removes both timers, and closures still happen.

## Rules that transfer

- **Label every number** measured, derived or guessed, and never let a
  derived size stand in for a measured one: two of the audit's largest
  derived terms were wrong in opposite directions (the material grid ratio,
  the render vertices) until a live process read settled them.
- **Never patch a shared constant**; rewrite the instruction that loads it.
- **Never widen an overflowing multiply** whose result feeds int32 indexing
  elsewhere; shrink the input instead.
- **Anything a peer must agree on stays bit-identical**, proven against the
  original machine code, not "close enough".
- **Profile shares are per thread and include page faults.** Divide a window's
  samples by a full thread's count before calling anything serial, and
  measure warm and cold before promising a speedup.
- **Every optimisation ships off** until an in-game load-time or memory
  measurement on the same map says otherwise; several here are still waiting
  for that measurement.
- **Same-machine tuning does not transfer.** A budget or a reserve that is
  right on the developer's box is wrong on a smaller one; derive it from the
  machine, and rig-test the policy by simulating a smaller machine.

## Measure these first on TPF3

1. The tile size, the base sample spacing and whether a derived
   high-resolution cache is rebuilt on load or persisted; if rebuilt, the
   dirty-set shape of the alignment pass on a full load.
2. The octree root: constant or derived from the map, and what an entity
   beyond it does.
3. Every 32-bit multiply that sizes an area raster, starting with whatever
   "Creating streets" becomes.
4. Whether the per-tile render grids (material indices, vertices) are
   retained after entry, by reading the process, not the code.
5. Autosave duration and size on the largest map the room will allow, against
   the stall and load timeouts.
6. Whether town and industry placement and runtime founding are on one path,
   and whether it reads anything but the seed and the terrain.

## The TF3 prototype

`crates/tpf3mp-bigmap` and `mod/tpf3mp_bigmap_1` carry Big Maps'
features over as far as they can go before anyone has read TF3's
executable. Whether the project ships big maps, and which of them, is
the owner's to decide (PLAN.md, "Big maps"); this is what it would be
built on. Every number in it is TPF2 build 35924's, labelled measured or
derived (`WorldModel::TPF2_BUILD_35924`), until TF3's are measured.

| Big Maps on TPF2 | the prototype |
|---|---|
| The added size rows, 32 to 128 km, labelled in the page's own km, with the ratio dropdown shaping each (`add_size_rows`) | `ladder`: the rows and their 1:k shapes. The mod's `ladder.lua` is generated from the settings, and `menu.lua` answers a pick the way the TPF2 detour answered `GetNumTilesNew`: stock rows to the game, added rows by position. On TF3 the New Game menu may be script, in which case the mod adds the rows itself and the ratio labels, which TPF2 could not change, can say what they do. The mod registers nothing until that recipe is known. |
| The street raster's 32-bit wall past 180 tiles (`street_raster`, `cell_budget_millions`) | `ceilings`: the cell count at the stock cell, and the cell the budget needs. |
| The octree root's 32,768 m wall past 256 tiles (`octree`, `octree_depth` 11 to 13) | `ceilings`: the map's half-extent against the root at the depth in use. |
| The heightmap's 32-bit pixel count (derived, about 722 tiles) | `ceilings`: refused, since no setting passes it. |
| The memory law, 2.5 MB per km² plus the game | `ceilings`: the expected peak for every size, before it is generated. |
| Density levels, placement attempts | `config` and `features`: settings that change the simulation. |
| Byte-verified sites, each feature off with a log line when its sites are missing | `features`: each feature names the profile targets it needs (roles such as `bigmap::octree_root`, TF3's functions to be found). In a room, a missing feature that changes the simulation refuses the room instead of degrading. |
| "Every peer needs the same `octree_depth`" | `terms`: the size, octree depth, street cell, placement budget and density levels, with a fingerprint the room compares, and the names of what differs. |
| The minimap | [MINIMAP.md](MINIMAP.md): a script mod on TF3. |
| Terrain cache compression, dedup, lazy zeroing, the SSE2 terrain paths, generator buffers, faster saves | Not in the prototype: each rests on a TPF2 structure that has to be found in TF3 first ("Measure these first on TPF3" above). |

Big Maps' own branch (`feat/bigmaps-40420`) carries its patches in the
hook, switched on by `TPF3MP_BIGMAP_*` variables. As a native mod with a
plugin (a proposed D29 amendment), `crates/tpf3mp-bigmap-plugin` is the
same code as a package for build 40420, switched on by its package's
settings, which a room's terms carry ([NATIVE_MODS.md](NATIVE_MODS.md),
"Plugins").

`cargo run -p tpf3mp-bigmap -- ladder` prints the ladder under a settings
file (`--config`, the example is `crates/tpf3mp-bigmap/tpf3mp_bigmap.example.toml`,
Big Maps' own settings), and `check 320x320` what one size costs and
needs. `lua` regenerates the mod's `ladder.lua`; a test fails if it was
edited or the settings changed without it.
