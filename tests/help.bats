#!/usr/bin/env bats

setup() {
    META_BIN="$BATS_TEST_DIRNAME/../target/debug/meta"
    META_GIT_BIN="$BATS_TEST_DIRNAME/../target/debug/meta-git"
    META_PROJECT_BIN="$BATS_TEST_DIRNAME/../target/debug/meta-project"
    META_RUST_BIN="$BATS_TEST_DIRNAME/../target/debug/meta-rust"

    if [ ! -f "$META_BIN" ] || [ ! -f "$META_GIT_BIN" ] || [ ! -f "$META_PROJECT_BIN" ] || [ ! -f "$META_RUST_BIN" ]; then
        cargo build --workspace --quiet
    fi

    TEST_DIR="$(mktemp -d)"
    NO_CONFIG_DIR="$(mktemp -d)"
    mkdir -p "$TEST_DIR/.meta/plugins" "$TEST_DIR/api" "$TEST_DIR/web"
    mkdir -p "$NO_CONFIG_DIR/.meta/plugins"
    cp "$META_GIT_BIN" "$TEST_DIR/.meta/plugins/meta-git"
    cp "$META_PROJECT_BIN" "$TEST_DIR/.meta/plugins/meta-project"
    cp "$META_RUST_BIN" "$TEST_DIR/.meta/plugins/meta-rust"
    cp "$META_GIT_BIN" "$NO_CONFIG_DIR/.meta/plugins/meta-git"
    cp "$META_RUST_BIN" "$NO_CONFIG_DIR/.meta/plugins/meta-rust"
    chmod +x "$TEST_DIR/.meta/plugins/meta-git" "$TEST_DIR/.meta/plugins/meta-project" "$TEST_DIR/.meta/plugins/meta-rust"
    chmod +x "$NO_CONFIG_DIR/.meta/plugins/meta-git"
    chmod +x "$NO_CONFIG_DIR/.meta/plugins/meta-rust"

    cat > "$TEST_DIR/.meta.json" <<'JSON'
{
    "projects": {
        "api": "git@github.com:org/api.git",
        "web": "git@github.com:org/web.git"
    }
}
JSON

    cd "$TEST_DIR"
}

teardown() {
    rm -rf "$TEST_DIR"
    rm -rf "$NO_CONFIG_DIR"
}

@test "meta help is plugin-aware and matches meta --help" {
    run "$META_BIN" --help
    [ "$status" -eq 0 ]
    help_flag="$output"

    run "$META_BIN" help
    [ "$status" -eq 0 ]
    [ "$output" = "$help_flag" ]
    [[ "$output" == *"git"* ]]
    [[ "$output" == *"project"* ]]
}

@test "built-in command help does not execute commands" {
    run "$META_BIN" context --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage: meta context"* ]]
    [[ "$output" != *"# Meta Workspace"* ]]

    run "$META_BIN" plugin search --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage: meta plugin search <QUERY>"* ]]
    [[ "$output" != *"required arguments were not provided"* ]]

    run "$META_BIN" plugin list --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage: meta plugin list"* ]]
    [[ "$output" != *"Installed plugins"* ]]
}

@test "git worktree help reaches worktree command implementation" {
    run "$META_BIN" git worktree --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Manage git worktrees across repos"* ]]
    [[ "$output" == *"--dry-run"* ]]
    [[ "$output" != *"Adapted Commands"* ]]

    run "$META_BIN" git worktree create --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage:"* ]]
    [[ "$output" == *"create"* ]]
    [[ "$output" == *"--dry-run"* ]]

    run "$META_BIN" git worktree list --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage:"* ]]
    [[ "$output" == *"list"* ]]
}

@test "nested plugin help does not require a meta config" {
    cd "$NO_CONFIG_DIR"

    run "$META_BIN" git worktree --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Manage git worktrees across repos"* ]]

    run "$META_BIN" git pull --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Pass-through git command"* ]]
    [[ "$output" == *"Current scope:"* ]]
}

@test "rust and cargo help use the simple plugin help without side effects" {
    cd "$NO_CONFIG_DIR"

    for command in "cargo --help" "cargo build --help" "rust --help" "rust build --help"; do
        run "$META_BIN" $command
        [ "$status" -eq 0 ]
        [[ "$output" == *"meta cargo <command>"* ]]
        [[ "$output" == *"Build all Rust projects"* ]]
        [[ "$output" != *"Could not find meta config"* ]]
        [[ "$output" != *"No Rust projects found"* ]]
    done
}

@test "git snapshot help reaches snapshot command implementation" {
    run "$META_BIN" git snapshot --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Workspace State Management"* ]]
    [[ "$output" != *"Adapted Commands"* ]]

    run "$META_BIN" git snapshot create --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"meta git snapshot create"* ]]
    [[ "$output" == *"Save workspace git state"* ]]
    [[ "$output" != *"Adapted Commands"* ]]
}

@test "git pass-through help is meta-aware" {
    for subcommand in pull fetch checkout; do
        run "$META_BIN" git "$subcommand" --help
        [ "$status" -eq 0 ]
        [[ "$output" == *"Pass-through git command"* ]]
        [[ "$output" == *"Runs \`git $subcommand\` in each repo"* ]]
        [[ "$output" == *"Current scope:"* ]]
        [[ "$output" == *"--dry-run"* ]]
        [[ "$output" == *"--recursive"* ]]
        [[ "$output" != *"Adapted Commands"* ]]
    done
}

@test "project child command help is explicit and agent-oriented" {
    run "$META_BIN" project list --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"meta project list"* ]]
    [[ "$output" == *"Agent hints:"* ]]
    [[ "$output" == *"--recursive --json"* ]]

    run "$META_BIN" project check --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"meta project check"* ]]
    [[ "$output" == *"missing"* ]]

    run "$META_BIN" project dependents --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"meta project dependents"* ]]
    [[ "$output" == *"blast radius"* ]]
}

@test "worktree create dry-run prints planned custom operations" {
    git init --quiet
    git config user.email "test@test.com"
    git config user.name "Test"
    touch README.md
    git add README.md
    git commit --quiet -m "init root"

    for repo in api web; do
        git -C "$repo" init --quiet
        git -C "$repo" config user.email "test@test.com"
        git -C "$repo" config user.name "Test"
        touch "$repo/README.md"
        git -C "$repo" add README.md
        git -C "$repo" commit --quiet -m "init $repo"
    done

    run "$META_BIN" --dry-run git worktree create preview --repo api
    [ "$status" -eq 0 ]
    [[ "$output" == *"[DRY RUN] Would create worktree set 'preview'"* ]]
    [[ "$output" == *"api:"* ]]
    [[ "$output" == *"bash:"* ]]
    [ ! -d "$TEST_DIR/.worktrees/preview" ]
}
