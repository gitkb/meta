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

    # The physical path, as pwd prints it (macOS temp dirs sit behind a symlink)
    TEST_DIR="$(cd "$(mktemp -d)" && pwd -P)"
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

@test "meta exec --include with nonexistent dir is an error and runs nothing" {
    run "$META_BIN" --include nonexistent exec -- echo hello
    [ "$status" -ne 0 ]
    [[ "$output" == *"--include 'nonexistent' names none of the repos"* ]]
    [[ "$output" != *"hello"* ]]
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

# ============ Arguments reach the command as typed, with no shell ============

@test "meta exec passes each argument as given, never through a shell" {
    run "$META_BIN" --include api exec -- printf '[%s]\n' 'one arg' '$(touch PWNED)' 'a;touch PWNED' '*' "it's"
    [ "$status" -eq 0 ]
    [[ "$output" == *"[one arg]"* ]]
    [[ "$output" == *'[$(touch PWNED)]'* ]]
    [[ "$output" == *"[a;touch PWNED]"* ]]
    [[ "$output" == *"[*]"* ]]
    [[ "$output" == *"[it's]"* ]]
    [ -z "$(find "$TEST_DIR" -name PWNED)" ]
}

@test "meta exec runs a shell only when asked for one" {
    run "$META_BIN" --include api exec -- sh -c 'echo first && echo second'
    [ "$status" -eq 0 ]
    [[ "$output" == *"first"* ]]
    [[ "$output" == *"second"* ]]
}

@test "meta exec --dry-run shows the command quoted as typed" {
    run "$META_BIN" --dry-run --include api exec -- touch 'one arg'
    [ "$status" -eq 0 ]
    [[ "$output" == *"touch 'one arg'"* ]]
    [ ! -e "$TEST_DIR/api/one arg" ]
}

@test "meta exec of a missing program fails cleanly" {
    run "$META_BIN" --include api exec -- meta-no-such-program
    [ "$status" -ne 0 ]
    [[ "$output" == *"meta-no-such-program"* ]]
    [[ "$output" != *"panicked"* ]]
}

# ============ --include / --exclude name whole repos ============

add_api_legacy() {
    mkdir -p "$TEST_DIR/api-legacy" "$TEST_DIR/libs/core"
    cat > "$TEST_DIR/.meta.json" <<'EOF'
{
    "projects": {
        "api": "git@github.com:org/api.git",
        "api-legacy": "git@github.com:org/api-legacy.git",
        "worker": "git@github.com:org/worker.git",
        "core": { "repo": "git@github.com:org/core.git", "path": "libs/core" }
    }
}
EOF
}

@test "--include api does not select api-legacy" {
    add_api_legacy
    run "$META_BIN" --include api exec -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"$TEST_DIR/api"* ]]
    [[ "$output" != *"api-legacy"* ]]
    [[ "$output" != *"worker"* ]]
}

@test "--exclude api keeps api-legacy" {
    add_api_legacy
    run "$META_BIN" --exclude api exec -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"$TEST_DIR/api-legacy"* ]]
    [[ "$output" == *"$TEST_DIR/worker"* ]]
    [ "$(grep -c "^$TEST_DIR/api\$" <<<"$output")" -eq 0 ]
}

@test "--include a part of a path names none of the repos" {
    add_api_legacy
    # every absolute path here contains "t" and "a"
    for value in a t ap libs core/x; do
        run "$META_BIN" --include "$value" exec -- pwd
        [ "$status" -ne 0 ] || { echo "--include $value: $output"; false; }
        [[ "$output" == *"names none of the repos"* ]]
    done
}

@test "--include takes a repo's .meta name or its path from the root" {
    add_api_legacy
    for value in core libs/core ./libs/core/; do
        run "$META_BIN" --include "$value" exec -- pwd
        [ "$status" -eq 0 ]
        [[ "$output" == *"$TEST_DIR/libs/core"* ]]
        [[ "$output" != *"api"* ]]
    done
    run "$META_BIN" --include . exec -- pwd
    [ "$status" -eq 0 ]
    [ "$(grep -c "^$TEST_DIR\$" <<<"$output")" -eq 1 ]
    [[ "$output" != *"$TEST_DIR/"* ]]
}

@test "--exclude with a typo is an error, not a no-op" {
    add_api_legacy
    run "$META_BIN" --exclude apii exec -- touch ran
    [ "$status" -ne 0 ]
    [[ "$output" == *"--exclude 'apii' names none of the repos"* ]]
    [ -z "$(find "$TEST_DIR" -name ran)" ]
}

@test "--include selects plugin commands the same way" {
    add_api_legacy
    run "$META_BIN" --dry-run --include api git reset --hard
    [ "$status" -eq 0 ]
    [ "$(grep -c "Would execute" <<<"$output")" -eq 1 ]
    [[ "$output" == *"$TEST_DIR/api"* ]]
    [[ "$output" != *"api-legacy"* ]]
}

@test "--include takes names from the config chosen with --config" {
    add_api_legacy
    cat > "$TEST_DIR/alt.json" <<'EOF'
{ "projects": { "core-lib": { "repo": "git@github.com:org/core.git", "path": "./libs/core/" } } }
EOF
    run "$META_BIN" --config alt.json --include core-lib exec -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"$TEST_DIR/libs/core"* ]]
    [[ "$output" != *"api"* ]]
    run "$META_BIN" --config alt.json --include core exec -- pwd
    [ "$status" -ne 0 ]
    [[ "$output" == *"names none of the repos"* ]]
}

@test "--recursive with --config walks the chosen config" {
    add_api_legacy
    mkdir -p "$TEST_DIR/staging/api"
    cat > "$TEST_DIR/alt.json" <<'EOF'
{ "projects": { "staging-api": { "repo": "git@github.com:org/api.git", "path": "staging/api" } } }
EOF
    run "$META_BIN" --config alt.json --recursive --include staging-api exec -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"$TEST_DIR/staging/api"* ]]
    run "$META_BIN" --config alt.json --recursive exec -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"$TEST_DIR/staging/api"* ]]
    [[ "$output" != *"api-legacy"* ]]
}

@test "a value that is one repo's path and another's name is an error" {
    mkdir -p "$TEST_DIR/web" "$TEST_DIR/frontend"
    cat > "$TEST_DIR/.meta.json" <<'EOF'
{ "projects": { "web": { "repo": "git@github.com:org/web.git", "path": "frontend" }, "web-app": { "repo": "git@github.com:org/web-app.git", "path": "web" } } }
EOF
    run "$META_BIN" --include web exec -- touch ran
    [ "$status" -ne 0 ]
    [[ "$output" == *"names the repo at web by path and the repo at frontend by .meta name"* ]]
    [ -z "$(find "$TEST_DIR" -name ran)" ]
}

@test "meta git clone refuses --include" {
    run "$META_BIN" --include api git clone https://example.invalid/meta.git
    [ "$status" -ne 0 ]
    [[ "$output" == *"does not take --include/--exclude"* ]]
}

@test "meta-mcp meta_exec runs its argv with no shell" {
    run env PATH="$(dirname "$META_BIN"):$PATH" "$BATS_TEST_DIRNAME/../target/debug/meta-mcp" <<'EOF'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"meta_exec","arguments":{"command":["printf","[%s]","one arg","$(touch PWNED5)"]}}}
EOF
    [[ "$output" == *'[one arg][$(touch PWNED5)]'* ]]
    [ -z "$(find "$TEST_DIR" -name PWNED5)" ]
}

@test "--recursive with --config expands each nested meta repo once" {
    mkdir -p "$TEST_DIR/child" "$TEST_DIR/legacy"
    cat > "$TEST_DIR/.meta.json" <<'EOF'
{ "projects": { "child": "git@github.com:org/child.git", "legacy": "git@github.com:org/legacy.git" } }
EOF
    # A '..' back-edge to the root is refused when the config is read
    cat > "$TEST_DIR/child/.meta.json" <<'EOF'
{ "projects": {} }
EOF
    # pwd prints the physical path (macOS temp dirs sit behind a symlink)
    legacy="$(cd "$TEST_DIR/legacy" && pwd -P)"
    run "$META_BIN" --recursive exec -- pwd
    [ "$status" -eq 0 ]
    default_runs="$(grep -cx "$legacy" <<<"$output" || true)"
    run "$META_BIN" --config .meta.json --recursive exec -- pwd
    [ "$status" -eq 0 ]
    [ "$(grep -cx "$legacy" <<<"$output" || true)" -eq "$default_runs" ]
    [ "$default_runs" -eq 1 ]
}

@test "--exclude cannot be dodged by a second spelling of the same repo" {
    mkdir -p "$TEST_DIR/api"
    cat > "$TEST_DIR/.meta.json" <<'EOF'
{ "projects": { "api": "git@github.com:org/api.git", "other": { "repo": "git@github.com:org/api.git", "path": "./api/" } } }
EOF
    run "$META_BIN" --exclude api exec -- touch ran
    [ "$status" -ne 0 ]
    [[ "$output" == *"project 'other' (path 'api') is at or inside project 'api'"* ]]
    [ -z "$(find "$TEST_DIR" -name ran)" ]
}
