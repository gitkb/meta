#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# Integration tests for `meta git worktree` cloud/agent extensions (meta-6)
# Tests: --meta, --ephemeral, --ttl, --from-ref, prune, lifecycle hooks,
#         context detection, ephemeral exec, centralized store

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
    META_DATA="$(mktemp -d)"
    export META_DATA_DIR="$META_DATA"

    # Create .meta config with two projects
    cat > "$TEST_DIR/.meta.json" <<'EOF'
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
    git -C "$TEST_DIR" add .meta.json
    git -C "$TEST_DIR" commit --quiet -m "init meta"

    cd "$TEST_DIR"
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

    # Verify store file was created with custom fields
    STORE="$META_DATA/worktree.json"
    [ -f "$STORE" ]
    python3 -c "
import json, sys
with open('$STORE') as f:
    data = json.load(f)
entries = data['worktrees']
# Find our entry (key is absolute path)
entry = next(v for v in entries.values() if v['name'] == 'meta-test')
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

@test "worktree create --ephemeral sets ephemeral flag in store" {
    run "$META_BIN" git worktree create eph-test --repo backend --ephemeral
    [ "$status" -eq 0 ]

    STORE="$META_DATA/worktree.json"
    python3 -c "
import json
with open('$STORE') as f:
    data = json.load(f)
entry = next(v for v in data['worktrees'].values() if v['name'] == 'eph-test')
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

@test "worktree create --ttl stores ttl_seconds in store" {
    run "$META_BIN" git worktree create ttl-test --repo backend --ttl 2h
    [ "$status" -eq 0 ]

    STORE="$META_DATA/worktree.json"
    python3 -c "
import json
with open('$STORE') as f:
    data = json.load(f)
entry = next(v for v in data['worktrees'].values() if v['name'] == 'ttl-test')
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

@test "worktree create --ttl invalid format fails" {
    run "$META_BIN" git worktree create ttl-bad --repo backend --ttl "notavalue"
    [ "$status" -ne 0 ]
}

# ============ --from-ref ============

@test "worktree create --from-ref creates branch from ref" {
    # Create a tag in backend repo to use as ref
    git -C backend tag v1.0.0

    run "$META_BIN" git worktree create from-tag --repo backend --from-ref v1.0.0
    [ "$status" -eq 0 ]
    [ -d ".worktrees/from-tag/backend" ]

    # The branch should exist and point to same commit as the tag
    TAG_SHA=$(git -C backend rev-parse v1.0.0)
    WT_SHA=$(git -C ".worktrees/from-tag/backend" rev-parse HEAD)
    [ "$TAG_SHA" = "$WT_SHA" ]
}

@test "worktree create --from-ref with commit hash" {
    HASH=$(git -C backend rev-parse HEAD)

    run "$META_BIN" git worktree create from-hash --repo backend --from-ref "$HASH"
    [ "$status" -eq 0 ]
    [ -d ".worktrees/from-hash/backend" ]

    WT_SHA=$(git -C ".worktrees/from-hash/backend" rev-parse HEAD)
    [ "$HASH" = "$WT_SHA" ]
}

@test "worktree create --from-ref nonexistent ref warns and skips" {
    run "$META_BIN" git worktree create bad-ref --repo backend --from-ref nonexistent-ref-xyz
    # Should either fail or warn — the repo should be skipped
    if [ "$status" -eq 0 ]; then
        # If it succeeded, the repo should not have been created
        [ ! -d ".worktrees/bad-ref/backend" ] || [[ "$output" == *"warning"* ]]
    fi
}

@test "worktree create --from-ref multi-repo applies to all" {
    git -C backend tag shared-base
    git -C frontend tag shared-base

    run "$META_BIN" git worktree create from-ref-multi --repo backend --repo frontend --from-ref shared-base
    [ "$status" -eq 0 ]
    [ -d ".worktrees/from-ref-multi/backend" ]
    [ -d ".worktrees/from-ref-multi/frontend" ]
}

@test "worktree create --from-ref and --from-pr mutual exclusion" {
    run "$META_BIN" git worktree create bad-combo --repo backend --from-ref v1.0.0 --from-pr org/repo#123
    [ "$status" -ne 0 ]
    [[ "$output" == *"cannot be used with"* ]] || [[ "$output" == *"Cannot specify both"* ]] || [[ "$output" == *"mutually exclusive"* ]]
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

@test "worktree create positional and --from-ref conflict" {
    run "$META_BIN" git worktree create pos-conflict v1.0.0 --from-ref v1.0.0 --repo backend
    [ "$status" -ne 0 ]
    [[ "$output" == *"cannot be used with"* ]] || [[ "$output" == *"Cannot specify both"* ]] || [[ "$output" == *"mutually exclusive"* ]]
}

@test "worktree create positional and --from-pr conflict" {
    run "$META_BIN" git worktree create pos-pr-conflict v1.0.0 --from-pr org/repo#123 --repo backend
    [ "$status" -ne 0 ]
    [[ "$output" == *"cannot be used with"* ]] || [[ "$output" == *"Cannot specify both"* ]] || [[ "$output" == *"mutually exclusive"* ]]
}

# ============ Centralized Store ============

@test "worktree create writes to centralized store" {
    run "$META_BIN" git worktree create store-test --repo backend --no-deps
    [ "$status" -eq 0 ]

    STORE="$META_DATA/worktree.json"
    [ -f "$STORE" ]
    python3 -c "
import json
with open('$STORE') as f:
    data = json.load(f)
assert len(data['worktrees']) == 1
entry = next(v for v in data['worktrees'].values() if v['name'] == 'store-test')
assert entry['project'] != ''
assert 'created_at' in entry
assert len(entry['repos']) == 1
assert entry['repos'][0]['alias'] == 'backend'
"
}

@test "worktree remove removes from centralized store" {
    "$META_BIN" git worktree create store-rm --repo backend --no-deps
    STORE="$META_DATA/worktree.json"

    # Verify entry exists
    python3 -c "
import json
with open('$STORE') as f:
    data = json.load(f)
assert any(v['name'] == 'store-rm' for v in data['worktrees'].values())
"

    # Remove
    run "$META_BIN" git worktree remove store-rm
    [ "$status" -eq 0 ]

    # Verify entry removed
    python3 -c "
import json
with open('$STORE') as f:
    data = json.load(f)
assert not any(v['name'] == 'store-rm' for v in data['worktrees'].values())
"
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

@test "worktree add updates store entry with new repo" {
    "$META_BIN" git worktree create store-add --repo backend
    run "$META_BIN" git worktree add store-add --repo frontend
    [ "$status" -eq 0 ]

    STORE="$META_DATA/worktree.json"
    python3 -c "
import json
with open('$STORE') as f:
    data = json.load(f)
entry = next(v for v in data['worktrees'].values() if v['name'] == 'store-add')
aliases = [r['alias'] for r in entry['repos']]
assert 'backend' in aliases
assert 'frontend' in aliases
"
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

    # Configure hook in .meta
    cat > "$TEST_DIR/.meta.json" <<EOF
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

@test "post-destroy hook receives payload on stdin" {
    HOOK_LOG="$TEST_DIR/hook-destroy.json"

    cat > "$TEST_DIR/.meta.json" <<EOF
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
    cat > "$TEST_DIR/.meta.json" <<EOF
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
    # Default .meta has no hooks section
    run "$META_BIN" git worktree create no-hook --repo backend
    [ "$status" -eq 0 ]
    [ -d ".worktrees/no-hook/backend" ]
}

@test "hooks work with YAML config (.meta.yaml)" {
    HOOK_LOG="$TEST_DIR/hook-yaml.json"

    # Create YAML config with hook
    rm -f "$TEST_DIR/.meta.json"
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

    # Restore JSON config for other tests
    cat > "$TEST_DIR/.meta.json" <<'EOF'
{
    "projects": {
        "backend": "git@github.com:org/backend.git",
        "frontend": "git@github.com:org/frontend.git"
    }
}
EOF
    rm -f "$TEST_DIR/.meta.yaml"
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

    # Worktree should be gone
    [ ! -d ".worktrees/prune-exp" ]

    # Store entry should be gone
    STORE="$META_DATA/worktree.json"
    python3 -c "
import json
with open('$STORE') as f:
    data = json.load(f)
assert not any(v['name'] == 'prune-exp' for v in data['worktrees'].values())
"
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

@test "worktree prune removes orphaned entries from store" {
    "$META_BIN" git worktree create prune-orphan2 --repo backend
    rm -rf ".worktrees/prune-orphan2"

    run "$META_BIN" git worktree prune
    [ "$status" -eq 0 ]

    STORE="$META_DATA/worktree.json"
    python3 -c "
import json
with open('$STORE') as f:
    data = json.load(f)
assert not any(v['name'] == 'prune-orphan2' for v in data['worktrees'].values())
"
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
    cat > "$TEST_DIR/.meta.json" <<EOF
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

@test "worktree exec --ephemeral removes from store" {
    run "$META_BIN" git worktree exec --ephemeral eph-store --repo backend -- echo done
    [ "$status" -eq 0 ]

    STORE="$META_DATA/worktree.json"
    if [ -f "$STORE" ]; then
        python3 -c "
import json
with open('$STORE') as f:
    data = json.load(f)
assert not any(v['name'] == 'eph-store' for v in data['worktrees'].values())
"
    fi
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

    # Store should be clean
    STORE="$META_DATA/worktree.json"
    python3 -c "
import json
with open('$STORE') as f:
    data = json.load(f)
assert not any(v['name'] == 'lifecycle-meta' for v in data['worktrees'].values())
"
}

# ============ Store Resilience ============

@test "worktree commands work without store file" {
    # Remove store if it exists
    rm -f "$META_DATA/worktree.json"

    run "$META_BIN" git worktree create no-store --repo backend
    [ "$status" -eq 0 ]
    [ -d ".worktrees/no-store/backend" ]

    run "$META_BIN" git worktree list
    [ "$status" -eq 0 ]
    [[ "$output" == *"no-store"* ]]
}

@test "worktree prune with empty store" {
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
    cat > "$TEST_DIR/.meta.json" <<EOF
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
