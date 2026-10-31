#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"
load "${BATS_TEST_DIRNAME}/helpers/terminal.bash"

# `--dry-run` changes nothing: for each command on this closed list, the
# whole sandbox (workspace, every repo's .git and worktree sets, remotes,
# HOME, XDG and meta data dirs) is the same before and after, and no stub
# (editor, ssh, hook) ran. A command that can't preview refuses.

setup() {
    clear_git_local_env
    isolate_user_env
    TARGET="$BATS_TEST_DIRNAME/../target/debug"
    if [ ! -f "$TARGET/meta" ] || [ ! -f "$TARGET/meta-git" ]; then
        cargo build --workspace --quiet
    fi
    SANDBOX="$BATS_TEST_TMPDIR"
    CALLS="$SANDBOX/calls"
    # a temporary file a preview made would show in the snapshot too
    export TMPDIR="$SANDBOX/tmp"
    mkdir -p "$TMPDIR"

    # Programs a preview must never start: each records that it ran
    STUBS="$SANDBOX/stubs"
    mkdir -p "$STUBS"
    for stub in ssh editor gh; do
        printf '#!/bin/sh\necho "%s $*" >> "%s"\nexit 1\n' "$stub" "$CALLS" > "$STUBS/$stub"
        chmod +x "$STUBS/$stub"
    done
    export PATH="$STUBS:$(cd "$TARGET" && pwd -P):$PATH"
    export GIT_EDITOR="$STUBS/editor" EDITOR="$STUBS/editor"
    export GIT_AUTHOR_NAME=Test GIT_AUTHOR_EMAIL=test@example.invalid
    export GIT_COMMITTER_NAME=Test GIT_COMMITTER_EMAIL=test@example.invalid

    # A workspace with two cloned repos and one not cloned yet (over ssh)
    mkdir -p "$SANDBOX/remotes"
    for repo in backend frontend; do
        git init --quiet --initial-branch=main "$SANDBOX/seed/$repo"
        echo "# $repo" > "$SANDBOX/seed/$repo/README.md"
        git -C "$SANDBOX/seed/$repo" add README.md
        git -C "$SANDBOX/seed/$repo" commit --quiet -m "init $repo"
        git clone --quiet --bare "$SANDBOX/seed/$repo" "$SANDBOX/remotes/$repo.git"
    done
    WS="$SANDBOX/ws"
    git init --quiet --initial-branch=main "$WS"
    cat > "$WS/.meta.yaml" <<EOF
{
    "projects": {
        "backend": "file://$SANDBOX/remotes/backend.git",
        "frontend": "file://$SANDBOX/remotes/frontend.git",
        "missing": "git@example.invalid:org/missing.git"
    },
    "worktree": {"hooks": {
        "post-create": "cat >> $SANDBOX/hooks",
        "post-destroy": "cat >> $SANDBOX/hooks",
        "post-prune": "cat >> $SANDBOX/hooks"
    }}
}
EOF
    printf '/backend/\n/frontend/\n/missing/\n/.worktrees/\n' > "$WS/.gitignore"
    git -C "$WS" add .meta.yaml .gitignore
    git -C "$WS" commit --quiet -m workspace
    for repo in backend frontend; do
        git clone --quiet "file://$SANDBOX/remotes/$repo.git" "$WS/$repo"
    done
    cd "$WS"

    # A clean set, a set with work only --force discards, staged changes
    meta git worktree create clean --repo backend >/dev/null
    meta git worktree create dirty --repo backend >/dev/null
    echo "unsaved" > .worktrees/dirty/backend/notes.txt
    echo change >> backend/README.md
    git -C backend add README.md
}

teardown() {
    cd /
}

# Everything under the sandbox: each path's type, mode, link target and
# content hash; then each repo's refs and worktrees as git reports them
snapshot() {
    python3 - "$SANDBOX" <<'EOF'
import hashlib, os, stat, sys
root = sys.argv[1]
for top, dirs, files in os.walk(root):
    dirs.sort()
    for name in sorted(dirs + files):
        path = os.path.join(top, name)
        st = os.lstat(path)
        line = f"{os.path.relpath(path, root)} {oct(st.st_mode)}"
        if stat.S_ISLNK(st.st_mode):
            line += " -> " + os.readlink(path)
        elif stat.S_ISREG(st.st_mode):
            with open(path, "rb") as f:
                line += " " + hashlib.sha256(f.read()).hexdigest()
        print(line)
EOF
    for repo in "$WS" "$WS/backend" "$WS/frontend"; do
        git -C "$repo" for-each-ref
        git -C "$repo" worktree list --porcelain
    done
}

# Run `meta --dry-run ARGS...` (through $RUNNER, if set): it exits with $1
# and changes nothing
preview() {
    local expected="$1"
    shift
    local before
    before="$(snapshot)"
    run $RUNNER meta --dry-run "$@"
    echo "$output"
    [ "$status" -eq "$expected" ]
    [ ! -e "$CALLS" ]
    [ "$(snapshot)" = "$before" ]
}

@test "worktree create previews" {
    preview 0 git worktree create new --all
    [[ "$output" == *"Would create worktree set 'new'"* ]]
    # meta's own checks of each checkout fail a preview as a run
    git -C backend config core.bare true
    preview 1 git worktree create new --repo backend
    [[ "$output" == *"core.bare=true"* ]]
    git -C backend config core.bare false
    git -C backend symbolic-ref HEAD refs/heads/unborn
    preview 1 git worktree create new --repo backend --no-deps
    [[ "$output" == *"has no commit to branch from"* ]]
    git -C backend symbolic-ref HEAD refs/heads/main
}

@test "worktree create -r previews the nested repos when --dry-run comes first" {
    # backend is a meta repo of its own, with lib in it
    git init --quiet --initial-branch=main backend/lib
    echo lib > backend/lib/README.md
    git -C backend/lib add README.md
    git -C backend/lib commit --quiet -m lib
    printf '{"projects": {"lib": "file:///nowhere/lib.git"}}\n' > backend/.meta.yaml
    preview 0 -r git worktree --dry-run create t --all
    [[ "$output" == *"backend/lib:"* ]]
}

@test "worktree add previews" {
    preview 0 git worktree add clean --repo frontend
    [[ "$output" == *"Would add to worktree set 'clean'"* ]]
    preview 0 --json git worktree add clean --repo frontend
    python3 -c 'import json, sys; out = json.load(sys.stdin); assert out["dry_run"] and out["repos"][0]["alias"] == "frontend", out' <<< "$output"
    # meta's own check of the checkout fails a preview as a run
    git -C frontend config core.bare true
    preview 1 git worktree add clean --repo frontend
    [[ "$output" == *"core.bare=true"* ]]
    git -C frontend config core.bare false
    # as is a repo with no commit for the new branch to start at
    git -C frontend symbolic-ref HEAD refs/heads/unborn
    preview 1 git worktree add clean --repo frontend
    [[ "$output" == *"has no commit to branch from"* ]]
    git -C frontend symbolic-ref HEAD refs/heads/main
}

@test "a preview with nothing to create fails as a run does" {
    mkdir "$SANDBOX/empty"
    printf '{"projects": {}}\n' > "$SANDBOX/empty/.meta.yaml"
    cd "$SANDBOX/empty"
    preview 1 git worktree create e --all
    [[ "$output" == *"No repos selected; no worktree was created"* ]]
    preview 1 git worktree exec e --ephemeral --all -- true
    [[ "$output" == *"No repos selected; no worktree was created"* ]]
}

@test "worktree create --from-pr previews the checks a run makes before GitHub" {
    preview 1 git worktree create p --repo backend --from-pr nonsense
    [[ "$output" == *"Invalid --from-pr format"* ]]
    preview 1 git worktree create p --repo backend --from-pr someorg/unrelated#5
    [[ "$output" == *"No selected repo has origin 'someorg/unrelated'"* ]]
    # a PR of a selected repo previews with no call to GitHub (the gh stub)
    git -C backend remote set-url origin git@github.com:org/backend.git
    preview 0 git worktree create p --repo backend --from-pr org/backend#12
    [[ "$output" == *"<the head branch of org/backend#12>"* ]]
}

@test "worktree remove previews" {
    preview 0 git worktree remove clean
    [[ "$output" == *"Would remove worktree 'clean'"* ]]
    preview 0 --json git worktree remove clean
    [[ "$output" == *'"dry_run": true'* ]]
}

@test "a preview's git status leaves every index as it was" {
    # same content, new time: a status that could take the index lock
    # would refresh the index and write it
    touch -t 202001010000 .worktrees/clean/backend/README.md
    preview 0 git worktree remove clean
}

@test "worktree remove --force previews the work it would discard" {
    preview 0 git worktree remove dirty --force
    [[ "$output" == *"would discard work in worktree 'dirty'"* ]]
    [[ "$output" == *"notes.txt"* ]]
}

@test "worktree remove refuses work to lose in a preview too" {
    preview 1 git worktree remove dirty
    [[ "$output" == *"has work that removing it would lose"* ]]
}

@test "the worktree command's own --dry-run is the same preview" {
    before="$(snapshot)"
    run meta git worktree remove dirty --force --dry-run
    [ "$status" -eq 0 ]
    [[ "$output" == *"Would remove worktree 'dirty'"* ]]
    [ "$(snapshot)" = "$before" ]
}

@test "worktree prune previews" {
    meta git worktree create old --repo backend --ephemeral --ttl 1s >/dev/null
    sleep 2
    preview 0 git worktree prune
    [[ "$output" == *"Would prune 1 worktree(s)"* ]]
    preview 0 --json git worktree prune
    [[ "$output" == *'"dry_run": true'* ]]
    # a set prune keeps fails --strict, in a preview as in a run
    meta git worktree create kept --repo backend --ephemeral --ttl 1s >/dev/null
    echo work > .worktrees/kept/backend/untracked
    sleep 2
    preview 1 --strict git worktree prune
    [[ "$output" == *"Kept worktrees: kept"* ]]
}

@test "worktree exec previews" {
    preview 0 git worktree exec clean -- touch made-by-exec
    [[ "$output" == *"[DRY RUN]"*"touch made-by-exec"* ]]
}

@test "worktree exec --ephemeral creates, runs and removes nothing" {
    preview 0 git worktree exec eph --ephemeral --repo backend -- touch made-by-exec
    [[ "$output" == *"Would create worktree set 'eph'"* ]]
    [[ "$output" == *"Then would run \`touch made-by-exec\` in each repo planned above, and remove the set."* ]]
    # no nested config is read (here a broken one)
    printf '{broken' > "$WS/frontend/.meta.yaml"
    preview 0 git worktree exec eph --ephemeral --repo backend -- touch made-by-exec
    rm "$WS/frontend/.meta.yaml"
    # a repo without the start commit is skipped, as in a run
    git -C "$WS/frontend" tag only-in-frontend
    preview 0 git worktree exec eph only-in-frontend --ephemeral --all -- touch made-by-exec
    [[ "$output" == *"Skipping 'backend': ref 'only-in-frontend' not found"* ]]
    [[ "$output" == *"frontend:"* ]]
    [[ "$output" != *"backend:"* ]]
    # a ref no repo has fails as in a run
    preview 1 git worktree exec eph nosuchref --ephemeral --all -- touch made-by-exec
    [[ "$output" == *"No selected repo has ref 'nosuchref'; no worktree was created"* ]]
    preview 1 git worktree create new nosuchref --all
    [[ "$output" == *"No selected repo has ref 'nosuchref'"* ]]
    # the set's own .meta.yaml, not yet made, names its repos for a filter
    preview 1 git worktree exec eph --ephemeral --repo backend --include backend -- touch made-by-exec
    [[ "$output" == *"can't preview --include/--exclude of exec --ephemeral"* ]]
    preview 1 git worktree exec eph --ephemeral --repo backend --exclude backend -- touch made-by-exec
    [[ "$output" == *"can't preview --include/--exclude of exec --ephemeral"* ]]
}

@test "exec previews" {
    # (an uncloned repo fails a real run too)
    preview 0 --exclude missing exec -- touch made-by-exec
    [[ "$output" == *"[DRY RUN]"*"touch made-by-exec"* ]]
}

@test "exec --add-aliases-to-global-looprc previews" {
    preview 0 --add-aliases-to-global-looprc exec -- true
    [[ "$output" == *"Would add your shell's aliases"* ]]
}

@test "git clone previews" {
    preview 0 git clone "file://$SANDBOX/remotes/backend.git" cloned
    [[ "$output" == *"Would clone meta repository"* ]]
}

@test "git update previews" {
    preview 0 git update
    [[ "$output" == *"Would clone 1 missing repositories"* ]]
}

@test "a remote git command previews without opening ssh connections" {
    preview 0 --exclude missing --parallel git fetch
    [[ "$output" == *"[DRY RUN]"*"git fetch"* ]]
}

@test "git commit -m previews" {
    preview 0 git commit -m message
    [[ "$output" == *"[DRY RUN]"*"git commit -m message"* ]]
}

@test "git commit --edit previews without an editor" {
    RUNNER=with_terminal preview 0 git commit --edit
    [[ "$output" == *"Would open the editor '$STUBS/editor'"*"backend"* ]]
    # with no terminal it fails as a run does
    RUNNER=without_terminal preview 1 git commit --edit
    [[ "$output" == *"needs the terminal"* ]]
}

@test "context previews without writing its cache" {
    preview 0 context
}

@test "init claude previews" {
    preview 0 init claude
    [[ "$output" == *"Would create"*"settings.json"* ]]
    [[ "$output" == *"nothing was written"* ]]
}

@test "plugin install refuses --dry-run" {
    preview 1 plugin install --local someone/meta-thing
    [[ "$output" == *"meta plugin install has no preview"* ]]
}

@test "plugin uninstall refuses --dry-run" {
    # a local plugin, for uninstall to refuse to remove
    mkdir -p .meta/plugins
    cp "$TARGET/meta-git" .meta/plugins/meta-git
    preview 1 plugin uninstall --local meta-git
    [[ "$output" == *"meta plugin uninstall has no preview"* ]]
    [ -x .meta/plugins/meta-git ]
}

@test "plugin update refuses --dry-run" {
    preview 1 plugin update
    [[ "$output" == *"meta plugin update has no preview"* ]]
    # checking and listing make no plugins folder
    [ ! -e "$META_DATA_DIR/plugins" ]
    preview 0 plugin update --check
    preview 0 plugin list
}
