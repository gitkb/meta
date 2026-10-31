#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

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

    # Programs a preview must never start: each records that it ran
    STUBS="$SANDBOX/stubs"
    mkdir -p "$STUBS"
    for stub in ssh editor; do
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

# Run `meta --dry-run ARGS...`: it exits with $1 and changes nothing
preview() {
    local expected="$1"
    shift
    local before
    before="$(snapshot)"
    run meta --dry-run "$@"
    echo "$output"
    [ "$status" -eq "$expected" ]
    [ ! -e "$CALLS" ]
    [ "$(snapshot)" = "$before" ]
}

@test "worktree create previews" {
    preview 0 git worktree create new --all
    [[ "$output" == *"Would create worktree set 'new'"* ]]
}

@test "worktree add previews" {
    preview 0 git worktree add clean --repo frontend
    [[ "$output" == *"Would add to worktree set 'clean'"* ]]
    preview 0 --json git worktree add clean --repo frontend
    python3 -c 'import json, sys; out = json.load(sys.stdin); assert out["dry_run"] and out["repos"][0]["alias"] == "frontend", out' <<< "$output"
}

@test "worktree remove previews" {
    preview 0 git worktree remove clean
    [[ "$output" == *"Would remove worktree 'clean'"* ]]
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
    run meta --dry-run --json git worktree prune
    [[ "$output" == *'"dry_run": true'* ]]
}

@test "worktree exec previews" {
    preview 0 git worktree exec clean -- touch made-by-exec
    [[ "$output" == *"[DRY RUN]"*"touch made-by-exec"* ]]
}

@test "worktree exec --ephemeral creates, runs and removes nothing" {
    preview 0 git worktree exec eph --ephemeral --repo backend -- touch made-by-exec
    [[ "$output" == *"Would create worktree set 'eph'"* ]]
    [[ "$output" == *"Then would run \`touch made-by-exec\` in ., backend and remove the set."* ]]
    # its filters select as in a run, and a name that is none is an error
    preview 0 git worktree exec eph --ephemeral --repo backend --include backend -- touch made-by-exec
    [[ "$output" == *"Then would run \`touch made-by-exec\` in backend and remove the set."* ]]
    preview 1 git worktree exec eph --ephemeral --repo backend --include typo -- touch made-by-exec
    [[ "$output" == *"typo"* ]]
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
    preview 0 git commit --edit
    [[ "$output" == *"Would open the editor"*"backend"* ]]
}

@test "context previews without writing its cache" {
    preview 0 context
}

@test "init claude previews" {
    preview 0 init claude
    [[ "$output" == *"would write"*"settings.json"* ]] || [[ "$output" == *"Would create"*"settings.json"* ]]
    [[ "$output" == *"nothing was written"* ]]
}

@test "plugin install refuses --dry-run" {
    preview 1 plugin install --local someone/meta-thing
    [[ "$output" == *"meta plugin install has no preview"* ]]
}

@test "plugin uninstall refuses --dry-run" {
    preview 1 plugin uninstall --local meta-git
    [[ "$output" == *"meta plugin uninstall has no preview"* ]]
}

@test "plugin update refuses --dry-run" {
    preview 1 plugin update
    [[ "$output" == *"meta plugin update has no preview"* ]]
}
