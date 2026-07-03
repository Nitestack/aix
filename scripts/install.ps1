#Requires -Version 5.1
# install.ps1 — download and install aix to %LOCALAPPDATA%\Programs\aix (or $env:AIX_INSTALL_DIR)
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$ErrorActionPreference = 'Stop'

$Repo       = 'Nitestack/aix'
$Target     = 'x86_64-pc-windows-msvc'
$InstallDir = if ($env:AIX_INSTALL_DIR) { $env:AIX_INSTALL_DIR } `
              else { Join-Path $env:LOCALAPPDATA 'Programs\aix' }

$Url  = "https://github.com/$Repo/releases/latest/download/aix-$Target.exe"
$Dest = Join-Path $InstallDir 'aix.exe'

Write-Host "Downloading aix ($Target)..."
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

Invoke-WebRequest -Uri $Url -OutFile $Dest -UseBasicParsing
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
