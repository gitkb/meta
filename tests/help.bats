#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

setup() {
    sandbox
    # api and web are plain folders: help never runs in a repo
    mkdir -p "$TEST_DIR/api" "$TEST_DIR/web"
    printf 'projects:\n  api: git@github.com:org/api.git\n  web: git@github.com:org/web.git\n' > "$TEST_DIR/.meta.yaml"
    make_workspace --plugins "git project cargo"
    NO_CONFIG_DIR="$BATS_TEST_TMPDIR/no-config"
    mkdir -p "$NO_CONFIG_DIR/.meta/plugins"
    cp "$META_BIN_DIR/meta-git" "$META_BIN_DIR/meta-cargo" "$NO_CONFIG_DIR/.meta/plugins/"
    cd "$TEST_DIR"
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
    [[ "$output" == *"Usage: meta plugin search [OPTIONS] <QUERY>"* ]]
    [[ "$output" != *"required arguments were not provided"* ]]

    run "$META_BIN" plugin list --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage: meta plugin list"* ]]
    [[ "$output" != *"Installed plugins"* ]]
}

@test "git worktree help reaches worktree command implementation" {
    run "$META_BIN" git worktree --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage: meta git worktree"* ]]
    [[ "$output" == *"--dry-run"* ]]
    [[ "$output" == *"--json"* ]]
    [[ "$output" != *"Adapted Commands"* ]]

    run "$META_BIN" git worktree create --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage:"* ]]
    [[ "$output" == *"create"* ]]
    [[ "$output" == *"--dry-run"* ]]
    [[ "$output" == *"[COMMIT_ISH]"* ]]
    [[ "$output" != *"--from-ref"* ]]

    run "$META_BIN" git worktree list --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage:"* ]]
    [[ "$output" == *"list"* ]]
}

@test "nested plugin help does not require a meta config" {
    cd "$NO_CONFIG_DIR"
    export PATH="$NO_CONFIG_DIR/.meta/plugins:$PATH"

    run "$META_BIN" git worktree --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage: meta git worktree"* ]]

    run "$META_BIN" git pull --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage: meta"*"git pull"* ]]
}

@test "cargo help uses the simple plugin help without side effects" {
    cd "$NO_CONFIG_DIR"
    export PATH="$NO_CONFIG_DIR/.meta/plugins:$PATH"

    for command in "cargo --help" "cargo build --help"; do
        run "$META_BIN" $command
        [ "$status" -eq 0 ]
        [[ "$output" == *"meta cargo"* ]]
        [[ "$output" != *"Could not find meta config"* ]]
        [[ "$output" != *"No Rust projects found"* ]]
    done
}

@test "git pass-through help is meta's, names the repos and runs no git" {
    # a git that says it ran
    mkdir -p "$BATS_TEST_TMPDIR/stub"
    printf '#!/bin/sh\ntouch "%s"\n' "$BATS_TEST_TMPDIR/git-ran" > "$BATS_TEST_TMPDIR/stub/git"
    chmod +x "$BATS_TEST_TMPDIR/stub/git"
    for subcommand in pull fetch checkout; do
        PATH="$BATS_TEST_TMPDIR/stub:$PATH" run "$META_BIN" git "$subcommand" --help
        [ "$status" -eq 0 ]
        [[ "$output" == *"Usage: meta"*"git $subcommand"* ]]
        [[ "$output" == *"api"* && "$output" == *"web"* ]]
        [ ! -e "$BATS_TEST_TMPDIR/git-ran" ]
        [[ "$output" == *"--dry-run"* ]]
        [[ "$output" == *"--recursive"* ]]
        [[ "$output" != *"Adapted Commands"* ]]
    done
}

@test "project child command help is explicit and agent-oriented" {
    run "$META_BIN" project list --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"meta project list"* ]]
    [[ "$output" == *"--recursive"* && "$output" == *"--json"* ]]

    run "$META_BIN" project check --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"meta project check"* ]]
    [[ "$output" == *"missing"* ]]

    run "$META_BIN" project dependents --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"meta project dependents"* ]]
}

@test "worktree create dry-run shows each repo's planned git command and makes nothing" {
    rmdir api web
    make_workspace --root-repo api web

    run "$META_BIN" --dry-run git worktree create preview --repo api
    [ "$status" -eq 0 ]
    [[ "$output" == *"$TEST_DIR/.worktrees/preview"* ]]
    [[ "$output" == *"git -C $TEST_DIR/api worktree add -b preview $TEST_DIR/.worktrees/preview/api"* ]]
    [[ "$output" != *"$TEST_DIR/web"* ]]
    [ ! -d "$TEST_DIR/.worktrees" ]
    [ -z "$(git -C api branch --list preview)" ]
}

@test "help lists cargo but not the rust plugin name as a command" {
    # Only the workspace's plugins: none from PATH, not even system ones
    run env PATH="$TEST_DIR/no-such-dir" "$META_BIN" --help
    [ "$status" -eq 0 ]
    [[ "$output" =~ [[:space:]]cargo[[:space:]] ]]
    if printf '%s\n' "$output" | grep -Eq '^  rust[[:space:]]'; then false; fi
}

@test "unknown command errors and suggestions escape controls and newlines" {
    run "$META_BIN" $'bad\033[31m\nFORGED'
    [ "$status" -ne 0 ]
    [[ "$output" != *$'\033'* ]]
    [[ "$output" != *$'\nFORGED'* ]]
    [[ "${lines[0]}" == *'bad\u{1b}[31m\nFORGED'* ]]
    [[ "$output" == *'meta exec --'*'\nFORGED'* ]]
}

@test "argument errors from meta and loop are escaped on one line" {
    LOOP_BIN="$META_BIN_DIR/loop"
    for cmd in "$META_BIN" "$LOOP_BIN"; do
        run "$cmd" $'--bo\033]0;TITLE\007\nFORGED'
        [ "$status" -ne 0 ]
        [[ "$output" != *$'\033'* ]]
        [[ "$output" != *$'\nFORGED'* ]]
    done
    # loop's verbose echo of its config path
    # (loop colours its own result lines, so look for the injected sequence)
    run "$LOOP_BIN" --verbose --config $'bad\033[2J\nFORGED' true
    [[ "$output" != *$'\033[2J'* ]]
    [[ "$output" != *$'\nFORGED'* ]]
    run "$LOOP_BIN" --verbose true $'bad\033[2J\nFORGED'
    [[ "$output" != *$'\033[2J'* ]]
    [[ "$output" != *$'\nFORGED'* ]]
    run "$META_BIN" --depth $'1\033[2J\nFORGED' exec -- true
    [ "$status" -ne 0 ]
    [[ "$output" != *$'\033'* ]]
    [[ "$output" != *$'\nFORGED'* ]]
}

@test "an argument that isn't UTF-8 gets clap's own error" {
    run "$META_BIN" exec -- ls $'\xff'
    [ "$status" -ne 0 ]
    [[ "$output" == *"invalid UTF-8"* ]]
    run "$META_BIN_DIR/loop" true $'\xff'
    [ "$status" -ne 0 ]
    [[ "$output" == *"invalid UTF-8"* ]]
}

@test "meta help <command> prints a built-in's own help" {
    for words in "exec" "context" "init" "init claude" "plugin" "plugin search" "help"; do
        run "$META_BIN" help $words
        [ "$status" -eq 0 ]
        [[ "$output" == *"Usage: meta $words"* ]]
        # one help option, under meta's name
        [ "$(grep -c -- '-h, --help' <<<"$output")" -eq 1 ]
    done
    run "$META_BIN" help plugin nosuch
    [ "$status" -ne 0 ]
    [[ "$output" == *"meta plugin has no command 'nosuch'"* ]]
}

@test "meta --help before a command still prints meta's help" {
    for words in "exec" "plugin install" "context"; do
        run "$META_BIN" --help $words
        [ "$status" -eq 0 ]
        [[ "$output" == *"Usage: meta [OPTIONS] [COMMAND]"* ]]
        [[ "$output" != *"required arguments"* ]]
    done
}

@test "worktree list help documents the options atc relies on" {
    run "$META_BIN" git worktree list --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"--recursive"* ]]
    [[ "$output" == *"--metadata-only"* ]]
    [[ "$output" == *"--json"* ]]
}

@test "meta --help lists the plugins whatever name meta was started by" {
    for name in meta.exe m; do
        run bash -c "exec -a $name '$META_BIN' --help"
        [ "$status" -eq 0 ]
        [[ "$output" == *"Usage: meta [OPTIONS] [COMMAND]"* ]]
        [[ "$output" == *"[plugin: git"* ]]
    done
}
