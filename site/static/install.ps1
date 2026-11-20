# Installs tvpipe from its GitHub Releases on Windows.
#
#   irm https://tvpipe.party/install.ps1 | iex
#
# Environment:
#   TVPIPE_VERSION      version to install, e.g. 0.1.0 (default: the latest)
#   TVPIPE_INSTALL_DIR  directory for tvpipe.exe (default: %LOCALAPPDATA%\tvpipe\bin)
#
# Downloads tvpipe-VERSION-TARGET.zip and SHA256SUMS from the release,
# checks the archive's SHA-256, copies tvpipe.exe into the install directory
# and adds that directory to the user PATH. Works in Windows PowerShell 5.1
# and PowerShell 7.

# The newest tvpipe version (X.Y.Z) among GitHub releases (the API's
# objects): tags vX.Y.Z only, no drafts or prereleases; $null if none.
function Select-TvpipeVersion($releases) {
    $best = $releases |
        Where-Object { "$($_.tag_name)" -match '^v\d+\.\d+\.\d+$' -and -not $_.draft -and -not $_.prerelease } |
        ForEach-Object { [version]("$($_.tag_name)".Substring(1)) } |
        Sort-Object -Descending |
        Select-Object -First 1
    if ($best) { return "$best" }
    return $null
}

function Install-Tvpipe {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'  # Invoke-WebRequest is slow with it
    $repo = 'mitsuhiko/tvpipe'

    if ($PSVersionTable.PSVersion.Major -lt 6) {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    }

    # The machine's architecture, also from an emulated (x64 on ARM64) shell.
    $arch = (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment' -ErrorAction SilentlyContinue).PROCESSOR_ARCHITECTURE
    if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
    switch ($arch) {
        'AMD64' { $target = 'x86_64-pc-windows-msvc' }
        'ARM64' { $target = 'aarch64-pc-windows-msvc' }
        default { throw "no prebuilt tvpipe for the $arch architecture (build it from source: https://github.com/$repo)" }
    }

    $version = "$env:TVPIPE_VERSION".Trim() -replace '^v', ''
    if (-not $version) {
        # The highest vX.Y.Z tag among the releases: games have their own
        # releases in the repository (NAME-vX.Y.Z), so the newest release
        # or GitHub's "latest" need not be tvpipe's.
        try {
            $releases = Invoke-RestMethod -UseBasicParsing -Headers @{ Accept = 'application/vnd.github+json' } "https://api.github.com/repos/$repo/releases?per_page=100"
            $version = Select-TvpipeVersion $releases
        } catch {
            $version = $null
        }
        if (-not $version) {
            # The API allows 60 requests an hour without a token: GitHub's
            # latest release, which the release workflow marks.
            try {
                $release = Invoke-RestMethod -UseBasicParsing "https://api.github.com/repos/$repo/releases/latest"
            } catch {
                throw "could not find the latest release of ${repo}: $($_.Exception.Message)"
            }
            if ($release.tag_name -notmatch '^v\d+\.\d+\.\d+$') { throw "the latest release of $repo is not tvpipe's ($($release.tag_name))" }
            $version = $release.tag_name -replace '^v', ''
        }
    }

    $dir = $env:TVPIPE_INSTALL_DIR
    if (-not $dir) { $dir = Join-Path $env:LOCALAPPDATA 'tvpipe\bin' }

    $name = "tvpipe-$version-$target"
    $base = "https://github.com/$repo/releases/download/v$version"
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("tvpipe-install-" + [Guid]::NewGuid())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        Write-Host "Installing tvpipe $version ($target) into $dir"
        $zip = Join-Path $tmp "$name.zip"
        $sums = Join-Path $tmp 'SHA256SUMS'
        foreach ($d in @(@("$base/$name.zip", $zip), @("$base/SHA256SUMS", $sums))) {
            try {
                Invoke-WebRequest -UseBasicParsing -Uri $d[0] -OutFile $d[1]
            } catch {
                throw "could not download $($d[0]): $($_.Exception.Message)"
            }
        }

        $expected = $null
        foreach ($line in Get-Content $sums) {
            $parts = $line -split '\s+', 2
            if ($parts.Count -eq 2 -and $parts[1].TrimStart('*') -eq "$name.zip") { $expected = $parts[0].ToLower() }
        }
        if (-not $expected) { throw "SHA256SUMS has no entry for $name.zip" }
        $actual = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
        if ($expected -ne $actual) { throw "checksum mismatch for $name.zip (expected $expected, got $actual)" }

        Expand-Archive -Path $zip -DestinationPath $tmp -Force
        $exe = Join-Path $tmp "$name\tvpipe.exe"
        if (-not (Test-Path $exe)) { throw "the archive has no $name\tvpipe.exe" }

        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        $dest = Join-Path $dir 'tvpipe.exe'
        if (Test-Path $dest) {
            # A running tvpipe.exe cannot be overwritten, but it can be renamed.
            $old = "$dest.old"
            Remove-Item -Force $old -ErrorAction SilentlyContinue
            Move-Item -Force $dest $old
        }
        Copy-Item $exe $dest
        Remove-Item -Force "$dest.old" -ErrorAction SilentlyContinue
        Write-Host "Installed $dest"
        try { & $dest --version } catch { }
        Write-Host ''

        # Add the directory to the user PATH (registry, keeping %VARS%
        # unexpanded) and to this session's PATH.
        $key = Get-Item 'HKCU:\Environment'
        $userPath = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
        $entries = @($userPath -split ';' | Where-Object { $_ })
        $known = $entries | ForEach-Object { [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') }
        if ($known -notcontains $dir.TrimEnd('\')) {
            $newPath = (@($entries) + $dir) -join ';'
            $kind = 'ExpandString'
            if ($key.GetValueNames() -contains 'Path' -and $key.GetValueKind('Path') -eq 'String') { $kind = 'String' }
            Set-ItemProperty -Path 'HKCU:\Environment' -Name Path -Value $newPath -Type $kind
            # Setting any user variable broadcasts WM_SETTINGCHANGE, so new
            # terminals and Explorer see the new PATH.
            [Environment]::SetEnvironmentVariable('TVPIPE_INSTALL_REFRESH', '1', 'User')
            [Environment]::SetEnvironmentVariable('TVPIPE_INSTALL_REFRESH', $null, 'User')
            Write-Host "Added $dir to your user PATH (new terminals pick it up)."
        }
        if ((($env:Path -split ';') | ForEach-Object { $_.TrimEnd('\') }) -notcontains $dir.TrimEnd('\')) {
            $env:Path = "$env:Path;$dir"
        }

        Write-Host ''
        Write-Host 'Next steps:'
        Write-Host '  Find AirPlay and Cast receivers:  tvpipe discover'
        Write-Host '  Check picture and latency:        tvpipe test-pattern --to "Living Room"'
        Write-Host ''
        Write-Host 'Windows Firewall may ask to allow tvpipe the first time (AirPlay); allow it on private networks.'
        if (Get-Command ffmpeg -ErrorAction SilentlyContinue) {
            Write-Host "ffmpeg found: $((Get-Command ffmpeg).Source)"
        } else {
            Write-Host 'Optional: install ffmpeg 6 or newer to mirror files, URLs and capture devices that'
            Write-Host 'tvpipe cannot send as they are (e.g. winget install ffmpeg).'
        }
        Write-Host 'Documentation: https://tvpipe.party/'
    } finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
}

# TVPIPE_INSTALL_SOURCED=1: only define the functions (scripts/test-installers.sh).
if ($env:TVPIPE_INSTALL_SOURCED -ne '1') {
    try {
        Install-Tvpipe
    } catch {
        Write-Host "tvpipe install: $($_.Exception.Message)" -ForegroundColor Red
    }
}
