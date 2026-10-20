# ReedME upstream refresh — 2026-10-20

The development candidate `1.3.2-reedme.2` integrates upstream TPF3-MP dev
`02829e3608011351789b6ca0117093177f64b201` into the repaired ReedME fork.
The input fork commit is `9261e8eafd4cec315c2c91d817533ceabefc3b28`;
the shared source baseline is `538733611cc5e4c8ea30d48513d5b0df761db05a`.
Upstream main/acceptance and its latest published release remain at v1.3.1;
this refresh imports development work, not a newer published player release.

## Changes integrated

The upstream range contains 191 commits, including 21 first-parent merges.
It adds native rolling world-read and scan-plan work, incident tracing,
bridge/tunnel and signal handling, waypoints and terrain/tree/line work,
height-aware edge resolution, switch/bulldozer and refit corrections, crossing
and tool-continuation infrastructure, native-mod room terms and registry UI,
and the external-plugin/Big Maps framework. Experimental tools and scan modes
retain their acceptance gates. Big Maps is not enabled by this refresh.

Our eight original repairs remain: verified portable Proton installation,
Linux Steam-library paths, native loan timing/accounting, producing-world
epochs and acknowledged loads, counted snapshot retention, ordered durable
pointers and retrying demanded failed saves. The later exclusive-transfer
retry and preflight-before-any-mutation protections remain too.

The combined bridge carries **both** stable room identity/world epochs and
the agreed scan plan. Initial, late and restored seats propagate that plan;
regressions exercise non-default plans and preserve the exact load-count and
world-equality checks. A launcher UI fixture now scrolls an executable-mod
Install button into view before physical clicks, while retaining explicit
supported/enabled and unsupported/disabled assertions.

The full scenario suite exposed a duplicate world offer while another member
joined before a prior load acknowledgement arrived. The server now preserves
that pending offer and epoch on the same live stream, while still resending
on replacement connections and changed worlds/frontiers. A deterministic
queue-level regression checks those paths and stale acknowledgements; removing
the new guard makes it fail immediately. Load-count assertions remain intact.

A loan fixture assumed two randomly drawn company cooldowns expired together.
It now drives opposite cooldown bounds explicitly and checks the exact
boundary, first-company-only refresh and eventual second-company refresh.
Forged-offer, native-offer preservation, single-booking and company-isolation
assertions remain. Production finance code was unchanged by this correction.

## Wire, persistence and governance

The candidate uses protocol **24**, IPC bridge **30**, action schema **28**,
Lua contract **15**, and room-log format **11**. These numbers distinguish
independent changes: the previous fork and upstream both used bridge 29 and
format 9 for different layouts. Earlier clients cannot join. Formats **8, 9
and 10 are refused before certificate/key writes, store opening, recovery,
rewrite or cleanup**. The ambiguous legacy readers were removed. Use fresh
room data and snapshot directories, preserving complete old directories and
matching binaries; no migration or live-session upgrade is claimed.

ReedME remains the owner and CODEOWNER. Existing D30 is preserved. D31 records
the owner's explicit approval of the external-plugin model: validated project
signatures or an authenticated deliberately configured server index, full
canonical package identity and explicit executable consent before payload
fetch/install/enable, with dependency and game-start checks. Consent source
means the project or server authority, not a specific certificate, mirror or
key fingerprint. The native-mod key is separate from the updater key; no
registry, native signing key, package source or plugin installation was
configured by this refresh. Fork defaults remain ReedME and loopback.

The upstream double-slip exception is archived as **U-D30**, not adopted as a
fork decision. Ordinary rooms retain `strict_junctions=false` and
`double_slip_only=false`. Imported exception vectors opt in only in explicit
disposable acceptance copies. Original upstream commits and co-author trailers
are preserved by a merge rather than rewritten or reattributed.

## Game and MPFever comparison

The [official TF3 notes](https://wiki.transportfever3.com/doku.php?id=releasenotes)
list stable **40420** and preview **40422**. Upstream still selects the exact
Windows Steam 40420 profile, executable SHA256
`74861ac43b041aebc5179154345b3cf1ec83154c8e6cc58e0d9e02ff5fa602e4`.
No verified 40422 profile is present; the candidate does not claim support for
it. No offsets were guessed or game-build gates bypassed.

[MPFever 0.3.20](https://github.com/dkdknight/MPFever/releases/tag/0.3.20)
adds missing-mod checks and Windows Unicode-account-path fixes. Its separate
host/proxy architecture is not the base chosen for this fork, and its release
still documents separate-company and refund/accounting limits. It was reviewed
as a reference; no MPFever source or game-folder proxy was imported.

## Validation and next playtest

Local results and source/binary identities are recorded in
`reedme-upstream-refresh-validation-2026-10-20.json`. Hosted results for the
exact pushed revision are linked on [PR #1](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/pull/1).
The original 11 October receipts remain historical evidence for their revision.

The final local workspace run passes **1,966 tests, 0 failed, 10 ignored**.
Formatting and Linux plus Windows GNU workspace/all-target Clippy pass with
warnings denied. Ignored/native archive checks and model-only fixtures do not
establish real-game acceptance.
The fork delta passes whitespace checks against upstream dev. The unchanged
upstream Big Maps game-UI copy retains 14 existing trailing-whitespace lines;
the complete imported diff therefore does not have a clean whitespace check.

The native Linux server plus two Windows fake-game clients under Proton
Experimental `experimental-11.0-20261014-x86_64` completed 1,200 steps each
with matching final lanes and zero divergences. Windows launcher, agent and
hook DLL development binaries were crosscompiled. Model tests, synthetic
Proton runs and UI fixtures do not prove real TF3 injection or gameplay.
The 72 Windows native-mod tests pass under Proton. All 42 optimized model
regression runs and the paced 80-bot, poor-network, tunnel and real-compaction
load cases pass without divergent replicas. The separate tpfre suite passes
49 tests with one ignored; ten Python helper suites pass. Teal comparison
introduces zero compiler errors relative to the installed original, whose
existing compiler errors remain. Twenty-one launcher sample screens were
rendered; the consent, browser, manager and lobby views passed visual review
at 1100×690 on Linux. These use a sample backend, not a live package source.

The inspected Steam installation is still a Linux ELF, Steam depot BuildID
25754343, with no Windows `TransportFever3.exe` or initialized game prefix.
The old Linux stdout is stale and is not current internal-version evidence.
The owner confirmed Linux is the current installation. Linux can remain the
OS: the required game edition is Windows 40420 under Proton. The owner must
select Proton in Steam, allow its Windows installation, and start it once;
this update did not change Steam, installed game files or real saves.

Follow [REEDME_SETUP.md](../docs/REEDME_SETUP.md) and
[GAME_TESTING.md](../docs/GAME_TESTING.md) after that input exists. Agent-run
Proton automation still needs its tools/game path adapted/validated for the
native Linux server; the Windows PowerShell helper starts a Windows server,
whose compaction under Wine has a known failure. Actual competitive acceptance
requires two independently founded companies, ordinary UI actions and balances,
asset ownership/station permissions, 1x/4x traffic, every scheduled checkpoint,
saves, late join, reconnect and server recovery. Genuine private native archives
and verifier runner remain release inputs. No signed player release, plugin
package or public game server was published.
