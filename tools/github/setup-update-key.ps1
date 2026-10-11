# Sets up the release update key, once, for the repository's owner
# (docs/OPERATIONS.md, "Before the first release", step 2):
#
# 1. asks where to keep the private key: a folder outside every git
#    repository, best a removable drive;
# 2. makes an ed25519 key there (openssl); the private key is never printed;
# 3. creates the `release` environment: you as required reviewer, only tags
#    `v*` may deploy to it;
# 4. stores the key as that environment's secret TPF3MP_UPDATE_SIGNING_KEY;
# 5. sets the repository variable TPF3MP_UPDATE_PUBLIC_KEY;
# 6. re-runs the last release run of main, which drafts the release.
#
# Run it yourself (setup-update-key.cmd, or this file with PowerShell): it
# acts as you, through your signed-in `gh`. It refuses when the repository
# already trusts an update key: replacing it would leave every launcher
# built since unable to update.
param([string]$Repo = "ReedME/Transport-Fever-3-Multiplayer-Mod", [string]$KeyDir = "")
# Not "Stop": Windows PowerShell 5 treats a native command's stderr as an
# error even when redirected; every step checks $LASTEXITCODE instead.
$ErrorActionPreference = "Continue"

function Step($text) { Write-Host ""; Write-Host "== $text" -ForegroundColor Cyan }
function Fail($text) { Write-Host ""; Write-Host "Stopped: $text" -ForegroundColor Red; exit 1 }

Step "Checking the tools and the repository"
$openssl = (Get-Command openssl -ErrorAction SilentlyContinue).Source
if (-not $openssl -and (Test-Path "C:\Program Files\Git\usr\bin\openssl.exe")) { $openssl = "C:\Program Files\Git\usr\bin\openssl.exe" }
if (-not $openssl) { Fail "openssl not found (it comes with Git for Windows)" }
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { Fail "the GitHub CLI (gh) is not installed" }
$me = gh api user --jq .login
if ($LASTEXITCODE -ne 0 -or -not $me) { Fail "gh is not signed in: run 'gh auth login'" }
$myId = gh api user --jq .id
$owner = gh api "repos/$Repo" --jq .owner.login
if ($me -ne $owner) { Fail "gh is signed in as $me, not the repository's owner $owner" }
$existing = gh variable get TPF3MP_UPDATE_PUBLIC_KEY --repo $Repo 2>$null
if ($LASTEXITCODE -eq 0 -and $existing) {
  Fail "the repository already trusts an update key ($existing). Launchers built with it update only from releases it signs; replace it only on purpose, by hand."
}
Write-Host "signed in as $me; openssl at $openssl"

Step "Where to keep the private key"
if (-not $KeyDir) {
  Add-Type -AssemblyName System.Windows.Forms
  $dialog = New-Object System.Windows.Forms.FolderBrowserDialog
  $dialog.Description = "Pick where to keep the TPF3-MP update key: outside any git repository, best a USB stick. Whoever has this file can publish updates to every player."
  $dialog.ShowNewFolderButton = $true
  if ($dialog.ShowDialog() -ne [System.Windows.Forms.DialogResult]::OK) { Fail "no folder picked" }
  $KeyDir = $dialog.SelectedPath
}
if (-not (Test-Path $KeyDir -PathType Container)) { Fail "$KeyDir is not a folder" }
# Never inside a git repository, where it could be committed.
git -C $KeyDir rev-parse --is-inside-work-tree 2>$null | Out-Null
if ($LASTEXITCODE -eq 0) { Fail "$KeyDir is inside a git repository; pick a folder outside every repository" }
$pem = Join-Path $KeyDir "tpf3mp-update-key.pem"
if (Test-Path $pem) { Fail "$pem already exists; move it away or pick another folder" }

Step "Making the key"
& $openssl genpkey -algorithm ed25519 -out $pem
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $pem)) { Fail "openssl could not make the key" }
# The public key: the last 32 bytes of its DER form, in base64. Through a
# file, not a pipe: PowerShell 5 pipes text, not bytes.
$der = [System.IO.Path]::GetTempFileName()
try {
  & $openssl pkey -in $pem -pubout -outform DER -out $der
  if ($LASTEXITCODE -ne 0) { Fail "openssl could not read the key back" }
  $bytes = [System.IO.File]::ReadAllBytes($der)
} finally { Remove-Item $der -ErrorAction SilentlyContinue }
if ($bytes.Length -lt 32) { Fail "the public key is too short" }
$public = [Convert]::ToBase64String([byte[]]$bytes[($bytes.Length - 32)..($bytes.Length - 1)])
Write-Host "private key: $pem"
Write-Host "public key:  $public"

Step "The release environment: you approve every signing, only v* tags"
$envBody = @{
  reviewers = @(@{ type = "User"; id = [int64]$myId })
  deployment_branch_policy = @{ protected_branches = $false; custom_branch_policies = $true }
} | ConvertTo-Json -Depth 4 -Compress
$envBody | gh api -X PUT "repos/$Repo/environments/release" --input - | Out-Null
if ($LASTEXITCODE -ne 0) { Fail "could not create the release environment (the key is made, nothing else is set: run this again with -KeyDir pointing elsewhere after moving $pem away)" }
$policies = gh api "repos/$Repo/environments/release/deployment-branch-policies" --jq '[.branch_policies[] | "\(.type):\(.name)"] | join(",")'
if ($policies -notmatch "tag:v\*") {
  gh api -X POST "repos/$Repo/environments/release/deployment-branch-policies" -f "name=v*" -f "type=tag" | Out-Null
  if ($LASTEXITCODE -ne 0) { Fail "could not limit the release environment to v* tags" }
}

Step "The signing key, as the environment's secret"
Get-Content -Raw $pem | gh secret set TPF3MP_UPDATE_SIGNING_KEY --env release --repo $Repo
if ($LASTEXITCODE -ne 0) { Fail "could not store the signing key" }

Step "The public key, as the repository variable"
gh variable set TPF3MP_UPDATE_PUBLIC_KEY --repo $Repo --body $public
if ($LASTEXITCODE -ne 0) { Fail "could not set TPF3MP_UPDATE_PUBLIC_KEY" }

Step "Checking"
$reviewers = gh api "repos/$Repo/environments/release" --jq '[.protection_rules[] | select(.type == "required_reviewers") | .reviewers[].reviewer.login] | join(",")'
$secrets = gh secret list --env release --repo $Repo
$variable = gh variable get TPF3MP_UPDATE_PUBLIC_KEY --repo $Repo
$tags = gh api "repos/$Repo/environments/release/deployment-branch-policies" --jq '[.branch_policies[] | "\(.type):\(.name)"] | join(",")'
Write-Host "release environment reviewers: $reviewers"
Write-Host "release environment deploys:   $tags"
Write-Host "release environment secrets:   $(($secrets | ForEach-Object { ($_ -split "\s+")[0] }) -join ', ')"
Write-Host "TPF3MP_UPDATE_PUBLIC_KEY:      $variable"
if ($reviewers -notmatch [regex]::Escape($me) -or $tags -notmatch "tag:v\*" -or "$secrets" -notmatch "TPF3MP_UPDATE_SIGNING_KEY" -or $variable -ne $public) {
  Fail "something did not take; check Settings, Environments, release"
}

Step "Drafting the release"
$run = gh run list --repo $Repo --workflow release.yml --branch main --limit 1 --json databaseId --jq '.[0].databaseId'
if ($run) {
  gh run rerun $run --repo $Repo | Out-Null
  Write-Host "re-ran release run $run; the draft appears under Releases in a few minutes"
} else {
  Write-Host "no release run of main yet; the next promotion to main drafts it"
}

Write-Host ""
Write-Host "Done." -ForegroundColor Green
Write-Host "Now move $pem to safe offline storage (an encrypted USB stick or a password manager) and delete it from this PC."
Write-Host "Never commit it, share it, or paste it anywhere: whoever has it can publish updates to every player."
