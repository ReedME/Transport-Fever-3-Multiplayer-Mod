# Bridge and tunnel window: local two-game validation

Validated on `feat/bridge-tunnel-window` (dev at 8500932), Transport Fever 3
build 40420 (Steam, Windows), 2026-10-11. One run,
`target/game-runs/run-1011-202254`: the rig's local server and two games
started by `tools/game/room.ps1` on one PC, from a player's own save (`MP`,
a large world with electrified tracks, roads, bridges and tunnels). The
`bridges` gate was on in the repository's mod, so both games ran the same
mod. A player changed the world through the ordinary GUI in both games; no
console command was sent. No production relay and no game-install file was
touched. Both games were quit through `tools/game/quit.ps1`.

## Acceptance outcome

- Picking another type in a bridge's or tunnel's window (`bridge_and_tunnel.tl`,
  `createBridgeOrTunnelProposal`, sent as `makeWorldBuildProposalCmd`)
  went to the room as an ordinary build, through `capture.windowBuild`
  and `capture.modify`, and both games applied it:

  | action | player | what | went as | rebuilt in place |
  |---|---|---|---|---|
  | 0 | host | a track bridge's type | `BuildTrack` | 35 edges |
  | 1, 2 | guest | the same bridge's type, twice | `BuildTrack` | 35 edges |
  | 3 | guest | a road tunnel's type | `BuildRoad` | 9 edges |
  | 8 | guest | a road bridge's type, with a bus stop on it (action 7) | `BuildRoad` | 6 edges |

  Each game's `hook.log` has the same `mod: building ...` and `mod: upgrade
  applied: ... rebuilt in place` lines for each of them, byte for byte.
- The window's proposal has the shape `capture.inPlace` expects: no
  construction, no node added or removed, every new edge between the ends
  of one it replaces. None was refused.
- The bus stop on the road bridge was still there in both games after its
  type changed (seen by the player).
- The player reports that the window's "none" choice (a bridge rebuilt as
  ground) went as well; the logs do not say which of the track swaps it
  was, since the build line is cut at 1000 characters.
- The room's rolling world checks matched at all 21 windows of the run
  (steps 1 to 1050): the game was paused for the first swaps and ran
  afterwards, so the windows after step 250 cover every swap. Nothing was
  refused, nothing failed to apply (`not applied`) and nothing diverged.
  `stdout.txt` had no Lua error after the worlds loaded; the only ones
  were the save's railroad crossing mod (`mw_bahnuebergaenge`) failing
  its `.rcr.lua` files while each game loaded the world, before any swap.
- The `bridges` acceptance gate is now enabled.

The TPF2 multiplayer project saw the same shape for its bridge window in
its own game (`tpf2-multiplayer`, `native/src/slice_hook.cpp`,
`CALLER_BRIDGE_UPGRADE`: "0 new nodes, 10 added/10 removed bridge edges",
2026-09-14). It never carried tunnel types.

## Second run: a signal on the bridge, a track tunnel

`target/game-runs/run-1011-210631`, the same save and PC, with this branch
merged locally with `fix/signal-dotted-settings` (Signal Distance's
`signal_distance.local`, which had kept any signal from being placed in
the first run):

| action | player | what | went as | rebuilt in place |
|---|---|---|---|---|
| 1 | host | a signal on the track bridge's edge 134154 | `PlaceStop` | (signal 135926 in both games) |
| 2 | host | that bridge's type, the signal on it | `BuildTrack` | 35 edges, the signal's among them |
| 0 | guest | a new track through a hill (a tunnel) | `BuildTrack` | |
| 1 | guest | that tunnel's type | `BuildTrack` | 13 edges |

The window's proposal listed the signal on its rebuilt edge, so
`engine.keptInPlace` let it through, and the signal still stood in both
games afterwards (seen by the player). The rolling world checks matched
at all 17 windows (steps 1 to 850); the game ran from just after the
bridge's swap, so the windows cover all four. Nothing was refused, failed to apply or
diverged; `stdout.txt` again had only the railroad crossing mod's
load-time errors.

## Not exercised

- A waypoint on a swapped bridge (it goes the same way as a signal).
- Two players changing one bridge in the same step; a bridge shared by
  two companies' edges; other platforms.
