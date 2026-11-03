#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# Integration tests for `meta git worktree` subcommand
# Tests: create, add, list, status, diff, exec, remove, configuration, edge cases

setup() {
    sandbox
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
{
    "projects": {
        "backend": "git@github.com:org/backend.git",
        "frontend": "git@github.com:org/frontend.git"
    }
}
EOF
    make_workspace --root-repo --plugins git backend frontend
    cd "$TEST_DIR"
}

# Helper: Set up a nested meta repo structure
# Creates vendor/ with its own .meta.yaml containing nested-lib
# Args: $1 = nested lib name (default: nested-lib)
setup_nested_meta() {
    local nested_name="${1:-nested-lib}"

    mkdir -p "$TEST_DIR/vendor/$nested_name"

    # Update root config to include vendor as nested meta repo
    cat > "$TEST_DIR/.meta.yaml" <<EOF
{
    "projects": {
        "backend": "git@github.com:org/backend.git",
        "frontend": "git@github.com:org/frontend.git",
        "vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}
    }
}
EOF

    # Create nested .meta.yaml inside vendor
    cat > "$TEST_DIR/vendor/.meta.yaml" <<EOF
{"projects": {"$nested_name": "git@github.com:org/$nested_name.git"}}
EOF

    # Initialize the nested git repo
    git -C "$TEST_DIR/vendor/$nested_name" init --quiet --initial-branch=main
    echo "# $nested_name" > "$TEST_DIR/vendor/$nested_name/README.md"
    git -C "$TEST_DIR/vendor/$nested_name" add README.md
    git -C "$TEST_DIR/vendor/$nested_name" commit --quiet -m "init $nested_name"
}

# ============ Create ============

@test "worktree create makes isolated git worktree" {
    run "$META_BIN" git worktree create auth-fix --repo backend
    [ "$status" -eq 0 ]
    [ -d ".worktrees/auth-fix/backend" ]
    BRANCH=$(git -C ".worktrees/auth-fix/backend" branch --show-current)
    [ "$BRANCH" = "auth-fix" ]
}

@test "worktree create with explicit branch" {
    run "$META_BIN" git worktree create myfix --branch fix/the-bug --repo backend
    [ "$status" -eq 0 ]
    BRANCH=$(git -C ".worktrees/myfix/backend" branch --show-current)
    [ "$BRANCH" = "fix/the-bug" ]
}

@test "worktree create reports corrupt bare config with scoped repair" {
    git -C backend config --local core.bare true

    run "$META_BIN" git worktree create corrupt-source --repo backend

    [ "$status" -ne 0 ]
    [[ "$output" == *"core.bare=true"* ]]
    [[ "$output" == *"/backend"* ]]
    [[ "$output" == *"config --local core.bare false"* ]]
    [ ! -e ".worktrees/corrupt-source/backend" ]
}

@test "worktree status still names the branch of a member whose config says bare" {
    run "$META_BIN" git worktree create bare-member --repo backend
    [ "$status" -eq 0 ]
    git -C backend config --local core.bare true

    run "$META_BIN" git worktree status bare-member --json
    git -C backend config --local core.bare false
    [ "$status" -eq 0 ]
    [ "$(printf '%s' "$output" | jq -r '.repos[] | select(.alias == "backend") | .branch')" = "bare-member" ]
}

@test "worktree create --all creates worktrees for all repos" {
    run "$META_BIN" git worktree create full-task --all
    [ "$status" -eq 0 ]
    [ -d ".worktrees/full-task/backend" ]
    [ -d ".worktrees/full-task/frontend" ]
}

@test "worktree exec keeps the user's own GIT_CONFIG_* settings, colour or not" {
    run "$META_BIN" git worktree create cfg --repo backend --no-deps
    [ "$status" -eq 0 ]
    for force in "" 1; do
        CLICOLOR_FORCE=$force GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=user.name GIT_CONFIG_VALUE_0="Env Name" \
            run "$META_BIN" git worktree exec cfg -- git config user.name
        [ "$status" -eq 0 ]
        [[ "$output" == *"Env Name"* ]]
    done
}

@test "git's error over several lines keeps its line breaks" {
    git -C backend switch --quiet -c feature/login
    run "$META_BIN" git worktree create login --repo backend --no-deps --branch feature/login
    [ "$status" -ne 0 ]
    [[ "$output" == *$'\n  fatal: '*"already used by worktree"* ]]
    [[ "$output" != *'\nfatal'* ]]
}

@test "worktree create warns about a depends_on that names no project, and fails on it with --strict" {
    cat > .meta.yaml <<'EOF'
projects:
  backend: {repo: "git@github.com:org/backend.git", depends_on: [nosuch]}
  frontend: "git@github.com:org/frontend.git"
EOF
    run "$META_BIN" git worktree create loose --repo backend
    [ "$status" -eq 0 ]
    [[ "$output" == *"warning: 'backend' depends on 'nosuch', which no project is or provides"* ]]

    run "$META_BIN" --strict git worktree create tight --repo backend
    [ "$status" -ne 0 ]
    [[ "$output" == *"'backend' depends on 'nosuch'"*"(strict mode)"* ]]
    [ ! -e ".worktrees/tight" ]

    # two projects provide what one depends on: a warning, an error with --strict
    printf '%s\n' 'projects:' \
        '  backend: {repo: "git@github.com:org/backend.git", depends_on: [db]}' \
        '  frontend: {repo: "git@github.com:org/frontend.git", provides: [db]}' \
        '  shared: {repo: "git@github.com:org/shared.git", provides: [db]}' > .meta.yaml
    mkdir -p shared && git -C shared init --quiet --initial-branch=main
    git -C shared -c user.name=t -c user.email=t@t commit --quiet --allow-empty -m init
    run "$META_BIN" --dry-run git worktree create both --repo backend
    [ "$status" -eq 0 ]
    [[ "$output" == *"warning: 'backend' depends on 'db', which 'frontend' and 'shared' provide; 'shared' is used"* ]]
    run "$META_BIN" --strict --dry-run git worktree create both --repo backend
    [ "$status" -ne 0 ]
    [[ "$output" == *"'backend' depends on 'db', which 'frontend'"*"(strict mode)"* ]]
    # a set that needs no db is none of its business
    run "$META_BIN" --strict --dry-run git worktree create front --repo frontend
    [ "$status" -eq 0 ]
    [[ "$output" != *"depends on 'db'"* ]]
    # --all holds backend: its problems count too
    run "$META_BIN" --strict --dry-run git worktree create every --all
    [ "$status" -ne 0 ]
    [[ "$output" == *"'backend' depends on 'db', which 'frontend'"*"(strict mode)"* ]]
    # a project named db is what the name means, whoever provides one
    printf '%s\n' 'projects:' \
        '  backend: {repo: "git@github.com:org/backend.git", depends_on: [frontend]}' \
        '  frontend: {repo: "git@github.com:org/frontend.git", provides: [frontend]}' \
        '  shared: {repo: "git@github.com:org/shared.git", provides: [frontend]}' > .meta.yaml
    run "$META_BIN" --strict --dry-run git worktree create named --repo backend
    [ "$status" -eq 0 ]
    [[ "$output" != *"warning:"* ]]
    # nor of meta context, which doesn't act on the graph
    run "$META_BIN" context --no-status
    [ "$status" -eq 0 ]
    [[ "$output" != *"warning:"* ]]
}

@test "a warning keeps a name's line break escaped, so it can't forge a line" {
    bad=$'bad\nwarning: FORGED'
    mkdir -p ".worktrees/$bad/r"
    echo "gitdir: /does-not-exist" > ".worktrees/$bad/r/.git"
    run "$META_BIN" git worktree list --recursive --metadata-only
    [[ "$output" == *'bad\nwarning: FORGED'* ]]
    [[ "$output" != *$'\nwarning: FORGED'* ]]
}

@test "worktree prune names a set whose created_at it can't read, and fails on it with --strict" {
    run "$META_BIN" git worktree create aged --repo backend --no-deps --ttl 1h
    [ "$status" -eq 0 ]
    record="$(git -C .worktrees/aged/backend rev-parse --git-dir)/meta-worktree.json"
    [ -f "$record" ]
    sed -i.bak 's/"created_at": *"[^"]*"/"created_at": "not-a-date"/' "$record"
    run "$META_BIN" git worktree prune
    [ "$status" -eq 0 ]
    [[ "$output" == *"warning: Malformed created_at 'not-a-date'"* ]]
    run "$META_BIN" git worktree list
    [ "$status" -eq 0 ]
    [[ "$output" == *"warning: Malformed created_at"* ]]
    run "$META_BIN" --strict git worktree prune
    [ "$status" -ne 0 ]
    [[ "$output" == *"Malformed created_at"*"(strict mode)"* ]]
    [ -d ".worktrees/aged" ]
}

@test "worktree create and add clone the repos the set needs that aren't cloned, and only those" {
    for repo in backend frontend; do
        git clone --quiet --bare "$repo" "$TEST_DIR/$repo.git"
    done
    cat > .meta.yaml <<EOF
projects:
  api: {repo: "file://$TEST_DIR/backend.git", depends_on: [web, db]}
  web: "file://$TEST_DIR/frontend.git"
  db: "file://$TEST_DIR/backend.git"
  tool: "file://$TEST_DIR/frontend.git"
  unrelated: "file://$TEST_DIR/frontend.git"
  backend: "git@github.com:org/backend.git"
EOF
    run "$META_BIN" --dry-run git worktree create all-at-once --repo api
    [ "$status" -eq 0 ]
    [[ "$output" == *"Would clone the repos the set needs that aren't cloned (api, web, db)"* ]]
    [ ! -e api ] && [ ! -e ".worktrees/all-at-once" ]
    run "$META_BIN" git worktree create all-at-once --repo api
    [ "$status" -eq 0 ]
    [[ "$output" == *"Cloning 3 repos the set needs: api, web, db"* ]]
    [[ "$output" == *"Cloned api, web, db"* ]]
    [ -d ".worktrees/all-at-once/api" ] && [ -d ".worktrees/all-at-once/db" ] && [ -d ".worktrees/all-at-once/web" ]
    [ ! -e unrelated ] && [ ! -e tool ]
    run "$META_BIN" git worktree add all-at-once --repo tool
    [ "$status" -eq 0 ]
    [[ "$output" == *"Cloning 1 repo the set needs: tool"* ]]
    [ -d ".worktrees/all-at-once/tool" ]
    [ ! -e unrelated ]
}

@test "worktree create clones the meta repo on the way to a nested repo it asks for, in one go" {
    # a remote meta repo clients that lists desktop; neither is cloned
    git init --quiet --initial-branch=main "$TEST_DIR/src-desktop"
    git -C "$TEST_DIR/src-desktop" commit --quiet --allow-empty -m desktop
    git clone --quiet --bare "$TEST_DIR/src-desktop" "$TEST_DIR/desktop.git"
    git init --quiet --initial-branch=main "$TEST_DIR/src-clients"
    printf 'projects:\n  desktop: "file://%s/desktop.git"\n' "$TEST_DIR" > "$TEST_DIR/src-clients/.meta.yaml"
    echo "desktop/" > "$TEST_DIR/src-clients/.gitignore"
    git -C "$TEST_DIR/src-clients" add -A
    git -C "$TEST_DIR/src-clients" commit --quiet -m clients
    git clone --quiet --bare "$TEST_DIR/src-clients" "$TEST_DIR/clients.git"
    cat > .meta.yaml <<EOF
projects:
  clients: {repo: "file://$TEST_DIR/clients.git", meta: true}
  backend: "git@github.com:org/backend.git"
EOF
    run "$META_BIN" --dry-run git worktree create t -r --repo clients/desktop
    [ "$status" -eq 0 ]
    [[ "$output" == *"Would clone the meta repos on the way to the repos asked for (clients)"* ]]
    [ ! -e clients ]
    run "$META_BIN" git worktree create t -r --repo clients/desktop
    [ "$status" -eq 0 ]
    [[ "$output" == *"Cloning 1 repo the set needs: clients"* ]]
    [[ "$output" == *"Cloning 1 repo the set needs: clients/desktop"* ]]
    [ -e .worktrees/t/clients/desktop/.git ]
    [ -e .worktrees/t/clients/.git ]
}

@test "worktree create says to move aside a folder that blocks a member, and offers no command that would fail" {
    git clone --quiet --bare frontend "$TEST_DIR/frontend.git"
    cat > .meta.yaml <<EOF
projects:
  api: {repo: "file://$TEST_DIR/frontend.git", depends_on: [web]}
  web: "file://$TEST_DIR/frontend.git"
  backend: "git@github.com:org/backend.git"
EOF
    mkdir web && echo notes > web/notes.txt
    run "$META_BIN" git worktree create blocked --repo api
    [ "$status" -ne 0 ]
    [[ "$output" == *"'web' at $TEST_DIR/web: not a git repository"* ]]
    [[ "$output" == *"Move $TEST_DIR/web aside (or delete it, if nothing in it is needed), then run this create again: it clones what isn't there."* ]]
    [[ "$output" != *"clone-missing"* ]]
    [ ! -e api ] && [ ! -e .worktrees/blocked ]
    mv web web.aside
    run "$META_BIN" git worktree create blocked --repo api
    [ "$status" -eq 0 ]
    [ -d .worktrees/blocked/web ]
}

@test "worktree create names every repo it couldn't clone, and makes nothing" {
    git clone --quiet --bare frontend "$TEST_DIR/frontend.git"
    cat > .meta.yaml <<EOF
projects:
  api: {repo: "file://$TEST_DIR/frontend.git", depends_on: [web, db]}
  web: "file://$TEST_DIR/no-access-web.git"
  db: "file://$TEST_DIR/no-access-db.git"
  backend: "git@github.com:org/backend.git"
EOF
    run "$META_BIN" git worktree create denied --repo api
    [ "$status" -ne 0 ]
    # api clones; the two it can't are named, and nothing else
    [[ "$output" == *"2 repos the set needs can't be used; no worktree was created:"* ]]
    [[ "$output" == *"'web' at $TEST_DIR/web: not cloned"* ]]
    [[ "$output" == *"'db' at $TEST_DIR/db: not cloned"* ]]
    [[ "$output" != *"'api' at"* ]]
    [[ "$output" == *"meta --include 'web,db' git clone-missing"* ]]
    [[ "$output" == *"Cloning failed"* ]]
    [ ! -e ".worktrees/denied" ]
}

@test "worktree create --all skips a repo that is not cloned; --repo and add refuse it when it can't be cloned" {
    rm -rf frontend
    # a remote that can't be cloned (as without access), kept offline
    sed -i.bak "s#git@github.com:org/frontend.git#file://$TEST_DIR/nowhere.git#" .meta.yaml
    run "$META_BIN" git worktree create some --all
    [ "$status" -eq 0 ]
    [[ "$output" == *"'frontend' at $TEST_DIR/frontend: not cloned"*"skipping"* ]]
    [ -d ".worktrees/some/backend" ]
    [ ! -e ".worktrees/some/frontend" ]

    # all or nothing with --strict
    run "$META_BIN" git worktree create strict --all --strict
    [ "$status" -ne 0 ]
    [[ "$output" == *"'frontend' at $TEST_DIR/frontend: not cloned"* ]]
    [[ "$output" != *"skipping"* ]]
    [ ! -e ".worktrees/strict" ]

    run "$META_BIN" git worktree create one --repo frontend
    [ "$status" -ne 0 ]
    [[ "$output" == *"'frontend' at $TEST_DIR/frontend: not cloned"* ]]
    [ ! -e ".worktrees/one" ]

    # nor added to a set; a directory that is no repository neither, not
    # even as a worktree of the workspace repo around it
    run "$META_BIN" git worktree add some --repo frontend
    [ "$status" -ne 0 ]
    [[ "$output" == *"'frontend' at $TEST_DIR/frontend: not cloned"* ]]
    mkdir -p frontend/.git
    run "$META_BIN" git worktree add some --repo frontend:other
    [ "$status" -ne 0 ]
    [[ "$output" == *"'frontend' at $TEST_DIR/frontend: not a git repository"* ]]
    [ ! -e ".worktrees/some/frontend" ]
    if git rev-parse --verify --quiet refs/heads/other; then false; fi
}

@test "worktree create defaults branch name to task name" {
    run "$META_BIN" git worktree create my-task --repo backend
    [ "$status" -eq 0 ]
    BRANCH=$(git -C ".worktrees/my-task/backend" branch --show-current)
    [ "$BRANCH" = "my-task" ]
}

@test "worktree create per-repo branch override" {
    run "$META_BIN" git worktree create override-test --repo backend:custom-branch
    [ "$status" -eq 0 ]
    BRANCH=$(git -C ".worktrees/override-test/backend" branch --show-current)
    [ "$BRANCH" = "custom-branch" ]
}

@test "worktree create with dot-alias creates meta repo as root" {
    run "$META_BIN" git worktree create full --repo . --repo backend
    [ "$status" -eq 0 ]
    # The .worktrees/full directory IS the meta repo worktree
    [ -f ".worktrees/full/.git" ]
    [ -d ".worktrees/full/backend" ]
}

@test "worktree create --json emits structured output" {
    run "$META_BIN" git worktree create json-test --repo backend --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "import json,sys; d=json.load(sys.stdin); assert d['name']=='json-test'"
}

@test "worktree create with existing name fails" {
    "$META_BIN" git worktree create dupe --repo backend
    run "$META_BIN" git worktree create dupe --repo backend
    [ "$status" -ne 0 ]
    [[ "$output" == *"already exists"* ]]
}

@test "worktree create uses existing branch if available" {
    git -C backend branch existing-branch
    run "$META_BIN" git worktree create existing-branch --repo backend
    [ "$status" -eq 0 ]
    BRANCH=$(git -C ".worktrees/existing-branch/backend" branch --show-current)
    [ "$BRANCH" = "existing-branch" ]
}

@test "worktree create multiple repos with same default branch" {
    run "$META_BIN" git worktree create multi --repo backend --repo frontend
    [ "$status" -eq 0 ]
    [ -d ".worktrees/multi/backend" ]
    [ -d ".worktrees/multi/frontend" ]
    B1=$(git -C ".worktrees/multi/backend" branch --show-current)
    B2=$(git -C ".worktrees/multi/frontend" branch --show-current)
    [ "$B1" = "multi" ]
    [ "$B2" = "multi" ]
}

# ============ Add ============

@test "worktree add extends existing worktree set" {
    "$META_BIN" git worktree create extend-test --repo backend
    run "$META_BIN" git worktree add extend-test --repo frontend
    [ "$status" -eq 0 ]
    [ -d ".worktrees/extend-test/frontend" ]
}

@test "worktree add and create start a new branch from the remote's default branch, not the checkout's" {
    "$META_BIN" git worktree create base-set --repo backend
    for repo in backend frontend; do
        git clone --quiet --bare "$repo" "$TEST_DIR/$repo.git"
        git -C "$repo" remote add origin "$TEST_DIR/$repo.git"
        git -C "$repo" fetch --quiet origin
        git -C "$repo" remote set-head origin main
        # the canonical checkout is on a feature branch, ahead of main
        git -C "$repo" switch --quiet -c feature
        git -C "$repo" commit --quiet --allow-empty -m "feature work"
    done
    local main_commit
    main_commit=$(git -C frontend rev-parse origin/main)
    run "$META_BIN" git worktree add base-set --repo frontend
    [ "$status" -eq 0 ]
    [[ "$output" == *"Added 'frontend' to worktree 'base-set' (branch: base-set) (new from origin/main)"* ]]
    [ "$(git -C .worktrees/base-set/frontend rev-parse HEAD)" = "$main_commit" ]
    # a branch of its own: it tracks nothing, so a push or pull never reaches main
    run git -C .worktrees/base-set/frontend rev-parse --abbrev-ref '@{upstream}'
    [ "$status" -ne 0 ]
    run "$META_BIN" git worktree create other-set --repo backend
    [ "$status" -eq 0 ]
    [[ "$output" == *"backend -> other-set (new from origin/main)"* ]]
    [ "$(git -C .worktrees/other-set/backend rev-parse HEAD)" = "$(git -C backend rev-parse origin/main)" ]
    # add takes the commit-ish to start from, as create does
    "$META_BIN" git worktree create third-set --repo backend
    run "$META_BIN" git worktree add third-set feature --repo frontend
    [ "$status" -eq 0 ]
    [[ "$output" == *"(new from feature)"* ]]
    [ "$(git -C .worktrees/third-set/frontend rev-parse HEAD)" = "$(git -C frontend rev-parse feature)" ]
    # a remote that names no default branch: HEAD, with a warning in one line
    git -C backend remote set-head origin --delete
    run "$META_BIN" git worktree create fourth-set --repo backend
    [ "$status" -eq 0 ]
    [[ "$output" == *"so 'fourth-set' starts from HEAD (feature); give the commit-ish to start from, or set it with: git -C "*"remote set-head origin --auto"* ]]
    [[ "$output" == *"backend -> fourth-set (new from HEAD (feature))"* ]]
}

@test "worktree add rejects dot-alias" {
    "$META_BIN" git worktree create code-only --repo backend
    run "$META_BIN" git worktree add code-only --repo .
    [ "$status" -ne 0 ]
    [[ "$output" == *"create"* ]]
}

@test "worktree add to nonexistent set fails" {
    run "$META_BIN" git worktree add nonexistent --repo backend
    [ "$status" -ne 0 ]
}

@test "worktree add --json emits structured output" {
    "$META_BIN" git worktree create add-json --repo backend
    run "$META_BIN" git worktree add add-json --repo frontend --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "import json,sys; d=json.load(sys.stdin); assert d['name']=='add-json'"
}

# ============ List ============

@test "worktree list shows all worktree sets" {
    "$META_BIN" git worktree create task-a --repo backend
    "$META_BIN" git worktree create task-b --repo frontend
    run "$META_BIN" git worktree list
    [ "$status" -eq 0 ]
    [[ "$output" == *"task-a"* ]]
    [[ "$output" == *"task-b"* ]]
}

@test "worktree list --json outputs valid JSON" {
    "$META_BIN" git worktree create list-json --repo backend
    run "$META_BIN" git worktree list --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json,sys
data = json.load(sys.stdin)
assert any(w['name'] == 'list-json' for w in data['worktrees'])
"
}

@test "worktree list shows nothing when no worktrees exist" {
    run "$META_BIN" git worktree list
    [ "$status" -eq 0 ]
    [[ "$output" == *"No worktree"* ]] || [ -z "$output" ]
}

@test "worktree list shows repo count per worktree" {
    "$META_BIN" git worktree create multi-list --repo backend --repo frontend
    run "$META_BIN" git worktree list
    [ "$status" -eq 0 ]
    [[ "$output" == *"multi-list"* ]]
    # each repo, on the set's branch
    grep -Eq 'backend +-> multi-list' <<<"$output"
    grep -Eq 'frontend +-> multi-list' <<<"$output"
}

# ============ Status ============

@test "worktree status shows branch and clean state" {
    "$META_BIN" git worktree create status-clean --repo backend
    run "$META_BIN" git worktree status status-clean
    [ "$status" -eq 0 ]
    [[ "$output" == *"status-clean"* ]]
    grep -Eq 'backend +status-clean +clean$' <<<"$output"
}

@test "worktree status shows dirty state" {
    "$META_BIN" git worktree create status-dirty --repo backend
    echo "change" >> ".worktrees/status-dirty/backend/README.md"
    run "$META_BIN" git worktree status status-dirty
    [ "$status" -eq 0 ]
    grep -Eq 'backend +status-dirty +1 modified$' <<<"$output"
}

@test "worktree status --json includes modified files" {
    "$META_BIN" git worktree create status-json --repo backend
    echo "change" >> ".worktrees/status-json/backend/README.md"
    run "$META_BIN" git worktree status status-json --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json,sys
d = json.load(sys.stdin)
repo = next(r for r in d['repos'] if r['alias'] == 'backend')
assert repo['dirty'] == True
assert repo['modified_count'] >= 1
"
}

@test "worktree status on nonexistent worktree fails" {
    run "$META_BIN" git worktree status nonexistent
    [ "$status" -ne 0 ]
}

# ============ Diff ============

@test "worktree diff shows changes vs base" {
    "$META_BIN" git worktree create diff-test --repo backend
    echo "new content" >> ".worktrees/diff-test/backend/README.md"
    git -C ".worktrees/diff-test/backend" add -A
    git -C ".worktrees/diff-test/backend" commit --quiet -m "test change" \
        --author "Test <test@test.com>"
    run "$META_BIN" git worktree diff diff-test
    [ "$status" -eq 0 ]
    grep -Eq 'backend +\+1 -0' <<<"$output"
}

@test "worktree diff --json returns structured output" {
    "$META_BIN" git worktree create diff-json --repo backend
    echo "change" >> ".worktrees/diff-json/backend/README.md"
    git -C ".worktrees/diff-json/backend" add -A
    git -C ".worktrees/diff-json/backend" commit --quiet -m "change" \
        --author "Test <test@test.com>"
    run "$META_BIN" git worktree diff diff-json --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json,sys
d = json.load(sys.stdin)
assert d['totals']['files_changed'] >= 1
"
}

@test "worktree diff --base allows explicit base ref" {
    "$META_BIN" git worktree create diff-base --repo backend
    run "$META_BIN" git worktree diff diff-base --base main
    [ "$status" -eq 0 ]
}

@test "worktree diff --stat shows summary" {
    "$META_BIN" git worktree create diff-stat --repo backend
    echo "stats" >> ".worktrees/diff-stat/backend/README.md"
    git -C ".worktrees/diff-stat/backend" add -A
    git -C ".worktrees/diff-stat/backend" commit --quiet -m "stat change" \
        --author "Test <test@test.com>"
    run "$META_BIN" git worktree diff diff-stat --stat
    [ "$status" -eq 0 ]
    grep -Eq 'backend +\+1 -0' <<<"$output"
}

@test "worktree diff on nonexistent worktree fails" {
    run "$META_BIN" git worktree diff nonexistent
    [ "$status" -ne 0 ]
}

# ============ Exec ============

@test "worktree exec runs command in worktree repos" {
    "$META_BIN" git worktree create exec-test --repo backend --repo frontend
    run "$META_BIN" git worktree exec exec-test -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"exec-test/backend"* ]]
    [[ "$output" == *"exec-test/frontend"* ]]
}

@test "worktree exec --include filters repos" {
    "$META_BIN" git worktree create exec-filter --repo backend --repo frontend
    run "$META_BIN" git worktree exec exec-filter --include backend -- echo found
    [ "$status" -eq 0 ]
    MATCH_COUNT=$(echo "$output" | grep -c "found" || true)
    [ "$MATCH_COUNT" -eq 1 ]
}

@test "worktree exec --exclude filters repos" {
    "$META_BIN" git worktree create exec-excl --repo backend --repo frontend
    run "$META_BIN" git worktree exec exec-excl --exclude frontend -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"backend"* ]]
    [[ "$output" != *"exec-excl/frontend"* ]]
}

@test "worktree exec --include names whole repos, never parts of paths" {
    "$META_BIN" git worktree create exec-whole --repo backend --repo frontend
    run "$META_BIN" git worktree exec exec-whole --include end -- touch ran
    [ "$status" -ne 0 ]
    [[ "$output" == *"--include 'end' names none of the repos"* ]]
    [ -z "$(find .worktrees/exec-whole -name ran)" ]
    run "$META_BIN" git worktree exec exec-whole --exclude ./backend/ -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"exec-whole/frontend"* ]]
    [[ "$output" != *"exec-whole/backend"* ]]
}

@test "worktree exec --parallel runs concurrently" {
    "$META_BIN" git worktree create exec-par --repo backend --repo frontend
    run "$META_BIN" git worktree exec exec-par --parallel -- echo parallel-ok
    [ "$status" -eq 0 ]
    [[ "$output" == *"parallel-ok"* ]]
}

@test "worktree exec --json produces structured output" {
    "$META_BIN" git worktree create exec-json --repo backend
    run "$META_BIN" git worktree exec exec-json --json -- echo json-out
    [ "$status" -eq 0 ]
    [[ "$output" == *"json-out"* ]]
}

@test "worktree exec propagates exit codes" {
    "$META_BIN" git worktree create exec-fail --repo backend
    run "$META_BIN" git worktree exec exec-fail -- false
    [ "$status" -ne 0 ]
}

@test "worktree exec on nonexistent worktree fails" {
    run "$META_BIN" git worktree exec nonexistent -- echo hello
    [ "$status" -ne 0 ]
}

@test "worktree exec with no command shows error" {
    "$META_BIN" git worktree create exec-nocmd --repo backend
    run "$META_BIN" git worktree exec exec-nocmd --
    [ "$status" -eq 2 ]
    [[ "$output" == *"Usage: meta git worktree exec"* ]]
}

# ============ Remove ============

@test "worktree remove removes worktree set" {
    "$META_BIN" git worktree create temp --repo backend --no-deps
    [ -d ".worktrees/temp" ]
    run "$META_BIN" git worktree remove temp
    [ "$status" -eq 0 ]
    [ ! -d ".worktrees/temp" ]
}

@test "worktree remove deletes the branches it created that hold nothing unique, and names the ones it keeps" {
    git clone --quiet --bare frontend "$TEST_DIR/frontend.git"
    git -C frontend remote add origin "$TEST_DIR/frontend.git"
    git -C frontend fetch --quiet origin
    git -C backend branch existing
    "$META_BIN" git worktree create b1 --repo backend --repo frontend --no-deps
    # a commit only this branch has, and one pushed to the remote
    git -C .worktrees/b1/backend commit --quiet --allow-empty -m "only on b1"
    git -C .worktrees/b1/frontend commit --quiet --allow-empty -m "pushed"
    git -C .worktrees/b1/frontend push --quiet origin b1
    "$META_BIN" git worktree create b2 --repo backend:existing --no-deps
    run "$META_BIN" --dry-run git worktree remove b1
    [ "$status" -eq 0 ]
    [[ "$output" == *"would keep branch 'b1' in $TEST_DIR/backend: 1 commit on no remote, other branch or tag (first: "*"only on b1)"* ]]
    [[ "$output" == *"would delete branch 'b1' in $TEST_DIR/frontend"* ]]
    git -C frontend rev-parse --verify --quiet refs/heads/b1
    run "$META_BIN" git worktree remove b1
    [ "$status" -eq 0 ]
    [[ "$output" == *"kept branch 'b1' in $TEST_DIR/backend: 1 commit on no remote, other branch or tag"* ]]
    [[ "$output" == *"deleted branch 'b1' in $TEST_DIR/frontend (all its commits are on a remote or another branch)"* ]]
    git -C backend rev-parse --verify --quiet refs/heads/b1
    run git -C frontend rev-parse --verify --quiet refs/heads/b1
    [ "$status" -ne 0 ]
    # a branch the set only checked out was not created for it: kept, unmentioned
    run "$META_BIN" git worktree remove b2
    [ "$status" -eq 0 ]
    [[ "$output" != *"branch 'existing'"* ]]
    git -C backend rev-parse --verify --quiet refs/heads/existing
}

@test "worktree remove refuses dirty worktree without --force" {
    "$META_BIN" git worktree create dirty-test --repo backend
    echo "uncommitted" >> ".worktrees/dirty-test/backend/README.md"
    run "$META_BIN" git worktree remove dirty-test
    [ "$status" -ne 0 ]
    [[ "$output" == *"uncommitted"* ]] || [[ "$output" == *"dirty"* ]] || [[ "$output" == *"--force"* ]]
}

@test "worktree remove --force removes dirty worktree" {
    "$META_BIN" git worktree create force-test --repo backend
    echo "uncommitted" >> ".worktrees/force-test/backend/README.md"
    run "$META_BIN" git worktree remove force-test --force
    [ "$status" -eq 0 ]
    [ ! -d ".worktrees/force-test" ]
}

@test "worktree remove nonexistent worktree fails" {
    run "$META_BIN" git worktree remove nonexistent
    [ "$status" -ne 0 ]
}

@test "worktree remove with multiple repos removes all" {
    "$META_BIN" git worktree create multi-remove --repo backend --repo frontend --no-deps
    [ -d ".worktrees/multi-remove/backend" ]
    [ -d ".worktrees/multi-remove/frontend" ]
    run "$META_BIN" git worktree remove multi-remove
    [ "$status" -eq 0 ]
    [ ! -d ".worktrees/multi-remove" ]
}

# ============ Configuration ============

@test "META_WORKTREES env overrides default location" {
    CUSTOM_DIR="$(cd "$(mktemp -d)" && pwd -P)"
    META_WORKTREES="$CUSTOM_DIR" run "$META_BIN" git worktree create env-test --repo backend
    [ "$status" -eq 0 ]
    [ -d "$CUSTOM_DIR/env-test/backend" ]
    rm -rf "$CUSTOM_DIR"
}

@test "worktree create keeps its sets out of git status without touching .gitignore" {
    "$META_BIN" git worktree create gitignore-test --repo backend
    [ ! -e .gitignore ]
    grep -qx "/.worktrees" .git/info/exclude
    run git status --porcelain
    [[ "$output" != *".worktrees"* ]]
}

# ============ Edge Cases ============

@test "worktree create with invalid name (path traversal) fails" {
    run "$META_BIN" git worktree create "../escape" --repo backend
    [ "$status" -ne 0 ]
}

@test "worktree create with invalid name (dot prefix) fails" {
    run "$META_BIN" git worktree create ".hidden" --repo backend
    [ "$status" -ne 0 ]
}

@test "worktree create with invalid name (slash) fails" {
    run "$META_BIN" git worktree create "has/slash" --repo backend
    [ "$status" -ne 0 ]
}

@test "worktree create with invalid name (unicode) fails" {
    run "$META_BIN" git worktree create "修正" --repo backend
    [ "$status" -ne 0 ]
    [[ "$output" == *"ASCII"* ]] || [[ "$output" == *"alphanumeric"* ]]
}

@test "worktree create with unknown repo alias fails" {
    run "$META_BIN" git worktree create bad-alias --repo nonexistent-repo
    [ "$status" -ne 0 ]
    [[ "$output" == *"not found"* ]] || [[ "$output" == *"unknown"* ]] || [[ "$output" == *"Unknown"* ]]
}

@test "worktree create with no repos specified fails" {
    run "$META_BIN" git worktree create no-repos
    [ "$status" -ne 0 ]
}

@test "worktree help shows usage" {
    run "$META_BIN" git worktree
    [ "$status" -eq 0 ]
    [[ "$output" == *"create"* ]]
    [[ "$output" == *"remove"* ]]
    [[ "$output" == *"list"* ]]
}

# ============ Full Lifecycle ============

@test "full lifecycle: create, list, status, exec, remove" {
    # Create (use --no-deps to avoid auto-including root repo which may have test changes)
    run "$META_BIN" git worktree create lifecycle --repo backend --repo frontend --no-deps
    [ "$status" -eq 0 ]

    # List
    run "$META_BIN" git worktree list
    [ "$status" -eq 0 ]
    [[ "$output" == *"lifecycle"* ]]

    # Status
    run "$META_BIN" git worktree status lifecycle
    [ "$status" -eq 0 ]
    [[ "$output" == *"backend"* ]]
    [[ "$output" == *"frontend"* ]]

    # Exec
    run "$META_BIN" git worktree exec lifecycle -- echo lifecycle-ok
    [ "$status" -eq 0 ]
    [[ "$output" == *"lifecycle-ok"* ]]

    # Remove
    run "$META_BIN" git worktree remove lifecycle
    [ "$status" -eq 0 ]
    [ ! -d ".worktrees/lifecycle" ]

    # List should show nothing
    run "$META_BIN" git worktree list
    [ "$status" -eq 0 ]
    [[ "$output" != *"lifecycle"* ]]
}

# ============ Unrecognized command handling (meta-7) ============

@test "unrecognized worktree command is escaped on one line" {
    cd "$TEST_DIR"
    run "$META_BIN" git worktree $'bad\033]0;TITLE\007\nFORGED'
    [ "$status" -eq 2 ]
    [[ "$output" != *$'\033'* ]]
    [[ "$output" != *$'\nFORGED'* ]]
    [[ "$output" == *"unrecognized subcommand 'bad"* ]]
}

@test "an invalid option value is escaped on one line" {
    cd "$TEST_DIR"
    run "$META_BIN" git worktree create ttl-bad --repo backend --ttl $'bad\033[2J\nFORGEDs'
    [ "$status" -ne 0 ]
    [[ "$output" != *$'\033'* ]]
    [[ "$output" != *$'\nFORGED'* ]]
    [[ "$output" == *"--ttl"* ]]
}

@test "an unknown recursive repo alias is escaped on one line" {
    cd "$TEST_DIR"
    run "$META_BIN" git worktree create badrepo --recursive --repo $'a\033[2J\nFORGED'
    [ "$status" -ne 0 ]
    [[ "$output" != *$'\033'* ]]
    [[ "$output" != *$'\nFORGED'* ]]
}

@test "rejected refs and --meta values are escaped on one line" {
    cd "$TEST_DIR"
    for strict in "" --strict; do
        run "$META_BIN" $strict git worktree create probe $'bad\033[2J\nFORGED' --repo . --no-deps
        [ "$status" -ne 0 ]
        [[ "$output" != *$'\033'* ]]
        [[ "$output" != *$'\nFORGED'* ]]
        [[ "$output" == *'bad\u{1b}[2J\nFORGED'* ]]
    done
    # The dry-run preview echoes them too
    for args in "probe" "--from-pr" "--meta"; do
        case "$args" in
            probe) run "$META_BIN" git worktree create preview $'bad\033[2J\nFORGED' --repo . --no-deps --dry-run ;;
            --from-pr) run "$META_BIN" git worktree create preview --from-pr $'o/r#1\033[2J\nFORGED' --repo . --no-deps --dry-run ;;
            --meta) run "$META_BIN" git worktree create preview --meta $'k=v\033[2J\nFORGED' --repo . --no-deps --dry-run ;;
        esac
        [[ "$output" != *$'\033'* ]]
        [[ "$output" != *$'\nFORGED'* ]]
    done
    for flag in --meta --from-pr; do
        run "$META_BIN" --strict git worktree create probe --repo . --no-deps "$flag" $'bad\033[2J\nFORGED'
        [ "$status" -ne 0 ]
        [[ "$output" != *$'\033'* ]]
        [[ "$output" != *$'\nFORGED'* ]]
        [[ "$output" == *'bad\u{1b}[2J\nFORGED'* ]]
    done
}

@test "names given to exec, status, diff and --branch are escaped on one line" {
    cd "$TEST_DIR"
    E=$'bad\033[2J\nFORGED'
    check() {
        [ "$status" -ne 0 ]
        [[ "$output" != *$'\033'* ]]
        [[ "$output" != *$'\nFORGED'* ]]
    }
    run "$META_BIN" git worktree exec "$E" -- true; check
    run "$META_BIN" git worktree status "$E"; check
    run "$META_BIN" git worktree diff "$E"; check
    run "$META_BIN" git worktree create okb --all --branch "$E"; check
    # A real set, compared against a bad base
    "$META_BIN" git worktree create based --repo backend
    run "$META_BIN" git worktree diff based --base "$E"
    [[ "$output" != *$'\033'* ]]
    [[ "$output" != *$'\nFORGED'* ]]
}

@test "gh's error for --from-pr is escaped on one line" {
    cd "$TEST_DIR"
    mkdir -p "$TEST_DIR/fakebin"
    printf '#!/bin/sh\nprintf "no such PR\\033[2J\\nFORGED\\n" >&2\nexit 1\n' > "$TEST_DIR/fakebin/gh"
    chmod +x "$TEST_DIR/fakebin/gh"
    PATH="$TEST_DIR/fakebin:$PATH" run "$META_BIN" git worktree create probe --from-pr o/r#1 --repo . --no-deps
    [ "$status" -ne 0 ]
    [[ "$output" == *"Failed to resolve PR #1"* ]]
    [[ "$output" != *$'\033'* ]]
    [[ "$output" != *$'\nFORGED'* ]]
}

@test "unrecognized worktree command shows help" {
    cd "$TEST_DIR"
    run "$META_BIN" git worktree blablabla
    [ "$status" -eq 2 ]
    # the error, once
    [ "$(grep -c "unrecognized subcommand 'blablabla'" <<<"$output")" -eq 1 ]
    # Must show ACTUAL help content (not just a reference to --help)
    [[ "$output" == *"Usage"* ]]
    [[ "$output" == *"create"* ]]
    [[ "$output" == *"remove"* ]]
    [[ "$output" == *"list"* ]]
}

# ============ Edge Cases: Remove with dot-alias ============

@test "worktree remove with dot-alias removes children before root" {
    # Create worktree with . (meta repo) + child repos
    run "$META_BIN" git worktree create dot-remove --repo . --repo backend --repo frontend
    [ "$status" -eq 0 ]
    [ -f ".worktrees/dot-remove/.git" ]
    [ -d ".worktrees/dot-remove/backend" ]
    [ -d ".worktrees/dot-remove/frontend" ]

    # Remove — children must be removed before "." (root)
    run "$META_BIN" git worktree remove dot-remove --force
    [ "$status" -eq 0 ]
    [ ! -d ".worktrees/dot-remove" ]

    # Original repos should be intact
    [ -d "backend" ]
    [ -d "frontend" ]

    # The branches the set made held nothing new: deleted with it
    [ -z "$(git -C backend branch --list dot-remove)" ]
    [ -z "$(git -C frontend branch --list dot-remove)" ]
}

# ============ Edge Cases: Add duplicate repo ============

@test "worktree add duplicate repo fails" {
    "$META_BIN" git worktree create dup-repo --repo backend
    run "$META_BIN" git worktree add dup-repo --repo backend
    [ "$status" -ne 0 ]
    [[ "$output" == *"already"* ]] || [[ "$output" == *"exists"* ]] || [[ "$output" == *"duplicate"* ]]
}

# ============ Edge Cases: Diff with no changes ============

@test "worktree diff with no changes shows zero" {
    "$META_BIN" git worktree create diff-clean --repo backend
    run "$META_BIN" git worktree diff diff-clean --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json, sys
d = json.load(sys.stdin)
assert d['totals']['files_changed'] == 0, f'expected 0 files changed, got: {d[\"totals\"][\"files_changed\"]}'
assert d['totals']['insertions'] == 0
assert d['totals']['deletions'] == 0
"
}

# ============ Edge Cases: Status mixed dirty/clean ============

@test "worktree status shows mixed dirty and clean repos" {
    "$META_BIN" git worktree create mixed-status --repo backend --repo frontend
    # Make backend dirty
    echo "dirty change" >> ".worktrees/mixed-status/backend/README.md"
    # Leave frontend clean

    run "$META_BIN" git worktree status mixed-status --json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json, sys
d = json.load(sys.stdin)
repos = {r['alias']: r for r in d['repos']}
assert repos['backend']['dirty'] == True, f'backend should be dirty: {repos[\"backend\"]}'
assert repos['frontend']['dirty'] == False, f'frontend should be clean: {repos[\"frontend\"]}'
"
}

# ============ Edge Cases: --all and --repo mutually exclusive ============

@test "worktree create --all --repo rejects combination" {
    run "$META_BIN" git worktree create all-repo-combo --all --repo backend
    [ "$status" -ne 0 ]
    [[ "$output" == *"cannot be used with"* ]] || [[ "$output" == *"conflict"* ]]
}

# ============ Edge Cases: worktrees_dir config ============

@test "worktrees_dir config option overrides default location" {
    CUSTOM_DIR="$TEST_DIR/custom-wt-dir"

    cat > "$TEST_DIR/.meta.yaml" <<EOF
{
    "projects": {
        "backend": "git@github.com:org/backend.git",
        "frontend": "git@github.com:org/frontend.git"
    },
    "worktrees_dir": "$CUSTOM_DIR"
}
EOF

    run "$META_BIN" git worktree create cfg-dir --repo backend
    [ "$status" -eq 0 ]
    [ -d "$CUSTOM_DIR/cfg-dir/backend" ]
    # Default location should NOT have it
    [ ! -d ".worktrees/cfg-dir" ]
}

# ============ Nested Repo (meta-53) ============

@test "a parent a nested repo needs is in the set for its place: its depends_on is not checked" {
    setup_nested_meta
    sed -i.bak 's#"vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}#"vendor": {"repo": "git@github.com:org/vendor.git", "meta": true, "depends_on": ["missing"]}#' .meta.yaml
    grep -q '"missing"' .meta.yaml
    run "$META_BIN" --dry-run git worktree create parent-check --recursive --repo vendor/nested-lib
    [ "$status" -eq 0 ]
    [[ "$output" != *"depends on 'missing'"* ]]
    # asked for, its depends_on is followed, and checked
    run "$META_BIN" --strict --dry-run git worktree create parent-check --recursive --repo vendor
    [ "$status" -ne 0 ]
    [[ "$output" == *"'vendor' depends on 'missing'"*"(strict mode)"* ]]
}

@test "worktree create with nested repo alias succeeds" {
    setup_nested_meta

    # Create worktree with nested repo path
    run "$META_BIN" git worktree create nested-test --repo vendor/nested-lib
    [ "$status" -eq 0 ]

    # The worktree preserves the full nested path so relative references remain valid
    [ -d ".worktrees/nested-test/vendor/nested-lib" ]

    # Verify it's on the expected branch
    BRANCH=$(git -C ".worktrees/nested-test/vendor/nested-lib" branch --show-current)
    [ "$BRANCH" = "nested-test" ]
}

@test "worktree create refuses a repo named twice by short and full alias on two branches" {
    setup_nested_meta

    run "$META_BIN" git worktree create two-names --recursive --repo nested-lib:one --repo vendor/nested-lib:two
    [ "$status" -ne 0 ]
    [[ "$output" == *"is given twice"* ]]
    [ ! -e ".worktrees/two-names" ]
}

@test "worktree list and status show a repo nested in another repo of the set without -r" {
    setup_nested_meta
    # vendor is a repo of its own, holding nested-lib
    echo "nested-lib/" > vendor/.gitignore
    git -C vendor init --quiet --initial-branch=main
    git -C vendor add .gitignore .meta.yaml
    git -C vendor commit --quiet -m "vendor"
    "$META_BIN" git worktree create nest --recursive --repo vendor --repo vendor/nested-lib
    [ -e .worktrees/nest/vendor/nested-lib/.git ]
    run "$META_BIN" git worktree list nest
    [ "$status" -eq 0 ]
    grep -Eq 'vendor/nested-lib +-> nest' <<<"$output"
    run "$META_BIN" git worktree list
    grep -Eq 'vendor/nested-lib +-> nest' <<<"$output"
    run "$META_BIN" --json git worktree status nest
    [ "$status" -eq 0 ]
    [[ "$output" == *'"alias": "vendor/nested-lib"'* ]]
}

@test "worktree add with nested repo alias succeeds" {
    setup_nested_meta

    # First create worktree with backend
    "$META_BIN" git worktree create add-nested --repo backend

    # Then add nested repo
    run "$META_BIN" git worktree add add-nested --repo vendor/nested-lib
    [ "$status" -eq 0 ]
    [ -d ".worktrees/add-nested/vendor/nested-lib" ]
}

@test "worktree create with invalid nested repo path fails" {
    # Setup nested meta structure with other-lib (not nested-lib)
    setup_nested_meta "other-lib"

    # Try to create worktree with non-existent nested path
    run "$META_BIN" git worktree create bad-nested --repo vendor/nonexistent
    [ "$status" -ne 0 ]
    [[ "$output" == *"Unknown repo"* ]]
}

@test "worktree create with mixed repos: root, regular, and nested" {
    setup_nested_meta

    # Create worktree with root (.), regular (backend), and nested (vendor/nested-lib)
    run "$META_BIN" git worktree create mixed-wt --repo . --repo backend --repo vendor/nested-lib
    [ "$status" -eq 0 ]

    # All three should exist
    [ -d ".worktrees/mixed-wt" ]
    [ -d ".worktrees/mixed-wt/backend" ]
    [ -d ".worktrees/mixed-wt/vendor/nested-lib" ]

    # Verify branches
    BRANCH_BACKEND=$(git -C ".worktrees/mixed-wt/backend" branch --show-current)
    [ "$BRANCH_BACKEND" = "mixed-wt" ]
    BRANCH_NESTED=$(git -C ".worktrees/mixed-wt/vendor/nested-lib" branch --show-current)
    [ "$BRANCH_NESTED" = "mixed-wt" ]
}

@test "worktree exec works with nested repos" {
    setup_nested_meta

    # Create worktree with backend and nested repo
    "$META_BIN" git worktree create exec-nested --repo backend --repo vendor/nested-lib

    # Run a command across all repos in the worktree
    run "$META_BIN" git worktree exec exec-nested -- git branch --show-current
    [ "$status" -eq 0 ]
    [[ "$output" == *"exec-nested"* ]]
}

@test "worktree status shows nested repos" {
    setup_nested_meta

    # Create worktree with nested repo
    "$META_BIN" git worktree create status-nested --repo vendor/nested-lib

    # Check status
    run "$META_BIN" git worktree status status-nested
    [ "$status" -eq 0 ]
    [[ "$output" == *"nested-lib"* ]]
}

@test "worktree list shows worktrees with nested repos" {
    setup_nested_meta

    # Create worktree with nested repo
    "$META_BIN" git worktree create list-nested --repo vendor/nested-lib

    # Check list
    run "$META_BIN" git worktree list
    [ "$status" -eq 0 ]
    [[ "$output" == *"list-nested"* ]]
}

@test "--include before git worktree is refused, not ignored" {
    "$META_BIN" git worktree create global-filter --repo backend --repo frontend
    run "$META_BIN" --exclude frontend git worktree exec global-filter -- touch ran
    [ "$status" -ne 0 ]
    [[ "$output" == *"does not take --include/--exclude/--tag before it"* ]]
    # --tag too, though the tag is real
    echo '{"projects": {"backend": {"repo": "git@github.com:org/backend.git", "tags": ["api"]}, "frontend": "git@github.com:org/frontend.git"}}' > .meta.yaml
    run "$META_BIN" --tag api git worktree exec global-filter -- touch ran
    [ "$status" -ne 0 ]
    [[ "$output" == *"does not take --include/--exclude/--tag before it"* ]]
    [ -z "$(find .worktrees/global-filter -name ran)" ]
}

@test "inside a worktree set, a filter typo is an error" {
    "$META_BIN" git worktree create tagged --repo backend --repo frontend
    cd .worktrees/tagged
    run "$META_BIN" --exclude typo exec -- pwd
    [ "$status" -ne 0 ]
    [[ "$output" == *"--exclude 'typo' names none of the repos"* ]]
    run "$META_BIN" --tag nonexistent exec -- pwd
    [ "$status" -ne 0 ]
    [[ "$output" == *"--tag 'nonexistent' is no project's tag"* ]]
}

@test "in a set made elsewhere without its root, --tag uses the workspace's config" {
    cat > .meta.yaml <<'EOF'
{
    "projects": {
        "backend": { "repo": "git@github.com:org/backend.git", "tags": ["back"] },
        "frontend": { "repo": "git@github.com:org/frontend.git", "tags": ["front"] }
    }
}
EOF
    OUT="$(cd "$(mktemp -d)" && pwd -P)"
    META_WORKTREES="$OUT/.worktrees" "$META_BIN" git worktree create tagged --repo backend --repo frontend --no-deps
    cd "$OUT/.worktrees/tagged"
    run "$META_BIN" --tag front exec -- touch ran
    [ "$status" -eq 0 ]
    [ -f frontend/ran ]
    [ ! -e backend/ran ]

    # Without a config to read the tags from, --tag is an error, not "every repo"
    mv "$TEST_DIR/.meta.yaml" "$TEST_DIR/meta.json.bak"
    rm frontend/ran
    run "$META_BIN" --tag front exec -- touch ran
    [ "$status" -ne 0 ]
    [[ "$output" == *"cannot select its repos"* ]]
    [ -z "$(find . -name ran)" ]
    cd /
    rm -rf "$OUT"
}

@test "worktree exec --include works with a relative META_WORKTREES" {
    "$META_BIN" git worktree create rel-root --repo backend --repo frontend
    run env META_WORKTREES=.worktrees "$META_BIN" git worktree exec rel-root --include backend -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"rel-root/backend"* ]]
    [[ "$output" != *"rel-root/frontend"* ]]
}

@test "--primary inside a set acts on the workspace the set was made from" {
    # the set holds the root, so it has a copy of .meta.yaml of its own
    "$META_BIN" git worktree create fix --repo backend
    [ -f .worktrees/fix/.meta.yaml ]
    cd .worktrees/fix/backend
    run "$META_BIN" -i frontend exec -- pwd
    [ "$status" -ne 0 ]
    run "$META_BIN" --primary -i frontend exec -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"$TEST_DIR/frontend"* ]]
    [[ "$output" != *".worktrees"* ]]
    run "$META_BIN" --primary context --json
    [ "$status" -eq 0 ]
    [[ "$output" == *"\"name\": \"$(basename "$TEST_DIR")\""* ]]
    [[ "$output" == *"\"frontend\""* ]]
}

@test "--primary in a set whose records name two workspaces says so, without pointing back at --primary" {
    "$META_BIN" git worktree create mixed --repo backend --repo frontend --no-deps
    record="$(git -C .worktrees/mixed/frontend rev-parse --absolute-git-dir)/meta-worktree.json"
    python3 - "$record" "$(mktemp -d)" <<'PY'
import json, sys
path, other = sys.argv[1], sys.argv[2]
record = json.load(open(path)); record["project"] = other
json.dump(record, open(path, "w"))
PY
    cd .worktrees/mixed/backend
    run "$META_BIN" --primary exec -- pwd
    [ "$status" -ne 0 ]
    [[ "$output" == *"--primary can't tell which workspace worktree set"* ]]
    [[ "$output" == *"Run meta from that workspace's directory"* ]]
    [[ "$output" != *"Pass --primary"* ]]
}
