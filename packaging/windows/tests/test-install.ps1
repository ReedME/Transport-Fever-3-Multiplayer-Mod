# Tests tools\install.ps1 against made-up packages, Steam folders and games,
# the way players run it: in its own Windows PowerShell, through its exit
# code and the files it leaves. CI runs it on Windows; so can anyone:
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File packaging\windows\tests\test-install.ps1

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$Installer = Join-Path (Split-Path -Parent $PSScriptRoot) 'tools\install.ps1'
$Root = Join-Path ([IO.Path]::GetTempPath()) ('tpf3mp-install-test-' + [guid]::NewGuid().ToString('N'))
$Failures = 0

function Write-File([string]$Path, [string]$Text) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Path) | Out-Null
    [IO.File]::WriteAllText($Path, $Text)
}

function Read-File([string]$Path) { [IO.File]::ReadAllText($Path) }

function Check([bool]$Condition, [string]$What) {
    if (-not $Condition) { throw "expected: $What" }
}

# A package with the mod, a Steam folder with a game in a library and an
# account that has played it, and a folder of the test's own for what the
# installer keeps (%LOCALAPPDATA%).
function New-Setup([string]$Name) {
    $dir = Join-Path $Root $Name
    $package = Join-Path $dir 'package'
    New-Item -ItemType Directory -Force -Path (Join-Path $package 'tools') | Out-Null
    Copy-Item -LiteralPath $Installer -Destination (Join-Path $package 'tools\install.ps1')
    Write-File (Join-Path $package 'tpf3mp-package.json') '{"version":"9.8.7","platform":"windows-x64"}'
    Write-File (Join-Path $package 'mod\tpf3mp_1\mod.lua') '-- mod'
    Write-File (Join-Path $package 'mod\tpf3mp_1\res\x.lua') '-- x'
    $steam = Join-Path $dir 'Steam'
    $library = Join-Path $dir 'Library'
    $game = Join-Path $library 'steamapps\common\Transport Fever 3'
    Write-File (Join-Path $game 'TransportFever3.exe') 'exe'
    $escaped = $library.Replace('\', '\\')
    # Steam still lists a library on a drive that is gone and one on a
    # network share that cannot be reached (.invalid never resolves),
    # before the game's.
    $gone = [string](@([char[]]'ZYXWVUTSRQPONMLKJIHGFE' | Where-Object { -not (Get-PSDrive -Name $_ -ErrorAction SilentlyContinue) })[0]) + ':\\SteamLibrary'
    $offline = '\\\\tpf3mp-offline.invalid\\SteamLibrary'
    Write-File (Join-Path $steam 'steamapps\libraryfolders.vdf') "`"libraryfolders`"`n{`n`t`"0`"`n`t{`n`t`t`"path`"`t`t`"$gone`"`n`t}`n`t`"1`"`n`t{`n`t`t`"path`"`t`t`"$offline`"`n`t}`n`t`"2`"`n`t{`n`t`t`"path`"`t`t`"$escaped`"`n`t}`n}`n"
    Write-File (Join-Path $library 'steamapps\appmanifest_3493540.acf') "`"AppState`"`n{`n`t`"appid`"`t`t`"3493540`"`n`t`"installdir`"`t`t`"Transport Fever 3`"`n}`n"
    $mods = Join-Path $steam 'userdata\12345\3493540\local\staging_area'
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $mods) | Out-Null
    return @{
        Package = $package
        Steam = $steam
        Game = $game
        Mods = $mods
        Data = (Join-Path $dir 'localappdata')
        Record = (Join-Path $dir 'localappdata\TPF3-MP\installed.json')
    }
}

# Runs the package's installer as a player would; returns its exit code
# and what it printed.
function Invoke-Installer($Setup, [string[]]$Arguments) {
    $saved = $env:LOCALAPPDATA
    $env:LOCALAPPDATA = $Setup.Data
    try {
        $script = Join-Path $Setup.Package 'tools\install.ps1'
        $all = @('-SteamRoot', $Setup.Steam) + $Arguments
        $output = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $script @all 2>&1 | Out-String
        return @{ Code = $LASTEXITCODE; Output = $output }
    }
    finally {
        $env:LOCALAPPDATA = $saved
    }
}

function Assert-Code($Result, [int]$Code) {
    Check ($Result.Code -eq $Code) "exit code $Code, got $($Result.Code): $($Result.Output)"
}

# Nothing in the game's folder but the game.
function Assert-GameUntouched($Setup) {
    $files = @(Get-ChildItem -LiteralPath $Setup.Game -Recurse -Force | ForEach-Object { $_.Name })
    Check (($files -join ',') -eq 'TransportFever3.exe') "the game's folder untouched, found $($files -join ',')"
}

function Test([string]$Name, [scriptblock]$Body) {
    try {
        & $Body
        Write-Host "ok   $Name"
    }
    catch {
        $script:Failures += 1
        Write-Host "FAIL $Name`n     $($_.Exception.Message)" -ForegroundColor Red
    }
}

Test "installs the mod in Steam's mods folder, and nothing in the game's" {
    $s = New-Setup 'install'
    Assert-Code (Invoke-Installer $s @()) 0
    Check ((Read-File "$($s.Mods)\tpf3mp_1\res\x.lua") -eq '-- x') 'the mod installed'
    $record = Read-File $s.Record | ConvertFrom-Json
    Check ($record.version -eq '9.8.7') 'the version recorded'
    # Compared by what is there: runners' temp folders may be spelled short.
    Check ((Split-Path -Leaf $record.mod) -eq 'tpf3mp_1' -and (Read-File "$($record.mod)\res\x.lua") -eq '-- x') 'where the mod went recorded'
    Assert-GameUntouched $s

    # Again, with a newer mod: the old one goes to the backups.
    Write-File "$($s.Package)\mod\tpf3mp_1\res\x.lua" '-- x 2'
    Assert-Code (Invoke-Installer $s @()) 0
    Check ((Read-File "$($s.Mods)\tpf3mp_1\res\x.lua") -eq '-- x 2') 'the newer mod'
    Check (@(Get-ChildItem $s.Mods -Force).Count -eq 1) 'no staging folder left in the mods folder'

    Assert-Code (Invoke-Installer $s @('-Uninstall')) 0
    Check (-not (Test-Path "$($s.Mods)\tpf3mp_1")) 'the mod gone'
    Check (-not (Test-Path $s.Record)) 'the record gone'
    Check (@(Get-ChildItem -Recurse -Filter 'x.lua' "$($s.Data)\TPF3-MP\backups").Count -ge 2) 'the mods kept in the backups'
    Assert-GameUntouched $s
    Assert-Code (Invoke-Installer $s @('-Uninstall')) 1
}

Test 'installs into a mods folder given' {
    $s = New-Setup 'given'
    $elsewhere = Join-Path (Split-Path -Parent $s.Game) 'elsewhere'
    Assert-Code (Invoke-Installer $s @($elsewhere)) 0
    Check (Test-Path "$elsewhere\tpf3mp_1\mod.lua") 'the mod where it was told'
}

Test 'installs, repairs and uninstalls with a canonical Windows mods path' {
    $s = New-Setup 'canonical path with spaces'
    $canonical = '\\?\' + $s.Mods
    Assert-Code (Invoke-Installer $s @('-ModsDir', $canonical)) 0
    Check ((Read-File "$($s.Mods)\tpf3mp_1\res\x.lua") -eq '-- x') 'canonical path installed'
    $record = Read-File $s.Record | ConvertFrom-Json
    Check ($record.mod -eq "$($s.Mods)\tpf3mp_1") 'record uses a provider-compatible path'

    # Older records and explicitly provided Steam roots can also be canonical.
    $record.mod = '\\?\' + $record.mod
    Write-File $s.Record ($record | ConvertTo-Json)
    Write-File "$($s.Package)\mod\tpf3mp_1\res\x.lua" '-- repaired'
    $s.Steam = '\\?\' + $s.Steam
    Assert-Code (Invoke-Installer $s @()) 0
    Check ((Read-File "$($s.Mods)\tpf3mp_1\res\x.lua") -eq '-- repaired') 'repair uses recorded canonical path'
    $record = Read-File $s.Record | ConvertFrom-Json
    $record.mod = '\\?\' + $record.mod
    Write-File $s.Record ($record | ConvertTo-Json)
    Assert-Code (Invoke-Installer $s @('-Uninstall')) 0
    Check (-not (Test-Path "$($s.Mods)\tpf3mp_1")) 'canonical recorded mod uninstalled'
    Assert-GameUntouched $s
}

Test 'converts extended UNC paths without dropping the server or share' {
    # Load just the pure helper; do not execute the installer or touch a share.
    $tokens = $null
    $parseErrors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile($Installer, [ref]$tokens, [ref]$parseErrors)
    $helper = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Convert-InstallerPath'
    }, $true)
    . ([scriptblock]::Create($helper.Extent.Text))
    Check ((Convert-InstallerPath '\\?\UNC\server\share\staging_area') -eq '\\server\share\staging_area') 'UNC server and share preserved'
    Check ((Convert-InstallerPath '\\server\share\staging_area') -eq '\\server\share\staging_area') 'ordinary UNC unchanged'
    Check ((Convert-InstallerPath 'C:\Steam\staging_area') -eq 'C:\Steam\staging_area') 'ordinary drive unchanged'
}

Test 'removes a folder whose file another program holds for a moment' {
    # Load just the helper, as an antivirus scanning a fresh copy would
    # hold one of its files open for a second.
    $tokens = $null
    $parseErrors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile($Installer, [ref]$tokens, [ref]$parseErrors)
    $helper = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Remove-Retrying'
    }, $true)
    . ([scriptblock]::Create($helper.Extent.Text))
    $dir = Join-Path $Root 'held'
    Write-File "$dir\a.lua" 'a'
    Write-File "$dir\res\b.lua" 'b'
    $ready = "$dir.ready"
    $holder = Start-Process -FilePath powershell.exe -WindowStyle Hidden -PassThru -ArgumentList @(
        '-NoProfile', '-Command',
        "`$h = [IO.File]::Open('$dir\res\b.lua', 'Open', 'Read', 'Read'); [IO.File]::WriteAllText('$ready', ''); Start-Sleep -Milliseconds 1500; `$h.Dispose()")
    try {
        $deadline = (Get-Date).AddSeconds(30)
        while (-not (Test-Path $ready)) {
            Check ((Get-Date) -lt $deadline) 'the holder started'
            Start-Sleep -Milliseconds 100
        }
        Remove-Retrying $dir
        Check (-not (Test-Path $dir)) 'the folder removed once the file is free'
    }
    finally {
        Stop-Process -Id $holder.Id -Force -ErrorAction SilentlyContinue
    }
}

Test 'refuses a record that names anything but the mod' {
    $s = New-Setup 'record'
    Assert-Code (Invoke-Installer $s @()) 0
    foreach ($bad in @('C:\\Windows', "$($s.Mods.Replace('\', '\\'))\\..\\tpf3mp_1", 'tpf3mp_1')) {
        Write-File $s.Record "{`"version`":`"1`",`"mod`":`"$bad`"}"
        Assert-Code (Invoke-Installer $s @('-Uninstall')) 1
        Assert-Code (Invoke-Installer $s @()) 1
        Check ((Read-File "$($s.Mods)\tpf3mp_1\mod.lua") -eq '-- mod') "the mod untouched with $bad"
    }
}

Test "installs into a mods folder given when Steam's folder is on a share that cannot be reached" {
    $s = New-Setup 'offline steam'
    $s.Steam = '\\tpf3mp-offline.invalid\Steam'
    Assert-Code (Invoke-Installer $s @($s.Mods)) 0
    Check ((Read-File "$($s.Mods)\tpf3mp_1\res\x.lua") -eq '-- x') 'the mod installed'
}

Test 'does not treat an unexpected Steam path inspection error as a missing path' {
    $tokens = $null
    $parseErrors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile($Installer, [ref]$tokens, [ref]$parseErrors)
    $helper = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Test-LibraryPath'
    }, $true)
    . ([scriptblock]::Create($helper.Extent.Text))

    $oldFunction = Get-Item Function:\Test-Path -ErrorAction SilentlyContinue
    Set-Item Function:\Test-Path -Value {
        throw [System.IO.IOException]::new('Inspection failed.', [System.ComponentModel.Win32Exception]::new(5))
    }
    try {
        $propagated = $false
        try { $null = Test-LibraryPath 'C:\private\Steam' Container }
        catch [System.IO.IOException] { $propagated = $true }
        Check $propagated 'an unexpected path inspection error propagates'
    }
    finally {
        if ($oldFunction) { Set-Item Function:\Test-Path -Value $oldFunction.ScriptBlock }
        else { Remove-Item Function:\Test-Path -ErrorAction SilentlyContinue }
    }
}

Test 'says when Steam has no mods folder for the game yet' {
    $s = New-Setup 'fresh'
    Remove-Item -Recurse "$($s.Steam)\userdata"
    $result = Invoke-Installer $s @()
    Assert-Code $result 1
    Check ($result.Output -match 'start the game once') "says to start the game once: $($result.Output)"
    Check (-not (Test-Path $s.Record)) 'no record'
}

Test 'puts everything back when a step fails' {
    $s = New-Setup 'rollback'
    Assert-Code (Invoke-Installer $s @()) 0
    # The record cannot be rewritten: a folder stands in its way.
    Remove-Item $s.Record
    New-Item -ItemType Directory -Path $s.Record | Out-Null
    Write-File "$($s.Package)\mod\tpf3mp_1\mod.lua" '-- new'
    Assert-Code (Invoke-Installer $s @()) 1
    Check ((Read-File "$($s.Mods)\tpf3mp_1\mod.lua") -eq '-- mod') 'the installed mod put back'
    Check (@(Get-ChildItem $s.Mods -Force).Count -eq 1) 'nothing left over'
}

# Updates and uninstalls with a file of the installed mod held open, the way
# an editor or a console standing in its folder holds it, then without.
function Assert-OpenFileRefused($Setup) {
    Assert-Code (Invoke-Installer $Setup @()) 0
    Write-File "$($Setup.Package)\mod\tpf3mp_1\mod.lua" '-- new'
    $open = [IO.File]::Open("$($Setup.Mods)\tpf3mp_1\res\x.lua", 'Open', 'Read', 'Read')
    $backupRoot = Join-Path $Setup.Data 'TPF3-MP\backups'
    $backupRootExistedBefore = Test-Path -LiteralPath $backupRoot -PathType Container
    $backupNamesBefore = @()
    if ($backupRootExistedBefore) {
        $backupNamesBefore = @(Get-ChildItem -LiteralPath $backupRoot -Directory | Select-Object -ExpandProperty Name)
    }
    try {
        foreach ($uninstall in @($false, $true)) {
            $result = Invoke-Installer $Setup @(if ($uninstall) { '-Uninstall' })
            Assert-Code $result 1
            Check ($result.Output -match 'close it, then try again') "says to close what has it open: $($result.Output)"
            Check ($result.Output -match 'Nothing was changed') "says nothing was changed: $($result.Output)"
            Check ((Read-File "$($Setup.Mods)\tpf3mp_1\mod.lua") -eq '-- mod') "the installed mod untouched (uninstall: $uninstall)"
            Check (Test-Path "$($Setup.Mods)\tpf3mp_1\res\x.lua") "the open file still there (uninstall: $uninstall)"
            Check (Test-Path $Setup.Record) 'the record kept'
            Check ((Test-Path -LiteralPath $backupRoot -PathType Container) -eq $backupRootExistedBefore) "backup folder existence unchanged (uninstall: $uninstall)"
            $backupNamesAfter = @()
            if ($backupRootExistedBefore) {
                $backupNamesAfter = @(Get-ChildItem -LiteralPath $backupRoot -Directory | Select-Object -ExpandProperty Name)
            }
            Check (($backupNamesAfter -join '|') -eq ($backupNamesBefore -join '|')) "no backup directory left by the refused move (uninstall: $uninstall)"
        }
    }
    finally {
        $open.Dispose()
    }
    Check (@(Get-ChildItem $Setup.Mods -Force).Count -eq 1) 'nothing left over in the mods folder'
    Assert-Code (Invoke-Installer $Setup @()) 0
    Check ((Read-File "$($Setup.Mods)\tpf3mp_1\mod.lua") -eq '-- new') 'updated once the file is closed'
    Check (@(Get-ChildItem -Recurse -Filter 'mod.lua' "$($Setup.Data)\TPF3-MP\backups" | Where-Object { (Read-File $_.FullName) -eq '-- mod' }).Count -eq 1) 'the old mod in the backups'
    Check (@(Get-ChildItem $Setup.Mods -Force).Count -eq 1) 'nothing left over after the update'
}

Test 'changes nothing while a file in the installed mod is open' {
    Assert-OpenFileRefused (New-Setup 'open file')
}

Test 'changes nothing while a file in the installed mod is open, on another drive than the backups' {
    $s = New-Setup 'other drive'
    # A drive letter of its own for the Steam folder, so moving into the
    # backups crosses drives and has to copy.
    $letter = [string](@([char[]]'ZYXWVUTSRQPONMLKJIHGFE' | Where-Object { -not (Get-PSDrive -Name $_ -ErrorAction SilentlyContinue) })[0]) + ':'
    subst $letter (Split-Path -Parent $s.Steam)
    Check ($LASTEXITCODE -eq 0) "subst $letter"
    try {
        $s.Steam = "$letter\Steam"
        $s.Mods ="$letter\Steam\userdata\12345\3493540\local\staging_area"
        Assert-OpenFileRefused $s
    }
    finally {
        subst $letter /D
    }
}

Test 'changes nothing while a file in the installed mod is open, with the backups on another volume behind a junction' {
    # A rename does not cross volumes even where the drive letters match,
    # as with an AppData moved to another disk. Needs a folder on another
    # volume: TPF3MP_TEST_OTHER_VOLUME, or the CI runner's own temp folder.
    $other = $env:TPF3MP_TEST_OTHER_VOLUME
    if (-not $other -and $env:RUNNER_TEMP -and [IO.Path]::GetPathRoot($env:RUNNER_TEMP) -ne [IO.Path]::GetPathRoot($Root)) { $other = $env:RUNNER_TEMP }
    if (-not $other) {
        Write-Host '     skipped: no folder on another volume (set TPF3MP_TEST_OTHER_VOLUME)'
        return
    }
    $s = New-Setup 'junction'
    $far = Join-Path $other ('tpf3mp-install-test-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $far | Out-Null
    cmd /c mklink /J "$($s.Data)" "$far" | Out-Null
    Check ($LASTEXITCODE -eq 0) "a junction to $far"
    try {
        Assert-OpenFileRefused $s
    }
    finally {
        # Not through Remove-Item: Windows PowerShell deletes what a
        # junction points to.
        cmd /c rmdir "$($s.Data)" | Out-Null
        Remove-Item -LiteralPath $far -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Test 'refuses while the game runs' {
    $s = New-Setup 'running'
    $exe = Join-Path $s.Game 'Running.exe'
    Copy-Item "$env:SystemRoot\System32\PING.EXE" $exe
    $game = Start-Process -FilePath $exe -ArgumentList '-n', '30', '127.0.0.1' -WindowStyle Hidden -PassThru
    try {
        Start-Sleep -Milliseconds 500
        $result = Invoke-Installer $s @()
        Assert-Code $result 1
        Check ($result.Output -match 'Close Transport Fever 3') "says to close the game: $($result.Output)"
        Check (-not (Test-Path "$($s.Mods)\tpf3mp_1")) 'no mod'
    }
    finally {
        Stop-Process -Id $game.Id -Force -ErrorAction SilentlyContinue
    }
}

Remove-Item -Recurse -Force $Root -ErrorAction SilentlyContinue
if ($Failures -gt 0) {
    Write-Host "$Failures installer test(s) failed" -ForegroundColor Red
    exit 1
}
Write-Host 'all installer tests passed'
