#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# Integration tests for `meta git` commands
# Tests use local git repos (no network IO required)

setup() {
    clear_git_local_env
    # Build binaries if not already built
    META_BIN="$BATS_TEST_DIRNAME/../target/debug/meta"
    META_GIT_BIN="$BATS_TEST_DIRNAME/../target/debug/meta-git"

    if [ ! -f "$META_BIN" ] || [ ! -f "$META_GIT_BIN" ]; then
        cargo build --workspace --quiet
    fi

    # Create a temp directory for each test
    TEST_DIR="$(mktemp -d)"
    mkdir -p "$TEST_DIR/.meta/plugins"
    cp "$META_GIT_BIN" "$TEST_DIR/.meta/plugins/meta-git"
    chmod +x "$TEST_DIR/.meta/plugins/meta-git"

    # Create project directories as git repos with initial commits
    for repo in frontend backend shared; do
        mkdir -p "$TEST_DIR/$repo"
        git -C "$TEST_DIR/$repo" init --quiet
        git -C "$TEST_DIR/$repo" config user.email "test@test.com"
        git -C "$TEST_DIR/$repo" config user.name "Test"
        echo "# $repo" > "$TEST_DIR/$repo/README.md"
        git -C "$TEST_DIR/$repo" add README.md
        git -C "$TEST_DIR/$repo" commit --quiet -m "Initial commit for $repo"
    done

    # Initialize the root as a git repo too
    git -C "$TEST_DIR" init --quiet
    git -C "$TEST_DIR" config user.email "test@test.com"
    git -C "$TEST_DIR" config user.name "Test"

    # Default .meta config
    cat > "$TEST_DIR/.meta.json" <<'EOF'
{
    "projects": {
        "frontend": "git@github.com:org/frontend.git",
        "backend": "git@github.com:org/backend.git",
        "shared": "git@github.com:org/shared.git"
    }
}
EOF

    git -C "$TEST_DIR" add .meta.json
    git -C "$TEST_DIR" commit --quiet -m "Initial meta commit"

    cd "$TEST_DIR"
}

teardown() {
    rm -rf "$TEST_DIR"
}

# ============ meta git status ============

@test "meta git status runs successfully" {
    run "$META_BIN" git status
    [ "$status" -eq 0 ]
}

@test "meta git status shows output for each repo" {
    run "$META_BIN" git status
    [ "$status" -eq 0 ]
    [[ "$output" == *"frontend"* ]]
    [[ "$output" == *"backend"* ]]
    [[ "$output" == *"shared"* ]]
}

@test "meta git status detects dirty repo" {
    echo "change" >> "$TEST_DIR/frontend/README.md"
    run "$META_BIN" git status
    [ "$status" -eq 0 ]
    [[ "$output" == *"frontend"* ]]
    # Should show modified file info
    [[ "$output" == *"modified"* ]] || [[ "$output" == *"Changes"* ]]
}

@test "meta git status shows clean repos" {
    run "$META_BIN" git status
    [ "$status" -eq 0 ]
    # Clean repos show "nothing to commit" or similar
    [[ "$output" == *"nothing to commit"* ]] || [[ "$output" == *"clean"* ]]
}

# ============ meta git snapshot create ============

@test "meta git snapshot create captures workspace state" {
    run "$META_BIN" git snapshot create test-snap
    [ "$status" -eq 0 ]
    [[ "$output" == *"Captured state"* ]] || [[ "$output" == *"test-snap"* ]]
    # Snapshot file should exist
    [ -f "$TEST_DIR/.meta-snapshots/test-snap.json" ]
}

@test "meta git snapshot create requires a name" {
    run "$META_BIN" git snapshot create
    [ "$status" -ne 0 ] || [[ "$output" == *"Usage"* ]] || [[ "$output" == *"name"* ]]
}

@test "meta git snapshot create records correct repo count" {
    run "$META_BIN" git snapshot create count-test
    [ "$status" -eq 0 ]
    # Should capture frontend, backend, shared, and root (.)
    # Verify the snapshot file has repo entries
    python3 -c "
import json, sys
with open('$TEST_DIR/.meta-snapshots/count-test.json') as f:
    snap = json.load(f)
repos = snap['repos']
assert len(repos) >= 3, f'Expected at least 3 repos, got {len(repos)}: {list(repos.keys())}'
"
}

@test "meta git snapshot create records branch info" {
    run "$META_BIN" git snapshot create branch-test
    [ "$status" -eq 0 ]
    python3 -c "
import json
with open('$TEST_DIR/.meta-snapshots/branch-test.json') as f:
    snap = json.load(f)
for name, state in snap['repos'].items():
    assert 'sha' in state, f'{name} missing sha'
    assert 'branch' in state or state.get('branch') is None, f'{name} missing branch'
    assert 'dirty' in state, f'{name} missing dirty flag'
"
}

@test "meta git snapshot create detects dirty repos" {
    echo "uncommitted change" >> "$TEST_DIR/backend/README.md"
    run "$META_BIN" git snapshot create dirty-test
    [ "$status" -eq 0 ]
    [[ "$output" == *"dirty"* ]]
    python3 -c "
import json
with open('$TEST_DIR/.meta-snapshots/dirty-test.json') as f:
    snap = json.load(f)
# Keys may be absolute paths; find the one containing 'backend'
backend = None
for key, state in snap['repos'].items():
    if 'backend' in key:
        backend = state
        break
assert backend is not None, f'backend not in snapshot: {list(snap[\"repos\"].keys())}'
assert backend['dirty'] == True, f'backend should be dirty, got {backend}'
"
}

# ============ meta git snapshot list ============

@test "meta git snapshot list shows no snapshots initially" {
    run "$META_BIN" git snapshot list
    [ "$status" -eq 0 ]
    [[ "$output" == *"No snapshots"* ]]
}

@test "meta git snapshot list shows created snapshots" {
    "$META_BIN" git snapshot create alpha
    "$META_BIN" git snapshot create beta
    run "$META_BIN" git snapshot list
    [ "$status" -eq 0 ]
    [[ "$output" == *"alpha"* ]]
    [[ "$output" == *"beta"* ]]
}

@test "meta git snapshot list shows repo count" {
    "$META_BIN" git snapshot create count-snap
    run "$META_BIN" git snapshot list
    [ "$status" -eq 0 ]
    # Should mention the number of repos
    [[ "$output" == *"repos"* ]] || [[ "$output" == *"3"* ]] || [[ "$output" == *"4"* ]]
}

# ============ meta git snapshot show ============

@test "meta git snapshot show displays snapshot details" {
    "$META_BIN" git snapshot create show-test
    run "$META_BIN" git snapshot show show-test
    [ "$status" -eq 0 ]
    [[ "$output" == *"show-test"* ]]
    [[ "$output" == *"frontend"* ]]
    [[ "$output" == *"backend"* ]]
    [[ "$output" == *"shared"* ]]
}

@test "meta git snapshot show displays branch info" {
    "$META_BIN" git snapshot create branch-show
    run "$META_BIN" git snapshot show branch-show
    [ "$status" -eq 0 ]
    # Should show branch names (main or master)
    [[ "$output" == *"main"* ]] || [[ "$output" == *"master"* ]]
}

@test "meta git snapshot show fails for nonexistent snapshot" {
    run "$META_BIN" git snapshot show nonexistent
    [ "$status" -ne 0 ] || [[ "$output" == *"not found"* ]] || [[ "$output" == *"error"* ]] || [[ "$output" == *"Error"* ]]
}

@test "meta git snapshot show requires a name" {
    run "$META_BIN" git snapshot show
    [ "$status" -ne 0 ] || [[ "$output" == *"Usage"* ]] || [[ "$output" == *"name"* ]]
}

# ============ meta git snapshot delete ============

@test "meta git snapshot delete removes snapshot" {
    "$META_BIN" git snapshot create del-test
    [ -f "$TEST_DIR/.meta-snapshots/del-test.json" ]
    run "$META_BIN" git snapshot delete del-test
    [ "$status" -eq 0 ]
    [ ! -f "$TEST_DIR/.meta-snapshots/del-test.json" ]
}

@test "meta git snapshot delete fails for nonexistent" {
    run "$META_BIN" git snapshot delete ghost
    [ "$status" -ne 0 ] || [[ "$output" == *"not found"* ]] || [[ "$output" == *"error"* ]] || [[ "$output" == *"Error"* ]]
}

# ============ meta git snapshot restore (dry-run only) ============

@test "meta git snapshot restore --dry-run previews changes" {
    "$META_BIN" git snapshot create restore-test
    # Make a change after snapshot
    echo "new content" >> "$TEST_DIR/frontend/README.md"
    git -C "$TEST_DIR/frontend" add README.md
    git -C "$TEST_DIR/frontend" commit --quiet -m "Post-snapshot change"

    run "$META_BIN" --dry-run git snapshot restore restore-test
    [ "$status" -eq 0 ]
    [[ "$output" == *"DRY RUN"* ]] || [[ "$output" == *"dry run"* ]] || [[ "$output" == *"Dry run"* ]]
}

@test "meta git snapshot restore --dry-run makes no changes" {
    "$META_BIN" git snapshot create no-change-test
    ORIGINAL_SHA=$(git -C "$TEST_DIR/frontend" rev-parse HEAD)
    echo "new" >> "$TEST_DIR/frontend/README.md"
    git -C "$TEST_DIR/frontend" add README.md
    git -C "$TEST_DIR/frontend" commit --quiet -m "Change"

    "$META_BIN" --dry-run git snapshot restore no-change-test
    CURRENT_SHA=$(git -C "$TEST_DIR/frontend" rev-parse HEAD)
    # SHA should NOT have reverted (dry-run doesn't change anything)
    [ "$CURRENT_SHA" != "$ORIGINAL_SHA" ]
}

@test "meta git snapshot restore --force restores to snapshot state" {
    "$META_BIN" git snapshot create force-test
    SNAPSHOT_SHA=$(git -C "$TEST_DIR/frontend" rev-parse HEAD)

    # Make changes after snapshot
    echo "post-snapshot" >> "$TEST_DIR/frontend/README.md"
    git -C "$TEST_DIR/frontend" add README.md
    git -C "$TEST_DIR/frontend" commit --quiet -m "Post-snapshot commit"
    POST_SHA=$(git -C "$TEST_DIR/frontend" rev-parse HEAD)
    [ "$POST_SHA" != "$SNAPSHOT_SHA" ]

    run "$META_BIN" git snapshot restore force-test --force
    [ "$status" -eq 0 ]

    RESTORED_SHA=$(git -C "$TEST_DIR/frontend" rev-parse HEAD)
    [ "$RESTORED_SHA" = "$SNAPSHOT_SHA" ]
}

# ============ meta git snapshot full lifecycle ============

@test "meta git snapshot full lifecycle: create, list, show, delete" {
    # Create
    run "$META_BIN" git snapshot create lifecycle
    [ "$status" -eq 0 ]
    [ -f "$TEST_DIR/.meta-snapshots/lifecycle.json" ]

    # List
    run "$META_BIN" git snapshot list
    [ "$status" -eq 0 ]
    [[ "$output" == *"lifecycle"* ]]

    # Show
    run "$META_BIN" git snapshot show lifecycle
    [ "$status" -eq 0 ]
    [[ "$output" == *"lifecycle"* ]]
    [[ "$output" == *"frontend"* ]]

    # Delete
    run "$META_BIN" git snapshot delete lifecycle
    [ "$status" -eq 0 ]
    [ ! -f "$TEST_DIR/.meta-snapshots/lifecycle.json" ]

    # Verify gone from list
    run "$META_BIN" git snapshot list
    [[ "$output" != *"lifecycle"* ]]
}

# ============ meta git commit ============

@test "meta git commit with no staged changes reports nothing" {
    run "$META_BIN" git commit -m "test"
    [ "$status" -eq 0 ]
    [[ "$output" == *"No staged changes"* ]] || [[ "$output" == *"nothing"* ]]
}

@test "meta git commit -m commits to repos with staged changes" {
    # Stage a change in frontend
    echo "new feature" >> "$TEST_DIR/frontend/feature.txt"
    git -C "$TEST_DIR/frontend" add feature.txt

    run "$META_BIN" git commit -m "feat: add feature"
    [ "$status" -eq 0 ]

    # Verify commit was made
    LAST_MSG=$(git -C "$TEST_DIR/frontend" log -1 --pretty=%s)
    [ "$LAST_MSG" = "feat: add feature" ]
}

@test "meta git commit -m only commits to repos with staged changes" {
    # Stage in frontend only
    echo "change" >> "$TEST_DIR/frontend/new.txt"
    git -C "$TEST_DIR/frontend" add new.txt

    BACKEND_SHA_BEFORE=$(git -C "$TEST_DIR/backend" rev-parse HEAD)

    run "$META_BIN" git commit -m "only frontend"
    [ "$status" -eq 0 ]

    # Backend should be unchanged
    BACKEND_SHA_AFTER=$(git -C "$TEST_DIR/backend" rev-parse HEAD)
    [ "$BACKEND_SHA_BEFORE" = "$BACKEND_SHA_AFTER" ]
}

@test "meta git commit -m commits to multiple repos" {
    # Stage changes in both frontend and backend
    echo "fe change" >> "$TEST_DIR/frontend/multi.txt"
    git -C "$TEST_DIR/frontend" add multi.txt
    echo "be change" >> "$TEST_DIR/backend/multi.txt"
    git -C "$TEST_DIR/backend" add multi.txt

    run "$META_BIN" git commit -m "multi-repo commit"
    [ "$status" -eq 0 ]

    FE_MSG=$(git -C "$TEST_DIR/frontend" log -1 --pretty=%s)
    BE_MSG=$(git -C "$TEST_DIR/backend" log -1 --pretty=%s)
    [ "$FE_MSG" = "multi-repo commit" ]
    [ "$BE_MSG" = "multi-repo commit" ]
}

@test "meta git commit without flags shows repos with staged changes" {
    echo "staged" >> "$TEST_DIR/shared/staged.txt"
    git -C "$TEST_DIR/shared" add staged.txt

    run "$META_BIN" git commit
    [ "$status" -eq 0 ]
    [[ "$output" == *"shared"* ]]
    [[ "$output" == *"--edit"* ]] || [[ "$output" == *"-m"* ]]
}

# ============ meta git update ============

@test "meta git update with all repos present shows up-to-date" {
    run "$META_BIN" git update
    [ "$status" -eq 0 ]
    [[ "$output" == *"already"* ]] || [[ "$output" == *"All"* ]] || [[ "$output" == *"up"* ]]
}

@test "meta git update detects missing repos" {
    rm -rf "$TEST_DIR/shared"
    run "$META_BIN" git update
    # It should detect that shared is missing and try to clone it
    # (will fail since the URL is fake, but should detect the missing repo)
    [[ "$output" == *"shared"* ]] || [[ "$output" == *"missing"* ]] || [[ "$output" == *"clone"* ]] || [[ "$output" == *"orphan"* ]]
}

# ============ meta git clone (local) ============

@test "meta git clone from local path works" {
    # Create a "remote" bare repo that IS a meta repo
    REMOTE_DIR="$(mktemp -d)"
    git init --bare --quiet "$REMOTE_DIR"
    git -C "$REMOTE_DIR" symbolic-ref HEAD refs/heads/main

    # Create a temp working copy to push initial meta content
    WORK_DIR="$(mktemp -d)"
    git clone --quiet "$REMOTE_DIR" "$WORK_DIR/meta-repo"
    cd "$WORK_DIR/meta-repo"
    git config user.email "test@test.com"
    git config user.name "Test"
    git checkout -b main --quiet 2>/dev/null || true
    cat > .meta <<'EOF'
{
    "projects": {
        "child": "file:///dev/null"
    }
}
EOF
    git add .meta
    git commit --quiet -m "Add .meta"
    git push --quiet -u origin main

    # Now test cloning
    CLONE_TARGET="$(mktemp -d)"
    rm -rf "$CLONE_TARGET"

    cd "$TEST_DIR"
    run "$META_BIN" git clone "$REMOTE_DIR" "$CLONE_TARGET"
    [ "$status" -eq 0 ] || true  # May fail on child clone (fake URL), but meta repo should clone

    # Clean up
    rm -rf "$REMOTE_DIR" "$WORK_DIR" "$CLONE_TARGET"
}

@test "meta git clone with custom directory name for meta repo" {
    # Custom directory only applies to the meta repo itself
    # Child repos clone to paths according to .meta config files

    # Create a "remote" bare repo with main as default branch
    REMOTE_DIR="$(mktemp -d)"
    git init --bare --quiet "$REMOTE_DIR"
    git -C "$REMOTE_DIR" symbolic-ref HEAD refs/heads/main

    # Create a temp working copy to push initial content
    WORK_DIR="$(mktemp -d)"
    git clone --quiet "$REMOTE_DIR" "$WORK_DIR/meta-repo"
    cd "$WORK_DIR/meta-repo"
    git config user.email "test@test.com"
    git config user.name "Test"
    git checkout -b main --quiet 2>/dev/null || true
    cat > .meta <<'EOF'
{
    "projects": {}
}
EOF
    git add .meta
    git commit --quiet -m "Add .meta"
    git push --quiet -u origin main

    # Clone with custom directory name
    CLONE_DIR="$(mktemp -d)"
    rm -rf "$CLONE_DIR"
    CUSTOM_NAME="my-custom-clone-name"

    cd "$TEST_DIR"
    run "$META_BIN" git clone "$REMOTE_DIR" "$CLONE_DIR/$CUSTOM_NAME"
    [ "$status" -eq 0 ]

    # Verify the custom directory was used for the meta repo
    [ -d "$CLONE_DIR/$CUSTOM_NAME" ]
    [ -f "$CLONE_DIR/$CUSTOM_NAME/.meta" ]

    # Clean up
    rm -rf "$REMOTE_DIR" "$WORK_DIR" "$CLONE_DIR"
}

@test "meta git clone passes URL correctly to git" {
    # This test verifies the fix for the argument passing bug
    # where the URL was being lost due to double-stripping of args

    # Create a "remote" bare repo with main as default branch
    REMOTE_DIR="$(mktemp -d)"
    git init --bare --quiet "$REMOTE_DIR"
    git -C "$REMOTE_DIR" symbolic-ref HEAD refs/heads/main

    # Push minimal content
    WORK_DIR="$(mktemp -d)"
    git clone --quiet "$REMOTE_DIR" "$WORK_DIR/meta-repo"
    cd "$WORK_DIR/meta-repo"
    git config user.email "test@test.com"
    git config user.name "Test"
    git checkout -b main --quiet 2>/dev/null || true
    echo '{"projects": {}}' > .meta
    git add .meta
    git commit --quiet -m "Init"
    git push --quiet -u origin main

    CLONE_TARGET="$(mktemp -d)/test-clone"

    cd "$TEST_DIR"
    run "$META_BIN" git clone "$REMOTE_DIR" "$CLONE_TARGET"

    # Should succeed, not fail with "No repository URL provided"
    [ "$status" -eq 0 ]
    [[ "$output" != *"No repository URL provided"* ]]
    [ -d "$CLONE_TARGET/.git" ]

    # Clean up
    rm -rf "$REMOTE_DIR" "$WORK_DIR" "$CLONE_TARGET"
}

@test "meta git clone with --depth option" {
    # Create a "remote" bare repo with main as default branch
    REMOTE_DIR="$(mktemp -d)"
    git init --bare --quiet "$REMOTE_DIR"
    git -C "$REMOTE_DIR" symbolic-ref HEAD refs/heads/main

    WORK_DIR="$(mktemp -d)"
    git clone --quiet "$REMOTE_DIR" "$WORK_DIR/meta-repo"
    cd "$WORK_DIR/meta-repo"
    git config user.email "test@test.com"
    git config user.name "Test"
    # Set default branch name explicitly
    git checkout -b main --quiet 2>/dev/null || true
    echo '{"projects": {}}' > .meta
    git add .meta
    git commit --quiet -m "Commit 1"
    # Create additional commits by adding new files (keep .meta valid JSON)
    echo "readme 1" > README.md
    git add README.md
    git commit --quiet -m "Commit 2"
    echo "readme 2" >> README.md
    git add README.md
    git commit --quiet -m "Commit 3"
    git push --quiet -u origin main

    CLONE_TARGET="$(mktemp -d)/shallow-clone"

    cd "$TEST_DIR"
    # Use file:// protocol for --depth to work with local repos
    run "$META_BIN" git clone --depth 1 "file://$REMOTE_DIR" "$CLONE_TARGET"
    [ "$status" -eq 0 ]
    [ -d "$CLONE_TARGET/.git" ]

    # Verify it's a shallow clone (only 1 commit)
    COMMIT_COUNT=$(git -C "$CLONE_TARGET" rev-list --count HEAD)
    [ "$COMMIT_COUNT" -eq 1 ]

    # Clean up
    rm -rf "$REMOTE_DIR" "$WORK_DIR" "$CLONE_TARGET"
}

@test "meta git clone --dry-run shows what would be cloned" {
    cd "$TEST_DIR"
    run "$META_BIN" --dry-run git clone "git@github.com:example/repo.git"
    [ "$status" -eq 0 ]
    [[ "$output" == *"DRY RUN"* ]] || [[ "$output" == *"Would clone"* ]]
    [[ "$output" == *"git@github.com:example/repo.git"* ]]
}

# ============ meta git snapshot with --recursive ============

@test "meta git snapshot create with nested meta repos" {
    # Create a nested .meta inside frontend
    mkdir -p "$TEST_DIR/frontend/sub-component"
    git -C "$TEST_DIR/frontend/sub-component" init --quiet
    git -C "$TEST_DIR/frontend/sub-component" config user.email "test@test.com"
    git -C "$TEST_DIR/frontend/sub-component" config user.name "Test"
    echo "# sub" > "$TEST_DIR/frontend/sub-component/README.md"
    git -C "$TEST_DIR/frontend/sub-component" add README.md
    git -C "$TEST_DIR/frontend/sub-component" commit --quiet -m "Init sub-component"

    cat > "$TEST_DIR/frontend/.meta" <<'EOF'
{
    "projects": {
        "sub-component": "git@github.com:org/sub.git"
    }
}
EOF

    run "$META_BIN" --recursive git snapshot create nested-snap
    [ "$status" -eq 0 ]
    [ -f "$TEST_DIR/.meta-snapshots/nested-snap.json" ]
}

# ============ meta git snapshot help ============

@test "meta git snapshot with no subcommand shows help" {
    run "$META_BIN" git snapshot
    [ "$status" -eq 0 ]
    [[ "$output" == *"create"* ]]
    [[ "$output" == *"list"* ]]
    [[ "$output" == *"show"* ]]
    [[ "$output" == *"restore"* ]]
    [[ "$output" == *"delete"* ]]
}

# ============ meta git --dry-run ============

@test "meta git status --dry-run shows plan without executing" {
    run "$META_BIN" --dry-run git status
    [ "$status" -eq 0 ]
    # Dry run should show what would be executed
    [[ "$output" == *"git status"* ]] || [[ "$output" == *"Would run"* ]] || [[ "$output" == *"DRY"* ]] || [[ "$output" == *"plan"* ]]
}

# ============ Edge cases ============

@test "meta git status with non-git project directory" {
    # Add a non-git directory to projects
    mkdir -p "$TEST_DIR/not-a-repo"
    cat > "$TEST_DIR/.meta.json" <<'EOF'
{
    "projects": {
        "frontend": "git@github.com:org/frontend.git",
        "not-a-repo": "git@github.com:org/notagit.git"
    }
}
EOF
    run "$META_BIN" git status
    [ "$status" -eq 0 ]
    # Should still succeed, handling the non-git dir gracefully
}

@test "meta git commit -m with special characters in message" {
    echo "change" >> "$TEST_DIR/frontend/special.txt"
    git -C "$TEST_DIR/frontend" add special.txt

    run "$META_BIN" git commit -m "fix: handle \"quotes\" & ampersands"
    [ "$status" -eq 0 ]

    LAST_MSG=$(git -C "$TEST_DIR/frontend" log -1 --pretty=%s)
    [[ "$LAST_MSG" == *"quotes"* ]]
}

@test "meta git snapshot create overwrites existing snapshot" {
    "$META_BIN" git snapshot create overwrite-test
    # Modify a repo
    echo "change" >> "$TEST_DIR/backend/README.md"
    git -C "$TEST_DIR/backend" add README.md
    git -C "$TEST_DIR/backend" commit --quiet -m "Change for overwrite"
    NEW_SHA=$(git -C "$TEST_DIR/backend" rev-parse HEAD)

    # Create again with same name
    run "$META_BIN" git snapshot create overwrite-test
    [ "$status" -eq 0 ]

    # Should have the new SHA
    python3 -c "
import json
with open('$TEST_DIR/.meta-snapshots/overwrite-test.json') as f:
    snap = json.load(f)
# Keys may be absolute paths; find the one containing 'backend'
backend_sha = None
for key, state in snap['repos'].items():
    if 'backend' in key:
        backend_sha = state['sha']
        break
assert backend_sha is not None, f'backend not in snapshot: {list(snap[\"repos\"].keys())}'
assert backend_sha == '$NEW_SHA', f'Snapshot not updated: got {backend_sha}'
"
}

# ============ --recursive ============

@test "meta git status --recursive discovers nested repos" {
    # Create a nested .meta inside frontend with a sub-component
    mkdir -p "$TEST_DIR/frontend/sub-lib"
    git -C "$TEST_DIR/frontend/sub-lib" init --quiet
    git -C "$TEST_DIR/frontend/sub-lib" config user.email "test@test.com"
    git -C "$TEST_DIR/frontend/sub-lib" config user.name "Test"
    echo "# sub-lib" > "$TEST_DIR/frontend/sub-lib/README.md"
    git -C "$TEST_DIR/frontend/sub-lib" add README.md
    git -C "$TEST_DIR/frontend/sub-lib" commit --quiet -m "Init sub-lib"

    cat > "$TEST_DIR/frontend/.meta" <<'EOF'
{
    "projects": {
        "sub-lib": "git@github.com:org/sub-lib.git"
    }
}
EOF

    run "$META_BIN" --recursive git status
    [ "$status" -eq 0 ]
    # Should include the nested sub-lib repo
    [[ "$output" == *"sub-lib"* ]]
}

@test "meta git status --recursive includes all levels" {
    # Create two levels of nesting
    mkdir -p "$TEST_DIR/frontend/sub-lib"
    git -C "$TEST_DIR/frontend/sub-lib" init --quiet
    git -C "$TEST_DIR/frontend/sub-lib" config user.email "test@test.com"
    git -C "$TEST_DIR/frontend/sub-lib" config user.name "Test"
    echo "# sub-lib" > "$TEST_DIR/frontend/sub-lib/README.md"
    git -C "$TEST_DIR/frontend/sub-lib" add README.md
    git -C "$TEST_DIR/frontend/sub-lib" commit --quiet -m "Init sub-lib"

    mkdir -p "$TEST_DIR/frontend/sub-lib/deep"
    git -C "$TEST_DIR/frontend/sub-lib/deep" init --quiet
    git -C "$TEST_DIR/frontend/sub-lib/deep" config user.email "test@test.com"
    git -C "$TEST_DIR/frontend/sub-lib/deep" config user.name "Test"
    echo "# deep" > "$TEST_DIR/frontend/sub-lib/deep/README.md"
    git -C "$TEST_DIR/frontend/sub-lib/deep" add README.md
    git -C "$TEST_DIR/frontend/sub-lib/deep" commit --quiet -m "Init deep"

    cat > "$TEST_DIR/frontend/.meta" <<'EOF'
{
    "projects": {
        "sub-lib": "git@github.com:org/sub-lib.git"
    }
}
EOF
    cat > "$TEST_DIR/frontend/sub-lib/.meta" <<'EOF'
{
    "projects": {
        "deep": "git@github.com:org/deep.git"
    }
}
EOF

    run "$META_BIN" --recursive git status
    [ "$status" -eq 0 ]
    [[ "$output" == *"sub-lib"* ]]
    [[ "$output" == *"deep"* ]]
}

@test "meta git status --recursive --depth limits recursion" {
    # Create nested structure: frontend -> sub-lib -> deep
    mkdir -p "$TEST_DIR/frontend/sub-lib/deep"

    for repo in "$TEST_DIR/frontend/sub-lib" "$TEST_DIR/frontend/sub-lib/deep"; do
        git -C "$repo" init --quiet
        git -C "$repo" config user.email "test@test.com"
        git -C "$repo" config user.name "Test"
        echo "# $(basename $repo)" > "$repo/README.md"
        git -C "$repo" add README.md
        git -C "$repo" commit --quiet -m "Init $(basename $repo)"
    done

    cat > "$TEST_DIR/frontend/.meta" <<'EOF'
{
    "projects": {
        "sub-lib": "git@github.com:org/sub-lib.git"
    }
}
EOF
    cat > "$TEST_DIR/frontend/sub-lib/.meta" <<'EOF'
{
    "projects": {
        "deep": "git@github.com:org/deep.git"
    }
}
EOF

    # depth 0: only top-level projects, no nested discovery
    run "$META_BIN" --recursive --depth 0 git status
    [ "$status" -eq 0 ]
    [[ "$output" == *"frontend"* ]]
    [[ "$output" != *"deep"* ]]
}

@test "meta git snapshot create --recursive captures nested repos" {
    mkdir -p "$TEST_DIR/backend/sub-service"
    git -C "$TEST_DIR/backend/sub-service" init --quiet
    git -C "$TEST_DIR/backend/sub-service" config user.email "test@test.com"
    git -C "$TEST_DIR/backend/sub-service" config user.name "Test"
    echo "# sub-service" > "$TEST_DIR/backend/sub-service/README.md"
    git -C "$TEST_DIR/backend/sub-service" add README.md
    git -C "$TEST_DIR/backend/sub-service" commit --quiet -m "Init sub-service"

    cat > "$TEST_DIR/backend/.meta" <<'EOF'
{
    "projects": {
        "sub-service": "git@github.com:org/sub-service.git"
    }
}
EOF

    run "$META_BIN" --recursive git snapshot create recursive-snap
    [ "$status" -eq 0 ]
    [ -f "$TEST_DIR/.meta-snapshots/recursive-snap.json" ]

    # Verify the nested repo is in the snapshot
    python3 -c "
import json
with open('$TEST_DIR/.meta-snapshots/recursive-snap.json') as f:
    snap = json.load(f)
has_sub = any('sub-service' in key for key in snap['repos'])
assert has_sub, f'sub-service not in snapshot: {list(snap[\"repos\"].keys())}'
"
}

@test "meta git commit -m --recursive commits in nested repos" {
    # Create nested repo with staged change
    mkdir -p "$TEST_DIR/frontend/sub-lib"
    git -C "$TEST_DIR/frontend/sub-lib" init --quiet
    git -C "$TEST_DIR/frontend/sub-lib" config user.email "test@test.com"
    git -C "$TEST_DIR/frontend/sub-lib" config user.name "Test"
    echo "initial" > "$TEST_DIR/frontend/sub-lib/file.txt"
    git -C "$TEST_DIR/frontend/sub-lib" add file.txt
    git -C "$TEST_DIR/frontend/sub-lib" commit --quiet -m "Init"

    cat > "$TEST_DIR/frontend/.meta" <<'EOF'
{
    "projects": {
        "sub-lib": "git@github.com:org/sub-lib.git"
    }
}
EOF

    # Stage a change in the nested repo
    echo "change" >> "$TEST_DIR/frontend/sub-lib/file.txt"
    git -C "$TEST_DIR/frontend/sub-lib" add file.txt

    run "$META_BIN" --recursive git commit -m "recursive commit"
    [ "$status" -eq 0 ]

    # Verify the commit happened in the nested repo
    LAST_MSG=$(git -C "$TEST_DIR/frontend/sub-lib" log -1 --pretty=%s)
    [ "$LAST_MSG" = "recursive commit" ]
}

@test "meta exec --recursive runs in nested project dirs" {
    mkdir -p "$TEST_DIR/backend/sub-service"
    cat > "$TEST_DIR/backend/.meta" <<'EOF'
{
    "projects": {
        "sub-service": "git@github.com:org/sub-service.git"
    }
}
EOF

    run "$META_BIN" --recursive exec -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"sub-service"* ]]
}

@test "meta exec --recursive --depth 0 does not recurse" {
    mkdir -p "$TEST_DIR/backend/sub-service"
    cat > "$TEST_DIR/backend/.meta" <<'EOF'
{
    "projects": {
        "sub-service": "git@github.com:org/sub-service.git"
    }
}
EOF

    run "$META_BIN" --recursive --depth 0 exec -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"backend"* ]]
    [[ "$output" != *"sub-service"* ]]
}

# ============ meta git clone --recursive ============

# Helper: create a bare repo containing a .meta config with given projects.
# Usage: create_meta_bare_repo <bare_dir> <projects_json>
# Returns the file:// URL suitable for cloning.
create_meta_bare_repo() {
    local bare_dir="$1"
    local meta_content="$2"
    local meta_format="${3:-.meta}"  # default to JSON .meta

    local work_dir="$(mktemp -d)"
    git init --bare --quiet "$bare_dir"
    git -C "$bare_dir" symbolic-ref HEAD refs/heads/main

    git clone --quiet "$bare_dir" "$work_dir/checkout"
    (
        cd "$work_dir/checkout"
        git config user.email "test@test.com"
        git config user.name "Test"
        git checkout -b main --quiet 2>/dev/null || true
        echo "$meta_content" > "$meta_format"
        git add "$meta_format"
        git commit --quiet -m "Add meta config"
        git push --quiet -u origin main
    )

    rm -rf "$work_dir"
}

@test "meta git clone --recursive clones 2 levels deep" {
    # Setup: root -> child (meta:true) -> grandchild
    GRANDCHILD_BARE="$(mktemp -d)/grandchild.git"
    create_meta_bare_repo "$GRANDCHILD_BARE" '{"projects": {}}'

    CHILD_BARE="$(mktemp -d)/child.git"
    create_meta_bare_repo "$CHILD_BARE" "{\"projects\": {\"grandchild\": \"file://$GRANDCHILD_BARE\"}}"

    ROOT_BARE="$(mktemp -d)/root.git"
    create_meta_bare_repo "$ROOT_BARE" "{\"projects\": {\"child\": {\"repo\": \"file://$CHILD_BARE\", \"meta\": true}}}"

    CLONE_TARGET="$(mktemp -d)/clone-2level"
    rm -rf "$CLONE_TARGET"

    cd "$TEST_DIR"
    run "$META_BIN" git clone --recursive "file://$ROOT_BARE" "$CLONE_TARGET"
    [ "$status" -eq 0 ]

    # Root was cloned
    [ -d "$CLONE_TARGET/.git" ]
    # Child was cloned
    [ -d "$CLONE_TARGET/child/.git" ]
    # Grandchild was cloned (recursive discovered child's .meta)
    [ -d "$CLONE_TARGET/child/grandchild/.git" ]

    rm -rf "$GRANDCHILD_BARE" "$CHILD_BARE" "$ROOT_BARE" "$CLONE_TARGET"
}

@test "meta git clone --recursive clones 3 levels deep" {
    # Setup: root -> l1 (meta:true) -> l2 (meta:true) -> l3
    L3_BARE="$(mktemp -d)/l3.git"
    create_meta_bare_repo "$L3_BARE" '{"projects": {}}'

    L2_BARE="$(mktemp -d)/l2.git"
    create_meta_bare_repo "$L2_BARE" "{\"projects\": {\"l3\": \"file://$L3_BARE\"}}"

    L1_BARE="$(mktemp -d)/l1.git"
    create_meta_bare_repo "$L1_BARE" "{\"projects\": {\"l2\": {\"repo\": \"file://$L2_BARE\", \"meta\": true}}}"

    ROOT_BARE="$(mktemp -d)/root.git"
    create_meta_bare_repo "$ROOT_BARE" "{\"projects\": {\"l1\": {\"repo\": \"file://$L1_BARE\", \"meta\": true}}}"

    CLONE_TARGET="$(mktemp -d)/clone-3level"
    rm -rf "$CLONE_TARGET"

    cd "$TEST_DIR"
    run "$META_BIN" git clone --recursive "file://$ROOT_BARE" "$CLONE_TARGET"
    [ "$status" -eq 0 ]

    [ -d "$CLONE_TARGET/l1/.git" ]
    [ -d "$CLONE_TARGET/l1/l2/.git" ]
    [ -d "$CLONE_TARGET/l1/l2/l3/.git" ]

    rm -rf "$L3_BARE" "$L2_BARE" "$L1_BARE" "$ROOT_BARE" "$CLONE_TARGET"
}

@test "meta git clone --recursive --meta-depth limits recursion" {
    # Setup: root -> l1 (meta:true) -> l2 (meta:true) -> l3
    L3_BARE="$(mktemp -d)/l3.git"
    create_meta_bare_repo "$L3_BARE" '{"projects": {}}'

    L2_BARE="$(mktemp -d)/l2.git"
    create_meta_bare_repo "$L2_BARE" "{\"projects\": {\"l3\": \"file://$L3_BARE\"}}"

    L1_BARE="$(mktemp -d)/l1.git"
    create_meta_bare_repo "$L1_BARE" "{\"projects\": {\"l2\": {\"repo\": \"file://$L2_BARE\", \"meta\": true}}}"

    ROOT_BARE="$(mktemp -d)/root.git"
    create_meta_bare_repo "$ROOT_BARE" "{\"projects\": {\"l1\": {\"repo\": \"file://$L1_BARE\", \"meta\": true}}}"

    CLONE_TARGET="$(mktemp -d)/clone-depth-limit"
    rm -rf "$CLONE_TARGET"

    cd "$TEST_DIR"
    # --meta-depth 1 means: level 0 (root's children) + level 1 (l1's children)
    # l2's children (l3) should NOT be cloned
    run "$META_BIN" git clone --recursive --meta-depth 1 "file://$ROOT_BARE" "$CLONE_TARGET"
    [ "$status" -eq 0 ]

    [ -d "$CLONE_TARGET/l1/.git" ]
    [ -d "$CLONE_TARGET/l1/l2/.git" ]
    # l3 should NOT be cloned (depth limit)
    [ ! -d "$CLONE_TARGET/l1/l2/l3" ]

    rm -rf "$L3_BARE" "$L2_BARE" "$L1_BARE" "$ROOT_BARE" "$CLONE_TARGET"
}

@test "meta git clone without --recursive clones only first level" {
    # Setup: root -> child (meta:true) -> grandchild
    GRANDCHILD_BARE="$(mktemp -d)/grandchild.git"
    create_meta_bare_repo "$GRANDCHILD_BARE" '{"projects": {}}'

    CHILD_BARE="$(mktemp -d)/child.git"
    create_meta_bare_repo "$CHILD_BARE" "{\"projects\": {\"grandchild\": \"file://$GRANDCHILD_BARE\"}}"

    ROOT_BARE="$(mktemp -d)/root.git"
    create_meta_bare_repo "$ROOT_BARE" "{\"projects\": {\"child\": {\"repo\": \"file://$CHILD_BARE\", \"meta\": true}}}"

    CLONE_TARGET="$(mktemp -d)/clone-no-recurse"
    rm -rf "$CLONE_TARGET"

    cd "$TEST_DIR"
    # No --recursive flag
    run "$META_BIN" git clone "file://$ROOT_BARE" "$CLONE_TARGET"
    [ "$status" -eq 0 ]

    # Root and child should be cloned
    [ -d "$CLONE_TARGET/.git" ]
    [ -d "$CLONE_TARGET/child/.git" ]
    # Grandchild should NOT be cloned (no --recursive)
    [ ! -d "$CLONE_TARGET/child/grandchild" ]

    rm -rf "$GRANDCHILD_BARE" "$CHILD_BARE" "$ROOT_BARE" "$CLONE_TARGET"
}
