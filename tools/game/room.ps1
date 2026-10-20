# Starts a room of real games on this PC and gets them playing one world
# (docs/GAME_TESTING.md, "Starting a room"):
#
# 1. installs the repository's mod into the game's staging area;
# 2. starts tpf3mp-rig: a throwaway local server, one agent and one game
#    per player, each game with the hook loaded, all in one room;
# 3. once the room's game starts and the host's (p1) hook.log says its main
#    menu is up, loads the fixture save in the host through its console;
#    the room saves that world, and the guests load it from their main menus; a guest that has not after -GuestWait seconds
#    fails the setup instead of loading a different world;
# 4. closes the host's console and zooms each game in a little.
#
# Prints "run: <folder>" and "games: p1=<pid> p2=<pid> ..."; the rig keeps
# running until the games quit (quit.ps1).
#
#   room.ps1 [-Players 2 (1 for one game alone, e.g. measuring)] [-Run name] [-Fixture tpf3mp_fixture3] [-NoInstall]
#
# Refuses while any Transport Fever 3, tpf3mp-rig or tpf3mp-server runs:
# they may be someone else's test. Existing processes are never stopped.
param(
  [ValidateRange(1,8)][int]$Players = 2,
  [ValidatePattern("^[A-Za-z0-9_-]+$")][string]$Run = ("run-" + (Get-Date -Format "MMdd-HHmmss")),
  [ValidatePattern("^[A-Za-z0-9_-]+$")][string]$Fixture = "tpf3mp_fixture3",
  [string]$GameBuild = "40420",
  [int]$Stagger = 25,
  [int]$MenuWait = 180,
  [int]$MenuSettle = 5,
  [int]$GuestWait = 150,
  [string]$CloseConsoleAt = "557,47",
  [switch]$NoInstall
)
$ErrorActionPreference = "Stop"
. "$PSScriptRoot\env.ps1"
if (-not $GameExe) { throw "Transport Fever 3 not found; set TPF3MP_GAME_EXE" }
if (-not $GameLocal) { throw "the game's userdata folder not found; set TPF3MP_GAME_LOCAL" }
$rig = "$Bin\tpf3mp-rig.exe"
if (-not (Test-Path $rig)) { throw "no $rig; build it: cargo build --release -p tpf3mp-testkit --bin tpf3mp-rig; then cargo build --release -p tpf3mp-hook --lib" }
if (-not (Test-Path "$Bin\tpf3mp_hook.dll")) { throw "no tpf3mp_hook.dll in $Bin; cargo build --release -p tpf3mp-hook" }
if (-not (Test-Path "$GameSaves\$Fixture.sav")) { throw "no save $Fixture in $GameSaves (docs/GAME_TESTING.md, 'Fixture saves')" }

$games = @(Get-Process TransportFever3 -ErrorAction SilentlyContinue)
$ours = @(Get-Process tpf3mp-rig, tpf3mp-server -ErrorAction SilentlyContinue)
if ($games.Count -gt 0 -or $ours.Count -gt 0) {
  $list = (@($games) + @($ours) | ForEach-Object { "$($_.ProcessName) $($_.Id)" }) -join ", "
  throw "Already running: $list. Quit only your own sessions first; this script never stops existing games or servers."
}

$runDir = "$Work\$Run"
if (Test-Path -LiteralPath $runDir) { throw "Run directory already exists: $runDir" }

if (-not $NoInstall) {
  $parent = [IO.Path]::GetFullPath((Join-Path $GameLocal 'staging_area'))
  $target = [IO.Path]::GetFullPath($ModStaging)
  if ($target -ne (Join-Path $parent 'tpf3mp_1')) { throw 'Unexpected staging target' }
  foreach ($path in @($GameLocal,$parent,$target)) {
    if ((Test-Path -LiteralPath $path) -and ((Get-Item -LiteralPath $path).Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw "Refusing linked staging path: $path" }
  }
  if (Test-Path -LiteralPath $target) { Remove-Item -LiteralPath $target -Recurse -Force -ErrorAction Stop }
  New-Item -ItemType Directory -Force $parent | Out-Null
  Copy-Item -Recurse "$Repo\mod\tpf3mp_1" $ModStaging
  "installed the mod into $ModStaging"
}

New-Item -ItemType Directory -Force $runDir | Out-Null
$rigArgs = @("--players", "$Players", "--stagger", "$Stagger", "--wait-for-games", "--server", "local",
  "--game", "`"$GameExe`"", "--data-root", "`"$runDir`"", "--game-build", $GameBuild)
$proc = Start-Process -FilePath $rig -ArgumentList $rigArgs -PassThru -WindowStyle Hidden `
  -RedirectStandardOutput "$runDir\rig.out" -RedirectStandardError "$runDir\rig.err"
Set-Content "$runDir\rig.pid" $proc.Id
"run: $runDir (rig pid $($proc.Id))"

# The room's game starts once every game's hook has attached.
$out = "$runDir\rig.out"
$deadline = (Get-Date).AddSeconds(600)
while ((Get-Date) -lt $deadline -and -not ((Test-Path $out) -and (Select-String -Path $out -Pattern "game started with|rig: stopped" -Quiet))) {
  if ($proc.HasExited) { break }
  Start-Sleep -Seconds 3
}
if (-not ((Test-Path $out) -and (Select-String -Path $out -Pattern "game started with" -Quiet))) {
  "the room did not start; the rig said:"
  Get-Content $out, "$runDir\rig.err" -ErrorAction SilentlyContinue | Select-Object -Last 15
  exit 1
}
$pids = @(Select-String -Path $out -Pattern "\(game pid (\d+)\)" | ForEach-Object { [int]$_.Matches[0].Groups[1].Value })
# The room-start message can precede the last launcher's PID message.
$pidDeadline = (Get-Date).AddSeconds(180)
while ($pids.Count -lt $Players -and -not $proc.HasExited -and (Get-Date) -lt $pidDeadline) {
  Start-Sleep -Milliseconds 250
  $pids = @(Select-String -Path $out -Pattern "\(game pid (\d+)\)" | ForEach-Object { [int]$_.Matches[0].Groups[1].Value })
}
if ($pids.Count -ne $Players) { throw 'Rig did not report every game PID' }

$load = 'local ns=app.SaveGameNamespace.getSavegame() for _,i in ipairs(app.findAllSavegames(ns)) do ' +
  'if i.saveName=="' + $Fixture + '" then local id=api.type.SavegameId.new() id.path=i.path ' +
  'id.saveGameName=i.saveName id.saveGameNamespace=ns print("@@loading ' + $Fixture + '") app.loadGame(id,false,nil) end end'
$consoled = @()
function Hook-Says([string]$player, [string]$pattern) {
  $log = "$runDir\$player\hook.log"
  (Test-Path $log) -and (Select-String -Path $log -Pattern $pattern -Quiet)
}
function Wait-Menu([string]$player, [int]$gp, [bool]$RequireRoom = $true) {
  # The console takes input once the game is at its main menu: the hook has
  # served the menu's page and, on one of the menu's frames, seen the room
  # begin. Then -MenuSettle seconds for the page to be built.
  $deadline = (Get-Date).AddSeconds($MenuWait)
  while (-not ((Hook-Says $player "main_page.tl SERVED") -and ((-not $RequireRoom) -or (Hook-Says $player "the room began at the main menu")))) {
    if (Hook-Says $player "main_page.tl MISSED") {
      throw "$player's main menu came without the mod's page; see $runDir\$player\hook.log. Games left running"
    }
    if (-not (Get-Process -Id $gp -ErrorAction SilentlyContinue)) {
      throw "$player's game (pid $gp) quit before its main menu; see $runDir\$player\hook.log and $GameStdout"
    }
    if ((Get-Date) -ge $deadline) {
      throw "$player's game was not at its main menu after $MenuWait s; nothing typed. Games left running in $runDir"
    }
    Start-Sleep -Seconds 1
  }
  Start-Sleep -Seconds $MenuSettle
}
function Load-Fixture([string]$player, [int]$gp) {
  Wait-Menu $player $gp
  $said = & "$PSScriptRoot\console.ps1" -GamePid $gp -Lua $load -Open
  $script:consoled += $gp
  if (-not ($said -match "@@loading $Fixture")) { "${player}: the console did not echo the fixture's load; waiting for the hook anyway" }
}

# Wait for every window to finish opening before typing into the host. A
# guest opening its main menu can otherwise steal focus halfway through the
# load command, leaving an incomplete Lua chunk and a room that never starts.
for ($i = 0; $i -lt $pids.Count; $i++) {
  Wait-Menu "p$($i + 1)" $pids[$i] $false
}
Load-Fixture "p1" $pids[0]
# The hook's own lines for the room's save; a bare "holding" also matches
# the paused-tick fix's "holding tickCount" lines.
$saved = "saved the world for the room|was not saved for the room|holding the world"
# Alone in its room, the host saves no world for guests: it plays at once.
if ($Players -eq 1) { $saved += "|playing the room's world" }
$deadline = (Get-Date).AddSeconds(150)
while ((Get-Date) -lt $deadline -and -not (Hook-Says "p1" $saved)) { Start-Sleep -Seconds 3 }
if (-not (Hook-Says "p1" $saved)) {
  throw "Host did not load the fixture within 150 s of typing it; see $runDir\p1\hook.log. Games left running"
}
for ($i = 1; $i -lt $pids.Count; $i++) {
  $player = "p$($i + 1)"
  $deadline = (Get-Date).AddSeconds($GuestWait)
  while ((Get-Date) -lt $deadline -and -not (Hook-Says $player "playing the room's world from its save")) { Start-Sleep -Seconds 3 }
  if (-not (Hook-Says $player "playing the room's world from its save")) {
    throw "$player did not load the shared snapshot. Test is not ready; games left running for diagnosis in $runDir"
  }
}
# A lone host plays from step 1; with guests the host loads the room's save too.
$hostReady = if ($Players -eq 1) { "playing the room's world from step 1" } else { "playing the room's world from its save" }
# The guests can be ready before the host has replaced its own world.
$deadline = (Get-Date).AddSeconds($GuestWait)
while ((Get-Date) -lt $deadline -and -not (Hook-Says "p1" $hostReady)) { Start-Sleep -Seconds 3 }
if (-not (Hook-Says "p1" $hostReady)) {
  throw "Host has not loaded the shared snapshot; test not ready in $runDir"
}
Start-Sleep -Seconds 15

$g = "$PSScriptRoot\gamewin.ps1"
$c = $CloseConsoleAt -split ","
foreach ($gp in $pids) {
  try {
    # The console the fixture's load left open.
    if ($consoled -contains $gp) { & $g click $c[0] $c[1] -GamePid $gp | Out-Null; Start-Sleep -Seconds 1 }
    & $g scroll 650 380 -Clicks 6 -GamePid $gp | Out-Null; Start-Sleep -Seconds 1
  } catch { "could not reach game $gp`: $_" }
}
for ($i = 0; $i -lt $pids.Count; $i++) {
  $player = "p$($i + 1)"
  "== $player"
  Get-Content "$runDir\$player\hook.log" -ErrorAction SilentlyContinue |
    Select-String "playing the room's world|$saved" |
    Select-Object -Last 2 | ForEach-Object { $_.Line }
}
"games: " + ((0..($pids.Count - 1) | ForEach-Object { "p$($_ + 1)=$($pids[$_])" }) -join " ")
