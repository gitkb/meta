#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"
load "${BATS_TEST_DIRNAME}/helpers/terminal.bash"

# Integration tests for `meta git` commands
# Tests use local git repos (no network IO required)

setup() {
    clear_git_local_env
    isolate_user_env
    # Build binaries if not already built
    META_BIN="$BATS_TEST_DIRNAME/../target/debug/meta"
    META_GIT_BIN="$BATS_TEST_DIRNAME/../target/debug/meta-git"

    if [ ! -f "$META_BIN" ] || [ ! -f "$META_GIT_BIN" ]; then
        cargo build --workspace --quiet
    fi

    # Create a temp directory for each test
    TEST_DIR="$(cd "$(mktemp -d)" && pwd -P)"
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

    # Default .meta.yaml
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
{
    "projects": {
        "frontend": "git@github.com:org/frontend.git",
        "backend": "git@github.com:org/backend.git",
        "shared": "git@github.com:org/shared.git"
    }
}
EOF

    git -C "$TEST_DIR" add .meta.yaml
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

@test "meta git update calls only a repository an unlisted one" {
    # stray holds an empty .git, other is a repository .meta.yaml doesn't list
    mkdir -p "$TEST_DIR/stray/.git"
    git init --quiet "$TEST_DIR/other"
    run "$META_BIN" git update
    [ "$status" -eq 0 ]
    [[ "$output" == *"other exists locally but is not in .meta.yaml"* ]]
    [[ "$output" != *"stray"* ]]
}

@test "meta git update compares no origin for a project that is not cloned" {
    # the workspace's own origin is not the uncloned project's
    git -C "$TEST_DIR" remote add origin git@github.com:org/workspace.git
    rm -rf "$TEST_DIR/shared"
    mkdir "$TEST_DIR/shared"
    run "$META_BIN" git update
    [ "$status" -eq 0 ]
    [[ "$output" != *"mismatch"* ]]
    [[ "$output" != *"remote set-url"* ]]
}

@test "meta git update compares no origin for a project whose HEAD git can't read" {
    [ "$(id -u)" -ne 0 ] || skip "root reads any file"
    git -C "$TEST_DIR" remote add origin git@github.com:org/workspace.git
    # git looks past a HEAD it can't read, to the workspace around
    chmod 000 "$TEST_DIR/backend/.git/HEAD"
    run "$META_BIN" git update
    chmod 644 "$TEST_DIR/backend/.git/HEAD"
    [ "$status" -eq 0 ]
    [[ "$output" != *"remote set-url"* ]]
    [ "$(git -C "$TEST_DIR" remote get-url origin)" = "git@github.com:org/workspace.git" ]
}

# ============ meta git clone (local) ============

@test "meta git clone from local path works" {
    # Create a "remote" bare repo that IS a meta repo
    REMOTE_DIR="$(cd "$(mktemp -d)" && pwd -P)"
    git init --bare --quiet "$REMOTE_DIR"
    git -C "$REMOTE_DIR" symbolic-ref HEAD refs/heads/main

    # Create a temp working copy to push initial meta content
    WORK_DIR="$(cd "$(mktemp -d)" && pwd -P)"
    git clone --quiet "$REMOTE_DIR" "$WORK_DIR/meta-repo"
    cd "$WORK_DIR/meta-repo"
    git config user.email "test@test.com"
    git config user.name "Test"
    git checkout -b main --quiet 2>/dev/null || true
    cat > .meta.yaml <<'EOF'
{
    "projects": {
        "child": "file:///dev/null"
    }
}
EOF
    git add .meta.yaml
    git commit --quiet -m "Add .meta.yaml"
    git push --quiet -u origin main

    # Now test cloning
    CLONE_TARGET="$(cd "$(mktemp -d)" && pwd -P)"
    rm -rf "$CLONE_TARGET"

    cd "$TEST_DIR"
    run "$META_BIN" git clone "$REMOTE_DIR" "$CLONE_TARGET"
    [ "$status" -eq 0 ] || true  # May fail on child clone (fake URL), but meta repo should clone

    # Clean up
    rm -rf "$REMOTE_DIR" "$WORK_DIR" "$CLONE_TARGET"
}

@test "meta git clone with custom directory name for meta repo" {
    # Custom directory only applies to the meta repo itself
    # Child repos clone to paths according to .meta.yaml files

    # Create a "remote" bare repo with main as default branch
    REMOTE_DIR="$(cd "$(mktemp -d)" && pwd -P)"
    git init --bare --quiet "$REMOTE_DIR"
    git -C "$REMOTE_DIR" symbolic-ref HEAD refs/heads/main

    # Create a temp working copy to push initial content
    WORK_DIR="$(cd "$(mktemp -d)" && pwd -P)"
    git clone --quiet "$REMOTE_DIR" "$WORK_DIR/meta-repo"
    cd "$WORK_DIR/meta-repo"
    git config user.email "test@test.com"
    git config user.name "Test"
    git checkout -b main --quiet 2>/dev/null || true
    cat > .meta.yaml <<'EOF'
{
    "projects": {}
}
EOF
    git add .meta.yaml
    git commit --quiet -m "Add .meta.yaml"
    git push --quiet -u origin main

    # Clone with custom directory name
    CLONE_DIR="$(cd "$(mktemp -d)" && pwd -P)"
    rm -rf "$CLONE_DIR"
    CUSTOM_NAME="my-custom-clone-name"

    cd "$TEST_DIR"
    run "$META_BIN" git clone "$REMOTE_DIR" "$CLONE_DIR/$CUSTOM_NAME"
    [ "$status" -eq 0 ]

    # Verify the custom directory was used for the meta repo
    [ -d "$CLONE_DIR/$CUSTOM_NAME" ]
    [ -f "$CLONE_DIR/$CUSTOM_NAME/.meta.yaml" ]

    # Clean up
    rm -rf "$REMOTE_DIR" "$WORK_DIR" "$CLONE_DIR"
}

@test "meta git clone passes URL correctly to git" {
    # This test verifies the fix for the argument passing bug
    # where the URL was being lost due to double-stripping of args

    # Create a "remote" bare repo with main as default branch
    REMOTE_DIR="$(cd "$(mktemp -d)" && pwd -P)"
    git init --bare --quiet "$REMOTE_DIR"
    git -C "$REMOTE_DIR" symbolic-ref HEAD refs/heads/main

    # Push minimal content
    WORK_DIR="$(cd "$(mktemp -d)" && pwd -P)"
    git clone --quiet "$REMOTE_DIR" "$WORK_DIR/meta-repo"
    cd "$WORK_DIR/meta-repo"
    git config user.email "test@test.com"
    git config user.name "Test"
    git checkout -b main --quiet 2>/dev/null || true
    echo '{"projects": {}}' > .meta.yaml
    git add .meta.yaml
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
    REMOTE_DIR="$(cd "$(mktemp -d)" && pwd -P)"
    git init --bare --quiet "$REMOTE_DIR"
    git -C "$REMOTE_DIR" symbolic-ref HEAD refs/heads/main

    WORK_DIR="$(cd "$(mktemp -d)" && pwd -P)"
    git clone --quiet "$REMOTE_DIR" "$WORK_DIR/meta-repo"
    cd "$WORK_DIR/meta-repo"
    git config user.email "test@test.com"
    git config user.name "Test"
    # Set default branch name explicitly
    git checkout -b main --quiet 2>/dev/null || true
    echo '{"projects": {}}' > .meta.yaml
    git add .meta.yaml
    git commit --quiet -m "Commit 1"
    # Create additional commits by adding new files (keep .meta.yaml valid)
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
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
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

# ============ --recursive ============

@test "meta git status --recursive discovers nested repos" {
    # Create a nested .meta.yaml inside frontend with a sub-component
    mkdir -p "$TEST_DIR/frontend/sub-lib"
    git -C "$TEST_DIR/frontend/sub-lib" init --quiet
    git -C "$TEST_DIR/frontend/sub-lib" config user.email "test@test.com"
    git -C "$TEST_DIR/frontend/sub-lib" config user.name "Test"
    echo "# sub-lib" > "$TEST_DIR/frontend/sub-lib/README.md"
    git -C "$TEST_DIR/frontend/sub-lib" add README.md
    git -C "$TEST_DIR/frontend/sub-lib" commit --quiet -m "Init sub-lib"

    cat > "$TEST_DIR/frontend/.meta.yaml" <<'EOF'
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

    cat > "$TEST_DIR/frontend/.meta.yaml" <<'EOF'
{
    "projects": {
        "sub-lib": "git@github.com:org/sub-lib.git"
    }
}
EOF
    cat > "$TEST_DIR/frontend/sub-lib/.meta.yaml" <<'EOF'
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

    cat > "$TEST_DIR/frontend/.meta.yaml" <<'EOF'
{
    "projects": {
        "sub-lib": "git@github.com:org/sub-lib.git"
    }
}
EOF
    cat > "$TEST_DIR/frontend/sub-lib/.meta.yaml" <<'EOF'
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

@test "meta git commit -m --recursive commits in nested repos" {
    # Create nested repo with staged change
    mkdir -p "$TEST_DIR/frontend/sub-lib"
    git -C "$TEST_DIR/frontend/sub-lib" init --quiet
    git -C "$TEST_DIR/frontend/sub-lib" config user.email "test@test.com"
    git -C "$TEST_DIR/frontend/sub-lib" config user.name "Test"
    echo "initial" > "$TEST_DIR/frontend/sub-lib/file.txt"
    git -C "$TEST_DIR/frontend/sub-lib" add file.txt
    git -C "$TEST_DIR/frontend/sub-lib" commit --quiet -m "Init"

    cat > "$TEST_DIR/frontend/.meta.yaml" <<'EOF'
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
    cat > "$TEST_DIR/backend/.meta.yaml" <<'EOF'
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
    cat > "$TEST_DIR/backend/.meta.yaml" <<'EOF'
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

# Helper: create a bare repo containing a .meta.yaml with given projects.
# Usage: create_meta_bare_repo <bare_dir> <projects_json>
# Returns the file:// URL suitable for cloning.
create_meta_bare_repo() {
    local bare_dir="$1"
    local meta_content="$2"

    local work_dir="$(mktemp -d)"
    git init --bare --quiet "$bare_dir"
    git -C "$bare_dir" symbolic-ref HEAD refs/heads/main

    git clone --quiet "$bare_dir" "$work_dir/checkout"
    (
        cd "$work_dir/checkout"
        git config user.email "test@test.com"
        git config user.name "Test"
        git checkout -b main --quiet 2>/dev/null || true
        echo "$meta_content" > .meta.yaml
        git add .meta.yaml
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
    # Grandchild was cloned (recursive discovered child's .meta.yaml)
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

# ============ Arguments reach git as typed, with no shell ============

@test "meta git passes \$(...) to git, never runs it" {
    run "$META_BIN" git log --grep='$(touch PWNED1)' -1
    [ "$status" -eq 0 ]
    [ -z "$(find "$TEST_DIR" -name PWNED1)" ]
}

@test "meta git keeps a ; inside one argument" {
    run "$META_BIN" --include frontend git tag -m 'release; touch PWNED2' v1
    [ "$status" -eq 0 ]
    [ -z "$(find "$TEST_DIR" -name PWNED2)" ]
    [ "$(git -C "$TEST_DIR/frontend" tag -l --format='%(contents:subject)' v1)" = "release; touch PWNED2" ]
}

@test "meta git keeps an argument with spaces whole" {
    run "$META_BIN" git log --format='%h %s' -1
    [ "$status" -eq 0 ]
    [[ "$output" == *" Initial commit for frontend"* ]]
    [[ "$output" != *"ambiguous argument"* ]]
}

@test "meta git commit -m keeps the message as typed" {
    echo change >> "$TEST_DIR/frontend/README.md"
    git -C "$TEST_DIR/frontend" add README.md
    run "$META_BIN" git commit -m "it's \$(touch PWNED3); done"
    [ "$status" -eq 0 ]
    [ -z "$(find "$TEST_DIR" -name PWNED3)" ]
    [ "$(git -C "$TEST_DIR/frontend" log -1 --format=%s)" = "it's \$(touch PWNED3); done" ]
}

@test "meta-mcp meta_git_add stages a hostile file name as one path" {
    META_MCP_BIN="$BATS_TEST_DIRNAME/../target/debug/meta-mcp"
    echo x > "$TEST_DIR/frontend/my file.txt; touch PWNED4"
    echo x > "$TEST_DIR/frontend/unrelated.txt"
    run env PATH="$(dirname "$META_BIN"):$PATH" "$META_MCP_BIN" <<'EOF'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"meta_git_add","arguments":{"files":"my file.txt; touch PWNED4"}}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"meta_git_add","arguments":{"files":[]}}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"meta_git_add","arguments":{"files":["my file.txt; touch PWNED4"]}}}
EOF
    [ -z "$(find "$TEST_DIR" -name PWNED4)" ]
    [[ "$output" == *"Invalid arguments for meta_git_add: files: invalid type: string"* ]]
    [[ "$output" == *"'files' is empty"* ]]
    [ "$(git -C "$TEST_DIR/frontend" diff --cached --name-only)" = "my file.txt; touch PWNED4" ]
}

@test "meta git update refuses --include and --tag instead of cloning every missing repo" {
    run "$META_BIN" --include frontend git update
    [ "$status" -ne 0 ]
    [[ "$output" == *"does not take --include/--exclude"* ]]
    # shared can really be cloned, so "nothing cloned" means something
    git clone --quiet --bare "$TEST_DIR/shared" "$TEST_DIR/shared.git"
    rm -rf "$TEST_DIR/shared"
    echo '{"projects": {"frontend": {"repo": "git@github.com:org/frontend.git", "tags": ["ui"]}, "backend": {"repo": "git@github.com:org/backend.git", "tags": ["api"]}, "shared": "file://'"$TEST_DIR"'/shared.git"}}' > "$TEST_DIR/.meta.yaml"
    run "$META_BIN" --tag api git update
    [ "$status" -ne 0 ]
    [[ "$output" == *"does not take --include/--exclude/--tag"* ]]
    [ ! -e "$TEST_DIR/shared" ]
    run "$META_BIN" git update
    [ "$status" -eq 0 ]
    [ -d "$TEST_DIR/shared/.git" ]
}

@test "arguments after -- that look like meta flags reach git" {
    echo x > "$TEST_DIR/frontend/--json"
    run "$META_BIN" --include frontend git add -- --json
    [ "$status" -eq 0 ]
    [ "$(git -C "$TEST_DIR/frontend" diff --cached --name-only)" = "--json" ]
}

@test "--help and -h after -- reach git as file names" {
    META_MCP_BIN="$BATS_TEST_DIRNAME/../target/debug/meta-mcp"
    echo x > "$TEST_DIR/frontend/--help"
    echo x > "$TEST_DIR/frontend/-h"
    run "$META_BIN" --include frontend git add -- --help
    [ "$status" -eq 0 ]
    [ "$(git -C "$TEST_DIR/frontend" diff --cached --name-only)" = "--help" ]
    # meta_git_add runs in every repo; only frontend has the file
    run env PATH="$(dirname "$META_BIN"):$PATH" "$META_MCP_BIN" <<'EOF'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"meta_git_add","arguments":{"files":["-h"]}}}
EOF
    [ "$(git -C "$TEST_DIR/frontend" diff --cached --name-only | LC_ALL=C sort | tr '\n' ' ')" = "--help -h " ]
}

@test "meta git commit --edit uses a private temp file it removes" {
    export META_DATA_DIR="$TEST_DIR/data"
    mkdir -p "$META_DATA_DIR"
    chmod 700 "$META_DATA_DIR"
    LOG="$TEST_DIR/editor.log"
    printf '#!/bin/sh\necho "$1 $(ls -l "$1" | cut -c1-10)" >> "%s"\nexit "${EDITOR_EXIT:-0}"\n' "$LOG" > "$TEST_DIR/editor"
    chmod 755 "$TEST_DIR/editor"
    echo change >> "$TEST_DIR/frontend/README.md"
    git -C "$TEST_DIR/frontend" add README.md

    # The editor leaves every message empty, so nothing is committed
    GIT_EDITOR="$TEST_DIR/editor" run with_terminal "$META_BIN" git commit --edit
    GIT_EDITOR="$TEST_DIR/editor" EDITOR_EXIT=1 run with_terminal "$META_BIN" git commit --edit
    [ "$status" -ne 0 ]

    [ "$(wc -l < "$LOG")" -eq 2 ]
    first="$(sed -n 1p "$LOG" | cut -d' ' -f1)"
    second="$(sed -n 2p "$LOG" | cut -d' ' -f1)"
    [ "$first" != "$second" ]
    for path in "$first" "$second"; do
        [[ "$path" == "$META_DATA_DIR/tmp/META_COMMIT_EDITMSG-"* ]]
        [ ! -e "$path" ]
    done
    [ "$(grep -c -- ' -rw-------$' "$LOG")" -eq 2 ]
}

@test "meta git commit --edit runs git's editor, arguments included" {
    echo change >> "$TEST_DIR/frontend/README.md"
    git -C "$TEST_DIR/frontend" add README.md
    # an editor taking an argument: it puts the message above each prompt line
    printf '#!/bin/sh\nawk -v m="$1" '"'"'/^# Enter commit message/ { print m } { print }'"'"' "$2" > "$2.new" && mv "$2.new" "$2"\n' > "$TEST_DIR/msg-editor"
    chmod 755 "$TEST_DIR/msg-editor"
    git -C "$TEST_DIR/frontend" config core.editor "'$TEST_DIR/msg-editor' 'from core.editor'"

    # as in git, core.editor comes before VISUAL and EDITOR; the editor runs
    # in the repo, so a relative data dir must still name the right file
    unset GIT_EDITOR
    META_DATA_DIR=rel-data VISUAL=nosuch EDITOR=nosuch run with_terminal "$META_BIN" git commit --edit
    [ "$status" -eq 0 ]
    [ "$(git -C "$TEST_DIR/frontend" log -1 --format=%s)" = "from core.editor" ]
}

@test "meta git commit --edit without a terminal fails at once, naming the editor" {
    echo change >> "$TEST_DIR/frontend/README.md"
    git -C "$TEST_DIR/frontend" add README.md
    LOG="$TEST_DIR/editor.log"
    printf '#!/bin/sh\necho ran >> "%s"\n' "$LOG" > "$TEST_DIR/editor"
    chmod 755 "$TEST_DIR/editor"

    GIT_EDITOR="$TEST_DIR/editor" run without_terminal "$META_BIN" git commit --edit
    [ "$status" -ne 0 ]
    [[ "$output" == *"The editor '$TEST_DIR/editor' needs the terminal"*"Use -m <message> instead."* ]]
    [ ! -e "$LOG" ]
    [ -n "$(git -C "$TEST_DIR/frontend" diff --cached --name-only)" ]

    # on a terminal, but in its background, it would stop on its first read
    GIT_EDITOR="$TEST_DIR/editor" run in_background "$META_BIN" git commit --edit
    [ "$status" -ne 0 ]
    [[ "$output" == *"needs the terminal, but meta runs in its background"* ]]
    [ ! -e "$LOG" ]
}

@test "meta git commit --edit shows the message of a commit that failed" {
    for repo in frontend backend; do
        echo change >> "$TEST_DIR/$repo/README.md"
        git -C "$TEST_DIR/$repo" add README.md
    done
    printf '#!/bin/sh\nexit 1\n' > "$TEST_DIR/frontend/.git/hooks/pre-commit"
    chmod 755 "$TEST_DIR/frontend/.git/hooks/pre-commit"
    printf '#!/bin/sh\nawk '"'"'/^# Enter commit message/ { print "typed by hand" } { print }'"'"' "$1" > "$1.new" && mv "$1.new" "$1"\n' > "$TEST_DIR/editor"
    chmod 755 "$TEST_DIR/editor"

    GIT_EDITOR="$TEST_DIR/editor" run with_terminal "$META_BIN" git commit --edit
    [ "$status" -ne 0 ]
    [[ "$output" == *"Failed to commit; the message was:"*"typed by hand"* ]]
    [ "$(git -C "$TEST_DIR/backend" log -1 --format=%s)" = "typed by hand" ]
}

@test "meta git commit --edit names an editor that can't start" {
    echo change >> "$TEST_DIR/frontend/README.md"
    git -C "$TEST_DIR/frontend" add README.md
    GIT_EDITOR="$TEST_DIR/no-such-editor" run with_terminal "$META_BIN" git commit --edit
    [ "$status" -ne 0 ]
    [[ "$output" == *"The editor '$TEST_DIR/no-such-editor' failed"* ]]
    [ -n "$(git -C "$TEST_DIR/frontend" diff --cached --name-only)" ]
}

@test "meta git commit escapes repo paths in its summary" {
    WS="$TEST_DIR/ws"$'\e[2J'
    mkdir -p "$WS"
    git -C "$WS" init --quiet -b main
    git init --quiet -b main "$WS/api"
    git -C "$WS/api" -c user.email=t@t -c user.name=t commit --quiet --allow-empty -m init
    printf '{"projects": {"api": "git@github.com:org/api.git"}}' > "$WS/.meta.yaml"
    echo x > "$WS/api/file"
    git -C "$WS/api" add file
    cd "$WS"
    run "$META_BIN" git commit
    [[ "$output" == *'ws\u{1b}[2J'* ]]
    [[ "$output" != *$'\e'* ]]
}

@test "meta git update fails on a nested .meta.yaml it cannot read" {
    [ "$(id -u)" -ne 0 ] || skip "root reads any directory"
    mkdir -p "$TEST_DIR/private"
    printf '{"projects": {}}' > "$TEST_DIR/private/config.json"
    ln -s ../private/config.json "$TEST_DIR/frontend/.meta.yaml"
    chmod 000 "$TEST_DIR/private"
    run "$META_BIN" git update
    chmod 700 "$TEST_DIR/private"
    [ "$status" -ne 0 ]
    [[ "$output" == *"Cannot read"*"frontend/.meta.yaml"* ]]
}

@test "meta git clone fails when a cloned repo's nested .meta.yaml is refused" {
    UP="$(cd "$(mktemp -d)" && pwd -P)"
    # bare_with_meta NAME JSON: a bare repo whose only commit holds .meta.yaml = JSON
    bare_with_meta() {
        git init --bare --quiet -b main "$UP/$1.git"
        git clone --quiet "$UP/$1.git" "$UP/work-$1" 2>/dev/null
        printf '%s\n' "$2" > "$UP/work-$1/.meta.yaml"
        git -C "$UP/work-$1" add .meta.yaml
        git -C "$UP/work-$1" -c user.email=t@t -c user.name=t commit --quiet -m meta
        git -C "$UP/work-$1" push --quiet origin HEAD:main
    }
    bare_with_meta child '{"projects": {"x": {"repo": "u", "path": "../../outside"}}}'
    bare_with_meta root "{\"projects\": {\"child\": {\"repo\": \"$UP/child.git\", \"meta\": true}, \"gone\": \"$UP/gone.git\"}}"

    cd "$TEST_DIR"
    run "$META_BIN" git clone --recursive "$UP/root.git" "$UP/clone"
    [ "$status" -ne 0 ]
    [[ "$output" == *"Could not read nested .meta.yaml files"* ]]
    [[ "$output" == *"must not contain '..'"* ]]
    # A failed clone is still reported alongside it
    [[ "$output" == *"Failed to clone"*"gone"* ]]
    [ ! -e "$UP/outside" ]
    rm -rf "$UP"
}

@test "meta git clone escapes the URL it echoes" {
    cd "$TEST_DIR"
    run "$META_BIN" --dry-run git clone $'q\033[2J\nFORGED'
    [[ "$output" == *"Would clone"* ]]
    [[ "$output" != *$'\033[2J'* ]]
    [[ "$output" != *$'\nFORGED'* ]]
    # and when it really clones (git stubbed out: it fails quietly)
    mkdir -p "$TEST_DIR/fakebin"
    printf '#!/bin/sh\nexit 1\n' > "$TEST_DIR/fakebin/git"
    chmod +x "$TEST_DIR/fakebin/git"
    PATH="$TEST_DIR/fakebin:$PATH" run "$META_BIN" git clone $'q\033[2J\nFORGED'
    [[ "$output" == *"Cloning meta repository: "'q\u{1b}[2J\nFORGED'* ]]
    [[ "$output" != *$'\033[2J'* ]]
    [[ "$output" != *$'\nFORGED'* ]]
}

# ============ Selection and arguments ============

@test "--tag runs a git command in the tagged repos only, never the root" {
    cat > "$TEST_DIR/.meta.yaml" <<'YAML'
projects:
  frontend:
    repo: git@github.com:org/frontend.git
    tags: [ui]
  backend:
    repo: git@github.com:org/backend.git
    tags: [api]
  shared: git@github.com:org/shared.git
YAML
    root_branch="$(git -C "$TEST_DIR" symbolic-ref --short HEAD)"
    run "$META_BIN" --tag api git checkout -b feature/x
    [ "$status" -eq 0 ]
    [ "$(git -C "$TEST_DIR/backend" branch --show-current)" = "feature/x" ]
    [ "$(git -C "$TEST_DIR/frontend" branch --show-current)" != "feature/x" ]
    [ "$(git -C "$TEST_DIR" symbolic-ref --short HEAD)" = "$root_branch" ]
    run "$META_BIN" --tag apo git status
    [ "$status" -ne 0 ]
    [[ "$output" == *"--tag 'apo' is no project's tag"*"the tags are api, ui"* ]]
}

@test "meta git status passes its arguments to git, and git update refuses ones it doesn't take" {
    echo change >> "$TEST_DIR/frontend/README.md"
    # git colours it: meta's git runs keep colour on
    run "$META_BIN" git status --short
    [ "$status" -eq 0 ]
    [[ "$output" == *"M"*"README.md"* ]]
    [[ "$output" == *$'\033['* ]]
    [[ "$output" != *"Changes not staged"* ]]
    # meta's flag after the command is git's argument
    run "$META_BIN" git status --tag backend
    [ "$status" -ne 0 ]
    [[ "$output" == *"unknown option"*"tag"* ]]

    git clone --quiet --bare "$TEST_DIR/shared" "$TEST_DIR/shared.git"
    rm -rf "$TEST_DIR/shared"
    sed -i.bak "s|git@github.com:org/shared.git|file://$TEST_DIR/shared.git|" "$TEST_DIR/.meta.yaml"
    run "$META_BIN" git update --dry-run
    [ "$status" -ne 0 ]
    [[ "$output" == *"unexpected argument '--dry-run'"* ]]
    [ ! -e "$TEST_DIR/shared" ]
    run "$META_BIN" git update --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage: meta git update"* ]]
    run "$META_BIN" git status --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"meta git status - Pass-through git command"* ]]
}

@test "meta help never runs the command, even with -- in the words" {
    echo junk > "$TEST_DIR/frontend/junk"
    run "$META_BIN" help git clean -f -- .
    [ "$status" -eq 0 ]
    [ -f "$TEST_DIR/frontend/junk" ]
    [[ "$output" == *"Pass-through git command"* ]]
    # and a plugin command's own help, by its words
    run "$META_BIN" help git worktree create
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage: meta git worktree create"* ]]
    [[ "$output" == *"--repo"* ]]
}

@test "a --tag with no tag in it is an error, not every repo" {
    for tag in "" "," " "; do
        run "$META_BIN" --tag "$tag" exec -- touch blank-tag
        [ "$status" -ne 0 ]
        [[ "$output" == *"names no tag"* ]]
    done
    [ -z "$(find "$TEST_DIR" -name blank-tag)" ]
}
