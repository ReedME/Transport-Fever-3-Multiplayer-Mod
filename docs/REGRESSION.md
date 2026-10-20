# The regression harness

Scripted play-throughs, several games to a room, checked as they go.
Actors build streets and track, place stations and depots, buy vehicles,
make lines and assign them, edit and bulldoze, and found and leave
companies. Every game checks its world at fixed points of the script, and
the games must end in the same world. TPF2MP's suite of this kind took
about two hours; this one plays its whole library in seconds, and is meant
to stay under 10 minutes a platform once the real game plays it.

Today the games play a model of the game (below), not Transport Fever 3.
The `junctions` scenario builds a street junction, applies turns,
crosswalks and timed light phases, refuses another company's edit and
resets it. The `double_slip` scenario turns a double slip switch on and
off where two tracks cross, the one junction edit a room carries before
the junction acceptance.
Junction state participates in the model's network digest.
The native/Lua adapter tests and pending real-game checklist are in
[HOOKS.md](HOOKS.md#junction-tools).
The script, the runner and the checks do not depend on which: the real
game's hook plugs into the same script once it applies actions ("With the
real game").

## Running it

```sh
cargo run --release -p tpf3mp-testkit --bin tpf3mp-regress              # every scenario
cargo run --release -p tpf3mp-testkit --bin tpf3mp-regress -- --smoke   # the quick subset
cargo run --release -p tpf3mp-testkit --bin tpf3mp-regress -- --list
cargo run --release -p tpf3mp-testkit --bin tpf3mp-regress -- --only town --repeat 20 --replicas 3
cargo run --release -p tpf3mp-testkit --bin tpf3mp-regress -- --offline # no server: the scripts only
```

```text
PASS bus-line          3575 steps    0.95 s  2 replicas  act lag p50 47ms max 52ms
PASS two-companies     4425 steps    1.20 s  2 replicas  act lag p50 46ms max 52ms
...
PASS town             20000 steps    5.23 s  2 replicas  act lag p50 46ms max 52ms
9 runs: 9 passed, 0 failed, 5.2 s
```

It exits non-zero when anything failed, and prints each failure under
its scenario. Each scenario plays in a room of its own, several at once
(`--parallel`, by default half the processors), against a server in the
same process unless `--server host:port` (with `--pin-cert`) names one.
`--repeat` plays each scenario again with another world seed;
`--replicas` puts more games in each room, the extra ones only watching.
`--speed`, `--sps`, `--input-delay-ms` and `--checkpoint-interval` set
the rooms; by default they run at the fastest the server allows. That
takes a fast machine: on a small one, or in a debug build, play slower
(`--speed 400`) and fewer rooms at once. A game whose agent is starved
for 30 s gives up, and its scenario fails with "the agent stopped
responding".

Where it runs:

- **`ci`**, in `cargo test`: every scenario offline, and the quick subset
  and a four-player room through a real server
  (`crates/tpf3mp-testkit/tests/regress.rs`), one room at a time at
  quarter speed. A replica that drifts must
  fail its scenario, and a wrong expectation must fail on every replica.
- **`acceptance`**, on every platform: the whole library five times, three
  games to a room, optimized, at half speed and two rooms at a time.

## How it plays

A scenario is a list of items (`crates/tpf3mp-testkit/src/regress/script.rs`):

- **act**: actor *n* does an action of the portable schema
  (`tpf3mp_proto::action`, BUILDING.md). Actors are the room's players in
  the order they joined. The next item waits until the room has ordered
  the action;
- **run** *n* steps;
- **expect** a check: counts of edges, stations, depots, lines and their
  stops and vehicles, idle vehicles, ignored actions, passengers
  delivered, an actor's money or company.

Each game in the room runs the whole stack a game uses: client, bridge,
shared-memory link, `Session` and step gate, with the model world behind
`Game`. An actor's game sends its actions through `Session::command`,
the path a player's captured actions take, and every game applies them
as the events the room orders. Every game walks the script with a
`Cursor` driven only by those events and the steps it runs, so all of
them reach each item at the same step and judge each check on the same
world, with no side channel between them. The same script therefore plays
the same across processes and machines.

The world runs on while an action travels to the server and back, as a
game does: about 50 ms, or 180 steps at the harness's speed (240 steps a
second at 16x). How many steps is not fixed, so a check must hold however
many passed: money is checked right after building, before fares come in,
not after a long run.

A game sends its acts at least 60 ms apart (`HarnessPlan::min_gap`): a
room takes 20 intents a second from one player, after a burst of 40, and
a script can act faster than that where the round trip is short.

A game stops at the first checkpoint after the script ends, so the room
compares every world there too. The scenario fails if:

- a check fails on any game;
- the room refuses an action, or an action is not ordered within the
  stall limit (`HarnessPlan::stall_steps`);
- the room reports a divergence;
- the games stopped at different steps, ended with different lanes, or
  judged a check differently.

Rooms are played by the game's own rules (`native`), as rooms are by
default: the server orders every action, and each game applies what fits
its world and ignores the rest, counting it. Every game ignores the same
actions, which the `refusals` scenario checks one reason at a time.

## Writing a scenario

Scenarios live in `crates/tpf3mp-testkit/src/regress/library.rs`, built
with small helpers (`road`, `track`, `construction`, `buy`, `line`,
`assign`, `replace`, `place_stop`, `bulldoze_*`, `terraform`, `company`):

```rust
Script::default()
    .act(0, road(vec![new(at(0, 0)), new(at(1000, 0))]))
    .act(0, construction(BUS_STATION, at(0, 20), "Harbour"))
    // ...
    .act(0, assign(&[0, 1, 2], Some(0), 0))
    .expect(Check::Idle(0))
    .run(2_000)
    .expect(Check::DeliveredAtLeast(1))
    .scenario("bus-line", "what it covers", true, 1)
```

- Positions are in metres (`at(x, y)`).
- Stations, lines and vehicles are named by canonical id, counting from 0
  in the order they were made.
- Each player founds a company on joining, in join order: actor *n*'s is
  company *n*. A scenario that counts companies, or names one an action
  founds, needs exactly its actors in the room (`.exact()`).
- Mark it `smoke` when it is quick and covers something the other quick
  ones do not.

`cargo test -p tpf3mp-testkit --lib regress` plays it offline at once. A
new action or rule the hook learns gets a scenario here in the same
change.

## The model game

`regress/model.rs` plays the action schema on a small deterministic
world: street and track graphs with node snapping and edge splits within
BUILDING.md's tolerances, constructions (stations and depots by file
name), stops on edges, vehicles, lines, terrain and companies with
money. Vehicles on lines carry passengers between stations, with fares
and upkeep, driven by a seeded generator. Vehicles and stations have
names and vehicles a colour, set as their windows set them (`Rename`,
`VehicleOp` `Recolor`) and only by the company that owns them; the
`line-editing` and `two-companies` scenarios check both. An action is applied whole or
not at all. An edge with a stop on it is removed only where the same build
rebuilds it between the same ends (an upgrade tool's build), and the stop
stays; the `upgrades` scenario plays that, an electrified track and a
terraform in bands. Its lanes are the network, constructions, lines, vehicles and
the economy.

It is not Transport Fever 3. It tests the stack, the scripts and the
checks, and it pins down what the hook must do (resolve by position, fail
closed, ids in creation order). It says nothing about the game's engine.

## With the real game

What the hook needs before the real game can play these scenarios, in
PLAN.md's order:

1. **Actions applied** (`Tf3Game::apply`, Part 2 and Part 3). Each
   scenario then tests the replay of the actions it uses.
2. **An observation**: the hook answers `Observation`'s questions from
   the game (edge counts per network, constructions, stations, depots,
   objects on edges, lines with their stops and vehicles, vehicles and idle
   vehicles, each player's company and money, passengers carried, actions
   refused, terrain cells changed), through the Lua mod's `api` reads.
3. **A test mode** in the hook: it reads a scenario file named in its
   environment and walks the same `Cursor` inside its `Game`, sending its
   actor's actions through `Session::command`. The cursor needs only the
   events and steps the game already sees, so it runs in the hook
   unchanged. Scenarios then need a file format (serde on `Scenario`).
4. **A start from a save**: every game loads the same fixture save through
   `StepGate::Load` instead of an empty map, so scenarios can use towns
   and industries.
5. **Speed**: the real game steps only as fast as it simulates and draws.
   Whether it runs faster minimized, or with drawing skipped by the hook,
   is not known yet (PLAN.md, Part 2); until it is, rooms run at the
   game's top speed.
6. **Several rooms at once**: `tpf3mp-rig` starts the games of one room;
   memory decides how many a PC runs.

A person starts real games with the rig. Automation never launches the
game (AGENTS.md).
