#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# Integration tests for `meta exec` and loop pass-through options
# Tests: exec, --include, --exclude, --parallel, --dry-run, --tag

setup() {
    sandbox
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
projects:
  api: {repo: git@github.com:org/api.git, tags: [backend, rust]}
  worker: {repo: git@github.com:org/worker.git, tags: [backend, rust]}
  frontend: {repo: git@github.com:org/frontend.git, tags: [ui, typescript]}
EOF
    # projects are repos of their own (meta runs nothing in a plain folder)
    make_workspace --init-only --plugins git api worker frontend
    cd "$TEST_DIR"
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
    [[ "$output" == *"$TEST_DIR/api"* ]]
    [[ "$output" == *"$TEST_DIR/worker"* ]]
    [[ "$output" == *"$TEST_DIR/frontend"* ]]
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
    [ "$status" -eq 1 ]
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

@test "meta --tag never selects the root, and a tag no project has is an error" {
    run "$META_BIN" --tag ui exec -- touch tagged
    [ "$status" -eq 0 ]
    [ -f "$TEST_DIR/frontend/tagged" ]
    [ ! -e "$TEST_DIR/tagged" ]
    # -i . adds the root to what --tag selects
    run "$META_BIN" --include . --tag ui exec -- touch both
    [ "$status" -eq 0 ]
    [ -f "$TEST_DIR/both" ] && [ -f "$TEST_DIR/frontend/both" ]
    [ ! -e "$TEST_DIR/api/both" ]

    run "$META_BIN" --tag nonexistent exec -- touch typo
    [ "$status" -ne 0 ]
    [[ "$output" == *"--tag 'nonexistent' is no project's tag"* ]]
    [[ "$output" == *"the tags are backend, rust, typescript, ui"* ]]
    [[ "$output" == *".meta.yaml"* ]]
    [ -z "$(find "$TEST_DIR" -name typo)" ]
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
    run "$META_BIN" --tag ui --dry-run exec -- touch ran
    [ "$status" -eq 0 ]
    [[ "$output" == *"touch ran"* ]]
    [ ! -e "$TEST_DIR/frontend/ran" ]
}

# ============ meta git pass-through (loop fallback) ============

@test "meta git branch runs across repos via loop" {
    for repo in api worker frontend; do
        git -C "$TEST_DIR/$repo" add README.md
        git -C "$TEST_DIR/$repo" commit --quiet -m "init"
    done

    run "$META_BIN" --include api,worker,frontend exec -- git branch
    [ "$status" -eq 0 ]
    [ "$(grep -c '^\* main$' <<<"$output")" -eq 3 ]
}

@test "meta exec -- git log --oneline works" {
    for repo in api worker frontend; do
        git -C "$TEST_DIR/$repo" add README.md
        git -C "$TEST_DIR/$repo" commit --quiet -m "init $repo"
    done

    run "$META_BIN" --include api exec -- git log --oneline
    [ "$status" -eq 0 ]
    [[ "$output" == *"init api"* ]]
}

# ============ Edge cases ============

@test "meta exec with no command after -- is a usage error" {
    run "$META_BIN" exec --
    [ "$status" -eq 2 ]
    [[ "$output" == *"Usage: meta exec"* ]]
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

@test "each repo's name comes before its output, and a sequential prompt reads the terminal's input" {
    for mode in --parallel --sequential; do
        run "$META_BIN" "$mode" --include api,worker exec -- sh -c 'echo "out-of-$(basename "$PWD")"'
        [ "$status" -eq 0 ]
        api_label=$(grep -n '^.*\./api:$' <<<"$output" | cut -d: -f1)
        api_out=$(grep -n '^out-of-api$' <<<"$output" | cut -d: -f1)
        worker_label=$(grep -n '^.*\./worker:$' <<<"$output" | cut -d: -f1)
        [ -n "$api_label" ] && [ -n "$api_out" ] && [ -n "$worker_label" ]
        [ "$api_label" -lt "$api_out" ]
        [ "$api_out" -lt "$worker_label" ]
    done
    run bash -c 'printf "y\nn\n" | "$1" --sequential --include api,worker exec -- sh -c "read a; echo answer=\$a"' _ "$META_BIN"
    [ "$status" -eq 0 ]
    [[ "$output" == *"./api:"*"answer=y"*"./worker:"*"answer=n"* ]]
}

@test "a failed command's summary is the whole report, and meta exits 1" {
    for mode in --parallel --sequential; do
        run "$META_BIN" "$mode" --include api exec -- sh -c 'exit 3'
        [ "$status" -eq 1 ]
        [[ "$output" == *"1 out of 1 commands failed"* ]]
        [[ "$output" != *"Error:"* ]]
    done
    # silenced, there is no summary: the error says it
    run "$META_BIN" --silent --include api exec -- sh -c 'exit 3'
    [ "$status" -eq 1 ]
    [ "$output" = "Error: 1 command failed: api" ]
}

@test "a failure every repo shares prints once; different failures print per repo" {
    bats_require_minimum_version 1.5.0
    run "$META_BIN" --parallel exec -- sh -c 'echo shared failure >&2; exit 3'
    [ "$status" -eq 1 ]
    [ "$(grep -cx 'shared failure' <<<"$output")" -eq 1 ]
    [[ "$output" == *"4 out of 4 commands failed"* ]]
    [[ "$output" == *"./api, ./frontend, ./worker: sh -c"*"(exit code 3)"* ]]

    run "$META_BIN" --parallel exec -- sh -c 'echo "failed in ${PWD##*/}" >&2; exit 3'
    [ "$status" -eq 1 ]
    for repo in api worker frontend; do
        [[ "$output" == *"failed in $repo"* ]]
        [[ "$output" == *"✗ ./$repo: sh -c"* ]]
    done

    # JSON keeps every repo's record
    run --separate-stderr "$META_BIN" --json --parallel exec -- sh -c 'echo shared failure >&2; exit 3'
    jq -e '[.results[] | select(.error and .stderr == "shared failure\n")] | length == 4' <<<"$output"
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

@test "a command a signal ends has exit_code null and the signal in --json" {
    # stdout alone: the JSON
    run sh -c '"$1" --json --include api exec -- sh -c "kill -9 \$\$" 2>/dev/null' _ "$META_BIN"
    [ "$status" -ne 0 ]
    echo "$output" | jq -e '.results[0] | .exit_code == null and .signal == 9'
    # one that exits has its code and no signal
    run sh -c '"$1" --json --include api exec -- sh -c "exit 3" 2>/dev/null' _ "$META_BIN"
    echo "$output" | jq -e '.results[0] | .exit_code == 3 and (has("signal") | not)'
    # one that never started has no exit code or signal, and fails
    run sh -c '"$1" --json --include api exec -- meta-no-such-program 2>/dev/null' _ "$META_BIN"
    echo "$output" | jq -e '.results[0] | (has("exit_code") or has("signal") | not) and .error.code == "tool-failed"'
}

@test "meta exec expands ./.looprc aliases over ~/.looprc ones" {
    echo '{"aliases":{"meta-say":"echo GLOBAL","only-global":"echo G2"}}' > "$HOME/.looprc"
    echo '{"aliases":{"meta-say":"echo LOCAL"}}' > .looprc
    run "$META_BIN" --include api exec -- meta-say
    [ "$status" -eq 0 ]
    [[ "$output" == *LOCAL* ]]
    [[ "$output" != *GLOBAL* ]]
    run "$META_BIN" --include api exec -- only-global
    [ "$status" -eq 0 ]
    [[ "$output" == *G2* ]]
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
    [ "$status" -eq 1 ]
    # Even though api fails, worker and frontend should complete
    [[ "$output" == *"worker-ok"* ]]
    [[ "$output" == *"frontend-ok"* ]]
}

# ============ Unrecognized command handling (meta-7) ============

@test "unrecognized top-level command names the line to run and where the commands are" {
    run "$META_BIN" blablabla
    [ "$status" -eq 2 ]
    [[ "$output" == *"unrecognized command"* ]]
    [[ "$output" == *"blablabla"* ]]
    [[ "$output" == *"meta exec -- blablabla"* ]]
    # meta --help, which lists plugins' commands too, not a copy of meta's
    # own help without them
    [[ "$output" == *"meta --help"* ]]
    [[ "$output" != *"Usage"* ]]
}

@test "an unrecognized project subcommand shows project's commands" {
    run "$META_BIN" project blablabla
    [ "$status" -eq 2 ]
    [[ "$output" == *"unrecognized command"* ]]
    [[ "$output" == *"project blablabla"* ]]
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

# mkrepo DIR...: each a git repository of its own, as a project folder must be
mkrepo() {
    for dir in "$@"; do
        mkdir -p "$dir"
        git -C "$dir" init --quiet
    done
}

add_api_legacy() {
    mkrepo "$TEST_DIR/api-legacy" "$TEST_DIR/libs/core"
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
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

@test "--include takes a repo's .meta.yaml name or its path from the root" {
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
    mkrepo "$TEST_DIR/staging/api"
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
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
{ "projects": { "web": { "repo": "git@github.com:org/web.git", "path": "frontend" }, "web-app": { "repo": "git@github.com:org/web-app.git", "path": "web" } } }
EOF
    run "$META_BIN" --include web exec -- touch ran
    [ "$status" -ne 0 ]
    [[ "$output" == *"names the repo at web by path and the repo at frontend by .meta.yaml name"* ]]
    [ -z "$(find "$TEST_DIR" -name ran)" ]
}

@test "meta git clone refuses --include" {
    run "$META_BIN" --include api git clone https://example.invalid/meta.git
    [ "$status" -ne 0 ]
    [[ "$output" == *"does not take --include/--exclude"* ]]
}

@test "meta-mcp meta_exec runs its argv with no shell" {
    run env PATH="$(dirname "$META_BIN"):$PATH" "$META_BIN_DIR/meta-mcp" <<'EOF'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"meta_exec","arguments":{"command":["printf","[%s]","one arg","$(touch PWNED5)"]}}}
EOF
    [[ "$output" == *'[one arg][$(touch PWNED5)]'* ]]
    [ -z "$(find "$TEST_DIR" -name PWNED5)" ]
}

@test "--recursive with --config expands each nested meta repo once" {
    mkrepo "$TEST_DIR/child" "$TEST_DIR/child/inner" "$TEST_DIR/legacy"
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
{ "projects": { "child": "git@github.com:org/child.git", "legacy": "git@github.com:org/legacy.git" } }
EOF
    cat > "$TEST_DIR/child/.meta.yaml" <<'EOF'
{ "projects": { "inner": "git@github.com:org/inner.git" } }
EOF
    # pwd prints the physical path (macOS temp dirs sit behind a symlink);
    # the nested project is where expanding twice would show
    legacy="$(cd "$TEST_DIR/child/inner" && pwd -P)"
    run "$META_BIN" --recursive exec -- pwd
    [ "$status" -eq 0 ]
    default_runs="$(grep -cx "$legacy" <<<"$output" || true)"
    run "$META_BIN" --config .meta.yaml --recursive exec -- pwd
    [ "$status" -eq 0 ]
    [ "$(grep -cx "$legacy" <<<"$output" || true)" -eq "$default_runs" ]
    [ "$default_runs" -eq 1 ]
}

@test "--recursive stops at a refused nested config, with or without --config" {
    mkdir -p "$TEST_DIR/child" "$TEST_DIR/legacy"
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
{ "projects": { "child": "git@github.com:org/child.git", "legacy": "git@github.com:org/legacy.git" } }
EOF
    cat > "$TEST_DIR/child/.meta.yaml" <<'EOF'
{ "projects": { "up": { "repo": "git@github.com:org/up.git", "path": ".git/up" } } }
EOF
    run "$META_BIN" --recursive exec -- touch ran
    [ "$status" -ne 0 ]
    [[ "$output" == *"leads into .git"* ]]
    run "$META_BIN" --config .meta.yaml --recursive exec -- touch ran
    [ "$status" -ne 0 ]
    [[ "$output" == *"leads into .git"* ]]
    [ -z "$(find "$TEST_DIR" -name ran)" ]
}

@test "--exclude cannot be dodged by a second spelling of the same repo" {
    mkdir -p "$TEST_DIR/api"
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
{ "projects": { "api": "git@github.com:org/api.git", "other": { "repo": "git@github.com:org/api.git", "path": "./api/" } } }
EOF
    run "$META_BIN" --exclude api exec -- touch ran
    [ "$status" -ne 0 ]
    [[ "$output" == *"project 'other' (path 'api') is at or inside project 'api'"* ]]
    [ -z "$(find "$TEST_DIR" -name ran)" ]
}

@test "exec keeps each command's stdout on stdout and stderr on stderr, bytes as written" {
    for mode in --parallel --sequential; do
        run bash -c '"$1" "$2" --include api exec -- sh -c "echo OUT; echo ERR >&2" 2>/dev/null' _ "$META_BIN" "$mode"
        [ "$status" -eq 0 ]
        [[ "$output" == *"OUT"* ]]
        [[ "$output" != *"ERR"* ]]
        run bash -c '"$1" "$2" --include api exec -- sh -c "echo ERR >&2" 2>&1 >/dev/null' _ "$META_BIN" "$mode"
        [[ "$output" == *"ERR"* ]]
        # not UTF-8: passed on as it is
        run bash -c '"$1" "$2" --include api exec -- printf "a\377b" | od -An -c' _ "$META_BIN" "$mode"
        [[ "$output" == *"a 377   b"* ]]
    done
}

@test "exec stdout broken pipe exits quietly without a panic" {
    for mode in --parallel --sequential; do
        run bash -c 'set -o pipefail; "$1" "$2" exec -- seq 1 20000 2>"$3" | head -1' _ "$META_BIN" "$mode" "$TEST_DIR/stderr"
        [[ "$status" -eq 0 || "$status" -eq 141 ]]
        [ ! -s "$TEST_DIR/stderr" ]
    done
}

@test "meta's own output to a closed stdout exits quietly without a panic" {
    for args in "--json plugin list" "init --help" "--help"; do
        run bash -c 'set -o pipefail; "$1" $2 2>"$3" | head -c0' _ "$META_BIN" "$args" "$TEST_DIR/stderr"
        [[ "$status" -eq 0 || "$status" -eq 141 ]]
        [ ! -s "$TEST_DIR/stderr" ]
    done
}

@test "loop with no command prints clap's help: coloured on a terminal, plain into a pipe" {
    load "${BATS_TEST_DIRNAME}/helpers/terminal.bash"
    LOOP_BIN="$META_BIN_DIR/loop"
    TERM=xterm run with_terminal "$LOOP_BIN"
    [[ "$output" == *$'\033['* ]]
    run bash -c '"$1" | od -An -c | tail -1' _ "$LOOP_BIN"
    [[ "$output" != *'\n  \n'* ]]
    run "$LOOP_BIN"
    [[ "$output" == *"Usage: loop"* ]]
    [[ "$output" != *$'\033['* ]]
}

@test "loop's output to a closed stdout exits 141 quietly" {
    LOOP_BIN="$META_BIN_DIR/loop"
    # stdout's read end closed before loop starts (not `head -c0`, which
    # BSD head refuses)
    closed() { python3 -c 'import os, subprocess, sys
r, w = os.pipe(); os.close(r)
p = subprocess.run(sys.argv[1:], stdout=w, stderr=subprocess.PIPE)
sys.stderr.buffer.write(p.stderr); sys.exit(p.returncode)' "$@"; }
    for args in "-i api echo hi" "--help"; do
        run closed "$LOOP_BIN" $args
        [ "$status" -eq 141 ]
        [ -z "$output" ]
    done
    # -v's own lines are on stderr; nothing else is
    run closed "$LOOP_BIN" -v -i api echo hi
    [ "$status" -eq 141 ]
    [[ "$output" != *"rror"* && "$output" != *"panicked"* ]]
}

@test "a failure cut off by a closed stdout isn't reported as success" {
    run bash -c 'set -o pipefail; "$1" exec -- sh -c "seq 1 200000; exit 3" 2>"$2" | head -1' _ "$META_BIN" "$TEST_DIR/stderr"
    [ "$status" -eq 141 ]
    [ ! -s "$TEST_DIR/stderr" ]
}

@test "plugin output to a closed stdout exits quietly without a panic" {
    mkdir -p .meta/plugins
    cat > .meta/plugins/meta-chatty <<'EOF'
#!/bin/sh
if [ "$1" = --meta-plugin-info ]; then
    echo '{"name":"chatty","version":"1","commands":["chatty"]}'
else
    seq 1 200000
fi
EOF
    chmod +x .meta/plugins/meta-chatty
    for args in chatty "chatty go"; do
        run bash -c 'set -o pipefail; "$1" $2 2>"$3" | head -1' _ "$META_BIN" "$args" "$TEST_DIR/stderr"
        [[ "$status" -eq 0 || "$status" -eq 141 ]]
        [ ! -s "$TEST_DIR/stderr" ]
    done
}

@test "a plugin plan under --json --verbose leaves stdout the envelope alone" {
    mkdir -p .meta/plugins
    printf '%s\n' '#!/bin/sh' \
        'if [ "$1" = --meta-plugin-info ]; then' \
        '    echo '"'"'{"name":"plan","version":"1","commands":["plan"]}'"'"'' \
        'else' \
        '    echo '"'"'{"plan":{"steps":[{"dir":".","argv":["echo","from-step"]}]}}'"'"' > "$META_RESPONSE"' \
        'fi' > .meta/plugins/meta-plan
    chmod +x .meta/plugins/meta-plan
    run bash -c '"$1" --json --verbose plan go 2>/dev/null' _ "$META_BIN"
    [ "$status" -eq 0 ]
    jq -e '.success == true and .results[0].stdout == "from-step\n"' <<<"$output"
    [ "$(jq -s length <<<"$output")" -eq 1 ]
}

@test "a plugin killed by a signal ends meta by it; one that fails silently under --json leaves JSON" {
    mkdir -p .meta/plugins
    cat > .meta/plugins/meta-die <<'EOF'
#!/bin/sh
if [ "$1" = --meta-plugin-info ]; then
    echo '{"name":"die","version":"1","commands":["die","die quiet"]}'
elif grep -q quiet "$META_REQUEST"; then
    echo boom >&2; exit 3
else
    kill -TERM $$
fi
EOF
    chmod +x .meta/plugins/meta-die
    run "$META_BIN" die now
    [ "$status" -eq 143 ]
    run bash -c '"$1" --json die quiet 2>/dev/null' _ "$META_BIN"
    [ "$status" -eq 3 ]
    jq -e '.success == false and .error.code == "plugin-failed"' <<<"$output"
}

@test "a plugin's own argument that isn't UTF-8 is a usage error, not a panic" {
    run "$META_GIT_BIN" $'\xff'
    [ "$status" -eq 2 ]
    [[ "$output" == *"Unknown flag"*"Use via: meta git"* ]]
    [[ "$output" != *"panicked"* ]]
    run "$META_GIT_BIN" --bogus
    [ "$status" -eq 2 ]
    [[ "$output" == *"Unknown flag: --bogus"*"Use via: meta git"* ]]
}

@test "an error meant for a closed stdout under --json exits 141 quietly" {
    closed() { python3 -c 'import os, subprocess, sys
r, w = os.pipe(); os.close(r)
p = subprocess.run(sys.argv[1:], stdout=w, stderr=subprocess.PIPE)
sys.stderr.buffer.write(p.stderr); sys.exit(p.returncode)' "$@"; }
    run closed "$META_BIN" --json nosuchcmd
    [ "$status" -eq 141 ]
    [ -z "$output" ]
    run closed "$META_BIN" --json --include nope exec -- true
    [ "$status" -eq 141 ]
    [ -z "$output" ]
}

@test "a last line without its newline to a closed stdout still exits 141" {
    mkdir -p .meta/plugins
    printf '%s\n' '#!/bin/sh' \
        'if [ "$1" = --meta-plugin-info ]; then' \
        '    echo '"'"'{"name":"frag","version":"1","commands":["frag"]}'"'"'' \
        'elif grep -q fail "$META_REQUEST"; then printf fragment; exit 3' \
        'else printf fragment; fi' > .meta/plugins/meta-frag
    chmod +x .meta/plugins/meta-frag
    # whether the plugin succeeded or failed
    for how in now fail; do
        run python3 -c 'import os, subprocess, sys
r, w = os.pipe(); os.close(r)
sys.exit(subprocess.run(sys.argv[1:], stdout=w).returncode)' "$META_BIN" frag "$how"
        [ "$status" -eq 141 ]
    done
}

@test "meta's --json makes a plugin command's errors JSON; a .looprc's json_output keeps colour out" {
    run bash -c '"$1" --json git worktree status missing 2>/dev/null' _ "$META_BIN"
    [ "$status" -eq 1 ]
    jq -e '.success == false' <<<"$output"
    echo '{"json_output": true}' > .looprc
    # loop's --verbose lines go to stderr, so stdout stays JSON
    run bash -c '"$1" -v -i api true 2>/dev/null' _ "$META_BIN_DIR/loop"
    jq -e '.success == true' <<<"$output"
    # nor in -v's lines on stderr, printed after the .looprc is read
    CLICOLOR_FORCE=1 run bash -c '"$1" -v -i api true 2>&1 >/dev/null' _ "$META_BIN_DIR/loop"
    [[ "$output" == *"Verbose mode enabled"* ]]
    [[ "$output" != *$'\033['* ]]
    CLICOLOR_FORCE=1 run "$META_BIN_DIR/loop" -i api sh -c 'if [ -n "$CLICOLOR_FORCE" ]; then echo forced; else echo plain; fi'
    jq -e '.results[0].stdout == "plain\n"' <<<"$output"
}

@test "a hostile command name is escaped in meta's own lines" {
    E=$'bad\033[2J\nFORGED'
    for mode in --parallel --sequential --dry-run; do
        run "$META_BIN" "$mode" exec -- "$E"
        [[ "$output" != *$'\033[2J'* ]]
        [[ "$output" != *$'\nFORGED'* ]]
        [[ "$output" == *'bad\u{1b}[2J\nFORGED'* ]]
    done
    # and in meta's own --verbose echo of the command
    run "$META_BIN" --verbose exec -- "$E"
    [[ "$output" == *"Executing command:"*'bad\u{1b}[2J\nFORGED'* ]]
    [[ "$output" != *$'\033[2J'* ]]
    [[ "$output" != *$'\nFORGED'* ]]
    # and in the line for a listed project that isn't cloned
    mkdir "$TEST_DIR/ghostws"
    cd "$TEST_DIR/ghostws"
    echo '{"projects": {"ghost": "git@x:o/ghost.git"}}' > .meta.yaml
    run "$META_BIN" --sequential exec -- "$E"
    [[ "$output" == *"ghost: not cloned"* ]]
    [[ "$output" != *$'\033[2J'* ]]
    [[ "$output" != *$'\nFORGED'* ]]
}

@test "include and exclude errors escape controls and stay on one line" {
    for flag in --include --exclude; do
        run "$META_BIN" "$flag" $'bad\033[31m\nFORGED' exec -- echo hello
        [ "$status" -ne 0 ]
        [[ "$output" != *$'\033'* ]]
        [[ "$output" != *$'\n'* ]]
        [[ "$output" == *'bad\u{1b}[31m\nFORGED'* ]]
    done
}

# ============ One selection ============

@test "-r --tag selects a nested project by its own tag under an untagged parent" {
    mkrepo "$TEST_DIR/tools" "$TEST_DIR/tools/cli"
    printf 'projects:\n  cli:\n    repo: git@github.com:org/cli.git\n    tags: [cli-tools]\n' > "$TEST_DIR/tools/.meta.yaml"
    cat > "$TEST_DIR/.meta.yaml" <<'YAML'
projects:
  api:
    repo: git@github.com:org/api.git
    tags: [backend]
  tools:
    repo: git@github.com:org/tools.git
    meta: true
YAML
    run "$META_BIN" -r -t cli-tools exec -- touch picked
    [ "$status" -eq 0 ]
    [ -f "$TEST_DIR/tools/cli/picked" ]
    [ -z "$(find "$TEST_DIR" -name picked -not -path '*/tools/cli/*')" ]
    # without -r, the tag isn't one of the workspace's own projects
    run "$META_BIN" -t cli-tools exec -- true
    [ "$status" -ne 0 ]
    [[ "$output" == *"--recursive reads the tags of nested meta repos too"* ]]
}

@test "a filter error names the .meta.yaml it read" {
    mkdir -p "$TEST_DIR/infra/tf"
    printf 'projects:\n  tf: git@github.com:org/tf.git\n' > "$TEST_DIR/infra/.meta.yaml"
    cd "$TEST_DIR/infra"
    run "$META_BIN" --include api exec -- true
    [ "$status" -ne 0 ]
    [[ "$output" == *"--include 'api' names none of the repos"*"$TEST_DIR/infra/.meta.yaml"* ]]
}

@test "meta's options go before the command; after it they belong to the command" {
    # misplaced before the program: refused by name, nothing runs
    run "$META_BIN" exec --dry-run -- touch should-not-exist
    [ "$status" -ne 0 ]
    [[ "$output" == *"--dry-run is an option of meta itself: it goes before the command"* ]]
    [ -z "$(find "$TEST_DIR" -name should-not-exist)" ]
    # after the program: its arguments, in every repo (README's old example)
    run "$META_BIN" exec echo test --tag backend
    [ "$status" -eq 0 ]
    [ "$(grep -c 'test --tag backend' <<<"$output")" -eq 4 ]
    # a program whose name starts with - still runs after --
    run "$META_BIN" exec -- -no-such-program
    [ "$status" -eq 1 ]
    [[ "$output" != *"option of meta itself"* ]]
    [[ "$output" == *"-no-such-program (didn't start)"* ]]
}

@test "json usage errors are one JSON document" {
    bats_require_minimum_version 1.5.0
    # meta's own parse error, with the line to run
    run --separate-stderr "$META_BIN" --json exec --include api -- pwd
    [ "$status" -eq 2 ]
    [ "$(jq -r .success <<<"$output")" = false ]
    [[ "$(jq -r .error.hint <<<"$output")" == *"as in meta --json --include api exec -- pwd"* ]]
    [ "$(jq -s length <<<"$output")" -eq 1 ]
    # a plugin's, relayed as it printed it
    run --separate-stderr "$META_BIN" --json git update --dry-run
    [ "$status" -eq 2 ]
    [ "$(jq -s length <<<"$output")" -eq 1 ]
    [[ "$(jq -r .error.hint <<<"$output")" == *"as in meta --json --dry-run git update"* ]]
    # --json that isn't meta's, before the command, prints no JSON
    run --separate-stderr "$META_BIN" exec --json-x -- pwd
    [ "$status" -eq 2 ]
    [ -z "$output" ]
    run "$META_BIN" exec -- echo --json --bad
    [ "$status" -eq 0 ]
    [[ "$output" == *"--json --bad"* ]]
    [[ "$output" != *'"success"'* ]]
}

@test "json results name each repo by path and list warnings" {
    bats_require_minimum_version 1.5.0
    rm -rf frontend
    run --separate-stderr "$META_BIN" --json --exclude . git diff --quiet
    [ "$status" -eq 0 ]
    [ "$(jq -r '[.results[].project] | join(" ")' <<<"$output")" = "api worker" ]
    # empty output is "", never a missing key
    [ "$(jq -r '[.results[] | has("stdout") and has("stderr")] | all' <<<"$output")" = true ]
    [ "$(jq -r '.results[0].stdout' <<<"$output")" = "" ]
    [[ "$(jq -r '.warnings[0].message' <<<"$output")" == "frontend is not cloned"* ]]
    [ "$(jq -r '.warnings[0].project' <<<"$output")" = frontend ]
    [ "$(jq -r .error <<<"$output")" = null ]
    # each project name selects its repo as it is
    project="$(jq -r '.results[0].project' <<<"$output")"
    run --separate-stderr "$META_BIN" --json --include "$project" exec -- true
    [ "$(jq -r '[.results[].project] | join(" ")' <<<"$output")" = "api" ]
    # the tip the human output ends with
    run --separate-stderr "$META_BIN" --json --include api git status -r
    [ "$status" -eq 1 ]
    [[ "$(jq -r .error.hint <<<"$output")" == *"as in meta --json --include api -r git status" ]]
}

# Live output reaches the user's terminal, file or pipe as the command
# writes it, through meta (which keeps an excerpt for the summary)

@test "a closed pipe ends a live run at once" {
    # at most 20 seconds, on macOS too (it has no timeout)
    run perl -e 'alarm 20; exec @ARGV' bash -c "\"$META_BIN\" --sequential -i api exec -- sh -c 'while :; do echo out; echo err >&2; done' 2>&1 | head -1"
    [ "$status" -eq 0 ]
}

@test "live stdout and stderr keep their order in one file or pipe" {
    local say='echo one; echo two >&2; echo three; echo four >&2'
    "$META_BIN" --sequential -i api exec -- sh -c "$say" > "$TEST_DIR/to-file" 2>&1
    "$META_BIN" --sequential -i api exec -- sh -c "$say" 2>&1 | cat > "$TEST_DIR/to-pipe"
    for out in to-file to-pipe; do
        [ "$(grep -E '^(one|two|three|four)$' "$TEST_DIR/$out" | tr '\n' ' ')" = "one two three four " ]
    done
}

@test "a slow reader gets all of a live run's stderr" {
    local say='head -c 300000 /dev/zero | tr "\0" x >&2; echo; echo END-MARK >&2'
    "$META_BIN" --sequential -i api exec -- sh -c "$say" 2>&1 | (sleep 1; cat > "$TEST_DIR/slow")
    grep -q 'END-MARK' "$TEST_DIR/slow"
    [ "$(wc -c < "$TEST_DIR/slow")" -ge 300000 ]
}

@test "a background program outlives meta and may still write to stderr" {
    # started by the command; writes once meta has ended
    local late="(sleep 1; echo progress >&2; touch \"\$0\") &"
    wait_for() { for _ in $(seq 100); do [ -e "$1" ] && return 0; sleep 0.1; done; return 1; }
    "$META_BIN" --sequential -i api exec -- sh -c "$late" "$TEST_DIR/live-file" > "$TEST_DIR/live-out" 2>&1
    wait_for "$TEST_DIR/live-file"
    grep -q progress "$TEST_DIR/live-out"
    "$META_BIN" --sequential -i api exec -- sh -c "$late" "$TEST_DIR/live-pipe" 2>&1 | cat > /dev/null
    wait_for "$TEST_DIR/live-pipe"
    for order in --sequential --parallel; do
        "$META_BIN" --silent $order -i api exec -- sh -c "$late" "$TEST_DIR/silent$order-file" > "$TEST_DIR/silent-out" 2>&1
        wait_for "$TEST_DIR/silent$order-file"
        "$META_BIN" --silent $order -i api exec -- sh -c "$late" "$TEST_DIR/silent$order-pipe" 2>&1 | cat > /dev/null
        wait_for "$TEST_DIR/silent$order-pipe"
    done
}

@test "many failures: the first few with their output, one line each after, then every name" {
    printf 'projects:\n' > .meta.yaml
    for i in 1 2 3 4 5 6 7; do printf '  r%s: git@github.com:org/r%s.git\n' "$i" "$i" >> .meta.yaml; done
    make_workspace --init-only --plugins git r1 r2 r3 r4 r5 r6 r7
    cd "$TEST_DIR"
    run "$META_BIN" --parallel exec -- sh -c '[ -d .meta ] && exit 0; echo "first of ${PWD##*/}" >&2; echo "fatal: ${PWD##*/} broke" >&2; exit 3'
    [ "$status" -eq 1 ]
    local summary=${output#*Summary:}
    [[ "$summary" == *"✗ ./r5: sh -c "*" (exit code 3)"$'\n'"    first of r5"$'\n'"    fatal: r5 broke"* ]]
    [[ "$summary" == *"✗ ./r6: sh -c "*" (exit code 3): fatal: r6 broke"$'\n'* ]]
    [[ "$summary" != *"first of r6"* ]]
    [ "$(grep -v '^[[:space:]]*$' <<<"$summary" | tail -1)" = "✗ Failed (7): ./r1, ./r2, ./r3, ./r4, ./r5, ./r6, ./r7" ]
}

@test "a live command on a terminal still sees one, and its output keeps its order" {
    load "${BATS_TEST_DIRNAME}/helpers/terminal.bash"
    # the words are made as it runs: the summary's line shows the command
    local probe='[ -t 1 ] && printf "%s-on-a-terminal\n" out; [ -t 2 ] && printf "%s-on-a-terminal\n" err >&2; echo one; echo two >&2; echo three; exit 3'
    run with_terminal "$META_BIN" --sequential -i api exec -- sh -c "$probe"
    [[ "$output" == *"out-on-a-terminal"* && "$output" == *"err-on-a-terminal"* ]]
    # and the summary still has its reason, in the order it was written
    local summary=${output#*Summary:}
    [[ "$summary" == *"    out-on-a-terminal"*"    err-on-a-terminal"*"    one"*"    two"*"    three"* ]]
    # not on a terminal, it sees none (as without meta)
    run "$META_BIN" --sequential -i api exec -- sh -c "$probe"
    [[ "$output" != *"out-on-a-terminal"* && "$output" != *"err-on-a-terminal"* ]]
}

@test "a parallel failure's excerpt ends with what it wrote last, after a long stdout" {
    for _ in 1 2 3 4 5; do
        run "$META_BIN" --parallel -i api,worker exec -- sh -c 'seq 1 50000; echo "error: build failed in ${PWD##*/}" >&2; exit 2'
        [ "$status" -eq 1 ]
        local summary=${output#*Summary:}
        for repo in api worker; do
            [[ "$summary" == *"    49999"$'\n'"    50000"$'\n'"    error: build failed in $repo"* ]]
        done
    done
}

@test "a second Ctrl-C kills what the commands started too, and still prints the summary" {
    # what a command started is found in /proc
    [ "$(uname)" = Linux ] || skip "Linux only"
    # frontend fails at once; api and worker ignore Ctrl-C, as does the
    # sleep each leaves behind
    cat > "$TEST_DIR/ctrlc.py" <<'PY'
import os, pty, sys, time
pid, fd = pty.fork()
if pid == 0:
    os.execvp(sys.argv[1], sys.argv[1:])
out = b""
import select
def wait_for(path_names, seconds):
    end = time.time() + seconds
    while time.time() < end:
        if all(os.path.exists(p) for p in path_names):
            return
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try:
                out_chunk = os.read(fd, 65536)
            except OSError:
                return
            globals()["out"] += out_chunk
wait_for([os.environ["TEST_DIR"] + "/sleep-" + n for n in ("api", "worker")], 20)
os.write(fd, b"\x03")
time.sleep(1)
os.write(fd, b"\x03")
end = time.time() + 10
while time.time() < end:
    r, _, _ = select.select([fd], [], [], 0.2)
    if not r:
        continue
    try:
        chunk = os.read(fd, 65536)
    except OSError:
        break
    if not chunk:
        break
    out += chunk
os.waitpid(pid, 0)
sys.stdout.buffer.write(out)
PY
    local script='[ "${PWD##*/}" = frontend ] && { echo "fatal: frontend broke" >&2; exit 1; }; trap "" INT; sleep 60 & echo $! > "$TEST_DIR/sleep-${PWD##*/}"; wait; wait'
    TEST_DIR="$TEST_DIR" run python3 "$TEST_DIR/ctrlc.py" "$META_BIN" --parallel -i api,worker,frontend exec -- sh -c "$script"
    [[ "$output" == *"Interrupted: 1 of 3 commands finished, 1 failed"* ]]
    [[ "$output" == *"fatal: frontend broke"* ]]
    [[ "$output" == *"Stopped while running: ./api, ./worker"* || "$output" == *"Stopped while running: ./worker, ./api"* ]]
    # what they left behind is gone too
    sleep 0.5
    for repo in api worker; do
        ! kill -0 "$(cat "$TEST_DIR/sleep-$repo")" 2>/dev/null
    done
}

@test "--full-errors shows each failure's whole output; without it the summary caps it" {
    local long='for i in $(seq 30); do echo "line $i" >&2; done; head -c 1048576 /dev/zero | tr "\0" x >&2; echo END >&2; exit 2'
    for mode in --parallel --sequential; do
        run "$META_BIN" $mode --full-errors -i api exec -- sh -c "$long"
        [ "$status" -eq 1 ]
        # (cut with sed: a pattern on 2 MB is slow in bash)
        printf '%s\n' "$output" | sed -n '/Summary:/,$p' > "$TEST_DIR/summary"
        [ "$(grep -c '^    line [0-9]*$' "$TEST_DIR/summary")" -eq 30 ]
        ! grep -q 'more lines\|more characters' "$TEST_DIR/summary"
        # the megabyte line, whole
        [ "$(grep -c '^    x*END$' "$TEST_DIR/summary")" -eq 1 ]
        [ "$(grep '^    x*END$' "$TEST_DIR/summary" | wc -c)" -eq $((4 + 1048576 + 3 + 1)) ]
        run "$META_BIN" $mode -i api exec -- sh -c "$long"
        # (cut with sed: a pattern on 2 MB is slow in bash)
        printf '%s\n' "$output" | sed -n '/Summary:/,$p' > "$TEST_DIR/summary"
        grep -q '^    … 22 more lines$' "$TEST_DIR/summary"
        grep -q '… 1048279 more characters …' "$TEST_DIR/summary"
        # the first 5 and the last 3, with the long line after them
        [ "$(grep -c '^    line [0-9]*$' "$TEST_DIR/summary")" -eq 8 ]
    done
}
