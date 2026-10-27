#!/usr/bin/env bats

# install.sh and install.ps1 against a fake release served from a local HTTP server.

sha256() {
    if command -v sha256sum > /dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi
}

setup() {
    TEST_DIR="$(cd "$(mktemp -d)" && pwd -P)"
    RELEASES="$TEST_DIR/releases"
    INSTALL_DIR="$TEST_DIR/bin"
    mkdir -p "$RELEASES/latest/download" "$TEST_DIR/archive"

    local os arch
    os=$(uname -s | tr '[:upper:]' '[:lower:]')
    case "$(uname -m)" in
        x86_64|amd64) arch="x64" ;;
        *) arch="arm64" ;;
    esac
    ASSET="meta-${os}-${arch}.tar.gz"

    for binary in meta meta-git meta-project meta-rust meta-mcp loop; do
        printf '#!/bin/sh\necho %s 1.0.0\n' "$binary" > "$TEST_DIR/archive/$binary"
        printf 'fake %s\n' "$binary" > "$TEST_DIR/archive/$binary.exe"
    done
    tar -czf "$RELEASES/latest/download/$ASSET" -C "$TEST_DIR/archive" \
        meta meta-git meta-project meta-rust meta-mcp loop
    (cd "$RELEASES/latest/download" && sha256 "$ASSET" > "$ASSET.sha256")

    # Windows checksums are certutil output, CRLF line endings included
    for zip in meta-windows-x64.zip meta-windows-arm64.zip; do
        python3 -c 'import sys, zipfile
with zipfile.ZipFile(sys.argv[1], "w") as z:
    for f in sys.argv[2:]:
        z.write(f, f.rsplit("/", 1)[-1])' "$RELEASES/latest/download/$zip" "$TEST_DIR"/archive/*.exe
        local hash
        hash=$(sha256 "$RELEASES/latest/download/$zip" | awk '{print $1}')
        printf 'SHA256 hash of %s:\r\n%s\r\nCertUtil: -hashfile command completed successfully.\r\n' \
            "$zip" "$hash" > "$RELEASES/latest/download/$zip.sha256"
    done

    python3 "$BATS_TEST_DIRNAME/helpers/release_server.py" "$RELEASES" "$TEST_DIR/port" 3>&- &
    SERVER_PID=$!
    for _ in $(seq 100); do
        [ -s "$TEST_DIR/port" ] && break
        sleep 0.1
    done
    [ -s "$TEST_DIR/port" ] || { echo "release server did not start" >&2; return 1; }
    export META_DOWNLOAD_URL="http://127.0.0.1:$(cat "$TEST_DIR/port")"
    export INSTALL_DIR
}

teardown() {
    kill "$SERVER_PID" 2>/dev/null || true
    rm -rf "$TEST_DIR"
}

tamper() {
    printf 'x' >> "$RELEASES/latest/download/$1"
}

run_ps1() {
    command -v pwsh > /dev/null || skip "pwsh is not installed"
    TEMP="$TEST_DIR" run pwsh -NoProfile -File "$BATS_TEST_DIRNAME/../install.ps1" <<< "n"
}

@test "install.sh installs a release whose checksum matches" {
    run bash "$BATS_TEST_DIRNAME/../install.sh"
    [ "$status" -eq 0 ]
    [[ "$output" == *"Installation complete"* ]]
    run "$INSTALL_DIR/meta"
    [ "$output" = "meta 1.0.0" ]
}

@test "install.sh refuses a tampered archive and leaves the installed meta alone" {
    mkdir -p "$INSTALL_DIR"
    echo old > "$INSTALL_DIR/meta"
    tamper "$ASSET"
    run bash "$BATS_TEST_DIRNAME/../install.sh"
    [ "$status" -eq 1 ]
    [[ "$output" == *"Checksum mismatch for $ASSET; nothing was installed"* ]]
    [ "$(cat "$INSTALL_DIR/meta")" = "old" ]
    [ ! -e "$INSTALL_DIR/meta-git" ]
}

@test "install.sh refuses a release without a .sha256" {
    rm "$RELEASES/latest/download/$ASSET.sha256"
    run bash "$BATS_TEST_DIRNAME/../install.sh"
    [ "$status" -eq 1 ]
    [[ "$output" == *"Download failed (HTTP 404: no such release asset"*"): $META_DOWNLOAD_URL/latest/download/$ASSET.sha256"* ]]
    [ ! -e "$INSTALL_DIR" ]
}

@test "install.sh refuses an empty checksum even when the hash tool fails" {
    : > "$RELEASES/latest/download/$ASSET.sha256"
    mkdir "$TEST_DIR/path"
    printf '#!/bin/sh\nexit 1\n' > "$TEST_DIR/path/sha256sum"
    chmod +x "$TEST_DIR/path/sha256sum"
    PATH="$TEST_DIR/path:$PATH" run bash "$BATS_TEST_DIRNAME/../install.sh"
    [ "$status" -eq 1 ]
    [[ "$output" == *"No SHA-256 hash in $META_DOWNLOAD_URL/latest/download/$ASSET.sha256; nothing was installed"* ]]
    [ ! -e "$INSTALL_DIR" ]
}

@test "install.sh names the URL and the reason when a download fails" {
    VERSION=9.9.9 run bash "$BATS_TEST_DIRNAME/../install.sh"
    [ "$status" -eq 1 ]
    [[ "$output" == *"Download failed (HTTP 404: no such release asset; check VERSION"*"): $META_DOWNLOAD_URL/download/v9.9.9/$ASSET"* ]]
    [[ "$output" != *"gzip"* ]]

    echo 403 > "$RELEASES/latest/download/$ASSET.status"
    run bash "$BATS_TEST_DIRNAME/../install.sh"
    [ "$status" -eq 1 ]
    [[ "$output" == *"Download failed (HTTP 403: rate limited or forbidden; wait and retry): $META_DOWNLOAD_URL/latest/download/$ASSET"* ]]

    echo 502 > "$RELEASES/latest/download/$ASSET.status"
    run bash "$BATS_TEST_DIRNAME/../install.sh"
    [ "$status" -eq 1 ]
    [[ "$output" == *"Download failed (HTTP 502: server error; retry later)"* ]]
}

@test "install.sh needs sha256sum or shasum" {
    mkdir "$TEST_DIR/path"
    ln -s "$(command -v curl)" "$TEST_DIR/path/curl"
    ln -s "$(command -v tar)" "$TEST_DIR/path/tar"
    PATH="$TEST_DIR/path" run "$BASH" "$BATS_TEST_DIRNAME/../install.sh"
    [ "$status" -eq 1 ]
    [[ "$output" == *"sha256sum or shasum is required"* ]]
}

@test "install.ps1 installs a release whose certutil checksum matches" {
    run_ps1
    [ "$status" -eq 0 ]
    [[ "$output" == *"Installation complete"* ]]
    [ "$(cat "$INSTALL_DIR/meta.exe")" = "fake meta" ]
}

@test "install.ps1 refuses a tampered archive" {
    tamper meta-windows-x64.zip
    tamper meta-windows-arm64.zip
    run_ps1
    [ "$status" -eq 1 ]
    [[ "$output" == *"Checksum mismatch for meta-windows-"*".zip; nothing was installed"* ]]
    [ ! -e "$INSTALL_DIR" ]
}

@test "install.ps1 refuses a release without a .sha256" {
    rm "$RELEASES"/latest/download/meta-windows-*.zip.sha256
    run_ps1
    [ "$status" -eq 1 ]
    [[ "$output" == *"Download failed (HTTP 404: no such release asset"*".zip.sha256"* ]]
    [ ! -e "$INSTALL_DIR" ]
}

@test "install.ps1 refuses a checksum file without a hash" {
    for zip in meta-windows-x64.zip meta-windows-arm64.zip; do
        printf 'CertUtil: -hashfile command failed.\r\n' > "$RELEASES/latest/download/$zip.sha256"
    done
    run_ps1
    [ "$status" -eq 1 ]
    [[ "$output" == *"No SHA-256 hash in $META_DOWNLOAD_URL/latest/download/meta-windows-"*".zip.sha256; nothing was installed"* ]]
    [ ! -e "$INSTALL_DIR" ]
}

@test "install.ps1 names the URL and the reason when a download fails" {
    echo 403 > "$RELEASES/latest/download/meta-windows-x64.zip.status"
    echo 403 > "$RELEASES/latest/download/meta-windows-arm64.zip.status"
    run_ps1
    [ "$status" -eq 1 ]
    [[ "$output" == *"Download failed (HTTP 403: rate limited or forbidden; wait and retry): $META_DOWNLOAD_URL/latest/download/meta-windows-"* ]]
}
