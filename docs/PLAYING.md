# Playing

How to play Transport Fever 3 together with TPF3-MP. Windows is the first
supported game platform; Linux and macOS packages remain for development.

## What you need

- Transport Fever 3, the same build and mods as everyone in your room, in
  the same order. The room compares everyone's before a game starts, and
  tells you exactly which mods to add, remove or update if yours differ.
  Mods that only change what you see (windows, overlays, a minimap: your
  *personal* mods) may differ, when you list your mods with `--mods`: the
  launcher scans each one and leaves those out of the comparison, and the
  room's world loads with your personal mods and without other players'
  ([MODS.md](MODS.md); proposed, D25). `tpf3mp-modscan --installed` says
  which of your mods are personal, and why.
  Without `--mods` the launcher finds your mods itself: choose your
  personal ones in the lobby, and it remembers them; a room you create
  takes its shared mods from its start save, and the lobby shows each
  player which of them they have.
- The Windows x64 launcher from the project's releases. Linux x64 and
  macOS on Apple silicon packages are available for development; their
  real-game acceptance is still pending.

You do not need to forward any port or open anything on your router: your
launcher connects out to the server, and everything goes through it.

## Installing

### Windows: one EXE (recommended)

Download `TPF3-MP.exe` from the latest GitHub release and run it. Click
**Install TPF3-MP**: the launcher downloads and checks the signed package,
installs itself under `%LOCALAPPDATA%\Programs\TPF3-MP`, installs the mod
through its readable `tools\install.ps1`, and creates Start menu and desktop
shortcuts. You can clear the desktop shortcut option in setup. No
administrator rights are needed.

Setup shows the detected Steam mods folder. If several accounts have played
the game, choose yours; if detection fails, paste its `staging_area` path.
Start the game through Steam once if its user folder does not exist yet,
then close it before installing. Open the launcher when setup finishes.
Creating a world from **Multiplayer** selects TPF3-MP automatically in the
world's **Mods** tab. Keep it active. When preparing a world separately,
find **TPF3-MP** in that tab and click **Activate**. Future updates install
the matching mod before opening the launcher.

Reopening the downloaded EXE opens the installed launcher, including when
offline. An initial install needs an internet connection and a signed
published release. A failed download can be retried; a failed mod install
shows its error and keeps Play unavailable until setup succeeds.

The Windows installer accepts detected paths beginning with `\\?\` as well
as ordinary drive and UNC paths. If an older release reports that argument
`drive` is null, remove the leading `\\?\` from the mods-folder field (so a
local Steam path begins with `C:\`, for example) and retry installation.

**Settings → Repair installation** restores a managed installation from
the latest signed package and reinstalls its mod. In a portable package it
reinstalls the bundled mod. **Uninstall** removes the mod, and for managed
installs also removes the launcher and shortcuts after the setup window
closes. Windows Settings → Apps has the same uninstall entry. Saves,
identities and settings remain; removed files go to TPF3-MP's backups.

### Proton and Wine: the Windows ZIP package

Use the Windows x64 ZIP, extract it, and run its `TPF3-MP.exe` in the
**same Proton/Wine prefix as the Windows game**. The standalone managed
EXE requires native Windows PowerShell for shortcuts and removal, and
refuses setup under Wine before changing files. Wine's bundled PowerShell
is a stub that can report success without executing an installer.

The portable launcher's setup copies only `tpf3mp_1` into the account's
`3493540/local/staging_area`, verifies every bundled file and its installation
record, and keeps replaced files in its data folder's `backups`. Repair and
mod removal use this same Rust path. Game-process enumeration must succeed
and find no running game before changes; close games in other Wine prefixes
too, since a prefix can only inspect its own Windows processes. Managed
Windows shortcuts and registry installation are unavailable in this path.

Linux Steam library paths, including secondary libraries, are translated
through Wine's `Z:` drive. If auto-discovery still finds another executable,
pass `--game-exe` with the Windows game's full path. The current native hook
requires the exact supported Windows build; a native Linux executable cannot
use the Windows hook. These installation checks do not establish game or
injection compatibility under Proton; real-game acceptance is still required.

### Portable packages and other platforms

1. Unpack the package anywhere you can write to, such as your Documents
   folder: the launcher updates the files in it (see "Updates").
2. **The mod.** Start Transport Fever 3 once, so Steam makes its folder for
   your mods, and close it again. Then:
   - **Native Windows:** double-click `INSTALL_TPF3MP.cmd` in the package.
   - **Proton/Wine:** start the packaged `TPF3-MP.exe` and use its setup,
     as described above; do not use the PowerShell scripts.
   - **Linux and macOS:** run `./install.sh` from the package's folder.

   Native Windows and Linux/macOS installers are readable scripts: open `tools\install.ps1`
   (Windows) or `install.sh` (Linux and macOS) to read exactly what it
   changes. It puts the TPF3-MP mod, `tpf3mp_1`, in Steam's folder for
   your Transport Fever 3 mods, `<Steam>/userdata/<account>/3493540/local/staging_area`,
   and notes its version in TPF3-MP's data folder, which the launcher
   shows. Creating a world from **Multiplayer** activates it automatically.
   For a world prepared separately, select **TPF3-MP** in its **Mods** tab
   and click **Activate**. To put it in another mods folder, drop that folder onto
   `INSTALL_TPF3MP.cmd`, or run `./install.sh "<the mods folder>"`.

   Nothing goes into the game's own folder, and no launch option is set.
   The installer refuses, and changes nothing, while the game is running.
   A step that fails undoes the ones before it. Nothing is deleted: a
   TPF3-MP mod it replaces or takes out goes to the `backups` folder in
   TPF3-MP's data folder. `UNINSTALL_TPF3MP.cmd` or `./uninstall.sh` takes
   the mod out again.

   On Windows the launcher installs the matching mod after an update.
   On Linux and macOS, run the installer again after an update of TPF3-MP.

The part of TPF3-MP that runs inside the game is not installed at all: the
launcher loads it into the game it starts, into that game alone, for as
long as it runs (see "The launcher"). Started from Steam,
Transport Fever 3 is the plain game, as if TPF3-MP were not there.

## The launcher

Start the launcher from the package:

- **Windows:** `TPF3-MP.exe`. The first time, Windows may say it protected
  your PC from an unknown app: choose **More info**, then **Run anyway**.
- **macOS:** `TPF3-MP.app`. The first time, macOS refuses to open an app
  from an unidentified developer. On macOS 15 and later: try to open it
  once, then in **System Settings**, **Privacy & Security**, choose **Open
  Anyway**. On earlier versions: right-click it, choose **Open**, then
  **Open** again.
- **Linux:** `tpf3mp-launcher`. It needs a desktop with Vulkan or OpenGL
  drivers, as the game does.

It opens the TPF3-MP window. Keep it open while you play: it holds your
connection to the server, closing it ends your session, and during a game
it asks first. If the native window cannot open or stops working, the
launcher opens the same launcher as a page in your browser instead
(`--browser` does so on purpose); that page works only on your own machine,
in the tab the launcher opened, and has the whole lobby in it. When the
window fails, the page takes over the existing launcher session, including
its room and game link. If Windows cannot open the page automatically, a
Windows dialog shows the private address and copies it to the clipboard when
available; otherwise it explains how to copy the address from the dialog.
Paste it into a browser on this computer. If the configured local port is
already in use, the launcher tries another loopback port. If it still cannot
serve the page, a Windows notice explains why; the existing session remains
connected, so keep the launcher running while you play.

The window starts the game and shows where things stand; you play from the
game's own **Multiplayer** button (next section).

- **Start Transport Fever 3**, the big button, starts the game with
  TPF3-MP in it, with Steam running. Only a game started here has the
  Multiplayer button and joins rooms: started from Steam, it is the plain
  game. Press it once; the launcher refuses to start a second game while
  the first still runs. It is also the first step of **How to play: in the
  game** on the left, which ticks the steps off as you go.
- **Server** names the server you play on: by default the project's relay
  (**EU**, in Germany), which nearly everyone uses; its dot is green while
  the server is online. The pill
  at the top of the panel says whether you are connected, in a room's
  lobby, or playing.
- **Your room.** Once you are in a room, the left side shows it: its
  players, whose mods differ, and the **Session log**, which tells you
  what happened, such as your world being replaced by the room's or your
  connection coming back. Its chat and buttons are in the game.
- **While the room's world comes**, the big button follows your game:
  receiving the room's world, loading it, and playing, with the room's
  speed.
- **Your game**, the bar along the bottom, says where Steam has Transport
  Fever 3 and whether the TPF3-MP mod is installed (see "Installing").
  **Settings** has the server, updates and diagnostics (see "Changing the
  server" below); the **support code** and the **log session** at the
  bottom are what to quote to the server's operator.
- If the game closes or crashes once it has connected, the launcher
  notices within a second: the Session log says "the game session failed:
  Transport Fever 3 closed", and you are back on the server, out of the
  room. Join it again with its invite and start the game again.

**Which launcher is this?** The bottom left of the window says its
version, the protocol it speaks to servers and the commit it was built
from, as `v1.2.0 · protocol 16 · <commit>`; **Settings**, **About this
launcher** lists them too. The first line of its log names the file that
runs. On Windows, the file's **Properties**, **Details** show the same
version and commit.

**One launcher at a time.** Starting the launcher while another one runs
does not leave you with two:

- another one of the very same build is already running: it says so,
  names its file, and does not start; use the window that runs (it may be
  minimised or behind the game);
- an older one of another build is running, for example one left open
  before you installed a new version: the new one asks it to close, waits
  a moment and takes its place. A game that the old one started and that
  is still at its main menu follows the new one, unless it runs an older
  TPF3-MP hook; then the launcher says to close the game and start it again
  from here. The old one stays open while it is in a room, and the new one
  then says so instead of starting: leave the room, close the old one,
  and start the new one again;
- a newer one is running and you started an older one, for example from a
  shortcut to an old install: the older one does not start, and names
  both files;
- a launcher from before this check (it does not say its version) is
  running: the new one does not start, names that one's file and asks you
  to close it.

**Lobby in this window instead**, under the big button, brings the whole
lobby into the launcher, as it was before the game had its Multiplayer
button: connect with your name (and an invite, to join in one step),
create a room with its rules and password, join with an invite, the room's
players with **Remove** for the owner, **Copy invite**, **Ready**,
**Start game**, **Leave room** and the chat. Use it if the game's main menu
has no Multiplayer button, for example after a game update the hook does
not know yet (see "When something does not work"). **Lobby in the game's
menu instead** puts it back.

### Changing the server

In a one-server playtest, the launcher can connect to a server you type. A
multi-region release routes only to the trusted servers it was built with.

1. Open **Settings** (top right). The **Server** card says where you play.
2. In a one-server playtest, type a server address as `host:port` and press
   **Use this server**. If you were connected, the launcher reconnects under
   the same name and remembers the choice.
3. In a multi-region release, Settings lists the trusted regions and their
   pings. A saved trusted-region pin can be cleared with **Reset to default**;
   the launcher then routes automatically again.

You cannot change a server while in a room: leave it first. On a
multi-region release, a region in the trusted list can be chosen on the
invite form; without that choice, a six-character invite is resolved across
every listed region. An address pasted beside an invite is ignored. The
browser page (`--browser`) follows the same trusted-region rules under
**Settings: server**.

### When TPF3-MP has several servers

Under D12 (approved 2026-10-08), a release may come with more than one
server, such as **EU** and **US**. Then, unless you have pinned one of those
listed regions:

- the launcher connects to the server closest to you, the one with the
  lowest ping, and Settings lists every server with its ping;
- **Public rooms** shows the rooms of every server, each card with its
  server and ping (`US · 110 ms`); a click joins it on its server;
- if a listed server is still connecting when you open **Public rooms**,
  the launcher waits up to five seconds for every region to respond. If a
  server is still connecting or unavailable, **Public rooms** reports an
  incomplete list and asks you to retry;
- a room you host goes to the closest server;
- a six-character invite code works on any listed server. The launcher
  checks the code and password on all regions without joining; exactly one
  match is required before it joins. If multiple regions accept the same
  code and credentials, choose the intended listed region. Wrong
  passwords and absent invites give the same generic response. If a
  region is unavailable or times out, the lookup fails closed and asks you
  to retry. A public-room card and an explicit region choice go directly
  to that listed region.

A multi-region release accepts only its compiled trusted servers;
**Reset to default** returns a pinned launcher to automatic regional play.

## Playing from the game's Multiplayer button

Connecting, rooms, the lobby and chat are in the game. Everything you do
there goes through the launcher, which you keep open, and shows in its
window too.

1. **Start the game from the launcher**: **Start Transport Fever 3**.
2. **Click Multiplayer.** The game's main menu has two TPF3-MP cards to
   the right of its own: **Multiplayer**, which says under its title where
   you are (not connected, online on EU, your room and how many are ready),
   and **Join a friend**. The top bar has a **Multiplayer** button too,
   next to Settings. Each opens the Multiplayer window, one page at a
   time. **Join a friend** opens a focused form for your name, six-character
   invite and optional password. **Join room** connects and joins in one step;
   a failed connection stays on the form so you can retry.
3. **Connect.** The first page: type the name others will see, or keep the
   one the launcher remembers, and press **Connect to EU** (the server is
   the launcher's; there is none to type). You can also go straight to either of the two cards:
   **Join a room** and **Host a room**. Each opens its page, and **Back**
   returns to this one. **Server...**, at its bottom, shows the server you play
   on by its name, marked (default) when it is the launcher's own (the
   window never shows a server's address, but in this field); type another
   (`host:port`) and **Use this server**, or **Reset to default**.
   Changing it disconnects you and connects to the new one. A one-server
   setup keeps invites on that server; a regional release resolves invites
   across its listed regions. Not while in a room.
   **Your banner**, next to it, picks the picture the others see on your
   card in a room, from the game's own pictures; **Default** goes back to
   the one chosen for you. Under **Characters** you can pick one of the
   campaign's characters instead, by name: their portrait then shows
   beside your name. The launcher takes the portraits from your own game's
   campaign when it starts, so they are there only if your game has the
   campaign; a player whose game lacks your portrait sees your default
   banner. The launcher remembers your pick.
4. **Join a room, or host one.**
   - **Join a room**: the rooms their owners made public, as cards like
     the main menu's, each with the picture of its map's climate, its
     name, players out of its limit, companies and the game's year,
     **Playing** once its game runs, and a lock if it has a password.
     Click one to join it; one with a password asks for it first.
     **Previous**, **Next** and **Refresh** page through the list, which
     also refreshes itself every ten seconds. **Join with code**, at
     the bottom right, opens a page for a friend's room: the **invite
     code** they sent you, such as `K7QM2X` (upper or lower case), the
     room's password if it has one, and the server and name you join
     with; **Join** or **Cancel**. A private room is joined by invite,
     either here or through **Join a friend** on the main menu.
   - **Host a room**: the **world** the room starts from, a big card:
     click it to pick a save on the game's own **Load Game** page, which
     then reads "The room's save and mods" and has **Use for the room**
     instead of Load Game. Pick a save, check or change its mods on its
     **Mods** tab and its settings on **Gameplay Settings**, as you would
     to load it, and press **Use for the room**: the room starts from that
     save, with exactly those mods and settings in every player's game.
     An active tutorial is quit automatically on the first simulation update
     in a multiplayer room (unpause if needed), using the game's normal Quit Tutorial cleanup on
     every replica. This releases the tutorial's construction and finance
     restrictions; playing through the tutorial together is not supported.
     Single-player tutorials and campaign missions are not auto-quit.
     The first tile, **New world**, starts the room from a world you
     create: the game opens its normal setup screens for climate, map and
     settings once the room is made. TPF3-MP is always among the room's
     mods, so a save from single player without it works too: every game
     adds it when it loads the room's world. (A launcher started with
     `--mods` loads saves with their own mods; there a save without
     TPF3-MP is refused with "This save doesn't have the TPF3-MP mod
     enabled: load it once, turn TPF3-MP on in its mods, save it, then
     pick it again", since the room's game cannot run in a world without
     it.) **How you play**, two pictures: **Co-op**, everyone for
     the room's one company, or **Competitive**, each player founding a
     company of their own in the game. Then the **room name** (your
     name's room if you leave it empty), **Players**, 2 to 16, **Who can
     find it**: **Private**, invite only (the default), or **Public**, in
     the room list, with your save's climate and year; the **Rules**, when
     the server offers more than one (`native` is the game's own rules and
     economy, as in single player; a description says what the others
     are); and an optional **password**. Then **Create room**. You own the
     room: you start its game and can remove players. With a new world,
     once it has loaded and everyone is ready, multiplayer starts
     automatically.
   - **Your mods**, at the bottom of both pages: the mods only you play
     with (only you see them), to turn on or off with the game's own
     **Activate** button. Mods every player needs are the room's, picked
     by its owner with the save.
5. **The room.** Three tabs:
   - **Room**: the world the room starts from, as a big card (its
     picture, name, climate and year; the owner clicks it to pick another
     save and its mods on the Load Game page, until the game starts);
     the players as picture cards of their banners (and their portrait
     beside it, if they picked a character), each marked **Owner**,
     **You**, **Ready** or **Not ready**, **Away**, and how many of the
     room's mods they lack; the room's **invite code** to send your
     friends (**Copy** beside it puts it on the clipboard), its players,
     play style, password, server and mods; and the room's chat (type and
     press Enter or **Send**). A new save goes up to the room ("Sending
     mptest to the room: 42%", with a bar), and **Start the game** waits
     until the room has it. Everyone is then asked to get ready again,
     since they agreed to the save and its mods before. Picking the save
     first, when the room had none, asks nobody again.
   - **The room's mods**: every mod the room's world runs, as tiles like
     the game's mod selector's, each marked **Installed**, **Missing** or
     **Another version**, and where it comes from. A missing mod from
     Mod Hub has **Install**: it opens the mod's Mod Hub page in the game,
     where you see what it is and **Subscribe**; Mod Hub downloads it with
     your own account, and the tile follows it until it is installed.
     **Install all missing** asks once for all of them, showing what each
     is on Mod Hub, before subscribing. A mod not on Mod Hub says to ask
     the owner where to get it. Not signed in to Mod Hub, the tab offers
     the game's Mod Hub page to sign in. **Start the game** stays off,
     naming who, while anyone's mods differ from the room's.
   - **Only for you**: your own mods, as **Your mods** above. You can
     change them until the room's game starts.
6. **Get ready.** At the main menu you are marked ready by yourself: a
   guest at once, the owner once the room has the save picked in step 4
   or 5. **Ready** and **Not ready** set it by hand. When the owner
   changes the save in the room, guests press **Ready** again.
7. **Start.** The owner presses **Start the game** once everyone is
   ready and the room has its save (until then it says it is waiting for
   everyone, or that the save is still on its way). Every player's
   game loads the room's world from the menu and starts it, with no Start
   Game to press. New worlds created through the lobby start automatically
   after the owner finishes the normal world-generation screens.
8. **Play.** While the world comes, the window says how far it is
   ("Receiving the room's world: 42% (48.0 MB of 112 MB)", then "Loading
   the room's world..."), and the chat and **Leave room** still work.
   Each player's row says how far their game is: **Downloading 42%**,
   then **Loading...**, then **Playing**. In
   the game, the Multiplayer window on the game bar has the room (see
   "While you play").

What the window says:

- A line under the page's title says what is under way ("Creating the room...")
  until the launcher answers, then what happened. Anything refused, such
  as a wrong invite, a full room or a name that is too long, shows in red
  there, and the button can be pressed again.
- In a room, **The room's mods** tab shows which of the room's mods you
  lack or have in another version, and installs those from Mod Hub;
  outside one (a join refused because your game differs), "Your game
  differs from the room's" names the mods to add, remove or update.
- "This game has no link to the TPF3-MP launcher": the game was not
  started from the launcher. Close it and start it from there.
- **Remove** (the bin, for the owner) and **Leave room** ask first.
  **Disconnect** leaves the server. To leave from a running game, use
  **Quit → Return to Main Menu**, then **Multiplayer → Leave room**.
  You can join again without restarting the game or launcher.

## Updates

The launcher checks for a new version when it starts and every few hours,
and downloads it in the background. When it is ready, the badge at the top
says so: **Settings**, then **Restart and update**, installs it and
restarts the launcher. During a game it waits: the update installs the
next time you start TPF3-MP.

The launcher installs only what the TPF3-MP project signed: a download
whose signature, version or contents do not check out is refused, and an
install that fails or is cut short puts the old files back, at once or at
the next start. The old version's files stay until the new version has
opened its window; if it fails to three times running, the launcher goes
back to the version before and does not install that one again. Updates
go into the package's folder, so unpack it where you can write, not into a
protected folder such as Program Files.

## While you play

- **The Multiplayer window.** In the room's game the game bar shows the
  room in one line: its name, how many players are in the game, its speed,
  and new chat. Click it, or the Multiplayer button among the mods'
  buttons, for the Multiplayer window: the room's players (the host, you,
  anyone away), its speed, whether your world matches the room's, and the
  room's newest chat, where you can write to everyone. Rooms and invites
  are on the main menu's Multiplayer window (see "Playing from the game's
  Multiplayer button").
- **Reading the room.** Its in-game window keeps room status at the top,
  with Players and Companies side by side. Company cards separate the
  name, balance, debt and members; your company comes first. Scroll the
  company column for its management controls or more companies. Chat has
  its own scrolling history, with the message field always below it.
  Long names and messages wrap rather than widening the window.
- **Speed and pause.** The room's owner sets the room's speed, pause
  included, with the game's own speed buttons, and everyone's game runs at
  it. Guests' speed buttons highlight the room's accepted speed,
  including pause. Their buttons and speed shortcuts are disabled;
  their tooltip and the Multiplayer window say **Host controls speed**.
  You can build while the room is paused. Everyone receives the ordered
  build and its normal construction costs without advancing game time or
  moving vehicles. Editing and demolition follow the same room ordering.
  Outside a multiplayer game the normal controls return.
- **Joining later.** You can join a game that is already running: the
  room sends you its world, and your game loads it and catches up.
- **Losing the connection.** If your connection or the server drops, the
  launcher rejoins the room by itself, and your game only pauses. If you
  were away too long to catch up, the room sends you its world again.
  The server keeps your seat for 10 minutes (its operator may set longer).
  The launcher stops trying when the server says the room is gone, after
  5 minutes without getting back in, or when the connection drops again
  right after each of 5 rejoins in a row. Both windows then say **The
  room is gone (closed or the server restarted)**, or that it could not
  rejoin, and you are back on the server in no room: create or join
  another. **Leave room** works while it is rejoining too, in the
  launcher and in the game's Multiplayer window: it stops at once.
- **Leaving.** **Leave room** gives up your seat. It always works: if the
  server cannot be told, you leave anyway, and the server lets the seat
  go after its 10 minutes. The owner can also remove
  a player whose game froze; a removed player cannot come back to that
  room. If the room's game had not begun yet, your game keeps running and
  follows you into the next room you create or join: no need to restart
  it.
- **Your world disagrees.** Every few seconds everyone's game compares the
  world with the room's. If yours has drifted, the room tries to send an
  agreed replacement for your game to reload. A notice says so. With three
  or more different worlds, the votes can agree on parts that no one whole
  world contains. The room then keeps its last agreed save and cannot
  repair that disagreement automatically; it continues running. If the
  notices keep returning, the owner should pause play and coordinate a
  return to a known agreed save with the server operator. There is no
  automatic rollback to that older save.
- **Saving.** The room saves everyone's game together from time to time,
  which you notice as a short pause, like an autosave.
- **Room saves in your save folder.** To load the room's world, your game
  copies it into Transport Fever 3's save folder
  (`<Steam>/userdata/<account>/3493540/local/save`) as
  `tpf3mp_room_<number>.sav`, and the room's saves pass through there as
  `tpf3mp_<number>_<number>.sav`. They are not offered as saves to start
  a room from. Each is a whole world, so TPF3-MP removes those of games
  that have ended, when your game starts and each time it loads a room's
  world; the copy of a game still running stays. Your own saves are never
  touched.
- **Loans.** Take and pay back loans in the finance window as usual: every
  player's game books them together. Each company has its own offers and
  loans, up to four loans at once. An offer you take goes on a four-to-eight
  month cooldown before that slot gets a new offer; the interest and
  repayments are your company's alone.
- **Renaming and recolouring.** Rename a station, a vehicle, a town or a
  depot from its window or the line manager, and recolour a vehicle from
  its window, as usual: every player's game applies it.
- **Subsidies.** Accept or decline a subsidy offer from its card as
  usual. An offer is everyone's until a company takes it: the first to
  accept gets it, and it leaves every player's list. A subsidy you took is
  your company's alone: the money up front, the reward and the penalty are
  booked to it, and only your company's lines count towards it. The
  game's subsidy cards show every taken subsidy as accepted, another
  company's too. A "Transport Workers" subsidy still completes by anyone's
  commuters.
- **Bridge and tunnel types.** Click a bridge or tunnel and pick another
  type, or none, in its window as usual: it is rebuilt in every player's
  game. A stop on it stays where it was.
- **Level crossing types.** The preparatory route supports only a native
  type-only map proposal. The ordinary window may emit a rebuild even for a
  type swap; that UI flow is not carried and remains refused, as do rebuilds
  that change segments or junction settings. `crossings` stays gated off
  pending ordinary-start two-player acceptance. The type picked in the road
  or track tool's toolbar for a new crossing is built and waits for its
  acceptance too: until then each game lays the era's type, as before.
- **Line waypoints.** Add waypoints to a line's route in the line
  manager as usual: the line is changed in every player's game, each
  waypoint on the same lane or, for an aircraft, at the same place.
- **Industry Greenification marketing campaigns and Historic
  Preservation.**
  These channels remain refused pending a two-player game acceptance run.
  Their mechanics are implemented but are not enabled for play yet; see
  [COVERAGE.md](COVERAGE.md).
- **Prospecting.** Prospect for resources near a town from the
  construction menu as usual: every player's game starts the prospection
  together, a moment after your click, and uses your company's permit.
  When it ends, months later, every game finds the same industry at the
  same place, or nothing, and says so in the same notification.
- **Company ranks.** Take a new rank in the company window as usual: every
  player's game takes it together, a moment after your click. With one
  company in the room the rank grows as in single player. With more (a
  proposal awaiting the owner, D23), each company's progress is its share
  of each town it serves: the town's people, split by the cargo and
  passengers each company carries for it, times the company's rating
  there. The company window shows your company's own rank and permits.
- **Roads, tracks, stations and depots.** Build them with the game's own
  street, track and construction tools: every player's game builds them
  together, a moment after your click, and your company pays as usual.
  After the room builds a segment, choose the next start point. Continuing
  automatically from that segment is available for controlled testing with
  `TPF3MP_TOOL_CONTINUE=1`; it is off by default pending an input-cancellation
  check in the real game. A
  station or depot placed by a road is joined to it, as in single player.
  Street stops, on one side or both, go on with the stop tool; the stop
  goes where your cursor is on the road. The road tools tab works too:
  tram tracks, bus lanes, noise barriers, trees along the road and the
  lock against the town's changes, and a road built through a stretch
  with stops keeps them. So do the track menu's tools: electrification,
  a track's type and its decorations (seen with the road tools; the track
  tools are not yet tried in a real game). Remove them, and roads and
  tracks, with the bulldozer: a two-sided stop goes with one click, and
  a track takes its signals with it.
- **Airfields and airports.** Build the game's stock airfield or airport
  from **Air** with a hangar if you want to buy aircraft there. Place roads
  near the passenger building as in the base game. Use **Configure** on
  your own airport to add or remove its terminal, hangar and other modules;
  the editor's bulldozer removes individual modules, while the main
  bulldozer removes the whole airport. Buy aircraft in its hangar's depot
  window, then add the airport's airplane terminal to a line and assign
  the aircraft. An airport without a hangar cannot sell aircraft. The
  game's line manager may offer a helicopter terminal at a large airport;
  choose an airplane terminal for an airplane line. Another company's
  airport follows its station-access policy for your lines, but only its
  owner can change its modules or remove it.
- **Terraforming** (not in multiplayer yet: it is switched off until a
  two-player game has shown it works; a stroke changes nothing, and the
  hook's log says why). Once on: raise, lower, smooth and flatten the ground,
  and the heightmap brush, as usual: every player's game reshapes the same
  cells to the same heights, a moment after each stroke, and your company
  pays. Your own game changes the ground only when the room's copy of a
  stroke arrives, so while you hold the mouse down the brush works on the
  ground as it was before your last strokes came back. A stroke of more
  than 65,536 cells (a square about 1 km across) is refused.
- **Painting the ground and planting.** Paint the ground and use the asset
  brush (trees, rocks, bushes) as usual: every player's game paints the
  same texels and builds the same groups of trees, a moment after each
  stroke, and your company pays. A large stroke goes as several parts.
- **Town buildings.** Bulldoze a town's building as usual: every player's
  game removes the same building, a moment after your click; your company
  pays the demolition, and the town's opinion of it changes as in single
  player, the same in every game. Bulldozing a town street takes the
  buildings along it with it, as in single player; if by the time it
  arrives the game would take another building than the ones you saw go
  (the town grew meanwhile), nothing is removed in any game and the
  hook's log says why. Bulldozing trees and rocks works the same way:
  every game takes the same ones out of their group.
- **Vehicles and lines.** Buy vehicles in a depot's store, make and change
  lines in the line manager, and send vehicles out, stop them or sell
  them, as usual: every player's game does it together, and your window
  hears it a moment after your click. On Steam build 40420, station clicks
  made while a new line waits for the room are kept in order. On other
  supported builds, wait for the new line to appear before adding its next
  station; the copied line window has only been verified for build 40420.
- **Companies.** Everyone starts in the save's own company, together. In
  the Multiplayer window you can found a company of your own, join
  another, rename or recolour yours, and dissolve it once you are its last
  player and it owns nothing; any split works, two players in one company
  and one in another included. What you build and buy is your company's
  and paid by it, and what another company owns (its vehicles, lines,
  depots, stations and roads) is theirs: you cannot change or remove it.
  The game's own windows show your company: its money in the corner, and
  your things as yours. A new company starts with no money: borrow on the
  terms shown in the Multiplayer window, which also shows its loans and pays
  them back. The finance window shows that company's own offers and loans.
  With more than one company, vehicles and their
  markers on the map wear their company's colour, and a new colour
  repaints them. The colour button offers the companies' colours first,
  then the game's own.
- **Headquarters.** Each company builds one headquarters of its own, from
  the construction menu as usual, once its rank allows: another
  company's headquarters does not use up yours. A second one for the
  same company is refused. Each headquarters gives its own town the
  game's growth bonus, as in a single-player game. With more than one
  company, the town labels on the map crown every company's headquarters
  town as its capital, and every player sees them all: yours in the
  game's blue, another company's in that company's colour, each with a
  line under it naming whose it is ("Capital of Rival", or "Capital of
  Rival and Pals" when two companies have theirs by the same town). The
  line is hidden when you zoom far out; the crown and colour stay. With
  one company it is the game's own capital, as in single player. The
  game bar's transported figures and the finance window's company value still show
  the room's first company's.
- **Your company's head, passwords and stations** (D22: station access
  approved; the other policies remain proposed). The
  player who founded a company is its head while they play for it; after
  that, whoever has played for it longest. The Multiplayer window shows
  each company's head. The head can give the company a password: then
  others join it only by typing the password next to its Join button.
  The password goes to the server, which keeps it from every game and
  every log; nobody, the head included, can read it back, so share it
  the way you share a room's. The head can also remove or change the
  password, send a player back to the room's first company, and choose
  who may stop at the company's stations: **Deny by default** or **Allow
  by default** for every company (those founded later included), and
  **Allow** or **Deny** for each other company on its own, which wins over
  the default (**Default** puts it back). Stations start open: your lines
  may stop at another company's station, and the line manager offers it,
  unless its head denies your company. Access is checked when creating or
  changing a route; existing services keep running after access is denied.
  You still cannot change or remove another company's station, and your vehicles use your own
  depots. The room's first company is everyone's: it has no head and no
  password. The game's company window renames your company too.
- **Terrain.** Raise, lower, smooth, flatten and the heightmap brush share
  their height changes through the room, and so do terrain paint and the
  asset brush.
- **Achievements.** A game with TPF3-MP active still earns achievements:
  the mod keeps them on, as the game lets a mod do. This holds even when
  the save has other mods that would switch them off.
- **Not in multiplayer yet.** What the room cannot share with everyone yet
  does not happen in your game either. The game bar says "Not in
  multiplayer yet: …" for what the game's windows do that the room does
  not carry yet. Full junction editing (lane arrows, crosswalks, lights,
  phases and reset) remains behind `strict_junctions`, off by default. With
  it off, only the junction window's double-slip toggle is accepted; at a
  track-only node, its observed Auto-to-No preference side effect is kept.
  Other junction edits and crossing-tool tram-lane edits still show "Not in
  multiplayer yet" and build nothing. So does a tool that would move a stop
  or signal onto another stretch of road. A track started
  or ended between a track's signals now keeps each signal on the piece of
  the cut track the tool chose (`splitSignals`; accepted in a two-game room
  on build 40420). Mods' signals go too. A waypoint on the cut track still
  stops the build. A track started close to a node of a track with
  signals, where the tool moves that node and puts the signal back on the
  redrawn track, keeps the old signal there too (`refitSignals`; accepted
  in a two-game room on build 40420).

## Playtesting before the game is out

Until Transport Fever 3 is released, `tpf3mp-fakegame` in the package
stands in for it: a small toy game behind the same step gate, which builds
track, saves and loads worlds. Everything but TPF3 itself can be tried,
across PCs and systems:

1. Start TPF3-MP as above.
2. Start `tpf3mp-fakegame` from the same folder (from a terminal on Linux
   and macOS). The window's **Game** part says the game is connected.
3. Connect, create or join a room, get ready and start, as in a real game.
   The fake game has no save to load, so it does not mark you ready:
   press **Ready**.
   The fake game plays by itself: watch the **Game** part count steps, and
   try chatting, leaving and rejoining, and joining a game already running.

Run one fake game next to each launcher. It stops when its room's game
ends.

Developers who want a whole room on one PC, several fake games each with
its own agent, use the multiplayer rig instead (`tpf3mp-rig`, see
[DEVELOPMENT.md](DEVELOPMENT.md)).

## Your identity

The launcher creates a key for you on first use, in your user data folder
(`TPF3-MP/identity.key`). It is what makes you the same player next time,
so you can come back to your seat. There are no accounts or passwords. Keep
the file private, and copy it if you move to another computer.

Other players never see your IP address: everything goes through the
server.

## When something does not work

The bottom of the window shows your **support code**, six letters and
digits like an invite's (**Copy** copies it). It names your connection in
the server's log: quote it to the server's operator with your report.
Next to it, while diagnostics are on, is your **log session**, a code of
the same kind that names this whole run of the launcher, across every
time it connected; the game's Multiplayer window shows it too, on its
first page and under **Server...**, with its own **Copy**. Quote both.
Neither lets anybody into your room, so they are safe to post. There is
nothing to send: while you are connected, the launcher sends its logs to
the server by itself (see "Diagnostics"), so the operator finds what
happened to you from those codes alone. The launcher also keeps its log on your machine
(`TPF3-MP/logs` in your user data folder, one file a day, a week kept).

**"This launcher is too old for the server"?** The server speaks a newer
protocol than the launcher you started. The message names the file that
runs, such as `C:\Users\you\AppData\Local\Programs\TPF3-MP\TPF3-MP.exe`. If
that is not the newest TPF3-MP you installed, an old copy is still open
or a shortcut points at an old install: close every TPF3-MP window, then
start the newest one (check the version at the bottom left of its
window). Otherwise update it (**Settings**, **Restart and update**), or
download and install the newest package. **"This launcher is newer than
the server"** means the server has not been updated yet: tell its
operator.

**No Multiplayer button on the game's main menu?** Only a game started
from the launcher has it, and only when the TPF3-MP mod is installed and
activated (see "Installing") and TPF3-MP knows where the game's menu is
in its build. **Lobby in this window instead** in the launcher does
everything the button's window does, with the game started from the
launcher as before.

### Diagnostics

While you are connected, the launcher sends the lines of its logs to the
server you play on, so its operator can see what went wrong for you from
your support code or log session, without asking you for files (D10 and
its amendment). The lines are:

- the launcher's own log;
- the in-game hook's log (`hook.log`) and the game's own log
  (`stdout.txt`), from where they stood when you started the launcher;
- the text of the game's error reports (the `.txt` and `.json` files in
  its `crash_dump` folder) written since.

Never the game's crash dumps (`.dmp`), your saves, your identity key or
any file whose name looks like a key, certificate or token. Each source
sends only so much a minute, so a log that runs away sends its newest
lines and skips older ones. Before a line leaves your machine, paths are
cut to their last part (so your user name and your Steam account are not
in them), and IP addresses, invites, keys and passwords, account IDs,
e-mail addresses and Steam IDs are taken out; the server does the same
again. The server keeps the lines for a limited time, 30 days unless its
operator chose otherwise.

Set **Send diagnostics** to **Off**, in **Settings** (or untick it at the
bottom of the browser page), to stop: the launcher then sends nothing
more from any of these logs, forgets the lines it had not sent yet,
never sends what the logs gain while it is off, and remembers your
choice. The log session is not shown while it is off.

### The game's own logs

The game's crash dumps are never sent, and the logs' older parts (from
before you started the launcher) neither. When the operator needs them, `tpf3mp-agent collect-logs`, run from the TPF3-MP folder, puts them
into one zip with TPF3-MP's logs: `tpf3mp-logs-<time>.zip` in your
Downloads folder (in `TPF3-MP` when there is no Downloads folder).
`--out <folder>` puts it elsewhere, `--since 2h` takes a shorter window,
and `--game-log <file>` adds a log kept elsewhere. Attach the zip to your
report, with your support code.

The zip holds:

- `tpf3mp/logs/`: the launcher's logs, which record its crashes too;
- `tpf3mp/hook.log`: the in-game hook's log;
- `game/…`: the game's own log (`stdout.txt`) and crash reports, from
  `<Steam>/userdata/<account>/3493540/local/crash_dump/`, where Transport
  Fever 3 writes them on Windows. It also looks in `local/stdout.txt`,
  where Transport Fever 2 kept its log, marked "TPF2 location, not seen on
  TF3 Windows", until Linux and macOS are checked;
- `manifest.txt`: the versions of TPF3-MP, its protocol and its link to the
  game, your system, your support code when connected, every file with its
  size, and which places were not found.

Only files changed in the last week are taken, newest first, up to 64 MB;
a long text log that does not fit whole keeps its end. What was left out
is listed in the manifest.

The zip never holds your identity key, invite keys, certificates or
tokens: only the logs folders above are read, and any file there whose name
looks like a key, certificate or token is withheld all the same. Your saved
worlds and remembered server are not included, and TPF3-MP's logs hold no
IP addresses. The game's own logs are the game's: look through the zip
before sharing it publicly if you want to be sure.

- **"the server is out of reach over UDP (...) and through wss://..."**:
  your network blocks both routes, or the server is down. Some school,
  office and hotel networks block the UDP the game uses; the launcher then
  tries a WebSocket connection on port 443 by itself. If the server's
  operator gave you a tunnel address, start the launcher with
  `--tunnel <address>`; if your network never passes UDP, add
  `--tunnel-only`.
- **"connected via tunnel"** next to the connection: your network blocks
  UDP, and the game plays through the tunnel. It works, but lost packets
  cost a little more delay.
- **Under Proton or Wine** (Linux, Steam Deck) the Windows launcher plays
  over UDP as well: Wine refuses some socket options QUIC uses, so the
  launcher's log says it uses a plain UDP socket, which works the same.
  Should UDP not open at all, it takes the tunnel by itself, unless you
  started it with `--no-tunnel`. With `--game-exe`, the launcher shows that
  program's folder as the game's, never the native Linux game Steam may
  list beside it.
- **A version mismatch**: your package and the server are different
  versions. The message says which side is older.
- **"Your game differs from the room's"**: the window lists what to change:
  the game build, the mods you lack, the mods the room does not run, and
  the mods you have in another version. Everyone needs the owner's build
  and shared mods in the same order; personal mods are not compared. In the room, a **differ** pill next to a
  player shows whose game differs from the owner's; each player sees their
  own list. In the game's Multiplayer page the room's mods tab shows the
  same, mod by mod, and installs what you lack from Mod Hub.
- **"too many players are connected from this network"**: the server
  limits connections per network. Close another game, or ask the operator.
- **"that invite is for another server"**: this is the one-server or
  explicitly selected-server behavior. On a regional release, a bare code
  is checked against every listed region; choose a listed region if the
  invite is ambiguous.
- **"invite lookup is incomplete"**: one trusted region was unavailable,
  timed out or rate-limited its check. No room was joined. Check the
  connection and retry, or choose a listed region explicitly.
- **"the invite or password is not valid"**: the room closed, the code
  is mistyped, or the password is wrong.
- **"too many requests; try again in a moment"** when joining: too many
  wrong codes or passwords came from your network in the last 10
  minutes. Wait, then check the code.

The lobby and in-game Multiplayer window show **Copy** beside the invite code.
It copies only the room code, with brief **Copied** feedback.
