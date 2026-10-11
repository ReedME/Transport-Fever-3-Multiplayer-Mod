# ReedME review implementation and validation — 2026-10-11

All eight findings from the dated static review have repairs and regressions.
The local source checks, optimized multiplayer model and Windows fake-game
checks under Proton pass. This does not establish compatibility with the real
Transport Fever 3 executable or approve a player release.

The baseline remains upstream v1.3.1 at
`538733611cc5e4c8ea30d48513d5b0df761db05a`. The candidate is
`1.3.2-reedme.1` on `codex/proton-competitive-fixes` in the public ReedME fork.
The earlier review/feasibility receipts describe the original inspection;
this document and `reedme-implementation-validation-2026-10-11.json` describe
the repair work.

## Findings closed in code

| Finding | Result | Focused evidence |
|---|---|---|
| R01: false installation success under Proton | Portable Windows ZIP installs, syncs, repairs and removes the mod in Rust. Verify the installed tree and record. Running-game/query failures refuse maintenance; failed postconditions roll back. Managed EXE setup refuses Wine clearly. Native Windows scripts retain output verification. | Launcher installer fixtures: 8 on Linux, 7 applicable fixtures under Proton; setup fixtures. |
| R02: old-world reports attributed to replacement | Server-issued epochs identify the producing world; an offer stays separate from acknowledged load. Checkpoint/save/progress reports require the current acknowledged epoch. Delayed ingest is rejected; exact resume replays the original load acknowledgement; save filenames include the epoch. | Delayed-ingest barrier, acknowledgement replay, stale report gates and filename isolation fixtures; late-join/rebase/restart integration scenarios. |
| R03: loans billed before borrowing | Each loan has its own payment timestamp. Debt-free time does not accrue retroactive installments; a delayed update pays at most once, matching native scheduling. | Debt-free gaps, staggered/boundary origins and borrowing during another due update. |
| R04: displayed finance differs from accounting | Founded-company loans use native fixed principal and full-term interest, including final rounding corrections. Finance offers, installment and payoff use that model. | Native facade, payoff, migration and final-rounding fixtures. Native display can round the combined amount differently from separately rounded journal entries. |
| R05: secondary Linux Steam libraries missed | Windows client converts Linux VDF library paths to Wine drive paths before checking Windows absolute-path rules; Windows and UNC paths retain their semantics. | Linux-authored VDF with spaces, legacy entries, native Windows paths and relative-path refusal. |
| R06: stale cleanup deletes retained worlds | Cleanup reads the live retention set. Counted pending guards protect same-ID operations; active transfers and ingest are held. Stale results, failed sends and bridge destruction release their own holds. Long ingest/GC does not hold the bridge publication lock. | Delayed cleanup, pending same-ID results, active transfer/ingest and closed-channel/receiver-drop fixtures. |
| R07: unordered snapshot pointers | Per-room serialized commits ignore obsolete pointer jobs and keep recovery holds through failed writes. Successful Unix pointer replacement also syncs the parent directory. | Reversed-job barrier with failed/latest persistence and pointer round trip; server persistence/snapshot integrations. |
| R08: failed paused saves never retried | Skip decisions compare the last agreed frontier. Attempt time controls retry backoff, so demanded failed saves retry without world movement. | Paused demanded-save failure/backoff/success fixture. |

Independent peers reviewed finance/native-source parity and the final
generation, pointer and hold lifecycle. Review followups also closed
same-snapshot count loss, lost load acknowledgements, event-number filename
reuse and abandoned-completion retention.

Cross-platform validation exposed another replacement-fetch race: aborting an
async download does not cancel blocking snapshot I/O, which can still own the
exclusive transfer claim. A same-snapshot replacement now waits asynchronously
within the existing idle deadline for that claim to release. Resuming directly
also removes the pending-list/read race; if the old worker removed that record,
the validated offered manifest can start the replacement. Other errors still
propagate. Deterministic QUIC fixtures observe the actual contention before
releasing the old owner, verify exact received bytes and independent snapshot
progress, and bound a busy-owner timeout. Restoring the old opener in a scratch
checkout makes the overlap regression fail. The restored-room fixture waits for an actual completed load
and a rebound server before its next player joins; its exact load-count and
world-equality assertions remain.

## Persistence and competitive policies

Loan migration retains outstanding principal and existing journals, books no
money, and starts the remaining schedule at the current game time. Future
terms change from the old annuity model to the native full-term-interest
model; original fields remain as legacy metadata. Test paused saved loans and
infinite-money settings in the real game.

Stable RoomId now reaches the Lua roster. Same-room reconnect/reload/recovery
preserves password seals. Rehosting in another room clears unusable seals with
a persistent notice while preserving memberships, assets and station access.
Locked legacy saves with unknown provenance reset once with distinct wording.
Unknown/display/invite identifiers do not trigger resets.

Three cyclic lane-majority reports can yield no complete matching replica.
A dedicated regression proves such saves promote no snapshot. The server
warns explicitly and retains the last agreed world. Continued retries can
remain unresolved; the owner/operator must coordinate recovery. There is no
automatic rollback of the room log or arbitrary-replica selection.

The repair uses protocol **20**, bridge **29** and room-log format **9**.
Protocol-19 clients cannot connect. Old format-8 room data is refused and
preserved before recovery rewrites or snapshot cleanup; a startup preflight
checks every room log first and refuses unidentified/damaged first records too.
The CLI checks before writing a certificate or invite key. Byte-preservation
fixtures cover mixed versions, quarantined logs, compaction files and damaged
headers; an actual CLI refusal fixture covers identity files. No migration is
implemented. Keep old binaries, secret, logs and
snapshots together and use fresh fork rooms/data directories. This candidate
has not passed a migration of running upstream sessions.

## Validation completed locally

- `cargo fmt --all -- --check` and `git diff --check`.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` on Linux
  and the Windows GNU target.
- `cargo test --workspace --locked`: **1,735 passed, 0 failed, 9 ignored**.
  The ignored environment/hardware tests are not counted as passes. Broad
  testing exposed an existing parallel-test race over the hook's simulation
  seed; shared test-only guards fix it without changing production behavior
  or weakening the assertion. Full hook suite: 439 passed, 3 ignored.
- Optimized launcher, agent, server, hook and testkit builds. Server/agent
  version checks report `1.3.2-reedme.1`.
- `tpfre` format, clippy and tests: **49 passed, 1 ignored**. The separate
  lockfile records the fork's hookcore version without dependency upgrades.
- Python release-day/lobby/lane/probe/announcement/verification helpers,
  signing setup refusal regression, Unix installer tests and launcher-page
  JavaScript syntax checks.
- Optimized multiplayer regressions: **39/39 runs**, three repetitions,
  at least three replicas, speed 800, two rooms at once.
- Paced 10-room/80-bot load, bad-network latency/jitter/2% loss, WebSocket
  tunnel load and logged compaction load. Each requires no failed bot/room or
  divergent replica; the compaction check requires actual log compactions.
- Windows fake-game rig through Proton Experimental
  `experimental-11.0-20261001-x86_64` in a disposable prefix: two players,
  **1,200 steps each**, matching all three final lane digests, zero
  divergences. Both loaded the room world. The existing plain-UDP fallback
  handled Wine's unsupported socket options. Seven Windows installer
  fixtures also passed in that prefix.
- Windows Steam discovery fixtures under Proton: **11 passed, 1 ignored**.
- The two late-join paths and barrier-based restored-world scenario pass
  under Proton on the final Windows candidate.
- Native Linux server with Windows clients under Proton: two fake players,
  1,200 steps each, matching final lanes and zero divergences.

Additional Windows-server scenarios under Wine exposed an unresolved open-log
compaction failure (`Access denied` during replacement). The initial supported
topology uses a native Linux server with Windows clients under Proton. Native
Windows server CI is separate from running that server inside Wine.

These tests exercise model worlds and Lua API stand-ins. They cannot prove
native injection, real engine determinism, gameplay presentation or long
competitive sessions. Crosscompiled Windows development binaries omit native
Windows version resources; hosted Windows CI remains a separate build gate.
Windows pointer replacement has the platform's existing crash-durability
limits; Unix parent-directory sync is covered by the implementation.

## Fork readiness and remaining gates

The public fork, CODEOWNER and repository/update defaults belong to ReedME.
Branch protections are configured. The protected `release` environment has
the ReedME reviewer, `v*` tag policy and signing secret; the public update key
is configured separately. Private signing material stays outside Git. The
Linux signing helper refuses existing repository trust before creating files.
The fork's issue tracker is enabled; no upstream service was configured.

Development uses loopback with an explicit pinned certificate; public server
variables remain unset until an operated server is chosen. Deployment pulls
`ghcr.io/reedme/tpf3mp-server`, with feature images kept separate from `latest`.
The portable private verifier accepts Windows or Linux runners and retains
exact archive/selected-bundle/clean-commit gates. No private runner or genuine
supported Windows archive has been configured, so native release verification
and player packaging remain blocked.

The local inspected game is an ELF executable. A matching Windows 40420
`TransportFever3.exe` and its real Proton prefix are still needed. The
required executable SHA256 is
`74861ac43b041aebc5179154345b3cf1ec83154c8e6cc58e0d9e02ff5fa602e4`.
No installed game files or Steam configuration were changed. No signed player
release or public game server was published. Cross-platform CI/acceptance and
feature-image publication are tracked in the validation receipt; the actual
two-player game acceptance checklist is in [REEDME_SETUP.md](../docs/REEDME_SETUP.md).

The native server sequences trusted players' commands. This repair does not
implement an independently authoritative, cheat-resistant company economy.
