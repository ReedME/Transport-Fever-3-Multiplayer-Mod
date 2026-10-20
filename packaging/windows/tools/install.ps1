<#
Installs the TPF3-MP mod for Transport Fever 3, or takes it out again. This
file is the whole installer: INSTALL_TPF3MP.cmd and UNINSTALL_TPF3MP.cmd
only start it. It needs no administrator rights.

    INSTALL_TPF3MP.cmd                   finds your mods folder through Steam
    INSTALL_TPF3MP.cmd "<mods folder>"   or drop the mods folder onto it
    UNINSTALL_TPF3MP.cmd                 takes the mod out again

What it changes, and nothing else:

- The TPF3-MP mod, `mod\tpf3mp_1` in this package, goes into Steam's folder
  for your Transport Fever 3 mods, <Steam>\userdata\<account>\3493540\local\staging_area,
  or the folder -ModsDir names. A tpf3mp_1 already there is moved to
  %LOCALAPPDATA%\TPF3-MP\backups first.
- %LOCALAPPDATA%\TPF3-MP\installed.json records the version and where the
  mod went, which the launcher shows and the uninstall takes out.

It puts nothing in the game's folder. The game runs TPF3-MP only when the
TPF3-MP launcher starts it, for a multiplayer session; started from Steam,
it is the plain game, and the mod does nothing unless a game enables it.

It changes nothing while the game is running, or when its record names
anything but the mod. When a step fails, the steps before it are undone.
Nothing is deleted: what it replaces or takes out goes to the backups.
#>
[CmdletBinding()]
param(
    # Where the mod goes. Without it, Steam's folder for your Transport
    # Fever 3 mods.
    [Parameter(Position = 0)]
    [string]$ModsDir,
    # Where Steam is, when it is not where the registry says.
    [string]$SteamRoot,
    # Take out what an earlier install put in.
    [switch]$Uninstall,
    # A read-only preflight for the launcher's updater.
    [switch]$CheckOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$SteamApp = '3493540'
$ModName = 'tpf3mp_1'
$Package = Split-Path -Parent $PSScriptRoot
$Steps = New-Object System.Collections.Generic.List[hashtable]
# Set when a move failed and could not be put back by itself.
$Incomplete = $false

function Say([string]$Text) { Write-Host $Text }

# Rust's canonical Steam paths use the Win32 extended namespace. Windows
# PowerShell 5.1's FileSystem provider cannot create folders through that
# spelling (New-Item reports a null drive). Keep local and UNC paths intact
# while converting only these two equivalent namespace prefixes.
function Convert-InstallerPath([string]$Path) {
    if ($Path.StartsWith('\\?\UNC\', [StringComparison]::OrdinalIgnoreCase)) {
        return '\\' + $Path.Substring(8)
    }
    if ($Path -match '^\\\\\?\\[A-Za-z]:\\') { return $Path.Substring(4) }
    return $Path
}

function Get-Field($Object, [string]$Name) {
    if ($null -eq $Object) { return $null }
    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property) { return $null }
    return $property.Value
}

# Renames a file or folder on its drive in one step. A folder with a file
# open in it (an editor, a console standing in it) is refused whole, where
# Move-Item moves what it can file by file and stops partway.
function Rename-Whole([string]$Path, [string]$Target) {
    try {
        if (Test-Path -LiteralPath $Path -PathType Container) { [IO.Directory]::Move($Path, $Target) }
        else { [IO.File]::Move($Path, $Target) }
    }
    catch {
        $reason = $_.Exception
        if ($reason.InnerException) { $reason = $reason.InnerException }
        throw "Could not move $Path ($($reason.Message)) If a program or a console has it or a file in it open, close it, then try again."
    }
}

# Removes a file or folder, trying again for a few seconds: an antivirus
# scanning what was just copied holds its files open for a moment.
function Remove-Retrying([string]$Path) {
    for ($attempt = 1; ; $attempt++) {
        try {
            Remove-Item -LiteralPath $Path -Recurse -Force
            return
        }
        catch {
            if ($attempt -ge 6 -or -not (Test-Path -LiteralPath $Path)) { throw }
            Start-Sleep -Milliseconds 500
        }
    }
}

# Moves a file or folder to $Target, across volumes too. A rename does not
# cross volumes (another drive, or a junction to one, such as a moved
# AppData), so there it is first renamed beside itself, which refuses a
# folder in use before anything changes, then copied, and removed only
# once the copy is complete.
function Move-Path([string]$Path, [string]$Target) {
    if (Test-Path -LiteralPath $Target) { throw "$Target is already there" }
    try {
        Rename-Whole $Path $Target
        return
    }
    catch {
        if (-not (Test-Path -LiteralPath $Path)) { throw }
    }
    $aside = Join-Path (Split-Path -Parent $Path) ('.' + (Split-Path -Leaf $Path) + '-moving-' + [guid]::NewGuid().ToString('N'))
    Rename-Whole $Path $aside
    try { Copy-Item -LiteralPath $aside -Destination $Target -Recurse }
    catch {
        $failed = $_
        try { if (Test-Path -LiteralPath $Target) { Remove-Retrying $Target } }
        catch {
            $script:Incomplete = $true
            Write-Host "Could not remove the partial copy in ${Target}: $($_.Exception.Message)" -ForegroundColor Yellow
        }
        try { Rename-Whole $aside $Path }
        catch {
            $script:Incomplete = $true
            Write-Host "Could not put back ${Path}: it is in $aside." -ForegroundColor Yellow
        }
        throw $failed
    }
    try { Remove-Retrying $aside }
    catch {
        $script:Incomplete = $true
        Write-Host "Moved $Path to $Target, but could not remove what is left in ${aside}: delete it. ($($_.Exception.Message))" -ForegroundColor Yellow
    }
}

# Moves a file or folder into the backups, and says where it went.
function Move-Into([string]$Path, [string]$Folder) {
    $folderExisted = Test-Path -LiteralPath $Folder -PathType Container
    $parent = Split-Path -Parent $Folder
    $parentExisted = Test-Path -LiteralPath $parent -PathType Container
    $target = Join-Path $Folder (Split-Path -Leaf $Path)
    try {
        New-Item -ItemType Directory -Force -Path $Folder | Out-Null
        Move-Path $Path $target
    }
    catch {
        $failure = $_
        foreach ($directory in @(@{ Path = $Folder; Existed = $folderExisted }, @{ Path = $parent; Existed = $parentExisted })) {
            if ($directory.Existed -or -not (Test-Path -LiteralPath $directory.Path -PathType Container)) { continue }
            try {
                if (@(Get-ChildItem -LiteralPath $directory.Path -Force).Count -eq 0) {
                    Remove-Item -LiteralPath $directory.Path -Force
                }
            }
            catch {
                $script:Incomplete = $true
                Write-Host "Could not remove empty backup folder $($directory.Path): $($_.Exception.Message)" -ForegroundColor Yellow
            }
        }
        throw $failure
    }
    return $target
}

# Remembers how to take back a step, should a later one fail: move $Path
# back to $Back, remove $Path (a copy this run made), or rewrite $Path with
# the text $Back had (removing it when it had none).
function On-Failure([string]$Undo, [string]$Path, $Back) {
    $Steps.Insert(0, @{ Undo = $Undo; Path = $Path; Back = $Back })
}

function Get-SteamRoots {
    $candidates = New-Object System.Collections.Generic.List[string]
    if ($SteamRoot) {
        $candidates.Add((Convert-InstallerPath $SteamRoot))
    }
    else {
        try {
            $path = (Get-ItemProperty -LiteralPath 'HKCU:\Software\Valve\Steam' -Name SteamPath).SteamPath
            if ($path) { $candidates.Add($path.Replace('/', '\')) }
        }
        catch { }
        foreach ($base in @(${env:ProgramFiles(x86)}, $env:ProgramFiles)) {
            if ($base) { $candidates.Add((Join-Path $base 'Steam')) }
        }
    }
    $seen = @{}
    foreach ($candidate in $candidates) {
        if (-not (Test-LibraryPath $candidate Container)) { continue }
        $full = [IO.Path]::GetFullPath($candidate).TrimEnd('\')
        if ($seen.ContainsKey($full.ToLowerInvariant())) { continue }
        $seen[$full.ToLowerInvariant()] = $true
        $full
    }
}

# Whether a file or folder of Steam or one of its libraries is there. On an
# unavailable network share, Test-Path reports a small set of Win32 network
# errors instead of saying no; other inspection failures must stop the
# installer because it cannot safely check whether the game is running.
function Test-LibraryPath([string]$Path, [string]$Type) {
    try { return [bool](Test-Path -LiteralPath $Path -PathType $Type) }
    catch {
        $exception = $_.Exception
        while ($exception) {
            if ($exception -is [System.ComponentModel.Win32Exception] -and $exception.NativeErrorCode -in @(53, 64, 67, 1231, 1232)) {
                return $false
            }
            $exception = $exception.InnerException
        }
        throw
    }
}

# The game's folder, from Steam's list of libraries and the game's manifest
# in one of them: only to see whether the game is running.
function Find-Game {
    foreach ($root in @(Get-SteamRoots)) {
        $libraries = @($root)
        $folders = Join-Path $root 'steamapps\libraryfolders.vdf'
        if (Test-Path -LiteralPath $folders -PathType Leaf) {
            foreach ($line in Get-Content -LiteralPath $folders) {
                if ($line -match '^\s*"path"\s+"(.+)"\s*$') { $libraries += $Matches[1].Replace('\\', '\') }
            }
        }
        foreach ($library in $libraries) {
            # A library on a drive that is gone (an unplugged disk) or a
            # network share that is offline cannot be running the game.
            # Join-Path throws on a missing drive.
            if (-not (Test-LibraryPath $library Container)) { continue }
            $manifest = Join-Path $library "steamapps\appmanifest_$SteamApp.acf"
            if (-not (Test-LibraryPath $manifest Leaf)) { continue }
            foreach ($line in Get-Content -LiteralPath $manifest) {
                # One plain folder name under steamapps\common, nothing more.
                if ($line -match '^\s*"installdir"\s+"([^"\\/:*?<>|]+)"\s*$' -and $Matches[1] -notmatch '^\.+$') {
                    $dir = Join-Path $library ('steamapps\common\' + $Matches[1])
                    if (Test-LibraryPath $dir Container) { return $dir }
                }
            }
        }
    }
    return $null
}

# Steam's folder for this player's Transport Fever 3 mods: in the account
# that played it last. The game makes <account>\3493540\local when it
# first runs. Mods made for build 40391 install into its staging_area and
# are then activated in Mod Hub (investigation/TF3_MODS_2026-09-27.md).
function Find-ModsDir {
    $locals = @()
    foreach ($root in @(Get-SteamRoots)) {
        $userdata = Join-Path $root 'userdata'
        if (-not (Test-Path -LiteralPath $userdata -PathType Container)) { continue }
        foreach ($account in @(Get-ChildItem -LiteralPath $userdata -Directory)) {
            if ($account.Name -notmatch '^\d+$') { continue }
            $local = Join-Path $account.FullName "$SteamApp\local"
            if (Test-Path -LiteralPath $local -PathType Container) { $locals += Get-Item -LiteralPath $local }
        }
    }
    if ($locals.Count -eq 0) { return $null }
    $latest = @($locals | Sort-Object LastWriteTimeUtc -Descending)
    if ($latest.Count -gt 1) {
        Say "More than one Steam account has played Transport Fever 3 here; using the one that played last. Give the mods folder to choose."
    }
    return (Join-Path $latest[0].FullName 'staging_area')
}

function Assert-GameClosed {
    if (@(Get-Process -Name TransportFever3 -ErrorAction SilentlyContinue).Count -gt 0) {
        throw 'Close Transport Fever 3 first.'
    }
    $game = Find-Game
    if (-not $game) { return }
    $prefix = $game.TrimEnd('\') + '\'
    foreach ($process in @(Get-Process)) {
        $path = $null
        try { $path = $process.Path } catch { }
        if ($path -and $path.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Close Transport Fever 3 first: $($process.ProcessName) is running from its folder."
        }
    }
}

# What an earlier install recorded, refused unless it names the mod's own
# folder: the uninstall moves out what it names.
function Read-Record([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $null }
    try { $record = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json }
    catch { throw "$Path is damaged." }
    $mod = Convert-InstallerPath ([string](Get-Field $record 'mod'))
    if (-not [IO.Path]::IsPathRooted($mod) -or (Split-Path -Leaf $mod) -cne $ModName -or $mod -match '(^|[\\/])\.\.([\\/]|$)') {
        throw "$Path names something other than the TPF3-MP mod."
    }
    $record.mod = $mod
    return $record
}

function Write-Record([string]$Path, [string]$Version, [string]$Mod) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Path) | Out-Null
    $old = $null
    if (Test-Path -LiteralPath $Path) { $old = Get-Content -LiteralPath $Path -Raw }
    $record = [ordered]@{ version = $Version; mod = $Mod }
    [IO.File]::WriteAllText($Path, ($record | ConvertTo-Json))
    On-Failure 'Rewrite' $Path $old
}

# Copies the mod beside its place first, so the swap is a rename.
function Install-Mod([string]$Mods, [string]$Backups) {
    New-Item -ItemType Directory -Force -Path $Mods | Out-Null
    $target = Join-Path $Mods $ModName
    $staging = Join-Path $Mods ('.' + $ModName + '-install-' + [guid]::NewGuid().ToString('N'))
    try {
        Copy-Item -LiteralPath (Join-Path $Package "mod\$ModName") -Destination $staging -Recurse
        if (Test-Path -LiteralPath $target) {
            $kept = Move-Into $target $Backups
            On-Failure 'MoveBack' $kept $target
        }
        Rename-Whole $staging $target
        On-Failure 'Remove' $target
    }
    finally {
        if (Test-Path -LiteralPath $staging) {
            try { Remove-Retrying $staging }
            catch {
                $script:Incomplete = $true
                Write-Host "Could not remove the unfinished copy in ${staging}: delete it. ($($_.Exception.Message))" -ForegroundColor Yellow
            }
        }
    }
    return $target
}

function Install([string]$RecordPath, [string]$Backups) {
    $version = 'unknown'
    $marker = Join-Path $Package 'tpf3mp-package.json'
    if (Test-Path -LiteralPath $marker) { $version = [string](Get-Field (Get-Content -LiteralPath $marker -Raw | ConvertFrom-Json) 'version') }
    if (-not (Test-Path -LiteralPath (Join-Path $Package "mod\$ModName") -PathType Container)) {
        Say "This package has no TPF3-MP mod yet: there is nothing to install."
        return
    }
    $previous = Read-Record $RecordPath
    $mods = Convert-InstallerPath $ModsDir
    if (-not $mods -and $null -ne $previous) { $mods = Split-Path -Parent ([string](Get-Field $previous 'mod')) }
    if (-not $mods) { $mods = Find-ModsDir }
    if (-not $mods) {
        throw "Steam has no folder for your Transport Fever 3 mods yet: start the game once, then install again. Or drop the mods folder onto INSTALL_TPF3MP.cmd."
    }
    # A full path: renames go through .NET, which resolves a relative one
    # against another folder than PowerShell's current one.
    $mods = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($mods)
    Say "Mods folder: $mods"
    $mod = Install-Mod $mods $Backups
    Write-Record $RecordPath $version $mod
    Say "Installed the TPF3-MP mod in $mod."
    Say 'In the game, open Mod Hub, find TPF3-MP under your mods and click Activate.'
    Say ''
    Say "TPF3-MP $version is installed. Play by starting TPF3-MP.exe: the game runs TPF3-MP only when the launcher starts it."
}

function Remove-Install([string]$RecordPath, [string]$Backups) {
    $record = Read-Record $RecordPath
    if ($null -eq $record) { throw "The TPF3-MP mod is not installed." }
    $mod = [string](Get-Field $record 'mod')
    if (Test-Path -LiteralPath $mod -PathType Container) {
        $kept = Move-Into $mod $Backups
        On-Failure 'MoveBack' $kept $mod
        Say "Took the mod out of $(Split-Path -Parent $mod)."
    }
    $kept = Move-Into $RecordPath $Backups
    On-Failure 'MoveBack' $kept $RecordPath
    Say ''
    Say "The TPF3-MP mod is taken out."
}

try {
    if (-not $env:LOCALAPPDATA) { throw 'LOCALAPPDATA is not set, so there is nowhere for the record and backups.' }
    $data = Join-Path $env:LOCALAPPDATA 'TPF3-MP'
    $recordPath = Join-Path $data 'installed.json'
    $backups = Join-Path $data ('backups\' + (Get-Date -Format 'yyyyMMdd-HHmmss-fff'))
    Assert-GameClosed
    if ($CheckOnly) { exit 0 }
    if ($Uninstall) { Remove-Install $recordPath $backups } else { Install $recordPath $backups }
    if (Test-Path -LiteralPath $backups) { Say "What was replaced or taken out is in $backups." }
    exit 0
}
catch {
    $failure = $_.Exception.Message
    $undone = $true
    foreach ($step in $Steps) {
        try {
            switch ($step.Undo) {
                'MoveBack' { Move-Path $step.Path $step.Back }
                'Remove' { Remove-Item -LiteralPath $step.Path -Recurse -Force }
                'Rewrite' {
                    if ($null -ne $step.Back) { [IO.File]::WriteAllText($step.Path, $step.Back) }
                    else { Remove-Item -LiteralPath $step.Path -Force }
                }
            }
        }
        catch {
            $undone = $false
            Write-Host "Could not put back $($step.Path): $($_.Exception.Message)" -ForegroundColor Yellow
        }
    }
    # A move, before the failure or while putting back, left something
    # behind that it named.
    if ($Incomplete) { $undone = $false }
    Write-Host ''
    if ($undone) {
        Write-Host "Nothing was changed: $failure" -ForegroundColor Red
    }
    else {
        Write-Host "It failed, and not everything could be put back (see above): $failure" -ForegroundColor Red
        Write-Host "The backups are in $backups." -ForegroundColor Red
    }
    exit 1
}
