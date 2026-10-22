# Install cosmoscope on Windows 10/11 from the latest GitHub release.
#   irm https://raw.githubusercontent.com/phanminhtai1029/cosmoscope/main/install/install.ps1 | iex
# Installs into %LOCALAPPDATA%\Programs\cosmoscope and adds it to your user PATH.
# It runs in its own scope, so 'iex' leaves no settings or variables behind in your session.
& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'

    $repo = 'phanminhtai1029/cosmoscope'
    $asset = 'cosmoscope-windows-x86_64.exe'
    $base = "https://github.com/$repo/releases/latest/download"
    $dir = Join-Path $env:LOCALAPPDATA 'Programs\cosmoscope'
    $exe = Join-Path $dir 'cosmoscope.exe'

    if (-not [Environment]::Is64BitOperatingSystem) {
        throw 'cosmoscope needs 64-bit Windows.'
    }
    # Windows on ARM runs the x64 build through emulation, which Windows 11 has and Windows 10 lacks
    $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    if ($arch -eq 'ARM64' -and [Environment]::OSVersion.Version.Build -lt 22000) {
        throw 'cosmoscope needs Windows 11 on ARM; Windows 10 on ARM cannot run x64 programs.'
    }

    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    # Download beside the installed copy and check it first, so a broken download never
    # replaces a working install.
    $tmp = "$exe.download"
    $sums = "$dir\SHA256SUMS.download"
    Write-Host "Downloading $base/$asset"
    try {
        Invoke-WebRequest -Uri "$base/$asset" -OutFile $tmp -UseBasicParsing
        Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile $sums -UseBasicParsing
        $want = $null
        foreach ($line in Get-Content $sums) {
            if ($line -match '^([0-9a-fA-F]{64})\s+\*?(\S+)$' -and $Matches[2] -eq $asset) {
                $want = $Matches[1]
            }
        }
        if (-not $want) { throw "SHA256SUMS has no entry for $asset." }
        if ((Get-FileHash -Algorithm SHA256 -Path $tmp).Hash -ne $want) {
            throw 'The download does not match its SHA256 checksum. Please try again.'
        }
        # Windows will not overwrite a program that is running, but it will rename it. The
        # old copy is removed right away, or by the next install if it is still running.
        $old = $null
        if (Test-Path $exe) {
            $old = "$exe.$(Get-Random).old"
            Move-Item -LiteralPath $exe -Destination $old
        }
        try {
            Move-Item -LiteralPath $tmp -Destination $exe
        } catch {
            # put the working copy back, so a failed swap never leaves no cosmoscope.exe
            if ($old -and -not (Test-Path $exe)) { Move-Item -LiteralPath $old -Destination $exe }
            throw
        }
        Get-ChildItem -Path $dir -Filter 'cosmoscope.exe.*.old' | ForEach-Object {
            try { Remove-Item -LiteralPath $_.FullName -Force } catch { }
        }
    } finally {
        Remove-Item -LiteralPath $tmp, $sums -Force -ErrorAction SilentlyContinue
    }
    Unblock-File -Path $exe

    # Edit the registry value itself: [Environment]::SetEnvironmentVariable would store PATH
    # expanded, freezing every %VARIABLE% entry in it to today's value.
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
    try {
        $raw = [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
        $entries = @($raw -split ';' | Where-Object { $_ })
        $expanded = $entries | ForEach-Object { [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') }
        if ($expanded -notcontains $dir) {
            $kind = if ($key.GetValueNames() -contains 'Path') { $key.GetValueKind('Path') } else { 'ExpandString' }
            $key.SetValue('Path', (($entries + $dir) -join ';'), $kind)
            # Setting (and removing) any user variable tells Windows to reload the environment,
            # so new terminals see the new PATH.
            [Environment]::SetEnvironmentVariable('COSMOSCOPE_INSTALL', '1', 'User')
            [Environment]::SetEnvironmentVariable('COSMOSCOPE_INSTALL', $null, 'User')
            Write-Host "Added $dir to your user PATH."
        }
    } finally {
        $key.Close()
    }
    if (-not (($env:Path -split ';') -contains $dir)) { $env:Path = "$env:Path;$dir" }

    Write-Host "Installed: $exe"

    # Planets are drawn as real images in Windows Terminal, which Windows 11 has built in.
    # Where it is missing (Windows 10), offer to install it.
    $hasWt = [bool](Get-Command wt.exe -ErrorAction SilentlyContinue)
    if (-not $hasWt -and -not $env:CI -and (Get-Command winget.exe -ErrorAction SilentlyContinue)) {
        $answer = Read-Host 'Install Windows Terminal too, so planets are drawn as real images? [Y/n]'
        if ($answer -notmatch '^\s*n') {
            winget install --id Microsoft.WindowsTerminal --exact --source winget `
                --accept-package-agreements --accept-source-agreements
            $hasWt = [bool](Get-Command wt.exe -ErrorAction SilentlyContinue)
        }
    }
    Write-Host 'Run it with: cosmoscope   (open a new terminal window if the command is not found)'
    if (-not $hasWt) {
        Write-Host 'For planets drawn as real images, get Windows Terminal: https://aka.ms/terminal'
    }
}
