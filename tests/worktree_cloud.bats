#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# Integration tests for `meta git worktree` cloud/agent extensions (meta-6)
# Tests: --meta, --ephemeral, --ttl, <commit-ish>, prune, lifecycle hooks,
#         context detection, ephemeral exec, per-worktree records

setup() {
    clear_git_local_env
    isolate_user_env
    META_BIN="$BATS_TEST_DIRNAME/../target/debug/meta"
    META_GIT_BIN="$BATS_TEST_DIRNAME/../target/debug/meta-git"

    if [ ! -f "$META_BIN" ] || [ ! -f "$META_GIT_BIN" ]; then
        cargo build --workspace --quiet
    fi

    TEST_DIR="$(cd "$(mktemp -d)" && pwd -P)"
    mkdir -p "$TEST_DIR/.meta/plugins"
    cp "$META_GIT_BIN" "$TEST_DIR/.meta/plugins/meta-git"
    chmod +x "$TEST_DIR/.meta/plugins/meta-git"
    META_DATA="$(cd "$(mktemp -d)" && pwd -P)"
    export META_DATA_DIR="$META_DATA"

    # Create .meta.yaml with two projects
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
{
    "projects": {
        "backend": "git@github.com:org/backend.git",
        "frontend": "git@github.com:org/frontend.git"
    }
}
EOF

    # Initialize git repos with at least one commit
    for repo in backend frontend; do
        mkdir -p "$TEST_DIR/$repo"
        git -C "$TEST_DIR/$repo" init --quiet
        git -C "$TEST_DIR/$repo" config user.email "test@test.com"
        git -C "$TEST_DIR/$repo" config user.name "Test"
        echo "# $repo" > "$TEST_DIR/$repo/README.md"
        git -C "$TEST_DIR/$repo" add README.md
        git -C "$TEST_DIR/$repo" commit --quiet -m "init $repo"
    done

    # Initialize meta repo
    git -C "$TEST_DIR" init --quiet
    git -C "$TEST_DIR" config user.email "test@test.com"
    git -C "$TEST_DIR" config user.name "Test"
    git -C "$TEST_DIR" add .meta.yaml
    git -C "$TEST_DIR" commit --quiet -m "init meta"

    cd "$TEST_DIR"
}

# The file where meta records the set a linked worktree checkout belongs to
record_file() {
    echo "$(git -C "$1" rev-parse --absolute-git-dir)/meta-worktree.json"
}

teardown() {
    cd /
    rm -rf "$TEST_DIR"
    rm -rf "$META_DATA"
    unset META_DATA_DIR
}

# ============ --meta key=value ============

@test "worktree create --meta stores custom metadata" {
    run "$META_BIN" git worktree create meta-test --repo backend --meta agent=review-bot --meta run_id=abc123
    [ "$status" -eq 0 ]

    # The record beside the worktree holds the custom fields
    REC="$(record_file .worktrees/meta-test/backend)"
    python3 -c "
import json
entry = json.load(open('$REC'))
assert entry['custom']['agent'] == 'review-bot', f'got: {entry[\"custom\"]}'
assert entry['custom']['run_id'] == 'abc123'
"
}

@test "worktree create --meta appears in --json output" {
    run "$META_BIN" git worktree create meta-json --repo backend --meta env=ci --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json, sys
d = json.load(sys.stdin)
assert d['custom']['env'] == 'ci', f'got custom: {d.get(\"custom\")}'
"
}

@test "worktree create --meta without = warns" {
    run "$META_BIN" git worktree create meta-warn --repo backend --meta badformat
    [ "$status" -eq 0 ]
    [[ "$output" == *"warning"* ]] || [[ "$output" == *"missing"* ]]
}

# ============ --ephemeral ============

@test "worktree create --ephemeral records the ephemeral flag" {
    run "$META_BIN" git worktree create eph-test --repo backend --ephemeral
    [ "$status" -eq 0 ]

    REC="$(record_file .worktrees/eph-test/backend)"
    python3 -c "
import json
entry = json.load(open('$REC'))
assert entry['ephemeral'] == True
"
}

@test "worktree create --ephemeral in --json output" {
    run "$META_BIN" git worktree create eph-json --repo backend --ephemeral --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json, sys
d = json.load(sys.stdin)
assert d['ephemeral'] == True
"
}

# ============ --ttl ============

@test "worktree create --ttl records ttl_seconds" {
    run "$META_BIN" git worktree create ttl-test --repo backend --ttl 2h
    [ "$status" -eq 0 ]

    REC="$(record_file .worktrees/ttl-test/backend)"
    python3 -c "
import json
entry = json.load(open('$REC'))
assert entry['ttl_seconds'] == 7200, f'got: {entry[\"ttl_seconds\"]}'
"
}

@test "worktree create --ttl in --json output" {
    run "$META_BIN" git worktree create ttl-json --repo backend --ttl 30m --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json, sys
d = json.load(sys.stdin)
assert d['ttl_seconds'] == 1800, f'got: {d.get(\"ttl_seconds\")}'
"
}

@test "worktree create --ttl various formats" {
    # Seconds
    run "$META_BIN" git worktree create ttl-s --repo backend --ttl 30s --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "import json,sys; assert json.load(sys.stdin)['ttl_seconds'] == 30"

    "$META_BIN" git worktree remove ttl-s --force

    # Minutes
    run "$META_BIN" git worktree create ttl-m --repo backend --ttl 5m --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "import json,sys; assert json.load(sys.stdin)['ttl_seconds'] == 300"

    "$META_BIN" git worktree remove ttl-m --force

    # Days
    run "$META_BIN" git worktree create ttl-d --repo backend --ttl 2d --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "import json,sys; assert json.load(sys.stdin)['ttl_seconds'] == 172800"
}

@test "worktree create --ttl invalid format fails with one clear error" {
    run "$META_BIN" git worktree create ttl-bad --repo backend --ttl "notavalue"
    [ "$status" -ne 0 ]
    [[ "$output" == *"invalid value 'notavalue' for '--ttl <DURATION>'"* ]]
    # Not "error: error:", and not the whole git help after it
    [[ "$output" != *"error: error:"* ]]
    [[ "$output" != *"Pass-through Commands"* ]]
    [ ! -e .worktrees/ttl-bad ]
}

# ============ Positional <commit-ish> ============

@test "worktree create positional commit-ish creates branch from tag" {
    git -C backend tag v2.0.0

    run "$META_BIN" git worktree create pos-tag v2.0.0 --repo backend
    [ "$status" -eq 0 ]
    [ -d ".worktrees/pos-tag/backend" ]

    TAG_SHA=$(git -C backend rev-parse v2.0.0)
    WT_SHA=$(git -C ".worktrees/pos-tag/backend" rev-parse HEAD)
    [ "$TAG_SHA" = "$WT_SHA" ]
}

@test "worktree create positional commit-ish with commit hash" {
    HASH=$(git -C backend rev-parse HEAD)

    run "$META_BIN" git worktree create pos-hash "$HASH" --repo backend
    [ "$status" -eq 0 ]
    [ -d ".worktrees/pos-hash/backend" ]

    WT_SHA=$(git -C ".worktrees/pos-hash/backend" rev-parse HEAD)
    [ "$HASH" = "$WT_SHA" ]
}

@test "worktree create positional commit-ish multi-repo" {
    git -C backend tag pos-shared
    git -C frontend tag pos-shared

    run "$META_BIN" git worktree create pos-multi pos-shared --repo backend --repo frontend
    [ "$status" -eq 0 ]
    [ -d ".worktrees/pos-multi/backend" ]
    [ -d ".worktrees/pos-multi/frontend" ]
}

@test "worktree create positional and --from-pr conflict" {
    run "$META_BIN" git worktree create pos-pr-conflict v1.0.0 --from-pr org/repo#123 --repo backend
    [ "$status" -ne 0 ]
    [[ "$output" == *"cannot be used with"* ]] || [[ "$output" == *"Cannot specify both"* ]] || [[ "$output" == *"mutually exclusive"* ]]
}

# ============ Records ============

@test "worktree create records the set beside git, in no global store" {
    run "$META_BIN" git worktree create store-test --repo backend --no-deps
    [ "$status" -eq 0 ]

    [ ! -e "$META_DATA/worktree.json" ]
    REC="$(record_file .worktrees/store-test/backend)"
    [[ "$REC" == */backend/.git/worktrees/backend/meta-worktree.json ]]
    [ -f "$REC" ]
    python3 -c "
import json, os
entry = json.load(open('$REC'))
assert entry['set'] == os.path.realpath('.worktrees/store-test'), entry
assert entry['project'] != ''
assert 'created_at' in entry
"
}

@test "worktree remove deletes the records with the worktrees" {
    "$META_BIN" git worktree create store-rm --repo backend --no-deps
    [ -f "$(record_file .worktrees/store-rm/backend)" ]

    run "$META_BIN" git worktree remove store-rm
    [ "$status" -eq 0 ]

    # git deleted the worktree's admin directory, record and all
    [ ! -e "$TEST_DIR/backend/.git/worktrees/backend" ]
    [ ! -e "$META_DATA/worktree.json" ]
}

@test "worktree remove --json outputs structured result" {
    "$META_BIN" git worktree create destroy-json --repo backend --repo frontend --no-deps

    run "$META_BIN" git worktree remove destroy-json --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json, sys
d = json.load(sys.stdin)
assert d['name'] == 'destroy-json', f'name mismatch: {d}'
assert 'path' in d, 'missing path field'
assert d['repos_removed'] == 2, f'expected 2 repos, got: {d.get(\"repos_removed\")}'
"
}

@test "worktree add gives the new repo the set's record" {
    "$META_BIN" git worktree create store-add --repo backend --ttl 1h
    run "$META_BIN" git worktree add store-add --repo frontend
    [ "$status" -eq 0 ]

    cmp "$(record_file .worktrees/store-add/backend)" "$(record_file .worktrees/store-add/frontend)"
}

@test "worktree add to a set meta did not create records nothing" {
    mkdir -p .worktrees
    git -C backend worktree add --quiet -b byhand ../.worktrees/byhand/backend
    run "$META_BIN" git worktree add byhand --repo frontend
    [ "$status" -eq 0 ]
    [ ! -e "$(record_file .worktrees/byhand/frontend)" ]

    run "$META_BIN" git worktree remove byhand
    [ "$status" -ne 0 ]
    [[ "$output" == *"no record"* ]]
    [ -d .worktrees/byhand/backend ]
}

@test "worktree list shows store metadata" {
    "$META_BIN" git worktree create list-meta --repo backend --ephemeral --ttl 1h --meta env=staging
    run "$META_BIN" git worktree list --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json, sys
data = json.load(sys.stdin)
wt = next(w for w in data['worktrees'] if w['name'] == 'list-meta')
assert wt.get('ephemeral') == True, f'ephemeral: {wt.get(\"ephemeral\")}'
assert wt.get('ttl_remaining_seconds') is not None
assert wt.get('custom', {}).get('env') == 'staging'
"
}

# ============ Lifecycle Hooks ============

@test "post-create hook receives payload on stdin" {
    HOOK_LOG="$TEST_DIR/hook-create.json"

    # Configure hook in .meta.yaml
    cat > "$TEST_DIR/.meta.yaml" <<EOF
{
    "projects": {
        "backend": "git@github.com:org/backend.git",
        "frontend": "git@github.com:org/frontend.git"
    },
    "worktree": {
        "hooks": {
            "post-create": "cat > $HOOK_LOG"
        }
    }
}
EOF

    run "$META_BIN" git worktree create hook-test --repo backend --meta agent=test-bot
    [ "$status" -eq 0 ]
    [ -f "$HOOK_LOG" ]

    python3 -c "
import json
with open('$HOOK_LOG') as f:
    payload = json.load(f)
assert payload['action'] == 'create'
assert payload['name'] == 'hook-test'
assert 'repos' in payload
"
}

@test "a worktree hook doesn't inherit the repository of a git hook running meta" {
    HOOK_LOG="$TEST_DIR/hook-toplevel"
    HOOK_REPO="$(cd "$(mktemp -d)" && pwd -P)"
    git -C "$HOOK_REPO" init --quiet

    cat > "$TEST_DIR/.meta.yaml" <<EOF
{
    "projects": {
        "backend": "git@github.com:org/backend.git"
    },
    "worktree": {
        "hooks": {
            "post-create": "git rev-parse --show-toplevel > $HOOK_LOG 2>&1 || true"
        }
    }
}
EOF

    run env GIT_DIR="$HOOK_REPO/.git" GIT_WORK_TREE="$HOOK_REPO" \
        "$META_BIN" git worktree create from-hook --repo backend
    [ "$status" -eq 0 ]
    [ -f "$HOOK_LOG" ]
    [ "$(cat "$HOOK_LOG")" != "$HOOK_REPO" ]
}

@test "post-destroy hook receives payload on stdin" {
    HOOK_LOG="$TEST_DIR/hook-destroy.json"

    cat > "$TEST_DIR/.meta.yaml" <<EOF
{
    "projects": {
        "backend": "git@github.com:org/backend.git"
    },
    "worktree": {
        "hooks": {
            "post-destroy": "cat > $HOOK_LOG"
        }
    }
}
EOF

    "$META_BIN" git worktree create hook-destroy --repo backend
    run "$META_BIN" git worktree remove hook-destroy --force
    [ "$status" -eq 0 ]
    [ -f "$HOOK_LOG" ]

    python3 -c "
import json
with open('$HOOK_LOG') as f:
    payload = json.load(f)
assert payload['action'] == 'destroy'
assert payload['name'] == 'hook-destroy'
"
}

@test "hook failure does not block main operation" {
    cat > "$TEST_DIR/.meta.yaml" <<EOF
{
    "projects": {
        "backend": "git@github.com:org/backend.git"
    },
    "worktree": {
        "hooks": {
            "post-create": "exit 1"
        }
    }
}
EOF

    # Should succeed despite hook failure
    run "$META_BIN" git worktree create hook-fail --repo backend
    [ "$status" -eq 0 ]
    [ -d ".worktrees/hook-fail/backend" ]
}

@test "hook not configured is silently ignored" {
    # Default .meta.yaml has no hooks section
    run "$META_BIN" git worktree create no-hook --repo backend
    [ "$status" -eq 0 ]
    [ -d ".worktrees/no-hook/backend" ]
}

@test "hooks work with YAML config (.meta.yaml)" {
    HOOK_LOG="$TEST_DIR/hook-yaml.json"

    # Create YAML config with hook
    cat > "$TEST_DIR/.meta.yaml" <<EOF
projects:
  backend: "git@github.com:org/backend.git"
  frontend: "git@github.com:org/frontend.git"
worktree:
  hooks:
    post-create: "cat > $HOOK_LOG"
EOF

    run "$META_BIN" git worktree create yaml-hook-test --repo backend --meta env=yaml
    [ "$status" -eq 0 ]
    [ -f "$HOOK_LOG" ]

    # Verify hook received payload
    python3 -c "
import json
with open('$HOOK_LOG') as f:
    payload = json.load(f)
assert payload['action'] == 'create'
assert payload['name'] == 'yaml-hook-test'
assert payload['custom']['env'] == 'yaml'
"
}

# ============ Prune ============

@test "worktree prune --dry-run shows candidates without removing" {
    # Create a worktree with a very short TTL
    "$META_BIN" git worktree create prune-ttl --repo backend --ttl 1s

    # Wait for TTL to expire
    sleep 2

    run "$META_BIN" git worktree prune --dry-run
    [ "$status" -eq 0 ]
    [[ "$output" == *"prune-ttl"* ]]
    [[ "$output" == *"ttl_expired"* ]] || [[ "$output" == *"Would prune"* ]]

    # Worktree should still exist
    [ -d ".worktrees/prune-ttl" ]
}

@test "worktree prune removes TTL-expired worktrees" {
    "$META_BIN" git worktree create prune-exp --repo backend --ttl 1s
    sleep 2

    run "$META_BIN" git worktree prune
    [ "$status" -eq 0 ]

    # Worktree should be gone, and git's entry for it with its record
    [ ! -d ".worktrees/prune-exp" ]
    [ ! -e "$TEST_DIR/backend/.git/worktrees/backend" ]
}

@test "worktree prune detects orphaned entries" {
    "$META_BIN" git worktree create prune-orphan --repo backend

    # Manually remove the directory without going through destroy
    rm -rf ".worktrees/prune-orphan"

    run "$META_BIN" git worktree prune --dry-run
    [ "$status" -eq 0 ]
    [[ "$output" == *"prune-orphan"* ]]
    [[ "$output" == *"orphaned"* ]] || [[ "$output" == *"Would prune"* ]]
}

@test "worktree prune deletes the records of an orphaned set" {
    "$META_BIN" git worktree create prune-orphan2 --repo backend
    rm -rf ".worktrees/prune-orphan2"

    run "$META_BIN" git worktree prune
    [ "$status" -eq 0 ]
    [[ "$output" == *"prune-orphan2"* ]]

    # Only meta's record goes; git's entry stays for 'git worktree prune'
    [ -d "$TEST_DIR/backend/.git/worktrees/backend" ]
    [ ! -e "$TEST_DIR/backend/.git/worktrees/backend/meta-worktree.json" ]
    run "$META_BIN" git worktree prune
    [ "$status" -eq 0 ]
    [[ "$output" != *"prune-orphan2"* ]]
}

@test "worktree prune --json outputs structured result" {
    "$META_BIN" git worktree create prune-json --repo backend --ttl 1s
    sleep 2

    run "$META_BIN" git worktree prune --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json, sys
d = json.load(sys.stdin)
assert 'removed' in d
assert d['dry_run'] == False
assert any(e['name'] == 'prune-json' for e in d['removed'])
"
}

@test "worktree prune --dry-run --json outputs structured result" {
    "$META_BIN" git worktree create prune-dj --repo backend --ttl 1s
    sleep 2

    run "$META_BIN" git worktree prune --dry-run --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json, sys
d = json.load(sys.stdin)
assert d['dry_run'] == True
assert any(e['name'] == 'prune-dj' for e in d['removed'])
"
    # Worktree still exists
    [ -d ".worktrees/prune-dj" ]
}

@test "worktree prune with nothing to prune" {
    "$META_BIN" git worktree create prune-safe --repo backend
    run "$META_BIN" git worktree prune
    [ "$status" -eq 0 ]
    [[ "$output" == *"Nothing to prune"* ]] || [[ "$output" == *"removed"* ]]
    # Worktree should still be there
    [ -d ".worktrees/prune-safe" ]
}

@test "worktree prune post-prune hook fires" {
    HOOK_LOG="$TEST_DIR/hook-prune.json"
    cat > "$TEST_DIR/.meta.yaml" <<EOF
{
    "projects": {
        "backend": "git@github.com:org/backend.git"
    },
    "worktree": {
        "hooks": {
            "post-prune": "cat > $HOOK_LOG"
        }
    }
}
EOF

    "$META_BIN" git worktree create prune-hook --repo backend --ttl 1s
    sleep 2

    run "$META_BIN" git worktree prune
    [ "$status" -eq 0 ]
    [ -f "$HOOK_LOG" ]

    python3 -c "
import json
with open('$HOOK_LOG') as f:
    payload = json.load(f)
assert payload['action'] == 'prune'
assert 'removed' in payload
"
}

# ============ Ephemeral Exec ============

@test "worktree exec --ephemeral creates, runs, and destroys" {
    run "$META_BIN" git worktree exec --ephemeral eph-exec --repo backend -- echo ephemeral-ok
    [ "$status" -eq 0 ]
    [[ "$output" == *"ephemeral-ok"* ]]

    # Worktree should be destroyed
    [ ! -d ".worktrees/eph-exec" ]
}

@test "worktree exec --ephemeral destroys on command failure" {
    run "$META_BIN" git worktree exec --ephemeral eph-fail --repo backend -- false
    [ "$status" -ne 0 ]

    # Worktree should still be destroyed
    [ ! -d ".worktrees/eph-fail" ]
}

@test "worktree exec --ephemeral propagates exit code" {
    # Use 'false' since loop_lib joins args into a single shell string
    run "$META_BIN" git worktree exec --ephemeral eph-code --repo backend -- false
    [ "$status" -ne 0 ]
    # Worktree should still be cleaned up
    [ ! -d ".worktrees/eph-code" ]
}

@test "worktree exec --ephemeral with --meta" {
    run "$META_BIN" git worktree exec --ephemeral eph-meta --repo backend --meta agent=ci -- echo ok
    [ "$status" -eq 0 ]
    [[ "$output" == *"ok"* ]]
    # Worktree should be gone
    [ ! -d ".worktrees/eph-meta" ]
}

@test "worktree exec --ephemeral leaves no worktree or record behind" {
    run "$META_BIN" git worktree exec --ephemeral eph-store --repo backend -- echo done
    [ "$status" -eq 0 ]

    [ ! -e ".worktrees/eph-store" ]
    [ ! -e "$TEST_DIR/backend/.git/worktrees/backend" ]
}

# ============ Context Detection ============

@test "context detection: commands run in worktree repos when inside" {
    "$META_BIN" git worktree create ctx-test --repo backend --repo frontend
    [ -d ".worktrees/ctx-test/backend" ]
    [ -d ".worktrees/ctx-test/frontend" ]

    # Run from inside the worktree — should detect context
    cd ".worktrees/ctx-test/backend"
    run "$META_BIN" exec -- pwd
    [ "$status" -eq 0 ]
    # Should run in worktree repos, not project repos
    [[ "$output" == *"ctx-test"* ]]
}

@test "context detection: --primary overrides worktree context" {
    "$META_BIN" git worktree create ctx-primary --repo backend --no-deps
    [ -d ".worktrees/ctx-primary/backend" ]

    cd ".worktrees/ctx-primary/backend"
    # With --primary, should ignore worktree context
    run "$META_BIN" --primary exec -- pwd
    [ "$status" -eq 0 ]
    # Output should NOT include the worktree path if --primary works
    [[ "$output" != *"ctx-primary/backend"* ]] || [[ "$output" == *"$TEST_DIR/backend"* ]]
}

# ============ Combined Flags ============

@test "worktree create with all cloud flags" {
    run "$META_BIN" git worktree create full-cloud --repo backend --ephemeral --ttl 30m --meta agent=ci --meta run_id=123 --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json, sys
d = json.load(sys.stdin)
assert d['name'] == 'full-cloud'
assert d['ephemeral'] == True
assert d['ttl_seconds'] == 1800
assert d['custom']['agent'] == 'ci'
assert d['custom']['run_id'] == '123'
"
}

@test "worktree full lifecycle with metadata" {
    # Create with metadata
    run "$META_BIN" git worktree create lifecycle-meta --repo backend --repo frontend \
        --ephemeral --ttl 1h --meta env=test --meta version=2
    [ "$status" -eq 0 ]

    # List should show metadata
    run "$META_BIN" git worktree list --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json, sys
d = json.load(sys.stdin)
wt = next(w for w in d['worktrees'] if w['name'] == 'lifecycle-meta')
assert wt['ephemeral'] == True
"

    # Destroy
    run "$META_BIN" git worktree remove lifecycle-meta --force
    [ "$status" -eq 0 ]
    [ ! -d ".worktrees/lifecycle-meta" ]

    # git's entries, and the records in them, are gone
    [ -z "$(ls -A "$TEST_DIR/backend/.git/worktrees" 2>/dev/null)" ]
    [ -z "$(ls -A "$TEST_DIR/frontend/.git/worktrees" 2>/dev/null)" ]
}

# ============ No Global State ============

@test "worktree commands write nothing to the meta data dir" {
    run "$META_BIN" git worktree create no-store --repo backend
    [ "$status" -eq 0 ]
    [ -d ".worktrees/no-store/backend" ]

    run "$META_BIN" git worktree list
    [ "$status" -eq 0 ]
    [[ "$output" == *"no-store"* ]]

    run "$META_BIN" git worktree prune
    [ "$status" -eq 0 ]
    run "$META_BIN" git worktree remove no-store
    [ "$status" -eq 0 ]
    [ ! -e "$META_DATA/worktree.json" ] && [ ! -e "$META_DATA/worktree.lock" ]
}

@test "worktree prune with no sets" {
    run "$META_BIN" git worktree prune
    [ "$status" -eq 0 ]
    [[ "$output" == *"Nothing to prune"* ]] || [[ "$output" == *"No worktrees"* ]]
}

# ============ Deterministic Ordering ============

@test "worktree list output is sorted alphabetically" {
    # Create worktrees in non-alphabetical order
    run "$META_BIN" git worktree create zzz-last --repo backend
    [ "$status" -eq 0 ]
    run "$META_BIN" git worktree create aaa-first --repo frontend
    [ "$status" -eq 0 ]
    run "$META_BIN" git worktree create mmm-middle --repo backend
    [ "$status" -eq 0 ]

    # Verify sorted order in output
    run "$META_BIN" git worktree list
    [ "$status" -eq 0 ]

    # Extract worktree names in order
    first_pos=$(echo "$output" | grep -n "aaa-first" | head -1 | cut -d: -f1)
    middle_pos=$(echo "$output" | grep -n "mmm-middle" | head -1 | cut -d: -f1)
    last_pos=$(echo "$output" | grep -n "zzz-last" | head -1 | cut -d: -f1)

    [ "$first_pos" -lt "$middle_pos" ]
    [ "$middle_pos" -lt "$last_pos" ]
}

@test "worktree list --json output is sorted alphabetically" {
    run "$META_BIN" git worktree create zzz-sorted --repo backend
    [ "$status" -eq 0 ]
    run "$META_BIN" git worktree create aaa-sorted --repo frontend
    [ "$status" -eq 0 ]

    run "$META_BIN" git worktree list --json
    [ "$status" -eq 0 ]

    # Verify aaa comes before zzz in JSON output
    aaa_pos=$(echo "$output" | grep -n "aaa-sorted" | head -1 | cut -d: -f1)
    zzz_pos=$(echo "$output" | grep -n "zzz-sorted" | head -1 | cut -d: -f1)
    [ "$aaa_pos" -lt "$zzz_pos" ]
}

# ============ Hook stdin EOF ============

@test "hook receives complete stdin payload (EOF delivered)" {
    HOOK_OUTPUT="$TEST_DIR/hook-stdin-test.json"

    # Configure hook that reads ALL of stdin (tests EOF delivery)
    cat > "$TEST_DIR/.meta.yaml" <<EOF
{
    "projects": {
        "backend": "git@github.com:org/backend.git",
        "frontend": "git@github.com:org/frontend.git"
    },
    "worktree": {
        "hooks": {
            "post-create": "cat > $HOOK_OUTPUT"
        }
    }
}
EOF

    run "$META_BIN" git worktree create stdin-test --repo backend --meta agent=test --no-deps
    [ "$status" -eq 0 ]

    # Verify hook received valid complete JSON
    [ -f "$HOOK_OUTPUT" ]
    python3 -c "
import json
with open('$HOOK_OUTPUT') as f:
    data = json.load(f)
assert data['action'] == 'create'
assert data['name'] == 'stdin-test'
assert data['custom']['agent'] == 'test'
assert len(data['repos']) == 1
"
}

# ============ Ephemeral Exec Name Position ============

@test "worktree exec --ephemeral with name after flags" {
    # Name comes after --repo flag (tests enumerate-based extraction)
    run "$META_BIN" git worktree exec --ephemeral --repo backend name-after-flags -- echo found-it
    [ "$status" -eq 0 ]
    [[ "$output" == *"found-it"* ]]
    [ ! -d ".worktrees/name-after-flags" ]
}

@test "worktree exec --ephemeral name matches repo value (regression)" {
    # When worktree name equals a --repo value, the filter must use index-based
    # exclusion (not string equality) to avoid stripping the --repo's value.
    run "$META_BIN" git worktree exec --ephemeral --repo backend backend -- echo works
    [ "$status" -eq 0 ]
    [[ "$output" == *"works"* ]]
    [ ! -d ".worktrees/backend" ]
}

@test "worktree create with name after flags (extract_name robustness)" {
    # extract_name must skip flag values even when name comes after --repo value
    run "$META_BIN" git worktree create --repo backend name-after-repo
    [ "$status" -eq 0 ]
    [ -d ".worktrees/name-after-repo" ]
    [[ "$output" == *"name-after-repo"* ]]

    # Clean up
    run "$META_BIN" git worktree remove name-after-repo --force
    [ "$status" -eq 0 ]
}

# ============ Help ============

@test "worktree help includes cloud commands" {
    run "$META_BIN" git worktree
    [ "$status" -eq 0 ]
    [[ "$output" == *"prune"* ]]
    [[ "$output" == *"--ephemeral"* ]] || [[ "$output" == *"ephemeral"* ]]
    [[ "$output" == *"--meta"* ]] || [[ "$output" == *"meta"* ]]
    [[ "$output" == *"--ttl"* ]] || [[ "$output" == *"ttl"* ]]
    # Verify REMOVE OPTIONS section is present
    [[ "$output" == *"REMOVE OPTIONS"* ]]
    [[ "$output" == *"--force"* ]]
}
