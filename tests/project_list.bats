#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# Integration tests for `meta project list` / `meta project ls`

setup() {
    clear_git_local_env
    # Build binaries if not already built
    META_BIN="$BATS_TEST_DIRNAME/../target/debug/meta"
    META_PROJECT_BIN="$BATS_TEST_DIRNAME/../target/debug/meta-project"

    if [ ! -f "$META_BIN" ] || [ ! -f "$META_PROJECT_BIN" ]; then
        cargo build --workspace --quiet
    fi

    # Create a temp directory for each test
    TEST_DIR="$(mktemp -d)"
    mkdir -p "$TEST_DIR/.meta/plugins"
    cp "$META_PROJECT_BIN" "$TEST_DIR/.meta/plugins/meta-project"
    chmod +x "$TEST_DIR/.meta/plugins/meta-project"

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

    # Create project directories
    mkdir -p "$TEST_DIR/frontend"
    mkdir -p "$TEST_DIR/backend"
    mkdir -p "$TEST_DIR/shared"

    cd "$TEST_DIR"
}

teardown() {
    rm -rf "$TEST_DIR"
}

@test "meta project list shows all projects" {
    run "$META_BIN" project list
    [ "$status" -eq 0 ]
    [[ "$output" == *"frontend"* ]]
    [[ "$output" == *"backend"* ]]
    [[ "$output" == *"shared"* ]]
}

@test "meta project ls is an alias for list" {
    run "$META_BIN" project ls
    [ "$status" -eq 0 ]
    [[ "$output" == *"frontend"* ]]
    [[ "$output" == *"backend"* ]]
    [[ "$output" == *"shared"* ]]
}

@test "meta project list --json outputs valid JSON" {
    run "$META_BIN" project list --json
    [ "$status" -eq 0 ]
    # Validate it's parseable JSON
    echo "$output" | python3 -c "import sys, json; json.load(sys.stdin)"
}

@test "meta project list --json contains project data" {
    run "$META_BIN" project list --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import sys, json
data = json.load(sys.stdin)
names = [p['name'] for p in data['projects']]
assert 'frontend' in names, f'frontend not in {names}'
assert 'backend' in names, f'backend not in {names}'
assert 'shared' in names, f'shared not in {names}'
"
}

@test "meta project list --json includes repo URLs" {
    run "$META_BIN" project list --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import sys, json
data = json.load(sys.stdin)
repos = {p['name']: p['repo'] for p in data['projects']}
assert repos['frontend'] == 'git@github.com:org/frontend.git'
assert repos['backend'] == 'git@github.com:org/backend.git'
"
}

@test "meta project list --recursive discovers nested projects" {
    # Create a nested .meta inside frontend
    cat > "$TEST_DIR/frontend/.meta" <<'EOF'
{
    "projects": {
        "frontend-lib": "git@github.com:org/frontend-lib.git"
    }
}
EOF
    mkdir -p "$TEST_DIR/frontend/frontend-lib"

    run "$META_BIN" --recursive project list
    [ "$status" -eq 0 ]
    [[ "$output" == *"frontend"* ]]
    [[ "$output" == *"frontend-lib"* ]]
}

@test "meta project list --recursive --json includes nested projects" {
    cat > "$TEST_DIR/frontend/.meta" <<'EOF'
{
    "projects": {
        "frontend-lib": "git@github.com:org/frontend-lib.git"
    }
}
EOF
    mkdir -p "$TEST_DIR/frontend/frontend-lib"

    run "$META_BIN" --recursive project list --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import sys, json
data = json.load(sys.stdin)
# Find the frontend project
frontend = next(p for p in data['projects'] if p['name'] == 'frontend')
assert frontend.get('is_meta') == True, 'frontend should be marked as meta'
assert len(frontend.get('projects', [])) > 0, 'frontend should have sub-projects'
child = frontend['projects'][0]
assert child['name'] == 'frontend-lib'
"
}

@test "meta project list with extended format shows tags" {
    cat > "$TEST_DIR/.meta.json" <<'EOF'
{
    "projects": {
        "frontend": {
            "repo": "git@github.com:org/frontend.git",
            "tags": ["ui", "react"]
        },
        "backend": {
            "repo": "git@github.com:org/backend.git",
            "tags": ["api"]
        }
    }
}
EOF

    run "$META_BIN" project list --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import sys, json
data = json.load(sys.stdin)
frontend = next(p for p in data['projects'] if p['name'] == 'frontend')
assert 'ui' in frontend['tags']
assert 'react' in frontend['tags']
backend = next(p for p in data['projects'] if p['name'] == 'backend')
assert 'api' in backend['tags']
"
}

@test "meta project list with no .meta file shows error" {
    rm "$TEST_DIR/.meta.json"
    run "$META_BIN" project list
    [ "$status" -ne 0 ] || [[ "$output" == *"No .meta"* ]] || [[ "$output" == *"error"* ]]
}

@test "meta project list with YAML config" {
    rm "$TEST_DIR/.meta.json"
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
projects:
  frontend:
    repo: "git@github.com:org/frontend.git"
    tags: ["ui"]
  backend:
    repo: "git@github.com:org/backend.git"
EOF

    run "$META_BIN" project list --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import sys, json
data = json.load(sys.stdin)
names = [p['name'] for p in data['projects']]
assert 'frontend' in names
assert 'backend' in names
"
}

@test "meta project list handles nested meta repos" {
    # Verify nested meta repo entries (repo + meta: true) are handled correctly
    cat > "$TEST_DIR/.meta.json" <<'EOF'
{
    "projects": {
        "backend": "git@github.com:org/backend.git",
        "vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}
    }
}
EOF
    mkdir -p "$TEST_DIR/vendor"

    run "$META_BIN" project list
    [ "$status" -eq 0 ]
    [[ "$output" == *"vendor"* ]]
    [[ "$output" == *"backend"* ]]
}

@test "meta project list --json includes nested meta repos" {
    cat > "$TEST_DIR/.meta.json" <<'EOF'
{
    "projects": {
        "backend": "git@github.com:org/backend.git",
        "vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}
    }
}
EOF
    mkdir -p "$TEST_DIR/vendor"

    run "$META_BIN" project list --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import sys, json
data = json.load(sys.stdin)
names = [p['name'] for p in data['projects']]
assert 'vendor' in names, f'vendor not in {names}'
assert 'backend' in names, f'backend not in {names}'
# vendor should have a repo (nested meta repos are regular repos that happen to contain .meta)
vendor = next(p for p in data['projects'] if p['name'] == 'vendor')
assert vendor.get('repo') == 'git@github.com:org/vendor.git', f'vendor should have repo: {vendor}'
"
}

@test "meta project list --recursive shows nested meta repo children" {
    # Setup: vendor is a nested meta repo with nested-lib inside
    cat > "$TEST_DIR/.meta.json" <<'EOF'
{
    "projects": {
        "backend": "git@github.com:org/backend.git",
        "vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}
    }
}
EOF
    mkdir -p "$TEST_DIR/vendor/nested-lib"
    cat > "$TEST_DIR/vendor/.meta" <<'EOF'
{"projects": {"nested-lib": "git@github.com:org/nested-lib.git"}}
EOF

    run "$META_BIN" project list --recursive
    [ "$status" -eq 0 ]
    # Should show vendor's child project
    [[ "$output" == *"nested-lib"* ]]
    [[ "$output" == *"vendor"* ]]
    [[ "$output" == *"backend"* ]]
}

@test "meta project list --recursive from nested meta-repo shows root tree" {
    # Create a nested .meta inside frontend
    cat > "$TEST_DIR/frontend/.meta" <<'EOF'
{
    "projects": {
        "frontend-lib": "git@github.com:org/frontend-lib.git"
    }
}
EOF
    mkdir -p "$TEST_DIR/frontend/frontend-lib"

    # Run from inside the nested meta-repo (frontend/)
    cd "$TEST_DIR/frontend"
    run "$META_BIN" --recursive project list --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import sys, json
data = json.load(sys.stdin)
# Should show ROOT's projects (frontend, backend, shared), not frontend's
names = [p['name'] for p in data['projects']]
assert 'frontend' in names, f'frontend not in root projects: {names}'
assert 'backend' in names, f'backend not in root projects: {names}'
assert 'shared' in names, f'shared not in root projects: {names}'
# cwd field should be present and absolute
assert 'cwd' in data, 'cwd field missing from output'
assert data['cwd'].startswith('/'), f'cwd should be absolute: {data[\"cwd\"]}'
"
}

@test "meta project list --json includes cwd field" {
    run "$META_BIN" project list --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import sys, json
data = json.load(sys.stdin)
assert 'cwd' in data, 'cwd field missing from JSON output'
cwd = data['cwd']
assert cwd.startswith('/'), f'cwd should be absolute path: {cwd}'
"
}

@test "meta project list --recursive --json shows nested paths" {
    # Setup: vendor is a nested meta repo with nested-lib inside
    cat > "$TEST_DIR/.meta.json" <<'EOF'
{
    "projects": {
        "backend": "git@github.com:org/backend.git",
        "vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}
    }
}
EOF
    mkdir -p "$TEST_DIR/vendor/nested-lib"
    cat > "$TEST_DIR/vendor/.meta" <<'EOF'
{"projects": {"nested-lib": "git@github.com:org/nested-lib.git"}}
EOF

    run "$META_BIN" project list --recursive --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import sys, json
data = json.load(sys.stdin)
# Find vendor node
vendor = next((p for p in data['projects'] if p['name'] == 'vendor'), None)
assert vendor is not None, 'vendor not found'
# Verify vendor has nested projects
assert 'projects' in vendor, f'vendor should have projects: {vendor}'
nested = vendor['projects']
assert len(nested) > 0, f'vendor should have nested projects: {vendor}'
# Verify nested-lib is in the nested projects
nested_lib = next((p for p in nested if p['name'] == 'nested-lib'), None)
assert nested_lib is not None, f'nested-lib not found in vendor projects: {nested}'
"
}
