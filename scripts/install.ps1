#Requires -Version 5.1
# install.ps1 — download and install aix to %LOCALAPPDATA%\Programs\aix (or $env:AIX_INSTALL_DIR)
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$ErrorActionPreference = 'Stop'

$Repo       = 'get-aix/aix'
$Target     = 'x86_64-pc-windows-msvc'
$InstallDir = if ($env:AIX_INSTALL_DIR) { $env:AIX_INSTALL_DIR } `
              else { Join-Path $env:LOCALAPPDATA 'Programs\aix' }

$BaseUrl = "https://github.com/$Repo/releases/latest/download"
$Channel = if ($BaseUrl.EndsWith('/releases/download/nightly')) { 'nightly' } else { 'stable' }
$Asset   = "aix-$Target.exe"
$Dest    = Join-Path $InstallDir 'aix.exe'
$Tmp     = [IO.Path]::GetTempFileName()
$VersionTmp = $null

Write-Host "Downloading aix ($Target)..."
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

try {
    Invoke-WebRequest -Uri "$BaseUrl/$Asset" -OutFile $Tmp -UseBasicParsing

    # Verify against the release's SHA256SUMS (skipped for releases without one).
    $Sums = $null
    try {
        $Sums = (Invoke-WebRequest -Uri "$BaseUrl/SHA256SUMS" -UseBasicParsing).Content
        if ($Sums -is [byte[]]) { $Sums = [Text.Encoding]::UTF8.GetString($Sums) }
    } catch {
        Write-Warning 'SHA256SUMS not published for this release; skipping verification.'
    }
    if ($Sums) {
        $Line = $Sums -split "`n" | Where-Object { ($_ -split '\s+')[1] -in @($Asset, "*$Asset") }
        if (-not $Line) { throw "No checksum for $Asset in SHA256SUMS" }
        $Expected = ($Line -split '\s+')[0].ToLowerInvariant()
        $Actual   = (Get-FileHash -Algorithm SHA256 -Path $Tmp).Hash.ToLowerInvariant()
        if ($Expected -ne $Actual) { throw "Checksum mismatch for $Asset (expected $Expected, got $Actual)" }
        Write-Host 'Checksum verified.'
    }

    $UpdateTarget = 'stable'
    $InstalledBuild = 'stable'
    if ($Channel -eq 'nightly') {
        $VersionTmp = [IO.Path]::GetTempFileName()
        Invoke-WebRequest -Uri "$BaseUrl/NIGHTLY_VERSION" -OutFile $VersionTmp -UseBasicParsing
        if ($Sums) {
            $VersionLine = $Sums -split "`n" | Where-Object { ($_ -split '\s+')[1] -in @('NIGHTLY_VERSION', '*NIGHTLY_VERSION') }
            if (-not $VersionLine) { throw 'No checksum for NIGHTLY_VERSION in SHA256SUMS' }
            $ExpectedVersionHash = ($VersionLine -split '\s+')[0].ToLowerInvariant()
            $ActualVersionHash = (Get-FileHash -Algorithm SHA256 -Path $VersionTmp).Hash.ToLowerInvariant()
            if ($ExpectedVersionHash -ne $ActualVersionHash) {
                throw "Checksum mismatch for NIGHTLY_VERSION (expected $ExpectedVersionHash, got $ActualVersionHash)"
            }
        }
        $BuildId = [IO.File]::ReadAllText($VersionTmp).Trim()
        if ($BuildId -notmatch '^[0-9a-fA-F]{40}$') {
            throw 'NIGHTLY_VERSION does not contain a 40-character commit ID'
        }
        $UpdateTarget = 'nightly'
        $InstalledBuild = "nightly:$BuildId"
    }

    Move-Item -Force -Path $Tmp -Destination $Dest
} finally {
    if (Test-Path $Tmp) { Remove-Item -Force $Tmp }
    if ($VersionTmp -and (Test-Path $VersionTmp)) { Remove-Item -Force $VersionTmp }
}
Write-Host "Installed to $Dest"

$StatePath = Join-Path $InstallDir '.aix-update-state'
$StateTmp = Join-Path $InstallDir ('.aix-update-state.' + [guid]::NewGuid().ToString('N') + '.tmp')
try {
    [IO.File]::WriteAllText($StateTmp, "target=$UpdateTarget`ninstalled=$InstalledBuild`n", [Text.Encoding]::ASCII)
    Move-Item -Force -Path $StateTmp -Destination $StatePath
} finally {
    if (Test-Path $StateTmp) { Remove-Item -Force $StateTmp }
}
Write-Host "Update target: $UpdateTarget"

# Add to user PATH if not already present
$UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$Dirs = $UserPath -split ';' | Where-Object { $_ -ne '' }

if ($InstallDir -notin $Dirs) {
    $NewPath = ($Dirs + $InstallDir) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $NewPath, 'User')
    Write-Host "Added $InstallDir to your PATH."
    Write-Host 'Restart your terminal for the change to take effect.'
} else {
    Write-Host "$InstallDir is already in your PATH."
}
