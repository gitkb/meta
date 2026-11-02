#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# Integration tests for `meta plugin install`, `list`, `uninstall` and
# `update`, against a release server on this machine: nothing reaches the
# network.

setup() {
    sandbox
    PLUGINS_DIR="$META_DATA_DIR/plugins"
    # a registry that refuses at once (port 9), unless a test serves one
    mkdir -p "$META_DATA_DIR"
    printf 'registries:\n  - http://127.0.0.1:9\n' > "$META_DATA_DIR/config.yaml"
    cd "$TEST_DIR"
}

teardown() {
    if [ -n "${SERVER_PID:-}" ]; then kill "$SERVER_PID" 2>/dev/null || true; fi
}

# serve: RELEASES (a GitHub-shaped folder) over HTTP; META_GITHUB_URL and the
# registry point at it. A request for PATH answers N when PATH.status holds N.
serve() {
    RELEASES="$BATS_TEST_TMPDIR/releases"
    mkdir -p "$RELEASES"
    python3 "$BATS_TEST_DIRNAME/helpers/release_server.py" "$RELEASES" "$BATS_TEST_TMPDIR/port" 3>&- &
    SERVER_PID=$!
    for _ in $(seq 100); do
        [ -s "$BATS_TEST_TMPDIR/port" ] && break
        sleep 0.1
    done
    [ -s "$BATS_TEST_TMPDIR/port" ]
    SERVER="http://127.0.0.1:$(cat "$BATS_TEST_TMPDIR/port")"
    export META_GITHUB_URL="$SERVER"
    printf 'registries:\n  - %s/registry\n' "$SERVER" > "$META_DATA_DIR/config.yaml"
}

# release PATH VERSION SAYS: a tar.gz at RELEASES/PATH holding a plugin
# meta-x of VERSION whose `meta x run` prints SAYS
release() {
    local build="$BATS_TEST_TMPDIR/build-$RANDOM"
    mkdir -p "$build" "$(dirname "$RELEASES/$1")"
    cat > "$build/meta-x" <<EOF
#!/bin/sh
case "\$1" in
    --meta-plugin-info) echo '{"name":"x","version":"$2","commands":["x run"]}' ;;
    --meta-plugin-exec) cat > /dev/null; echo "$3" ;;
esac
EOF
    chmod +x "$build/meta-x"
    tar -czf "$RELEASES/$1" -C "$build" meta-x
}

# ============ Installing from a release ============

@test "a shorthand install puts a plugin there that meta runs" {
    serve
    release gitkb/meta-x/releases/latest/download/meta-x-any.tar.gz 1.0.0 "x ran"
    run "$META_BIN" plugin install gitkb/meta-x
    [ "$status" -eq 0 ]
    [ -x "$PLUGINS_DIR/meta-x" ]
    run "$META_BIN" plugin list
    [[ "$output" == *"1.0.0"* ]]
    printf 'projects: {}\n' > .meta.yaml
    run "$META_BIN" x run
    [ "$status" -eq 0 ]
    [[ "$output" == *"x ran"* ]]
}

@test "META_PLATFORM picks that platform's asset" {
    serve
    release gitkb/meta-x/releases/latest/download/meta-x-any.tar.gz 1.0.0 any
    release gitkb/meta-x/releases/latest/download/meta-x-plan9-x64.tar.gz 1.0.0 plan9
    META_PLATFORM=plan9-x64 run "$META_BIN" plugin install gitkb/meta-x
    [ "$status" -eq 0 ]
    printf 'projects: {}\n' > .meta.yaml
    run "$META_BIN" x run
    [[ "$output" == *plan9* ]]
    [ "$(jq -r '.plugins["meta-x"].platform' "$PLUGINS_DIR/.manifest.json")" = plan9-x64 ]
}

@test "install --local puts the plugin in the workspace's .meta/plugins" {
    serve
    release gitkb/meta-x/releases/latest/download/meta-x-any.tar.gz 1.0.0 local
    printf 'projects: {}\n' > .meta.yaml
    run "$META_BIN" plugin install --local gitkb/meta-x
    [ "$status" -eq 0 ]
    [ -x "$TEST_DIR/.meta/plugins/meta-x" ]
    [ ! -e "$PLUGINS_DIR/meta-x" ]
}

@test "update --check names the newer release and changes nothing" {
    serve
    release gitkb/meta-x/releases/download/v1.0.0/meta-x-any.tar.gz 1.0.0 one
    release gitkb/meta-x/releases/latest/download/meta-x-any.tar.gz 2.0.0 two
    mkdir -p "$RELEASES/api/v3/repos/gitkb/meta-x/releases"
    echo '{"tag_name": "v2.0.0"}' > "$RELEASES/api/v3/repos/gitkb/meta-x/releases/latest"
    run "$META_BIN" plugin install gitkb/meta-x@v1.0.0
    [ "$status" -eq 0 ]
    cp "$PLUGINS_DIR/meta-x" "$BATS_TEST_TMPDIR/before"
    cp "$PLUGINS_DIR/.manifest.json" "$BATS_TEST_TMPDIR/before.json"
    run "$META_BIN" plugin update x --check
    [ "$status" -eq 0 ]
    [[ "$output" == *"v1.0.0"*"v2.0.0"* ]]
    cmp "$PLUGINS_DIR/meta-x" "$BATS_TEST_TMPDIR/before"
    cmp "$PLUGINS_DIR/.manifest.json" "$BATS_TEST_TMPDIR/before.json"
}

@test "an archive URL with a query string installs" {
    serve
    release direct/meta-x.tar.gz 1.0.0 direct
    run "$META_BIN" plugin install "$SERVER/direct/meta-x.tar.gz?token=x"
    [ "$status" -eq 0 ]
    [ -x "$PLUGINS_DIR/meta-x" ]
}

# ============ Failures install nothing ============

@test "a download that fails says so and installs nothing" {
    serve
    for code in 404 500; do
        mkdir -p "$RELEASES/direct"
        echo "$code" > "$RELEASES/direct/meta-x.tar.gz.status"
        run "$META_BIN" plugin install "$SERVER/direct/meta-x.tar.gz"
        [ "$status" -ne 0 ]
        [[ "$output" == *"Failed to download"* ]]
        [ ! -e "$PLUGINS_DIR/meta-x" ]
    done
}

@test "a repo with no release for this platform is not found, and nothing installs" {
    serve
    run "$META_BIN" plugin install gitkb/meta-x
    [ "$status" -ne 0 ]
    [[ "$output" == *"Could not find release"* ]]
    [ ! -e "$PLUGINS_DIR/meta-x" ]
}

@test "a download that isn't an archive installs nothing" {
    serve
    mkdir -p "$RELEASES/direct"
    echo "not an archive" > "$RELEASES/direct/notanarchive"
    run "$META_BIN" plugin install "$SERVER/direct/notanarchive"
    [ "$status" -ne 0 ]
    [[ "$output" == *"Unsupported archive format"* ]]
    [ -z "$(ls -A "$PLUGINS_DIR" 2>/dev/null | grep -v '^\.')" ]
}

@test "a name the registry doesn't know is not found" {
    serve
    mkdir -p "$RELEASES/registry/plugins"
    echo 404 > "$RELEASES/registry/plugins/nonexistent.status"
    run "$META_BIN" plugin install nonexistent
    [ "$status" -eq 1 ]
    [[ "$output" == *"Plugin 'nonexistent' not found in any registry"* ]]
    [ ! -e "$PLUGINS_DIR/meta-nonexistent" ]
}

@test "plugin install fails on invalid GitHub shorthand" {
    run "$META_BIN" plugin install invalid/format/too/many/slashes
    [ "$status" -ne 0 ]
}

# ============ Listing, uninstalling and escaping ============

@test "plugin list shows empty when no plugins installed" {
    # nor any on PATH, such as the ones `make install` puts there
    run env PATH="$BATS_TEST_TMPDIR/no-plugins" "$META_BIN" plugin list
    [ "$status" -eq 0 ]
    [[ "$output" =~ "No plugins installed" || "$output" =~ "Installed plugins: 0" ]]
}

@test "plugin uninstall fails when plugin not installed" {
    run "$META_BIN" plugin uninstall nonexistent-plugin
    [ "$status" -ne 0 ]
    [[ "$output" =~ "not installed" ]]
}

@test "plugin install --local requires workspace" {
    run "$META_BIN" plugin install --local nonexistent/repo
    [ "$status" -ne 0 ]
    [[ "$output" =~ "Not in a meta workspace" ]]
    [[ "$output" =~ "Create one here" ]]
}

@test "plugin list --local outside workspace shows error" {
    run "$META_BIN" plugin list --local
    [ "$status" -ne 0 ]
    [[ "$output" =~ "Not in a meta workspace" ]]
}

@test "plugin list --local in workspace with no plugins" {
    # A config file establishes a workspace.
    printf 'projects: {}\n' > "$TEST_DIR/.meta.yaml"
    run "$META_BIN" plugin list --local
    [ "$status" -eq 0 ]
    [[ "$output" =~ "No project-local plugins installed" ]]
}

@test "plugin list --local shows only local plugins" {
    # Create workspace with local plugin directory.
    printf 'projects: {}\n' > "$TEST_DIR/.meta.yaml"
    mkdir -p "$TEST_DIR/.meta/plugins"
    cat > "$TEST_DIR/.meta/plugins/meta-local-test" <<'PLUGIN'
#!/bin/sh
echo '{"name":"local-test","version":"2.0.0","commands":["local-test"]}'
PLUGIN
    chmod +x "$TEST_DIR/.meta/plugins/meta-local-test"

    # Create manifest
    cat > "$TEST_DIR/.meta/plugins/.manifest.json" <<EOF
{
  "plugins": {
    "meta-local-test": {
      "source": "test/repo",
      "version": "v1.0.0",
      "installed": "2024-01-01T00:00:00Z",
      "platform": "darwin-arm64"
    }
  }
}
EOF

    mkdir -p "$TEST_DIR/sub/dir"
    cd "$TEST_DIR/sub/dir"
    run "$META_BIN" plugin list --local
    [ "$status" -eq 0 ]
    [[ "$output" == *"$TEST_DIR/.meta/plugins/meta-local-test"* ]]
    [[ "$output" == *"2.0.0"* ]]
    [[ "$output" != *"v1.0.0"* ]]
    [[ "$output" =~ "local-test" ]]  # meta- prefix is stripped in output
    [[ "$output" =~ "Project-local plugins" ]]
}

@test "plugin uninstall --local fails when plugin not installed" {
    printf 'projects: {}\n' > "$TEST_DIR/.meta.yaml"
    mkdir -p "$TEST_DIR/.meta/plugins"
    run "$META_BIN" plugin uninstall --local nonexistent-plugin
    [ "$status" -ne 0 ]
    [[ "$output" =~ "not installed" ]]
}

@test "plugin search escapes text from the registry index" {
    mkdir -p "$BATS_TEST_TMPDIR/reg/plugins"
    cat > "$BATS_TEST_TMPDIR/reg/plugins/index.json" <<'EOF'
{"plugins": {"evil": {"name": "evil\u001b]0;T\u0007", "description": "d\u001b[2J",
  "version": "1\u001b[31m", "author": "a\u001b]52;c;eA==\u0007"}}}
EOF
    python3 -u -m http.server 0 --bind 127.0.0.1 --directory "$BATS_TEST_TMPDIR/reg" > "$BATS_TEST_TMPDIR/srv.log" 2>&1 &
    SRV=$!
    for _ in $(seq 50); do grep -q 'port [0-9]' "$BATS_TEST_TMPDIR/srv.log" && break; sleep 0.1; done
    PORT="$(grep -o 'port [0-9]*' "$BATS_TEST_TMPDIR/srv.log" | head -1 | cut -d' ' -f2)"
    mkdir -p "$META_DATA_DIR"
    printf 'registries:\n  - http://127.0.0.1:%s\n' "$PORT" > "$META_DATA_DIR/config.yaml"
    run "$META_BIN" plugin search evil
    kill "$SRV"
    [ "$status" -eq 0 ]
    [[ "$output" == *'evil\u{1b}]0;T\u{7}'* ]]
    [[ "$output" == *'d\u{1b}[2J'* ]]
    [[ "$output" == *'a\u{1b}]52;c;eA==\u{7}'* ]]
    [[ "$output" != *$'\e'* ]]
}

@test "log records escape control characters from registry config" {
    mkdir -p "$META_DATA_DIR"
    # Port 9 refuses at once; the URL is only logged, never reached
    printf 'registries:\n  - "http://127.0.0.1:9/r\\e[31m"\n' > "$META_DATA_DIR/config.yaml"
    RUST_LOG=debug run "$META_BIN" plugin search anything
    [[ "$output" == *'Fetching registry index from: http://127.0.0.1:9/r\u{1b}[31m'* ]]
    [[ "$output" != *$'\e'* ]]
}

@test "a plugin name in uninstall's error is escaped on one line" {
    cd "$TEST_DIR"
    echo 'projects: {}' > .meta.yaml
    run "$META_BIN" plugin uninstall $'bad\033[2J\nFORGED' --local
    [ "$status" -ne 0 ]
    [[ "$output" != *$'\033'* ]]
    [[ "$output" != *$'\nFORGED'* ]]
    [[ "$output" == *"is not installed"* ]]
}

@test "a plugin name in update's errors is escaped on one line" {
    cd "$TEST_DIR"
    for check in --check ""; do
        run "$META_BIN" plugin update $'bad\033[2J\nFORGED' $check
        [ "$status" -ne 0 ]
        [[ "$output" != *$'\033'* ]]
        [[ "$output" != *$'\nFORGED'* ]]
        [[ "$output" == *"bad\u{1b}[2J\nFORGED"* ]]
    done
}
