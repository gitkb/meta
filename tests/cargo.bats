#!/usr/bin/env bats

# Integration tests for the explicit `meta cargo` / `meta rust` namespaces.
# A controlled fake Cargo records cwd + argv so these tests never operate on
# real build artifacts.

setup() {
    META_BIN="$BATS_TEST_DIRNAME/../target/debug/meta"
    META_RUST_BIN="$BATS_TEST_DIRNAME/../target/debug/meta-rust"

    if [ ! -f "$META_BIN" ] || [ ! -f "$META_RUST_BIN" ]; then
        cargo build --workspace --quiet
    fi

    TEST_DIR="$(mktemp -d)"
    TEST_DIR="$(cd "$TEST_DIR" && pwd -P)"
    CARGO_LOG="$TEST_DIR/cargo.log"
    HOME="$TEST_DIR/home"
    export HOME

    mkdir -p \
        "$TEST_DIR/.meta/plugins" \
        "$TEST_DIR/bin" \
        "$HOME" \
        "$TEST_DIR/rust-app/target" \
        "$TEST_DIR/docs" \
        "$TEST_DIR/nested/nested-rust/target" \
        "$TEST_DIR/nested/nested-docs" \
        "$TEST_DIR/target"

    cp "$META_RUST_BIN" "$TEST_DIR/.meta/plugins/meta-rust"
    chmod +x "$TEST_DIR/.meta/plugins/meta-rust"

    cat > "$TEST_DIR/.meta.yaml" <<'YAML'
defaults:
  parallel: false

projects:
  rust-app:
    repo: git@github.com:org/rust-app.git
  docs:
    repo: git@github.com:org/docs.git
  nested:
    repo: git@github.com:org/nested.git
    meta: true
YAML

    cat > "$TEST_DIR/nested/.meta.yaml" <<'YAML'
projects:
  nested-rust:
    repo: git@github.com:org/nested-rust.git
  nested-docs:
    repo: git@github.com:org/nested-docs.git
YAML

    cat > "$TEST_DIR/Cargo.toml" <<'TOML'
[package]
name = "fixture-root"
version = "0.0.0"
TOML
    cat > "$TEST_DIR/rust-app/Cargo.toml" <<'TOML'
[package]
name = "fixture-rust-app"
version = "0.0.0"
TOML
    cat > "$TEST_DIR/nested/nested-rust/Cargo.toml" <<'TOML'
[package]
name = "fixture-nested-rust"
version = "0.0.0"
TOML

    touch \
        "$TEST_DIR/target/keep" \
        "$TEST_DIR/rust-app/target/keep" \
        "$TEST_DIR/nested/nested-rust/target/keep"

    cat > "$TEST_DIR/bin/cargo" <<'SH'
#!/bin/sh
set -eu

: "${CARGO_LOG:?CARGO_LOG must be set}"
{
    printf 'BEGIN\n'
    printf 'cwd=<%s>\n' "$PWD"
    printf 'argc=<%s>\n' "$#"
    for arg in "$@"; do
        printf 'arg=<%s>\n' "$arg"
    done
    printf 'END\n'
} >> "$CARGO_LOG"

if [ "${1-}" = "definitely-not-a-command" ]; then
    printf 'fake cargo: no such command: %s\n' "$1" >&2
    exit 101
fi
SH
    chmod +x "$TEST_DIR/bin/cargo"

    cd "$TEST_DIR"
}

teardown() {
    rm -rf "$TEST_DIR"
}

run_with_fake_cargo() {
    PATH="$TEST_DIR/bin:$PATH" CARGO_LOG="$CARGO_LOG" "$META_BIN" "$@"
}

@test "cargo clean --recursive runs once in each Rust project" {
    run run_with_fake_cargo cargo clean --recursive

    [ "$status" -eq 0 ]
    [ "$(grep -c '^BEGIN$' "$CARGO_LOG")" -eq 3 ]
    [ "$(grep -F -x -c "cwd=<$TEST_DIR>" "$CARGO_LOG")" -eq 1 ]
    [ "$(grep -F -x -c "cwd=<$TEST_DIR/rust-app>" "$CARGO_LOG")" -eq 1 ]
    [ "$(grep -F -x -c "cwd=<$TEST_DIR/nested/nested-rust>" "$CARGO_LOG")" -eq 1 ]
    [ "$(grep -F -x -c 'arg=<clean>' "$CARGO_LOG")" -eq 3 ]
    ! grep -F -q 'arg=<--recursive>' "$CARGO_LOG"
    ! grep -F -q "cwd=<$TEST_DIR/docs>" "$CARGO_LOG"
    ! grep -F -q "cwd=<$TEST_DIR/nested>" "$CARGO_LOG"
    ! grep -F -q "cwd=<$TEST_DIR/nested/nested-docs>" "$CARGO_LOG"
}

@test "Cargo and Rust namespace plans ignore Loop aliases" {
    cat > "$TEST_DIR/.looprc" <<'JSON'
{"aliases":{"cargo":"true"}}
JSON

    run run_with_fake_cargo cargo clean --recursive

    [ "$status" -eq 0 ]
    [ "$(grep -c '^BEGIN$' "$CARGO_LOG")" -eq 3 ]
    [ "$(grep -F -x -c 'arg=<clean>' "$CARGO_LOG")" -eq 3 ]

    rm -f "$CARGO_LOG"
    run run_with_fake_cargo rust clean --recursive

    [ "$status" -eq 0 ]
    [ "$(grep -c '^BEGIN$' "$CARGO_LOG")" -eq 3 ]
    [ "$(grep -F -x -c 'arg=<clean>' "$CARGO_LOG")" -eq 3 ]
}

@test "leading Cargo global options preserve recursive compatibility" {
    run run_with_fake_cargo cargo --locked clean --recursive

    [ "$status" -eq 0 ]
    [ "$(grep -c '^BEGIN$' "$CARGO_LOG")" -eq 3 ]
    [ "$(grep -F -x -c 'arg=<--locked>' "$CARGO_LOG")" -eq 3 ]
    [ "$(grep -F -x -c 'arg=<clean>' "$CARGO_LOG")" -eq 3 ]
    ! grep -F -q 'arg=<--recursive>' "$CARGO_LOG"
    grep -F -x -q "cwd=<$TEST_DIR/nested/nested-rust>" "$CARGO_LOG"
    ! grep -F -q "cwd=<$TEST_DIR/docs>" "$CARGO_LOG"
}

@test "cargo clean recursive dry-run prints the exact plan without cleanup" {
    run run_with_fake_cargo --dry-run cargo clean --recursive

    [ "$status" -eq 0 ]
    [ ! -e "$CARGO_LOG" ]
    [ "$(printf '%s\n' "$output" | grep -F -x -c '  cargo clean')" -eq 3 ]
    [[ "$output" == *"$TEST_DIR"* ]]
    [[ "$output" == *"$TEST_DIR/rust-app"* ]]
    [[ "$output" == *"$TEST_DIR/nested/nested-rust"* ]]
    [[ "$output" != *"$TEST_DIR/docs"* ]]
    [ -f "$TEST_DIR/target/keep" ]
    [ -f "$TEST_DIR/rust-app/target/keep" ]
    [ -f "$TEST_DIR/nested/nested-rust/target/keep" ]
}

@test "cargo update retains its recursive argument and does not recurse Meta scope" {
    run run_with_fake_cargo cargo update --recursive

    [ "$status" -eq 0 ]
    [ "$(grep -c '^BEGIN$' "$CARGO_LOG")" -eq 2 ]
    [ "$(grep -F -x -c 'arg=<update>' "$CARGO_LOG")" -eq 2 ]
    [ "$(grep -F -x -c 'arg=<--recursive>' "$CARGO_LOG")" -eq 2 ]
    ! grep -F -q "cwd=<$TEST_DIR/nested/nested-rust>" "$CARGO_LOG"
}

@test "Cargo payload after the separator is never intercepted by Meta" {
    run run_with_fake_cargo cargo test -- --recursive --help

    [ "$status" -eq 0 ]
    [ "$(grep -c '^BEGIN$' "$CARGO_LOG")" -eq 2 ]
    [ "$(grep -F -x -c 'argc=<4>' "$CARGO_LOG")" -eq 2 ]
    [ "$(grep -F -x -c 'arg=<test>' "$CARGO_LOG")" -eq 2 ]
    [ "$(grep -F -x -c 'arg=<-->' "$CARGO_LOG")" -eq 2 ]
    [ "$(grep -F -x -c 'arg=<--recursive>' "$CARGO_LOG")" -eq 2 ]
    [ "$(grep -F -x -c 'arg=<--help>' "$CARGO_LOG")" -eq 2 ]
    ! grep -F -q "cwd=<$TEST_DIR/nested/nested-rust>" "$CARGO_LOG"
    [[ "$output" != *"Run any Cargo command across selected Rust projects"* ]]
}

@test "build check clippy and installed Cargo extensions pass through" {
    run run_with_fake_cargo cargo build --release
    [ "$status" -eq 0 ]
    [ "$(grep -F -x -c 'arg=<build>' "$CARGO_LOG")" -eq 2 ]
    [ "$(grep -F -x -c 'arg=<--release>' "$CARGO_LOG")" -eq 2 ]

    rm -f "$CARGO_LOG"
    run run_with_fake_cargo cargo check --all-targets
    [ "$status" -eq 0 ]
    [ "$(grep -F -x -c 'arg=<check>' "$CARGO_LOG")" -eq 2 ]
    [ "$(grep -F -x -c 'arg=<--all-targets>' "$CARGO_LOG")" -eq 2 ]

    rm -f "$CARGO_LOG"
    run run_with_fake_cargo cargo clippy --all-targets -- -D warnings
    [ "$status" -eq 0 ]
    [ "$(grep -F -x -c 'argc=<5>' "$CARGO_LOG")" -eq 2 ]
    [ "$(grep -F -x -c 'arg=<clippy>' "$CARGO_LOG")" -eq 2 ]
    [ "$(grep -F -x -c 'arg=<-D>' "$CARGO_LOG")" -eq 2 ]
    [ "$(grep -F -x -c 'arg=<warnings>' "$CARGO_LOG")" -eq 2 ]

    rm -f "$CARGO_LOG"
    run run_with_fake_cargo cargo nextest run --recursive
    [ "$status" -eq 0 ]
    [ "$(grep -F -x -c 'argc=<3>' "$CARGO_LOG")" -eq 2 ]
    [ "$(grep -F -x -c 'arg=<nextest>' "$CARGO_LOG")" -eq 2 ]
    [ "$(grep -F -x -c 'arg=<run>' "$CARGO_LOG")" -eq 2 ]
    [ "$(grep -F -x -c 'arg=<--recursive>' "$CARGO_LOG")" -eq 2 ]
    ! grep -F -q "cwd=<$TEST_DIR/nested/nested-rust>" "$CARGO_LOG"
}

@test "Meta controls go before the namespace and postfix controls belong to Cargo" {
    run run_with_fake_cargo --verbose --include rust-app cargo check

    [ "$status" -eq 0 ]
    [[ "$output" == *"Verbose mode enabled"* ]]
    [ "$(grep -c '^BEGIN$' "$CARGO_LOG")" -eq 1 ]
    [ "$(grep -F -x -c 'argc=<1>' "$CARGO_LOG")" -eq 1 ]
    grep -F -x -q 'arg=<check>' "$CARGO_LOG"

    rm -f "$CARGO_LOG"
    run run_with_fake_cargo --include rust-app cargo check --verbose --dry-run

    [ "$status" -eq 0 ]
    [ "$(grep -c '^BEGIN$' "$CARGO_LOG")" -eq 1 ]
    [ "$(grep -F -x -c 'argc=<3>' "$CARGO_LOG")" -eq 1 ]
    grep -F -x -q 'arg=<check>' "$CARGO_LOG"
    grep -F -x -q 'arg=<--verbose>' "$CARGO_LOG"
    grep -F -x -q 'arg=<--dry-run>' "$CARGO_LOG"
}

@test "rust is behaviorally equivalent to the canonical cargo namespace" {
    run run_with_fake_cargo cargo check --message-format json
    [ "$status" -eq 0 ]
    cp "$CARGO_LOG" "$TEST_DIR/cargo-namespace.log"

    rm -f "$CARGO_LOG"
    run run_with_fake_cargo rust check --message-format json
    [ "$status" -eq 0 ]

    cmp -s "$TEST_DIR/cargo-namespace.log" "$CARGO_LOG"
}

@test "Cargo argument values remain data across the shell plan boundary" {
    command_substitution="\$(touch \"$TEST_DIR/command-substitution-ran\")"
    semicolon="; touch \"$TEST_DIR/semicolon-ran\""

    run run_with_fake_cargo \
        --include rust-app \
        --sequential \
        cargo clippy --all-targets -- -D warnings \
        "value with spaces" \
        "$command_substitution" \
        "$semicolon" \
        "left&right" \
        "it's data"

    [ "$status" -eq 0 ]
    [ "$(grep -c '^BEGIN$' "$CARGO_LOG")" -eq 1 ]
    [ "$(grep -F -x -c 'argc=<10>' "$CARGO_LOG")" -eq 1 ]
    grep -F -x -q 'arg=<value with spaces>' "$CARGO_LOG"
    grep -F -x -q "arg=<$command_substitution>" "$CARGO_LOG"
    grep -F -x -q "arg=<$semicolon>" "$CARGO_LOG"
    grep -F -x -q 'arg=<left&right>' "$CARGO_LOG"
    grep -F -x -q "arg=<it's data>" "$CARGO_LOG"
    [ ! -e "$TEST_DIR/command-substitution-ran" ]
    [ ! -e "$TEST_DIR/semicolon-ran" ]
}

@test "Cargo diagnoses invalid namespaced commands while top-level typos stay rejected" {
    run run_with_fake_cargo cargo definitely-not-a-command

    [ "$status" -ne 0 ]
    [[ "$output" == *"fake cargo: no such command"* ]]
    [[ "$output" != *"unrecognized command"* ]]
    [ "$(grep -c '^BEGIN$' "$CARGO_LOG")" -eq 2 ]

    rm -f "$CARGO_LOG"
    run run_with_fake_cargo carg clean

    [ "$status" -eq 1 ]
    [[ "$output" == *"unrecognized command 'carg'"* ]]
    [ ! -e "$CARGO_LOG" ]
}

@test "an empty Rust scope reports a clear result without invoking Cargo" {
    rm -f "$TEST_DIR/Cargo.toml" "$TEST_DIR/rust-app/Cargo.toml"

    run run_with_fake_cargo cargo check

    [ "$status" -eq 0 ]
    [[ "$output" == *"No Rust projects found"* ]]
    [ ! -e "$CARGO_LOG" ]
}

@test "an include or exclude selection with no Rust projects reports a clear result" {
    run run_with_fake_cargo --include docs cargo check

    [ "$status" -eq 0 ]
    [[ "$output" == *"No Rust projects found"* ]]
    [ ! -e "$CARGO_LOG" ]

    run run_with_fake_cargo --exclude "$TEST_DIR" cargo check

    [ "$status" -eq 0 ]
    [[ "$output" == *"No Rust projects found"* ]]
    [ ! -e "$CARGO_LOG" ]
}

@test "a worktree without Meta config still uses safe Cargo plugin dispatch" {
    rm -f "$TEST_DIR/.meta.yaml"
    mkdir -p \
        "$TEST_DIR/.worktrees/no-config/docs" \
        "$TEST_DIR/.worktrees/no-config/rust-app"

    printf 'gitdir: %s\n' \
        "$TEST_DIR/source-docs/.git/worktrees/no-config-docs" \
        > "$TEST_DIR/.worktrees/no-config/docs/.git"
    printf 'gitdir: %s\n' \
        "$TEST_DIR/source-rust/.git/worktrees/no-config-rust" \
        > "$TEST_DIR/.worktrees/no-config/rust-app/.git"
    cat > "$TEST_DIR/.worktrees/no-config/rust-app/Cargo.toml" <<'TOML'
[package]
name = "worktree-rust-app"
version = "0.0.0"
TOML

    cat > "$TEST_DIR/bin/carg" <<'SH'
#!/bin/sh
touch "${CARGO_LOG:?}.top-level-typo-ran"
SH
    chmod +x "$TEST_DIR/bin/carg"

    cd "$TEST_DIR/.worktrees/no-config/rust-app"
    dangerous='quoted"&still-data'
    run run_with_fake_cargo cargo check "value with spaces" "$dangerous"

    [ "$status" -eq 0 ]
    [ "$(grep -c '^BEGIN$' "$CARGO_LOG")" -eq 1 ]
    grep -F -x -q "cwd=<$TEST_DIR/.worktrees/no-config/rust-app>" "$CARGO_LOG"
    grep -F -x -q 'arg=<check>' "$CARGO_LOG"
    grep -F -x -q 'arg=<value with spaces>' "$CARGO_LOG"
    grep -F -x -q "arg=<$dangerous>" "$CARGO_LOG"
    ! grep -F -q "cwd=<$TEST_DIR/.worktrees/no-config/docs>" "$CARGO_LOG"

    cp "$CARGO_LOG" "$TEST_DIR/worktree-cargo.log"
    rm -f "$CARGO_LOG"
    run run_with_fake_cargo rust check "value with spaces" "$dangerous"

    [ "$status" -eq 0 ]
    cmp -s "$TEST_DIR/worktree-cargo.log" "$CARGO_LOG"

    rm -f "$CARGO_LOG"
    run run_with_fake_cargo carg clean

    [ "$status" -eq 1 ]
    [[ "$output" == *"unrecognized command 'carg'"* ]]
    [ ! -e "$CARGO_LOG.top-level-typo-ran" ]
    [ ! -e "$CARGO_LOG" ]
}
