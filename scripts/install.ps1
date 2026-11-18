#Requires -Version 5.1
# install.ps1 — download and install aix to %LOCALAPPDATA%\Programs\aix (or $env:AIX_INSTALL_DIR)
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$ErrorActionPreference = 'Stop'

$Repo       = 'get-aix/aix'
$Target     = 'x86_64-pc-windows-msvc'
$InstallDir = if ($env:AIX_INSTALL_DIR) { $env:AIX_INSTALL_DIR } `
              else { Join-Path $env:LOCALAPPDATA 'Programs\aix' }

$BaseUrl = "https://github.com/$Repo/releases/latest/download"
$Asset   = "aix-$Target.exe"
$Dest    = Join-Path $InstallDir 'aix.exe'
$Tmp     = [IO.Path]::GetTempFileName()

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

    Move-Item -Force -Path $Tmp -Destination $Dest
} finally {
    if (Test-Path $Tmp) { Remove-Item -Force $Tmp }
}
Write-Host "Installed to $Dest"

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
