# Install cosmoscope on Windows 10/11 from the latest GitHub release.
#   irm https://raw.githubusercontent.com/phanminhtai1029/cosmoscope/main/install/install.ps1 | iex
# Installs into %LOCALAPPDATA%\Programs\cosmoscope and adds it to your user PATH.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$repo = 'phanminhtai1029/cosmoscope'
$dir = Join-Path $env:LOCALAPPDATA 'Programs\cosmoscope'
$exe = Join-Path $dir 'cosmoscope.exe'
$url = "https://github.com/$repo/releases/latest/download/cosmoscope-windows-x86_64.exe"

New-Item -ItemType Directory -Force -Path $dir | Out-Null
Write-Host "Downloading $url"
Invoke-WebRequest -Uri $url -OutFile $exe -UseBasicParsing
Unblock-File -Path $exe

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not $userPath) { $userPath = '' }
if (-not (($userPath -split ';') -contains $dir)) {
    $newPath = (($userPath.TrimEnd(';')), $dir | Where-Object { $_ }) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    Write-Host "Added $dir to your user PATH."
}
if (-not (($env:Path -split ';') -contains $dir)) { $env:Path = "$env:Path;$dir" }

Write-Host "Installed: $exe"
Write-Host "Run it with: cosmoscope   (open a new terminal window if the command is not found)"
