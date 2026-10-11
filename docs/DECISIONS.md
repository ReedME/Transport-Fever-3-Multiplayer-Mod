# Decision log

Each entry records a decision, why it was made, and what it rejected. A later
entry may supersede an earlier one; entries are never rewritten.

## D1 (2026-09-18): Rust for all project code

The server, agent, protocol, canonical rules and in-game hook are written in
Rust.

- **Performance.** On par with C++, with no garbage collector. That matters
  for the hook, which runs on the game's own threads.
- **Memory safety.** The server faces the internet, and the hook must not
  crash the game.
- **One language across both ends.** Client and server share the protocol and
  rules crates, so the two sides cannot drift apart.
- **Platforms.** One codebase builds for Windows x64, Linux x64 and macOS
  arm64. quinn (QUIC) runs on all three.

Rejected:

- **C++.** No memory safety, and the team prefers to avoid it.
- **C#.** A GC runtime inside the game process is a poor fit for detours on
  the simulation thread. Also, .NET's `System.Net.Quic` requires Windows 11 or
  Server 2022 (TPF3's minimum is Windows 10) and supports macOS only
  "partially, through a non-standard Homebrew package". See
  <https://learn.microsoft.com/en-us/dotnet/fundamentals/networking/quic/quic-overview>.
  The hook would still need a second, native language.
- **Python.** Both TPF2 projects use it. It has a performance ceiling for the
  server, and shipping it to players means PyInstaller executables.

## D2 (2026-09-18): canonical server authority, native worlds as replicas

The server advances a deterministic canonical state machine: companies,
money, ownership, identities, topology, lines, vehicles, economy, calendar.
Native TPF3 worlds apply the same ordered events at the same step, report
postconditions, and are rebased when they drift. See [ARCHITECTURE.md](ARCHITECTURE.md).

- Mixed platforms (Windows, Linux, macOS arm64) in one room are a hard
  requirement.
- Different binaries from different compilers, and arm64 FMA contraction,
  make bit-identical native simulation across platforms very unlikely.

This supersedes the first plan of 2026-09-18, "server-sequenced pure
lockstep", which relied on native determinism. That plan's turn-seal
sequencing is kept as the ordering and pacing layer.

Rejected:

- **Pure native lockstep.** It only works within one binary.
- **Trusting one designated replica's native economy.** That replica's machine
  becomes the economic truth. It is kept as an open co-op question, not as
  the foundation.

## D3 (2026-09-18): QUIC first, WebSocket over TLS as fallback

- QUIC gives TLS 1.3, independent streams (a snapshot transfer never delays a
  sealed turn), datagrams for advisory traffic, and connection migration.
- WebSocket over TLS on TCP 443 covers networks that block UDP.
- Both carry the same messages.

Rejected: TCP-only, which has head-of-line blocking between snapshots and
turns, and raw UDP with custom reliability and cryptography, which is what
`tpf2-multiplayer` had to build by hand.

Update (2026-09-19): the fallback carries QUIC itself, not the messages. A
tunnel is a WebSocket whose binary messages are QUIC datagrams, and the
server merges tunnels into its one QUIC endpoint. A second transport for the
same messages would have needed its own framing, multiplexing, flow control
and authentication, and every feature would have had to work on both. QUIC
inside TCP pays twice for congestion control and suffers TCP's head-of-line
blocking, which is acceptable for networks that leave no other way.

## D4 (2026-09-18): operated servers, trusted by clients

Servers are run by the project: first on the existing German server, then on
regional VPS nodes.

- Clients trust the server.
- Players authenticate to it with per-install keys and room invites.
- Community-run servers are out of scope. Supporting them later would require
  player-signed intents, so that a server cannot forge actions.

## D5 (2026-09-18): one team, both TPF2 codebases as input

_Sep (TPF2MP, `tf2mp-relay`) and silver2127 (`tpf2-multiplayer`)
work on this repository together. Both TPF2 codebases are MIT licensed. Code
or test vectors taken from them are credited in the file that uses them.

## D6 (2026-09-19): the host chooses the room's rules, the game's own economy included

Each server offers a list of rules, and the host of a room picks one when
creating it. Each set of rules comes with its economy. The first on the list
is the default. The room keeps its rules for good: they are recorded in its
log, and a restarted server restores the room with the same rules or not at
all.

- **`native` is always offered, and is the default until others ship.** The
  server orders every player's commands and checks nothing about money. Each
  game runs its own economy as in single player. Checkpoints still compare
  every replica's world: one that drifts, for example on another platform,
  is re-sent the world the room agreed on.
- **Canonical rules are an option on the same list** (D2). The server
  validates intents and settles the economy itself, from TPF2MP's
  integer-arithmetic model. Those rooms get what D2 promises: money no
  client can forge, balance changes without client updates, and hidden
  information.
- The hook learns the room's rules when the game begins (`ToHook::Begin`),
  so with `native` it leaves the game's economy alone.

This amends D2. Canonical authority stays the design for rooms that choose
it, and stops being a requirement for every room. D2 rejected "trusting one
designated replica's native economy"; `native` rooms trust none: every replica
runs the economy, and the room's agreed world corrects any that disagrees.
Players who want the game as they know it can have it; players who want a
server that cannot be cheated choose the canonical rules.

Rejected:

- **One economy per server.** Groups on the same server want different
  games.
- **Changing rules mid-game.** The rules' state and the log would have to be
  converted. A new room is the way to switch.

## D7 (2026-09-26): a native launcher window that updates itself from signed releases

Players start TPF3-MP from a native window on Windows, Linux and macOS,
drawn with egui (`eframe`), in place of a page in their browser. The window
runs the agent's launcher backend in its own process; the page remains, as
`tpf3mp-agent launcher` and as the window's fallback on systems where no
window can open.

- **One program, one window.** No browser tab to keep open, no console on
  Windows, and closing the window during a game asks first.
- **Rust and one code base.** egui builds on all three platforms with the
  same crates as the rest; its UI is tested headless through AccessKit
  (`egui_kittest`), and screens can be rendered to images for review.
- **Updates are signed.** The launcher downloads the latest published
  release and installs it only if its manifest carries an Ed25519
  signature from the project's key, names a newer version, and describes
  the package byte for byte (SHA-256). The private key is a repository
  secret, the public half is built into every launcher. A launcher built
  without the key never updates. An update never interrupts a game: it
  installs when the player chooses or at the next start, and a failed
  install puts the old files back.

Rejected:

- **Tauri or another web view.** A web page in a native frame: still the
  browser engine, now shipped or required per platform (WebView2,
  WebKitGTK), and a second language for the UI.
- **Qt or GTK.** C++ or C libraries to build and ship on three platforms,
  against D1.
- **Unsigned updates over HTTPS.** Anyone who could publish a release, or
  replace an asset, would run code on every player's machine.

Update (2026-09-27), after a security review of the updater: signing
moved out of the build into `sign.yml`, which signs only a published
release, in a GitHub environment whose required reviewer approves each
signing. As a repository secret, the key reached every workflow on every
branch, so anyone who could push a branch could have signed an update and
skipped every check. Launchers trust a list of keys, so the key can be
changed. Installing is journalled and confirmed: an install that fails or
is cut short is undone, the old files stay until the new version opens its
window, a version that fails to three times is rolled back and not
installed again, and only files a journal names are deleted. One process
installs at a time. Releases are fetched from `releases/latest/download`,
over HTTPS only, rather than GitHub's rate-limited API.

## D8 (2026-09-26): player actions as a typed schema in millimetres, inside the opaque payload

A player action travels as `tpf3mp_proto::action::Action`: positions as
`i32` millimetres, resources by file name, and things that have no stable
position (companies, lines, vehicles, stations) by ids the server assigns.
See "The action schema" in [BUILDING.md](BUILDING.md).

- **Integers, not floats.** The same action is the same bytes on every
  platform, compares exactly and hashes the same; a millimetre is far below
  the tolerances replicas match geometry with.
- **Typed and bounded.** TPF2's text commands were parsed field by field and
  a mis-parse became a wrong build; here decoding checks every length and
  every index, so a replica only ever sees a well-formed action.
- **Its own version, inside the payload.** The network layer relays actions
  without reading them, so the schema can grow without a protocol change.

Rejected:

- **Floats in metres.** Not bit-identical once converted twice, and a
  canonical server cannot use them (D2).
- **Engine entity ids.** They differ between games and are recycled
  (BUILDING.md).
- **The TPF2 text format.** Unbounded, untyped, and hand-parsed on both
  sides.

## D9 (2026-09-27): the installer is scripts players can read

What puts TPF3-MP into the game (the mod, the hook, and on Windows the
proxy DLL in place of one of the game's own) ships in the package as
scripts: `INSTALL_TPF3MP.cmd`, which runs `tools\install.ps1`, on Windows,
and `install.sh` on Linux and macOS. They replace `tpf3mp-agent
install-hook`.

- **Players trust what they can read.** The installer changes the game's
  folder. TPF2MP's players did not trust a program doing that; its
  PowerShell installer, which anyone can open, is what they accepted. A
  script shows exactly what it changes, with nothing compiled in between.
- **The same rules as the code.** The scripts fail closed as the Rust
  installer did. They refuse while the game runs, in a folder that is not
  the game's, over another mod's proxy, when the game's own DLL is gone,
  and when their record names anything but TPF3-MP's files. A step that
  fails undoes the ones before it, and nothing is deleted: what is
  replaced goes to a backups folder. CI tests them on all three platforms,
  in the shells players have: Windows PowerShell 5.1, and the bash 3.2
  macOS ships.
- **An exception to D1, for this alone.** The launcher, the agent and the
  hook stay Rust.

Rejected:

- **A compiled installer** (the agent's `install-hook`, an MSI or a setup
  program): opaque to players, and flagged by SmartScreen and antivirus
  like any unsigned program.

Update (2026-09-27), with D11: the installers put in the mod alone. The
hook stays in the package, next to the launcher, and nothing goes into the
game's folder: no proxy, no hook, no record. The record is kept in
TPF3-MP's own data folder (`installed.json` on Windows, `installed.txt` on
Linux and macOS). The rest of this entry stands: the installers are still
readable scripts, fail closed, undo a failed step, delete nothing, and are
tested on all three platforms.

## D10 (2026-09-27): players' diagnostics go to the server by themselves

*The owner-approved amendment below adds the hook's and the game's
logs and the game's error reports to what goes, under one log session
code for a launcher's run.*

The launcher sends the lines of its log, redacted, to the server the player
plays on, which keeps them by session: the operator reads what went wrong
for a player from the support ID alone, as TPF2MP's relay let its operator
do. Asking players for files, which Collect logs still makes, comes late
and often not at all.

- **Over the game's connection.** A request on the control stream, from a
  client that has proven its identity, filed under its session: no second
  service, port or credential.
- **What TPF2MP's missed, fixed.** Its redaction missed paths outside
  `C:\Users`, Steam's `userdata\<account>` among them, and paths escaped
  in JSON; `tpf3mp_proto::redact` cuts every absolute path to its last
  part, on both sides. Its uploader lost the last lines before a session
  ended; closing now sends them, and lines left when a connection drops go
  with the next. Its only opt-out was giving up the relay; here a switch
  in the launcher stops them, and is remembered.
- **Never in a game's way.** Their own rate budget, a writer that does not
  make connections wait, a quota per session, and a total the oldest make
  room in.

Rejected:

- **Whole log files, or crash dumps.** Too large, and too much in them to
  redact reliably; Collect logs remains for those, on the player's say.
- **An HTTP upload to the server.** A second way in, needing its own
  authentication, rate limits and TLS, for what the game's connection
  already carries.

Update (2026-09-27): the launcher's window and page no longer offer
**Collect logs** or **Open logs folder**. With the log going to the server
by itself, players have nothing to send but their support ID. The game's
own log and crash dumps, which are never sent, come from `tpf3mp-agent
collect-logs` when an operator asks for them. The page's
`/api/collect-logs`, a way for the page to make the launcher write files,
is gone with the button.

### D10 amendment (approved by the owner, 2026-10-02): the hook's and the game's logs go too

**Approved by the owner (Juliansgith) on 2026-10-02 after the collection
scope was explained: "you can merge those in". Live-game acceptance may
follow integration into dev; this does not claim that acceptance passed.**

A contributor, silver2127, asked for all of a player's logs to reach the
server with one code naming them, and asked for D10 to change for it.
Today an operator sees the launcher's side of a failure and must ask the
player for the zip `tpf3mp-agent collect-logs` writes to see the hook's
and the game's, which, as D10 says of files, comes late and often not at
all. Most failures in the real game show only there.

- **What changes: the content, not the destination.** The launcher's
  diagnostics still go to the one server the player plays on, over the
  game's connection, as D10 has them. Besides the launcher's and the
  agent's lines, they now carry the in-game hook's `hook.log` and the
  game's own `stdout.txt`, each read from where it stood when the
  launcher started, and the text of the game's error reports (the `.txt`
  and `.json` files in its `crash_dump` folder) as they appear. Every line
  says its source (`launcher`, `agent`, `hook`, `game`, `crash`).
- **One code for a launcher's run.** Each line carries a *log session*: a
  code like a support code (D13's generator and format), chosen when the
  launcher starts and kept until it closes. The support code names one
  connection and changes with every reconnection; the log session names
  the whole run, so the operator reads all of it with one code, by source
  if they like (`diagnostics <code> hook`). The launcher's window, its
  page and the game's Multiplayer window show it, with Copy, while
  diagnostics are on. Like a support code it lets nobody into anything.
- **Sessions are labelled with the player.** The server already knows
  each session's player ID (`p-…`) and the name the launcher gave in its
  handshake, the one the lobby shows; it now keeps both with every line
  and in a run's index, lists them with each session, heads
  `diagnostics <code>` with them, and finds a player's sessions by name
  (`diagnostics --name <name>`) or ID (`--player`). The client sends
  nothing more for it. Names are not unique and can change: the player ID
  is the stable link between a player's sessions. The name is redacted
  like a line.
- **Why.** The failures that matter now happen in the game, after the
  launcher's part went well; the operator should see them from one code
  the player posts, as D10 meant for the launcher's.

What keeps it safe:

- **Lines, never files.** Text read line by line and sent as D10's lines
  are, each cut to 1 KiB. Never the game's `.dmp` minidumps, which are
  large, binary and hold memory nobody can redact; never a file whose
  name looks like a key, certificate or token; never the saves, the
  identity key or `launcher.json`. `collect-logs` stays for the dumps, on
  the player's say.
- **Redacted on both sides, with D10's rules and one more.** Paths, IP
  addresses, invites, keys and passwords, e-mail addresses and Steam IDs
  are taken out by `tpf3mp_proto::redact` before a line leaves and again
  on the server; the game's error reports name the Steam account as
  `"userId"`, so values after keys naming an account (`userId`,
  `account_id`, `steamid`, …) go too.
- **Bounded at every step.** On the player's machine each source has a
  budget of bytes a minute (the hook's log 192 KiB, the game's 96 KiB,
  error reports 512 KiB); a log that grows faster loses its oldest unread
  part, and the launcher's log says how much. At most 10,000 lines wait,
  the oldest going first, and a run sends 256 MiB at most. Files are read
  on a thread of their own, never the game's or a connection's. On the
  server the diagnostics budget is two requests a second with a burst of
  sixteen (one and eight before), on its own as before; a session keeps
  64 MiB at most (`--diagnostics-session-mib`, 8 before), all sessions
  within `--diagnostics-mib`, for `--diagnostics-days`, the oldest going
  first; the writer still never makes a connection wait.
- **The switch stops all of it.** **Send diagnostics** Off stops every
  source and forgets what waited; what the logs gain meanwhile is passed
  over, never sent later. The choice is remembered, as before.

This reverses D10's rejection of **whole log files** in part: the hook's
and the game's logs now go, as redacted lines within budgets rather than
as files, while crash dumps stay rejected. The rejection of **an HTTP
upload**, a second way in, stands: nothing here opens another service,
port or credential. It changes the protocol (version 16: each line's
source and the log session, `Request::Telemetry`) and the link to the
game (version 23: the window shows the log session).

Rejected:

- **Reusing the support code for every line.** It names a connection,
  and a launcher makes several in a run (reconnections, server changes);
  a run's logs would be split over codes the player never saw.
- **Sending to more servers than the one played on** (dev servers listed
  in the build or the settings), as first asked: a second destination for
  players' logs, held by the user for now.

## D11 (2026-09-27): the hook runs only in a game the launcher starts

TPF3-MP's code runs in Transport Fever 3 only when a player starts the game
from the TPF3-MP launcher, for a room they are in; that game runs it until
it closes. Started from Steam, the game is the plain game, with nothing of
TPF3-MP in it. TPF2MP worked this way, and its players expected it.

- **How it starts.** `tpf3mp-launch` starts the game with the hook in that
  one process. On Windows it starts the game suspended, has the game load
  the hook with `LoadLibraryW` on a thread the launcher creates in it,
  checks that the hook is there, and only then lets the game run; when any
  of that fails, the game is ended, not left running. This is how TPF2MP's
  injector started Transport Fever 2 (`--launch`). On Linux, the game gets
  `LD_PRELOAD` naming the hook, in its own environment only. macOS waits
  for the game: its hardened runtime refuses libraries it did not load
  itself.
- **The game is told which launcher started it.** The launcher passes the
  name of its link to the game (`TPF3MP_GAME_LINK`); a hook without it does
  nothing at all, so the hook is inert even if something else loads it.
  The game also gets `SteamAppId`, so that it does not restart through
  Steam without the hook. As TPF2MP's launcher did, it starts the game only
  while Steam runs, and one game at a time.
- **Nothing to undo.** Closing the game ends TPF3-MP's part in it. There is
  no file in the game's folder for Steam's file check to report, another
  mod to collide with, or a game update to break, and nothing to uninstall
  but the mod.
- **Fail closed, as before.** The hook still checks the game's build
  against its profiles and installs nothing when none matches (see
  [HOOKS.md](HOOKS.md)).

Rejected:

- **A proxy DLL in the game's folder** (TPF2's `alut.dll` trick,
  `tpf3mp-proxygen`): it loads the hook into every start of the game, from
  Steam too, until it is taken out; Steam's file check and game updates
  undo it, and it collides with other mods that proxy the same DLL. The
  generator is removed.
- **A Steam launch option with `LD_PRELOAD`** on Linux: the same, set by
  hand, and easy to forget when uninstalling.
- **Starting the game through Steam** (`steam://run`): the game would then
  start without the hook, since nothing of the launcher's reaches it.

## D12 (2026-09-27): the launcher plays on the project's server alone

*The 2026-09-30 server-setting amendment below remains proposed. The
regional amendment proposed 2026-10-06 was approved on 2026-10-08; for
regional releases it supersedes this original one-server limit and the
earlier no-invite-routing rule.*

A package's launcher plays on the one server it was built for, the
project's own (D4), set when the release is built
(`TPF3MP_DEFAULT_SERVER`). Players do not type a server and cannot choose
another, and an invite naming another server is refused, not followed.
TPF2MP's launcher offered its relay in a box players could change; this
one does not.

- **One place to meet.** Every player, and every invite, is on the same
  server: nobody mistypes an address or ends up alone on another.
- **Fail closed.** Clients trust the server they play on (D4): it sends
  the room's turns and worlds. Following any invite's server would let an
  invite send players to a server nobody vouches for.
- **Enforced in the launcher's backend**, so the window and the page
  behave alike. The server is shown, not asked for; Connect takes the
  player's name and, if they have one, an invite, to join in one step.
- **Development stays open.** `--server` on the command line fixes
  another server, for playtests against a local one; a build without a
  server, as a developer's own, offers the typed field as before. No
  release is drafted without `TPF3MP_DEFAULT_SERVER`.

This narrows D4 in the launcher: operated servers were the plan, and
now the players' launcher knows no other. More servers, such as regional
ones, come later through the launcher itself, never through what an
invite says.

Rejected:

- **A server field players can change** (as until now): an invite could
  name any server, and a typo ends in a lonely room.
- **Following an invite to its server** (as until now): the fail-closed
  reason above.

### D12 amendment (PROPOSED amendment, not decided, 2026-09-30): a default server players may change

**Proposed, for the owner (Juliansgith) to approve or refuse in the pull
request. D12 above stays in force until then.**

Asked about D12, the user answered after talking with the mod's
co-developer: "its supposed to be changable". The project's relay,
`tpf3mp.213-133-98-90.sslip.io:29470`, is the server "everyone more or
less should be using", so it becomes the default rather than the only one:

- **The launcher defaults to the project's relay.** A package plays on the
  server it was built for (`TPF3MP_DEFAULT_SERVER`, the release's choice);
  a build without one, a developer's included, plays on the relay, shown
  as **EU**. `--server` on the command line still overrides it for one run,
  for playtests.
- **Players may change the server in Settings.** The launcher's Settings
  (and the browser page, and the game's Multiplayer window through
  `LobbyAction::SetServer`) show the server played on, take another as
  `host:port` only, and offer **Reset to default**. The choice is
  remembered in `launcher.json` (`chosen_server`); changing it disconnects
  and connects to the new server; it is refused while in a room.
- **Invites still never switch servers.** D12's fail-closed reason stands:
  an invite that names another server is refused, and Connect with an
  invite joins on the player's own server. Only the player's own setting
  changes where they play, so a message cannot send anyone to a server
  they did not choose. Friends on another server all set it the same.
- **Trust is unchanged** (D4): a server must have a certificate from a
  public authority, as the relay does (Let's Encrypt, for its sslip.io
  name); `--pin-cert` remains for development servers.

This replaces "Players do not type a server and cannot choose another"
and the rejection of "a server field players can change"; the rejection
of following an invite to its server stays. The release workflow still
drafts no release without `TPF3MP_DEFAULT_SERVER`, so each release names
its server on purpose; the relay in the code is the fallback for builds
without it.

### D12 amendment (approved 2026-10-08): several operated servers, rooms hosted on the closest

The owner approved the regional-server proposal and automatic typed-invite
lookup. D12's trust boundary remains: a release may route only to servers it
lists, and text pasted beside a six-character invite is never routing
authority. For multi-server releases this replaces earlier D12 text that
said invites never change servers; one-server behavior remains as before.

Asked by a contributor (silver2127), who runs a second server on a VPS:
"set up the server on the VPS as well and add in to the game that hosts
pick the closest server but players can see games on both." D12 already
says regional servers "come later through the launcher itself, never
through what an invite says"; this is that step:

- **A release lists its servers.** The default server
  (`TPF3MP_DEFAULT_SERVER`, named by `TPF3MP_SERVER_NAME`) comes first,
  then the others the release vouches for (`TPF3MP_SERVERS`, such as
  `US=us.example.org:29470`, names and `host:port`s separated by commas).
  The list is built in, as the default server is: players still never
  type a server to meet, and a list that does not read stops the
  launcher rather than guessing.
- **Rooms are hosted on the closest server.** The launcher connects to
  the closest listed server that answers, by the round trip of the QUIC
  connection it already opens, and before creating a room it moves to the
  closest again. Pings within 10 ms of the lowest count as equal: the
  server played on stays, else the one listed first. A server that does
  not answer is passed over; when none answers, connecting fails as it
  always did.
- **Players see the rooms of every server.** While connected, the
  launcher keeps a quiet connection to each other listed server (no
  content declared, no room, no diagnostics). It gathers each server's
  pages from zero through the requested global page, sorts the rooms
  together, and returns 20 at a time; earlier local pages are cached while
  browsing. Each room card carries the listed server name through Join, so
  equal invite codes on different servers still select the clicked room.
- **Invites stay six-character codes.** A room card carries its compiled,
  trusted region as well as its code. A typed or pasted code is checked on
  every listed region, concurrently, with a read-only `ResolveInvite`
  request carrying the optional password. It returns no room details. The
  launcher joins only when every region answered and exactly one accepts
  the credentials. No match, a wrong password, and a banned player have
  the same `BadInvite` result; multiple matches require the player to
  choose a listed region. An unavailable, timed-out, or rate-limited region
  makes the lookup incomplete and retryable. Until a unique match exists,
  the launcher neither promotes a connection nor sends `JoinRoom`. It
  then joins once, and `JoinRoom` rechecks credentials, capacity and
  eligibility; a refusal never falls through to another region. Choosing a
  region or clicking its public room card routes only to that
  release-listed server. A pasted server-looking prefix is ignored. This
  keeps a hidden private room that shares a code with a public room from
  being mistaken for a unique public match.
- **Trust is unchanged** (D4): every listed server needs a certificate
  from a public authority, as the relay's. Each server keeps its own
  `invite.key`, rooms and logs; nothing passes between servers.
- **Protocol and rollout are coordinated.** `ResolveInvite` changes the
  protocol to version 19, so every client and every regional server in a
  release must be upgraded together. The launcher uses its already-open
  trusted connections; it does not rely on a global room registry. A
  release that lists only its default server behaves as before.
- **The operator chooses the rollout.** `--server` still plays on one
  server alone for a playtest; a release's compiled server list is the
  routing authority. This decision and implementation do not configure or
  deploy additional servers or change the live relay.

Trade-offs: each launcher holds one idle connection per other server
while connected (keep-alives only, while idle), and every server
sees each connected player's session, not only the one they play on. Every
typed invite probes every listed region; a miss consumes that server's
normal wrong-invite budget, so one guess against an eight-server release
counts against all eight budgets. Loading a later global page may need one
request per preceding local page, but those pages are kept for the current
browse.

Rejected:

- **Invites naming their server** (longer codes, or `EU-K7QM2X`): D13's
  six characters stay what players read out; the launcher finds the room.
- **Choosing the closest by geography or by the player's region
  setting**: a measured round trip needs no setting and follows the
  network players actually have.
- **Servers syncing rooms with each other**: every server stays on its
  own, and the launchers do the listing.

## D13 (2026-09-27): invites and support codes are six letters and digits

A room's invite is a code such as `K7QM2X`, and so is a player's support
code, the ID of their connection. Both are six characters from 31 letters
and digits: upper case, typed in either case, without the look-alikes 0,
1, I, L and O, and with at least one letter and one digit (about 740
million codes). They look alike but are separate: a support code finds a
session's diagnostics and lets nobody into a room, so it can be posted
in a public support channel.

- **Short enough to read out.** With one server (D12) an invite needs no
  address, and players pass it by voice or in a chat line, not a
  72-character token. The support code is quoted the same way.
- **The server finds the room by the code.** It keeps an HMAC of the code
  under its key (`invite.key`), not the code, in the room's log and its
  index, so a leaked log gives no invite away. Open rooms never share a
  code, and no two sessions share a support code while their diagnostics
  are kept.
- **Guessing is held off by the server, not by length.** A 256-bit token
  could not be guessed; a code can, given enough tries. An address may
  try 20 wrong invites (or passwords) in 10 minutes, and is then refused
  every join until the window ends: about 2,900 tries a day against 740
  million codes. A cross-region typed invite checks each listed server,
  and every negative code/password pair consumes that server's same
  per-address budget; if any server rate-limits the lookup, the launcher
  does not guess around it. A room's password still guards it on top, and
  its owner can kick anyone who gets in.
- **Logs name invites by key.** A six-character code cannot be spotted in
  a log line as `TPF3MP1.…` could. Code that logs one writes
  `invite=<code>`, which redaction hides, and `Invite`'s `Debug` never
  shows it.

This changes the protocol (version 6) and the room log (version 7):
rooms logged by earlier versions are set aside, not restored. No game
had been played on the project's server when it changed.

Rejected:

- **One code for both** a room and its players' support: posting it for
  support would let anyone into the room.
- **Longer codes, or the old token**: the server's limit is what stops
  guessing, and every character more is one more to read out.
- **Codes of letters alone**: an ordinary word in a message would pass
  for one.

## D14 (2026-09-28): release-day reverse engineering through one Rust indexer

The Windows executable is decoded with `tools/tpfre`: one parallel pass
into one SQLite file (functions, direct calls, references, strings,
`__FUNCSIG__`/`__FILE__` names, RTTI), then small queries that answer in
milliseconds with one fact per line. It is the tool coding agents use to
find the hook targets on release day.

- **Time.** TPF2's route was a full Ghidra auto-analysis ("expect hours",
  48 GB of RAM), then Python queries over its CSV dumps. The index takes
  about 4 seconds on TPF2's 72 MB executable, so a patch costs nothing to
  re-index, and an agent can ask hundreds of cheap questions instead of a
  few expensive ones.
- **The validated naming, kept.** It ports `tools/re/name_functions.py`'s
  rules and agrees with it name for name on TPF2 build 35924; signatures
  follow `make_profile.py` and match its output byte for byte.
- **Fail closed.** Queries refuse a binary whose SHA-256 is not the one
  indexed, and a database of another schema; names carry their source and
  confidence, and uncertain ones are marked.
- **Rust (D1)**, in its own Cargo workspace under `tools/`, so the
  project's workspace, lockfile and release builds are untouched. CI runs
  its format, lint and tests on Linux only.

The Python tools stay for what tpfre does not do: the survey report,
ELF and Mach-O (arm64) naming, and the Ghidra, x64dbg and IDA scripts.

Rejected:

- **Ghidra auto-analysis as the first step**: hours per build, and one
  project locked to one process, so questions queue.
- **Growing the Python tools**: a full disassembly of every function in
  Python takes minutes, and each query would reload the binary.

## D15 (2026-09-27): the mod hands the hook tables; Rust converts them

The Lua mod and the hook exchange actions as Lua tables in the game's own
units (metres, and plain fractions for directions). The hook converts
them to and from `Action` with `tpf3mp_proto::lua`. The mod has no
encoder: `tpf3mp/wire.lua`, which wrote postcard bytes by hand, and
`tpf3mp/fixed.lua`, which rounded metres to millimetres, are gone. The
bridge between them is version 2 (docs/HOOKS.md, "The Lua side").

- **One definition.** The schema, its bounds and its rounding were written
  twice, once in Rust and once in Lua, and kept in step by tests. Now
  they are written once. A new action needs no Lua encoder.
- **The hook needed it anyway.** Applying an event means handing the mod
  the action to run, so the hook had to turn an `Action` into a table in
  any case. Doing the reverse in the same place keeps one conversion.
- **Better refusals.** A table the schema refuses comes back with the path
  to the bad value (`polyline.vertices[2].pos.x`), where the Lua encoder
  raised on the first violation.
- **No game Lua in Rust's way.** The conversion works on a `LuaValue` tree,
  not on a Lua library: the hook reads the game's own Lua state with the
  game's functions, within `MAX_DEPTH` and `MAX_NODES`, and hands over the
  tree.

The unit of each field comes from the type it is in (`Pos` in metres,
`UnitDir` in fractions, and so on), so the Rust types stay as they are and
postcard encodes them as before; the action schema's version is
unchanged.

Rejected:

- **Keeping the Lua encoder** (as until now): two definitions of one
  schema, and still a decoder to write for applying events.
- **Integers in the tables** (millimetres from Lua): the rounding would
  stay in Lua, and every capture would need it.

## D16 (2026-09-27): the launcher is tearded's TPF2 launcher, ported, in a web view

*Superseded by D20 (the owner, 2026-09-28): the look stays, the web view goes.*

The launcher's window is a port of tearded's TPF2 Multiplayer Launcher
(MIT, `github.com/tearded/tpf-multiplayer-launcher`): its page, layout,
styles and look, in a Tauri web view, over the same launcher backend as
before (`crates/tpf3mp-launcher`, `ui/`). This supersedes D7's window,
drawn with egui; the rest of D7, the signed self-updates, stands.

- **The players know it.** TPF2's players use tearded's launcher; the TF3
  one looks and works the same: the scene, the wordmark, one big button
  that says what comes next, the game's folder along the bottom.
- **What is kept of tearded's**: the page and its stylesheet, the dialogs,
  the release notes and history, the update badge. **What is not**: its
  mod installer (D9's scripts install the mod), starting the game through
  Steam (D11: the launcher starts it with the hook), its C# helper and its
  Linux backend (the Rust agent does their work), and Tauri's updater
  (D7's stays). What is new: connecting, rooms, the lobby, chat, the
  game's progress, the support code.
- **No Node in the build.** The page is plain HTML, CSS and JavaScript,
  served as they are; the Rust side is a small shell of commands over the
  launcher (`shell.rs`). What the page shows is worked out in one pure
  module (`ui/view.js`), tested under plain Node.
- **Where a window cannot open**, the launcher falls back to the agent's
  page in the browser, as before.

Costs, accepted:

- Linux players need WebKitGTK 4.1 (most desktops have it); Windows needs
  WebView2 (Windows 11 has it, Windows 10 mostly does).
- The page's text is JavaScript, which the checks now test (`node --test`)
  next to the Rust.

This settles PLAN.md's open question about building into the TF2 launcher,
and resolves the "another launcher" row of its conflict table: the ask was
for the look, and the look is now tearded's own.

## D17 (2026-09-27): the room moves into the game after release

*Held by the owner until after launch (D20).*

Connecting, rooms, the lobby and chat move from the launcher into an
in-game panel (a game bar plugin, as the TF3 mods have), after release.
The launcher then keeps: starting the game with the hook, the mod's state,
updates and settings, the support code. The game shows the room and
takes the player's choices; the launcher carries the connection.

- **Why after release.** The in-game panel is built on TF3's GUI, known so
  far only from mods made for build 40391, and first runs on release day.
  Until it is proven there, the launcher keeps the room, so nobody is
  left unable to play.
- **What it needs.** The link between hook and agent carries the
  launcher's state to the game and its actions back (a bridge version
  bump); a game can start before a room is chosen, and loads the room's
  world from the menu.
- **D11 stands.** Only a game the launcher started has the hook, so only it
  shows the panel; a game Steam started shows nothing of TPF3-MP.

This replaces PLAN.md's in-game panel that "hands a code to the launcher".

**Amended 2026-09-30, by the owner: the hold is lifted.** The owner
(Juliansgith) asked on 2026-09-30 for the in-game multiplayer menu to work
now: the room's lobby moves into the game now, not after release. D20's
hold stays on D18 and D19 only.

- **What moves.** The Multiplayer entry on the game's main menu
  ([LOBBY.md](LOBBY.md)) opens a window that connects to the server,
  creates a room or joins one by invite, shows the room's players and
  their ready marks, chats, gets ready and, for the room's owner, starts
  the room's game. It is built on the game's own menu (its Lua, reached by
  the hook), not on a game bar plugin.
- **How.** The window is another front end of the launcher: its buttons
  are the launcher's actions, carried over the link between hook and agent
  (bridge version 9, [HOOKS.md](HOOKS.md), "The main menu's Multiplayer
  window"). The launcher still starts the game with the hook and holds the
  connection; it now starts the game before a room is chosen, and keeps
  everything it did, so a menu the hook cannot reach costs the player
  nothing.
- **D11 stays.** Only a game the launcher started has the hook, and so the
  entry; a game Steam started shows nothing of TPF3-MP.

## D18 (2026-09-27): players choose their version and track

*Held by the owner until after launch (D20).*

As in tearded's launcher, the player may choose which TPF3-MP to run:

- **Stable or Experimental** (Settings): Experimental also offers
  pre-releases, found through GitHub's release list.
- **Any earlier or later signed release** (the release history's
  Install): downloaded and checked exactly as an update (the signed
  manifest, which must name that version, then the package's size and
  hash), installed through the same journal, and then **held**: nothing
  updates on its own until the player resumes updates.

This amends D7, which installed only newer versions so that nobody could
push players back onto an older build. That protection stays for
everything automatic; going back is now the player's own, confirmed
choice in the launcher, of a release the project signed.

Consequences, accepted:

- A server plays only with its own protocol version, so an older TPF3-MP
  cannot join the project's server once it has moved on (the launcher
  says so before installing); a pre-release needs a server of its own.
- A version from before this decision does not know about holding: its
  updater moves on to the newest release again.
- Release notes and the history come from GitHub's API, 60 requests an
  hour per address: fetched when the player looks, and for the
  Experimental track's checks. Stable's checks still avoid the API.

## D19 (2026-09-28): a Dev track of untested builds, signed with a key of its own

*Held by the owner until after launch (D20); `dev-build.yml` is removed.*

Every push to `dev` is built and published as a **dev build**
(`dev-build.yml`), as soon as its packages are built: it runs no tests and
waits for none. Its version is the workspace's next patch version with
`-dev.<run number>`, such as `0.1.1-dev.14`. The launcher's Settings offer
a third track, **Dev builds**, which follows them, with a warning that
they are untested. It is for the team and testers.

Dev builds are signed at once, with no approval, so not with the release
key (D7), whose every signing a person approves. They have a **dev key**
of their own, in the `dev-builds` environment, which only `dev` may use.
A launcher trusts the dev key only while its player is on the Dev track,
and only for a version named `-dev.<n>`:

- it cannot sign a release or any other pre-release, for anyone;
- players on Stable or Experimental never take a dev build, and a dev
  build downloaded before switching away is deleted, since it no longer
  verifies;
- a dev build is published as a pre-release that is never GitHub's latest
  release, and only the newest ten are kept.

This amends D7 for the Dev track only: there, whoever can push to `dev`
can run code on the machines of the players who chose that track. That
is the team and its testers, who chose it; everyone else is as before.

Consequences, accepted:

- A push to `dev` reaches Dev-track players within the time a build
  takes, before `ci` has passed. `ci` still gates `acceptance` (AGENTS.md).
- A dev build plays only on a server running its protocol version: the
  project's server runs releases, so a dev build that changed the
  protocol needs a server of its own.
- A player who leaves the Dev track stays on their dev build until a
  release newer than it comes out, or until they install one from the
  history.

## D20 (2026-09-28): the launcher is a native window in the page's exact look

The owner's call, before launch. The launcher stays a native egui window
(D7), now drawn to look exactly as the page silver2127 ported from
tearded's TPF2 Multiplayer Launcher (D16) does: its city image, wordmark
and logo (`crates/tpf3mp-launcher/images`), its fonts (Segoe UI and
Consolas where the system has them), its colours, sizes and spacing,
taken from the page's computed styles, and its icons, drawn from the
page's own SVG. Its panels are frosted as the page's are: the city is
drawn again from a blurred copy under each. What it shows in each state
is the page's `view.js`, ported to `view.rs` and tested. Its screens are
rendered in the page's sample states for comparison
(`tests/screenshots.rs`).

- **Nothing to install.** A web view needs WebView2 on Windows, which
  stripped and LTSC installs lack, and WebKitGTK 4.1 on Linux, without
  which the launcher does not start at all: the system refuses to load
  it before any fallback could run. The egui window is one file that runs
  wherever the game does.
- **One language, tested as it is.** The window is Rust, and its tests
  click the real window (egui_kittest); the page's logic in JavaScript
  and its Node tests go.
- **The look was the point.** Everything a player sees of D16 stays.

Held until after launch, by the owner, so the release is built from what
was tested: moving the lobby into the game (D17), choosing versions and
tracks (D18), and the Dev track (D19), which let whoever can push to
`dev` run untested code on testers' machines. They come back only as the
owner decides; `dev` now takes pull requests, and a change to this file
or to PLAN.md needs the owner's approval (`.github/CODEOWNERS`).

Rejected:

- **Keeping the web view** (D16): the costs above, for a look egui draws
  as well.
- **egui's own look** (as before D16): the players' launcher looks as
  the team agreed it should.

## D21 (2026-09-30): a room's players choose their companies

The owner, on 2026-09-30: "In a game we should also allow for example 2
people 1 company and 1 person in another company."

- A room starts as one company, the save's own player, which every player
  plays for: co-op, as before.
- In the room's game a player founds a company of their own, joins
  another, renames or recolours theirs, or dissolves it as its last player
  once it owns nothing. Any split of the players is allowed, up to eight
  companies a room.
- A company is a Transport Fever 3 player entity. What a player does is
  booked to their company and paid by it; what another company owns cannot
  be changed or removed.
- A company other than the room's first borrows on the terms the game
  offers, and the room keeps those loans; the game's own loan script keeps
  the first company's.
- With more than one company, vehicles wear their company's colour.

Rejected:

- **One company a player, fixed** (TPF2MP's two rival companies): the owner
  asked for any split.
- **A company chosen only in the lobby, before the game** (TpF2
  Multiplayer's chips): choosing in the game lets a player change their
  mind, and a player who joins late chooses when they arrive.

## D22 (PROPOSED, not decided, 2026-09-30): who may do what to a company, company passwords and shared stations

**Status: proposed.** Written on feature branch `feat/company-play` on top
of D21's pull request; it is not a decision until the owner (Juliansgith)
approves it (AGENTS.md, "Decisions are the owner's").

The ask, on 2026-09-30: "lets work some more on the ingame ui company
switching picking colors company passwords, access control use other
companies stations".

- **A company's head.** The player who founded a company is its head while
  they play for it; after that, the player who has played for it longest.
  The room's first company is everyone's: it has no head.
- **Who may do what.** Any of a company's players builds, buys, runs
  lines, borrows and pays back, renames and recolours it (as D21). Its
  head alone gives it a password, changes it or takes it away, sends a
  player out of it (they play for the room's first company again; what
  they built stays the company's), and opens or closes its stations to
  other companies' lines. Its last player dissolves it once it owns
  nothing (as D21). Anyone joins a company without a password, the room's
  first always.
- **A password to join.** Joining a company with a password needs it. The
  player types it in the game; it travels beside the action to the server
  and no further. The server orders the action with the password's seal,
  an HMAC under its key bound to the room and the company, and every game
  compares that seal with the one the company keeps. No game, log or save
  ever holds the password, and the seal gives nothing away without the
  server's key (as D13 keeps room passwords). A player may send 20
  passwords in 10 minutes, as D13 holds room passwords to guessing.
- **Enforced where every game checks the same way.** Every rule is checked
  by every game when the room orders the action (`tpf3mp/companies.lua`),
  so one game's window deciding otherwise changes nothing; the server
  checks only what only it can, the password.
- **Using another company's stations.** A company's lines may stop at
  another company's stations: stopping changes nothing the station's
  company owns (D21 forbids changing or removing it). Stations start
  open; a company's head may close them to other companies' lines, and
  every game then refuses a new or changed line that stops there. The
  station's upkeep stays its owner's, and a line's fares and costs its
  company's, as TPF2MP's shared stations kept them. A company's vehicles
  still use its own depots.

Rejected:

- **The password in the action, hashed by the player's game**: a hash
  every game can check is one any player can replay, or guess against
  offline.
- **The server tracking who plays for which company**: it would need the
  game's own refusals (a company that still owns something cannot be
  dissolved) to keep its copy right; the games know that, the server does
  not.
- **Every player a say, or only the founder for good**: a vote needs
  rounds a room does not have; a founder who left would lock the company
  forever.
- **Sharing always on, or chosen station by station** (TPF2MP had always
  on, with a list of companies per company): one switch per company is
  what a player can see and understand; a list per company or station can
  follow if players ask.

### D22 station-access decision (2026-10-02)

The owner approved the station-access proposal from PR #49 on 2026-10-02:
"i think we can do the station access permissions". This decides the station
access portion and extends it with per-company overrides; the other proposed
D22 topics above are not decided by this entry.

- Stations start open. A founded company's head controls its default and may
  allow or deny individual other companies. An explicit choice overrides the
  default; **Default** removes that choice. Newly founded companies follow the
  default. Access is per company, not per individual station or player.
- The existing head rule applies: founder while a member, then the longest
  standing member. The room's first company stays shared, with open stations
  and no head. A company always uses its own stations.
- Every replica checks the same permissions for new and changed line stops,
  and the line manager offers stations by the same rule. Existing services
  are not forcibly removed when permission changes. The controls must explain
  that the change applies when adding or changing a route.
- Station upkeep remains the owner's; vehicle costs and line income remain
  the operating company's. This permission never grants construction editing,
  demolition or use of another company's depots.

## D23 (proposed, 2026-09-30): a company's progression is its share of each town, by deliveries and rating

**Proposed, not decided: the owner (Juliansgith) approves or changes it.**

Asked for on 2026-09-30: "we need to split the population to rank up
mechanic based on two factors, company rating and cargo + passengers
delivered per town then after the split add it up and that is the
company's score for progression", made precise the same day: "it should be
more on company rating per town when summing them all up, after the split
multiply by company rating/100".

What the game does (investigation/TPF3_PROGRESSION_2026-09-30.md): its
growth script keeps one company, the save's player. Its experience is the
highest world population it has seen, every town's residents whoever
serves them; its rank is the game's thresholds on that. There is no rating
of a company: the rating is the town's (its authority score, the lowest of
six parts), one for everyone.

Proposed:

- **One company, the game's own.** With one company in the room (co-op)
  nothing changes: the game keeps its own score and rank, and a rank the
  company window takes goes to the game's growth script as its own event,
  in every game at the same update.
- **More than one: each town split.** Each company's score is the sum over
  the towns of

  `population x share x rating / 100`

  - *population*: the town's residents, as the game counts them for its
    own score;
  - *share*: the company's share of the cargo delivered to the town in the
    last half year and of the passengers travelling to and from it on
    lines (averaged over the same half year), both from the game's own
    statistics per line, each line the company's that owns it. The two
    shares are weighed cargo 1 : passengers 1 (`progression.WEIGHTS`), a
    kind nobody carries there left out;
  - *rating*: the company's rating in that town, 0 to 100: the game's town
    rating with the two parts a company earns itself taken from its own
    lines by the game's own formulas (its passengers' happiness, its
    cargo on time), and the town's other parts (reputation, traffic,
    noise, pollution) as they are, the same for every company.
- **As the game's.** The score is taken four times a game month; the
  experience is the highest score reached and never falls; the rank it
  reaches is the game's own thresholds, and a company takes a rank it
  reached through the company window, which then gives it the game's
  permits (prospecting among them). The room's first company begins from
  the rank it earned before there were two.
- **The same in every game.** It is computed in the mod's game script
  from the simulation's state only, at the same game time in every game,
  and each town's parts and each score are written to `hook.log`.

Open for the owner:

- Split this way, the companies' scores add up to at most the world's
  population times the ratings, while the game's thresholds are set for
  one company holding the whole world: with two even companies each needs
  roughly twice the world's growth for a rank. Scaling the thresholds by
  the number of companies, or not, is the owner's call; nothing is scaled
  now.
- The room's first company keeps the experience the game gave it, which
  is the whole world's; the others begin at nothing.
- The game's own rank-up notices and its ticket price bonus follow the
  save's player alone (the growth script's); the others' are not shown.

Rejected:

- **Two splits added up** (by rating, and by deliveries, each a share of
  the population): the request's second wording multiplies by the rating
  instead, so a well-served town that rates a company badly gives it
  little.
- **One rating per company over all towns**: the game has none, and the
  request is for the rating in each town.

## D24 (2026-09-30, proposed): the launcher's window opens with the lobby in the game

*Proposed for the owner (Juliansgith) to decide; not in force until
approved.* The user asked on 2026-09-30 for "a nice ui multiplayer button in
game, join a lobby from a multiplayer button in game, move away from having
all the lobby stuff in the launcher".

- **The game is where players play the lobby.** The main menu's
  Multiplayer cards and button open the window that connects, creates and
  joins rooms (with the save the room starts from, its rules, players and
  password), shows the players, chats, gets ready and starts (D17 as
  amended; LOBBY.md).
- **The launcher's window starts the game and shows where things stand.**
  Its big button starts Transport Fever 3 with the hook (D11 stays: the
  launcher is still the only way the hook runs), then follows the room's
  world; the rest shows the server, the room and its players read-only,
  the session log, the support code, updates and settings. It holds the
  connection, as before.
- **The page's lobby stays one click away** ("Lobby in this window
  instead"), and in the browser page (`--browser`) as it is, for a game
  whose menu the hook cannot reach, so that costs the player nothing
  (D17's amendment). The launcher's backend keeps every lobby action; the
  auto-room flags and tests use it as before.

This touches D20, which says the window shows in each state what the
page's `view.js` shows: by default it now shows less than the page
(`view::present_in_game`), and the page's states are what it shows with
the lobby in the window (`view::present`, rendered by
`tests/screenshots.rs` as before, next to the `g*` screens of the default).

Rejected:

- **Removing the lobby from the launcher**: a game update that moves the
  menu's code would leave players unable to play until TPF3-MP catches up.
- **A pause-menu Multiplayer entry**: in the room's game the game bar's
  Multiplayer window has the room, and a copy of the pause menu is one
  more game file to carry over on every patch.

**Revised proposal (2026-10-02, not decided; for the owner).** A player
(silver2127) asked on 2026-10-02 to "move all lobby management stuff to the
mp menu as that is working pretty well now", after rooms were created,
joined, readied and started from the game's Multiplayer window in several
real-game playtests (two and three games a room, competitive and co-op).
This revision replaces the second and third points above and reverses the
first rejected option:

- **Every lobby action lives in the game's Multiplayer window only.**
  Connecting and choosing the server, creating and joining rooms (the
  start save, rules, password, public listing), the players and their
  ready marks, chat, companies, the owner's start, leaving and kicking.
- **The launcher's window has no lobby.** It starts Transport Fever 3 with
  the hook (D11 stays), holds the connection, and shows read-only where
  things stand: the server, the room's name and players, the support code
  and log session, the session log, updates and settings.
- **A rescue, not a second lobby.** The launcher's lobby (the page's
  `view::present`) stays in the build but hidden. It shows by itself only
  when the hook reports that it cannot reach the game's menu (a game
  update moved it), and from a "Lobby in this window" switch in Settings,
  off by default. So the risk the rejected option named, players unable
  to play until TPF3-MP catches up with a game patch, stays covered.
- **The browser page (`--browser`) and the auto-room flags keep the whole
  lobby**, for tests and headless use; the launcher's backend keeps every
  lobby action.

The rejected option "Removing the lobby from the launcher" is reversed in
part: its window loses the lobby, but the backend and the rescue keep it.

## D25 (2026-09-30, *proposed*): players may differ in personal mods

*Proposed, for the owner (Juliansgith) to approve or refuse. Nothing here is
decided until then.*

The user, on 2026-09-30: "scan a lua mod for what functions it calls and so
long as it doesn't call any ones that could cause a desync, or if it does
changes we cancel and replay them anyways", and of a timetable mod:
"timetables would cancel and replay but only on the player's own vehicles,
we just have to intercept certain lua functions."

- A room's players must run the same **shared** mods, in the same version
  and order, as now. They may differ in **personal** mods: mods that only
  change what one player sees, whose every change to the world goes through
  `api.cmd` from the GUI, where the room's guard carries it to every game or
  refuses it ([MODS.md](MODS.md)).
- A static scan (`tpf3mp-modscan`) sorts each listed mod, failing closed: a
  mod it cannot read, or whose files it does not know, is shared. The room's
  content check compares the shared mods alone.
- Every game loads the room's world with the save's shared mods and its own
  player's personal mods, leaving out other players' personal mods; nothing
  is stripped from a save.
- A game-script mod whose game scripts act only through commands the room
  carries from them (a timetable mod, a line namer: **carried**) may be
  personal once the measurements in MODS.md ("To measure in the game") pass:
  its player's game alone runs it, its commands go to the room as actions
  for that player's own company's vehicles and lines, and every game
  applies them. Until then it is shared unless the player asks
  (`--personal-game-scripts`).
- `"cosmetic": true` in a mod's manifest decides nothing: mods that change
  the world say it too.

Rejected:

- **Trust the scan alone**: Lua reaches anything by a name built at run
  time; what the scan misses, the guards refuse.
- **Trust "cosmetic"**: see above.
- **Strip personal mods from the save before it is handed out**: the game
  writes a save's mod list natively (`GameSaveCommandData.modDescs`); every
  game choosing its own list at load needs no change to the save.
- **Let a personal game-script mod act in its own game**: it would change
  that world alone.

Touches: PLAN.md, Part 3, "a rule for mods that send commands from the GUI",
left open for the team: this proposes the guard's answer (carried, else
refused), which the owner settles. "The room's required mods from Mod Hub
IDs" stands. No "mods round" in which the host sends its mods to joiners was
found in the code or the docs of `dev` (2026-09-30); if one is planned, it
would send the shared mods only.

## D26 (2026-09-30, *proposed*): a room's owner may list it publicly

*Proposed for the owner (Juliansgith) to decide; not in force until
approved.* The user asked on 2026-09-30 for a room browser in the game's
Multiplayer window: "a scrollable list where the buttons are the map type
the host picks, shows the lobby name, number of players/number of
companies and what year it is".

- **Private stays the default.** A room is joined by the invite its
  members pass on, as D13 has it, unless its owner creates it **public**
  (`CreateRoom::listing`). Nothing about a private room is ever listed.
- **A public room's invite is public.** The server's list
  (`ListRooms`, 20 a page) gives each public room's invite, name, rules,
  players and limit, whether it has a password, its phase, and what its
  owner declares: the map's climate, the game's year and its companies.
  A password still guards a public room.
- **Kept in memory only.** The server stores a public room's invite beside
  the room, never in its log, so a room restored after a restart is
  private again, and its log gives no invite away (D13).
- **Bounded and rate-limited.** A page holds at most 20 rooms; a connection
  asks for one page a second, with a burst of five.

This narrows D13's "invites cannot be used to probe which rooms exist" to
private rooms: a public room is meant to be found.

Rejected:

- **Public by default**: invites were private until now, and players who
  shared one with friends did not agree to strangers joining.
- **Listing without the invite, joining by room id**: a second way into a
  room beside the invite, for the same result.

## D27 (2026-10-02, owner-approved for integration): a shared mod's follow-up build goes to the room from its player's game

*Approved for integration by the owner on 2026-10-02 when authorizing the
new PRs to be merged alongside telemetry.* The user asked on 2026-10-02 to get their mods Parallel Tracks
and Auto Signals working in a room, as a pull request to TPF3-MP. It
answers, for builds, the question PLAN.md (Part 3) leaves open for the
team: "a rule for mods that send commands from the GUI".

Some shared mods build after the player builds: Parallel Tracks lays
tracks beside the one drawn, Parallel Roads roads, Auto Signals more
signals after the first. Each hears the build in its game script
(`onPostBuildProposal`), which runs in every game, and builds from its GUI
half (`guiUpdate`) with `makeWorldBuildProposalCmd`. So **every game that
runs the mod sends the follow-up**, each from its own player's settings,
for whichever player built (seen in the game, 2026-10-02: both games sent
Parallel Tracks' tracks for one player's track). Today the hook stops each
of them, in every game alike: the mod does nothing in a room.

- **The follow-up of this player's build goes to the room from this game.**
  In the game scripts' GUI state, a script's build is carried, as the
  action the build tools' capture makes of it, when the last build this
  game applied was its own player's, at most a few frames before
  (`tpf3mp/modbuild.lua`, `FOLLOW_FRAMES`). The room orders it for every
  game, as a tool's click.
- **Another player's build is left to that player's game**, which sends
  its own follow-up from its own settings. A script's build with no build
  of the player's just before it is stopped.
- **Every script's build there is the hook's to stop**: it is marked
  `playerInitiated` whatever the script asked, so none builds in one game
  alone. A script that asked for `false` would otherwise build in its own
  game only (the hook lets builds that are not player-initiated through,
  as towns' growth).
- **What the build tools' capture does not carry stays stopped**:
  constructions, removals, stops and signals, for now. The narrow
  signal-only exception approved below carries a signal's parameters and
  the signals added or removed by an in-place track rebuild. It does not
  permit other removals or a mixed track modification.

*Amendment approved by the owner on 2026-10-08:* a shared mod may rebuild
an existing track, including one used by a line, **only to add or remove
signals**. The capture must pair every rebuilt track with its original and
verify that its nodes, geometry, type, template, owner, style, lock,
decorations and lanes are unchanged. It sends only the signal changes to
the room. A change it cannot prove signal-only is refused in every game;
other script removals and roadside stops remain stopped. This exception
allows Auto Signals without permitting a general rebuild of a lined edge.

Two players whose builds apply within one window may both hand a mod's
follow-up: the room orders both and every game applies both alike, a
duplicate or a collision the game refuses, never a world of one game alone.
This is as precise as the room can be while mods do not say which build
they follow.

Rejected:

- **Forward every game's follow-up**: one per player who runs the mod, each
  from a different player's settings.
- **The host's game alone sends follow-ups**: a guest's track would get the
  host's settings, or none.
- **Mods must change first**: the rule works for the mods as they are;
  a mod that wants to be exact may still build only for its own player's
  builds.

## D28 (2026-10-04, *proposed*): the room's owner picks its mods on the game's own pages, and members install what they lack from Mod Hub

*Proposed, for the owner (Juliansgith) to approve or refuse. Nothing here is
decided until then.* The user asked on 2026-10-04 for the lobby to show a
room's mods, what each player lacks, and to install missing Mod Hub mods
from it, "as native as possible"; they decided that a mod's settings travel
with the room and that Mod Hub mods are compared by the file installed.

- **The owner picks the room's save, its mods and their settings on the
  game's own Load Game page**, opened from the room: the page's details
  tabs (Mods, Gameplay Settings) are the game's, and what they hold for
  the save becomes the room's instead of loading it (LOBBY.md, "The
  room's save and mods"). The room's mods are that list, not the start
  save's; TPF3-MP's own is always among them, last.
- **The owner declares content and the room's mods together**
  (`DeclareRoom`, protocol 18): the manifest, and beside it what players
  are told of each mod (name, source, Mod Hub number) and the settings,
  the game's own included. The room takes it whole or refuses it, and
  tells every member (`RoomMods`), before refusing a join to a running
  game too. The content fingerprint stays the only gate for starting and
  joining.
- **Every game loads the room's world with the room's mods in the room's
  order and the room's settings**, then its player's personal mods (D25),
  adding a mod the save lacks when the owner picked it.
- **A member installs a missing Mod Hub mod through their own game and
  Mod Hub account**, on the game's own Mod Hub page of the mod, or asked
  once for all, showing what their own Mod Hub resolves for the number.
  Mods never pass between players, and the launcher never talks to
  mod.io. The owner's Mod Hub number is a claim: a mod installed for it
  counts only when its id is the room's.
- **A Mod Hub mod's version names the file installed** (its `revision`,
  `+m` and Mod Hub's file id), so two downloads of one revision with
  different files differ; one whose file cannot be read matches no other
  (fail closed).

Rejected:

- **The game's mod selector page in our own window**: it worked, but the
  Load Game page already holds the save, its mods and settings in the
  player's habits, and a second copy of the page drifts on every patch.
- **The owner loads the save and the room takes what loaded**: the
  owner's game would enter the world before the room starts.
- **Our own mod list instead of the game's**: duplicates the game's
  activation order, dependencies, severities and presets.
- **The launcher downloads from mod.io's REST API**: needs an API key in
  the package and the player's login; the game already holds both.

Touches: PLAN.md, Part 3, "The room's required mods from Mod Hub IDs; a
missing mod is installed from Mod Hub, never received from another player":
this builds it. Not covered yet: the new world path (a room started from a
new world keeps the mods the game's New Game page picks), and the game's
experimental economy settings (`configDict`), which do not travel.

## D29 (2026-10-06, owner-approved 2026-10-08): native mods come from a signed index, and the hook enables only what is built into it

The owner approved this policy on 2026-10-08. It covers CKAN-style
management of **native mods**: mods that are more than Lua and so cannot
live on Mod Hub or mod.io. Big Maps is the first (hook patches behind
switches, targets in the build's profile, a New Game page the hook serves, a
Lua mod folder, [BIGMAPS.md](BIGMAPS.md)). Today such a mod is switched on
by hand with environment variables on its own branches. The design is in
[NATIVE_MODS.md](NATIVE_MODS.md). PR #117 and this follow-up implement the
framework only; they do not provide a trusted production key or index, a
player-facing installer, Big Maps as a package, or native-mod room terms on
the protocol. Approval of the policy does not mark those implementation
items complete.

- **One signed index.** The launcher installs a native mod only when it is
  listed in `native-mods.json`, signed with a **native-mods key** of the
  project (Ed25519, with the same code and key handling as releases, D7):
  the public half is built into the launcher
  (`TPF3MP_NATIVE_MODS_PUBLIC_KEY`), and a launcher built without one
  installs none. As with D19's dev key, the key is its own: it cannot sign
  a release, and a release key cannot sign the index. Signing is a workflow
  in an environment whose required reviewer approves each index, as for
  releases. Each index has a serial that only grows, so an older signed
  index cannot be replayed to bring back a withdrawn package.
- **Every entry pins what it may be.** A package names the game builds it
  runs on by the executable's SHA-256, as hook profiles do; every file with
  its size and SHA-256; its dependencies and conflicts; the hook features it
  enables; its settings and their defaults; and whether it changes the
  simulation. An index with one malformed entry is refused whole.
- **The launcher installs into its own folder.** Packages go to
  `native-mods/<id>/<version>/` in the launcher's data folder, never the
  game's. Every file is checked against the index as it downloads, and a
  version goes into place only once all of its files check out; the
  version before an upgrade is kept for a rollback. A registry names every
  file installed, and uninstalling deletes exactly those (D7's journal, for
  mods). Installing is not enabling: the player switches a package on.
- **The hook enables only what the launcher enabled, on the build it was
  enabled for** (D11). The launcher names the enabled packages' file in the
  game's environment (`TPF3MP_NATIVE_MODS`), written for the executable it
  starts; no other switch turns a native feature on. A feature runs only if
  this hook has it and the matched profile has every target it patches. A
  package that changes only what one player sees is left off with a reason
  when it cannot run; one that changes the simulation and cannot run keeps
  the game out of rooms (fail closed, as for a build without a profile).
- **First version: no new native code.** A package only *enables* features
  compiled into TPF3-MP's hook (a registry of feature ids,
  `features::BUILT_IN`) and ships data, Lua and settings. The index already
  has a place for **signed plugin libraries** (`plugins`, with an
  interface version), which this launcher refuses; loading them is a later
  decision.
- **In a room, simulation-changing native mods are part of the room's
  terms**, with their exact versions and settings, compared as the room's
  content is. A member who lacks one, or holds another version, installs
  the room's version from the signed index in one click, never from another
  player. A package that changes only what a player sees is the player's
  own (as personal mods, D25). Until the protocol carries these terms, a
  simulation-changing package refuses multiplayer (PLAN.md, Part 3: a
  channel not checked yet is refused).

Rejected:

- **Open URLs, as CKAN does** (anyone publishes a metadata file pointing at
  their download): native code from anywhere, run inside every player's
  game. D4's servers are trusted because the project runs them; code that
  runs beside them must be the project's too.
- **Native mods through mod.io or Mod Hub**: Mod Hub carries Lua mods for
  the game's own loader; the launcher does not talk to mod.io (D28), and a
  mod.io file is not signed by the project.
- **Players passing mods to each other**, as a room's owner sending what
  a member lacks: a member would run what another player's machine handed
  them. D28 refuses it for Lua mods; native mods have more reason to.
- **Unsigned packages over HTTPS**: the reason D7 rejected unsigned updates.

Touches: D7 (the same signing primitives, a second key for a second kind of
artifact), D19 (the per-purpose key it set the pattern for), D11 (the
launcher decides what the hook does in the game it starts), D28 (covers Mod
Hub mods only; native mods are a separate channel and change nothing
there), D4 (trust in the project, fail closed). PLAN.md, Part 3: a new item.
Not covered yet: the room's terms on the wire (an additive field of the
content declaration, with a protocol version bump), the launcher's page for
native mods, and Big Maps itself as the first package.

## D30 (2026-10-11): ReedME fork review implementation

ReedME authorized carrying out all items in the fork's
`investigation/REEDME_DEEP_REVIEW_2026-10-11.md`. This entry applies to the
ReedME fork; earlier owner decisions and contributor provenance remain
upstream history.

- Windows game builds through Proton are the initial client target.
  The portable Windows ZIP uses a transactional Rust mod installer and
  native process checks. This is a scoped D9 exception because Wine's
  PowerShell stub returns success without executing scripts. Native Windows
  keeps readable scripts, whose installed bytes and record must verify.
  Managed standalone installation is refused clearly under Wine.
- Native company loans use the game's fixed principal and full-term
  interest, with per-loan payment origins. Legacy annuity loans preserve
  outstanding principal and existing journals; a one-time remaining-term
  schedule transition starts at the current game time and exposes those
  exact terms in the finance window. It creates no money and charges no
  periods before that transition.
- New rooms imported from multiplayer saves discard previous-room company
  password seals with a visible reset notice. Same-room reconnect, reload
  and restoration retain seals; company membership, assets and station
  permissions remain. New passwords can be set by company heads afterward.
  Locked legacy saves without stable room provenance reset once on first
  upgraded room load with a distinct legacy notice; unknown identity never
  triggers a reset.
- Checkpoint/save reports identify their producing world. Stale background
  completions cannot affect another world, and snapshot retention and
  pointer persistence protect live/recoverable worlds.
  Per-lane votes without any complete matching replica promote no save. Keep
  the last agreed world and report this recovery limit; restoring that world
  requires owner intervention. Do not invent a composite world or silently
  select an arbitrary divergent replica.
- The fork uses its own repository/update trust and server images.
  Unconfigured developer launchers use loopback, never upstream production.
  Public packages need deliberately configured operated servers. Native
  archive verification and protected signed updates remain release gates.
- The first competitive scope is trusted players on identical Windows game
  builds. Native sequencing is not an authoritative anti-cheat economy.
  No real-game or public-release acceptance is implied by synthetic tests.

This authorization covers focused fixes, regressions, local test tooling
and fork governance. It does not replace missing game inputs with fabricated
verification or authorize publishing an unverified release.
