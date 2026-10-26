# Meta CLI Installation Script for Windows
# Downloads and installs meta from GitHub releases
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

    # The release publishes certutil output: a header line, the hash, a footer line
    Write-Info "Verifying checksum..."
    $expected = [regex]::Match("$(Get-Content -Raw $checksumPath)", '\b[0-9a-fA-F]{64}\b').Value
    if (-not $expected) {
        Write-Err "No SHA-256 hash in $baseUrl/$asset.sha256; nothing was installed"
    }
    $actual = (Get-FileHash -Algorithm SHA256 -Path $zipPath).Hash
    if ($actual -ne $expected) {
        Write-Err "Checksum mismatch for $asset; nothing was installed`n  Expected: $expected`n  Actual:   $actual"
    }

    Write-Info "Extracting..."
    try {
        Expand-Archive -Path $zipPath -DestinationPath $tempDir -Force
    } catch {
        Write-Err "Failed to extract archive: $_"
    }

    # Create install directory if needed
    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }

    # Install binaries
    Write-Info "Installing to $InstallDir..."
    $expectedBinaries = @("meta.exe", "meta-git.exe", "meta-project.exe", "meta-rust.exe", "meta-mcp.exe", "loop.exe")
    foreach ($binary in $expectedBinaries) {
        $binaryPath = Join-Path $tempDir $binary
        if (Test-Path $binaryPath) {
            Copy-Item $binaryPath -Destination $InstallDir -Force
            Write-Info "Installed $binary"
        } else {
            Write-Warn "Binary $binary not found in archive"
        }
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

Install-Meta
