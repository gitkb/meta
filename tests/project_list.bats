#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# Integration tests for `meta project list` / `meta project ls`

setup() {
    sandbox
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
{
    "projects": {
        "frontend": "git@github.com:org/frontend.git",
        "backend": "git@github.com:org/backend.git",
        "shared": "git@github.com:org/shared.git"
    }
}
EOF
    make_workspace --init-only --plugins project frontend backend shared
    cd "$TEST_DIR"
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
    run "$META_BIN" --json project list
    [ "$status" -eq 0 ]
    # Validate it's parseable JSON
    echo "$output" | python3 -c "import sys, json; json.load(sys.stdin)"
}

@test "meta project list --json contains project data" {
    run "$META_BIN" --json project list
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
    run "$META_BIN" --json project list
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
    # Create a nested .meta.yaml inside frontend
    cat > "$TEST_DIR/frontend/.meta.yaml" <<'EOF'
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
    cat > "$TEST_DIR/frontend/.meta.yaml" <<'EOF'
{
    "projects": {
        "frontend-lib": "git@github.com:org/frontend-lib.git"
    }
}
EOF
    mkdir -p "$TEST_DIR/frontend/frontend-lib"

    run "$META_BIN" --recursive --json project list
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
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
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

    run "$META_BIN" --json project list
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

@test "meta project list with no .meta.yaml file shows error" {
    rm "$TEST_DIR/.meta.yaml"
    run "$META_BIN" project list
    [ "$status" -ne 0 ]
    [[ "$output" == *".meta.yaml"* ]]
}

@test "meta project list with YAML config" {
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
projects:
  frontend:
    repo: "git@github.com:org/frontend.git"
    tags: ["ui"]
  backend:
    repo: "git@github.com:org/backend.git"
EOF

    run "$META_BIN" --json project list
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
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
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
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
{
    "projects": {
        "backend": "git@github.com:org/backend.git",
        "vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}
    }
}
EOF
    mkdir -p "$TEST_DIR/vendor"

    run "$META_BIN" --json project list
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import sys, json
data = json.load(sys.stdin)
names = [p['name'] for p in data['projects']]
assert 'vendor' in names, f'vendor not in {names}'
assert 'backend' in names, f'backend not in {names}'
# vendor should have a repo (nested meta repos are regular repos that happen to contain .meta.yaml)
vendor = next(p for p in data['projects'] if p['name'] == 'vendor')
assert vendor.get('repo') == 'git@github.com:org/vendor.git', f'vendor should have repo: {vendor}'
"
}

@test "meta project list --recursive shows nested meta repo children" {
    # Setup: vendor is a nested meta repo with nested-lib inside
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
{
    "projects": {
        "backend": "git@github.com:org/backend.git",
        "vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}
    }
}
EOF
    mkdir -p "$TEST_DIR/vendor/nested-lib"
    cat > "$TEST_DIR/vendor/.meta.yaml" <<'EOF'
{"projects": {"nested-lib": "git@github.com:org/nested-lib.git"}}
EOF

    run "$META_BIN" --recursive project list
    [ "$status" -eq 0 ]
    # Should show vendor's child project
    [[ "$output" == *"nested-lib"* ]]
    [[ "$output" == *"vendor"* ]]
    [[ "$output" == *"backend"* ]]
}

@test "meta -A project list from nested meta-repo shows root tree" {
    # Create a nested .meta.yaml inside frontend
    cat > "$TEST_DIR/frontend/.meta.yaml" <<'EOF'
{
    "projects": {
        "frontend-lib": "git@github.com:org/frontend-lib.git"
    }
}
EOF
    mkdir -p "$TEST_DIR/frontend/frontend-lib"

    # Run from inside the nested meta-repo (frontend/)
    cd "$TEST_DIR/frontend"
    run "$META_BIN" -A --json project list
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
    run "$META_BIN" --json project list
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
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
{
    "projects": {
        "backend": "git@github.com:org/backend.git",
        "vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}
    }
}
EOF
    mkdir -p "$TEST_DIR/vendor/nested-lib"
    cat > "$TEST_DIR/vendor/.meta.yaml" <<'EOF'
{"projects": {"nested-lib": "git@github.com:org/nested-lib.git"}}
EOF

    run "$META_BIN" --recursive --json project list
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

@test "meta project list takes a project path outside the workspace" {
    for path in "../../outside" "/abs/outside"; do
        printf '{"projects": {"sib": {"repo": "u", "path": "%s"}}}' "$path" > .meta.yaml
        run "$META_BIN" --json project list
        [ "$status" -eq 0 ]
        [[ "$output" == *"\"path\": \"$path\""* ]]
    done
    # but not the workspace's own folder, nor git's data
    for path in "." ".git/x"; do
        printf '{"projects": {"evil": {"repo": "u", "path": "%s"}}}' "$path" > .meta.yaml
        run "$META_BIN" project list
        [ "$status" -ne 0 ]
        [[ "$output" == *"Invalid project 'evil' in"* ]]
    done
}

@test "meta project list takes a project reached through a symlink out of the workspace" {
    OUTSIDE="$(cd "$(mktemp -d)" && pwd -P)"
    ln -s "$OUTSIDE" linked
    echo '{"projects": {"linked": "git@github.com:org/linked.git"}}' > .meta.yaml
    run "$META_BIN" project list
    rm -rf "$OUTSIDE"
    [ "$status" -eq 0 ]
    [[ "$output" == *"linked"* ]]
}

@test "meta project list never prints terminal escapes from .meta.yaml" {
    # Name, path and tag carry OSC/CSI sequences and a newline
    cat > .meta.yaml <<'J'
{"projects": {"x\u001b]0;TITLE\u0007": {"repo": "u", "path": "t\u001b[2J", "tags": ["n\nfake"]}}}
J
    run "$META_BIN" project list
    [ "$status" -ne 0 ]
    [[ "$output" != *$'\e'* ]]
    [[ "$output" != *$'\a'* ]]
    [[ "$output" == *'x\u{1b}]0;TITLE\u{7}'* ]]
}

@test "an unknown project subcommand is escaped on one line" {
    run "$META_BIN" project $'bad\033[2J\nFORGED'
    [[ "$output" != *$'\033'* ]]
    [[ "$output" != *$'\nFORGED'* ]]
    [[ "$output" == *"unrecognized command"* ]]
}

@test "project list refuses include, exclude and tag instead of ignoring them" {
    printf 'projects:\n  frontend:\n    repo: git@github.com:org/frontend.git\n    tags: [ui]\n' > "$TEST_DIR/.meta.yaml"
    run "$META_BIN" --tag ui --recursive --json project list
    [ "$status" -ne 0 ]
    [[ "$output" == *"meta project list does not take --include/--exclude/--tag"* ]]
    for flag in --include --exclude; do
        for command in list ls; do
            run "$META_BIN" "$flag" frontend project "$command" --recursive --json
            [ "$status" -ne 0 ]
            [[ "$output" == *"meta project $command does not take --include/--exclude/--tag"* ]]
            # after the subcommand too, and in --flag=value form: not one
            # of its arguments
            short="${flag:1:2}"
            for trailing in "$flag frontend" "$flag=frontend" "${short}frontend"; do
                run "$META_BIN" project "$command" $trailing --json
                [ "$status" -ne 0 ]
                [[ "$output" == *"unexpected argument"* ]]
            done
        done
    done
}

@test "meta project check names the command that clones what is missing" {
    rm -rf "$TEST_DIR/shared"
    run "$META_BIN" project check
    [ "$status" -ne 0 ]
    [[ "$output" == *"shared: not cloned (run 'meta git clone-missing')"* ]]
    [[ "$output" != *"meta project update"* ]]
}

@test "project check checks only the projects a filter selects" {
    cat > "$TEST_DIR/.meta.yaml" <<'YAML'
projects:
  frontend:
    repo: git@github.com:org/frontend.git
    tags: [ui]
  gone:
    repo: git@github.com:org/gone.git
    tags: [api]
YAML
    git -C "$TEST_DIR/frontend" remote add origin git@github.com:org/frontend.git
    run "$META_BIN" --tag ui project check
    [ "$status" -eq 0 ]
    [[ "$output" == *"All projects are cloned"* ]]
    run "$META_BIN" --tag api project check
    [ "$status" -ne 0 ]
    [[ "$output" == *"gone: not cloned"* ]]
    run "$META_BIN" --tag api project dependents frontend
    [ "$status" -ne 0 ]
    [[ "$output" == *"does not take --include/--exclude/--tag"* ]]
}

# root (frontend backend shared, platform) > platform (api, infra) > infra (tf)
reach_tree() {
    cp "$META_BIN_DIR/meta-git" .meta/plugins/meta-git
    printf '{"projects": {"frontend": "git@github.com:org/frontend.git", "backend": "git@github.com:org/backend.git", "shared": "git@github.com:org/shared.git", "platform": {"repo": "git@github.com:org/platform.git", "meta": true}}}\n' > .meta.yaml
    mkdir -p platform/api platform/infra/tf
    printf '{"projects": {"api": {"repo": "git@github.com:org/api.git", "depends_on": ["infra"]}, "infra": {"repo": "git@github.com:org/infra.git", "meta": true}}}\n' > platform/.meta.yaml
    printf '{"projects": {"tf": "git@github.com:org/tf.git"}}\n' > platform/infra/.meta.yaml
    for repo in . frontend platform platform/api platform/infra platform/infra/tf; do
        git init --quiet "$repo"
        git -C "$repo" commit --quiet --allow-empty -m init
    done
}

paths() { jq -r '[.results[].path] | join(" ")'; }

@test "plain reaches the nearest .meta.yaml, -r the meta repos under it, -A the whole workspace" {
    bats_require_minimum_version 1.5.0
    reach_tree
    cd platform
    run --separate-stderr "$META_BIN" --json exec -- true
    [ "$(paths <<<"$output")" = ". ./api ./infra" ]
    # -r goes down from here, not up to the root; repos named from the root
    run --separate-stderr "$META_BIN" -r --json exec -- true
    [ "$(paths <<<"$output")" = "./platform ./platform/api ./platform/infra ./platform/infra/tf" ]
    # --depth counts from where -r starts
    run --separate-stderr "$META_BIN" -r --depth 0 --json exec -- true
    [ "$(paths <<<"$output")" = "./platform ./platform/api ./platform/infra" ]
    run --separate-stderr "$META_BIN" -A --json exec -- true
    [[ "$(paths <<<"$output")" == ". ./backend ./frontend ./platform ./platform/api ./platform/infra ./platform/infra/tf ./shared" ]]
    # a repo outside where -r starts: the tip names -A, with the line
    run "$META_BIN" -r --include frontend exec -- true
    [ "$status" -ne 0 ]
    [[ "$output" == *"outside platform, where -r starts"*"as in meta -A --include frontend exec -- true"* ]]
    run --separate-stderr "$META_BIN" -r --include platform/infra/tf --json exec -- true
    [ "$(paths <<<"$output")" = "./platform/infra/tf" ]
    # past --depth is not outside: no -A tip; the -A line keeps --depth
    run "$META_BIN" -r --depth 0 --include platform/infra/tf exec -- true
    [ "$status" -ne 0 ]
    [[ "$output" != *"-A reaches"* ]]
    run "$META_BIN" -r --depth 1 --include frontend exec -- true
    [[ "$output" == *"as in meta -A --depth 1 --include frontend exec -- true"* ]]
    # an --exclude of a repo outside gets it too
    run "$META_BIN" -r --include platform/api --exclude frontend exec -- true
    [[ "$output" == *"as in meta -A --include platform/api --exclude frontend exec -- true"* ]]
    # -r context names the first level as their config does, so depends_on holds
    run --separate-stderr "$META_BIN" -r --json context --no-status
    [ "$(jq -c '.dependencies' <<<"$output")" = '{"api":["infra"]}' ]
    # -r in a cluster: the -A line has it as -A
    run "$META_BIN" -rs --include frontend exec -- true
    [[ "$output" == *"as in meta -As --include frontend exec -- true"* ]]
    # the plugins that find their repos themselves reach as far
    run "$META_BIN" -r project list
    [[ "$output" == *"tf"* ]]
    [[ "$output" != *"frontend"* ]]
    run "$META_BIN" -A project list
    [[ "$output" == *"frontend"* && "$output" == *"tf"* ]]
    run "$META_BIN" project list
    [ "$status" -eq 0 ]
    [[ "$output" == *"api"* ]]
    [[ "$output" != *"tf"* && "$output" != *"frontend"* ]]
    run "$META_BIN" -A --dry-run git update
    [ "$status" -eq 0 ]
    [[ "$output" == *"frontend"* ]]
    run "$META_BIN" -r --dry-run git update
    [ "$status" -eq 0 ]
    [[ "$output" == *"(tf not pulled"* ]]
    [[ "$output" != *"frontend"* ]]
    # worktree create: --repo across the meta repos -r or -A reaches
    run "$META_BIN" -r --dry-run git worktree create t --repo infra/tf --no-deps
    [ "$status" -eq 0 ]
    run "$META_BIN" -r --dry-run git worktree create t --repo frontend --no-deps
    [ "$status" -ne 0 ]
    run "$META_BIN" -A --dry-run git worktree create t --repo frontend --no-deps
    [ "$status" -eq 0 ]
    # --depth limits how far down -r looks for --repo
    run "$META_BIN" -r --depth 0 --dry-run git worktree create t --repo infra/tf --no-deps
    [ "$status" -ne 0 ]
    # at the root, -r and -A reach the same repos
    cd "$TEST_DIR"
    # a nested project's depends_on names its siblings, by their paths here
    run --separate-stderr "$META_BIN" -r --json context --no-status
    [ "$(jq -c '.dependencies' <<<"$output")" = '{"platform/api":["platform/infra"]}' ]
    run --separate-stderr "$META_BIN" -r --json exec -- true
    r="$(paths <<<"$output")"
    run --separate-stderr "$META_BIN" -A --json exec -- true
    [ "$(paths <<<"$output")" = "$r" ]
}
