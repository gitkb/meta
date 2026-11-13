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

    run "$META_BIN" --json git worktree status bare-member
    git -C backend config --local core.bare false
    [ "$status" -eq 0 ]
    [ "$(printf '%s' "$output" | jq -r '.results[0].data.repos[] | select(.alias == "backend") | .branch')" = "bare-member" ]
}

@test "worktree create --every-repo creates worktrees for all repos" {
    run "$META_BIN" git worktree create full-task --every-repo
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
    # A branch collision is caught before git now. A checkout hook still
    # gives a native git failure with more than one stderr line.
    printf '#!/bin/sh\nprintf "fatal: checkout hook failed\\nsecond line\\n" >&2\nexit 1\n' > backend/.git/hooks/post-checkout
    chmod +x backend/.git/hooks/post-checkout
    run "$META_BIN" git worktree create login --repo backend --no-deps
    [ "$status" -ne 0 ]
    [[ "$output" == *$'\n  fatal: '*"checkout hook failed"* ]]
    [[ "$output" == *$'\n  second line'* ]]
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
    # --every-repo holds backend: its problems count too
    run "$META_BIN" --strict --dry-run git worktree create every --every-repo
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
    run "$META_BIN" --strict git worktree list --metadata-only
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
    run "$META_BIN" --dry-run -r git worktree create t --repo clients/desktop
    [ "$status" -eq 0 ]
    [[ "$output" == *"Would clone the meta repos on the way to the repos asked for (clients)"* ]]
    [ ! -e clients ]
    run "$META_BIN" -r git worktree create t --repo clients/desktop
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
    [[ "$output" != *"git update"* ]]
    [ ! -e api ] && [ ! -e .worktrees/blocked ]
    mv web web.aside
    run "$META_BIN" git worktree create blocked --repo api
    [ "$status" -eq 0 ]
    [ -d .worktrees/blocked/web ]
}

@test "worktree create clones the meta repos on the way to what a nested repo depends on, in one go" {
    # bare remotes: repo NAME [FILE CONTENT]...
    remote() {
        local name=$1; shift
        git init --quiet --initial-branch=main "$TEST_DIR/src-$name"
        while [ $# -ge 2 ]; do printf '%s\n' "$2" > "$TEST_DIR/src-$name/$1"; shift 2; done
        git -C "$TEST_DIR/src-$name" add -A
        git -C "$TEST_DIR/src-$name" commit --quiet --allow-empty -m "$name"
        git clone --quiet --bare "$TEST_DIR/src-$name" "$TEST_DIR/$name.git"
    }
    remote desktop
    remote sdk
    remote core
    remote gitkb .meta.yaml "projects: {core: \"file://$TEST_DIR/core.git\"}" .gitignore "core/"
    remote open-source .meta.yaml "projects: {sdk: \"file://$TEST_DIR/sdk.git\", gitkb: {repo: \"file://$TEST_DIR/gitkb.git\", meta: true}}" .gitignore "sdk/
gitkb/"
    remote clients .meta.yaml "projects: {desktop: {repo: \"file://$TEST_DIR/desktop.git\", depends_on: [open-source/sdk, open-source/gitkb/core]}}" .gitignore "desktop/"
    cat > .meta.yaml <<EOF
projects:
  clients: {repo: "file://$TEST_DIR/clients.git", meta: true}
  open-source: {repo: "file://$TEST_DIR/open-source.git", meta: true}
  backend: "git@github.com:org/backend.git"
EOF
    run "$META_BIN" -r git worktree create t --repo clients/desktop
    [ "$status" -eq 0 ]
    [[ "$output" != *"warning"* ]]
    for repo in clients clients/desktop open-source open-source/sdk open-source/gitkb open-source/gitkb/core; do
        [ -e ".worktrees/t/$repo/.git" ]
    done
}

@test "worktree create clones the meta repo on the way to what a provider it needs depends on" {
    remote() {
        local name=$1; shift
        git init --quiet --initial-branch=main "$TEST_DIR/src-$name"
        while [ $# -ge 2 ]; do printf '%s\n' "$2" > "$TEST_DIR/src-$name/$1"; shift 2; done
        git -C "$TEST_DIR/src-$name" add -A
        git -C "$TEST_DIR/src-$name" commit --quiet --allow-empty -m "$name"
        git clone --quiet --bare "$TEST_DIR/src-$name" "$TEST_DIR/$name.git"
    }
    remote app
    remote service
    remote sdk
    remote vendor .meta.yaml "projects: {sdk: \"file://$TEST_DIR/sdk.git\"}" .gitignore "sdk/"
    # app needs api, which service provides; service needs vendor/sdk
    cat > .meta.yaml <<EOF
projects:
  app: {repo: "file://$TEST_DIR/app.git", depends_on: [api]}
  service: {repo: "file://$TEST_DIR/service.git", provides: [api], depends_on: [vendor/sdk]}
  vendor: {repo: "file://$TEST_DIR/vendor.git", meta: true}
  backend: "git@github.com:org/backend.git"
EOF
    run "$META_BIN" -r --strict git worktree create t --repo app
    [ "$status" -eq 0 ]
    [[ "$output" != *"warning"* ]]
    for repo in app service vendor vendor/sdk; do
        [ -e ".worktrees/t/$repo/.git" ]
    done
}

@test "worktree create -r --strict fails on a bad depends_on after cloning the meta repo on the way" {
    git init --quiet --initial-branch=main "$TEST_DIR/src-desktop"
    git -C "$TEST_DIR/src-desktop" commit --quiet --allow-empty -m desktop
    git clone --quiet --bare "$TEST_DIR/src-desktop" "$TEST_DIR/desktop.git"
    git init --quiet --initial-branch=main "$TEST_DIR/src-clients"
    printf 'projects:\n  desktop: {repo: "file://%s/desktop.git", depends_on: [nowhere]}\n' "$TEST_DIR" > "$TEST_DIR/src-clients/.meta.yaml"
    echo "desktop/" > "$TEST_DIR/src-clients/.gitignore"
    git -C "$TEST_DIR/src-clients" add -A
    git -C "$TEST_DIR/src-clients" commit --quiet -m clients
    git clone --quiet --bare "$TEST_DIR/src-clients" "$TEST_DIR/clients.git"
    cat > .meta.yaml <<EOF
projects:
  clients: {repo: "file://$TEST_DIR/clients.git", meta: true}
  backend: "git@github.com:org/backend.git"
EOF
    run "$META_BIN" -r --strict git worktree create t --repo clients/desktop
    [ "$status" -ne 0 ]
    [[ "$output" == *"Cloned clients"* ]]
    [[ "$output" == *"'clients/desktop' depends on 'clients/nowhere'"*"(strict mode)"* ]]
    [ ! -e .worktrees/t ]
    # without --strict, the same is a warning, once
    run "$META_BIN" -r git worktree create t --repo clients/desktop
    [ "$status" -eq 0 ]
    [ "$(grep -c "depends on 'clients/nowhere'" <<<"$output")" -eq 1 ]
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
    [[ "$output" == *"meta --include 'web,db' git update"* ]]
    [[ "$output" == *"Cloning failed"* ]]
    [ ! -e ".worktrees/denied" ]
}

@test "worktree create --every-repo skips a repo that is not cloned; --repo and add refuse it when it can't be cloned" {
    rm -rf frontend
    # a remote that can't be cloned (as without access), kept offline
    sed -i.bak "s#git@github.com:org/frontend.git#file://$TEST_DIR/nowhere.git#" .meta.yaml
    run "$META_BIN" git worktree create some --every-repo
    [ "$status" -eq 0 ]
    [[ "$output" == *"'frontend' at $TEST_DIR/frontend: not cloned"*"skipping"* ]]
    [ -d ".worktrees/some/backend" ]
    [ ! -e ".worktrees/some/frontend" ]

    # all or nothing with --strict
    run "$META_BIN" --strict git worktree create strict --every-repo
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
    run "$META_BIN" --json git worktree create json-test --repo backend
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "import json,sys; d=json.load(sys.stdin)['results'][0]['data']; assert d['name']=='json-test' and [r['alias'] for r in d['repos']]==['.','backend'], d"
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

@test "worktree create and add start a new branch from the project's branch: when its config names one" {
    for repo in backend frontend; do
        git -C "$repo" branch release
        git -C "$repo" switch --quiet release
        git -C "$repo" commit --quiet --allow-empty -m "release work"
        git -C "$repo" switch --quiet main
        git clone --quiet --bare "$repo" "$TEST_DIR/$repo.git"
        git -C "$repo" remote add origin "$TEST_DIR/$repo.git"
        git -C "$repo" fetch --quiet origin
        git -C "$repo" remote set-head origin main
    done
    printf '%s\n' '{"projects": {"backend": {"repo": "git@github.com:org/backend.git", "branch": "release"},' \
        '"frontend": "git@github.com:org/frontend.git"}}' > .meta.yaml
    run "$META_BIN" --dry-run git worktree create rel --repo backend --repo frontend --no-deps
    [ "$status" -eq 0 ]
    [[ "$output" == *"origin/release"* ]]
    run "$META_BIN" git worktree create rel --repo backend --no-deps
    [ "$status" -eq 0 ]
    [[ "$output" == *"backend -> rel (new from origin/release)"* ]]
    [ "$(git -C .worktrees/rel/backend rev-parse HEAD)" = "$(git -C backend rev-parse origin/release)" ]
    # one without branch: starts from the remote's default branch, as before
    run "$META_BIN" git worktree add rel --repo frontend
    [ "$status" -eq 0 ]
    [ "$(git -C .worktrees/rel/frontend rev-parse HEAD)" = "$(git -C frontend rev-parse origin/main)" ]
    # nothing moves the checkouts themselves
    [ "$(git -C backend branch --show-current)" = main ]
    # a branch: origin doesn't have is named, with how to fetch it
    git -C backend update-ref -d refs/remotes/origin/release
    run "$META_BIN" git worktree create gone --repo backend --no-deps
    [ "$status" -ne 0 ]
    [[ "$output" == *"its .meta.yaml names branch 'release', and there is no origin/release to start 'gone' from; fetch it with: git -C "*"fetch origin "*"refs/heads/release:refs/remotes/origin/release"* ]]
    [ ! -e .worktrees/gone ]
}

@test "worktree create says how to fetch a branch: a single-branch or shallow clone lacks, and that works" {
    git -C backend branch release
    git -C backend switch --quiet release
    git -C backend commit --quiet --allow-empty -m "release work"
    git -C backend switch --quiet main
    git clone --quiet --bare backend "$TEST_DIR/backend.git"
    printf '%s\n' '{"projects": {"backend": {"repo": "git@github.com:org/backend.git", "branch": "release"},' \
        '"frontend": "git@github.com:org/frontend.git"}}' > .meta.yaml
    local shape advice
    for shape in "--single-branch --branch main" "--depth 1"; do
        rm -rf backend .worktrees/one
        # shellcheck disable=SC2086
        git clone --quiet $shape "file://$TEST_DIR/backend.git" backend
        run "$META_BIN" git worktree create one --repo backend --no-deps
        [ "$status" -ne 0 ]
        [[ "$output" == *"its .meta.yaml names branch 'release', and there is no origin/release to start 'one' from; fetch it with: git -C "* ]]
        [ ! -e .worktrees/one ]
        # what it says to run, run as said
        advice=$(sed -n 's/.*fetch it with: //p' <<<"$output" | head -1)
        eval "$advice"
        run "$META_BIN" git worktree create one --repo backend --no-deps
        [ "$status" -eq 0 ]
        [[ "$output" == *"backend -> one (new from origin/release)"* ]]
        [ "$(git -C .worktrees/one/backend rev-parse HEAD)" = "$(git -C "$TEST_DIR/backend.git" rev-parse release)" ]
        "$META_BIN" git worktree remove one --force >/dev/null
    done
}

@test "worktree create and add read branch: from the config --config names, as the rest of the command does" {
    for repo in backend frontend; do
        git -C "$repo" branch release
        git -C "$repo" switch --quiet release
        git -C "$repo" commit --quiet --allow-empty -m "release work"
        git -C "$repo" switch --quiet main
        git clone --quiet --bare "$repo" "$TEST_DIR/$repo.git"
        git -C "$repo" remote add origin "$TEST_DIR/$repo.git"
        git -C "$repo" fetch --quiet origin
        git -C "$repo" remote set-head origin main
    done
    # .meta.yaml names no branch; other.yaml names release
    printf '{"projects": {"backend": "file://%s/backend.git", "frontend": "file://%s/frontend.git"}}\n' \
        "$TEST_DIR" "$TEST_DIR" > .meta.yaml
    printf '{"projects": {"backend": {"repo": "file://%s/backend.git", "branch": "release"}, "frontend": {"repo": "file://%s/frontend.git", "branch": "release"}}}\n' \
        "$TEST_DIR" "$TEST_DIR" > other.yaml
    run "$META_BIN" --config other.yaml --dry-run git worktree create rel --repo backend --no-deps
    [ "$status" -eq 0 ]
    [[ "$output" == *"origin/release"* ]]
    run "$META_BIN" --config other.yaml git worktree create rel --repo backend --no-deps
    [ "$status" -eq 0 ]
    [[ "$output" == *"backend -> rel (new from origin/release)"* ]]
    [ "$(git -C .worktrees/rel/backend rev-parse HEAD)" = "$(git -C backend rev-parse origin/release)" ]
    run "$META_BIN" --config other.yaml git worktree add rel --repo frontend
    [ "$status" -eq 0 ]
    [ "$(git -C .worktrees/rel/frontend rev-parse HEAD)" = "$(git -C frontend rev-parse origin/release)" ]
    # without it, .meta.yaml's: the remote's default
    run "$META_BIN" git worktree create plain --repo backend --no-deps
    [ "$status" -eq 0 ]
    [[ "$output" == *"backend -> plain (new from origin/main)"* ]]
    # a project create clones for the set is cloned on other.yaml's branch
    "$META_BIN" git worktree remove plain --force >/dev/null
    "$META_BIN" git worktree remove rel --force >/dev/null
    rm -rf frontend
    run "$META_BIN" --config other.yaml git worktree create fresh --repo frontend --no-deps
    [ "$status" -eq 0 ]
    [ "$(git -C frontend branch --show-current)" = release ]
}

@test "worktree create with a commit-ish and a branch that exists says what to do" {
    git -C backend branch keepme
    run "$META_BIN" git worktree create keepme main --repo backend --no-deps
    [ "$status" -ne 0 ]
    [[ "$output" == *"Branch 'keepme' already exists in $TEST_DIR/backend, and a commit-ish ('main') starts a new branch. Drop the commit-ish to check out the existing branch"* ]]
    [[ "$output" != *"fatal:"* ]]
    [ ! -e .worktrees/keepme ]
    run "$META_BIN" git worktree create keepme --repo backend --no-deps
    [ "$status" -eq 0 ]
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
    run "$META_BIN" --json git worktree add add-json --repo frontend
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "import json,sys; [r]=json.load(sys.stdin)['results']; assert r['project']=='frontend' and r['data']['alias']=='frontend' and r['data']['branch']=='add-json', r"
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
    run "$META_BIN" --json git worktree list
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json,sys
data = json.load(sys.stdin)
assert any(r['data']['name'] == 'list-json' for r in data['results'])
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
    run "$META_BIN" --json git worktree status status-json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json,sys
d = json.load(sys.stdin)
repo = next(r for r in d['results'][0]['data']['repos'] if r['alias'] == 'backend')
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
    run "$META_BIN" --json git worktree diff diff-json
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json,sys
d = json.load(sys.stdin)
backend = next(r['data'] for r in d['results'] if r['project'] == 'backend')
assert backend['files_changed'] >= 1 and backend['files'] == ['README.md'], backend
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

@test "worktree exec runs the repos one after another, at once only with --parallel" {
    "$META_BIN" git worktree create in-turn --repo backend --repo frontend
    local log="$TEST_DIR/order.log"
    local step="echo start >> $log; sleep 0.5; echo end >> $log"
    run "$META_BIN" git worktree exec in-turn -- sh -c "$step"
    [ "$status" -eq 0 ]
    # in turn: each start is followed by its end
    [[ "$(tr '\n' ' ' < "$log")" == "start end start end "* ]]
    [[ "$(tr '\n' ' ' < "$log")" != *"start start"* ]]
    rm "$log"
    run "$META_BIN" --parallel git worktree exec in-turn -- sh -c "$step"
    [ "$status" -eq 0 ]
    [ "$(head -2 "$log" | tr '\n' ' ')" = "start start " ]
}

@test "worktree exec --include filters repos" {
    "$META_BIN" git worktree create exec-filter --repo backend --repo frontend
    run "$META_BIN" --include backend git worktree exec exec-filter -- echo found
    [ "$status" -eq 0 ]
    MATCH_COUNT=$(echo "$output" | grep -c "found" || true)
    [ "$MATCH_COUNT" -eq 1 ]
}

@test "worktree exec --exclude filters repos" {
    "$META_BIN" git worktree create exec-excl --repo backend --repo frontend
    run "$META_BIN" --exclude frontend git worktree exec exec-excl -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"backend"* ]]
    [[ "$output" != *"exec-excl/frontend"* ]]
}

@test "worktree exec --include names whole repos, never parts of paths" {
    "$META_BIN" git worktree create exec-whole --repo backend --repo frontend
    run "$META_BIN" --include end git worktree exec exec-whole -- touch ran
    [ "$status" -ne 0 ]
    [[ "$output" == *"--include 'end' names none of the repos"* ]]
    [ -z "$(find .worktrees/exec-whole -name ran)" ]
    run "$META_BIN" --exclude ./backend/ git worktree exec exec-whole -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"exec-whole/frontend"* ]]
    [[ "$output" != *"exec-whole/backend"* ]]
}

@test "worktree exec --parallel runs concurrently" {
    "$META_BIN" git worktree create exec-par --repo backend --repo frontend
    run "$META_BIN" --parallel git worktree exec exec-par -- echo parallel-ok
    [ "$status" -eq 0 ]
    [[ "$output" == *"parallel-ok"* ]]
}

@test "worktree exec --json produces structured output" {
    "$META_BIN" git worktree create exec-json --repo backend
    run "$META_BIN" --json git worktree exec exec-json -- echo json-out
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
    # a branch the set only checked out was not created for it: left, and said so
    run "$META_BIN" git worktree remove b2
    [ "$status" -eq 0 ]
    [[ "$output" == *"left branch 'existing' in $TEST_DIR/backend (no record says meta made it for this set)"* ]]
    git -C backend rev-parse --verify --quiet refs/heads/existing
}

@test "worktree remove deletes ignored files and a branch work was reset away from, as git does" {
    # ignored files are disposable without --force, as for git worktree remove;
    # untracked ones are refused
    "$META_BIN" git worktree create ig --repo backend --no-deps
    echo .env >> backend/.git/info/exclude
    echo SECRET=1 > .worktrees/ig/backend/.env
    touch .worktrees/ig/backend/new.txt
    run "$META_BIN" git worktree remove ig
    [ "$status" -ne 0 ]
    [[ "$output" == *"1 file untracked: new.txt"* ]]
    [[ "$output" != *".env"* ]]
    rm .worktrees/ig/backend/new.txt
    # a commit reset away is only in the reflogs: git counts it discarded
    git -C .worktrees/ig/backend commit --quiet --allow-empty -m "reset away"
    git -C .worktrees/ig/backend reset --quiet --hard HEAD~1
    run "$META_BIN" git worktree remove ig
    [ "$status" -eq 0 ]
    [[ "$output" == *"deleted branch 'ig' in $TEST_DIR/backend"* ]]
    [ ! -e .worktrees/ig ]
    [ -z "$(git -C backend for-each-ref refs/meta)" ]
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
    run "$META_BIN" --recursive git worktree create badrepo --repo $'a\033[2J\nFORGED'
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
            probe) run "$META_BIN" --dry-run git worktree create preview $'bad\033[2J\nFORGED' --repo . --no-deps ;;
            --from-pr) run "$META_BIN" --dry-run git worktree create preview --from-pr $'o/r#1\033[2J\nFORGED' --repo . --no-deps ;;
            --meta) run "$META_BIN" --dry-run git worktree create preview --meta $'k=v\033[2J\nFORGED' --repo . --no-deps ;;
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
    run "$META_BIN" git worktree create okb --every-repo --branch "$E"; check
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
    run "$META_BIN" --json git worktree diff diff-clean
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json, sys
d = json.load(sys.stdin)
for r in d['results']:
    assert (r['data']['files_changed'], r['data']['insertions'], r['data']['deletions']) == (0, 0, 0), r
"
}

# ============ Edge Cases: Status mixed dirty/clean ============

@test "worktree status shows mixed dirty and clean repos" {
    "$META_BIN" git worktree create mixed-status --repo backend --repo frontend
    # Make backend dirty
    echo "dirty change" >> ".worktrees/mixed-status/backend/README.md"
    # Leave frontend clean

    run "$META_BIN" --json git worktree status mixed-status
    [ "$status" -eq 0 ]
    echo "$output" | python3 -c "
import json, sys
d = json.load(sys.stdin)
repos = {r['alias']: r for r in d['results'][0]['data']['repos']}
assert repos['backend']['dirty'] == True, f'backend should be dirty: {repos[\"backend\"]}'
assert repos['frontend']['dirty'] == False, f'frontend should be clean: {repos[\"frontend\"]}'
"
}

# ============ Edge Cases: --every-repo and --repo mutually exclusive ============

@test "worktree create --every-repo --repo rejects combination" {
    run "$META_BIN" git worktree create all-repo-combo --every-repo --repo backend
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
    run "$META_BIN" --dry-run --recursive git worktree create parent-check --repo vendor/nested-lib
    [ "$status" -eq 0 ]
    [[ "$output" != *"depends on 'missing'"* ]]
    # asked for, its depends_on is followed, and checked
    run "$META_BIN" --strict --dry-run --recursive git worktree create parent-check --repo vendor
    [ "$status" -ne 0 ]
    [[ "$output" == *"'vendor' depends on 'missing'"*"(strict mode)"* ]]
}

@test "project list recovers nested projects through a missing parent in a legacy set" {
    bats_require_minimum_version 1.5.0
    setup_nested_meta
    git -C vendor init --quiet -b main
    printf '/nested-lib/\n' > vendor/.gitignore
    git -C vendor add .meta.yaml .gitignore
    git -C vendor commit --quiet -m vendor
    "$META_BIN" git worktree create legacy --repo backend
    # Reproduce sets made by older add: the child is recorded, its parent
    # is only a directory, and the root checkout predates vendor's config.
    git -C vendor/nested-lib worktree add --quiet -b legacy "$TEST_DIR/.worktrees/legacy/vendor/nested-lib"
    local admin child
    admin="$(git -C .worktrees/legacy/backend rev-parse --absolute-git-dir)"
    child="$(git -C .worktrees/legacy/vendor/nested-lib rev-parse --absolute-git-dir)"
    cp "$admin/meta-worktree.json" "$child/meta-worktree.json"
    [ ! -e .worktrees/legacy/vendor/.git ]
    [ ! -e .worktrees/legacy/vendor/.meta.yaml ]
    cd .worktrees/legacy/vendor/nested-lib
    for scope in -r -A; do
        run --separate-stderr "$META_BIN" "$scope" --json project list
        [ "$status" -eq 0 ]
        [ "$(jq -r '.results[] | select(.project == "vendor/nested-lib") | "\(.path)=\(.data.repo)"' <<<"$output")" = "$TEST_DIR/.worktrees/legacy/vendor/nested-lib=git@github.com:org/nested-lib.git" ]
        # Each place appears once even where both configs list it.
        [ "$(jq '[.results[].project] | length == (unique | length)' <<<"$output")" = true ]
    done
    run --separate-stderr "$META_BIN" --json project list
    [ "$status" -eq 0 ]
    [ "$(jq '[.results[] | select(.project == "vendor/nested-lib")] | length' <<<"$output")" = 0 ]
    cd "$TEST_DIR/.worktrees/legacy"
    # Checked-out entries win, while the missing parent's children still
    # come from the source config and mark it as a meta project.
    jq '.projects.vendor = {repo: "git@github.com:org/local-vendor.git", meta: true}' .meta.yaml > new.json
    mv new.json .meta.yaml
    run --separate-stderr "$META_BIN" -r --json project list
    [ "$status" -eq 0 ]
    [ "$(jq -r '.results[] | select(.project == "vendor") | "\(.data.repo)=\(.data.is_meta)"' <<<"$output")" = "git@github.com:org/local-vendor.git=true" ]
    [ "$(jq '[.results[] | select(.project == "vendor/nested-lib")] | length' <<<"$output")" = 1 ]
    cd "$TEST_DIR"
    run "$META_BIN" git worktree add legacy --repo vendor
    [ "$status" -ne 0 ]
    [[ "$output" == *"already holds files"* ]]
    [ -f .worktrees/legacy/vendor/nested-lib/.git ]
}

@test "worktree add adds intermediate parents and reuses parents already in the set" {
    setup_nested_meta
    git -C vendor init --quiet -b main
    printf '/nested-lib/\n' > vendor/.gitignore
    git -C vendor add .meta.yaml .gitignore
    git -C vendor commit --quiet -m vendor
    "$META_BIN" git worktree create spine --repo backend
    run "$META_BIN" --json git worktree add spine --repo vendor/nested-lib
    [ "$status" -eq 0 ]
    [ -f .worktrees/spine/vendor/.git ]
    [ -f .worktrees/spine/vendor/nested-lib/.git ]
    [ "$(jq -c '[.results[].project]' <<<"$output")" = '["vendor","vendor/nested-lib"]' ]
    "$META_BIN" git worktree create parent-present --repo vendor
    run "$META_BIN" git worktree add parent-present --repo vendor/nested-lib
    [ "$status" -eq 0 ]
    [ -f .worktrees/parent-present/vendor/nested-lib/.git ]
}

@test "worktree create reports the set holding a branch before making a set" {
    bats_require_minimum_version 1.5.0
    "$META_BIN" git worktree create holder --repo backend
    run --separate-stderr "$META_BIN" --json git worktree create blocked --branch holder --repo backend
    [ "$status" -ne 0 ]
    [ "$(jq -r '.error.code' <<<"$output")" = conflict ]
    [[ "$(jq -r '.error.message' <<<"$output")" == *"holder"*"$TEST_DIR/.worktrees/holder"* ]]
    [ "$(jq -r '.error.hint' <<<"$output")" != null ]
    [ ! -e .worktrees/blocked ]
    run --separate-stderr "$META_BIN" --dry-run --json git worktree create preview --branch holder --repo backend
    [ "$status" -ne 0 ]
    [ "$(jq -r '.error.code' <<<"$output")" = conflict ]
    # A deleted checkout still reserves the branch until git forgets it.
    # Its admin record must still identify the holding set.
    rm -rf .worktrees/holder
    run --separate-stderr "$META_BIN" --json git worktree create missing-holder --branch holder --repo backend
    [ "$status" -ne 0 ]
    [ "$(jq -r '.error.code' <<<"$output")" = conflict ]
    [[ "$(jq -r '.error.message' <<<"$output")" == *"worktree set 'holder'"*"$TEST_DIR/.worktrees/holder"* ]]
    [ ! -e .worktrees/missing-holder ]
}

@test "worktree add reports the set holding a branch before adding any repo" {
    bats_require_minimum_version 1.5.0
    "$META_BIN" git worktree create holder --repo frontend
    "$META_BIN" git worktree create destination --repo backend --no-deps
    # A free repo first verifies that the whole plan is checked before adding.
    mkdir tool
    git -C tool init --quiet -b main
    echo tool > tool/README.md
    git -C tool add README.md
    git -C tool commit --quiet -m tool
    jq '.projects.tool = "git@github.com:org/tool.git"' .meta.yaml > new.json
    mv new.json .meta.yaml
    run --separate-stderr "$META_BIN" --json git worktree add destination --repo tool:fresh --repo frontend:holder
    [ "$status" -ne 0 ]
    [ "$(jq -r '.error.code' <<<"$output")" = conflict ]
    [[ "$(jq -r '.error.message' <<<"$output")" == *"holder"*"$TEST_DIR/.worktrees/holder"* ]]
    [ ! -e .worktrees/destination/tool ]
    [ ! -e .worktrees/destination/frontend ]
    if git -C tool rev-parse --verify -q fresh; then false; fi
}

@test "inside a set, -r reaches the meta repos checked out in it, as outside one" {
    bats_require_minimum_version 1.5.0
    setup_nested_meta
    git -C vendor init --quiet -b main
    printf '/nested-lib/\n' > vendor/.gitignore
    git -C vendor add -A
    git -C vendor commit --quiet -m init
    printf '/vendor/\n/backend/\n/frontend/\n.worktrees/\n' >> .gitignore
    git add -A && git commit --quiet -m vendor
    run "$META_BIN" -r git worktree create deep --every-repo
    [ "$status" -eq 0 ]
    [ -d .worktrees/deep/vendor/nested-lib/.git ] || [ -f .worktrees/deep/vendor/nested-lib/.git ]
    cd .worktrees/deep/backend
    local rows='[.results[] | "\(.project)=\(.path)=\(.data.repo)"] | join(" ")'
    run --separate-stderr "$META_BIN" -r --json project list
    [ "$status" -eq 0 ]
    # the nested repo, at the set's checkout, with the URL a PR names it by
    [[ " $(jq -r "$rows" <<<"$output") " == *" vendor/nested-lib=$TEST_DIR/.worktrees/deep/vendor/nested-lib=git@github.com:org/nested-lib.git "* ]]
    run --separate-stderr "$META_BIN" -A --json project list
    [[ " $(jq -r "$rows" <<<"$output") " == *" vendor/nested-lib="* ]]
    # without -r, the first level only, as outside a set
    run --separate-stderr "$META_BIN" --json project list
    [ "$status" -eq 0 ]
    [[ " $(jq -r "$rows" <<<"$output") " != *" vendor/nested-lib="* ]]
    [[ " $(jq -r "$rows" <<<"$output") " == *" vendor=$TEST_DIR/.worktrees/deep/vendor="* ]]
    # dependents reach as far
    run "$META_BIN" -r project dependents nested-lib
    [ "$status" -eq 0 ]
}

@test "a set of a workspace whose root is no repo names its own folder as the root" {
    bats_require_minimum_version 1.5.0
    rm -rf .git
    run "$META_BIN" git worktree create loose-root --repo backend
    [ "$status" -eq 0 ]
    [ ! -e .worktrees/loose-root/.meta.yaml ]
    cd .worktrees/loose-root/backend
    run --separate-stderr "$META_BIN" --json project list
    [ "$status" -eq 0 ]
    local set="$TEST_DIR/.worktrees/loose-root"
    [ "$(jq -r '.results[0].path' <<<"$output")" = "$set" ]
    [ "$(jq -r '.results[0].data.worktree.physical_root' <<<"$output")" = "$set" ]
    [ "$(jq -r '.results[] | select(.data.name == "backend") | "\(.project)=\(.path)"' <<<"$output")" = "backend=$set/backend" ]
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

    run "$META_BIN" --recursive git worktree create two-names --repo nested-lib:one --repo vendor/nested-lib:two
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
    "$META_BIN" --recursive git worktree create nest --repo vendor --repo vendor/nested-lib
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

@test "a checkout in a set is a member even when the set's checked-out config doesn't list it" {
    bats_require_minimum_version 1.5.0
    setup_nested_meta
    # vendor's commit has no .meta.yaml: only the workspace's file on disk
    # lists nested-lib, as when the workspace's configs aren't committed
    echo "nested-lib/" > vendor/.gitignore
    git -C vendor init --quiet --initial-branch=main
    git -C vendor add .gitignore
    git -C vendor commit --quiet -m "vendor"
    "$META_BIN" -r git worktree create full --repo vendor --repo vendor/nested-lib
    cd .worktrees/full
    run --separate-stderr "$META_BIN" -r --json exec -- pwd
    [ "$status" -eq 0 ]
    [ "$(jq -c '[.results[].project]' <<<"$output")" = '[".","vendor","vendor/nested-lib"]' ]
    # from inside the nested checkout, the command reaches the whole set
    cd vendor/nested-lib
    run --separate-stderr "$META_BIN" --json exec -- pwd
    [ "$status" -eq 0 ]
    [ "$(jq -c '[.results[].project]' <<<"$output")" = '[".","vendor","vendor/nested-lib"]' ]
    # moved by hand, the set keeps its checkouts as members
    cd "$TEST_DIR"
    mv .worktrees/full .worktrees/moved
    cd .worktrees/moved
    run --separate-stderr "$META_BIN" -r --json exec -- pwd
    [ "$status" -eq 0 ]
    [ "$(jq -c '[.results[].project]' <<<"$output")" = '[".","vendor","vendor/nested-lib"]' ]
    # a set made for the nested repo (create checks vendor out to hold it)
    cd "$TEST_DIR"
    "$META_BIN" -r git worktree create leaf --repo vendor/nested-lib --no-deps
    cd .worktrees/leaf
    run --separate-stderr "$META_BIN" --json exec -- pwd
    [ "$status" -eq 0 ]
    [ "$(jq -c '[.results[].project]' <<<"$output")" = '["vendor","vendor/nested-lib"]' ]
}

@test "a set's root checkout is a member when the workspace repo doesn't commit its config" {
    bats_require_minimum_version 1.5.0
    # the workspace repo keeps .meta.yaml out of git: its checkout in a set
    # has none
    echo ".meta.yaml" >> .gitignore
    git rm --cached --quiet .meta.yaml
    git add .gitignore
    git commit --quiet -m "config not committed"
    "$META_BIN" git worktree create rooted --repo backend
    [ -e .worktrees/rooted/.git ]
    [ ! -e .worktrees/rooted/.meta.yaml ]
    echo work > .worktrees/rooted/untracked.txt
    cd .worktrees/rooted
    run --separate-stderr "$META_BIN" --json exec -- pwd
    [ "$status" -eq 0 ]
    [ "$(jq -c '[.results[].project]' <<<"$output")" = '[".","backend"]' ]
    run "$META_BIN" git status --short
    [ "$status" -eq 0 ]
    [[ "$output" == *"untracked.txt"* ]]
    # a set without a root checkout has no root member, whatever lies in it
    cd "$TEST_DIR"
    "$META_BIN" git worktree create rootless --repo backend --no-deps
    echo notes > .worktrees/rootless/notes.txt
    cd .worktrees/rootless
    run --separate-stderr "$META_BIN" --json exec -- pwd
    [ "$status" -eq 0 ]
    [ "$(jq -c '[.results[].project]' <<<"$output")" = '["backend"]' ]
}

@test "loop commands in a set reach its nested members" {
    bats_require_minimum_version 1.5.0
    setup_nested_meta
    echo "nested-lib/" > vendor/.gitignore
    git -C vendor init --quiet --initial-branch=main
    git -C vendor add .gitignore .meta.yaml
    git -C vendor commit --quiet -m "vendor"
    "$META_BIN" -r git worktree create nest --repo vendor --repo vendor/nested-lib
    echo changed >> .worktrees/nest/vendor/nested-lib/README.md
    cd .worktrees/nest
    run "$META_BIN" git status -s
    [ "$status" -eq 0 ]
    [[ "$output" == *"vendor/nested-lib"*"M README.md"* ]]
    run "$META_BIN" git add -u
    [ "$status" -eq 0 ]
    run "$META_BIN" git commit -m "nested change"
    [ "$status" -eq 0 ]
    [ "$(git -C vendor/nested-lib log -1 --format=%s)" = "nested change" ]
    cd "$TEST_DIR"
    run "$META_BIN" git worktree exec nest -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *".worktrees/nest/vendor/nested-lib"* ]]
    # a nested member is chosen by its path in the set
    run "$META_BIN" --include vendor/nested-lib git worktree exec nest -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *"1 commands complete"* ]]
    # the set is the scope: -r adds nothing, so no hint names it; -A is the
    # whole set, not the workspace around it
    cd .worktrees/nest
    # (the set's own .meta.yaml, from before vendor was added to the
    # workspace's, which -A would show from the workspace)
    run --separate-stderr "$META_BIN" -A --json context --no-status
    [ "$(jq -r '.results[0].data.name' <<<"$output")" = nest ]
    [[ "$(jq -r '[.results[1:][].data.path] | join(" ")' <<<"$output")" != *"vendor"* ]]
    run "$META_BIN" -r --include nope git status
    [ "$status" -ne 0 ]
    [[ "$output" == *"--include 'nope' names none"* ]]
    [[ "$output" != *"--recursive"* ]]
    # a dirty nested member stops remove without --force, by name
    echo again >> vendor/nested-lib/README.md
    cd "$TEST_DIR"
    run "$META_BIN" git worktree remove nest
    [ "$status" -ne 0 ]
    [[ "$output" == *"nested-lib"* ]]
    [ -e .worktrees/nest/vendor/nested-lib/README.md ]
}

@test "meta's --include before worktree exec picks the set's repos" {
    "$META_BIN" git worktree create pick --repo backend --repo frontend
    run "$META_BIN" --include backend git worktree exec pick -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *".worktrees/pick/backend"* ]]
    [[ "$output" != *".worktrees/pick/frontend"* ]]
    run "$META_BIN" --exclude backend git worktree exec pick -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *".worktrees/pick/frontend"* ]]
    [[ "$output" != *".worktrees/pick/backend"* ]]
    # a name the set doesn't hold is an error naming the set
    run "$META_BIN" --include nope git worktree exec pick -- pwd
    [ "$status" -ne 0 ]
    [[ "$output" == *"--include 'nope' names none"* ]]
    [[ "$output" != *"--recursive"* ]]
    # by the set's own names: . is the set's root
    run "$META_BIN" --include . git worktree exec pick -- pwd
    [ "$status" -eq 0 ]
    [[ "$output" == *".worktrees/pick"* ]]
    [[ "$output" != *".worktrees/pick/backend"* ]]
    # other worktree commands refuse a selection
    run "$META_BIN" --include backend git worktree status pick
    [ "$status" -ne 0 ]
    [[ "$output" == *"does not take --include"* ]]
}

@test "worktree add with nested repo alias succeeds" {
    setup_nested_meta

    git -C vendor init --quiet -b main
    printf '/nested-lib/\n' > vendor/.gitignore
    git -C vendor add .meta.yaml .gitignore
    git -C vendor commit --quiet -m vendor

    # First create worktree with backend
    "$META_BIN" git worktree create add-nested --repo backend

    # Then add nested repo
    run "$META_BIN" git worktree add add-nested --repo vendor/nested-lib
    [ "$status" -eq 0 ]
    [ -f ".worktrees/add-nested/vendor/.git" ]
    [ -f ".worktrees/add-nested/vendor/nested-lib/.git" ]
    cd .worktrees/add-nested/vendor/nested-lib
    run "$META_BIN" -r --json project list
    [ "$status" -eq 0 ]
    [ "$(jq -r '.results[] | select(.project == "vendor/nested-lib") | .path' <<<"$output")" = "$TEST_DIR/.worktrees/add-nested/vendor/nested-lib" ]
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

@test "meta's --exclude and --tag before git worktree exec select the set's repos" {
    "$META_BIN" git worktree create global-filter --repo backend --repo frontend
    run "$META_BIN" --exclude frontend --exclude . git worktree exec global-filter -- touch ran
    [ "$status" -eq 0 ]
    [ "$(find .worktrees/global-filter -name ran)" = .worktrees/global-filter/backend/ran ]
    rm .worktrees/global-filter/backend/ran
    # --tag by the tags of the set's own .meta.yaml
    echo '{"projects": {"backend": {"repo": "git@github.com:org/backend.git", "tags": ["api"]}, "frontend": "git@github.com:org/frontend.git"}}' > .worktrees/global-filter/.meta.yaml
    run "$META_BIN" --tag api git worktree exec global-filter -- touch ran
    [ "$status" -eq 0 ]
    [ "$(find .worktrees/global-filter -name ran)" = .worktrees/global-filter/backend/ran ]
    # other worktree commands refuse a selection rather than ignore it
    run "$META_BIN" --tag api git worktree remove global-filter --force
    [ "$status" -ne 0 ]
    [[ "$output" == *"does not take --include/--exclude/--tag"* ]]
    [ -d .worktrees/global-filter ]
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

@test "a relative or empty META_WORKTREES is refused, naming it" {
    "$META_BIN" git worktree create rel-root --repo backend --repo frontend
    for value in .worktrees ""; do
        run env META_WORKTREES="$value" "$META_BIN" --include backend git worktree exec rel-root -- pwd
        [ "$status" -ne 0 ]
        [[ "$output" == *"META_WORKTREES is '$value', not an absolute path"* ]]
        run env META_WORKTREES="$value" "$META_BIN" git worktree create other --repo backend
        [ "$status" -ne 0 ]
        [ ! -e other ] && [ ! -e .worktrees/other ] && [ ! -e backend/other ]
    done
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
    run "$META_BIN" --primary --json context
    [ "$status" -eq 0 ]
    [[ "$output" == *"\"name\": \"$(basename "$TEST_DIR")\""* ]]
    [[ "$output" == *"\"frontend\""* ]]
}

@test "--primary in a set whose owner file can't be read says so, without pointing back at --primary" {
    "$META_BIN" git worktree create mixed --repo backend --repo frontend --no-deps
    echo '{broken' > .worktrees/.meta-sets/mixed.json
    cd .worktrees/mixed/backend
    run "$META_BIN" --primary exec -- pwd
    [ "$status" -ne 0 ]
    [[ "$output" == *"--primary can't tell which workspace worktree set"* ]]
    [[ "$output" == *"Run meta from that workspace's directory"* ]]
    [[ "$output" != *"Pass --primary"* ]]
}
