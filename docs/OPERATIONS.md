# Operations

How to run `tpf3mp-server` on a Linux host. The original deployment examples
below describe upstream's German relay host; they are not an operated
ReedME service or permission to modify upstream infrastructure. The fork
uses its own configured servers, repository, images and signing roots.
Current integration is protocol **24**, bridge **30**, action schema **28**
and room-log format **11**; see "Upgrades" before opening old room data.

## What the server needs

- **UDP port 29470** open to the internet: players connect with QUIC.
- **HTTPS on port 443** through the host's reverse proxy, for players whose
  networks block UDP (see [Tunnels](#tunnels)). Optional, but some school,
  office and hotel networks leave no other way.
- **A TLS certificate** for a hostname that points at the server, such as
  `tpf3mp.<ip>.sslip.io`. Agents verify it against public certificate
  authorities, exactly as a browser would.
- **A data volume** holding:
  - `/data/invite.key`: losing it invalidates every invite, including those
    of restored games.
  - `/data/rooms/`: one log per running game, so games survive restarts,
    and a pointer to each game's current snapshot.
  - `/data/rooms/snapshots/`: the world snapshots, deduplicated chunks of
    the games' saves. At most 64 GiB by default (`--snapshot-gib`).

  Back up the key, logs and complete snapshot directory together. A missing
  agreed snapshot can block late joins or restoration; a new save requires
  a connected replica that still holds that world.

## First deployment

1. Point a hostname at the server, and open the port:
   `ufw allow 29470/udp`.
2. Get a certificate for that hostname (see [Certificates](#certificates)).
   Place `fullchain.pem` and `privkey.pem` in `deploy/certs/`, readable by
   UID 65532, the image's non-root user:
   ```sh
   sudo chown 65532:65532 deploy/certs/*.pem
   sudo chmod 0440 deploy/certs/*.pem
   ```
3. Build and start the server:
   ```sh
   cd deploy && docker compose up -d --build
   ```
4. Check it from any machine:
   ```sh
   tpf3mp-agent connect tpf3mp.example.org:29470
   ```
   This prints the server version, a session's support code and the
   round trip.

### Beside tf2mp-relay

The project's own server runs TPF3-MP next to tf2mp-relay and other
sites, behind the host's nginx, whose certificates certbot keeps. TPF3-MP
shares nginx and certbot and nothing else: its own folder, its own
Compose project (`name: tpf3mp`, so it never mixes with the relay's
`deploy` project), its own hostname and ports.

1. **The code:** `git clone
   https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod.git
   /opt/tpf3mp`, at the commit `main` holds.
2. **The certificate:** `certbot certonly --nginx -d <host>`. Certbot's
   nginx plugin answers the challenge; no site changes.
3. **The tunnel's virtual host:** `deploy/nginx.conf.example`, with the
   hostname put in, as `/etc/nginx/sites-available/tpf3mp.conf`, linked
   from `sites-enabled`. Then `nginx -t`, and only if it passes,
   `systemctl reload nginx`, which leaves every site's connections
   standing.
4. **The certificate for the server,** now and after every renewal:
   `deploy/certbot-deploy-hook.sh`, with its `HOST` and `DEPLOY` set, as
   `/etc/letsencrypt/renewal-hooks/deploy/tpf3mp.sh`. Run it once by hand,
   with `RENEWED_LINEAGE=/etc/letsencrypt/live/<host>`.
5. **The port:** `ufw allow 29470/udp`.
6. **Start:** `cd /opt/tpf3mp/deploy && docker compose up -d --build`;
   later, `tpf3mp-ctl deploy` (see "Developer access").
7. **Check** from another machine, as above, and through the tunnel with
   `tpf3mp-agent connect <host>:29470 --tunnel-only`. The relay's own
   checks must still pass.

### Developer access

TPF3-MP's developers get an SSH account each on the project's server
that can reach TPF3-MP and nothing else there: not the host's other
sites, files, processes or ports, and not Docker, whose group would make
them root. Their logins run `deploy/host/tpf3mp-dev-shell`, which hands
the words they type to `deploy/host/tpf3mp-ctl`, run as root through one
sudo rule. It checks every argument and runs no shell:

```sh
ssh <you>@<server> status                 # the container, health, deployed commit
ssh <you>@<server> logs --since 2h        # --tail 200, --follow
ssh <you>@<server> logs --grep K7QM2X     # only a session's lines; or a player's (p-…) or room's (r-…)
ssh <you>@<server> rooms                  # the rooms open now, with their players
ssh <you>@<server> rooms --invite K7QM2X  # the room a player's invite opens
ssh <you>@<server> rooms --name ann       # the rooms of players named like ann; --player p-… one player's
ssh <you>@<server> diagnostics K7QM2X     # a player's diagnostics, or all
ssh <you>@<server> diagnostics AB2CD3 hook  # one source of a run's
ssh <you>@<server> diagnostics --name ann   # the sessions of players named like ann
ssh <you>@<server> metrics
ssh <you>@<server> deploy                 # build and run main
ssh <you>@<server> deploy 3ad6364         # an earlier commit of main
```

`deploy` fetches `main` from the project's repository and builds only
commits on it, which passed every check; one deploy runs at a time. SSH
refuses those accounts a shell, a terminal, file transfer and any
forwarding, and each use is logged to syslog as `tpf3mp-ctl`, with the
developer's name (`journalctl -t tpf3mp-ctl`), and an invite they looked up
as `<invite>`: it lets anyone into its room.

- **Setting it up,** once, and after any of its files change:
  `sh deploy/host/install.sh` as root. It checks the sudo rule with
  `visudo` and the SSH rules with `sshd -t`, and keeps the SSH rules only
  if root's effective settings did not change.
- **Adding a developer:** `tpf3mp-add-dev <name> <file with their SSH
  public key>` as root. The account has no password, and its home, which
  holds only the key, belongs to root.
- **Taking one out:** `userdel <name>` and `rm -r /home/<name>`.

## Certificates

The server reads its certificate at start. After a renewal, restart it:
`docker compose restart`. Two options:

- **certbot.** Use `certbot certonly --standalone -d <host>` while port 80
  is free, `--nginx` or `--webroot` behind the existing reverse proxy.
  `deploy/certbot-deploy-hook.sh` is the deploy hook: it copies the
  renewed files into `deploy/certs/`, fixes their owner and restarts the
  container (see "Beside tf2mp-relay").
- **Reuse the existing Caddy.** Add the hostname to the Caddyfile so Caddy
  obtains the certificate, then copy it from Caddy's storage
  (`certificates/acme-v02.api.letsencrypt.org-directory/<host>/`) with a
  small scheduled job. Caddy's files are root-only, so a copy with the right
  owner is required; do not mount them directly.

For local development, `--dev-self-signed <file>` writes a throwaway
certificate that agents pin with `--pin-cert <file>`.

## Monitoring

- **Metrics.** `http://127.0.0.1:9470/metrics` serves Prometheus text on the
  host: sessions, rooms and tunnels now, plus counters for handshakes
  refused, protocol violations, turns sealed, events ordered, intents
  refused, divergences, slow consumers, log compactions and tunnels opened
  and refused, and for snapshots: saves, snapshots agreed, failed uploads,
  late joins, rebases and bytes served.
- **Alerts.** `deploy/prometheus/tpf3mp.rules.yml` holds alerting rules for
  a Prometheus that scrapes the endpoint: the server down, replicas
  diverging, slow consumers, saves not arriving, handshake floods, protocol
  violations and refused tunnels.
- **Health.** `/healthz` returns `ok`.

## Logs

The server logs to standard output, and Docker keeps the log: at most ten
files of 50 MB each (`logging` in `compose.yaml`), so it never fills the
disk. The log never contains IP addresses or invites.

- **Following it:** `docker compose logs -f`.
- **One player's session.** Every session starts with a line naming its
  support code (`session=K7QM2X`) and the player (`player=p-…`); the
  launcher shows the player their support code. `docker compose logs |
  grep session=K7QM2X` shows that session; the player ID shows all of that player's sessions,
  and a room ID (`r-…`) the room's life.
- **A room, by its invite.** A player quotes the invite of a room that
  misbehaves: `curl http://127.0.0.1:9470/rooms?invite=K7QM2X` gives that
  room (404 when none open now has it), with its ID, name, phase and
  players (ID, name, connected). `/rooms` lists every room open now,
  `?player=p-…` and `?name=ann` (any case) those of a player. The invite
  itself is never written to a log; the room's ID is: grep for it.
- **For a bug report:** `./collect-logs.sh` in `deploy/` writes one
  archive: the log of the last 24 hours (`--since 2h` for another window,
  `--for <ID>` for one session, player or room), the container's state
  (restarts, out of memory), `/healthz`, `/metrics`, and the host's disk and
  memory. It holds no secrets and can be shared.
- **The player's side, without asking:** players' launchers send the
  lines of their logs, redacted, to the server they play on (unless the
  player switched that off): the launcher's own, and under the approved
  D10 amendment the in-game hook's `hook.log`, the game's `stdout.txt` and
  the text of the game's error reports, never its crash dumps. Every line
  names its source (`launcher`, `agent`, `hook`, `game`, `crash`) and the
  player's *log session*, the code of their launcher's run, which the
  launcher and the game's Multiplayer window show next to the support
  code. With either code a player quotes:

  ```sh
  curl http://127.0.0.1:9470/diagnostics/K7QM2X    # a session's lines, by support code
  curl http://127.0.0.1:9470/diagnostics/AB2CD3    # a whole run's, by log session
  curl 'http://127.0.0.1:9470/diagnostics/AB2CD3?source=hook'   # one source's
  curl http://127.0.0.1:9470/diagnostics           # the sessions with some
  curl 'http://127.0.0.1:9470/diagnostics?name=ann'          # whose name holds "ann", any case
  curl 'http://127.0.0.1:9470/diagnostics?player=p-3f2a91c0d4e5b6a7'  # one player's
  ```

  (`ssh <you>@<server> diagnostics AB2CD3 hook`, `diagnostics --name ann`
  or `diagnostics --player p-…` on the project's server.) The list names
  each session's player by ID and by the name their launcher gave, the
  one the lobby shows. A session's or run's lines start with a `who` line
  naming its sessions and their players, then one JSON object a line: when
  the server received it, the player's time, the player and their name,
  the run, the source, level, where it was logged (or the file it was
  read from) and the line. **Names are not unique and can change**: two
  players may share one, and one player may rename between runs. The
  player ID (`p-…`) is the stable link: find a name's sessions, then
  follow the player ID. Paths, addresses, invites, keys,
  account IDs and e-mail addresses are taken out on the player's machine
  and again here. At most the newest 64 MiB of one session or run come
  back at once. They are kept in `diagnostics` inside the data volume,
  64 MiB a session at most (`--diagnostics-session-mib`), for 30 days
  (`--diagnostics-days`, 0 for none) and within 1 GiB in all
  (`--diagnostics-mib`), the oldest going first; `diagnostics/runs/`
  indexes each run's sessions. Without `--data-dir`, none are kept. They
  are personal data of your players, named by the names they play under:
  keep the retention short, and delete a player's on request: find their
  sessions with `?player=p-…` and `rm` each session's file. The metrics
  `diagnostics_kept_total` and `diagnostics_dropped_total` count lines.
- **From the player:** their support code or log session is enough for
  the launcher's, the hook's and the game's logs: they are already here,
  under Diagnostics. For the game's crash dumps, which are never sent, or
  the logs from before the launcher started, ask for the zip
  `tpf3mp-agent collect-logs` writes. Its `manifest.txt` names the
  player's versions, system and support code, and lists the launcher's, the
  hook's and the game's logs it holds (see "The game's own logs" in
  [PLAYING.md](PLAYING.md)). Grep your log for the support code to put the
  two side by side.
- **For a log collector** (Loki, Elasticsearch, …): set
  `TPF3MP_LOG_FORMAT: json` in `compose.yaml` (`--log-format json`), for
  one JSON object per line with the same fields.
- **More detail:** `RUST_LOG=debug` adds per-connection refusals;
  `RUST_LOG=tpf3mp_server=debug,quinn=warn` narrows it.

A rise in `divergences_total` means replicas disagree with verdicts: look
at the platforms involved. A rise in `slow_consumers_total` means clients
cannot keep up with their turn streams. `uploads_failed_total` rising
while `snapshots_agreed_total` stands still means players' saves do not
reach the server: late joiners then wait.

## Upgrades

The 2026-10-20 ReedME refresh uses protocol 24, bridge 30, action schema 28
and room-log format 11. This follows the fork's protocol-20 / bridge-29 /
format-9 repair candidate and upstream's protocol-23 / bridge-29 / format-10
branch; those reused numbers do not make their layouts compatible. There is
no old-log migration. Preserve the old server binaries, secret, logs and
complete snapshot directories together, use fresh data and snapshot
directories for this fork, and start fresh rooms. Every earlier room-log
format, including 8, 9 and 10, is refused without rewriting it. Startup checks every log before opening the snapshot store or running
recovery cleanup. Unidentified first-record formats also refuse startup;
archive or repair those files deliberately, preserving the complete old data
directory. Do not point the candidate at live upstream room data. Importing a game
save into a fresh room applies the documented company-password reset policy.
The resume upgrade check below applies only to compatible persisted formats;
it cannot certify this incompatible transition as preserving running rooms.

Before promoting a release that changes the protocol or persistence, run
the check with the published player/server binaries and the candidate binaries:

```sh
TPF3MP_CHECK_OLD_CLIENT_EXPECTED=update \
  tools/acceptance/upgrade-check.sh <old bin dir> <new bin dir>
```

Use `connect` instead of `update` when the protocol is unchanged. The check uses
two local launcher APIs and fake games to start a room on the old version,
save it, restart the server, and verify both players resume the same world.
It needs the loopback admin port and two consecutive launcher HTTP ports;
`TPF3MP_CHECK_UI_PORT` changes the first launcher port from its default 47471.

The combined protocol 24 / bridge 30 adds exact room agreement for the
experimental native scanner alongside producing-world acknowledgements;
it requires an updated agent, hook and server together. Format-11 room
starts persist mode, parts and stride. Earlier formats are never recovered
through upstream's legacy readers in this fork. Existing native-mod
validation remains in force. A restored
room cannot silently switch scan algorithms. Native modes remain opt-in,
with no hard per-update latency bound; all members must start the launcher
and game with the same valid scan environment settings. The default `off`
path remains rolling-v1. A scanner mismatch on join or reconnect is refused
before replacing a seat or loading a world.

Every player must run the server's protocol version. The handshake tells
players on another version which side to update, and their launchers
update themselves. Upgrade the server when a release is published, which
also publishes its image, `ghcr.io/reedme/tpf3mp-server:<version>`
and `:latest` (`.github/workflows/image.yml`):

```sh
cd deploy && git pull && docker compose pull && docker compose up -d
```

To pin a version, and roll back by changing it, put
`TPF3MP_VERSION=0.2.0` in `deploy/.env`. The first time, the image may be
private to the repository's owner: make the package public on GitHub
(the repository's Packages, Package settings), or `docker login ghcr.io`
on the server. To run what is checked out instead of a published image,
build it: `docker compose up -d --build`.

Tell players first, from the host:

```sh
curl -X POST http://127.0.0.1:9470/announce -d "Restarting for an update in 5 minutes"
```

Every launcher shows the notice (up to 280 bytes) until the next one, in a
room or not. The admin endpoint answers how many connections were told.

What happens during the restart:

1. The old container gets SIGTERM and closes every session with
   `SHUTTING_DOWN`.
2. With `--data-dir` (the image's default), every running game has been
   logged turn by turn. The new server restores those games at start.
3. Players reconnect with the same identity and resume after the last turn
   they applied. The event log continues without a gap. Lobbies that had not
   started are not kept. If the restored room sends a replacement world while
   the game finishes an earlier load, the launcher waits for the replacement
   to finish before reporting that the game is ready.
4. A restored game that nobody reconnects to within 10 minutes closes and
   its log is deleted, like any running game whose players all disconnected
   (see [Room lifetime](#room-lifetime)). `--abandon-after-mins` sets both:
   raise it to keep games for players who come back another day. Such a
   game holds its room slot, counted against the address that created it,
   until it closes.

Persistence details:

- **Crash safety.** Each turn is written to the operating system as it is
  sealed, so a process crash loses nothing. A power loss or kernel crash can
  lose the last few turns; a client that saw them is told
  `ResumeUnavailable`, even once the room has sealed new turns with the same
  numbers (see "Histories" in PROTOCOL.md). Logs of an older format are
  set aside, not restored.
- **Damaged logs.** Recovery reads a log without changing it. A damaged final
  record is what a crash leaves behind, so it is cut off once the room is
  rebuilt. Any other damage leaves the log exactly as it was, renamed to
  `*.broken` (or `*.1.broken` and so on, never replacing an earlier one) and
  kept for diagnosis. Symbolic links are ignored.
- **Compaction.** Once a game's log passes 64 MiB (`--compact-log-mib`),
  it is rewritten to start from where the game stands: the canonical
  rules' saved state, plus the last hour of turns for players who resume.
  It is compacted again each time it grows by as much, or by its compacted
  size if that is more. A restart then replays only the turns after the
  rewrite. The new log is written and flushed under
  `<room>.log.compacting` and replaces the old one only when complete, so
  a crash during compaction leaves the old log, and the next start deletes
  the leftover.
- **Size limits.** A log that cannot be compacted, because its rules
  cannot save their state, stops growing at 1 GiB; the game continues but
  would not survive a restart. Each player may send 32 KiB of commands per
  second, with a 256 KiB burst, so an honest game takes days to get there.
  Recovery streams a log and holds at most the last 64 MiB of turns per
  room in memory.
- **Permissions.** On Linux, logs are readable by the server's user only:
  they hold invite and password tags and every command.
- **Invite key.** Restored games are rejoined with their original invites,
  which only verify with the same `invite.key`.
- **Rules.** Each game's log records the rules its host chose. A server that
  no longer offers those rules sets the log aside rather than restoring
  the game with others, so keep offering rules while games use them.
  `native`, the game's own economy, is always offered.

## Room lifetime

A room closes when nobody is left to play it:

| what happens | when the room closes |
|---|---|
| the last member of a lobby leaves or disconnects | at once: a lobby holds no seats |
| the last member of a running game leaves (**Leave room**) | at once |
| nobody is connected to a running game any more (games closed, launchers gone, network down) | after the grace period, unless a player returns |
| a game restored at start, until a player rejoins it | after the grace period |

The **grace period** is 10 minutes by default (`--abandon-after-mins`,
whole minutes, at least 1). It is what lets a player whose game crashed
or whose network dropped rejoin, and what carries running games across a
server restart; the launcher automatically retries for up to five minutes. A player who
rejoins in time keeps the game going, and the grace starts over the next
time everyone is gone.

While nobody is connected, the room stays out of the public room list
(its own players rejoin by invite), and the server logs
`nobody is connected to the game; it closes unless a player returns in
time` with the grace in seconds. When it closes it logs `closing a game
nobody returned to` and `room closed`, counts `rooms_abandoned`, leaves
the room list, frees its slot against the address that created it, and
deletes its log, its snapshot pointer and the snapshots only it held.
A restart after that restores nothing of it.

A disconnect the room was not told about, because its queue was full,
counts too: every tick the room lets go of seats whose connection has
closed, so no room waits forever for a player who is gone.

## Tunnels

Some networks let nothing but HTTPS out. Players there reach the server
through a WebSocket tunnel that carries the same QUIC connection, end-to-end
encrypted as always (see "Tunnels" in PROTOCOL.md). Agents try UDP first and,
after 3 s without an answer, also `wss://<server host>/tpf3mp`; whichever
connects first is kept.

The image listens for tunnels on TCP 29471 as plain WebSocket and takes each
player's address from `X-Forwarded-For` (`--tunnel-listen 0.0.0.0:29471
--tunnel-behind-proxy`). The compose file publishes that port on the host's
loopback only, for the reverse proxy that already runs `tf2mp-relay`'s
hostname. Add the TPF3-MP hostname to it:

```caddyfile
tpf3mp.example.org {
    handle /tpf3mp {
        # Overwrite, never trust, the client's own forwarding chain.
        reverse_proxy 127.0.0.1:29471 {
            header_up X-Forwarded-For {remote_host}
        }
    }
    handle {
        respond 404
    }
}
```

or with nginx:

```nginx
location = /tpf3mp {
    proxy_pass http://127.0.0.1:29471;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header X-Forwarded-For $remote_addr;
    proxy_read_timeout 120s;
}
```

Without a reverse proxy, the server can serve the tunnel's TLS itself with
its own certificate: `--tunnel-listen 0.0.0.0:443` without
`--tunnel-behind-proxy`. Another path is `--tunnel-path`; players then pass
the whole URL with `--tunnel`.

- **Limits.** Tunnels count against the same per-address limits as UDP
  sessions, by the player's address. At most a session's and a
  handshake's worth of tunnels per address, and as many in total as
  sessions and handshakes together, are open; a player over the share is
  answered `429`. A TLS and WebSocket handshake must finish within 10 s,
  and a tunnel must carry a QUIC connection within 10 s of opening, and
  closes 10 s after its connection ends, so holding one without playing
  gets nowhere. Browsers are refused, so a web page cannot open tunnels
  from its visitors' machines.
- **File descriptors.** Each tunnel is an open connection. The compose
  file raises the container's open-file limit to 65,536; outside Docker,
  raise it for the server's service the same way (`LimitNOFILE=` in a
  systemd unit), or rooms may fail to write their logs under load.
- **Trust.** `--tunnel-behind-proxy` believes the last `X-Forwarded-For`
  entry, and only from a proxy connecting from loopback or a private
  network, where a reverse proxy on the same host or in a container sits.
  `--tunnel-proxy <address or range>`, repeated as needed, narrows that to
  your proxy's own address. Requests without the header are refused.
- **Behind a CDN** the proxy's `{remote_host}` is the CDN's edge, and every
  player would share a few edges' limits. Have the proxy forward the
  CDN's client-address header instead, such as Cloudflare's
  `CF-Connecting-IP`: `header_up X-Forwarded-For {http.request.header.CF-Connecting-IP}`,
  and accept connections only from the CDN's ranges.
- **Metrics.** `tunnels` (open now), `tunnels_opened_total` and
  `tunnels_refused_total`.
- **Players.** `--tunnel <url>` names another tunnel, `--tunnel-only` skips
  UDP, `--no-tunnel` never falls back. The launcher takes the same flags and
  shows "connected via tunnel" when the fallback was needed.

## Snapshots

With `--data-dir`, the server keeps world snapshots in `snapshots/` inside
it (`--snapshot-dir` puts them elsewhere, `--no-snapshots` turns them off).
They let players join a game that has started, rejoin one they can no
longer resume, and repair replicas that diverged. "Snapshots" in
PROTOCOL.md describes the flow.

- **When games save.** Every 10 minutes of play (`--save-every-secs`),
  sooner when a player waits for a world, never twice within a minute
  (`--save-gap-secs`). Every player's game saves at the same step, which
  the players see as a short pause, like an autosave.
- **Traffic.** One player uploads each save; successive saves share most of
  their chunks, so only what changed moves. A player who joins downloads
  the whole world once, then only changes. Up to 32 transfers run at once.
- **Disk.** Each game keeps its current snapshot and the one before; chunks
  both share are stored once. Closed games release theirs, and at start the
  server releases snapshots of games that are gone.

## Big maps

A room waits 20 seconds for a game that stops advancing and 5 minutes for
one loading its world, then plays on and lets that game catch up. Big maps
outgrow both: on TPF2 a big-map save paused the game for 15-20 s and
entering such a world took 4-5 minutes ([BIGMAPS.md](BIGMAPS.md)). For a
server that hosts them, raise both:

```sh
tpf3mp-server ... --stall-timeout-secs 60 --load-timeout-mins 15
```

Longer waits also mean a frozen game holds its room up for longer before
the others play on. Their saves are larger too: a 1.4 GB world must upload
within the 90-minute limit, so at least about 260 KB/s from the player who
uploads it.

## Native mods

A server can offer its players native mods such as Big Maps
([NATIVE_MODS.md](NATIVE_MODS.md), "A server's own index"; a proposed
amendment to D29). Name a registry, a git repository whose `mods.txt` lists
the mods' own repositories:

```sh
tpf3mp-server ... --native-mods-registry https://github.com/<owner>/<registry>   --native-mods-every-minutes 60
```

The server reads the list and each mod's `tpf3mp-native.json` when it
starts and again every hour (at least every five minutes), and serves the
index made of them to every launcher that connects. A mod that cannot be
read, or is malformed, or claims another mod's id is left out with a
warning in the log; a registry that cannot be read keeps the last index.
For a GitHub registry it also reads the open pull requests and serves the
mods they would add as *proposed*, which players see only when they ask
and install only by choosing them. For each mod on GitHub it reads the
mod's releases too (and, with none, the repository), for the downloads and
dates the mod browser shows. Each reading takes one or two of GitHub's API
calls per mod and one for the pull requests; GitHub lets an unnamed caller
make 60 an hour, so with more than a couple of dozen mods give a token
(`TPF3MP_GITHUB_TOKEN`, read-only, public repositories). Without it the
mods are still served, and those GitHub refused show no downloads. Without
`--native-mods-registry` the server offers none.

Players trust a server's index as they trust its rooms: the launcher gets
a room's native mods by itself only from a server the release lists; from a
server a player set in Settings, only when they ask.

## Before the first release

Once, in this order. The update key and the server are built into the
launcher, so both must be set before the build players download: a
launcher built without the key never updates itself, and its players would
download every later version by hand; one built without the server would
have them type one. So the release workflow drafts no release without
either.

1. **Protect the branches:** run `tools/github/protect-branches.sh` as a
   repository administrator (see "What GitHub enforces" in
   [AGENTS.md](../AGENTS.md)).
2. **Set up the update key:** the key, the `release` environment with its
   secret `TPF3MP_UPDATE_SIGNING_KEY`, and the variable
   `TPF3MP_UPDATE_PUBLIC_KEY`, as under "Updates" in
   [Releases](#releases).
3. **Deploy the server** ([First deployment](#first-deployment), and the
   Caddy block under [Tunnels](#tunnels)), and check it from another
   machine with `tpf3mp-agent connect <host>:29470`.
4. **Set the variable `TPF3MP_DEFAULT_SERVER`** to its `host:port`: the
   server every player plays on by default (D12 in
   [DECISIONS.md](DECISIONS.md); under its proposed amendment players may
   change it in Settings). For the project's relay that is
   `tpf3mp.213-133-98-90.sslip.io:29470`, with `TPF3MP_SERVER_NAME` `EU`.
   For a regional release (D12, approved 2026-10-08), also set
   `TPF3MP_SERVERS`; see
   [More than one server](#more-than-one-server).
5. **Rebuild the draft:** re-run the latest `release` run of `main` (in
   Actions), or promote a new commit to `main`. It stops, and makes no
   draft, while `TPF3MP_UPDATE_PUBLIC_KEY` or `TPF3MP_DEFAULT_SERVER` is
   missing.
6. **Try the draft's packages:** on each platform, download the package
   from the draft, start the launcher and connect; it names the server by
   itself, and connects there without being told.
7. **Publish the draft,** then approve the `sign` run waiting in Actions:
   the release gets its `release.json` and `release.json.sig`, which
   launchers update from. Publishing also starts `image.yml`, which
   publishes the server image; the first time, make the package public
   (the repository's Packages, Package settings), or servers must
   `docker login ghcr.io` to pull it.

From then on, releasing again needs a version bump in `Cargo.toml`, on a
feature branch like any change, and launchers of earlier versions update
themselves once the new release is published and signed.


## More than one server

Under D12 (approved 2026-10-08), a release may list several operated
servers, such as the project's relay in Europe and a second one in America.
Launchers then show the public rooms of all of them, each with its server
and ping, host new rooms on the closest, and resolve a typed invite only
after every listed server confirms exactly one credential match. The
servers know nothing of each other. This is a deployment guide: this code
change has not configured or deployed regional servers or changed the
live relay. Regional clients and servers must all run protocol 24 together.

For each further server:

1. **Deploy it as the first** ([First deployment](#first-deployment)),
   with its own `invite.key`, data directory and admin token: nothing is
   shared with the other servers. It must run the same release as them.
2. **Give it a certificate from a public authority** for the name players
   reach it by, as the relay has: Let's Encrypt for its host name (a
   VPS's own name, or an sslip.io name of its address). Launchers trust
   servers by the public authorities alone; a self-signed certificate is
   refused. Open UDP 29470 (and TCP 443 for the tunnel, under
   [Tunnels](#tunnels)), and check it from another machine with
   `tpf3mp-agent connect <host>:29470`.
3. **Add it to the repository variable `TPF3MP_SERVERS`** (Settings,
   Secrets and variables, Actions, Variables): `NAME=host:port`, more
   than one separated by commas, such as `US=<host>:29470`. Names are 1 to
   24 letters, digits, spaces, dots, dashes or underscores, and are what
   players see. A release has at most eight servers including the default;
   names and addresses must be unique. Before packaging, the release
   workflow validates these variables with the same parser the launcher
   uses, including the host and port, so an invalid entry stops the build.
   The default server stays in `TPF3MP_DEFAULT_SERVER` and
   `TPF3MP_SERVER_NAME`, and is listed first. The release workflow refuses
   a list that does not read.
4. **Release**: only packages built with the variable know the server.
   Launchers of earlier releases keep playing on the default server alone.
   Every listed server must be upgraded to the same protocol-24 release
   before publishing that regional client package.

A server that is down is passed over for hosting. A typed invite waits up
to five seconds for every trusted region to be reachable, then probes them
concurrently with a five-second per-region request timeout. A missing,
timed-out or rate-limited answer makes the lookup incomplete; no room is
joined or primary connection promoted, and the player is asked to retry.
No private-room metadata is returned. A unique positive lookup is followed
by one authoritative JoinRoom which checks the credentials again. An
explicit trusted-region choice or public room card goes directly to that
region without probing the others.

When a player opens the
public room list, the launcher waits up to five seconds for every listed
server; if one is still connecting or unavailable, it reports that the list
is incomplete and asks the player to retry instead of silently presenting
the rooms from only the responding regions. Launchers look again every 30
seconds. Each connected launcher holds one quiet session on every listed
server, so every server's session count includes the players connected to
the others; they join no room there and send no diagnostics. Taking a server
off the list takes a release; until then, launchers find it down.

## Releases

`.github/workflows/release.yml` builds separate player and server archives
for Windows x64, Linux x64 and macOS arm64. The player archive has the
launcher (`TPF3-MP.exe`, `TPF3-MP.app`, `tpf3mp-launcher`), command-line
agent, in-game hook and mod. The server archive has `tpf3mp-server` and
this operations guide. Windows also gets a single `TPF3-MP.exe` setup
download. The Linux packages are built on Ubuntu 22.04, so they run on
distributions with an older C library too.

- **Cutting one.** Every push to `main`, which only receives what passed
  `acceptance` (see [AGENTS.md](../AGENTS.md)), builds the packages and
  attaches them to a draft release `v<version>`, the version in
  `Cargo.toml`. Review the draft on GitHub, then publish it; publishing
  creates the tag. Later pushes refresh the draft until it is published.
  After that, `main` needs a version bump before it can release again.
  Without the variable `TPF3MP_UPDATE_PUBLIC_KEY` no draft is made (see
  [Before the first release](#before-the-first-release)).
- **The server players play on.** Set the repository variable
  `TPF3MP_DEFAULT_SERVER` (Settings, Secrets and variables, Actions,
  Variables) to the public server's `host:port`. It is built into the
  packages' launcher as its default server (D12 in
  [DECISIONS.md](DECISIONS.md)). A one-server release refuses a room on a
  different server; a regional release resolves codes only across its
  compiled trusted list (D12, approved regional amendment). Under D12's
  proposed server-setting amendment players may change the server in
  Settings (remembered in `launcher.json` as `chosen_server`), and
  **Reset to default** returns to this one. A build without the variable,
  as a developer's, defaults to the project's relay
  (`tpf3mp.213-133-98-90.sslip.io:29470`, shown as `EU`; `setup::RELAY`
  in `tpf3mp-agent`), but the workflow still drafts no release without
  it, so each release names its server on purpose. `TPF3MP_SERVER_NAME`,
  such as `EU`, is what the launcher shows of the default server instead
  of its address, with a dot that
  is green while the server answers `https://<host>/tpf3mp/health`, the
  one path of its admin endpoint the host's nginx passes on
  (`deploy/nginx.conf.example`). No draft is made without it. For
  development and playtests, `--server <host:port>` on the launcher's
  command line plays on another server for that run, over the player's
  setting. The launcher trusts a server by the public certificate
  authorities, so a server players choose needs a real certificate, as
  the relay's (Let's Encrypt, for its sslip.io name) is; `--pin-cert` is
  for development servers. The packages also
  carry `PLAYING.md`.
- **More servers.** `TPF3MP_SERVERS`, empty by default, lists the
  release's other servers as `NAME=host:port` separated by commas, such
  as `US=us.example.org:29470`; see
  [More than one server](#more-than-one-server).
- **Updates.** The launcher installs a release only if it is signed with
  a key it trusts. Whoever holds that key can run code on every player's
  machine, so it lives where no branch or workflow but one can read it,
  and every signing needs your approval. Set it up once, the repository's
  owner, by hand: `tools/github/setup-update-key.cmd` (double-click it on
  Windows; it needs `gh` signed in as the owner) does the three steps
  below, keeps the private key in a folder you pick outside every
  repository, never prints it, and re-runs the last release run of `main`.
  Or by hand:

  1. Create the key on a trusted machine, and keep a copy of the `.pem`
     offline:

     ```sh
     openssl genpkey -algorithm ed25519 -out tpf3mp-update-key.pem
     openssl pkey -in tpf3mp-update-key.pem -pubout -outform DER | tail -c 32 | base64
     ```

  2. In Settings, Environments, create the environment **`release`**. Add
     yourself as **required reviewer**, and under deployment branches and
     tags allow only the tag pattern **`v*`**. Add the whole `.pem` file as
     its secret **`TPF3MP_UPDATE_SIGNING_KEY`**. Never make it a repository
     secret: those reach every workflow on every branch.
  3. In Settings, Secrets and variables, Actions, Variables, set
     **`TPF3MP_UPDATE_PUBLIC_KEY`** to the line the second command
     printed. Every launcher built from then on trusts it.

  Publishing a release then starts `sign.yml`, which waits for your
  approval in the Actions tab, writes `release.json` (per platform: the
  package's name, size and SHA-256) and signs it as `release.json.sig`.
  Drafts are never signed. Launchers fetch the latest published release's
  `release.json` (never GitHub's rate-limited API) and install it only if a
  trusted key signed it, its version is newer than theirs, and the package
  matches. Until a release is signed, launchers do not offer it.

  To move to a new key, add it to the variable, separated by a comma, and
  release: launchers from that release trust both. Sign with the new key
  once most players have that version, and remove the old one later.
  Losing every trusted key means players download the next version by
  hand once.
- **Announcing on Discord.** Once `sign.yml` has signed a published
  release, `.github/workflows/announce.yml` posts it to a Discord channel
  and mentions the announcement role: the release's name, a link to its page
  and the start of its notes. By then launchers offer the update. It posts
  nothing for a draft, a prerelease or an unsigned release, and the message
  can mention that role and no one else, whatever the notes say. A release
  signed a second time is not announced again; run the workflow by hand
  with the tag to post it again. Set it up once:

  1. In the Discord channel's settings, Integrations, Webhooks, create a
     webhook and copy its URL. In Settings, Secrets and variables, Actions,
     add it as the repository secret **`DISCORD_RELEASE_WEBHOOK`**.
  2. In Discord, with Developer Mode on (User Settings, Advanced),
     right-click the announcement role under Server Settings, Roles, and
     copy its ID. Set it as the repository variable
     **`DISCORD_ANNOUNCE_ROLE_ID`**.
  3. Try it: run **announce release** by hand in Actions with the latest
     tag. If the message shows the role but notifies no one, turn on
     "Allow anyone to @mention this role" in the role's settings.

  Without the secret the workflow posts nothing and still passes, as in
  forks; with a malformed webhook URL or role ID it fails and posts nothing.
- **Building without releasing.** Run the workflow by hand. The packages
  stay workflow artifacts, but the repository is public, so anyone can
  download them.
- **The in-game pieces.** The packages carry the hook library next to the
  launcher, which loads it into the game it starts and into no other
  (D11 in [DECISIONS.md](DECISIONS.md)), and the Lua mod as
  `mod/tpf3mp_1` once the repository has `mod/tpf3mp_1`. Nothing of
  TPF3-MP goes into the game's folder. Players install the mod alone, with
  the readable scripts in the package: `INSTALL_TPF3MP.cmd` (which runs
  `tools\install.ps1`) on Windows, `install.sh` on Linux and macOS (see
  "Installing" in [PLAYING.md](PLAYING.md), and D9).
- **Windows first install (1.1).** The release also carries `TPF3-MP.exe`,
  the same launcher as the ZIP. Without a package beside it, it opens setup
  and fetches the signed Windows package using the existing updater trust
  keys. First install and repair permit the same release version; ordinary
  updates still require a newer version. Setup installs per user, invokes
  `tools/install.ps1` for the mod and `tools/manage.ps1` for shortcuts and
  the Apps uninstall entry. A managed marker distinguishes it from the ZIP.
  Subsequent Windows starts synchronize an absent or outdated mod before
  opening the backend. Package replacement refuses while the game runs.
  The public EXE has no Windows publisher signature; Ed25519 authenticates
  downloaded packages, not the initial executable's Windows publisher.
- **Until the game is out** the hook finds no build profile and installs
  nothing, so the package is for trying the launcher and the netcode with
  the fake game.

## Capacity

Measured with `tpf3mp-loadtest` on one Windows desktop, with the server and
400 bot clients in the same process:

- 50 rooms of 8 players;
- 1,000 steps at 50 steps per second;
- 229,000 events applied across replicas;
- no divergence;
- p99 command latency 114 ms on loopback, through the socket that also
  takes tunnels, as deployed;
- with every bot in a TLS tunnel instead (`--tunneled`): p99 116 ms and
  the same run time. On loopback nothing is lost, so this measures the
  tunnel's own cost; on a lossy link, TCP holds datagrams back behind each
  lost segment;
- one full room of 64 players: p99 116 ms, no divergence;
- with every room logged and compacted past 8 KiB (`--data-dir`,
  `--compact-log-kib 8`), 3,000 steps: 121 compactions during the run,
  p99 114 ms, and memory level at about 150 MB.

The server on its own, as a separate release-build process with the bots in
another, 1,500 steps at 50 steps per second:

| rooms × players | logged | server CPU | memory | p99 |
|---|---|---|---|---|
| 100 × 4 | no | 0.38 cores | 31 MB | 112 ms |
| 200 × 4 | yes | 0.70 cores | 48 MB | 114 ms |

That is about a three-hundredth of a core per busy room, growing linearly,
with logging costing little. Rooms default to 5 steps per second, far
fewer turns than this.

Memory follows the rooms: 20 rooms of 4 players took a server at 13 MB at
rest to 33 MB after 5 minutes of play, as each room's resume window fills
(it holds up to an hour of turns). Once the games closed, it settled at
19 MB and stayed there; 15 minutes of play in one process showed no
divergence and steady latency.

Repeat against the real host after deploying. Every bot connects from the
machine running the load test, so first raise that address's limits on the
server, for example with `--max-sessions-per-address 1000
--max-handshakes-per-address 1000`, and restore them afterwards:

```sh
cargo run --release -p tpf3mp-testkit --bin tpf3mp-loadtest -- \
    --server tpf3mp.example.org:29470 --rooms 20 --bots 8 --paced
```

`--paced` makes the bots play at the room's pace behind a jitter buffer, as
games do, so the latencies it reports are the ones players would feel.
`--tunnel wss://tpf3mp.example.org/tpf3mp` sends every bot through the
tunnel instead, through the reverse proxy.

## Security notes

- The container runs as a non-root user with a read-only root filesystem,
  every Linux capability dropped, `no-new-privileges`, and limits on PIDs,
  memory and CPU. It mounts only its certificates (read-only) and its own
  data volume.
- The admin endpoint has no authentication. The compose file publishes it
  on the host's loopback only.
- One network address holds at most 8 sessions and 4 handshakes in progress
  (`--max-sessions-per-address`, `--max-handshakes-per-address`). An IPv6
  /64 counts as one address. A household or LAN party with more players
  behind one address needs a higher limit.
- Once half of the 256 handshake slots are busy, new clients must prove
  their address with a QUIC retry, so spoofed packets cost nothing. The
  `retries_sent` and `connections_refused` counters show when this happens.
- A session that stays outside any room for 10 minutes is closed
  (`idle_sessions_closed`).
- One address has at most 8 open rooms (`--max-rooms-per-address`). A room
  counts until it closes, and a running game with nobody connected closes
  after its grace period, 10 minutes by default (`rooms_abandoned`, see
  [Room lifetime](#room-lifetime)). Throwaway identities therefore cannot
  fill the server's rooms.
- An address that sends 20 wrong invites or room passwords within 10
  minutes is refused every join until the 10 minutes are up: invites are
  six characters (D13), and this is what keeps anyone from finding rooms
  by trying codes. A LAN party mistyping codes behind one address may
  meet it; it clears by itself.
- Rotate the invite key only deliberately: every existing invite stops
  working.

The crash-report collector snapshots existing file metadata when it starts and
when diagnostics are disabled. New reports are identified without comparing
filesystem timestamps to the wall clock, so coarse Windows timestamps do not
hide a new report. Each diagnostics setting change invalidates in-flight file
reads and upload retries. After an off/on transition, the next poll discards all
unread file content (including any new lines since re-enabling) to ensure that
content written while off is never sent later.
