# ReedME fork setup and acceptance

This public fork targets trusted friends competing as separate companies in
the Windows Steam game under Proton. The sequencer can run natively on Linux.
The review baseline is upstream v1.3.1 at
`538733611cc5e4c8ea30d48513d5b0df761db05a`; the repair candidate is
`1.3.2-reedme.1`. The dated review remains a record of that baseline, rather
than a statement about the repaired candidate.

## Repository and release ownership

Repository: <https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod>.
Updates use this fork's releases. Critical paths have `@ReedME` as CODEOWNER.
Branch protection is configured for `dev`, `acceptance` and `main`; changes
still move feature → dev → acceptance → main with the existing CI gates.

The `release` environment has a ReedME reviewer and accepts only `v*` tags.
Its `TPF3MP_UPDATE_SIGNING_KEY` secret and the repository's
`TPF3MP_UPDATE_PUBLIC_KEY` variable are configured. The private signing key
stays outside Git with owner-only permissions. `tools/github/setup-update-key.sh`
is the Linux setup helper; the PowerShell helper remains available on Windows.
Do not regenerate an existing key casually: installed launchers pin its public
key. Environment setup alone does not create a verified release.

## Local server first

The development fallback is `127.0.0.1:29470`, labelled Local. Specify the
server and its pinned development certificate when testing. A development
build does not contact the upstream relay automatically.

```bash
cargo build --locked -p tpf3mp-server -p tpf3mp-launcher -p tpf3mp-agent -p tpf3mp-hook -p tpf3mp-testkit
mkdir -p target/reedme-local
target/debug/tpf3mp-server --listen 127.0.0.1:29470 \
  --dev-self-signed target/reedme-local/server.der \
  --secret-file target/reedme-local/server.key \
  --data-dir target/reedme-local/server-data \
  --admin-listen 127.0.0.1:29471
```

Start the Windows launcher in the game's Proton prefix with
`--server 127.0.0.1:29470 --pin-cert <Windows-path-to-server.der>` and
`--game-exe <Windows-path-to-TransportFever3.exe>`. A second machine needs a
reachable explicit address and the same pinned certificate. Keep the server's
secret file and data directory together for same-room recovery. Protocol 20,
bridge 29 and persisted format 9 require matching candidate binaries; protocol
19 clients cannot join this server. Old format-8 room logs must remain backed
up and be handled by the format gate rather than silently reinterpreted.

The deployment default is `ghcr.io/reedme/tpf3mp-server`. A manually published
feature image uses `branch-codex-proton-competitive-fixes`; select it through
`TPF3MP_VERSION` in `deploy/compose.yaml`. A feature image must not replace
`latest`. Local builds can also build `deploy/Dockerfile` directly.
The initial review branch triggers its own acceptance and branch-image runs;
GitHub had not registered the fresh fork's manual workflows. Release branches
and packaging gates are unchanged.

Before publishing player packages, choose and operate the actual public server.
Set the `TPF3MP_DEFAULT_SERVER`/`TPF3MP_SERVERS` repository variables required
by `release.yml`; use a public TLS certificate or the documented release trust
configuration. Loopback is a local test target, not a public release endpoint.
Server setup is still local until an operated server/domain is supplied.

## Proton installation and game acceptance

Use the **portable Windows ZIP**, extracting its launcher, agent, hook and mod
together. Run its `TPF3-MP.exe` in the **same prefix and Steam environment** as
the Windows game. Portable installation, sync, repair and removal copy and
verify the mod in Rust; no functional PowerShell is required. The managed
standalone installer intentionally refuses Wine before installation changes,
because its shortcut/registry/self-removal path requires native Windows
PowerShell. A successful script exit alone no longer proves installation.

Maintenance refuses a running game or a failed process query. It validates
both installed bytes and the recorded staging path, preserves existing mod
files through backups, and rolls back failed postconditions. Do not copy a
proxy DLL or patch the game directory. See [PLAYING.md](PLAYING.md) for the
player flow.

The selected native bundle is `tf3_build40420_steam_windows`, with executable
SHA256 `74861ac43b041aebc5179154345b3cf1ec83154c8e6cc58e0d9e02ff5fa602e4`.
Steam depot BuildID and internal game build number are separate identifiers.
The installation inspected on this machine currently contains an ELF Linux
executable. A matching Windows `TransportFever3.exe` and its Proton prefix are
still required for game acceptance; no Steam settings or installed game files
have been changed by this repair work.

After source and synthetic checks pass, use a temporary local server and a
disposable world. Verify installation output, launcher injection and Steam
sign-in, then two companies at 1x, pause/resume and 4x. Cover debt-free time
before borrowing, staggered loans, displayed installment/payoff, automatic
saves, late join, reconnect, replacement-world reload and server restart.
Include a paused saved loan and infinite-money setting in finance acceptance.
Record every scheduled checkpoint, including missing samples, and inspect
game/hook/server logs. Synthetic or profile verification cannot substitute for
these runtime checks.

## Saved loans and company passwords

Existing annuity loans migrate once to the native accounting model, retaining
their outstanding principal. Migration books no money and charges no elapsed
months. Future payments use the remaining term and the native full-term
interest model; original fields are retained as legacy metadata. This is a
deliberate change in future repayment terms for saves from the old model.

Company password seals are scoped to a room and server key. Same-room reload,
reconnect and server restoration preserve them. Rehosting a save into a new
room clears unusable old seals and shows a reset notice, preserving companies,
assets and memberships. Old locked saves that predate room provenance cannot
prove same-room identity, so their seals clear once on first upgraded room
load with a distinct legacy notice. Unlocked legacy saves simply record the
current room identity. Set new passwords after that reset.

## Private exact-build verification

The verifier now runs on Linux or Windows and does not require a private
`quiet-cargo` wrapper. The archive must contain the complete genuine supported
Windows build, outside the checkout. Do not commit or upload game binaries.

1. Register a dedicated private self-hosted runner in this fork. Install Git,
   pinned Rust, Python 3.12+ and bash. The default labels remain
   `["self-hosted","Windows","X64","tpf3mp-game-builds"]`; set
   `TPF3MP_GAME_RUNNER_LABELS` to a JSON label array for a Linux runner.
2. Set `TPF3MP_GAME_ARCHIVE` to the complete 40408 archive and
   `TPF3MP_UPDATED_GAME_ARCHIVE` to the complete selected 40420 archive on that
   runner. The workflow preserves the older release's verification mapping.
3. Run `release.yml` manually with `verify_only=true` on the candidate branch.
   The portable helper builds `tpfre` into an external cache, runs live
   `verify-build`, and requires a report for the exact clean commit, selected
   bundle and target set. Optional candidate signature checks remain separate.

Local equivalent after obtaining the private archives:

```bash
TPF3MP_GAME_ARCHIVE=/private/40408 \
TPF3MP_UPDATED_GAME_ARCHIVE=/private/40420 \
python tools/github/verify_game_build.py --repo . --cache /private/tpfre-cache
```

These are example paths. No archive or private runner is configured yet.
Missing inputs stop packaging. Do not disable this gate or use `fast_track` to
make an unverified native release. Reports explicitly leave
`runtime_verified=false`; native ABI and real Proton acceptance remain required.

The native ruleset orders trusted clients' commands. It does not independently
validate a cheat-resistant company economy. The initial competitive scope
therefore remains private games among trusted players.
