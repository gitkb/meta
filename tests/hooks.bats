#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

setup() {
    clear_git_local_env
    TEST_DIR="$(mktemp -d)"
}

teardown() {
    rm -rf "$TEST_DIR"
}

@test "pre-push clears repository-local Git environment without mutating caller config" {
    local workspace="$TEST_DIR/workspace"
    local fake_bin="$TEST_DIR/bin"
    local calls="$TEST_DIR/cargo-calls"
    local config_before="$TEST_DIR/config-before"
    mkdir -p "$workspace" "$fake_bin"
    printf '[workspace]\nmembers = []\n' >"$workspace/Cargo.toml"
    git -C "$workspace" init --quiet
    cp "$workspace/.git/config" "$config_before"

    cat >"$fake_bin/cargo" <<'EOF'
#!/bin/bash
set -e
while IFS= read -r git_env_var; do
    if [[ "$git_env_var" =~ ^[A-Z0-9_]+$ ]] && env | grep -q "^${git_env_var}="; then
        echo "${git_env_var} leaked into cargo" >&2
        exit 97
    fi
done < <(git rev-parse --local-env-vars)
git rev-parse --show-toplevel
printf '%s\n' "$*" >>"$META_TEST_CARGO_CALLS"
EOF
    chmod +x "$fake_bin/cargo"

    cd "$workspace"
    run env \
        PATH="$fake_bin:$PATH" \
        META_TEST_CARGO_CALLS="$calls" \
        GIT_DIR="$workspace/.git" \
        GIT_WORK_TREE="$workspace" \
        GIT_INDEX_FILE="$workspace/.git/index" \
        sh "$BATS_TEST_DIRNAME/../.githooks/pre-push"

    [ "$status" -eq 0 ]
    [[ "$output" == *"$workspace"* ]]
    [ "$(wc -l <"$calls" | tr -d ' ')" -eq 3 ]
    cmp -s "$config_before" "$workspace/.git/config"
}
