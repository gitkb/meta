#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# Integration tests for `meta context`

setup() {
    clear_git_local_env
    META_BIN="$BATS_TEST_DIRNAME/../target/debug/meta"

    if [ ! -f "$META_BIN" ]; then
        cargo build --workspace --quiet
    fi

    TEST_DIR="$(mktemp -d)"

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

    cd "$TEST_DIR"
}

teardown() {
    cd /
    rm -rf "$TEST_DIR"
}

# ============ Default (markdown) output ============

@test "context shows workspace name and repo count" {
    run "$META_BIN" context
    [ "$status" -eq 0 ]
    [[ "$output" == *"Meta Workspace"* ]]
    [[ "$output" == *"2 repos"* ]]
}

@test "context shows repo names" {
    run "$META_BIN" context
    [ "$status" -eq 0 ]
    [[ "$output" == *"backend"* ]]
    [[ "$output" == *"frontend"* ]]
}

@test "context shows branch info" {
    run "$META_BIN" context
    [ "$status" -eq 0 ]
    # Both repos should be on main or master
    [[ "$output" == *"main"* ]] || [[ "$output" == *"master"* ]]
}

@test "context shows clean status for clean repos" {
    run "$META_BIN" context
    [ "$status" -eq 0 ]
    [[ "$output" == *"clean"* ]]
}

@test "context shows modified count for dirty repos" {
    # Make backend dirty
    echo "change" >> "$TEST_DIR/backend/README.md"

    run "$META_BIN" context
    [ "$status" -eq 0 ]
    [[ "$output" == *"modified"* ]]
}

@test "context shows key commands section" {
    run "$META_BIN" context
    [ "$status" -eq 0 ]
    [[ "$output" == *"Key Commands"* ]]
    [[ "$output" == *"meta git status"* ]]
    [[ "$output" == *"meta exec"* ]]
}

# ============ --no-status ============

@test "context --no-status omits branch and status" {
    run "$META_BIN" context --no-status
    [ "$status" -eq 0 ]
    [[ "$output" == *"backend"* ]]
    [[ "$output" == *"frontend"* ]]
    # Should show simple list (no table with Branch/Status columns)
    [[ "$output" != *"| Branch |"* ]]
}

# ============ --json ============

@test "context --json outputs valid JSON" {
    run "$META_BIN" context --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "import sys, json; json.load(sys.stdin)"
}

@test "context --json contains workspace name and repo count" {
    run "$META_BIN" context --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import sys, json
data = json.load(sys.stdin)
assert 'name' in data, 'missing name'
assert data['repo_count'] == 2, f'expected 2 repos, got {data[\"repo_count\"]}'
"
}

@test "context --json contains repo details" {
    run "$META_BIN" context --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import sys, json
data = json.load(sys.stdin)
repos = data['repos']
names = [r['name'] for r in repos]
assert 'backend' in names, f'backend not in {names}'
assert 'frontend' in names, f'frontend not in {names}'
for r in repos:
    assert 'branch' in r, f'missing branch for {r[\"name\"]}'
    assert 'dirty' in r, f'missing dirty for {r[\"name\"]}'
"
}

@test "context --json --no-status omits branch and dirty" {
    run "$META_BIN" context --json --no-status
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import sys, json
data = json.load(sys.stdin)
for r in data['repos']:
    assert 'branch' not in r, f'branch should be omitted for {r[\"name\"]}'
    assert 'dirty' not in r, f'dirty should be omitted for {r[\"name\"]}'
"
}

# ============ Tags ============

@test "context shows tags when present" {
    cat > "$TEST_DIR/.meta.json" <<'EOF'
{
    "projects": {
        "backend": {
            "repo": "git@github.com:org/backend.git",
            "tags": ["api", "rust"]
        },
        "frontend": "git@github.com:org/frontend.git"
    }
}
EOF

    run "$META_BIN" context
    [ "$status" -eq 0 ]
    [[ "$output" == *"Tags"* ]]
    [[ "$output" == *"api"* ]]
    [[ "$output" == *"rust"* ]]
}

# ============ Dependencies ============

@test "context shows dependencies when present" {
    cat > "$TEST_DIR/.meta.json" <<'EOF'
{
    "projects": {
        "backend": {
            "repo": "git@github.com:org/backend.git",
            "depends_on": ["frontend"]
        },
        "frontend": "git@github.com:org/frontend.git"
    }
}
EOF

    run "$META_BIN" context
    [ "$status" -eq 0 ]
    [[ "$output" == *"Dependencies"* ]]
}

@test "context --json includes dependencies when present" {
    cat > "$TEST_DIR/.meta.json" <<'EOF'
{
    "projects": {
        "backend": {
            "repo": "git@github.com:org/backend.git",
            "depends_on": ["frontend"]
        },
        "frontend": "git@github.com:org/frontend.git"
    }
}
EOF

    run "$META_BIN" context --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import sys, json
data = json.load(sys.stdin)
assert 'dependencies' in data, 'missing dependencies'
assert 'backend' in data['dependencies'], 'backend not in dependencies'
"
}

# ============ Error cases ============

@test "context outside meta workspace fails gracefully" {
    local empty_dir
    empty_dir="$(mktemp -d)"
    cd "$empty_dir"
    run "$META_BIN" context
    rm -rf "$empty_dir"
    [ "$status" -ne 0 ]
    [[ "$output" == *"meta"* ]] || [[ "$output" == *"config"* ]] || [[ "$output" == *"workspace"* ]]
}
