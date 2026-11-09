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
    # a copy an old `make install-plugins` left, which meta would run first,
    # and the user's own plugins, which stay
    mkdir -p "$HOME/.meta/plugins"
    touch "$HOME/.meta/plugins/meta-cargo" "$HOME/.meta/plugins/meta-mcp" "$HOME/.meta/plugins/meta-other"
    run bash "$BATS_TEST_DIRNAME/../install.sh"
    [ "$status" -eq 0 ]
    [[ "$output" == *"Installation complete"* ]]
    [[ "$output" == *"Removed the old copy $HOME/.meta/plugins/meta-cargo"* ]]
    [ "$(ls "$INSTALL_DIR" | sort)" = "$(sort <<< "$BINS")" ]
    [ "$(ls "$HOME/.meta/plugins" | tr '\n' ' ')" = "meta-mcp meta-other " ]
    for binary in $BINS; do
        run "$INSTALL_DIR/$binary"
        [ "$output" = "$binary 1.0.0" ]
    done
}

@test "install.sh installs what the archive holds and needs only meta" {
    mkdir "$TEST_DIR/odd"
    cp "$TEST_DIR/archive/meta" "$TEST_DIR/odd/meta"
    printf '#!/bin/sh\necho new 1.0.0\n' > "$TEST_DIR/odd/meta-new"
    chmod +x "$TEST_DIR/odd/meta-new"
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

# serve this tar as the release's archive, with its checksum
serve() {
    cp "$1" "$RELEASES/latest/download/$ASSET"
    (cd "$RELEASES/latest/download" && sha256 "$ASSET" > "$ASSET.sha256")
}

@test "install.sh into ~/.meta/plugins, or a link to it, keeps what it installed" {
    mkdir -p "$HOME/.meta/plugins"
    ln -s "$HOME/.meta/plugins" "$TEST_DIR/plugins-link"
    local dir
    for dir in "$HOME/.meta/plugins" "$TEST_DIR/plugins-link"; do
        rm -f "$HOME/.meta/plugins"/*
        INSTALL_DIR="$dir" run bash "$BATS_TEST_DIRNAME/../install.sh"
        [ "$status" -eq 0 ]
        [ "$(ls "$HOME/.meta/plugins" | sort)" = "$(sort <<< "$BINS")" ]
    done
}

@test "install.sh installs nothing from an archive with anything but programs in it" {
    local bad="$TEST_DIR/bad"
    mkdir -p "$bad/docs" && cp "$TEST_DIR/archive/meta" "$TEST_DIR/archive/loop" "$bad/"
    echo readme > "$bad/README.md"
    tar -cf "$TEST_DIR/twice.tar" -C "$bad" meta loop && tar -rf "$TEST_DIR/twice.tar" -C "$bad" loop
    gzip -c "$TEST_DIR/twice.tar" > "$TEST_DIR/twice.tar.gz"
    tar -czf "$TEST_DIR/folder.tar.gz" -C "$bad" meta loop docs
    tar -czf "$TEST_DIR/readme.tar.gz" -C "$bad" meta loop README.md
    tar -czf "$TEST_DIR/dot.tar.gz" -C "$bad" ./meta ./loop
    mkdir "$TEST_DIR/hard" && cp "$TEST_DIR/archive/meta" "$TEST_DIR/hard/" && ln "$TEST_DIR/hard/meta" "$TEST_DIR/hard/loop"
    tar -czf "$TEST_DIR/hard.tar.gz" -C "$TEST_DIR/hard" meta loop
    local archive expected
    for archive in twice:"loop twice" folder:"docs/, not a program at its top level" \
        readme:"README.md, which is not an executable file" dot:"./meta, not a program at its top level" \
        hard:"loop, which is not an executable file"; do
        expected="${archive#*:}"
        serve "$TEST_DIR/${archive%%:*}.tar.gz"
        run bash "$BATS_TEST_DIRNAME/../install.sh"
        [ "$status" -eq 1 ]
        [[ "$output" == *"$ASSET holds $expected; nothing was installed"* ]]
        [ ! -e "$INSTALL_DIR" ]
    done
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

@test "install.ps1 accepts a certutil checksum, as older releases publish" {
    local zip hash
    for zip in meta-windows-x64.zip meta-windows-arm64.zip; do
        hash=$(sha256 "$RELEASES/latest/download/$zip" | awk '{print $1}')
        printf 'SHA256 hash of %s:\r\n%s\r\nCertUtil: -hashfile command completed successfully.\r\n' \
            "$zip" "$hash" > "$RELEASES/latest/download/$zip.sha256"
    done
    run_ps1
    [ "$status" -eq 0 ]
    [ "$(cat "$INSTALL_DIR/meta.exe")" = "fake meta" ]
}

# serve a zip of these files (paths under $TEST_DIR/winbad) as both Windows archives
serve_zip() {
    local zip
    for zip in meta-windows-x64.zip meta-windows-arm64.zip; do
        (cd "$TEST_DIR/winbad" && python3 -c 'import sys, zipfile
with zipfile.ZipFile(sys.argv[1], "w") as z:
    for f in sys.argv[2:]: z.write(f)' "$RELEASES/latest/download/$zip" "$@")
        (cd "$RELEASES/latest/download" && sha256 "$zip" > "$zip.sha256")
    done
}

@test "install.ps1 installs nothing from an archive with anything but programs in it, or without meta.exe" {
    command -v pwsh > /dev/null || skip "pwsh is not installed"
    mkdir -p "$TEST_DIR/winbad/docs"
    cp "$TEST_DIR/archive/meta.exe" "$TEST_DIR/archive/loop.exe" "$TEST_DIR/winbad/"
    echo readme > "$TEST_DIR/winbad/README.md"
    echo doc > "$TEST_DIR/winbad/docs/a.exe"
    local case files expected
    for case in "meta.exe loop.exe docs/a.exe|docs/a.exe, not a program at its top level" \
        "meta.exe README.md|README.md, not a program at its top level" \
        "loop.exe|no meta.exe"; do
        files="${case%%|*}" expected="${case#*|}"
        serve_zip $files
        run_ps1
        [ "$status" -eq 1 ]
        [[ "$output" == *"holds $expected; nothing was installed"* ]]
        [ ! -e "$INSTALL_DIR" ]
    done
    # a Unix symlink among the entries
    local zip
    for zip in meta-windows-x64.zip meta-windows-arm64.zip; do
        symlink_zip "$RELEASES/latest/download/$zip" b.exe
        (cd "$RELEASES/latest/download" && sha256 "$zip" > "$zip.sha256")
    done
    run_ps1
    [ "$status" -eq 1 ]
    [[ "$output" == *"holds b.exe, which is not a regular file; nothing was installed"* ]]
    [ ! -e "$INSTALL_DIR" ]
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
        chmod +x "$SCRATCH/built/$name"
    done
}

# symlink_zip ZIP LINK: meta.exe, alpha.exe and b.exe as regular files, but
# LINK a Unix symlink (as a zip made on Unix records one)
symlink_zip() {
    python3 -c 'import sys, zipfile
with zipfile.ZipFile(sys.argv[1], "w") as z:
    for name in ("meta.exe", "alpha.exe", "b.exe"):
        info = zipfile.ZipInfo(name)
        info.external_attr = ((0o120777 if name == sys.argv[2] else 0o100755) << 16)
        z.writestr(info, "meta.exe" if name == sys.argv[2] else "fake " + name)' "$1" "$2"
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
    # without --manifest-path, the workspace it is in, from anywhere
    cd /
    run python3 "$PROGRAMS" bins
    [ "$status" -eq 0 ]
    [[ "$output" == *meta* ]]
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

@test "the archive check takes only programs: once each, executable files, by their bare names" {
    scratch_workspace
    built meta alpha b
    mkdir "$SCRATCH/folder" && cp "$SCRATCH/built/meta" "$SCRATCH/built/b" "$SCRATCH/folder/" && mkdir "$SCRATCH/folder/alpha"
    tar -czf "$SCRATCH/folder.tar.gz" -C "$SCRATCH/folder" meta alpha b
    run programs check "$SCRATCH/folder.tar.gz"
    [ "$status" -ne 0 ]
    [[ "$output" == *"alpha: a folder, not a program file"* && "$output" == *"missing alpha"* ]]

    tar -cf "$SCRATCH/twice.tar" -C "$SCRATCH/built" meta alpha b && tar -rf "$SCRATCH/twice.tar" -C "$SCRATCH/built" meta
    gzip "$SCRATCH/twice.tar"
    run programs check "$SCRATCH/twice.tar.gz"
    [ "$status" -ne 0 ]
    [[ "$output" == *"meta: in it twice"* ]]

    tar -czf "$SCRATCH/dot.tar.gz" -C "$SCRATCH/built" .
    run programs check "$SCRATCH/dot.tar.gz"
    [ "$status" -ne 0 ]
    [[ "$output" == *"dot.tar.gz: its members are named ./NAME: name each program alone"* ]]
    [[ "$output" != *missing* ]]

    # b a hard link to meta: an executable file once extracted, but not a
    # program file in the archive
    mkdir "$SCRATCH/hard" && cp "$SCRATCH/built/meta" "$SCRATCH/built/alpha" "$SCRATCH/hard/" && ln "$SCRATCH/hard/meta" "$SCRATCH/hard/b"
    tar -czf "$SCRATCH/hard.tar.gz" -C "$SCRATCH/hard" meta alpha b
    run programs check "$SCRATCH/hard.tar.gz"
    [ "$status" -ne 0 ]
    [[ "$output" == *"b: not a regular file, not a program file"* ]]

    # b.exe a Unix symlink in a zip
    symlink_zip "$SCRATCH/link.zip" b.exe
    run programs check "$SCRATCH/link.zip"
    [ "$status" -ne 0 ]
    [[ "$output" == *"b.exe: a link, not a program file"* ]]
    # a zip that records permissions but no file type (Python's writestr) is fine
    python3 -c 'import sys, zipfile
with zipfile.ZipFile(sys.argv[1], "w") as z:
    for name in ("meta.exe", "alpha.exe", "b.exe"): z.writestr(name, "x")' "$SCRATCH/typeless.zip"
    run programs check "$SCRATCH/typeless.zip"
    [ "$status" -eq 0 ]

    # only others may run b, as install.sh refuses too
    chmod 0455 "$SCRATCH/built/b"
    tar -czf "$SCRATCH/others.tar.gz" -C "$SCRATCH/built" meta alpha b
    run programs check "$SCRATCH/others.tar.gz"
    [ "$status" -ne 0 ]
    [[ "$output" == *"b: not executable, not a program file"* ]]

    chmod -x "$SCRATCH/built/b"
    tar -czf "$SCRATCH/plain.tar.gz" -C "$SCRATCH/built" meta alpha b
    run programs check "$SCRATCH/plain.tar.gz"
    [ "$status" -ne 0 ]
    [[ "$output" == *"b: not executable, not a program file"* ]]
}

@test "a new or renamed bin target reaches the archive and the formula with no other edit" {
    scratch_workspace
    printf '\n[[bin]]\nname = "gamma"\npath = "src/lib.rs"\n' >> "$SCRATCH/lib/Cargo.toml"
    sed -i.bak 's/name = "alpha"/name = "alpha2"/' "$SCRATCH/a/Cargo.toml"
    built meta alpha2 b gamma
    local platform
    # one dist folder for all four, as a run by hand reuses it
    for platform in darwin-arm64 darwin-x64 linux-arm64 linux-x64; do
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
    # and installs only programs
    mkdir "$SCRATCH/built/docs"
    tar -czf "$SCRATCH/meta-linux-arm64.tar.gz" -C "$SCRATCH/built" meta alpha2 b gamma docs
    run programs formula 1.2.3 "$SCRATCH"/meta-*.tar.gz
    [ "$status" -ne 0 ]
    [[ "$output" == *"docs: a folder, not a program file"* ]]
}

# the plugin folders as an old `make install-all` and a user left them: the
# two old copies (a file and a link), the user's own meta-mcp, and a plugin
# named after no program
plugin_folders() {
    rm -rf "$HOME/.meta/plugins" "$SCRATCH/.meta/plugins"
    mkdir -p "$HOME/.meta/plugins" "$SCRATCH/.meta/plugins" "$TEST_DIR/elsewhere"
    echo kept > "$TEST_DIR/elsewhere/meta-cargo"
    local dir
    for dir in "$HOME/.meta/plugins" "$SCRATCH/.meta/plugins"; do
        echo old > "$dir/meta-git"
        ln -s "$TEST_DIR/elsewhere/meta-cargo" "$dir/meta-cargo"
        echo mine > "$dir/meta-mcp"
        echo other > "$dir/alpha"
    done
}

@test "make install and uninstall remove only the two old plugin copies, once installed; make clean only the checkout's" {
    scratch_workspace
    mkdir -p "$SCRATCH/scripts" "$SCRATCH/stub"
    cp "$BATS_TEST_DIRNAME/../Makefile" "$BATS_TEST_DIRNAME/../Makefile.context.mk" \
        "$BATS_TEST_DIRNAME/../install.sh" "$SCRATCH/"
    cp "$PROGRAMS" "$SCRATCH/scripts/"
    # cargo that reads metadata for real, records the rest, and fails an
    # install when FAIL is set
    printf '#!/bin/sh\n[ "$1" = metadata ] && exec "%s" "$@"\necho "cargo $*" >> "%s"\n[ "$1" = install ] && [ -n "$FAIL" ] && exit 1\nexit 0\n' \
        "$(command -v cargo)" "$TEST_DIR/cargo.log" > "$SCRATCH/stub/cargo"
    chmod +x "$SCRATCH/stub/cargo"
    export PATH="$SCRATCH/stub:$PATH"
    local all="alpha meta-cargo meta-git meta-mcp " left="alpha meta-mcp "

    # building touches neither folder
    plugin_folders
    run make -C "$SCRATCH"
    [ "$status" -eq 0 ]
    [ "$(ls "$HOME/.meta/plugins" | tr '\n' ' ')" = "$all" ]
    [ "$(ls "$SCRATCH/.meta/plugins" | tr '\n' ' ')" = "$all" ]

    # clean: the checkout's old copies only
    run make -C "$SCRATCH" clean
    [ "$status" -eq 0 ]
    [ "$(ls "$SCRATCH/.meta/plugins" | tr '\n' ' ')" = "$left" ]
    [ "$(ls "$HOME/.meta/plugins" | tr '\n' ' ')" = "$all" ]
    [[ "$output" == *"Removed the old copy .meta/plugins/meta-git"* ]]

    # an install that fails removes nothing
    FAIL=1 run make -C "$SCRATCH" install
    [ "$status" -ne 0 ]
    [ "$(ls "$HOME/.meta/plugins" | tr '\n' ' ')" = "$all" ]

    # install, then uninstall: the two old copies go (the link, not what it
    # points to), the rest stays, and each removal is named
    local target
    for target in install uninstall; do
        plugin_folders
        run make -C "$SCRATCH" "$target"
        [ "$status" -eq 0 ]
        [ "$(ls "$HOME/.meta/plugins" | tr '\n' ' ')" = "$left" ]
        [ "$(ls "$SCRATCH/.meta/plugins" | tr '\n' ' ')" = "$all" ]
        [[ "$output" == *"Removed the old copy $HOME/.meta/plugins/meta-git"* ]]
        [[ "$output" == *"Removed the old copy $HOME/.meta/plugins/meta-cargo"* ]]
        [ "$(cat "$TEST_DIR/elsewhere/meta-cargo")" = kept ]
    done
    grep -q 'cargo install --locked --force --path a$' "$TEST_DIR/cargo.log"
    grep -q 'cargo uninstall pkg_a$' "$TEST_DIR/cargo.log"

    # a folder with an old copy's name is never removed, and doesn't fail
    rm -rf "$HOME/.meta/plugins"/* && mkdir "$HOME/.meta/plugins/meta-cargo"
    run make -C "$SCRATCH" uninstall
    [ "$status" -eq 0 ]
    [ -d "$HOME/.meta/plugins/meta-cargo" ]

    # ~/.meta/plugins a link to the folder cargo installs into: install keeps
    # what it installed there, wherever CARGO_INSTALL_ROOT or CARGO_HOME puts it
    local var root
    for var in CARGO_INSTALL_ROOT CARGO_HOME; do
        root="$TEST_DIR/root-$var"
        mkdir -p "$root/bin" && echo new > "$root/bin/meta-git" && echo new > "$root/bin/meta-cargo"
        rm -rf "$HOME/.meta/plugins" && ln -s "$root/bin" "$HOME/.meta/plugins"
        env "$var=$root" make -C "$SCRATCH" install
        [ "$(ls "$root/bin" | tr '\n' ' ')" = "meta-cargo meta-git " ]
    done

    # the checkout's .meta/plugins a link to ~/.meta/plugins: clean leaves it
    rm -rf "$HOME/.meta/plugins" "$SCRATCH/.meta/plugins"
    mkdir -p "$HOME/.meta/plugins" && echo old > "$HOME/.meta/plugins/meta-git"
    ln -s "$HOME/.meta/plugins" "$SCRATCH/.meta/plugins"
    run make -C "$SCRATCH" clean
    [ "$status" -eq 0 ]
    [ -f "$HOME/.meta/plugins/meta-git" ]
}
