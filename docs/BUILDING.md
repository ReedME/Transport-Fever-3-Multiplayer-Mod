# Building: roads, track and constructions as portable intents

Current fork integration: protocol **24**, bridge **30**, action schema
**28**, room-log format **11**. Dated captures and game observations below
remain upstream evidence. Earlier data/snapshot directories must be kept
with their original binaries; use fresh directories for this incompatible
refresh. The fork has not adopted U-D30's standalone double-slip exception.

How a build travels between games that share no entity ids. Everything here is
what [TpF2 Multiplayer](https://github.com/silver2127/tpf2-multiplayer)
(0.6.1.12, 2026-09-20) ships and runs in players' games on Transport Fever 2
build 35924; its own account is `docs/REPLICATION.md` and the reverse
engineering behind it is `docs/re/PROPOSALS.md` in that repository. Offsets and
RVAs will not survive into TPF3. The shapes the tools produce, the resolution
rules and the failure modes are the part worth carrying, and each one below was
paid for with a desync or a crash. [HOOKS.md](HOOKS.md) covers the two hooks
(factory and `CommandList::Add`) that make the capture possible; this page is
what to do with a captured proposal.

Where this page says "measured", the fact was taken from a live proposal dump
or a differential capture on two instances; "decompiled" means read from the
binary and not yet exercised in play.

## One command for every build tool

Every build tool in TPF2, the road and track tools, the construction placer,
the module editor, the stop and signal tools, the bulldozer, terraform, paint
and the asset brush, produces one `BuildProposal` command carrying one
`construction_builder_util::Proposal`. That object is a `StreetProposal` (added
and removed nodes and segments, edge objects to add and remove, frozen node
indices, segment tags) followed by the construction fields (`toRemove`, `toAdd`
of `ConstructionEntity`, an `old2new` map) and three terrain grids (height
modifications, material indices, material mask). Rotation is baked into node
positions; only a `ConstructionEntity` carries a matrix.

The shape tells the tool apart (measured unless noted):

| tool | shape of the proposal |
|---|---|
| road, track build | added nodes and segments; new pieces carry placeholder ids (`-1, -2, ...`), existing endpoints positive ids; a mid-span junction also removes the edge it splits |
| street or track upgrade (type, catenary, bus lane, tram track) | no added nodes; N added segments replace N removed segments, every endpoint an existing node |
| construction placement | `toAdd[0]` plus the template's own street pieces (a one-track modular rail station: 25 track nodes, 24 edges); `toRemove` empty |
| module edit | `toRemove[0]` the old construction, `toAdd[0]` the new `ConstructionEntity` (same file, new params); a modular station re-adds its internal track |
| stop, signal, waypoint placement | the edge removed and re-added, plus one edge-object record |
| stop or signal bulldoze | the edge removed and re-added without the object; a two-sided stop's two objects together (build 40420) |
| construction bulldoze | `toRemove` populated, nothing added |
| road or track bulldoze | removed nodes and segments, nothing added; on TPF3 a town street also lists the town buildings along it in `toRemove` (seen on build 40408), and a track bulldoze lists its signals in `edgeObjectsToRemove` (build 40420); a street's branch or a track stub ending on a street also removes the junction node and its two street edges and adds one edge joining their far ends, carried as a `BuildRoad` only when every removal pairs with a join or is the known stub (seen on build 40420) |
| tree or asset bulldoze (TPF3) | the asset group in `toRemove`, and `toAdd` one construction of no file, its desc `autoRemovable`: the group rebuilt without the assets removed, thin instances then full ones (`CreateProposalAddAsset`, decompiled; the shape seen on build 40408); nothing added when the last assets of a group go |
| terraform | no nodes or segments; a `Grid<{height, base}>` of 4 m cells |
| paint | no nodes or segments; the material index grid and its mask |
| asset brush | `toAdd` records of an asset-group type whose per-asset data is a vector of `{model path, matrix}` (decompiled); its commit clears `old2new` first |

Two consequences for a TPF3 profile: the factory hook needs one decoder per
shape, not per tool, and the shape is the only reliable way to classify the
command (the tool object is not on the command).

## Three ways an action travels

| mode | what happens |
|---|---|
| **strict** | The hook cancels the command inside the engine before it applies. The mod ships it with a stamp and every instance, the player's own included, applies it at that stamp. Nobody's world runs ahead. |
| **replay on peers** | The command applies natively at the click; the other instances apply it at the stamp a few steps later. The player's game is briefly ahead for that one action. |
| **poll** | No hook. The mod notices the change on the originating game by scanning and ships it; the others replay it. |

Every channel on this page is strict except one (a stop that replaces
another, under "Stops, signals and waypoints"). It was not always so: constructions started as replay-on-peers and that produced a
measurable height desync (the originator's native placement and the peers'
scripted one graded the terrain through two different code paths, so the
station's own track nodes landed on different z, invisible to a construction
digest that hashed only x and y). Strict means every instance runs
`construction_builder_util::Apply` with identical inputs, which is the only
way to get identical output from it.

Three rules hold for every channel:

- **Only cancel when something will replay.** The hook cancels nothing unless
  the script half has reported a live peer within the last 15 s. Alone or with
  the mod off, every command runs natively and the game is the stock game.
- **Never cancel on a failed decode.** A command the hook cannot read
  completely runs natively; the poll or replay path ships what it can. Losing
  the player's action is worse than a visible divergence, and a cancelled
  build that is then rebuilt from a misread proposal is the worst case of all:
  the wrong station on every peer.
- **Nothing on the wire names an entity id.** Entity ids differ between games,
  and on one game they are recycled. Roads travel as positions; constructions
  as file plus position; players as a logical company number that each
  receiver resolves to its own player entity.

## Roads and track

### The wire

A road or track command (`ROADP`) is a polyline of vertices as positions
`(x, y, z)` and links between vertex indices. Each link carries the street or
track type, catenary, bus lane and tram-track flags, the `BaseEdge` type and
type index (1 bridge, 2 tunnel) and its owner as a logical company. With the
polyline travel:

- **removals**, as endpoint positions plus the network kind, never ids;
- **fresh vertices**: the indices of the new nodes that the originator's
  engine attached to nothing (a vertex beside a road looks, geometrically,
  exactly like one on it; only the capture knows the difference);
- **the plan**: for each vertex, what the originator resolved it to (an
  existing node at a position, a split of the edge between two positions at a
  height, or a plain new node), so the peers repeat the decision instead of
  re-deriving it;
- **companion spans**: an unchanged bridge or tunnel span that the engine
  re-adds between two existing nodes as part of the command, rebuilt with its
  own properties rather than the new road's.

### Resolving a vertex on the receiver

Each instance resolves every vertex against its own world, in this order
(measured; the older `mp_bridge` mod had found the same rule and called it
"the road intersection bug"):

1. a node of the same network within 1.5 m horizontally and 0.5 m in
   height, the nearest: reuse it. The engine settles nodes at heights other
   than the requested ones on embankments and smoothed terrain, so a vertex
   naming an existing node carries that node's own position as it stands,
   its settled height included, never the height the tool asked for. A node
   0.5 m or more above or below it (a track or road stacked over another)
   is not it, and two nodes exactly as near refuse the build;
2. else an edge underneath (track snaps within 2.0 m, roads within 5.0 m):
   **split** it, removing the edge and re-adding both halves around a new
   node with Hermite tangents scaled by the parameter interval so the curve
   keeps its shape; a split within 2.5 m of an end snaps to that end;
3. else plant a new node.

`BuildProposal` refuses an edge whose endpoint sits partway along an existing
edge; the interactive tool splits for you and a raw proposal does not. When a
player snaps onto an existing road the originator's proposal contains no
removal at all (it attached to geometry it already had), so a receiver cannot
wait for a removal list to tell it to split: it has to find the edge itself.
Two rounds were lost hunting for an `edgesToRemove` that was never there, and
one wrong offset guess on that hunt would have fed arbitrary entity ids into a
removal list.

Rules that came out of the replay:

- **Gate the capture on edges, not on new nodes.** A road joining two existing
  junctions adds zero nodes and one edge. The first capture treated "fewer
  than two new nodes" as a failed decode, correctly did not cancel, and the
  road built locally and never shipped. One unreplicated road changes
  connectivity, town growth reacts to it on one game only, and the world
  digest gap widens forever: a road is never a static +1.
- **Split halves inherit the split edge's record**, not the new road's: the
  street type of the road being crossed, its flags and its type index. A
  depot apron stamped onto the halves of a town road makes the engine refuse
  "Construction not possible" on every town road.
- **An added edge between two existing nodes is an in-place replacement**, and
  its removal must travel. A road passing under a bridge makes the engine
  replace the bridge span in place; a replay that took it for a split added a
  second span between the same nodes. A removal is matched in the command's
  own network only, and a double removal rejects the whole proposal.
- **A replaced edge must carry its objects.** The engine's own upgrade keeps
  the old edge's stops and signals under their ids. A replay that removed an
  upgraded street and re-added it without them crashed three games at the
  same step, in `station_util::GetTerminalPersonEdges` from the catchment
  update, because every stop on that street pointed at a deleted edge. An
  edge with objects and no one-for-one replacement (a split or a reroute)
  makes the whole command skip on every instance, with a log line; on TF3
  the one exception is a track cut between its signals, whose signals the
  links keep by name (`kept`, "The action schema").
- **Prefer what the originator's proposal did over re-deriving it.** Whenever
  a receiver reconstructs a decision from geometry (is this vertex on that
  road? is this node a crossing?) it eventually decides differently from the
  engine. Ship the decision.

### Level crossings

A crossing is its own entity, created from an explicit list during
`CreateProposalData` and never inferred afterwards. Native shape: the road is
cut into half, connector, half, and the track is cut at both ends of the
connector; every crossing node has exactly two street and two track edges. The
engine records a crossing at a node where the other network forms a straight,
homogeneous two-edge run. A script proposal goes through the same pipeline, so
the replay's job is to reproduce the node layout the UI produced:

- a track vertex within 4.0 m of a road node shares that node and takes the
  road's height when the two differ by more than 0.25 m (moving the road node
  instead asserts the engine); otherwise the road under the vertex is split;
- routing through an existing node requires the track to touch it (0.75 m)
  and the node to be straight-through. `Crossing.cpp:232` asserts that a
  four-arm crossing is two straight lines (arm 0 anti-parallel to arm 2, arm 1
  to arm 3). Route a track through a road corner and the game freezes on a
  native assert on every instance replaying the plan; `pcall` and
  `ignoreErrors` see nothing;
- a candidate within 12 degrees of the track's own direction at its closest
  approach is parallel, not a crossing. The spacing of a double track is
  exactly the 5 m band the crossing pass searches, so upgrading the second
  track of a pair split the first, and a real crossing under about 12 degrees
  cannot be built anyway;
- an edge found under a vertex more than 2.5 m above or below it is over or
  under, not a split; plan view alone lies under bridges;
- a crossing the engine refuses ("Too much slope": the track on an embankment,
  the road below) is refused on every instance, which is at least
  consistent. The native tool would have re-graded it; the replay only sees
  numbers.

A crossing's type is the toolbar's (`rcType`): the tool maps its crossing
nodes to it in the proposal's `node2rcType`, which the capture cannot read,
so the hook reads it at the click and `BuildRoad`/`BuildTrack` carry it by
file (`crossing_type`, schema 28); every game maps its vertices' nodes the
same way, and a crossing already there keeps its own type (docs/HOOKS.md,
"Level crossing types"; gated by `acceptance.lua`'s `crossingTypes`).
Without it each game lays the era's type.

### Demolish

Road and track demolish (`EDEMO`) matches edges by their end nodes: same
network kind, within 1 m. The kind is load-bearing (a road node and a track
node can coincide at a crossing). An edge that carries stops or signals is
refused; orphaned nodes are removed. It is strict with no originator skip: the
bulldozer is a tool that waits on its completion callback, so the cancel fires
it. This channel did not exist for the first months, and the bug report it
produced read as a lag bug ("I demolished a road during lag and the crossing
did not form"): the road was simply gone on one game and present on the
others, at any latency.

### Local repairs are commands too

Anything the script half changes on its own, a heal of an orphaned split after
a station is removed, a cleanup, a retry, has to be timed from an agreed
command stamp in simulation steps. A heal that ran from a frame counter
merged the same road on three games at three different steps; the final
geometry was identical, the digests matched, and passengers were rerouted at
different moments, so the buses drifted. When every replicated command applies
on-step and the geometry agrees, grep for unstamped local proposals next.

## Constructions

Stations, depots, assets, harbours and airports.

### The wire

The hook reads the placement off the proposal at the factory: the file name,
the transform, the parameters (the `ConstructionEntity`'s Lua table, walked
into a serialisable form, `seed` kept, strings escaped so an embedded newline
cannot split a line-based stream), the name, and the player as a logical
company. The template's street pieces (a depot apron, a station forecourt, the
road a station was dropped onto and split) travel as a separate road record
(`ROADC`) paired to the construction by a placement serial stamped on both
records, never by distance or arrival order. Every instance then builds the
same scripted proposal at the stamp; the originator's native build is
cancelled and its completion callback fired.

### What a script proposal must carry

All measured:

- **`params.seed`.** Without it the factory returns a bare `false`. Two weeks
  of workarounds followed from one measurement made with the seed stripped.
- **A name.** `Apply` gives the construction and its child entities (the
  vehicle depot, the station entities) `NAME` and `PLAYER_OWNED` only when
  the name is non-empty. An unnamed child crashes the game when a player
  clicks it: the GUI select handler dereferences null. Neither a later
  `SetName` nor the script's `setPlayer` repairs a child afterwards.
- **A Context that matches the intent.** The UI places with terrain alignment
  and graph cleanup, and with `gatherBuildings` set the engine demolishes the
  footprint's town buildings itself, identically everywhere. A nil context is
  not equivalent, and `ignoreErrors` on a raw proposal does not demolish
  colliding buildings: it builds through them.
- **Not the script's `buildConstruction`.** It runs the template at raw
  coordinates, construction-owned and never joined to the road; a later street
  proposal cannot remove those pieces ("Construction not possible"). Replay
  the whole placement proposal instead.
- **The UI's resolved geometry.** The engine never snaps a template's snap
  nodes to existing world nodes; only the shape the UI resolved is buildable.
  A road depot dropped on a junction is one node and one segment welded onto
  the junction, and the replay's merge has to present exactly that (adopt the
  template's apron record onto the shipped segment, re-point the frozen node
  index, drop the template's own apron). Any new construction kind is first
  diffed field by field against a dump of the UI's proposal for the same
  placement; both of the above were invisible in the geometry.

### Index linkage

The construction-to-street linkage inside a proposal is index-based: the
construction's frozen nodes are indices into the added nodes, `segmentsBefore`
is the segment count before the template's edges were appended, and nothing on
a node or segment names its construction except the appended connector. So a
hook that patches a script-built proposal edits records in place and only ever
drops the **last** record of a vector; compacting shifted every later index and
the apply asserted. Placeholder ids are numbered like the UI's (`-1, -2, ...`):
large placeholders push the template's regenerated ids below them, which the
same bookkeeping cannot handle.

### Footprint and failure

- A cancelled placement builds with `gatherBuildings=true`, so the engine
  demolishes the same town buildings on every instance. For the non-cancelled
  path (parameters unreadable, so the native build stood) the originator
  ships the town buildings it still has nearby with the radius it gathered
  them in, and the peer removes the others inside that radius; a list whose
  survivors mostly do not exist on the peer is refused loudly. A bounding-box
  sweep over-clears (a modular station's box is about 170 by 120 m).
- On failure the replay retries once after clearing the footprint, then asks
  the originator to roll back: it bulldozes its own copy (same file within
  1 m) so the worlds stay equal. The visible symptom of a refused station is
  three links downstream: "a station got demolished but not by player
  action" and "vehicles were bought but never assigned", because the line
  update that named the station then found no station group. Read the
  rollback record first, then the peer's refusal, never the demolish.
- The parameter walk has no depth or entry cap, and a field it cannot read
  fails the whole decode (the build then runs natively behind a notice). An
  earlier walker dropped seventeen boolean entries of a rail station's
  parameters, cancelled anyway, and the replay asserted in the engine's snap
  node lookup on all three instances at once.

### TPF3: a depot placed onto existing track

The 2026-10-01 relay playtest exposed a replay error for a rail depot
placed with its entrance snapped to an existing track endpoint. The
captured proposal correctly named that endpoint and six internal track
segments arranged as a branching tree. Replay removed only the two
outermost segments, then built the remaining four as standalone track
alongside the depot's own generated track. Both games logged `Collision`;
the subsequent `refreshConstruction` snapping command failed with
`Construction Not Possible`. The depot remained built but disconnected.
A later train purchase succeeded, while assignment to the line failed in
both games: this was a placement failure, not a lost assignment message.

Construction replay now removes complete branches ending at new vertices
before building the external network. Existing-node and split references
are anchors: they are retained, along with the paths between them, so a
road rebuilt through a station junction still travels. The construction
generates its own internal track, then its refresh snaps the entrance.
Ordinary road and track builds do not use this branch removal.

The junction configurations the tool proposed go with the branches they
name (2026-10-02: a street terminal placed into a road was refused in
every game, "the junction no longer exists"). The tool configures the
station's own entrance node and the new junction its entrance joins, and
both name the entrance edge, which replay leaves out; so every game drops
a configuration whose node is a removed branch's vertex, or whose turns or
crosswalks name a removed branch's edge (`junctions.without`), and logs it
as "left to the construction". The construction and its refresh give those
junctions the game's own settings, the same in every game. Configurations
at existing nodes that name only the rebuilt street still travel as the
tool made them.

The originator already leaves out the construction's own tracks and
streets that reach nothing existing (`joinedOnly` in `capture.lua`); the
configurations on them go too (2026-10-07: a rail station snapped to a
track end was refused in every game, "the junction no longer exists",
because the tool had configured the switches of its other platform
tracks, whose nodes no game's build has). A configuration whose node is
one of those left-out nodes, or whose turns or crosswalks name one of
those edges, does not travel. The station builds those tracks and their
switches itself. It also carries fewer configurations: counted with its
own switches, a large station's preview had 99, over the 64 an action
holds.

Capture also applies replay's construction-owned junction rule before
encoding a joined station. A 144-setting station encountered on 2026-10-10
would otherwise exceed the action's 64-setting limit even though the station
itself creates most of those switches. This only removes settings for the
construction's own network; settings for external junctions still travel.
Where the game's one construction proposal refuses a snap onto existing
track, the staged rejoin path below is disabled in ordinary rooms. It
removes track before laying it again in a second command, which is not
atomic with the first.

`lua_mod.rs` reproduces the recorded depot topology: before the fix its
first build contains four duplicate nodes and edges; afterwards it contains
none, and the stand-in engine accepts the refresh. A longer station
entrance test preserves the external road junction, and a refused refresh
still reports failure. These are Lua regression tests, not a successful
real-game replay. The PC crashed after the original test session; the
remaining acceptance check is a fresh two-game placement onto existing
track, followed by buying and assigning a train and verifying its route
in both games. The change does not repair already broken placements.

On 2026-10-10 that placement was played in two games of the local rig
(Steam build 40420). A stock modular rail station and an underpass mod
(`grimes_unterfuehrung`, its own tracks in four, six and eight rows)
were snapped onto the ends of existing tracks, some of them loose pieces:
each was built and refreshed alike in both games. The train run is still
to be checked.

One placement was not: the underpass's eight tracks onto eight track ends
beside a station. Built alone, the construction was refused in every game
("Construction Not Possible", with no colliding entity named), though the
tool had placed it. A dry `makeProposalData` in the game showed why: with
four of the eight track ends standing where its own tracks end, the
construction alone cannot be built; without them it can. Only the tool
welds a construction's track ends onto existing nodes.

The staged `rejoinConstruction` implementation in `apply.lua` can join
this shape when the action's connection is only the construction's own
track at existing track nodes (no split, no removal, no street node):

1. it takes away the track pieces at those nodes (found at their own
   level: a node of a track right above or below is another place), the
   nodes, a far end the pieces leave with no edge, and the junction
   settings at the far ends that stay, and builds the construction, in
   one proposal;
2. it lays the pieces again at once, through the track build's own path:
   each between the same places with its tangents, template, lanes (an
   electrified piece stays electrified), decorations, lock and owner, now
   ending on the construction's own new track node where its old end was,
   named by its entity (the one track node within a few centimetres,
   height included, that did not exist before), not looked up again by
   position, which would take a node of a track right above it; a far
   end that went comes back as a node of its own. They are laid for free,
   as the game's refresh of a construction is: they were the player's
   already, and the tool only joined onto them;
3. the construction is refreshed as before.

A piece with a stop or signal on it, a bridge or tunnel, another
company's piece (D21), or a junction at a far end set by hand (its turns,
its lights, their phases or a double slip), and the build stays refused
with the game's own reason; the far ends' settings are otherwise the
game's own, and go with the pieces. Where the construction has no track
end at such a place, the staged path lays the piece back unjoined and
hook.log says so.

`acceptance.lua` keeps `rejoinConstruction = false` in the release mod.
If the construction alone is refused, replay stops before sending the
proposal that removes track. The flag is mod code covered by the room's
fingerprint, not a player option; tests alone turn it on to exercise the
staged path. That path still has a destructive failure mode: after step 1
builds, step 2 may be refused, leaving the original pieces gone while the
handler logs the failure and returns success. Before enabling it, establish
one atomic native proposal or a guaranteed recovery path for a failed
re-lay, prove the failure case without losing track, then perform an
ordinary-start two-game placement and train-route check. No such guarantee
is established by the staged runs below.

The values of the pieces are read before
step 1: the first real-game try read them from the removed edges'
components afterwards and step 2 failed in both games ("no Track node at
vertex 1"), which the stand-in engine, whose components are copies, did
not show. A later staged eight-track placement built, joined and refreshed
alike in both games, but does not prove every re-lay can succeed.

### Module edits and upgrades

Stock rail-station edits without external street changes now use
`createProposalReplaceConstruction` on each replica, as the game's own
construction UI does (Steam 40408, `gui/construction/construction.tl:1295`).
This preserves the full native replacement graph instead of reconstructing
it from a `SimpleProposal`. The regenerated proposal must replace exactly
the intended construction and recapture to the same file, name, parameters
and transform, without introducing external street edits. Ownership and
build errors still refuse the action. Build 40408's runtime rejects a full
`Proposal` in `makeProposalData`, despite its API declaration; native edits
therefore use the stock command path with `ignoreErrors=false` and verify
the resulting construction parameters. Native replacements already include
their track snapping and do not receive a second refresh.

This addresses the replay path implicated by five Basingstoke Station
module edits rejected with `Construction Not Possible` on 2026-10-05.
Stand-in engine regression coverage exercises bulk, liquid, flatbed and
goods module parameters with different local entity IDs, and refuses
changed parameters, moved constructions, extra additions/removals, external
street edits and critical errors. Two local games on a copy of the affected
save accepted a bulk-platform replacement and an additional track at
Basingstoke Station; both replayed the edits and subsequent rolling checks
agreed. The original save and the player's session were not modified.

The old construction and the new parameters come off the proposal; every
instance upgrades the construction with the same file within 10 m at the
stamp. Two traps:

- **Entity ids are recycled.** A strict replay built a station under an id
  that a bulldozed town building had just freed; the "already seen" set still
  held that id, the station was never adopted, and sixteen module edits were
  cancelled locally and applied nowhere. Any id-keyed set of seen things can
  hide a new entity; look things up by position when an edit "cannot find"
  what the player can see.
- **Town buildings are constructions.** The construction query returns
  `building/era_b/res_1_2x2_01.con` and its kin, and towns spawn one every ten
  seconds or so. The first poll captured twenty of them per game in three
  minutes and scheduled each other's town growth for replay. Filter by
  ownership (the `PLAYER_OWNED` component), never by a name blacklist.

On TPF3 an edit travels as a `BuildConstruction` with `replaces`, the old
construction by file and place, and every game replaces it within 2 m in
one proposal mapped old to new, as the game's own upgrade does
([HOOKS.md](HOOKS.md), "The build tools"). The construction tool's
proposals, the construction menu's parameters and the station window's
cargo buttons are carried so, with the streets an edit changes around the
construction as its connection (below); one replacing more than one
construction or one the room cannot name is refused. The module editor itself tells game scripts nothing of its
proposals on build 40408 (read from the binary: `UI::CGameUI` forwards
`builder.proposalCreate` for six other tools only), so the hook reads its
proposal natively at its call of `CommandList::Add` and hands the GUI the
same table a tool's proposal would be ([HOOKS.md](HOOKS.md), "The module
editor"). A real module edit was captured on Steam build 40408 on
2026-10-01: it replaces the old construction and rebuilds its own tracks.
The two-track station removes 50 nodes and 48 edges, but its component
lists only 46 frozen nodes. Its four unfrozen track ends each touch one
of its frozen edges. Capture accepts an unfrozen removed node only when
every incident edge is frozen in that construction and also removed by
the edit. An endpoint shared with external track, an empty incidence
list, or an unreadable list is refused. The regression test covers those
boundaries; capture evidence alone does not prove replay in both games.

The corrected capture was then exercised in two launcher-started games on
the local server, from a save containing that station: a platform extension
from the guest was applied in both games and was visible in the host.
Read-only queries of the resulting station's flattened parameters matched
exactly. A second module edit and an eight-track, 320 m station placement
also applied in both games; all 54 shared network checkpoints through step
2700 agreed. This proves those edits, not every module type or an edit that
also rebuilds external connecting track. One host startup failed before
testing and succeeded on rejoin; that loading failure remains unresolved.

A road station placed by a road snapped onto it, but came loose as soon as
it was edited (2026-10-03, adding a second entrance at its other end, and
in both games, the editing player's too, since every game replays the
edit): its street pieces no longer joined the road or made junctions with
it. The replacement builds the new construction alone, and a scripted
build makes the entrance again unsnapped, ending short of the road, as a
fresh build does; the fresh build is refreshed afterwards, the edit was
not. Every game now refreshes the new construction after an edit too,
which snaps its entrances onto the streets beside them. `lua_mod.rs`
covers the refresh, a refresh with nothing to snap (nothing sent) and a
refused one (the edit stands, logged). Seen in the game the same day: a
plain edit snapped again (`snapping 72194 +e-2:-1>57114 -e71473`).

A new exit onto a road the station did not join was refused: the module
editor's proposal splits that road through a new junction (three nodes and
four edges added; the station's own entrance node and edge and the road's
edge removed), and an edit carried no street change around its
construction. The hook reads only how many nodes and edges the editor
adds. Asked again in the game's console with the editor's parameters,
`createProposalReplaceConstruction` proposed exactly the editor's street
part. So the editing player's game asks it so, checks it against what the
hook read, and the edit carries the streets around it as its connection,
without the old construction's own removals; every game builds the
connection in the replacing proposal, the station's own entrances peeled
off as for a new station, then refreshes the station. The old entrance's
junction, where the split road ends at it, keeps no settings (they would
name the old entrance, which goes with the old station). Every edge a
construction's connection removes or splits must be the acting company's
or no company's, for new stations too, which did not check it. Covered by
`lua_mod.rs` (the capture and its refusals, the replay, a road of another
company, the old entrance's junction and another company's road at it, a
refused refresh after the split). Seen in two launcher-started games on
the local server the same day: a new exit onto another road was carried
from the module editor and replayed alike in both games (`building
+n-3(-1042.6,-1959.4,11.0) +e-1/0:48073>-3 … +e-2/0:-3>71864 … -e71975`,
then `snapping 72116 +e-3:-1>73619 +e-4:-2>71600 -e72101 -e73645`), both
entrances joined to their roads, and edits after it too.

### Demolish

Strict: every instance requires the same file within 2 m of the position. The
fallback for a bulldoze the hook left to run natively is a poll: a tracked
construction missing for two polls ships a demolish, and peers remove the
nearest one within 30 m; a record counts as present only while its entity
still carries a construction component of the recorded file, or a replacement
stands within 1 m. An optimistic demolish (native at the click, peers at the
stamp) splits the refund across sim-times and removes a different set of
passengers and cargo on each game, which the geometry digest never shows.

Bulldozing an entity that is already gone is an access violation, not an
error: a sweep that removes a building removes the asset groups that stood on
it, and the next id in the loop is dead. Check existence immediately before
every bulldoze in a loop.

## Stops, signals and waypoints

The engine's own stop tool removes the edge and re-adds it with the object
list carried verbatim: every untouched object under its positive id (which the
apply treats as re-parent, keeping its station group and lines) and the new one
as a negative index into the add list. A removed object goes into the remove
list and the apply rewrites its lines and station group before it dies. The
script's proposal conversion copies all of that, so placement and deletion are
lossless from Lua. Rules:

When the tool supplies neither an explicit edge parameter nor a model
transform, capture finds the closest point on the edge's 3D Hermite curve
to the viewing ray from `api.gui.camera.getEye()` through the cursor's
terrain hit. Projecting the terrain hit horizontally loses the height of
a bridge and can move the signal tens of metres along it. Explicit native
proposal coordinates still take priority. Replay uses the captured world
position, never another player's camera. A stand-in regression covers the
elevated case and explicit-coordinate precedence; real-game results are
recorded separately in the validation report.

- the `left` byte is the engine's, not the geometric side. Two signals that
  both stood geometrically left of their track carried `left` 0 and 1; a
  waypoint on the centreline has no side at all. Ship the engine's byte with
  the originator's unit tangent, and let the receiver flip it only when the
  edge it matched runs the other way;
- one stop per side per street edge. Two objects with the same side value on
  one edge is a fatal assert in lane creation. Guard by side, not by count;
- the junctions at the edge's ends keep their lane configurations. A script
  proposal must remove the configurations that name the edge it removes
  ("Unknown exception" otherwise, below), and must add them back naming the
  rebuilt edge: removed alone at a junction with traffic lights, the lights
  stay without a configuration, the build asserts
  (`GetComponentDataIndex`, component `BaseNodeConfig`) and leaves the
  world half rebuilt, and the simulation dies a second later (TF3 build
  40408, 2026-10-04: a two-sided stop between two traffic lights crashed a
  room; reproduced from the console in a single game);
- merging a new stop into a nearby group is not in the proposal: the apply
  pairs an opposite-side stop within 125 m, else joins any group within
  200 m. The same placement merges the same way everywhere for free;
- a compatible stop dropped on an occupied side **replaces** the old one and
  the engine re-points its lines, which a script proposal cannot express.
  That one action is not cancelled: it runs natively, is found by polling,
  and the originator re-ships every affected line afterwards;
- the edge is found by its end points within 2 m, else the nearest centreline
  within 14 m; an edge frozen into a construction is refused.

On TF3 the mod carries a stop placed with the stop tool, and a stop the
bulldozer removes, the same way (HOOKS.md, "The build tools"): a
`PlaceStop` or `Bulldoze::EdgeObject` read off the tool's proposal, the
edge named by its ends within 0.5 m, horizontally and in height, the stop's place by the point of the
edge's centreline where it stands, and the stop's construction, which the
GUI notes from the construction menu (TF3's proposal does not name it). A
two-sided stop goes as one `PlaceStop` built on both sides, and the
bulldozer's removal of it, both its stops at once, as one
`Bulldoze::EdgeObjects` (schema 27): every game finds each of the two by
its construction within 2 m of where it stood, checks they are opposite
`STOP_LEFT`/`STOP_RIGHT` objects in the same station group at the same
place along that edge, and rebuilds it once without them. An incomplete,
ambiguous or unrelated pair removes neither; each resolved stop must also
be one the acting company may touch. A stop that replaces another stays
refused. A track bulldozed with signals on it goes as `Bulldoze::Edges`:
every game finds the same tracks, refuses the removal if any of them
carries a stop or another company's signal, and removes the tracks with
exactly the signals on them, listed in the proposal's
`edgeObjectsToRemove` (an edge removed with objects left on it crashed
TPF2's games). Signals and waypoints go the same way, with
the settings the signal tool builds them with (its
`EdgeObjectBuilder.params`, which every game sets on the signal it builds:
a mod's settings, Auto Signals' spacing among them). Signals a mod's script
then places along the track (Auto Signals) go as one `PlaceSignals`
(HOOKS.md, "Scripts' follow-up builds").

## Terrain and the asset brush

- **Terraform.** The whole edit is a grid of 4 m cells, each `{target height,
  height before}`, on the proposal; a raise is a 10 by 9 grid, a smooth 83 by
  59. The hook stashes the grid at the factory and cancels the commit; every
  instance, at the stamp, sends an empty carrier proposal that the hook fills
  natively with the grid. The receiving game's height came out bit-identical
  to the originator's. A stroke is held until the originator's own replay has
  applied, so the next part of the stroke is computed against the replayed
  heights.

  **On Transport Fever 3** (build 40408, read from the binary, not yet seen
  in the game) the same shape holds: the terrain tools (`UI::TerrainModifier`:
  raise, lower, smooth, flatten, the heightmap brush), the painter and the
  asset brush are `UI::ProposalAction`s that queue their `WorldBuildProposal`
  from `ProposalAction::DoApply`, and tell game scripts nothing. The
  proposal's `terrain.baseHeightMod` (at 0x2d8 of the `Proposal`) is a grid
  `{ x0, y0, width, height; Vec2f cells }`, then the paint's material and
  mask grids. The hook reads the height grid at the click (docs/HOOKS.md,
  "Terraforming"); the GUI hands the room `Terraform` actions of it, the
  cells' two values rounded to the millimetre, in bands of whole rows of
  at most 4,096 cells; every game, the player's own included, arms the hook
  with the grid and sends an empty `Proposal` as the player's build, which
  the hook fills at its apply. So every game sets the same cells to the
  same heights in the same update. What a stroke changes is carried, not
  how the brush moved, so frame timing does not enter. TF3's tool is not
  held between parts of a stroke as TPF2-MP's was: while the mouse is
  down, the originator's tool computes against ground the room has not
  changed yet. The lanes (`tpf3mp/lanes.lua`) do not read the terrain, so a
  divergence in it alone is not caught at a checkpoint; INFERRED, TPF2's
  lesson, that one shows soon after in the edges and constructions built
  on it, which they do read.
  It stays refused in a room until `tpf3mp/acceptance.lua`'s `terraform`
  is turned on after a two-player game shows the same ground in every game
  (COVERAGE.md); until then the GUI's sender and every game's replay
  refuse it.
- **Paint** is the material index and mask grids on the same path. The
  material texels are simulation data, not a graphics setting: a paint applied
  in the right place with the two games at different texture resolutions.
- **The asset brush** commits asset-group records. On TF3, Lua can build
  an asset group (`Proposal.ConstructionEntity` with an autoRemovable desc
  and one subconstruction of the models, as the bulldozer's replay does),
  so the hook only reads the stroke and every game builds its groups in
  Lua (`PlantAssets`, schema 27); removed groups travel by their first
  asset and count. The cost of not replicating it was
  measured: 104 trees on one game, every replicated command on-step, and six
  hundred steps later the town-building and road lanes diverged in three
  towns 9 km apart, because town growth reads the assets. A desync minutes
  after the last command means an unreplicated native edit, not a
  replication bug.

## What does not fit a proposal

- Companies and ownership travel as a logical company number and a typed
  `PlayerOwned` component on the receiver; the engine silently discards a
  plain table assigned to that optional field.
- A construction placed by a **bare** replay (nodes 0, edges 0) has its
  platform track rebuilt from the `.con` template, so nothing pins its
  geometry; the placement serial and the paired street record are what keep
  it attached to the road on every game.
- Bridge and tunnel type ride on the segment record, not on a node, and a
  split half keeps them.

## The action schema

What an intent's payload carries: `tpf3mp_proto::action`, version
`ACTION_SCHEMA_VERSION` (**28**; combines station access, company perks
and preservation, plus named stops and gated asset removal. Schema 26 adds a
signal's settings to `PlaceStop` and `PlaceSignals`; the unreleased schema
27 combines `Paint`, `PlantAssets`, `Bulldoze::EdgeObjects`, `SetCrossingType`,
and link fields after `precedence`: edge-end positions, then signals kept
from a cut track (`position0`, `position1`, and `kept`; see "Edge ends"
below); 28 appends a road or track build's toolbar crossing type,
`crossing_type`. This is one combined layout and has not been released. This
integration combines the existing
junction schema with the selected vehicle, depot, demolition, precedence
and gated action additions described in [COVERAGE.md](COVERAGE.md).
The Lua mod builds an action from a captured
command, the payload travels opaque through the server, and every replica
resolves it against its own world by the rules above. Everything a TPF2
command carried as text travels here as typed, bounded fields.

`CalendarSpeed` carries `millis_per_day`, the exact integer from the game's
calendar speed control. Zero pauses the date; positive values set the day
length independently of simulation speed. The GUI guard queues the command
instead of running it locally, and each replica applies it through
`makeGameSetCalendarSpeedCmd` at the ordered action's step. Values outside
0–2,147,483,647 are refused before reaching the engine. The variant is
appended, preserving the existing schema-25 variants' bytes; old clients
cannot decode the new variant and must use the same mod build as the room.
Capture, wire round-trip and two Lua replica replays are covered by tests.
On 2026-10-06 two local build-40408 games also verified date pause from the
host and resume from the guest through the ordinary calendar UI, while
simulation updates continued during calendar pause.

**References.** An action never names an engine entity id. It uses:

- **positions**, in millimetres as `i32` on the game's own axes (±2,147 km,
  far past the 65.5 km of the largest map), rounded from metres to the
  nearest millimetre at capture. Tangents are in millimetres too, since their
  length shapes the curve; unit directions and a construction's rotation
  are in millionths. Integers keep the payload the same bytes on every
  platform, and a millimetre is far below every matching tolerance above;
- **resource file names** (`ResName`, up to 128 bytes): street, track,
  bridge and tunnel types, construction files, vehicle and stop models;
- **canonical ids** (`CompanyId`, `LineId`, `VehicleId`, `StationId`),
  which the server assigns to what an action creates and each replica maps
  to its own entity; and `TownId`, for the towns that come with the room's
  world, which every replica binds in entity order at the room's first
  update (docs/HOOKS.md, "The player's commands").

The acting company is not in the action: the server knows whose command it
is.

**The actions.** Variants are identified by position; new ones are
appended.

| action | carries |
|---|---|
| `BuildRoad` | street type (TF3: its road template), road style (TF3), bus lane, tram track (none, plain, electric), a polyline whose links may each name their own kind, decorations, the towns' lock and the company's ownership (the road modifiers), and the toolbar's level crossing type for the crossings it lays (schema 28) |
| `BuildTrack` | track type (TF3: its road template), road style (TF3), catenary, a polyline, and the toolbar's level crossing type (schema 28) |
| `Bulldoze` | edges of one network by their ends, with the town buildings the game removes along them, each by file and position; or a construction (a town building among them) by file and position; or a stop, signal or waypoint by its edge, position and model; or both stops of a two-sided stop, each by position and model, on one edge (schema 27) |
| `BuildConstruction` | file, transform, every parameter (`seed` included), name, the construction it replaces for a module edit, and its connection: the streets and tracks its tool built with it, as a polyline whose every link names its kind |
| `BuyVehicle` | the depot by its construction's file and position and its index among the construction's depots (its `depots`, then its subconstructions that are depots: an airfield's or airport's hangar; an airport's second hangar), the consist front to back (each part's model, facing, each compartment's load, colour), its groups and multiple units |
| `SellVehicle` | vehicles |
| `CreateLine` | name, colour, the line as the game keeps it: stops (station group, terminal, other terminals, load mode, waiting times, loading rules per cargo, the waypoints after it), transport modes, settings. A waypoint is on a lane of a street's, track's or construction's transport network (the edge by its ends, node 0 first, which must run the same way in every game; the construction by file and place), the lane's index and the place along it; or, for ships and aircraft, a position in the open; with the line manager's tag |
| `EditLine` | a line and one change: rename, recolour, the whole line anew, or delete |
| `AssignLine` | vehicles, the line or none, the first stop or none for the game's choice ("Next Reachable Stop") |
| `PlaceStop` | a stop, waypoint or signal (`object`): the edge (network and ends), the position along it, the engine's `left` flag, the originator's unit direction there, its construction, whether a stop is two-sided and whether a signal is one-way, and a signal's or waypoint's settings (`params`, the tool's, schema 26) |
| `Terraform` | the grid: corner, cell size, columns, and each cell's target and previous height; on TF3 the corner is the first cell's index in the terrain's own grid times the cell size (4 m), and a stroke larger than 4,096 cells goes as several, a band of whole rows each. Gated off (`acceptance.lua`, `terraform`) |
| `CompanyOp` | create, join, rename or delete a company; its head's password, players and stations (`ShareStations` the default, `StationAccess` one other company over it) |
| `Loan` | take a loan (the offer taken and the offer the game drew to follow it) or pay one back, each on its terms as TF3's loan script keeps them, the interest in millionths |
| `VehicleOp` | a vehicle and what its window does to it: stop or start, to the depot (never sold on arrival: build 40408 sells such a vehicle at the depot, then asks the removed vehicle where it is and fails its engine's assertion, `Engine.h:323`, in every game at once; `Action::validate` refuses it), reverse, depart, its colour |
| `ReplaceVehicle` | a vehicle and its new consist, as `BuyVehicle` carries one, each part also saying which of the vehicle's own parts it keeps (by index, same model), or none for a part bought new; its groups and multiple units. One vehicle each: a group edit is one action per vehicle, as the game sends it |
| `NotificationSeen` | a notification's popup played its first sound: every game's Notifications script marks it (its `initialSound` event), so no game plays it again |
| `Prospect` | prospecting near a town: the town, the cargo, the industry types that may be found in the originator's menu's order, and the company permit it uses. The outcome is not in it: every game's company script draws it from the game time, months later, alike ([investigation](../investigation/TPF3_PROSPECTING_2026-09-30.md)) |
| `ApplyRank` | a company rank to take, as the company window sends the game's growth script (`applyLevel`); the acting player's company takes it ([HOOKS.md](HOOKS.md), "Company ranks") |
| `Perk` | a company perk from the construction menu: Industry Greenification (the industry by its canonical id, `IndustryId`, which every game binds by its construction, and the permit), or a marketing campaign (the town, the campaign's duration and line cost factor, the permit, and the price the tool charged). Gated off (`acceptance.lua`, `perks`) ([HOOKS.md](HOOKS.md), "Company perks") |
| `Preserve` | a town building's Historic Preservation checkbox: the construction it stands in, by file and position, its index in that construction's town buildings, and whether it is preserved. Gated off (`acceptance.lua`, `preservation`) |
| `Paint` | a terrain paint stroke: the materials grid (first texel, columns, rows, run-length values, 255 unchanged) and its mask grid, the same way; a large stroke goes as several, a band of whole rows each. Behind `acceptance.lua`'s `paint` (schema 27) |
| `PlantAssets` | an asset brush stroke: the model files it uses, the groups it replaces (first asset and count) and the groups it builds, each asset its model's index and its matrix (`Transform`). Behind `acceptance.lua`'s `assets` (schema 27) |
| `SetCrossingType` | preparatory type-only replay for a level crossing picked in its window: the crossing by the places of all its nodes (sorted, each once), the type by its resource file. The mechanics path finds the one crossing on exactly those nodes, checks ownership, fills the crossing map through an empty build and reads the type back. `acceptance.lua` keeps `crossings` off pending ordinary-start two-player acceptance. Window rebuilds that map nodes while changing segments or junction settings are refused; this action cannot prove or reproduce those changes. |
| `PlaceSignals` | signals a mod's script places along existing tracks after its player's signal (Auto Signals): each track by its ends (node 0 first), each new signal's place along it from that end and side, each removed signal's place and construction; the new signals' construction, one-way and settings once. Every game rebuilds all the tracks in one proposal, or none. Behind `acceptance.lua`'s `signals` |

The ordinary crossing window may emit a rebuild even for a plain type swap.
The normal UI flow is therefore not carried unless the native proposal has
the separate type-only map shape; matching segment counts does not establish
that its geometry or junction settings are unchanged.

**Polylines.** A road or track build is a polyline: the tool's proposal by
positions, the originator's decisions included:

- `vertices`: each a position and how the originator's tool resolved it:
  `New` (a node the build adds), `Node(network)` (the existing node of that
  network there; a track vertex on a street node is a level crossing), or
  `Split(edge)` (a new node splitting that edge, named by network and ends;
  the halves keep the split edge's own component);
- `links`: the new edges, each two vertex indices, both Hermite tangents,
  the structure (`Ground`, `Bridge(type)`, `Tunnel(type)`) and, for an edge
  that is not the build's own street or track, its kind: network, road
  template and style. A piece of a street the build joins, rebuilt through
  the new junction, keeps that street's kind; so does a street a track
  crosses. A link also names where the edge itself ends, at either end,
  when that is not its vertex (`position0`, `position1`, schema 27; see
  "Edge ends" below). It can also name the signals it keeps from a track
  the build cuts (`kept`, schema 27): each by its removal's number, its
  place along that edge from the removal's `a` end, and its construction.
  A track started or ended between a track's signals cuts that track in
  two; each signal goes with the piece the tool put it on, the piece that
  goes on from one
  of its old track's ends. Near a node the tool moves that node onto the
  cut and redraws the track through it, and may keep a signal on the piece
  before the cut that now stands past it (seen on build 40420,
  2026-10-17); the game places it on that piece, so the room follows the
  tool. Every game finds the one signal of that construction within 0.25 m
  of that place on the removed edge, checks it is no waypoint (lines name
  waypoints; its signal list says, `SIGNAL_LIST`, `Signal.Type.WAYPOINT`,
  whatever its construction: a mod's signal goes like the game's own), the
  acting company's or no company's, and that the piece starts where its
  old track started or ends where it ended, the same way there
  (`geom.continues`: within 0.5 m in three dimensions, tangents within
  about 6 degrees), or runs where the signal stood, the same way
  (`geom.runsThrough`: its centreline within 0.5 m of that place,
  horizontally and in height, its direction there the old track's, within
  about 6 degrees; `geom.keepsSignal` asks both), and lists it on that
  piece under its own entity;
  anything missing or ambiguous refuses the build in every game. After
  the build each kept signal must still exist (else the room's replay
  stops), and how far it moved is said in the log. A cut where the tool
  adds a new signal itself (`edgeObjectsToAdd`, seen where it redraws the
  track around a moved node) is taken as a put-back signal, below. The
  kept-signal behavior passed local two-player acceptance on build 40420,
  2026-10-17; `acceptance.lua`'s `splitSignals` gate is enabled for that
  accepted case. A track started close to a node of a
  track with signals: the tool removes that node, redraws the track
  through its own, and on an edge whose node it removes replaces the
  signal with a new one (`edgeObjectsToRemove` the old signal,
  `edgeObjectsToAdd` one record of category 2 with no model, place or
  settings that read, the piece listing it as a new entity; seen on build
  40420, 2026-10-17). The room takes the new one as the old signal kept on
  that piece, the same `kept` as a cut's, so every game keeps the signal
  under its own entity with its own construction and settings, which the
  tool's record does not say. Only one for one (`engine.lua`
  `refitSignals`): each record a signal on one new track edge, each
  removal a signal on one removed track edge and no waypoint, each new
  signal on a piece that only one removed signal goes onto: a stretch of
  its old track, both ends on that signal's edge or on a removed track
  edge that meets it at a node the build removes, within 0.5 m (the tool
  sets its own nodes on the old track about the node it removes, before
  it, past it or both, and may put the signal back on the piece across
  that node; seen in run-1017-pr165-p2 and run-1017-refit), not rebuilt
  between the signal's edge's own nodes, and that runs where the signal
  stood, the same way (`geom.runsThrough`); anything else is refused as a
  build with a stop or signal. Captured only behind `acceptance.lua`'s
  `refitSignals`, on since a two-player game showed every game keeps the
  signal where it stood (run-1017-refit2, build 40420);
- `removals`: existing edges the build removes, of either network, by their
  ends: an upgrade's, a span the build passes under, the stretch the tool
  rebuilds around a new junction or crossing. A split parent is no removal;
  the split vertex names it;
- `removed_nodes`: existing nodes the build removes, by network and
  position.

TF3's street and track tools state every edge and node they add and remove
(seen on build 40408: a street drawn onto another's middle removes the old
street's nearest node and its two edges, and adds the junction, the new
street and the old street rebuilt through the junction in its own
template), so the capture ships exactly that and the receiver re-derives
nothing from geometry. The TF3 capture makes no `Split`; the variant stays,
and the receivers apply it.

**Bounds.** Decoding refuses anything out of bounds before it allocates:
at most 512 vertices and 512 links per build, 256 edges per removal list or
bulldoze, 1,024 construction parameters, 64 models per consist, 256
vehicles per sell or assignment, 256 stops per line, 8,192 terrain cells,
and the 48 KiB payload over all of it. Text follows the protocol's rules (no
control characters). A polyline must have a link, and every link must join
two different vertices it has; a terrain grid must fill whole rows.
Construction parameters are flattened to paths (`modules[3801].name`), each
an integer, a fixed-point number in millionths, a boolean or text.

**Versions.** The payload is the schema version, then the action, both
postcard. A replica refuses a payload of another version rather than guess.
The schema lives inside the protocol's opaque `Payload`, so changing it does
not change `PROTOCOL_VERSION`: the server never decodes it in `native`
rooms, and players in one room run the same mod because their content must
match. Change the version whenever an existing variant's encoding changes.
Only Rust encodes and decodes it: the mod works with tables (below).

**From Lua.** The mod (`mod/tpf3mp_1`) builds the action as a Lua table
that mirrors the Rust types field for field:
- a struct is a table of its fields by their Rust names, and a `None` is
  a field left out;
- an enum value is its variant name (`"Ground"`) or a one-entry table
  (`{Bridge = "cement.lua"}`);
- vertex indices start at 0;
- numbers are in the game's units: metres, and plain fractions for
  directions and rotations.

The hook turns the table into an `Action` with `tpf3mp_proto::lua`, and
an `Action` to apply back into a table the same way. That conversion
rounds metres to millimetres and fractions to millionths (the nearest,
halves away from zero) and refuses what the schema would: a field the
type lacks, a missing one, a fraction where a whole number goes, a value
out of range, text too long. Its errors name the path to the bad value
(`polyline.vertices[2].pos.x`). So the schema, its bounds and its rounding
are defined once, in Rust (D15).

`tpf3mp/engine.lua` reads a street or track tool's proposal as build 40408
hands it to game scripts (docs/HOOKS.md, "The build tools"), and
`tpf3mp/roads.lua` makes it the action: every node an edge names a vertex
(`New` for the proposal's own, `Node` for an existing one), every added edge
a link, every removed edge and node a removal. A node, tangent or removal it
cannot place, a stop or signal on an edge it moves or removes, or a
construction in the proposal fails the whole capture, and the tool shows
why. The replay (`tpf3mp/apply.lua`) finds existing nodes within 1.5 m
horizontally, and edges and removed nodes by their ends within 0.5 m
horizontally, each node also within 0.5 m in height, the nearest, in the
world before the build. The ends are the nodes' own positions, so an
elevated track over an older one along the same line is named apart from
it (until 2026-10-16 the match ignored height and took the lower). One that
is not there, two nodes exactly as near, or an edge to remove with a stop or
signal on it, fails the build in every game alike.
A node's lane configuration (`BASE_NODE_CONFIG`) names the edges at it, and
on build 40408 the game cannot read a script proposal that removes an edge
a configuration still names (`makeProposalData` raises "Unknown exception"
from its worker threads): the replay removes the configurations at the ends
of the edges it removes (`nodeConfigsToRemove`), except at a node it removes,
which takes its own along and may not be named for both. The replay now
adds preserved configurations back with references to the replacement
edges (`tpf3mp/junctions.lua`). A split matches the unique replacement
with the old edge's tangent at that endpoint, including curved roads.

To keep a junction's settings the replay reads the position of every node
of both networks, but reads an edge only where a setting names a place at
one of its ends: every edge is listed at both its nodes, so the edges at
the nodes there are every edge the whole world could match, and two that
match are still refused. Reading every edge of the world for this cost
about 600 ms of every road build that rebuilt an edge in a late game
(2026-10-16), a street upgrade included; with the nodes looked up by the
metre square they are in, 40-65 ms, and the whole build 80-130 ms.

A preview of another player's road (docs/HOOKS.md, "Build previews"), which
nothing of reaches the world, finds its nodes near the places it names
through the engine's octree: the edges whose boxes reach a column 2 m
wider than the tolerance there, and their end nodes, each read afresh.
Where the octree has none near enough, or the game has none, it reads the
whole network. Reading it whole cost 130-150 ms of each preview in the
GUI, several a second. A build the room applies never asks the octree:
nothing shows that it holds every edge, or one built earlier in the same
update, and one it missed could make two games resolve a vertex apart.
Missing lanes or an ambiguous replacement refuse the build instead of
resetting the player's settings. Before sending, the replay asks
the game's verdict (`makeProposalData`) and refuses a build it calls
critical, with its reasons.

**Edge ends.** An edge's own ends (`BaseEdge.position0`, `position1`)
need not be its nodes' positions. The track tool, laying a track onto the
free end of a street with tram tracks, starts the track on the street's
incoming tram lane, 2 m beside the end node (seen on build 40420 in a
single-player game: node at (-1737.13, -611.14), track start at
(-1737.44, -613.11)). Only there does the game join the two: the street's
tram lane and the track then share one transport-network node, and trams
drive from the street onto the track. A track that starts on the node's
centre, everything else the same, gets a network node of its own and ends
in a buffer stop. A script proposal shows the same, so it is the edge's
end, not the native tool, that joins them. The capture therefore carries
each link's own end where it lies a millimetre or more off its vertex
(`position0`, `position1`, at most 32 m off on each axis), and the replay
builds the edge to end there. Before schema 27 the replay put every end on
its node, and such a track ended in a buffer stop in every game of the
room (2026-10-15). With schema 27, checked in a two-game room the same
day: a tram street built and upgraded in one game, then two tracks laid
onto its free end; in both games the street's tram lanes and the tracks
share the end node's transport-network nodes (four, as in single player).
A split still builds each half from the split edge's node positions.
The tests `tpf3mp-proto/tests/lua_capture.rs` (a junction rebuilt around a
new street, a level crossing, a bridge, a tunnel, an upgrade, the TF3
proposal's shape) and `lua_mod.rs` (the replay) run it in Lua and decode
the bytes with the Rust schema.

### Junction edits (action schema 11)

`EditJunctions` carries node positions and connected edge endpoints in
millimetres, lane indices counted from the junction, crosswalk edges,
the Auto/Yes/No traffic-light preference, a light resource name, ordered
phases, their locked-lane indices, duration/minimum in milliseconds,
skip flags, double-slip and custom-phase flags. No engine entity or
resource integer crosses the wire. A missing configuration is an explicit
reset. Road/track polylines also carry the junction updates in their
proposal; construction entrance polylines use the same representation.

The adapter resolves references in three dimensions within 2 mm, refusing
ambiguity, missing resources, missing lanes and changes to another
company's edges. Lists are rebuilt in index order before assigning the
engine's vectors. It preserves the phase-to-connection relationship.
The schema caps an edit at 64 junctions, each with 256 connections,
256 crosswalks and 64 phases, and validates locked indices and durations.

Standalone edits are behind `strict_junctions` in `junctions.lua`, **off
by default** under PLAN.md Part 3. The fork also keeps
`double_slip_only = false`: no narrow exception is enabled. U-D30 records
upstream's decision; ReedME has not adopted it. For an authorized full
junction acceptance test, set `strict_junctions = true` only in matching
disposable mod copies on both games and use the matching hook/profile.

For a narrow mechanics fixture only, `strict_junctions = false` and
`double_slip_only = true` exercise upstream's exception. Capture and replay
then let one edit through and refuse the rest (`junctions.requireNarrow`),
each comparing the configuration put in with the one its own game has at
the node, entity by entity: the
junction window's double slip switch turned on or off, as the window was
seen to send it on build 40420. Nothing else at the node may change:
lane connections, crosswalks, light, phases (compared in whole
milliseconds, as the wire carries them) and the custom-phase mark stay
exactly as they are. The one side effect seen with the switch, the
preference from Auto to No, may come with it, and only at a node no
street joins, where a light has nothing to stop. A reset, a node with no
configuration yet and an edit that leaves the switch as it is are
refused. The crossing tool's tram lanes stay refused until the tool is
seen in the game. The refusal names what it saw, after the
`strict_junctions` reason, with the old and new light settings and the
lanes' count. Road and track builds carry their junction settings as
before, unchecked by this; the log's `handed the player's build` and
`building` lines name each node's settings they carry (`+j<node>{lanes
N tram M}`) and take out (`-j<node>`).

Reported upstream in two games on its local server on 2026-10-15 (Steam
Windows 40420): a double slip switch turned on at a track crossing in one game
was carried as `EditJunctions` and applied in both, with matching
checkpoints. Tests cover native
memory decoding, portable round trips into replicas with different IDs,
curved-edge preservation, refusal cases and checkpoint differences. This
is adapter evidence, not a completed real-game playtest. Follow the
two-game checklist in HOOKS.md before changing the default.

Not in version 1: companion spans (an unchanged bridge span the engine
re-adds), construction street pieces (`ROADC`), paint and the asset brush,
signals and waypoints as their own placements, vehicle orders beyond a line.

## Measure these first on a TPF3 build

In the order they were expensive on TPF2:

1. Dump the UI's proposal and a script-built proposal for the **same** depot
   on a junction and diff them field by field; the geometry will match and
   the flags will not.
2. Confirm whether the script's build-proposal path accepts a construction at
   all, and which single field it insists on (`seed` here).
3. Check whether `Apply` names and owns child entities, and what happens on a
   click when it does not.
4. Place one station on flat ground far from any road with two instances and
   diff every edge's height, not just x and y, between them.
5. Find the crossing recorder's straight-line assert and what the visitor
   requires of the other network at a shared node.
6. Build a road between two existing junctions and check the capture saw an
   edge with no new nodes.
7. Whether the road and track builders send their proposal through
   script (`api.cmd.sendCommand`). TF3's GUI is Teal code, and mods for
   build 40391 send commands from it
   ([investigation/TF3_MODS_2026-09-27.md](../investigation/TF3_MODS_2026-09-27.md)).
   If the builders do too, capture may move into the mod, and the hook's
   filter for our own replays must change (HOOKS.md, "The command
   pipeline").
