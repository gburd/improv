<#
.SYNOPSIS
Build the Improv installer, improv-<Version>-<Arch>.msi, from the programs
and documents of a release zip.

.DESCRIPTION
ci/windows-msi.ps1 -Version X.Y.Z -Arch x64|arm64 -Stage DIR -Out DIR

Stage is the unpacked release zip (improv-vX.Y.Z-<target>\): the four
programs, README.md, LICENSE-APACHE, LICENSE-MIT and CHANGELOG.md. The man
pages and completions in it are not installed. packaging\windows\improv.wxs
is the installer's source; this script builds it with the WiX Toolset 7.0.1
and checks it with Windows Installer's ICE validation. Out receives only the
.msi.

WiX comes from NuGet as the `wix` .NET tool package, pinned by version and
SHA-256, and runs on the .NET runtime (8 or later) that GitHub's Windows
runners have; nothing is installed. It is cached under %TEMP%.

Every WiX command gets -acceptEula wix7: WiX 7 refuses to run until its Open
Source Maintenance Fee EULA is accepted, and that switch is its documented
acceptance for build scripts and CI (https://wixtoolset.org/osmf/), which
writes no acceptance file. The maintainer accepted the EULA for Improv,
which is exempt from the fee (it makes less than US$10,000 a year).
#>
param(
    [Parameter(Mandatory)] [string] $Version,
    [Parameter(Mandatory)] [ValidateSet('x64', 'arm64')] [string] $Arch,
    [Parameter(Mandatory)] [string] $Stage,
    [Parameter(Mandatory)] [string] $Out
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$wixVersion = '7.0.1'
$wixSha256 = '25209A1F73B20106621F11ABE5E1A4E87C1D9F5C596795D246DE7BB64446DF17'

# Windows Installer compares three fields of at most 255, 255 and 65535,
# and a pre-release suffix is not a version to it.
if ($Version -notmatch '^(\d+)\.(\d+)\.(\d+)$' -or [int]$Matches[1] -gt 255 -or
    [int]$Matches[2] -gt 255 -or [int]$Matches[3] -gt 65535) {
    throw "windows-msi: version '$Version' is not one Windows Installer takes (X.Y.Z, X and Y at most 255)"
}
$root = Split-Path -Parent $PSScriptRoot
$Stage = (Resolve-Path $Stage).Path
foreach ($f in 'improv.exe', 'improv-tui.exe', 'improv-server.exe', 'improv-gui.exe',
    'README.md', 'LICENSE-APACHE', 'LICENSE-MIT', 'CHANGELOG.md') {
    if (-not (Test-Path -LiteralPath (Join-Path $Stage $f))) {
        throw "windows-msi: $Stage has no $f (want an unpacked release zip)"
    }
}
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$msi = Join-Path (Resolve-Path $Out).Path "improv-$Version-$Arch.msi"

# The pinned WiX: download it once, check it, unpack it.
$cache = Join-Path ([IO.Path]::GetTempPath()) "improv-wix-$wixVersion"
$wixDll = Join-Path $cache 'tools\net8.0\any\wix.dll'
if (-not (Test-Path -LiteralPath $wixDll)) {
    $zip = "$cache.zip"
    $url = "https://api.nuget.org/v3-flatcontainer/wix/$wixVersion/wix.$wixVersion.nupkg"
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $zip
    $sum = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash
    if ($sum -ne $wixSha256) {
        throw "windows-msi: wix.$wixVersion.nupkg has SHA-256 $sum, want $wixSha256"
    }
    Expand-Archive -Force -LiteralPath $zip -DestinationPath $cache
    Remove-Item -LiteralPath $zip
}

# Run wix.exe with Arguments (the command first); stop on a failure.
function Invoke-Wix([string[]] $Arguments) {
    & dotnet $wixDll @Arguments -acceptEula wix7
    if ($LASTEXITCODE -ne 0) { throw "windows-msi: wix $($Arguments[0]) failed ($LASTEXITCODE)" }
}

# -pdbtype none: no .wixpdb next to the .msi, which would be one more file
# in the release.
Invoke-Wix -Arguments @('build', '-arch', $Arch, '-pdbtype', 'none',
    '-d', "Version=$Version", '-d', "Stage=$Stage",
    '-d', "Icon=$root\crates\gui\assets\icon\improv.ico",
    '-o', $msi, "$root\packaging\windows\improv.wxs")
# Two ICEs are suppressed. ICE61 warns that a same-version MSI replaces
# the installed one, which improv.wxs asks for (AllowSameVersionUpgrades).
# ICE91 warns, per file, that a per-user folder does not move for a
# per-machine install, which this per-user-only package never does.
Invoke-Wix -Arguments @('msi', 'validate', '-sice', 'ICE61', '-sice', 'ICE91', $msi)
Write-Output "windows-msi: $msi"
