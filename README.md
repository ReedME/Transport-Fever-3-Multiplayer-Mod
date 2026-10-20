# TPF3-MP: Transport Fever 3 Multiplayer — ReedME fork

This fork retains the [11 October repairs](investigation/REEDME_REVIEW_IMPLEMENTATION_2026-10-11.md)
and includes upstream development changes through 19 October for competitive
companies through Proton. The candidate is `1.3.2-reedme.2`; it has not passed real-game acceptance and has no published
player release. See [fork setup and validation](docs/REEDME_SETUP.md).
The original project is [Juliansgith's TPF3-MP](https://github.com/Juliansgith/Transport-Fever-3-Multiplayer-Mod).

Play Transport Fever 3 together. Build railways, run lines and grow cities
with your friends in one world, each with your own company or sharing one.

![The TPF3-MP launcher during a game](docs/images/launcher.png)

> **Development candidate.** The upstream Windows release is the baseline;
> this fork's repairs still require a real Proton game session. Build and test
> against a temporary local server using [REEDME_SETUP.md](docs/REEDME_SETUP.md).

## What you get

- **Rooms with a six-character invite.** Create a room, send your friends
  its code (such as `K7QM2X`), and they are in. Add a password if you like.
- **A dedicated server orders the session.** Clients connect to that server.
  This fork currently uses local test servers; an operated public server is a
  release setup step.
- **Join a game that is already running.** Late players, and players who
  come back after a crash, receive the room's world and catch up.
- **Separate companies in the native economy.** The initial scope is
  competition among trusted friends. The sequencer does not independently
  validate a cheat-resistant economy.
- **Chat, and a clear view of the room**: who is in, who is ready, and
  exactly which mods to add or remove when yours differ from the room's.
- **A launcher that looks after itself.** On Windows one EXE installs the
  launcher and multiplayer mod. It downloads only releases the project
  signed and keeps the installed mod updated with the launcher.

## What you need

- **Transport Fever 3** on Steam, and Steam running.
- **The supported Windows game build**, on Windows or in the same Proton
  prefix as the Windows launcher. Native Linux game support is outside the
  initial scope; the Linux server is supported.
- Everyone in a room needs the same game version and the same mods; the
  launcher tells you what to change.

## Getting started

The following managed setup applies to native Windows once a fork release is
published. Proton uses the portable ZIP and [its setup flow](docs/PLAYING.md#proton-and-wine-the-windows-zip-package).
There is currently no published fork player package.

1. **Download from [this fork's releases](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/releases)**
   and open it. No ZIP extraction or administrator rights are needed.
   Windows may warn about an unknown app: choose **More info**, then **Run anyway**.
2. **Click Install TPF3-MP.** Setup finds Steam's mods folder (choose your
   account if there are several), downloads the signed package, and adds
   Start menu and desktop shortcuts. Start the game through Steam once first if its
   user folder does not exist yet.
3. **Open the launcher and start Transport Fever 3.**
4. **Click Join a friend** to enter your name and invite code, or
   **Multiplayer** to browse public rooms or host your own.
5. **Host a new world or use a save.** New worlds use the game's normal
   setup screens, with TPF3-MP selected automatically, and start multiplayer
   when setup finishes. Keep TPF3-MP active in the world's **Mods** tab.
   Send friends the room's invite code; they join and load the shared world.

Only a game the launcher starts joins the room; started from Steam,
Transport Fever 3 is the plain game, with nothing of TPF3-MP in it.

The full player's guide, with what to do when something does not work,
is in [docs/PLAYING.md](docs/PLAYING.md).

The ZIP remains available for portable/manual installation. Windows
Settings → Apps, or the launcher's Settings, can uninstall a managed
installation; saves and settings are kept. **Repair installation** restores
the package and mod if files are missing or damaged.

## Getting help

- The launcher shows your **support code** at the bottom. Quote it when you
  ask for help: it lets the server's operator find what went wrong for
  you, and it lets nobody into your room, so it is safe to post.
- Report problems in the
  [issues](https://github.com/ReedME/Transport-Fever-3-Multiplayer-Mod/issues).
- While you are connected, the launcher sends its log to the server, with
  your paths, codes and addresses taken out, so there are no files to
  send. You can turn this off in **Settings**.

## Credits

- The launcher is **tearded's TPF2 Multiplayer Launcher**, brought to
  Transport Fever 3.
- TPF3-MP builds on two Transport Fever 2 multiplayer mods by its team:
  **TPF2MP** by _Sep and **TpF2 Multiplayer** by silver2127.
- The city in the launcher is a Transport Fever 2 screenshot, and the
  logo is Transport Fever 3's, both © Urban Games, used under their
  fan-content terms.

- PR-level authorship, request origins, and prior-art sources are recorded
  in [Contribution provenance](docs/CONTRIBUTION_PROVENANCE.md).

TPF3-MP is an unofficial fan project, not made or endorsed by Urban Games
or Paradox Interactive. It does not redistribute any part of Transport
Fever 3.

## For developers and server operators

How it works, where it stands and how to build it:
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md). Running a server:
[docs/OPERATIONS.md](docs/OPERATIONS.md). How changes are made and
released: [AGENTS.md](AGENTS.md).

## License

MIT; see [LICENSE](LICENSE).
