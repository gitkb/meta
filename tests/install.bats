#!/usr/bin/env bats

# install.sh and install.ps1 against a fake release served from a local HTTP server.

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

sha256() {
    if command -v sha256sum > /dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi
}

setup() {
    # its own HOME too: curl reads ~/.curlrc
    sandbox
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

    # the release's archives, as release.yml packages them: every program the
    # workspace declares, built here as a script that names itself
    PROGRAMS="$BATS_TEST_DIRNAME/../scripts/programs.py"
    BINS="$(python3 "$PROGRAMS" bins)"
    local binary zip
    for binary in $BINS; do
        printf '#!/bin/sh\necho %s 1.0.0\n' "$binary" > "$TEST_DIR/archive/$binary"
        chmod +x "$TEST_DIR/archive/$binary"
        printf 'fake %s\n' "$binary" > "$TEST_DIR/archive/$binary.exe"
    done
    python3 "$PROGRAMS" package "$TEST_DIR/archive" "$TEST_DIR/dist" "$RELEASES/latest/download/$ASSET"
    for zip in meta-windows-x64.zip meta-windows-arm64.zip; do
        python3 "$PROGRAMS" package "$TEST_DIR/archive" "$TEST_DIR/dist-$zip" "$RELEASES/latest/download/$zip"
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
}

tamper() {
    printf 'x' >> "$RELEASES/latest/download/$1"
}

run_ps1() {
    command -v pwsh > /dev/null || skip "pwsh is not installed"
    TEMP="$TEST_DIR" run pwsh -NoProfile -File "$BATS_TEST_DIRNAME/../install.ps1" <<< "n"
}

@test "install.sh installs every program of a release whose checksum matches" {
    run bash "$BATS_TEST_DIRNAME/../install.sh"
    [ "$status" -eq 0 ]
    [[ "$output" == *"Installation complete"* ]]
    [ "$(ls "$INSTALL_DIR" | sort)" = "$(sort <<< "$BINS")" ]
    for binary in $BINS; do
        run "$INSTALL_DIR/$binary"
        [ "$output" = "$binary 1.0.0" ]
    done
}

@test "install.sh installs what the archive holds and needs only meta" {
    mkdir "$TEST_DIR/odd"
    cp "$TEST_DIR/archive/meta" "$TEST_DIR/odd/meta"
    printf '#!/bin/sh\necho new 1.0.0\n' > "$TEST_DIR/odd/meta-new"
    tar -czf "$RELEASES/latest/download/$ASSET" -C "$TEST_DIR/odd" meta meta-new
    (cd "$RELEASES/latest/download" && sha256 "$ASSET" > "$ASSET.sha256")
    run bash "$BATS_TEST_DIRNAME/../install.sh"
    [ "$status" -eq 0 ]
    [ "$(ls "$INSTALL_DIR" | tr '\n' ' ')" = "meta meta-new " ]
    run "$INSTALL_DIR/meta-new"
    [ "$output" = "new 1.0.0" ]

    rm -r "$INSTALL_DIR"
    tar -czf "$RELEASES/latest/download/$ASSET" -C "$TEST_DIR/odd" meta-new
    (cd "$RELEASES/latest/download" && sha256 "$ASSET" > "$ASSET.sha256")
    run bash "$BATS_TEST_DIRNAME/../install.sh"
    [ "$status" -eq 1 ]
    [[ "$output" == *"$ASSET holds no meta; nothing was installed"* ]]
    [ ! -e "$INSTALL_DIR" ]
}

@test "install.sh refuses a tampered archive and leaves the installed meta alone" {
    mkdir -p "$INSTALL_DIR"
    echo old > "$INSTALL_DIR/meta"
    tamper "$ASSET"
    run bash "$BATS_TEST_DIRNAME/../install.sh"
    [ "$status" -eq 1 ]
    [[ "$output" == *"Checksum mismatch for $ASSET; nothing was installed"* ]]
    [ "$(cat "$INSTALL_DIR/meta")" = "old" ]
    [ "$(ls "$INSTALL_DIR")" = "meta" ]
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

@test "install.ps1 installs every program of a release whose checksum matches" {
    run_ps1
    [ "$status" -eq 0 ]
    [[ "$output" == *"Installation complete"* ]]
    for binary in $BINS; do
        [ "$(cat "$INSTALL_DIR/$binary.exe")" = "fake $binary" ]
    done
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

# A workspace of its own, so the programs below are the ones it declares and
# not meta's of today: `meta` and `alpha` in one package, `b` (no [[bin]]
# section, so cargo's default for src/main.rs) in another, and a library.
scratch_workspace() {
    SCRATCH="$TEST_DIR/scratch"
    mkdir -p "$SCRATCH/a/src" "$SCRATCH/b/src" "$SCRATCH/lib/src" "$SCRATCH/built"
    printf '[workspace]\nmembers = ["a", "b", "lib"]\nresolver = "2"\n' > "$SCRATCH/Cargo.toml"
    printf '[package]\nname = "pkg_a"\nversion = "0.1.0"\nedition = "2021"\n\n[[bin]]\nname = "meta"\npath = "src/meta.rs"\n\n[[bin]]\nname = "alpha"\npath = "src/alpha.rs"\n' > "$SCRATCH/a/Cargo.toml"
    printf '[package]\nname = "b"\nversion = "0.1.0"\nedition = "2021"\n' > "$SCRATCH/b/Cargo.toml"
    printf '[package]\nname = "lib"\nversion = "0.1.0"\nedition = "2021"\n' > "$SCRATCH/lib/Cargo.toml"
    touch "$SCRATCH/a/src/meta.rs" "$SCRATCH/a/src/alpha.rs" "$SCRATCH/b/src/main.rs" "$SCRATCH/lib/src/lib.rs"
}

# programs.py on the scratch workspace
programs() {
    python3 "$PROGRAMS" --manifest-path "$SCRATCH/Cargo.toml" "$@"
}

# a built program for each name
built() {
    local name
    for name in "$@"; do
        echo "$name" > "$SCRATCH/built/$name"
    done
}

# an archive's files, sorted, on one line
listing() {
    python3 -c 'import sys, tarfile, zipfile
p = sys.argv[1]
names = zipfile.ZipFile(p).namelist() if p.endswith(".zip") else tarfile.open(p).getnames()
print(" ".join(sorted(names)))' "$1"
}

@test "the programs are the workspace's bin targets, and their packages own them" {
    scratch_workspace
    run programs bins
    [ "$status" -eq 0 ]
    [ "$(sort <<< "$output" | tr '\n' ' ')" = "alpha b meta " ]
    run programs packages
    [ "$(sort <<< "$output" | tr '\n' ' ')" = "b pkg_a " ]
    run programs packages --dirs
    [ "$(sort <<< "$output" | tr '\n' ' ')" = "a b " ]
}

@test "release packaging holds exactly the declared programs, Unix and Windows names" {
    scratch_workspace
    built meta alpha b meta.exe alpha.exe b.exe stray
    run programs package "$SCRATCH/built" "$SCRATCH/dist" "$SCRATCH/meta-linux-x64.tar.gz"
    [ "$status" -eq 0 ]
    [ "$(listing "$SCRATCH/meta-linux-x64.tar.gz")" = "alpha b meta" ]
    [ "$(ls "$SCRATCH/dist" | sort | tr '\n' ' ')" = "alpha b meta " ]
    [ "$(awk '{print $1}' "$SCRATCH/meta-linux-x64.tar.gz.sha256")" = "$(sha256 "$SCRATCH/meta-linux-x64.tar.gz" | awk '{print $1}')" ]
    run programs package "$SCRATCH/built" "$SCRATCH/dist-win" "$SCRATCH/meta-windows-x64.zip"
    [ "$status" -eq 0 ]
    [ "$(listing "$SCRATCH/meta-windows-x64.zip")" = "alpha.exe b.exe meta.exe" ]
    run programs check "$SCRATCH/meta-windows-x64.zip"
    [ "$status" -eq 0 ]
}

@test "release packaging refuses a declared program that wasn't built, and names it" {
    scratch_workspace
    built meta b
    run programs package "$SCRATCH/built" "$SCRATCH/dist" "$SCRATCH/meta-linux-x64.tar.gz"
    [ "$status" -ne 0 ]
    [[ "$output" == *"alpha is not in $SCRATCH/built"* ]]
    [ ! -e "$SCRATCH/meta-linux-x64.tar.gz" ]
}

@test "the archive check names a missing program and an extra one" {
    scratch_workspace
    built meta alpha b stray meta.exe b.exe
    tar -czf "$SCRATCH/missing.tar.gz" -C "$SCRATCH/built" meta b
    run programs check "$SCRATCH/missing.tar.gz"
    [ "$status" -ne 0 ]
    [[ "$output" == *"missing alpha"* ]]
    tar -czf "$SCRATCH/extra.tar.gz" -C "$SCRATCH/built" meta alpha b stray
    run programs check "$SCRATCH/extra.tar.gz"
    [ "$status" -ne 0 ]
    [[ "$output" == *"not a declared program: stray"* ]]
    (cd "$SCRATCH/built" && python3 -c 'import sys, zipfile
with zipfile.ZipFile(sys.argv[1], "w") as z:
    for f in sys.argv[2:]: z.write(f)' "$SCRATCH/missing.zip" meta.exe b.exe)
    run programs check "$SCRATCH/missing.zip"
    [ "$status" -ne 0 ]
    [[ "$output" == *"missing alpha.exe"* ]]
}

@test "a new or renamed bin target reaches the archive and the formula with no other edit" {
    scratch_workspace
    printf '\n[[bin]]\nname = "gamma"\npath = "src/lib.rs"\n' >> "$SCRATCH/lib/Cargo.toml"
    sed -i.bak 's/name = "alpha"/name = "alpha2"/' "$SCRATCH/a/Cargo.toml"
    built meta alpha2 b gamma
    local platform
    for platform in darwin-arm64 darwin-x64 linux-arm64 linux-x64; do
        rm -rf "$SCRATCH/dist"
        programs package "$SCRATCH/built" "$SCRATCH/dist" "$SCRATCH/meta-$platform.tar.gz"
    done
    [ "$(listing "$SCRATCH/meta-linux-x64.tar.gz")" = "alpha2 b gamma meta" ]
    run programs formula 1.2.3 "$SCRATCH"/meta-*.tar.gz
    [ "$status" -eq 0 ]
    [ "$(grep -o 'bin.install "[^"]*"' <<< "$output" | sort | tr '\n' ' ')" = 'bin.install "alpha2" bin.install "b" bin.install "gamma" bin.install "meta" ' ]
    [[ "$output" == *'version "1.2.3"'* ]]
    [[ "$output" == *"sha256 \"$(sha256 "$SCRATCH/meta-darwin-arm64.tar.gz" | awk '{print $1}')\""* ]]
    # the formula needs every platform's archive
    run programs formula 1.2.3 "$SCRATCH/meta-linux-x64.tar.gz"
    [ "$status" -ne 0 ]
}
