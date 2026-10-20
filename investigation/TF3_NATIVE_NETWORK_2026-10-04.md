# Reading the street and track network natively (build 40408)

Static findings from `tools/tpfre` on the build 40408 index
(`investigation/dayone-2026-09-29/TransportFever3.tpfdb`, exe sha256
de1daad3…), gathered on 2026-10-04 for a native NETWORK lane. Nothing here
has been read from a live edge yet; every reader built on it checks it
against the Lua read first. RVAs are hex.

Why: the checkpoint's Lua lane read froze every game in a room for about a
second every 10 s on a save with 2212 edges (docs/HOOKS.md, "What the
lanes cost").

## Checked in the game (2026-10-05)

`crates/tpf3mp-hook/src/netread.rs` reads the network lane with these
layouts. In a room of two games on `MP_crash_1004`, with
`TPF3MP_HOOK_NATIVE_LANES=compare`, its rows agreed with the mod's Lua
at every checkpoint read: 2212 edges, then 2213 after a town grew a street,
and 2126-2127 junctions. Two corrections:

- `CompVec+8` is not the type id: that `+8` is the type map's entry
  (`[it+8] = pools.size()+1` in `sub_94770`, `it` the entry
  `sub_9d1e0` found at `engine+0x48`). A type's id is its pool's index at
  `engine+0x78`; each entity's component list repeats it.
- `CompVec+0x98` is no slot count to bound by: read in the game it held
  values past any count, now and then. The game's own accessor
  (`sub_2806a0`) bounds nothing; the page table's length bounds a paged
  index.

Later the same night, also checked in the game:

- `Construction` (`CompVec<Construction>` vtable 0x3680510; 0x288 bytes,
  `imul rax, 0x288` at 0x28081b in `sub_2807d0`): its Lua binding
  (`sub_18829c0`, the call of `sub_1863020` at 0x1882ccd, name/offset
  pairs) puts `fileName` at +0 (a `ResName`), `params` +0x40, `transf` +0x58
  (16 float32, column-major: `UI::ModuleBuilder::SetConstruction`,
  `sub_544f10`, reads elements 12 and 13 at +0x88/+0x8c as the translation's
  x and y), `timeBuilt` +0x98, `frozenNodes` +0xa0, `frozenEdges` +0xb8,
  `depots` +0xd0, `stations` +0xe8, `industries` +0x100, `townBuildings`
  +0x118, `subconstructions` +0x130, `persistentMetadata` +0x148, `slots`
  +0x160, `slotConfig` +0x178, `dependentSlots` +0x1b8, `tag2modelInstance`
  +0x1f8, `entityAndIndex2labelTexts` +0x228, `tag2segments` +0x258. The
  constructions lane read from `fileName` and `transf` agreed with the mod's
  at every checkpoint (1857-1863 constructions).
- A `ResName` reaches the game's Lua as `first::/second`: the edges' road
  templates and the constructions' files read that way agreed with the
  Lua's `tostring`.
- The game's Lua sorts strings byte by byte: the hook's sorted, joined and
  hashed network lane agreed with the mod's `summary` at every checkpoint.

## Corrections to docs/HOOKS.md (~5037-5050)

- `GetComponentDataIndex` is `sub_a4b90`, not `0xd0920` (inside a phmap
  rehash, `sub_d0710`).
- The component pools are at `engine+0x78`, not `+0x88`; the element stride
  is `sizeof(T)` (0x48 is GameTime's).
- Take the engine as `CGameTime+8`.
- The proposal edge record (HOOKS.md ~5033) is not the component's layout.

## Component lookup

- `typeId = sub_a4cc0(engine+0x48, &type_info) ` (ComponentManager's phmap,
  returns the stored value minus 1; asserts if the type is unregistered).
  Type ids are assigned at a type's first AddComponent (`pools.size()+1`,
  `sub_94770` ~0x947f7): they differ per game; look them up at run time.
- `idx = sub_a4b90(engine, entity, typeId)` (asserts if the entity lacks the
  component). Use the per-entity bitset (`[engine+0xc0]`, 16 bytes per
  entity, bit = typeId; `HasComponent` `sub_2bb60a0`, no bounds check) or
  the Try variants first: BaseEdge `sub_280e40`, BaseNode `sub_2811b0`,
  BaseNodeConfig `sub_281290`.
- `pool = [engine+0x78][typeId]` (8-byte entries; typeId < (end-begin)/8).
  CompVec<T> (sizeof 0xa0): +0 vtable, +0x10/+0x38 free-list deques,
  +0x60 change counter (add and remove), +0x68/+0x70 dense vector<T>,
  +0x80/+0x88 page table of 16-byte `{T* page, ctrl}` (pages of 32). (Not
  +8 type id+1 nor +0x98 a slot count: see "Checked in the game".)
- Dense: `idx < 0x40000000`, data `[pool+0x68] + idx*sizeof(T)`, bound
  `(pool[0x70]-pool[0x68])/sizeof(T)`. Paged: `i = idx-0x40000000`, data
  `[[pool+0x80] + (i>>5)*16] + (i&31)*sizeof(T)`, bound
  `(i>>5) < (pool[0x88]-pool[0x80])>>4`. Which mode each type uses is
  unknown: handle both.

Type descriptors (name at descriptor+0x10): BaseEdge 0x3cea7c8
(`.?AUBaseEdge@component@ecs@@`), BaseNode 0x3cf2920, BaseNodeConfig
0x3cf3b48, GameTime 0x3cf0bd0. Sizes: BaseEdge 0x118 (`sub_95530`,
`imul rax,0x118`; Lua userdata 0x128 = 0x10 + 0x118), BaseNode 0x14
(`sub_2806a0`), BaseNodeConfig 0x78 (inlined, `sub_281290` 0x281319).

## Entities

- `[engine+0x90..+0x98)`: one 24-byte record per entity id, a
  vector<{int32 typeId, int32 dataIndex}>; slot count = (end-begin)/24.
- Alive: id >= 0, id < slots, and not (one pair with typeId < 0)
  (`sub_4f7db0` 0x4f7e0a..0x4f7e29). RemoveEntity (`sub_2bb75f0`) leaves
  one pair {-1,-1}; an empty record is a live entity without components.
- Ids are recycled (free list at engine+0xe0..0xf8); revisions at
  `[engine+0xa8]`, 12 bytes per entity, first int bumped on add and remove.
- No locks anywhere on these paths; writes are batched between Begin/End
  modification (`engine+0x1f0`). Read on the engine's own thread only.

## Layouts

BaseEdge (Lua binding registration `sub_1767d20`): node0 +0, node1 +4,
position0 +8, position1 +0x14 (Vec3f), tangent0 +0x20, tangent1 +0x2c,
type +0x38 (BaseEdgeType: 0 normal, 1 bridge, 2 tunnel; inferred),
typeIndex +0x3c, objects +0x40, edgeDecorations +0x58, laneConfigs +0x70
(vector<LaneConfig>, inferred), roadDevelopmentLocked +0x88, distance
+0x8c, roadType +0x90 (int32: TRACK 0, STREET 1; asserts in `sub_5bc120`,
`sub_904f50`), roadTemplate +0x98 and roadStyle +0xd8 (ResName, 0x40
bytes: two MSVC std::string of 0x20; which holds the path is unknown).

LaneConfig (0x18, registration ~0x1f64fa8): speed +0, width +4, height +8,
forward +0xc, transportModes +0x10 (a 32-bit std::bitset word, bit =
TransportMode: 0 PERSON … 14 TRAM_TRACK, 15 ELECTRIC_TRAM_TRACK), offset
+0x14.

BaseNode: position +0 (0x14 bytes).

BaseNodeConfig: as `crates/tpf3mp-hook/src/junctions.rs` reads it in a
proposal (laneConnections vector +0, 0x14 each; crosswalk phmap set
+0x18..0x47; doubleSlipSwitch +0x48; trafficLightPreference +0x4c; states
+0x50, 0x28 each; trafficLightType +0x68; userModifiedTrafficLightStates
+0x70).

Correction after live inspection (2026-10-05): `getComponent` returns a
borrowed `UserdataPtr` (vtable `0x36ee848`). `api.type.BaseEdge.new(component)`
creates the owned `UserdataValue<BaseEdge>` snapshot (vtable `0x374f800`).
The BaseNodeConfig copy constructor similarly creates an owned snapshot.
See [the checkpoint snapshot investigation](TF3_CHECKPOINT_SNAPSHOTS_2026-10-05.md).

## Signatures (`tpfre q sig`)

- `sub_a4b90`: `48 89 5C 24 20 89 54 24 10 57 48 81 EC E0 00 00 00`
- `sub_a4cc0`: `48 89 5C 24 10 57 48 81 EC 90 00 00 00 48 8B FA`
- `sub_2bb60a0` (HasComponent): `48 89 5C 24 08 48 89 74 24 10 57 48 83 EC 30 48 63 DA`
- The GetComponent<T> wrappers differ only in their RIP-relative
  descriptor and cannot be signed; anchor the descriptors by their RTTI
  name strings (descriptor = string - 0x10).
