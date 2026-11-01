#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# Integration tests for `meta context`

setup() {
    clear_git_local_env
    isolate_user_env
    META_BIN="$BATS_TEST_DIRNAME/../target/debug/meta"

    if [ ! -f "$META_BIN" ]; then
        cargo build --workspace --quiet
    fi

    TEST_DIR="$(cd "$(mktemp -d)" && pwd -P)"

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
    for key in ('branch', 'dirty', 'detached', 'head', 'untracked_count', 'upstream'):
        assert key not in r, f'{key} should be omitted for {r[\"name\"]}'
"
}

# ============ Tags ============

@test "context shows tags when present" {
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
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
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
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
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
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

@test "context shows only the projects --tag or --include select" {
    cat > "$TEST_DIR/.meta.yaml" <<'YAML'
projects:
  backend:
    repo: git@github.com:org/backend.git
    tags: [api]
  frontend:
    repo: git@github.com:org/frontend.git
    tags: [ui]
YAML
    run "$META_BIN" -t api context --json
    [ "$status" -eq 0 ]
    [ "$(python3 -c 'import json,sys; print(",".join(r["name"] for r in json.load(sys.stdin)["repos"]))' <<<"$output")" = "backend" ]
    run "$META_BIN" -i frontend context
    [ "$status" -eq 0 ]
    [[ "$output" == *"frontend"* ]]
    [[ "$output" != *"| backend"* ]]
    run "$META_BIN" -t nope context
    [ "$status" -ne 0 ]
    [[ "$output" == *"--tag 'nope' is no project's tag"* ]]
}

@test "context refuses --recursive instead of ignoring it" {
    run "$META_BIN" -r context
    [ "$status" -ne 0 ]
    [[ "$output" == *"does not take --recursive"* ]]
    run "$META_BIN" --depth 1 context
    [ "$status" -ne 0 ]
    [[ "$output" == *"or --depth"* ]]
}

# ============ Always read fresh ============

@test "context shows each change right after the last call, and keeps nothing" {
    git init --quiet --bare "$TEST_DIR/backend.git"
    git -C backend remote add origin "$TEST_DIR/backend.git"
    git -C backend push --quiet -u origin HEAD
    git clone --quiet "$TEST_DIR/backend.git" "$TEST_DIR/mate"
    run "$META_BIN" context --json
    [ "$status" -eq 0 ]
    # an edit, a staged new file, a fetched commit and a new project: none
    # of them moves a HEAD or a branch, which was all a cached context saw
    echo change >> backend/README.md
    echo new > frontend/new
    git -C frontend add new
    echo theirs > mate/theirs
    git -C mate add theirs
    git -C mate -c user.name=T -c user.email=t@t commit --quiet -m theirs
    git -C mate push --quiet
    git -C backend fetch --quiet
    printf 'projects:\n  backend: git@github.com:org/backend.git\n  frontend: git@github.com:org/frontend.git\n  extra: git@github.com:org/extra.git\n' > .meta.yaml
    run "$META_BIN" context --json
    [ "$status" -eq 0 ]
    python3 -c '
import json, sys
repos = {r["name"]: r for r in json.load(sys.stdin)["repos"]}
assert repos["backend"]["dirty"] and repos["backend"]["behind"] == 1, repos["backend"]
assert repos["frontend"]["dirty"], repos["frontend"]
assert "extra" in repos, repos
' <<<"$output"
    git -C frontend switch --quiet -c topic
    run "$META_BIN" context
    [[ "$output" == *"| frontend | topic | 1 modified, no upstream |"* ]]
    [ -z "$(ls -A "$META_DATA_DIR")" ]
}

# ============ What git says about each repo ============

@test "context names stopped rebases, detached heads, missing and gone upstreams, and untracked files" {
    git init --quiet --bare "$TEST_DIR/remote.git"
    for repo in backend frontend; do
        git -C "$repo" branch --quiet -M main
        git -C "$repo" remote add origin "$TEST_DIR/remote.git"
        git -C "$repo" push --quiet origin "main:$repo"
        git -C "$repo" branch --quiet -u "origin/$repo"
    done
    # backend: a rebase stopped on a conflict, then an untracked file
    git -C backend switch --quiet -c base
    echo theirs > backend/README.md
    git -C backend commit --quiet -am theirs
    git -C backend switch --quiet main
    echo mine > backend/README.md
    git -C backend commit --quiet -am mine
    run git -C backend rebase base
    [ "$status" -ne 0 ]
    echo note > backend/notes.txt
    # frontend: its upstream deleted and pruned
    git -C frontend push --quiet origin --delete frontend
    git -C frontend fetch --quiet --prune
    run "$META_BIN" context
    [ "$status" -eq 0 ]
    [[ "$output" == *"| backend | (rebasing main) | rebase in progress, 1 modified, 1 untracked |"* ]]
    [[ "$output" == *"| frontend | main | clean, upstream gone |"* ]]
    run "$META_BIN" context --json
    [ "$status" -eq 0 ]
    python3 -c '
import json, sys
repos = {r["name"]: r for r in json.load(sys.stdin)["repos"]}
b, f = repos["backend"], repos["frontend"]
assert (b["operation"], b["rebasing"], b["detached"]) == ("rebase", "main", True), b
assert "branch" not in b and len(b["head"]) >= 40, b
assert (b["modified_count"], b["untracked_count"]) == (2, 1), b
assert (f["upstream"], f["upstream_gone"], f["detached"]) == ("origin/frontend", True, False), f
' <<<"$output"

    git -C backend rebase --abort
    git -C backend switch --quiet --detach HEAD
    git -C frontend switch --quiet -c local
    run "$META_BIN" context
    [[ "$output" =~ "| backend | (detached "[0-9a-f]{7}") | 1 untracked |" ]]
    [[ "$output" == *"| frontend | local | clean, no upstream |"* ]]
    run "$META_BIN" context --json
    python3 -c '
import json, sys
repos = {r["name"]: r for r in json.load(sys.stdin)["repos"]}
b, f = repos["backend"], repos["frontend"]
assert b["detached"] and "operation" not in b and "upstream" not in b, b
assert not f["detached"] and "upstream" not in f and "upstream_gone" not in f, f
' <<<"$output"
}
