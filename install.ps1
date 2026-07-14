# Meta CLI Installation Script for Windows
# Downloads and installs meta from GitHub releases
#
# Usage: irm https://raw.githubusercontent.com/gitkb/meta/main/install.ps1 | iex

$ErrorActionPreference = "Stop"

$Repo = "gitkb/meta"
$InstallDir = if ($env:INSTALL_DIR) { $env:INSTALL_DIR } else { "$env:USERPROFILE\.meta\bin" }
$Version = if ($env:VERSION) { $env:VERSION } else { "latest" }

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

function Get-LatestVersion {
    try {
        $response = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
        return $response.tag_name -replace '^v', ''
    } catch {
        Write-Err "Failed to fetch latest version: $_"
    }
}

function Install-Meta {
    $arch = Get-Architecture
    $platform = "windows-$arch"

    if ($Version -eq "latest") {
        Write-Info "Fetching latest version..."
        $Version = Get-LatestVersion
        if (-not $Version) {
            Write-Err "Could not determine latest version"
        }
    }

    Write-Info "Installing meta v$Version for $platform..."

    $downloadUrl = "https://github.com/$Repo/releases/download/v$Version/meta-$platform.zip"
    $tempDir = New-Item -ItemType Directory -Path "$env:TEMP\meta-install-$(Get-Random)" -Force
    $zipPath = "$tempDir\meta.zip"

    Write-Info "Downloading from $downloadUrl..."
    try {
        Invoke-WebRequest -Uri $downloadUrl -OutFile $zipPath -UseBasicParsing
    } catch {
        Write-Err "Failed to download meta: $_"
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
