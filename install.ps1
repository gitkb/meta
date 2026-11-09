# Meta CLI Installation Script for Windows
# Downloads and installs meta from GitHub releases, which lag main. The
# recommended install builds main instead (in Git Bash or WSL):
#   git clone https://github.com/gitkb/meta && make -C meta install
#
# Usage: irm https://raw.githubusercontent.com/gitkb/meta/main/install.ps1 | iex

$ErrorActionPreference = "Stop"

$InstallDir = if ($env:INSTALL_DIR) { $env:INSTALL_DIR } else { "$env:USERPROFILE\.meta\bin" }
$Version = if ($env:VERSION) { $env:VERSION } else { "latest" }
# Base URL of the release downloads; point it at a mirror of the GitHub releases.
$DownloadBase = if ($env:META_DOWNLOAD_URL) { $env:META_DOWNLOAD_URL } else { "https://github.com/gitkb/meta/releases" }

function Write-Info($msg) {
    Write-Host "[INFO] $msg" -ForegroundColor Green
}

function Write-Warn($msg) {
    Write-Host "[WARN] $msg" -ForegroundColor Yellow
}

function Write-Err($msg) {
    Write-Host "[ERROR] $msg" -ForegroundColor Red
    exit 1
}

function Get-Architecture {
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    switch ($arch) {
        "X64" { return "x64" }
        "Arm64" { return "arm64" }
        default { Write-Err "Unsupported architecture: $arch" }
    }
}

# Download a URL to a file; on failure, name the URL and the reason
function Get-Download($url, $outFile) {
    try {
        Invoke-WebRequest -Uri $url -OutFile $outFile -UseBasicParsing
    } catch {
        $err = $_
        $status = [int]$err.Exception.Response.StatusCode
        $reason = switch ($status) {
            0 { "could not connect: $($err.Exception.Message)" }
            404 { "HTTP 404: no such release asset; check VERSION and that your platform has a release" }
            { $_ -in 403, 429 } { "HTTP ${status}: rate limited or forbidden; wait and retry" }
            { $_ -ge 500 } { "HTTP ${status}: server error; retry later" }
            default { "HTTP $status" }
        }
        Write-Err "Download failed ($reason): $url"
    }
}

function Install-Meta {
    $arch = Get-Architecture
    $platform = "windows-$arch"
    $asset = "meta-$platform.zip"

    if ($Version -eq "latest") {
        Write-Info "Installing the latest meta release for $platform..."
        $baseUrl = "$DownloadBase/latest/download"
    } else {
        Write-Info "Installing meta v$Version for $platform..."
        $baseUrl = "$DownloadBase/download/v$Version"
    }

    $tempDir = New-Item -ItemType Directory -Path "$env:TEMP\meta-install-$(Get-Random)" -Force
    $zipPath = Join-Path $tempDir $asset
    $checksumPath = "$zipPath.sha256"

    Write-Info "Downloading from $baseUrl/$asset..."
    Get-Download "$baseUrl/$asset" $zipPath
    Get-Download "$baseUrl/$asset.sha256" $checksumPath

    # The hash is the file's first 64 hex digits (older releases published certutil
    # output: a header line, the hash, a footer line)
    Write-Info "Verifying checksum..."
    $expected = [regex]::Match("$(Get-Content -Raw $checksumPath)", '\b[0-9a-fA-F]{64}\b').Value
    if (-not $expected) {
        Write-Err "No SHA-256 hash in $baseUrl/$asset.sha256; nothing was installed"
    }
    $actual = (Get-FileHash -Algorithm SHA256 -Path $zipPath).Hash
    if ($actual -ne $expected) {
        Write-Err "Checksum mismatch for $asset; nothing was installed`n  Expected: $expected`n  Actual:   $actual"
    }

    # Each entry must be a program, once: a regular .exe file at the top
    # level. The entries are checked before anything is extracted; anything
    # else and nothing is installed.
    try {
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $zip = [System.IO.Compression.ZipFile]::OpenRead($zipPath)
        # name, and Unix file type when the zip records one (0 when it doesn't)
        $entries = @($zip.Entries | ForEach-Object {
            [pscustomobject]@{ Name = $_.FullName; Type = ($_.ExternalAttributes -shr 16) -band 0xF000 }
        })
        $zip.Dispose()
    } catch {
        Write-Err "Failed to read archive: $_"
    }
    $seen = @{}
    foreach ($entry in $entries) {
        $name = $entry.Name
        if ($name -match '[/\\]' -or $name -notlike "*.exe") {
            Write-Err "$asset holds $name, not a program at its top level; nothing was installed"
        }
        if ($entry.Type -ne 0 -and $entry.Type -ne 0x8000) {
            Write-Err "$asset holds $name, which is not a regular file; nothing was installed"
        }
        if ($seen.ContainsKey($name)) {
            Write-Err "$asset holds $name twice; nothing was installed"
        }
        $seen[$name] = $true
    }
    if (-not $seen.ContainsKey("meta.exe")) {
        Write-Err "$asset holds no meta.exe; nothing was installed"
    }

    Write-Info "Extracting..."
    $programs = Join-Path $tempDir "programs"
    try {
        Expand-Archive -Path $zipPath -DestinationPath $programs -Force
    } catch {
        Write-Err "Failed to extract archive: $_"
    }

    # Create install directory if needed
    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }

    # Install the programs the release holds
    Write-Info "Installing to $InstallDir..."
    foreach ($binary in Get-ChildItem -Path $programs -File) {
        Copy-Item $binary.FullName -Destination $InstallDir -Force
        Write-Info "Installed $($binary.Name)"
    }

    # Cleanup
    Remove-Item -Path $tempDir -Recurse -Force

    # Check if InstallDir is in PATH
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath -notlike "*$InstallDir*") {
        Write-Warn "$InstallDir is not in your PATH"
        Write-Host ""
        Write-Host "To add it permanently, run:" -ForegroundColor Cyan
        Write-Host ""
        Write-Host "  [Environment]::SetEnvironmentVariable('Path', `$env:Path + ';$InstallDir', 'User')"
        Write-Host ""
        Write-Host "Or add it to the current session:"
        Write-Host ""
        Write-Host "  `$env:Path += ';$InstallDir'"
        Write-Host ""

        # Offer to add to PATH automatically
        $addToPath = Read-Host "Add to PATH now? (Y/n)"
        if ($addToPath -ne "n" -and $addToPath -ne "N") {
            [Environment]::SetEnvironmentVariable("Path", "$userPath;$InstallDir", "User")
            $env:Path += ";$InstallDir"
            Write-Info "Added $InstallDir to PATH"
        }
    }

    Write-Info "Installation complete!"
    Write-Host ""
    Write-Host "Run 'meta --help' to get started."
}

# Main
Write-Host "Meta CLI Installer for Windows" -ForegroundColor Cyan
Write-Host "===============================" -ForegroundColor Cyan
Write-Host ""
Write-Warn "This installs a release build, which can lag main by many fixes and commands."
Write-Warn "To build main instead (Git Bash or WSL): git clone https://github.com/gitkb/meta && make -C meta install"
Write-Host ""

Install-Meta
