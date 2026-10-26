#!/bin/bash
set -e

# Meta CLI Installation Script
# Downloads and installs meta from GitHub releases

INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${VERSION:-latest}"
# Base URL of the release downloads; point it at a mirror of the GitHub releases.
META_DOWNLOAD_URL="${META_DOWNLOAD_URL:-https://github.com/gitkb/meta/releases}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

error() {
    echo -e "${RED}[ERROR]${NC} $1" >&2
    exit 1
}

# Detect platform
detect_platform() {
    local os=$(uname -s | tr '[:upper:]' '[:lower:]')
    local arch=$(uname -m)

    case "$os" in
        darwin) os="darwin" ;;
        linux) os="linux" ;;
        *) error "Unsupported operating system: $os" ;;
    esac

    case "$arch" in
        x86_64|amd64) arch="x64" ;;
        arm64|aarch64) arch="arm64" ;;
        *) error "Unsupported architecture: $arch" ;;
    esac

    echo "${os}-${arch}"
}

# Download a URL to a file; on failure, name the URL and the reason
download() {
    local url="$1" out="$2" status reason
    status=$(curl -fsSL -o "$out" -w '%{http_code}' "$url") && return 0
    local rc=$?
    case "$status" in
        2??) reason="transfer failed, curl exit ${rc}" ;;
        404) reason="HTTP 404: no such release asset; check VERSION and that your platform has a release" ;;
        403|429) reason="HTTP ${status}: rate limited or forbidden; wait and retry" ;;
        5??) reason="HTTP ${status}: server error; retry later" ;;
        000) reason="could not connect" ;;
        *) reason="HTTP ${status}" ;;
    esac
    error "Download failed (${reason}): ${url}"
}

sha256() {
    if command -v sha256sum &> /dev/null; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

# Download, verify and install
install_meta() {
    local platform
    platform=$(detect_platform)
    local asset="meta-${platform}.tar.gz"
    local base_url

    if [ "$VERSION" = "latest" ]; then
        info "Installing the latest meta release for ${platform}..."
        base_url="${META_DOWNLOAD_URL}/latest/download"
    else
        info "Installing meta v${VERSION} for ${platform}..."
        base_url="${META_DOWNLOAD_URL}/download/v${VERSION}"
    fi

    tmp_dir=$(mktemp -d)
    trap 'rm -rf "$tmp_dir"' EXIT

    info "Downloading from ${base_url}/${asset}..."
    download "${base_url}/${asset}" "${tmp_dir}/${asset}"
    download "${base_url}/${asset}.sha256" "${tmp_dir}/${asset}.sha256"

    info "Verifying checksum..."
    local expected actual
    expected=$(awk '{print $1; exit}' "${tmp_dir}/${asset}.sha256")
    if [[ ! "$expected" =~ ^[0-9a-f]{64}$ ]]; then
        error "No SHA-256 hash in ${base_url}/${asset}.sha256; nothing was installed"
    fi
    actual=$(sha256 "${tmp_dir}/${asset}")
    if [ "$expected" != "$actual" ]; then
        error "Checksum mismatch for ${asset}; nothing was installed\n  Expected: ${expected}\n  Actual:   ${actual}"
    fi

    info "Extracting..."
    tar -xzf "${tmp_dir}/${asset}" -C "$tmp_dir"

    # Create install directory if needed
    mkdir -p "$INSTALL_DIR"

    # Install binaries
    info "Installing to ${INSTALL_DIR}..."
    local expected_binaries=("meta" "meta-git" "meta-project" "meta-rust" "meta-mcp" "loop")
    for binary_name in "${expected_binaries[@]}"; do
        local binary="$tmp_dir/$binary_name"
        if [ -f "$binary" ]; then
            cp "$binary" "$INSTALL_DIR/"
            chmod +x "$INSTALL_DIR/$binary_name"
            info "Installed $binary_name"
        else
            warn "Binary $binary_name not found in archive"
        fi
    done

    # Check if INSTALL_DIR is in PATH
    if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
        warn "$INSTALL_DIR is not in your PATH"
        echo ""
        echo "Add it to your shell configuration:"
        echo ""
        echo "  # For bash (~/.bashrc or ~/.bash_profile):"
        echo "  export PATH=\"\$PATH:$INSTALL_DIR\""
        echo ""
        echo "  # For zsh (~/.zshrc):"
        echo "  export PATH=\"\$PATH:$INSTALL_DIR\""
        echo ""
        echo "  # For fish (~/.config/fish/config.fish):"
        echo "  set -gx PATH \$PATH $INSTALL_DIR"
        echo ""
    fi

    info "Installation complete!"
    echo ""
    echo "Run 'meta --help' to get started."
}

# Main
main() {
    echo "Meta CLI Installer"
    echo "=================="
    echo ""

    # Check for curl
    if ! command -v curl &> /dev/null; then
        error "curl is required but not installed"
    fi

    # Check for tar
    if ! command -v tar &> /dev/null; then
        error "tar is required but not installed"
    fi

    if ! command -v sha256sum &> /dev/null && ! command -v shasum &> /dev/null; then
        error "sha256sum or shasum is required to verify the download but neither is installed"
    fi

    install_meta
}

main "$@"
