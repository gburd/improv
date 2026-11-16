<#
.SYNOPSIS
Install-test the Improv installer: install, run, upgrade, refuse a
downgrade, uninstall.

.DESCRIPTION
ci/windows-msi-test.ps1 -Msi improv-X.Y.Z-ARCH.msi -Newer improv-X.Y.(Z+1)-ARCH.msi [-Logs DIR]

Newer is the same programs built into an installer of a higher version
(ci/windows-msi.ps1 with the patch version plus one). Every step runs
msiexec quietly (/qn), per user, with a verbose log in Logs, and checks:

  1. Install Msi: the files under %LOCALAPPDATA%\Programs\Improv, the
     Start menu shortcut to improv-gui.exe, the bin folder on the user's
     PATH (in the registry, and in a new shell started from the updated
     environment, where `improv --version` must run by name), the
     Add/Remove Programs entry, and `improv-gui.exe --version`.
  2. Install Newer: an upgrade in place, one entry of the new version.
     (The programs are the same, so they still print the old version.)
  3. Install Msi again: refused with the downgrade message, Newer stays.
  4. Uninstall: no files, shortcut, PATH entry or entry left.

Run it as a user that has no Improv installed. It fails on the first
check that does not hold, with the logs left in Logs.
#>
param(
    [Parameter(Mandatory)] [string] $Msi,
    [Parameter(Mandatory)] [string] $Newer,
    [string] $Logs = (Join-Path ([IO.Path]::GetTempPath()) 'improv-msi-test')
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Msi = (Resolve-Path $Msi).Path
$Newer = (Resolve-Path $Newer).Path
New-Item -ItemType Directory -Force -Path $Logs | Out-Null
$Logs = (Resolve-Path $Logs).Path
$dir = Join-Path $env:LOCALAPPDATA 'Programs\Improv'
$bin = Join-Path $dir 'bin'
$programs = [Environment]::GetFolderPath('Programs')
$shortcut = Join-Path $programs 'Improv.lnk'
$files = 'bin\improv.exe', 'bin\improv-tui.exe', 'bin\improv-server.exe', 'bin\improv-gui.exe',
'README.md', 'LICENSE-APACHE', 'LICENSE-MIT', 'CHANGELOG.md'

function Fail([string] $what) {
    throw "windows-msi-test: $what (logs in $Logs)"
}

# The version an installer file carries, from its Property table.
function Get-MsiVersion([string] $path) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $db = $installer.OpenDatabase($path, 0)
    $view = $db.OpenView("SELECT Value FROM Property WHERE Property = 'ProductVersion'")
    $view.Execute() | Out-Null
    $view.Fetch().StringData(1)
}

# msiexec, waited for, quiet, per user, verbose log; its exit code.
function Invoke-Msiexec([string] $Action, [string] $Path, [string] $Log) {
    $a = @("/$Action", "`"$Path`"", '/qn', '/norestart', '/l*v', "`"$(Join-Path $Logs $Log)`"")
    (Start-Process -FilePath msiexec.exe -ArgumentList $a -Wait -PassThru).ExitCode
}

# The installed products named Improv, for this user, with what Settings,
# Apps (Add/Remove Programs) shows for each, from Windows Installer. NB:
# wrap a call in @(), since PowerShell unrolls a function's empty or
# one-item array into $null or the item.
function Get-ArpEntry {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    # Context 2, MSIINSTALLCONTEXT_USERUNMANAGED: installed per user, by
    # this user (1 is the per-user install an administrator assigned).
    foreach ($product in $installer.ProductsEx('', '', 2)) {
        $info = { param($name) $product.InstallProperty($name) }
        if ((& $info 'ProductName') -ne 'Improv') { continue }
        [pscustomobject]@{
            Code = $product.ProductCode()
            Version = & $info 'VersionString'
            Publisher = & $info 'Publisher'
            Url = & $info 'URLInfoAbout'
            Icon = & $info 'ProductIcon'
        }
    }
}

# The user's PATH as the registry holds it (what a new shell starts with).
function Get-UserPath {
    [Environment]::GetEnvironmentVariable('Path', 'User')
}

# Run `improv --version` by name in a new PowerShell whose PATH is the one
# a new login would get: the machine's, then the user's, from the registry.
function Invoke-ImprovFromNewShell {
    $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $cmd = "`$env:Path = '$machine;$(Get-UserPath)'; (Get-Command improv).Source; improv --version"
    $out = & powershell.exe -NoProfile -NonInteractive -Command $cmd
    if ($LASTEXITCODE -ne 0) { Fail "improv --version from a new shell exited $LASTEXITCODE" }
    $out
}

# A GUI-subsystem program: PowerShell does not wait for it unless piped.
function Invoke-GuiVersion {
    (& (Join-Path $bin 'improv-gui.exe') --version | Out-String).Trim()
}

function Assert-Installed([string] $version) {
    foreach ($f in $files) {
        if (-not (Test-Path -LiteralPath (Join-Path $dir $f))) { Fail "$f was not installed" }
    }
    if (-not (Test-Path -LiteralPath $shortcut)) { Fail "no Start menu shortcut $shortcut" }
    $lnk = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
    if ($lnk.TargetPath -ne (Join-Path $bin 'improv-gui.exe')) { Fail "the shortcut points at $($lnk.TargetPath)" }
    $entries = @((Get-UserPath) -split ';' | Where-Object { $_.TrimEnd('\') -eq $bin })
    if ($entries.Count -ne 1) { Fail "the user's PATH has $($entries.Count) entries for $bin" }
    $arp = @(Get-ArpEntry)
    if ($arp.Count -ne 1) { Fail "$($arp.Count) Add/Remove Programs entries for Improv" }
    $icon = [Environment]::ExpandEnvironmentVariables($arp[0].Icon)
    if ($arp[0].Version -ne $version -or $arp[0].Publisher -ne 'Greg Burd' -or
        $arp[0].Url -ne 'https://gburd.github.io/improv/' -or -not (Test-Path -LiteralPath $icon)) {
        Fail "Add/Remove Programs: $($arp[0] | Format-List | Out-String)"
    }
    $cli = @(Invoke-ImprovFromNewShell)
    if ($cli[0] -ne (Join-Path $bin 'improv.exe')) { Fail "a new shell finds improv at $($cli[0])" }
    $gui = Invoke-GuiVersion
    if ($gui -notmatch '^improv-gui \d') { Fail "improv-gui --version printed '$gui'" }
    Write-Output "  installed $version in $dir"
    Write-Output "  a new shell: $($cli -join ' / ')"
    Write-Output "  improv-gui.exe --version: $gui"
    Write-Output "  Start menu: $shortcut -> $($lnk.TargetPath)"
    Write-Output "  Add/Remove Programs: Improv $($arp[0].Version), $($arp[0].Publisher), $($arp[0].Url), icon $icon"
}

$old = Get-MsiVersion $Msi
$new = Get-MsiVersion $Newer
if ([version]$new -le [version]$old) { Fail "$Newer ($new) is not newer than $Msi ($old)" }
if (@(Get-ArpEntry).Count -or (Test-Path -LiteralPath $dir)) { Fail "Improv is already installed for this user" }

Write-Output "1. install $old"
$rc = Invoke-Msiexec -Action i -Path $Msi -Log install.log
if ($rc -ne 0) { Fail "msiexec /i $Msi exited $rc" }
Assert-Installed $old

Write-Output "2. upgrade to $new"
$rc = Invoke-Msiexec -Action i -Path $Newer -Log upgrade.log
if ($rc -ne 0) { Fail "msiexec /i $Newer exited $rc" }
Assert-Installed $new

Write-Output "3. downgrade to ${old}: refused"
# MajorUpgrade's launch condition stops the install with 1603 (a fatal
# error), after showing its message, which the log records.
$rc = Invoke-Msiexec -Action i -Path $Msi -Log downgrade.log
if ($rc -ne 1603) { Fail "msiexec /i $Msi over $new exited $rc, want 1603 (refused)" }
$why = 'A newer version of Improv is already installed'
if (-not (Select-String -Quiet -LiteralPath (Join-Path $Logs downgrade.log) -Encoding Unicode -Pattern $why)) {
    Fail 'the downgrade log does not say why it was refused'
}
Write-Output "  refused: $why."
Assert-Installed $new

Write-Output '4. uninstall'
$rc = Invoke-Msiexec -Action x -Path $Newer -Log uninstall.log
if ($rc -ne 0) { Fail "msiexec /x $Newer exited $rc" }
if (Test-Path -LiteralPath $dir) { Fail "$dir is still there: $(Get-ChildItem -Recurse -Name $dir)" }
if (Test-Path -LiteralPath $shortcut) { Fail "the Start menu shortcut is still there" }
if ((Get-UserPath) -split ';' | Where-Object { $_.TrimEnd('\') -eq $bin }) { Fail "the user's PATH still has $bin" }
if (@(Get-ArpEntry).Count) { Fail 'the Add/Remove Programs entry is still there' }
Write-Output "  nothing left: no $dir, no shortcut, no PATH entry, no Add/Remove Programs entry"
Write-Output "windows-msi-test: ok ($old, upgraded to $new, downgrade refused, uninstalled)"
