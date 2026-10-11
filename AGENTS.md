# Working on the ReedME TPF3-MP fork

This repository is ReedME's fork. ReedME is its owner; upstream decisions
and contributor attribution below remain historical context. The user
authorized implementing the 2026-10-11 review, including the scoped Proton
installer exception recorded in `docs/DECISIONS.md`. Build and test changes
on a feature branch and keep native game verification and signed releases
as separate gates. Do not deploy to or configure the upstream project.

How changes are made and released, for people and coding agents alike. Read
this before changing anything. [README.md](README.md) says what the project
is, for players; [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) where it stands
and how to build it; [docs/GAME_TESTING.md](docs/GAME_TESTING.md) how to
test in the real game; `docs/` says how it works. Check every task against
[docs/PLAN.md](docs/PLAN.md) and [docs/DECISIONS.md](docs/DECISIONS.md)
first, and flag a conflict instead of working around it ("Before starting
a task" below).

## Branches

Changes move through three long-lived branches, always in this order:

```
feature branch ──> dev ──> acceptance ──> main
 (your work)     (testing) (release     (release: CI/CD drafts
                            candidate)    the release)
```

| branch | holds | may receive | gate to the next |
|---|---|---|---|
| feature (`feat/…`, `fix/…`, `docs/…`) | one change in progress | your commits | `ci` green on the branch |
| `dev` | the integration and testing line | pull requests from feature branches whose `ci` is green; a change to `docs/DECISIONS.md` or `docs/PLAN.md` also needs the owner's approval | `ci` green on `dev` |
| `acceptance` | the release candidate | fast-forwards from `dev` | `ci` **and** `acceptance` green on `acceptance`, plus the manual checks below |
| `main` | what is released | fast-forwards from `acceptance` | the `release` workflow drafts the release |

Rules:

- **Never commit to `dev`, `acceptance` or `main` directly.** Work on a
  feature branch cut from `dev`, and merge it in once its `ci` is green.
- **Promote by fast-forward only.** `acceptance` and `main` never get
  commits of their own, so each always equals an earlier state of the
  branch before it, and a release is exactly a commit that passed every
  check. Force-pushes to these branches are never allowed.
- **Do not skip a stage.** Nothing reaches `main` without passing
  `acceptance` first, however small the change. A fix found in
  acceptance goes in on a feature branch, then through `dev` again.
- **A red check stops promotion.** Fix it on a feature branch; never
  promote around it or disable the check.
- New work starts from `dev`. Only `dev`, `acceptance`, `main` and
  branches in progress exist; delete a feature branch once `main` has it.
  The old milestone branches are kept as the tags
  `archive/m0-foundations` and `archive/m1-core`, and review
  proof-of-concept tests never merged as `archive/review-snapshots`.
- **Pull requests into `main` or `acceptance` are not the way in.**
  Merging one creates a commit that no check has seen and skips the stage
  before. Open pull requests into `dev` if you want a review; promote with
  the fast-forwards below.
- Contributors may freely propose decision changes and alternative designs in
  pull requests. Explain the trade-offs; the owner decides whether to adopt them.

### What GitHub enforces

`tools/github/protect-branches.sh`, run once by a repository administrator,
makes GitHub hold everyone, administrators included, to the rules above:

- `dev`, `acceptance` and `main` cannot be force-pushed or deleted;
- `acceptance` takes only commits whose `ci` checks passed;
- `main` takes only commits whose `ci` and `acceptance` checks passed.

A promotion pushes a commit already tested on the branch before, so it
passes; a merge commit made on `acceptance` or `main` has no checks and is
refused. When a job in `ci.yml` or `acceptance.yml` is renamed or added,
update the script's lists and run it again.

## The checks

- **`ci`** (`.github/workflows/ci.yml`) runs on every push and pull request,
  on Windows, Linux and macOS:
  - `cargo fmt --check`;
  - `clippy -D warnings`;
  - the whole test suite;
  - release builds of the binaries players and servers run;
  - the server container image;
  - that the launcher page's script parses;
  - on Linux, format, lint and tests of the binary-analysis kit
    `tools/tpfre` (D14), its own Cargo workspace.
  - on Linux, the release-day tools' tests (`tools/dayone`), the lane
    dump diff's (`tools/test_lane_diff.py`) and a real
    Lua's parse of the probe mods (`tools/probe/check_lua.py`).
- **`acceptance`** (`.github/workflows/acceptance.yml`) runs on pushes to
  `acceptance`. It runs optimized load tests on all three platforms, each
  failing on any failed bot or diverged replica:
  - rooms of bots at the room's pace;
  - a bad network (latency, jitter, loss);
  - every bot through the WebSocket tunnel;
  - rooms logged and compacted under load;
  - the regression scenarios, five times over, three games a room
    ([docs/REGRESSION.md](docs/REGRESSION.md));
  - a 15-minute soak on Linux.
- **Manual acceptance**, before promoting to `main`, for changes they
  touch:
  - the launcher window, used by hand against a local server: connect,
    create a room, join by invite, start the game from the launcher, play
    (see [docs/PLAYING.md](docs/PLAYING.md)). Until the game is out,
    `--game-exe` names `tpf3mp-fakegame` as the game: "Start Transport
    Fever 3" then starts it with the hook loaded into it, and it must
    join the room. `cargo test -p tpf3mp-launcher --test screenshots --
    --ignored` renders its screens to `target/launcher-screenshots/`, one
    for each sample state of the page it copies, for a look at the layout
    (D20);
  - a server upgrade that keeps running games (see "Upgrades" in
    [docs/OPERATIONS.md](docs/OPERATIONS.md)), when the log format, the
    protocol or persistence changed.
    `tools/acceptance/upgrade-check.sh <old bin dir> <new bin dir>` plays
    it through with the fake game, from `main`'s binaries to the
    candidate's;
  - once the game is out: a real game on each platform
    ([docs/DAY_ONE.md](docs/DAY_ONE.md)).
- **`release`** (`.github/workflows/release.yml`) runs on pushes to `main`.
  Before packaging (also on manual runs), `tpfre verify-build` checks the
  selected native bundle against the private archive on the dedicated
  `tpf3mp-game-builds` runner. Missing inputs or failed checks stop packaging;
  setup is in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#game-update-builds).
  It builds the packages for every platform and attaches them to a draft
  release `v<version>`, the version in `Cargo.toml`. A person reviews the
  draft and publishes it, which creates the tag. Once a version is
  published, `main` needs a version bump before it releases again: bump it
  on a feature branch like any change.

## Promoting

```bash
# a feature into dev: a pull request, merged once its ci run is green
# (and, when it changes the decisions or the plan, the owner approved it)
gh pr create --base dev --head feat/my-change

# dev into acceptance, once ci on dev is green
git switch acceptance && git pull --ff-only
git merge --ff-only dev && git push origin acceptance

# acceptance into main, once ci, acceptance and the manual checks are green
git switch main && git pull --ff-only
git merge --ff-only acceptance && git push origin main
```

Check a branch's runs with `gh run list --branch <branch>`. Promote only
the exact commit those runs tested: if the branch moved since, wait for the
new runs.

## Before starting a task

The team's plan is [docs/PLAN.md](docs/PLAN.md): what comes next, who
takes it, and a table of asks that conflict with a decision. Before
starting any task, check it against that page and
[docs/DECISIONS.md](docs/DECISIONS.md). Coding agents in particular:

- **Flag conflicts; do not settle them.** When a task asks for something
  a decision rules out (for example Steam networking or a player as host
  against D2 and D4, joining without the launcher against D11, choosing
  a server against D12, a proxy DLL against D9 and D11), stop and tell
  the person who asked: name the decision and what it says, and ask how
  to go on. Do not quietly build it, and do not quietly build something
  else instead.
- **Mention what changed.** When a task is based on an older version of
  the plan, say which of its items the plan has since changed, as marked
  there with *Changed:* or *Added:*.
- **Fail closed in the plan too.** An action whose channel is not checked
  yet is refused in a multiplayer game, never sent unchecked (PLAN.md,
  Part 3).
- **Keep the plan current.** Tick an item in the change that finishes it.
  A new decision goes into DECISIONS.md first, then the plan follows.
- **Decisions are the owner's.** Only this fork's owner (ReedME) makes or
  changes a decision, or settles a question the plan leaves open for
  them. Propose one in a pull request into `dev`: GitHub asks the owner
  to approve any change to `docs/DECISIONS.md` or `docs/PLAN.md`
  (`.github/CODEOWNERS`). Never write a decision, or mark an open
  question decided, on your own or another person's say.

## Rules for every change

- **Rust everywhere; fail closed.** When unsure, refuse, disconnect or set
  aside. Never guess, and never damage a player's game or a room's log.
  Decisions and their reasons are in [docs/DECISIONS.md](docs/DECISIONS.md);
  record a new decision there instead of rewriting an old one.
- **Tests with the change.** A behaviour change comes with a test that fails
  without it. Run `cargo fmt --all`, `cargo clippy --workspace --all-targets
  -- -D warnings` and `cargo test --workspace` before pushing.
- **Docs with the change.** Update the doc that describes what changed
  (`docs/PROTOCOL.md`, `docs/OPERATIONS.md`, `docs/PLAYING.md`, …) in the
  same change. Bump `PROTOCOL_VERSION`, `BRIDGE_VERSION` or the log's
  `FORMAT_VERSION` when their format changes.
- **Commit messages** say what changed for players or operators, in the
  imperative ("Let each room's host choose its rules"), with the details in
  the body.
- **Preserve contributor provenance.** Keep original commit authors and
  `Co-authored-by` metadata accurate; do not rewrite shared history to alter
  credit. In PRs and release notes, distinguish the PR opener, request or
  design origin, implementation authors, reviewers, and prior-art sources.
  Use the [PR template](.github/PULL_REQUEST_TEMPLATE.md), verify GitHub's
  commit author list, and follow [docs/CONTRIBUTION_PROVENANCE.md](docs/CONTRIBUTION_PROVENANCE.md).
  Never infer a person's role or preferred name from an account alone.
- **Other repositories are read-only.** The sibling TPF2 projects (`tf2mod`,
  `tf2mp-relay`, `tpf2-multiplayer`) and the game install are inputs; never
  modify them. Credit code or test vectors taken from them in the file that
  uses them.
- **The real game only as [docs/GAME_TESTING.md](docs/GAME_TESTING.md)
  says**: with `tools/game`, on the local rig's server, for a real-game test
  the owner asked for or a fix to something that failed in the game; only
  games you started; never another player's or the production server.
- **Secrets stay out of the repository**: keys, certificates and
  `invite.key` included.
