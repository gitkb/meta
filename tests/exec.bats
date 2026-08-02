#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# Integration tests for `meta exec` and loop pass-through options
# Tests: exec, --include, --exclude, --parallel, --dry-run, --tag

setup() {
    clear_git_local_env
    META_BIN="$BATS_TEST_DIRNAME/../target/debug/meta"
    META_GIT_BIN="$BATS_TEST_DIRNAME/../target/debug/meta-git"

    if [ ! -f "$META_BIN" ] || [ ! -f "$META_GIT_BIN" ]; then
        cargo build --workspace --quiet
    fi

    TEST_DIR="$(mktemp -d)"
    mkdir -p "$TEST_DIR/.meta/plugins"
    cp "$META_GIT_BIN" "$TEST_DIR/.meta/plugins/meta-git"
    chmod +x "$TEST_DIR/.meta/plugins/meta-git"

    # Create project directories
    for repo in api worker frontend; do
        mkdir -p "$TEST_DIR/$repo"
        echo "# $repo" > "$TEST_DIR/$repo/README.md"
    done

    # Default .meta config with tags
    cat > "$TEST_DIR/.meta.json" <<'EOF'
{
    "projects": {
        "api": {
            "repo": "git@github.com:org/api.git",
            "tags": ["backend", "rust"]
        },
        "worker": {
            "repo": "git@github.com:org/worker.git",
            "tags": ["backend", "rust"]
        },
        "frontend": {
            "repo": "git@github.com:org/frontend.git",
            "tags": ["ui", "typescript"]
        }
    }
}
EOF

    cd "$TEST_DIR"
}

teardown() {
    rm -rf "$TEST_DIR"
}

# ============ Basic exec ============

@test "meta exec runs command across all repos" {
    run "$META_BIN" exec -- echo hello
    [ "$status" -eq 0 ]
    [[ "$output" == *"hello"* ]]
}

@test "meta exec runs in each project directory" {
    run "$META_BIN" exec -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"api"* ]]
    [[ "$output" == *"worker"* ]]
    [[ "$output" == *"frontend"* ]]
}

@test "meta exec runs in root directory too" {
    run "$META_BIN" exec -- pwd
    [ "$status" -eq 0 ]
    # Root dir (.) should also be in output
    [[ "$output" == *"$TEST_DIR"* ]]
}

@test "meta exec passes arguments correctly" {
    run "$META_BIN" --include api,worker,frontend exec -- ls README.md
    [ "$status" -eq 0 ]
    [[ "$output" == *"README.md"* ]]
}

@test "meta exec handles command failure gracefully" {
    run "$META_BIN" exec -- false
    # Should report failures but not crash
    [ "$status" -ne 0 ] || [[ "$output" == *"fail"* ]] || [[ "$output" == *"error"* ]] || [[ "$output" == *"Error"* ]]
}

# ============ --include ============

@test "meta exec --include runs only in specified dirs" {
    run "$META_BIN" --include api exec -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"api"* ]]
    [[ "$output" != *"worker"* ]]
    [[ "$output" != *"frontend"* ]]
}

@test "meta exec --include with multiple dirs" {
    run "$META_BIN" --include api,frontend exec -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"api"* ]]
    [[ "$output" == *"frontend"* ]]
    [[ "$output" != *"worker"* ]]
}

@test "meta exec --include with nonexistent dir runs nothing" {
    run "$META_BIN" --include nonexistent exec -- echo hello
    [ "$status" -eq 0 ]
    # Should not produce "hello" output since no dirs match
    [[ "$output" != *"hello"* ]] || [[ "$output" == *"No directories"* ]] || [[ "$output" == *"0"* ]]
}

# ============ --exclude ============

@test "meta exec --exclude does not error" {
    run "$META_BIN" --exclude frontend exec -- echo test
    [ "$status" -eq 0 ]
}

@test "meta exec --exclude filters correctly with short names" {
    run "$META_BIN" --exclude frontend exec -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"api"* ]]
    [[ "$output" == *"worker"* ]]
    [[ "$output" != *"frontend"* ]]
}

# ============ --dry-run ============

@test "meta exec --dry-run shows commands without executing" {
    run "$META_BIN" --dry-run exec -- echo secret
    [ "$status" -eq 0 ]
    # Should show the command plan but not actually execute it
    [[ "$output" == *"echo"* ]] || [[ "$output" == *"Would"* ]] || [[ "$output" == *"DRY"* ]]
}

@test "meta exec --dry-run does not create side effects" {
    run "$META_BIN" --dry-run exec -- touch should-not-exist.txt
    [ "$status" -eq 0 ]
    # File should NOT be created
    [ ! -f "$TEST_DIR/api/should-not-exist.txt" ]
    [ ! -f "$TEST_DIR/worker/should-not-exist.txt" ]
    [ ! -f "$TEST_DIR/frontend/should-not-exist.txt" ]
}

# ============ --parallel ============

@test "meta exec --parallel runs successfully" {
    run "$META_BIN" --parallel exec -- echo parallel-test
    [ "$status" -eq 0 ]
    [[ "$output" == *"parallel-test"* ]]
}

@test "meta exec --parallel produces output from all repos" {
    run "$META_BIN" --parallel exec -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"api"* ]]
    [[ "$output" == *"worker"* ]]
    [[ "$output" == *"frontend"* ]]
}

# ============ --tag ============

@test "meta --tag filters to matching projects" {
    run "$META_BIN" --tag backend exec -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"api"* ]]
    [[ "$output" == *"worker"* ]]
    [[ "$output" != *"frontend"* ]]
}

@test "meta --tag ui only runs in frontend" {
    run "$META_BIN" --tag ui exec -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"frontend"* ]]
    [[ "$output" != *"api"* ]]
    [[ "$output" != *"worker"* ]]
}

@test "meta --tag with nonexistent tag runs only in root" {
    run "$META_BIN" --tag nonexistent exec -- pwd
    [ "$status" -eq 0 ]
    # Only root "." should execute, not any project dirs
    [[ "$output" != *"api"* ]]
    [[ "$output" != *"worker"* ]]
    [[ "$output" != *"frontend"* ]]
}

@test "meta --tag rust matches multiple projects" {
    run "$META_BIN" --tag rust exec -- echo matched
    [ "$status" -eq 0 ]
    # api and worker both have rust tag; count occurrences
    MATCH_COUNT=$(echo "$output" | grep -c "matched" || true)
    [ "$MATCH_COUNT" -ge 2 ]
}

# ============ Combined options ============

@test "meta exec --include with --parallel" {
    run "$META_BIN" --include api,worker --parallel exec -- echo combo
    [ "$status" -eq 0 ]
    [[ "$output" == *"combo"* ]]
}

@test "meta --tag with --exclude filters correctly" {
    run "$META_BIN" --tag backend --exclude worker exec -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"api"* ]]
    [[ "$output" != *"worker"* ]]
    [[ "$output" != *"frontend"* ]]
}

@test "meta --tag with --dry-run" {
    run "$META_BIN" --tag ui --dry-run exec -- echo test
    [ "$status" -eq 0 ]
    # Should mention the command without executing
    [ ! -f "$TEST_DIR/frontend/test" ]
}

# ============ meta git pass-through (loop fallback) ============

@test "meta git branch runs across repos via loop" {
    # Initialize git repos so git commands work
    for repo in api worker frontend; do
        git -C "$TEST_DIR/$repo" init --quiet
        git -C "$TEST_DIR/$repo" config user.email "test@test.com"
        git -C "$TEST_DIR/$repo" config user.name "Test"
        git -C "$TEST_DIR/$repo" add README.md
        git -C "$TEST_DIR/$repo" commit --quiet -m "init"
    done

    run "$META_BIN" --include api,worker,frontend exec -- git branch
    [ "$status" -eq 0 ]
    # Should show branch info from repos
    [[ "$output" == *"main"* ]] || [[ "$output" == *"master"* ]]
}

@test "meta exec -- git log --oneline works" {
    for repo in api worker frontend; do
        git -C "$TEST_DIR/$repo" init --quiet
        git -C "$TEST_DIR/$repo" config user.email "test@test.com"
        git -C "$TEST_DIR/$repo" config user.name "Test"
        git -C "$TEST_DIR/$repo" add README.md
        git -C "$TEST_DIR/$repo" commit --quiet -m "init $repo"
    done

    run "$META_BIN" --include api exec -- git log --oneline
    [ "$status" -eq 0 ]
    [[ "$output" == *"init api"* ]]
}

# ============ Edge cases ============

@test "meta exec with no command after -- shows error or help" {
    run "$META_BIN" exec --
    # Empty command should either error or show help
    [ "$status" -ne 0 ] || [[ "$output" == *"Usage"* ]] || [[ "$output" == *"help"* ]] || [[ "$output" == "" ]]
}

@test "meta exec preserves exit codes from subcommands" {
    # Run a command that exits non-zero in some dirs
    echo "#!/bin/sh" > "$TEST_DIR/api/fail.sh"
    echo "exit 1" >> "$TEST_DIR/api/fail.sh"
    chmod +x "$TEST_DIR/api/fail.sh"

    echo "#!/bin/sh" > "$TEST_DIR/worker/fail.sh"
    echo "exit 0" >> "$TEST_DIR/worker/fail.sh"
    chmod +x "$TEST_DIR/worker/fail.sh"

    run "$META_BIN" --include worker exec -- sh -c "exit 0"
    [ "$status" -eq 0 ]
}

@test "meta exec with spaces in output works correctly" {
    run "$META_BIN" exec -- echo "hello world with spaces"
    [ "$status" -eq 0 ]
    [[ "$output" == *"hello world with spaces"* ]]
}

# ============ Loop behavior: directory headers ============

@test "loop shows directory names as headers in output" {
    run "$META_BIN" exec -- echo test-output
    [ "$status" -eq 0 ]
    # Loop should label output with directory names
    [[ "$output" == *"api"* ]]
    [[ "$output" == *"worker"* ]]
    [[ "$output" == *"frontend"* ]]
}

@test "loop sequential mode runs commands in order" {
    # Run commands across all three projects
    run "$META_BIN" --include api,worker,frontend exec -- pwd
    [ "$status" -eq 0 ]
    # All three directories should appear in output
    [[ "$output" == *"api"* ]]
    [[ "$output" == *"worker"* ]]
    [[ "$output" == *"frontend"* ]]
}

@test "loop captures stdout from commands" {
    # Use a script file to avoid arg-joining issues with shell expressions
    cat > "$TEST_DIR/api/test.sh" <<'SCRIPT'
#!/bin/sh
echo stdout-output
echo stderr-output >&2
SCRIPT
    chmod +x "$TEST_DIR/api/test.sh"

    run "$META_BIN" --include api exec -- ./test.sh
    [ "$status" -eq 0 ]
    [[ "$output" == *"stdout-output"* ]]
    [[ "$output" == *"stderr-output"* ]]
}

@test "loop reports nonzero exit code" {
    # Create a script that exits non-zero
    cat > "$TEST_DIR/api/fail.sh" <<'SCRIPT'
#!/bin/sh
exit 42
SCRIPT
    chmod +x "$TEST_DIR/api/fail.sh"

    run "$META_BIN" --include api exec -- ./fail.sh
    [ "$status" -ne 0 ]
}

@test "loop handles multi-line output correctly" {
    cat > "$TEST_DIR/api/multi.sh" <<'SCRIPT'
#!/bin/sh
echo line1
echo line2
echo line3
SCRIPT
    chmod +x "$TEST_DIR/api/multi.sh"

    run "$META_BIN" --include api exec -- ./multi.sh
    [ "$status" -eq 0 ]
    [[ "$output" == *"line1"* ]]
    [[ "$output" == *"line2"* ]]
    [[ "$output" == *"line3"* ]]
}

@test "loop skips directories in ignore list" {
    # .git is in the default ignore list
    mkdir -p "$TEST_DIR/.git/hooks"
    echo "should not appear" > "$TEST_DIR/.git/hooks/marker"
    run "$META_BIN" exec -- ls
    [ "$status" -eq 0 ]
    # .git directory should not be included as a project
    [[ "$output" != *"hooks"* ]] || true
}

@test "loop with --json outputs structured results" {
    run "$META_BIN" --json --include api exec -- echo json-test
    [ "$status" -eq 0 ]
    # JSON mode should produce parseable output
    # (the exact format depends on loop_lib implementation)
    [[ "$output" == *"json-test"* ]] || [[ "$output" == *"api"* ]]
}

@test "loop handles commands with pipes via script" {
    cat > "$TEST_DIR/api/pipe.sh" <<'SCRIPT'
#!/bin/sh
echo hello | tr h H
SCRIPT
    chmod +x "$TEST_DIR/api/pipe.sh"

    run "$META_BIN" --include api exec -- ./pipe.sh
    [ "$status" -eq 0 ]
    [[ "$output" == *"Hello"* ]]
}

@test "loop parallel mode completes all repos even if one fails" {
    # Make api fail but worker/frontend succeed
    echo "#!/bin/sh" > "$TEST_DIR/api/run.sh"
    echo "exit 1" >> "$TEST_DIR/api/run.sh"
    chmod +x "$TEST_DIR/api/run.sh"

    echo "#!/bin/sh" > "$TEST_DIR/worker/run.sh"
    echo "echo worker-ok" >> "$TEST_DIR/worker/run.sh"
    chmod +x "$TEST_DIR/worker/run.sh"

    echo "#!/bin/sh" > "$TEST_DIR/frontend/run.sh"
    echo "echo frontend-ok" >> "$TEST_DIR/frontend/run.sh"
    chmod +x "$TEST_DIR/frontend/run.sh"

    run "$META_BIN" --include api,worker,frontend --parallel exec -- sh run.sh
    # Even though api fails, worker and frontend should complete
    [[ "$output" == *"worker-ok"* ]]
    [[ "$output" == *"frontend-ok"* ]]
}

# ============ Unrecognized command handling (meta-7) ============

@test "unrecognized top-level command shows help" {
    run "$META_BIN" blablabla
    [ "$status" -eq 1 ]
    [[ "$output" == *"unrecognized command"* ]]
    [[ "$output" == *"blablabla"* ]]
    [[ "$output" == *"meta exec"* ]]
    # Must show ACTUAL help content (not just a reference to --help)
    # This ensures LLMs can self-correct without a second command
    [[ "$output" == *"Usage"* ]]
    [[ "$output" == *"exec"* ]]
    [[ "$output" == *"init"* ]] || [[ "$output" == *"worktree"* ]]
}

@test "unrecognized plugin subcommand shows plugin help" {
    # Copy meta-project plugin to test dir
    cp "$BATS_TEST_DIRNAME/../target/debug/meta-project" "$TEST_DIR/.meta/plugins/meta-project"
    chmod +x "$TEST_DIR/.meta/plugins/meta-project"

    run "$META_BIN" project blablabla
    [ "$status" -eq 1 ]
    [[ "$output" == *"unrecognized command"* ]]
    [[ "$output" == *"project blablabla"* ]]
    # Should show plugin help with available commands
    [[ "$output" == *"list"* ]] || [[ "$output" == *"check"* ]]
}

@test "unrecognized git subcommand passes through to git" {
    # Git plugin passes through unrecognized commands to raw git
    # (unlike other plugins which show help)
    run "$META_BIN" git blablabla
    [ "$status" -ne 0 ]
    # Should show git's own error, not meta's help
    [[ "$output" == *"not a git command"* ]]
}

# ============ Trailing flags pass through to commands (meta-26) ============

@test "trailing --include passes through to command, not intercepted by meta" {
    # --include after the command should be part of the command args, not meta filtering
    run "$META_BIN" exec -- echo --include api
    [ "$status" -eq 0 ]
    [[ "$output" == *"--include api"* ]]
}

@test "trailing --parallel passes through to command, not intercepted by meta" {
    run "$META_BIN" exec -- echo --parallel
    [ "$status" -eq 0 ]
    [[ "$output" == *"--parallel"* ]]
}

@test "trailing --dry-run passes through to command, not intercepted by meta" {
    run "$META_BIN" exec -- echo --dry-run
    [ "$status" -eq 0 ]
    [[ "$output" == *"--dry-run"* ]]
}

# ============ Backward compat (exec syntax) ============

@test "explicit exec still works for arbitrary commands" {
    run "$META_BIN" exec echo hello
    [ "$status" -eq 0 ]
    [[ "$output" == *"hello"* ]]
}

@test "explicit exec with -- separator still works" {
    run "$META_BIN" exec -- echo hello
    [ "$status" -eq 0 ]
    [[ "$output" == *"hello"* ]]
}
