# ReedME TPF3-MP review — 11 October 2026

TPF3-MP is a credible base for competitive play between friends, with separate companies sharing one world. It already implements company identities, action ownership, ordered command replay, late joining and snapshot recovery. The immediate work is to make its Windows packaging behave correctly under Proton and close specific finance and recovery defects. Public competitive play with an authoritative, cheat-resistant economy remains a larger project.

The public fork is [ReedME/Transport-Fever-3-Multiplayer-Mod](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod). This review is pinned to **v1.3.1**, commit **538733611cc5e4c8ea30d48513d5b0df761db05a**, which was upstream `main` and `dev` when fetched. The local review branch is `codex/deep-review`, based on `origin/dev`. `origin` is ReedME; `upstream` is Juliansgith. MIT licensing and existing contributor credit are preserved.

**No builds, automated tests, workflow dispatches, game launches or runtime probes were performed for this review.** Findings below are established from source and read-only local evidence. Concurrency findings describe admissible code interleavings; they have not been reproduced. No gameplay code has been changed. The review and baseline receipt are local artifacts and have not been pushed, which would start the fork's CI.

## What was reviewed

| Area | Main source examined | Assessment |
|---|---|---|
| Windows/Proton launch | Agent Steam discovery, launcher setup, Windows installer, process injection and native build gates | Windows executable and DLL must run together inside the game's Proton environment. Setup currently assumes real PowerShell. |
| Simulation | Hook step driver, new cadence/interval modules, bridge replay and step release | Substantial implementation, but actual game scheduling and determinism still require runtime evidence. |
| Competitive companies | Lua company roster, player following, permission routing, loans, subsidies, shared infrastructure and representative fixtures | Core identity and ownership exist. Loan correctness needs attention. |
| Synchronization/recovery | Protocol, server room actor, checkpoints, saves, snapshot pointers, agent background work and storage | Strong separation of ordered events and world transfer; several asynchronous operations lack generation or ordering guards. |
| Releases and fork operations | CI/acceptance/release/signing workflows, updater, container deployment and owner setup helpers | Good signed-update design. The fork has no independent release configuration yet. |

The review used `AGENTS.md`, `docs/PLAN.md`, `docs/DECISIONS.md` and `docs/GAME_TESTING.md` as context. Some design documents predate release and describe future canonical authority. Current executable paths and the D6 native-economy amendment take precedence when assessing implemented behavior.

## Current architecture and competitive boundary

The Windows launcher starts the game suspended and injects the hook into that process. Lua captures supported actions and sends them through shared-memory messages to the agent. The server authenticates players and orders actions into sealed turns; each game applies the same actions at controlled simulation steps. Checkpoints compare world lanes. Shared saves provide late join and rebase, with all affected replicas reloading the same snapshot to keep entity numbering aligned.

Competitive identities are more than a visual company selector: room player IDs map to a persisted roster of native company entities. Commands use their authenticated origin and company membership; native GUI queries follow the selected company. Ownership checks are implemented for company assets, lines and vehicle actions. Shared station access has its own policy. Founder/head, member and spectator permissions have explicit paths. These paths are covered by stand-in API fixtures, which do not establish how every native GUI tool behaves in the real game.

The default `native` ruleset is `AcceptAll`: [ruleset.rs:46](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/blob/538733611cc5e4c8ea30d48513d5b0df761db05a/crates/tpf3mp-server/src/ruleset.rs#L46). It sequences intents but does not calculate or validate an authoritative company economy. Client-side replay checks remain valuable, but a synchronized result is not proof that the server independently checked its legitimacy. D6 explicitly adopts this tradeoff. The early canonical design in `ARCHITECTURE.md` should not be read as a completed public-match anti-cheat system.

For this fork, the first target should therefore be **trusted players competing with separate companies on identical Windows game builds under Proton**. Native Linux game support is not required for that target. A native Linux server is compatible with it because it runs the sequencer, rather than the Windows simulation.

Two further policy boundaries need an explicit acceptance decision. Company password seals include the room ID and server key, while the roster persists them in the save. Rehosting that save in a new room cannot validate the old password without a reset/rekey policy; existing members and a returning head have separate paths. With three or more divergent games, per-lane voting can also select a combination no complete replica holds, leaving no matching snapshot candidate. These are open persistence/recovery boundaries, rather than extra reproduced defects. Broader D22 head/password rules and D23 progression also remain marked proposed in the decision log despite related implementation.

## Prioritized findings

P1 means a launch blocker or a serious correctness defect to address before relying on a session. P2 means a narrower defect that needs a focused regression and repair. These priorities apply to our Proton/competitive target, not every upstream installation.

### R01 · P1 · Proton setup can report success without installing the mod

**Trigger:** run the Windows distributable or extracted Windows release package in an ordinary Proton prefix without a functional Windows PowerShell installation.

[installation.rs:234](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/blob/538733611cc5e4c8ea30d48513d5b0df761db05a/crates/tpf3mp-launcher/src/installation.rs#L234) invokes `powershell.exe -File`. `run_script` accepts a zero exit code, and `perform` then treats the mod/shortcut installation as complete. The bundled Proton Experimental PowerShell binary is a Wine stub. [Valve's implementation](https://raw.githubusercontent.com/ValveSoftware/wine/bleeding-edge/programs/powershell/main.c) logs arguments and returns zero without executing file scripts. Read-only binary inspection found matching diagnostic strings in the installed PE file; its version and digest are recorded in the baseline receipt.

**Effect:** setup can display completion while the mod and installation record are absent. PowerShell-based game-closed checks and maintenance also cannot be trusted through that stub. Extracting the release ZIP alone does not avoid the setup gate. A non-distributable development binary without the package marker does bypass it, but manual installation and lifecycle checks would still be needed.

**Repair:** implement the installation and game-closed checks in the Rust launcher, or provide a deliberate Proton installation path. Confirm installed files and the installation record before showing success. Do not solve this by assuming the stub's exit code means the script ran.

**Regression:** a process returning zero without producing the expected installation must fail setup. Later, validate first install, repeat launch, repair and uninstall inside the actual game prefix.

### R02 · P1 · Reports from a replaced world can be attributed to the new snapshot

**Trigger:** a checkpoint or background save-ingest report from world A arrives after the server has offered replacement world B on the same connection.

[room.rs:2945](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/blob/538733611cc5e4c8ea30d48513d5b0df761db05a/crates/tpf3mp-server/src/room.rs#L2945) sets `member.loaded` when the turn feed is enqueued, before fetching/loading completes. Checkpoint and save handlers stamp incoming reports with that current field. [GameMessage](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/blob/538733611cc5e4c8ea30d48513d5b0df761db05a/crates/tpf3mp-proto/src/control.rs#L650) carries step/event and lane digests, but no producing-world generation.

The agent makes this concrete for saves: [bridge.rs:1357](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/blob/538733611cc5e4c8ea30d48513d5b0df761db05a/crates/tpf3mp-agent/src/bridge.rs#L1357) starts background ingestion without a load-generation tag; completion at line 1672 always calls `report_saved`. `void_world` does not invalidate that result. Fetch and load acknowledgements already have attempt/generation guards. A delayed report for an open save event after the replacement snapshot's event can pass the server's stream-frontier check.

**Effect:** old-world lane digests can enter new-world rounds, potentially creating a false divergence verdict and another unnecessary rebase. The existing `a_world_handed_out_forgets_what_older_worlds_reported_past_it` regression clears reports already stored when the world is handed out; it does not reject a delayed old report arriving afterward.

**Repair:** carry a producing-world generation through checkpoint/save messages and background ingestion, reject stale results, and distinguish offered from acknowledged/loaded worlds on the server. A wire change requires coordinated protocol/version handling.

**Regression:** delay a save-ingest completion and checkpoint until after replacement loading begins, then verify they cannot affect the replacement's verdict or upload candidates.

### R03 · P1 · A new loan can be charged for months before it existed

**Trigger:** all founded-company loans are repaid, several months pass debt-free, then a company borrows again.

[companies.lua:1051](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/blob/538733611cc5e4c8ea30d48513d5b0df761db05a/mod/tpf3mp_1/content/scripts/tpf3mp/companies.lua#L1051) only initializes `roster.month` if absent. `due` at line 1107 requires outstanding loans, so the clock stops advancing when the roster has none. `chargeMonths` at line 1079 subsequently loops over the entire elapsed interval using the newly created loan. The simulation's monthly update calls that function.

**Effect:** repay the final loan in month 3, borrow in month 10, and the new loan receives seven payment periods on the next update. This debits interest/principal for time before the loan existed and can consume much of its term immediately.

**Repair:** record each loan's start/last-charged month, or advance the accounting clock even while debt-free and settle existing loans before inserting a new one. Cover the month-boundary ordering as well as debt-free gaps.

**Regression:** borrow → charge → repay all → wait seven months → borrow; assert no charge before the new loan's first due month. Also cover two companies borrowing on different dates.

### R04 · P2 · Native finance displays a different installment and payoff

Founded-company loans use an annuity with declining principal at [companies.lua:1038](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/blob/538733611cc5e4c8ea30d48513d5b0df761db05a/mod/tpf3mp_1/content/scripts/tpf3mp/companies.lua#L1038). The native loan GUI instead uses the shipped `loan_util.tl` fixed-principal/fixed-interest calculation. [loanTable:996](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/blob/538733611cc5e4c8ea30d48513d5b0df761db05a/mod/tpf3mp_1/content/scripts/tpf3mp/companies.lua#L996) exposes original amount, duration, percentage and paid count to that GUI, without the actual annuity payment or remaining liability.

The active path is the GUI `follow` wrapper for the founded company's loan-script `GAME_SCRIPT` component. The existing fixture makes the mismatch explicit: a 1,200 loan after one period has actual remaining principal 1,105 and payment 107, but the facade returns `1200:12000:0.12:1`. The native GUI computes payoff 1,100 and monthly payment 112 from those fields. Its repayment eligibility also uses the native payoff, so a balance of 1,102 can enable Repay while the action debits 1,105.

**Repair:** use the native calculation consistently for native-economy rooms, or explicitly adapt all loan display and eligibility functions to the custom model. Choosing a different model alone does not fix the incorrect display. The first company's native loans and founded companies also need equal treatment for fair competition.

**Regression:** compare the displayed payment, payoff, repayment eligibility and actual booked amounts using the shipped formula. Current fixtures assert the facade fields, not their native GUI interpretation.

### R05 · P2 · Proton misses Windows games in secondary Steam libraries

[steam.rs:149](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/blob/538733611cc5e4c8ea30d48513d5b0df761db05a/crates/tpf3mp-agent/src/steam.rs#L149) converts VDF library paths straight to Windows `PathBuf` and requires `is_absolute()`. Linux Steam records Unix paths such as `/mnt/games/SteamLibrary`; on Windows these have no drive prefix and fail that check. The existing `wine_path` conversion is used for `STEAM_COMPAT_CLIENT_INSTALL_PATH`, but not each listed library.

**Effect:** default-library discovery can work while a secondary-library Windows game is missed. `--game-exe` is a workaround for the first controlled test, not a complete automatic-discovery fix.

**Repair/regression:** convert Unix VDF paths to Wine's mapped drive form under the Proton path, retaining normal Windows/UNC handling. Test Linux-authored VDF content with Windows path semantics; the current host-native fixtures do not cover it.

### R06 · P2 · Stale cleanup jobs can delete a newly retained world

[bridge.rs:2093](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/blob/538733611cc5e4c8ea30d48513d5b0df761db05a/crates/tpf3mp-agent/src/bridge.rs#L2093) captures a keep-list and independently schedules `keep_only` on the blocking pool. [transfer.rs:98](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/blob/538733611cc5e4c8ea30d48513d5b0df761db05a/crates/tpf3mp-agent/src/transfer.rs#L98) releases every currently retained snapshot absent from that captured list, deletes received files absent from it, and runs garbage collection.

**Interleaving:** cleanup A captures the old set; a fetch/ingest retains B; cleanup A runs late and releases/deletes B. Cleanup B running earlier does not prevent that. Store GC excludes simultaneous ingest/assembly while collecting, but it cannot repair a stale decision to remove a live retention root.

**Effect:** a just-fetched or saved world can become unavailable for loading/uploading. **Repair:** serialize cleanup with retention changes or validate a retention generation and protect in-flight worlds. Use a barrier-controlled regression for the delayed cleanup; timing-based sleeps would be unreliable.

### R07 · P2 · Snapshot pointer writes are unordered and share a temporary path

[room.rs:2819](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/blob/538733611cc5e4c8ea30d48513d5b0df761db05a/crates/tpf3mp-server/src/room.rs#L2819) independently schedules each promoted snapshot pointer write. [snapshots.rs:413](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/blob/538733611cc5e4c8ea30d48513d5b0df761db05a/crates/tpf3mp-server/src/snapshots.rs#L413) uses the same `<room>.snapshot.partial` file with truncating `File::create`, followed by rename.

**Interleaving:** an older promotion's job runs after the newer one and overwrites the pointer, or simultaneous jobs collide at the shared temporary file. After more promotions, the older pointer may refer to a snapshot whose retention has been released. Restart recovery can then miss the latest usable world.

**Repair:** serialize persistence per room and preserve promotion order; use unique temporary paths and a monotonic generation check. Unique names alone do not prevent an older pointer replacing a newer one. Regression: complete two pointer jobs in reversed order and check the recovered save frontier.

### R08 · P2 · A failed save in an unchanged paused room may never retry

[save_if_due:3080](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/blob/538733611cc5e4c8ea30d48513d5b0df761db05a/crates/tpf3mp-server/src/room.rs#L3080) records the attempted save point before success. [Saves::due:313](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/blob/538733611cc5e4c8ea30d48513d5b0df761db05a/crates/tpf3mp-server/src/snapshots.rs#L313) refuses another save if the room already has a current snapshot and no steps/events have advanced beyond that attempt. The unsuccessful candidate path does not reset the attempted point.

**Trigger/effect:** an existing room is paused, a fresher save is required for a waiting member/rebase, and every save/upload candidate fails. When the attempt closes, `wanted` can remain true while the unchanged-point check suppresses retries, even after the retry delay. The member waits until the room advances or receives another action.

**Repair/regression:** distinguish attempted and successfully agreed save points, with bounded retries for demanded saves. Exercise all-candidate failure while paused with an older current snapshot present.

## Fork readiness

GitHub read-only inspection confirmed the fork is public and has no Actions runs, repository variables, repository secrets or self-hosted runners. `dev` was unprotected. That is normal for a new fork, but copying source does not copy upstream release infrastructure.

Before distributing ReedME builds:

1. Configure ReedME branch protections and ownership. `.github/CODEOWNERS` and the branch-protection/key-setup helper defaults still name Juliansgith; invoke setup helpers with the fork explicitly and adapt governance separately while retaining provenance.
2. Set our server address/list deliberately. Development builds otherwise fall back to upstream's relay. For local investigations use an explicit loopback server; do not accidentally send test rooms or diagnostic uploads to the upstream service.
3. Use our own update trust key and protected signing environment. The release workflow already sets `TPF3MP_REPOSITORY` to `github.repository`; ad-hoc builds fall back to upstream when it is omitted. Do not combine a fork update URL with an unrelated signing trust root.
4. Configure the private game-build verifier. The release workflow expects `TPF3MP_GAME_ARCHIVE`, a labeled Windows runner and the `quiet-cargo` helper outside the checkout. Those are absent here. Preserve verification rather than treating `fast_track` as the default remedy.
5. Republish/repoint server images deliberately. `deploy/compose.yaml` currently references `ghcr.io/juliansgith/tpf3mp-server`; an unqualified pull would still run upstream's image. Protocol and client/server versions must match.

These are onboarding tasks, separate from the code findings. No keys were generated, no secrets were read, no protections were changed and no service was deployed during the review.

## Evidence limits and the first test gate

Read-only GitHub inspection found successful upstream CI/build and private static game-build verification checks for the reviewed SHA. These are upstream results, not tests run by us. `acceptance.yml` uses deterministic stand-in games, bot rooms and synthetic network conditions; it does not load Transport Fever 3. Native profile verification also explicitly reports static compatibility rather than runtime verification.

The [v1.3.1 release notes](https://github.com/Juliansgith/Transport-Fever-3-Multiplayer-Mod/releases/tag/v1.3.1) describe a short Windows 40420 two-player run at 4x, with common world samples agreeing. They also disclose skipped scheduled samples, a missing final summary, an earlier save-adjacent guest hang, and a separate industry-spawn divergence. Automatic room saves were not exercised in that short run. This is useful evidence, but leaves recovery and sustained determinism open. The cadence/interval source review did not establish a separate confirmed pacing defect; those native memory and timing paths need runtime verification.

The local Steam installation inspected earlier contains a native Linux game executable, not `TransportFever3.exe`. This does not settle how the group's other machines are configured. Our first Proton run needs a confirmed **Windows Steam game build 40420**, the matching hook/profile and protocol 19 server. Steam's depot BuildID and the game's internal build number are different identifiers. No Steam settings or game installation were changed.

The compiled Windows 40420 profile expects executable SHA256 `74861ac43b041aebc5179154345b3cf1ec83154c8e6cc58e0d9e02ff5fa602e4`. Check that identity before a run. Injector address assumptions and the interval controller's concurrent engine-buffer lifecycle remain unverified under our Proton environment, even when pure logic fixtures pass.

Proposed sequence after the review:

1. Repair R01 and add an installation-output check; repair R05 or explicitly select the Windows game executable for the first controlled run. Confirm launcher and game share the same Proton prefix and Steam environment.
2. Add deterministic regressions for R02/R03/R06/R07 before relying on recovery or long competitive sessions. Reconcile R04 so both companies see and pay the same native loan terms; cover R08 alongside save failures.
3. Run the ordinary source checks and fake-room regressions on the exact candidate commit. Only then move to a temporary local server and disposable fixture save.
4. Start two players with matching game, hook and mod identities. Found two separate companies; verify independent balances, loans, asset ownership, station access and rejected cross-company actions through normal UI.
5. Test 1x first, then pause/resume and 4x. Cover automatic saves, late join, guest reconnect, world replacement and server restart. Record every scheduled checkpoint, including missing samples, rather than comparing only their intersection.

Stop a runtime test if the version gate fails, installation output is absent, the hook refuses a native profile, a command escapes capture, or replicas disagree. Keep the fixture and logs for diagnosis. A successful session should establish equal company accounting and recovery as well as visible shared vehicle movement.

The review is complete as a static starting audit. The fork is ready for focused repair work; it is not yet validated for the requested Proton competitive session.
