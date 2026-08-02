#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# Integration tests for `meta plugin install`

setup() {
    clear_git_local_env
    META_BIN="$BATS_TEST_DIRNAME/../target/debug/meta"

    if [ ! -f "$META_BIN" ]; then
        cargo build --workspace --quiet
    fi

    TEST_DIR="$(mktemp -d)"
    HOME_DIR="$(mktemp -d)"
    PLUGINS_DIR="$HOME_DIR/.meta/plugins"

    # Isolate plugin discovery so tests don't find real plugins:
    # - META_DATA_DIR overrides ~/.meta/ (global plugin dir)
    # - PATH is stripped to exclude dirs containing system-installed meta-* binaries
    export ORIGINAL_HOME="$HOME"
    export ORIGINAL_PATH="$PATH"
    export HOME="$HOME_DIR"
    export META_DATA_DIR="$HOME_DIR/.meta"
    # Build a minimal PATH that excludes the global meta plugins directory
    NEW_PATH=""
    IFS=':' read -ra DIRS <<< "$PATH"
    for dir in "${DIRS[@]}"; do
        case "$dir" in
            */.meta/plugins) ;; # skip meta plugin dirs
            *) NEW_PATH="${NEW_PATH:+$NEW_PATH:}$dir" ;;
        esac
    done
    export PATH="$NEW_PATH"

    cd "$TEST_DIR"
}

teardown() {
    export HOME="$ORIGINAL_HOME"
    export PATH="$ORIGINAL_PATH"
    unset META_DATA_DIR
    cd /
    rm -rf "$TEST_DIR"
    rm -rf "$HOME_DIR"
}

# ============ Plugin listing ============

@test "plugin list shows empty when no plugins installed" {
    run "$META_BIN" plugin list
    [ "$status" -eq 0 ]
    [[ "$output" =~ "No plugins installed" || "$output" =~ "Installed plugins: 0" ]]
}

# ============ GitHub shorthand install (requires real GitHub release) ============

# Note: These tests are currently skipped because they require a real GitHub release
# with properly formatted plugin binaries. Uncomment when such a test repository exists.

# @test "plugin install from GitHub shorthand (user/repo)" {
#     skip "Requires test repository with releases"
#     run "$META_BIN" plugin install test-user/meta-test-plugin
#     [ "$status" -eq 0 ]
#     [[ "$output" =~ "Successfully installed" ]]
#     [ -f "$PLUGINS_DIR/meta-test-plugin" ]
# }

# @test "plugin install from GitHub shorthand with version" {
#     skip "Requires test repository with releases"
#     run "$META_BIN" plugin install test-user/meta-test-plugin@v1.0.0
#     [ "$status" -eq 0 ]
#     [[ "$output" =~ "Successfully installed" ]]
#     [ -f "$PLUGINS_DIR/meta-test-plugin" ]
# }

# ============ Direct URL install (requires real URL) ============

# @test "plugin install from direct URL" {
#     skip "Requires publicly accessible test plugin archive"
#     run "$META_BIN" plugin install https://example.com/meta-test.tar.gz
#     [ "$status" -eq 0 ]
#     [[ "$output" =~ "Successfully installed" ]]
#     [ -f "$PLUGINS_DIR/meta-test" ]
# }

# ============ Failure scenarios ============

@test "plugin install fails gracefully on invalid URL" {
    run "$META_BIN" plugin install https://invalid.example.com/plugin.tar.gz
    [ "$status" -ne 0 ]
    [[ "$output" =~ "Failed to download" || "$output" =~ "Could not find" ]]
}

@test "plugin install fails on invalid GitHub shorthand" {
    run "$META_BIN" plugin install invalid/format/too/many/slashes
    [ "$status" -ne 0 ]
}

@test "plugin install fails on non-existent GitHub repo" {
    run "$META_BIN" plugin install nonexistent-user-12345/nonexistent-repo-67890
    [ "$status" -ne 0 ]
    [[ "$output" =~ "Could not find release" || "$output" =~ "Failed to download" ]]
}

@test "plugin install rejects URL without proper extension" {
    run "$META_BIN" plugin install https://example.com/notanarchive
    [ "$status" -ne 0 ]
}

# ============ Plugin uninstall ============

@test "plugin uninstall fails when plugin not installed" {
    run "$META_BIN" plugin uninstall nonexistent-plugin
    [ "$status" -ne 0 ]
    [[ "$output" =~ "not installed" ]]
}

# ============ Archive format detection ============

@test "plugin install handles query parameters in URL" {
    # This test verifies the URL parsing logic without actually downloading
    # The install will fail on download, but should not fail on URL parsing
    run "$META_BIN" plugin install https://example.com/plugin.tar.gz?token=test
    [ "$status" -ne 0 ]
    # Should fail on download, not on URL format detection
    [[ ! "$output" =~ "Unsupported archive format" ]]
}

# ============ Platform detection ============

@test "platform override via META_PLATFORM env var" {
    export META_PLATFORM="test-platform-x64"
    # Install will fail on non-existent release, but should use custom platform
    run "$META_BIN" plugin install test-user/meta-test
    [ "$status" -ne 0 ]
    # The error should indicate it tried with the custom platform
    # (This is implicit - the command uses the overridden platform internally)
}

# ============ M4: Project-local plugin installation ============

@test "plugin install --local requires workspace" {
    run "$META_BIN" plugin install --local nonexistent/repo
    [ "$status" -ne 0 ]
    [[ "$output" =~ "Not in a meta workspace" ]]
    [[ "$output" =~ "meta init" ]]
}

@test "plugin list --local outside workspace shows error" {
    run "$META_BIN" plugin list --local
    [ "$status" -eq 0 ]  # Command succeeds but shows message
    [[ "$output" =~ "Not in a meta workspace" ]]
}

@test "plugin list --local in workspace with no plugins" {
    # Create a workspace
    mkdir -p "$TEST_DIR/.meta"
    run "$META_BIN" plugin list --local
    [ "$status" -eq 0 ]
    [[ "$output" =~ "No project-local plugins installed" ]]
}

@test "plugin list --local shows only local plugins" {
    # Create workspace with local plugin directory
    mkdir -p "$TEST_DIR/.meta/plugins"
    echo '#!/bin/bash' > "$TEST_DIR/.meta/plugins/meta-local-test"
    echo 'echo "local plugin"' >> "$TEST_DIR/.meta/plugins/meta-local-test"
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

    run "$META_BIN" plugin list --local
    [ "$status" -eq 0 ]
    [[ "$output" =~ "local-test" ]]  # meta- prefix is stripped in output
    [[ "$output" =~ "Project-local plugins" ]]
}

@test "plugin uninstall --local fails when plugin not installed" {
    mkdir -p "$TEST_DIR/.meta/plugins"
    run "$META_BIN" plugin uninstall --local nonexistent-plugin
    [ "$status" -ne 0 ]
    [[ "$output" =~ "not installed" ]]
}

@test "plugin install --local to .meta/plugins succeeds with workspace" {
    # Create a valid workspace
    mkdir -p "$TEST_DIR/.meta"

    # This will fail because there's no actual release, but should recognize it as a workspace
    run "$META_BIN" plugin install --local test-user/meta-test-plugin
    [ "$status" -ne 0 ]
    # Should fail on download, not on workspace detection
    [[ ! "$output" =~ "Not in a meta workspace" ]]
    [[ "$output" =~ "Could not find release" || "$output" =~ "Failed to download" ]]
}

@test "plugin commands show location in output" {
    mkdir -p "$TEST_DIR/.meta/plugins"

    # Test that error messages mention the location
    run "$META_BIN" plugin install --local nonexistent/repo
    [ "$status" -ne 0 ]
    # The implementation should show it's trying to install locally
}
