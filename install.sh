#!/bin/bash
set -e

# Meta CLI Installation Script
# Downloads and installs meta from GitHub releases

REPO="gitkb/meta"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${VERSION:-latest}"

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
    echo -e "${RED}[ERROR]${NC} $1"
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

# Get latest version from GitHub
get_latest_version() {
    curl -sL "https://api.github.com/repos/${REPO}/releases/latest" | \
        grep '"tag_name":' | \
        sed -E 's/.*"([^"]+)".*/\1/' | \
        sed 's/^v//'
}

# Download and install
install_meta() {
    local platform=$(detect_platform)
    local version="$VERSION"

    if [ "$version" = "latest" ]; then
        info "Fetching latest version..."
        version=$(get_latest_version)
        if [ -z "$version" ]; then
            error "Could not determine latest version"
        fi
    fi

    info "Installing meta v${version} for ${platform}..."

    local download_url="https://github.com/${REPO}/releases/download/v${version}/meta-${platform}.tar.gz"
    local tmp_dir=$(mktemp -d)

    info "Downloading from ${download_url}..."
    if ! curl -sL "$download_url" -o "${tmp_dir}/meta.tar.gz"; then
        error "Failed to download meta"
    fi

    info "Extracting..."
    tar -xzf "${tmp_dir}/meta.tar.gz" -C "$tmp_dir"

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

    # Cleanup
    rm -rf "$tmp_dir"

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

    install_meta
}

main "$@"
