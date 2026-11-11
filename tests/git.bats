#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"
load "${BATS_TEST_DIRNAME}/helpers/terminal.bash"

# Integration tests for `meta git` commands
# Tests use local git repos (no network IO required)

setup() {
    sandbox
    # JSON: tests add projects to it with sed
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
{
    "projects": {
        "frontend": "git@github.com:org/frontend.git",
        "backend": "git@github.com:org/backend.git",
        "shared": "git@github.com:org/shared.git"
    }
}
EOF
    make_workspace --root-repo --plugins git frontend backend shared
    cd "$TEST_DIR"
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

@test "meta git status keeps the user's own GIT_CONFIG_* settings" {
    echo "change" >> "$TEST_DIR/frontend/README.md"
    GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=status.short GIT_CONFIG_VALUE_0=true \
        run "$META_BIN" --include frontend git status
    [ "$status" -eq 0 ]
    [[ "$output" == *"README.md"* ]]
    [[ "$output" != *"Changes not staged"* ]]
    # with colour forced, git gets colour after the user's entry, not over it
    CLICOLOR_FORCE=1 GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=status.short GIT_CONFIG_VALUE_0=true \
        run "$META_BIN" --include frontend git status
    [[ "$output" == *$'\033['* ]]
    [[ "$output" != *"Changes not staged"* ]]
}

@test "meta git status colours a terminal only, never a pipe or JSON" {
    echo "change" >> "$TEST_DIR/frontend/README.md"
    esc=$'\033['
    red=$'\033[31mmodified'
    run "$META_BIN" --include frontend git status
    [ "$status" -eq 0 ]
    [[ "$output" == *"modified"* ]]
    [[ "$output" != *"$esc"* ]]

    run "$META_BIN" --json --include frontend git status
    [ "$status" -eq 0 ]
    [[ "$output" != *"$esc"* ]]
    [[ "$output" != *'\u001b'* ]]

    # forced colour reaches git; JSON and NO_COLOR win over it
    CLICOLOR_FORCE=1 run "$META_BIN" --include frontend git status
    [[ "$output" == *"$red"* ]]
    CLICOLOR_FORCE=1 run "$META_BIN" --json --include frontend git status
    [[ "$output" != *'\u001b'* ]]
    NO_COLOR=1 CLICOLOR_FORCE=1 run "$META_BIN" --include frontend git status
    [[ "$output" != *"$esc"* ]]

    # on a terminal it is coloured, in both modes, unless TERM=dumb
    for mode in --parallel --sequential; do
        TERM=xterm run with_terminal "$META_BIN" $mode --include frontend git status
        [[ "$output" == *"$red"* ]]
        TERM=dumb run with_terminal "$META_BIN" $mode --include frontend git status
        [[ "$output" == *"modified"* ]]
        [[ "$output" != *"$red"* ]]
    done

    # stdout on a terminal, stderr in a file: the file gets no colour
    TERM=xterm run with_terminal sh -c '"$1" nosuchcmd 2>"$2"' _ "$META_BIN" "$TEST_DIR/err"
    grep -q 'error: unrecognized command' "$TEST_DIR/err"
    ! grep -q "$esc" "$TEST_DIR/err"
}

@test "with --json, stdout is one JSON document whatever happens, and usage errors exit 2" {
    # stdout alone; stderr in $TEST_DIR/stderr
    json_run() { run bash -c 'err=$1; shift; "$@" 2>"$err"' _ "$TEST_DIR/stderr" "$@"; }
    json_has() { jq -e "$1" >/dev/null <<<"$output"; }
    json_run "$META_BIN" --json nosuchcmd
    [ "$status" -eq 2 ]
    json_has '.success == false and .error.code == "usage" and (.error.message | contains("unrecognized command"))'

    json_run "$META_BIN" --json --include nope exec -- true
    [ "$status" -eq 1 ]
    json_has '.success == false and (.error.message | contains("--include"))'

    json_run "$META_BIN" --json git commit --amend
    [ "$status" -eq 2 ]
    json_has '.success == false and .error.code == "usage" and (.error.message | contains("--amend"))'

    # meta's --verbose lines, and a command's own --json
    json_run "$META_BIN" --json --verbose --include frontend exec -- true
    json_has '.success == true'
    json_run "$META_BIN" --include nope --json context
    json_has '.success == false'

    # a plugin's text, wrapped
    json_run "$META_BIN" --json git commit -m x
    [ "$status" -eq 0 ]
    json_has '.success == true and (.message | contains("No staged changes"))'

    # a report, then a failure: the report stays the only document
    json_run "$META_BIN" --json project check
    [ "$status" -eq 1 ]
    json_has '.success == false and (.error | type) == "object" and ([.results[] | select(.error)] | length > 0)'
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

# Give the workspace and its repos bare remotes they track, under
# $TEST_DIR/remotes, with .meta.yaml naming them
with_remotes() {
    mkdir -p "$TEST_DIR/remotes"
    sed -i.bak "s|git@github.com:org/\([a-z]*\)\.git|file://$TEST_DIR/remotes/\1.git|" "$TEST_DIR/.meta.yaml"
    rm "$TEST_DIR/.meta.yaml.bak"
    git -C "$TEST_DIR" commit --quiet -am "remotes"
    local repo name
    for repo in frontend backend shared .; do
        name=${repo/#./ws}
        git clone --quiet --bare "$TEST_DIR/$repo" "$TEST_DIR/remotes/$name.git"
        git -C "$TEST_DIR/$repo" remote add origin "file://$TEST_DIR/remotes/$name.git"
        git -C "$TEST_DIR/$repo" fetch --quiet origin
        git -C "$TEST_DIR/$repo" branch --quiet -u "origin/$(git -C "$TEST_DIR/$repo" branch --show-current)"
    done
}

# teammate NAME [FILE CONTENT]: a teammate pushes one commit to NAME's remote
teammate() {
    local mate="$TEST_DIR/remotes/mate-$1"
    [ -d "$mate" ] || git clone --quiet "$TEST_DIR/remotes/$1.git" "$mate"
    git -C "$mate" pull --quiet --ff-only
    if [ -n "${2:-}" ]; then
        printf '%s\n' "$3" > "$mate/$2"
    else
        echo "mate $RANDOM" >> "$mate/mate.txt"
    fi
    git -C "$mate" add -A
    git -C "$mate" -c user.name=Mate -c user.email=mate@test.com commit --quiet -m "$1: teammate"
    git -C "$mate" push --quiet
}

head_of() { git -C "$1" rev-parse HEAD; }

# within SECONDS CMD...: CMD, stopped with TERM if it runs longer, then
# KILL 5 s later; exits as CMD did (macOS has no timeout)
within() {
    perl -e '
        my $limit = shift;
        defined(my $pid = fork) or die "fork: $!";
        if (!$pid) { exec @ARGV or die "exec: $!" }
        $SIG{ALRM} = sub { kill "TERM", $pid; sleep 5; kill "KILL", $pid };
        alarm $limit;
        waitpid $pid, 0;
        exit($? & 127 ? 128 + ($? & 127) : $? >> 8);
    ' "$@"
}

# state REPO: what a repo's checkout is: HEAD, branch, every ref (the
# remote-tracking ones show a fetch), FETCH_HEAD, and the working tree (in
# the workspace repo, what it tracks: the clones are untracked in it)
state() {
    printf '%s %s\n' "$(git -C "$1" rev-parse HEAD)" "$(git -C "$1" branch --show-current)"
    git -C "$1" for-each-ref
    cat "$(git -C "$1" rev-parse --git-path FETCH_HEAD)" 2>/dev/null || echo "no FETCH_HEAD"
    if [ "$1" = . ]; then
        git -C "$1" status --porcelain --untracked-files=no
    else
        git -C "$1" status --porcelain --untracked-files=all
    fi
}

@test "meta git update clones what is missing and leaves every cloned repo as it is" {
    with_remotes
    # every remote moves ahead, the workspace's adding a project
    teammate frontend
    teammate backend
    git clone --quiet --bare "$TEST_DIR/shared" "$TEST_DIR/remotes/extra.git"
    local added
    added=$(sed "s|\"shared\": \(.*\)|\"shared\": \1, \"extra\": \"file://$TEST_DIR/remotes/extra.git\"|" .meta.yaml)
    teammate ws .meta.yaml "$added"
    # work in progress, a detached HEAD, and a missing project
    echo mine >> backend/README.md
    echo new > backend/new.txt
    git -C . switch --quiet --detach
    rm -rf shared
    local before
    before=$(for repo in . frontend backend; do state "$repo"; done)
    run "$META_BIN" git update
    [ "$status" -eq 0 ]
    [ "$(for repo in . frontend backend; do state "$repo"; done)" = "$before" ]
    [ -d shared/.git ]
    [[ "$output" == *"Update completed (1 cloned)"* ]]
    # the .meta.yaml on disk is what is read: what the remote's adds isn't cloned
    [ ! -e extra ]
    # nor with -r, --prune or --prune-only, which only remove what no
    # .meta.yaml lists
    rm -rf shared
    for options in "-r git update" "git update --prune" "-r git update --prune-only"; do
        # shellcheck disable=SC2086
        run "$META_BIN" $options
        [ "$status" -eq 0 ]
        [ "$(for repo in . frontend backend; do state "$repo"; done)" = "$before" ]
    done
    [ -d shared/.git ]
}

@test "meta git update reads a nested .meta.yaml only with -r, and changes no nested repo" {
    with_remotes
    git clone --quiet --bare "$TEST_DIR/frontend" "$TEST_DIR/remotes/inner.git"
    printf '{"projects": {"inner": "file://%s/remotes/inner.git"}}\n' "$TEST_DIR" > shared/.meta.yaml
    git -C shared add .meta.yaml
    git -C shared commit --quiet -m "nested"
    git -C shared push --quiet
    run "$META_BIN" git update
    [ "$status" -eq 0 ]
    [ ! -e shared/inner ]
    run "$META_BIN" -r git update
    [ "$status" -eq 0 ]
    [ -d shared/inner/.git ]
    # the nested meta repo and its project move ahead on their remotes
    teammate shared
    teammate inner
    local before
    before=$(for repo in . shared shared/inner; do state "$repo"; done)
    run "$META_BIN" --recursive git update
    [ "$status" -eq 0 ]
    [ "$(for repo in . shared shared/inner; do state "$repo"; done)" = "$before" ]
    [[ "$output" == *"Update completed (0 cloned)"* ]]
    run "$META_BIN" -r git update --prune
    [ "$status" -eq 0 ]
    [ "$(for repo in . shared shared/inner; do state "$repo"; done)" = "$before" ]
}

@test "meta --dry-run git update lists the clones, no pulls, and changes nothing" {
    with_remotes
    teammate frontend
    rm -rf "$TEST_DIR/shared"
    local before
    before=$(state frontend)
    run "$META_BIN" --dry-run git update
    [ "$status" -eq 0 ]
    [[ "$output" == *"git clone -- file://$TEST_DIR/remotes/shared.git $TEST_DIR/shared"* ]]
    [[ "$output" != *"pull"* ]]
    [ "$(state frontend)" = "$before" ]
    [ ! -e "$TEST_DIR/shared" ]
}

@test "meta git update and clone check out a project's branch:, the others their remote's default" {
    with_remotes
    # shared's remote gets a release branch ahead of main
    git clone --quiet remotes/shared.git remotes/mate-shared
    git -C remotes/mate-shared switch --quiet -c release
    git -C remotes/mate-shared -c user.name=Mate -c user.email=mate@test.com \
        commit --quiet --allow-empty -m "release work"
    git -C remotes/mate-shared push --quiet origin release
    jq '.projects.shared = {"repo": .projects.shared, "branch": "release"}' .meta.yaml > meta.json
    mv meta.json .meta.yaml
    git commit --quiet -am "shared starts from release"
    git push --quiet
    rm -rf shared frontend
    run "$META_BIN" --dry-run git update
    [ "$status" -eq 0 ]
    [[ "$output" == *"git clone --branch release -- file://$TEST_DIR/remotes/shared.git $TEST_DIR/shared"* ]]
    [[ "$output" == *"git clone -- file://$TEST_DIR/remotes/frontend.git $TEST_DIR/frontend"* ]]
    run "$META_BIN" git update
    [ "$status" -eq 0 ]
    [ "$(git -C shared branch --show-current)" = release ]
    [ "$(git -C shared rev-parse HEAD)" = "$(git -C remotes/shared.git rev-parse release)" ]
    [ "$(git -C frontend branch --show-current)" = main ]
    # a clone of the workspace does the same
    run "$META_BIN" git clone "file://$TEST_DIR/remotes/ws.git" "$TEST_DIR/fresh"
    [ "$status" -eq 0 ]
    [ "$(git -C "$TEST_DIR/fresh/shared" branch --show-current)" = release ]
    [ "$(git -C "$TEST_DIR/fresh/frontend" branch --show-current)" = main ]
    # nothing moves a cloned repo to it
    git -C shared switch --quiet main
    run "$META_BIN" git update
    [ "$(git -C shared branch --show-current)" = main ]
    # a branch the remote doesn't have fails that clone, naming project and
    # branch with git's error; the other clones still happen
    jq '.projects.frontend = {"repo": .projects.frontend, "branch": "nope"}' .meta.yaml > meta.json
    mv meta.json .meta.yaml
    rm -rf frontend backend
    run "$META_BIN" git update
    [ "$status" -eq 1 ]
    [[ "$output" == *"Failed to clone frontend (file://$TEST_DIR/remotes/frontend.git, branch nope)"* ]]
    [[ "$output" == *"nope"*"not found"* ]]
    [[ "$output" == *"1 of 2 repositories failed to clone: frontend (branch nope):"*"not found"* ]]
    [ ! -e frontend ]
    [ -d backend/.git ]
}

@test "a branch: that names no branch fails only the commands acting on that project" {
    with_remotes
    # shared becomes a meta repo whose .meta.yaml gives inner a bad branch:
    printf '{"projects": {"inner": {"repo": "file://%s/remotes/frontend.git", "branch": "HEAD"}, "ok": "file://%s/remotes/frontend.git"}}\n' \
        "$TEST_DIR" "$TEST_DIR" > shared/.meta.yaml
    printf 'inner/\nok/\n' > shared/.gitignore
    local said="Invalid branch of project 'inner' in $TEST_DIR/shared/.meta.yaml: branch 'HEAD' is not a name git takes for a branch"
    # listing and other commands go on, and say so
    run "$META_BIN" -r project list
    [ "$status" -eq 0 ]
    [[ "$output" == *"inner"* && "$output" == *"warning: $said"* ]]
    bats_require_minimum_version 1.5.0
    run --separate-stderr "$META_BIN" -r --json project list
    [ "$status" -eq 0 ]
    # its row says why; the listing is complete, so no Envelope warning
    # (atc reads one as missing members)
    jq -e --arg said "$said" '.success and .warnings == [] and ([.results[].data | select(.name == "inner") | .branch_problem] == [$said])' <<<"$output"
    jq -e '[.results[].data | select(.name == "inner") | has("branch")] == [false]' <<<"$output"
    [ -z "$stderr" ]
    run "$META_BIN" -r git status
    [ "$status" -eq 0 ]
    # update: that clone alone fails, with why; the other is cloned
    run "$META_BIN" -r git update
    [ "$status" -eq 1 ]
    [[ "$output" == *"1 of 2 repositories failed to clone: inner: $said"* ]]
    [ ! -e shared/inner ]
    [ -d shared/ok/.git ]
    # a worktree set: that project alone is refused, with why
    git clone --quiet remotes/frontend.git shared/inner
    run "$META_BIN" -r git worktree create t --repo shared/inner --no-deps
    [ "$status" -ne 0 ]
    [[ "$output" == *"$said"* ]]
    [ ! -e .worktrees/t ]
    # unless the set's branch never starts there: an explicit base, or a
    # branch the checkout has, in the preview and the run, create and add
    run "$META_BIN" -r --dry-run git worktree create based main --repo shared/inner --no-deps
    [ "$status" -eq 0 ]
    run "$META_BIN" -r git worktree create based main --repo shared/inner --no-deps
    [ "$status" -eq 0 ]
    git -C shared/inner branch mine
    run "$META_BIN" -r git worktree create mine --repo shared/inner --no-deps
    [ "$status" -eq 0 ]
    "$META_BIN" -r git worktree create other --repo shared/ok --no-deps >/dev/null
    run "$META_BIN" -r git worktree add other main --repo shared/inner
    [ "$status" -eq 0 ]
    run "$META_BIN" -r git worktree create t --repo shared/ok --no-deps
    [ "$status" -eq 0 ]
    # in the root .meta.yaml too: the commands that don't clone it still run
    jq '.projects.backend = {"repo": .projects.backend, "branch": "a..b"}' .meta.yaml > meta.json
    mv meta.json .meta.yaml
    run "$META_BIN" git status
    [ "$status" -eq 0 ]
    run "$META_BIN" exec -- true
    [ "$status" -eq 0 ]
}

@test "a project's branch: adds nothing to a plugin's request, so a plugin on an older SDK still reads it" {
    with_remotes
    jq '.projects.shared = {"repo": .projects.shared, "branch": "release"}' .meta.yaml > meta.json
    mv meta.json .meta.yaml
    # meta-git as installed, logging each request it gets
    local real="$TEST_DIR/meta-git-real"
    mv "$TEST_DIR/.meta/plugins/meta-git" "$real"
    printf '#!/bin/sh\n[ "$1" = --meta-plugin-exec ] && { cat "$META_REQUEST"; echo; } >> "%s/requests"\nexec "%s" "$@"\n' \
        "$TEST_DIR" "$real" > "$TEST_DIR/.meta/plugins/meta-git"
    chmod +x "$TEST_DIR/.meta/plugins/meta-git"
    run "$META_BIN" git status
    [ "$status" -eq 0 ]
    run "$META_BIN" git worktree create t main --repo shared --no-deps
    [ "$status" -eq 0 ]
    # each project carries only the fields an older SDK's request has: its
    # members refuse a field they don't know
    jq -se '[.[] | (.workspace.members // [])[], (.projects // [])[] | keys[]] | unique
        | . - ["name", "path", "abs", "repo", "tags", "depends_on", "dependency_problems",
               "left_out", "meta", "level", "present", "nested"] == []' "$TEST_DIR/requests"
}

@test "meta git update doesn't stall on git's stdout, and git asks for no credentials" {
    with_remotes
    rm -rf "$TEST_DIR/shared"
    # a git that writes more to stdout than a pipe holds, and records whether
    # it may prompt
    local real_git
    real_git="$(git --exec-path)/git"
    mkdir "$TEST_DIR/bin"
    printf '#!/bin/sh\ncase " $* " in *" clone "*)\n  head -c 262144 /dev/zero\n  echo "$GIT_TERMINAL_PROMPT" >> "%s/prompts";;\nesac\nexec "%s" "$@"\n' \
        "$TEST_DIR" "$real_git" > "$TEST_DIR/bin/git"
    chmod +x "$TEST_DIR/bin/git"
    PATH="$TEST_DIR/bin:$PATH" run within 60 "$META_BIN" git update
    [ "$status" -eq 0 ]
    [ -d "$TEST_DIR/shared/.git" ]
    [[ "$output" == *"Update completed (1 cloned)"* ]]
    [ "$(grep -c '^0$' "$TEST_DIR/prompts")" -eq 1 ]
}

@test "meta git update opens no ssh to remotes it won't contact" {
    # cloned repos listed with ssh URLs: nothing to clone
    printf '#!/bin/sh\ntouch "%s/ssh-ran"\nexit 255\n' "$TEST_DIR" > "$TEST_DIR/ssh"
    chmod +x "$TEST_DIR/ssh"
    PATH="$TEST_DIR:$PATH" run "$META_BIN" git update
    [ "$status" -eq 0 ]
    [ ! -e "$TEST_DIR/ssh-ran" ]
    [[ "$output" != *"SSH"* ]]
}

@test "meta git update in a worktree set refuses instead of updating the workspace" {
    with_remotes
    "$META_BIN" git worktree create review --repo frontend --no-deps >/dev/null
    cd "$TEST_DIR/.worktrees/review/frontend"
    run "$META_BIN" git update
    [ "$status" -ne 0 ]
    [[ "$output" == *"not worktree set 'review'"* ]]
}

@test "meta git update in a plain git worktree of the workspace updates that worktree" {
    with_remotes
    git worktree add --quiet "$TEST_DIR/.worktrees/x"
    cd "$TEST_DIR/.worktrees/x"
    # no .meta/plugins here: this build's plugin, not one installed
    PATH="$(dirname "$META_GIT_BIN"):$PATH" run "$META_BIN" git update
    [ "$status" -eq 0 ]
    [[ "$output" == *"Update completed (3 cloned)"* ]]
    [ -d "$TEST_DIR/.worktrees/x/frontend/.git" ]
}

@test "meta git update -r takes a second checkout of the workspace repo for no cycle" {
    with_remotes
    git clone --quiet "$TEST_DIR/remotes/ws.git" "$TEST_DIR/pages"
    git -C "$TEST_DIR/pages" rm --quiet .meta.yaml
    git -C "$TEST_DIR/pages" -c user.name=Mate -c user.email=mate@test.com commit --quiet -m "pages: no .meta.yaml"
    git -C "$TEST_DIR/pages" push --quiet origin HEAD:pages
    rm -rf "$TEST_DIR/pages"
    git clone --quiet --branch pages "$TEST_DIR/remotes/ws.git" "$TEST_DIR/pages"
    local added
    added=$(sed "s|\"shared\": \(.*\)|\"shared\": \1, \"pages\": \"file://$TEST_DIR/remotes/ws.git\"|" .meta.yaml)
    printf '%s\n' "$added" > .meta.yaml
    git commit --quiet -am pages
    git push --quiet
    run "$META_BIN" -r git update
    [ "$status" -eq 0 ]
    [[ "$output" != *"list each other"* ]]
    [[ "$output" == *"Update completed (0 cloned)"* ]]
}

@test "meta git clone -r and update -r of meta repos that list each other stop, naming the cycle" {
    TOP_BARE="$(mktemp -d)/top.git"
    KID_BARE="$(mktemp -d)/kid.git"
    create_meta_bare_repo "$KID_BARE" "{\"projects\": {\"top\": {\"repo\": \"file://$TOP_BARE\", \"meta\": true}}}"
    create_meta_bare_repo "$TOP_BARE" "{\"projects\": {\"kid\": {\"repo\": \"file://$KID_BARE\", \"meta\": true}}}"
    CYC="$(mktemp -d)/cyc"
    run within 60 "$META_BIN" -r git clone "file://$TOP_BARE" "$CYC"
    [ "$status" -eq 1 ]
    [[ "$output" == *"meta repos list each other: cyc → kid → top"* ]]
    [ -d "$CYC/kid/.git" ]
    [ ! -e "$CYC/kid/top" ]
    # the clone has no .meta/plugins: this build's plugin, not one installed
    cd "$CYC"
    PATH="$(dirname "$META_GIT_BIN"):$PATH" run within 60 "$META_BIN" -r git update
    [ "$status" -eq 1 ]
    [[ "$output" == *"meta repos list each other: cyc → kid → top"* ]]
    [ ! -e "$CYC/kid/top" ]
    # cloned by a relative URL, the top repo is the one git recorded
    rm -rf "$CYC"
    cd "$(dirname "$TOP_BARE")"
    PATH="$(dirname "$META_GIT_BIN"):$PATH" run within 60 "$META_BIN" -r git clone top.git "$CYC"
    [ "$status" -eq 1 ]
    [[ "$output" == *"meta repos list each other: cyc → kid → top"* ]]
    [ ! -e "$CYC/kid/top" ]
    rm -rf "$TOP_BARE" "$KID_BARE" "$CYC"
}


@test "meta git update detects missing repos" {
    with_remotes
    rm -rf "$TEST_DIR/shared" "$TEST_DIR/frontend"
    # a remote that doesn't exist: the clone fails without the network
    sed -i.bak "s|file://$TEST_DIR/remotes/shared.git|file://$TEST_DIR/nowhere.git|" "$TEST_DIR/.meta.yaml"
    run "$META_BIN" git update
    # it tries to clone shared from there, and says it failed
    [ "$status" -ne 0 ]
    [[ "$output" == *"shared"*"nowhere.git"* || "$output" == *"nowhere.git"*"shared"* ]]
    [[ "$output" == *"1 of 2 repositories failed to clone: shared"* ]]
    [ ! -e "$TEST_DIR/shared" ]
    # the other clone still happens
    [ -d "$TEST_DIR/frontend/.git" ]
}

@test "meta git update says what an unlisted repository holds, and never how to delete it" {
    with_remotes
    # stray holds an empty .git; frontend is renamed web in .meta.yaml, and
    # holds an unpushed commit, an untracked file and a worktree
    mkdir -p "$TEST_DIR/stray/.git"
    git -C frontend switch --quiet -c topic
    echo wip > frontend/wip.md
    git -C frontend add wip.md
    git -C frontend commit --quiet -m "unpushed work"
    echo scratch > frontend/scratch.txt
    git -C frontend worktree add --quiet "$TEST_DIR/remotes/fe-review"
    git clone --quiet --bare remotes/frontend.git remotes/web.git
    sed -i.bak 's|"frontend": "[^"]*"|"web": "file://'"$TEST_DIR"'/remotes/web.git"|' .meta.yaml
    rm .meta.yaml.bak
    git commit --quiet -am rename
    git push --quiet
    run "$META_BIN" git update
    [ "$status" -eq 0 ]
    [[ "$output" == *"frontend isn't in .meta.yaml; it holds "*"untracked: scratch.txt"*"1 commit on no remote: "*"unpushed work"*"1 linked worktree: $TEST_DIR/remotes/fe-review"* ]]
    [[ "$output" != *"rm -rf"* ]]
    [[ "$output" != *"stray"* ]]
    [ -d "$TEST_DIR/web/.git" ]
}

@test "meta git update calls a leftover that holds nothing one update --prune removes" {
    with_remotes
    git clone --quiet remotes/shared.git old
    run "$META_BIN" git update
    [ "$status" -eq 0 ]
    [[ "$output" == *"old isn't in .meta.yaml (nothing unpushed; meta git update --prune removes it)"* ]]
    [ -d "$TEST_DIR/old/.git" ]
}

# ============ meta git update: selection, --prune ============

@test "meta git update clones only the selected projects" {
    with_remotes
    rm -rf frontend shared
    run "$META_BIN" --include shared git update
    [ "$status" -eq 0 ]
    [ -d shared/.git ]
    [ ! -e frontend ]
    # with meta's -r, the selection reaches the nested projects too
    run "$META_BIN" --include frontend -r git update
    [ "$status" -eq 0 ]
    [ -d frontend/.git ]
}

@test "meta git update --prune keeps each leftover that holds work, and removes a clean one" {
    with_remotes
    local name
    for name in clean untracked modified stashed unpushed worktree nested; do
        git clone --quiet remotes/shared.git "$name"
    done
    echo new > untracked/new.txt
    echo more >> modified/README.md
    echo stash >> stashed/README.md
    git -C stashed stash --quiet
    echo wip > unpushed/wip.md
    git -C unpushed add wip.md
    git -C unpushed commit --quiet -m "unpushed work"
    git -C worktree worktree add --quiet "$TEST_DIR/remotes/wt"
    # a repo the leftover ignores, with work of its own, and a bare one
    printf '/inner/\n/bare.git/\n' > nested/.git/info/exclude
    git init --quiet nested/inner
    git clone --quiet remotes/shared.git bare
    printf '/bare.git/\n' > bare/.git/info/exclude
    git init --quiet --bare bare/bare.git
    # never a folder that is no repo, nor a link to a repo
    mkdir plain
    git clone --quiet remotes/shared.git remotes/elsewhere
    ln -s "$TEST_DIR/remotes/elsewhere" link
    run "$META_BIN" --dry-run git update --prune
    [ "$status" -eq 0 ]
    [[ "$output" == *"Would remove clean (nothing unpushed)"* ]]
    [ -d clean/.git ]
    run "$META_BIN" git update --prune
    [ "$status" -eq 0 ]
    [[ "$output" == *"Removed clean (nothing unpushed)"* ]]
    [[ "$output" == *"Kept untracked: it holds 1 file untracked: new.txt"* ]]
    [[ "$output" == *"Kept modified: it holds 1 file with uncommitted changes: README.md"* ]]
    [[ "$output" == *"Kept stashed: it holds 1 stash"* ]]
    [[ "$output" == *"Kept unpushed: it holds 1 commit on no remote: "*"unpushed work"* ]]
    [[ "$output" == *"Kept worktree: it holds 1 linked worktree: $TEST_DIR/remotes/wt"* ]]
    [[ "$output" == *"Kept nested: it holds another git repository: inner"* ]]
    [[ "$output" == *"Kept bare: it holds another git repository: bare.git"* ]]
    [[ "$output" == *"Kept link: it is a symbolic link"* ]]
    [[ "$output" == *"Update completed (0 cloned, 1 removed, 8 kept)"* ]]
    [ ! -e clean ]
    for name in untracked modified stashed unpushed worktree nested bare plain link remotes/elsewhere frontend backend shared; do
        [ -e "$name" ]
    done
}

@test "meta git update --prune with --config keeps the repos .meta.yaml lists" {
    with_remotes
    printf '.env\n' > frontend/.git/info/exclude
    echo SECRET=1 > frontend/.env
    printf 'projects:\n  shared: file://%s/remotes/shared.git\n' "$TEST_DIR" > other.yaml
    git add other.yaml && git commit --quiet -m other
    run "$META_BIN" --config other.yaml --dry-run git update --prune
    [ "$status" -eq 0 ]
    [[ "$output" != *"remove frontend"* ]]
    run "$META_BIN" --config other.yaml git update --prune
    [ "$status" -eq 0 ]
    run "$META_BIN" --config other.yaml git update --prune-only
    [ "$status" -eq 0 ]
    [ -f frontend/.env ]
    [ -d backend/.git ]
    # a leftover neither lists is still one
    git clone --quiet remotes/shared.git leftover
    run "$META_BIN" --config other.yaml git update --prune
    [[ "$output" == *"Removed leftover"* ]]
    [ ! -e leftover ]
}

@test "meta git update --prune removes a leftover holding only ignored files, naming them" {
    with_remotes
    git clone --quiet remotes/shared.git local-data
    printf '.env\ndata/\n' > local-data/.git/info/exclude
    echo SECRET=1 > local-data/.env
    mkdir local-data/data && echo row > local-data/data/a.db
    # a commit only the reflog holds is not kept: git counts it discarded
    git -C local-data switch --quiet --detach
    git -C local-data commit --quiet --allow-empty -m "left on a detached HEAD"
    git -C local-data switch --quiet main
    run "$META_BIN" git update
    [[ "$output" == *"local-data isn't in .meta.yaml (nothing unpushed; meta git update --prune removes it, and its ignored files: .env, data/)"* ]]
    run "$META_BIN" --dry-run git update --prune
    [ "$status" -eq 0 ]
    [[ "$output" == *"Would remove local-data (nothing unpushed; ignored files: .env, data/ deleted with it)"* ]]
    # --json names them too, in the preview and the run
    local json_check='
import json, sys
[r] = [r["data"] for r in json.load(sys.stdin)["results"] if r["project"] == "local-data"]
assert r["action"] == "removed" and r["reason"] == "nothing unpushed; ignored files: .env, data/ deleted with it", r
'
    run "$META_BIN" --json --dry-run git update --prune
    [ "$status" -eq 0 ]
    python3 -c "$json_check" <<<"$output"
    [ -f local-data/.env ]
    run "$META_BIN" --json git update --prune
    [ "$status" -eq 0 ]
    python3 -c "$json_check" <<<"$output"
    [ ! -e local-data ]
    git clone --quiet remotes/shared.git local-data
    echo SECRET=1 > local-data/.env
    printf '.env\ndata/\n' > local-data/.git/info/exclude
    run "$META_BIN" git update --prune
    [ "$status" -eq 0 ]
    [[ "$output" == *"Removed local-data (nothing unpushed; ignored files: .env deleted with it)"* ]]
    [ ! -e local-data ]
    run "$META_BIN" git update --prune --delete-ignored
    [ "$status" -ne 0 ]
}

@test "meta git update --prune clones what is missing and prunes what no .meta.yaml lists" {
    with_remotes
    rm -rf shared
    git clone --quiet remotes/frontend.git old
    run "$META_BIN" --dry-run git update
    [ "$status" -eq 0 ]
    [[ "$output" == *"git clone"* ]]
    run "$META_BIN" --json git update --prune
    [ "$status" -eq 0 ]
    jq -e '[.results[] | select(.project == "shared") | .data.action] == ["cloned"]' <<<"$output"
    jq -e '[.results[] | select(.project == "old") | .data.action] == ["removed"]' <<<"$output"
    [ -d shared/.git ]
    [ ! -e old ]
    # with everything cloned, the preview says so, not an empty list
    run "$META_BIN" --dry-run git update --prune
    [ "$status" -eq 0 ]
    [[ "$output" == *"[DRY RUN] Nothing to clone: every project the .meta.yaml files list is cloned"* ]]
    [[ "$output" != *"Would run"* ]]
}

@test "meta git update --prune-only removes what no .meta.yaml lists and clones nothing" {
    with_remotes
    rm -rf shared
    git clone --quiet remotes/frontend.git clean
    git clone --quiet remotes/frontend.git unpushed
    echo wip > unpushed/wip.md
    git -C unpushed add wip.md
    git -C unpushed commit --quiet -m "unpushed work"
    # a tag no remote has, on a pushed commit: its message is only here
    git clone --quiet remotes/frontend.git tagged
    git -C tagged tag -a v9 -m "release notes only here"
    # and one whose remote is over ssh: an ssh that shows it was run
    git clone --quiet remotes/frontend.git far
    git -C far tag v1
    git -C far remote set-url origin git@nohost.invalid:org/far.git
    mkdir -p "$TEST_DIR/bin"
    printf '#!/bin/sh\ntouch "%s/ssh-ran"\nexit 255\n' "$TEST_DIR" > "$TEST_DIR/bin/ssh"
    chmod +x "$TEST_DIR/bin/ssh"
    # update's warnings contact no remote: tags are named as unasked
    PATH="$TEST_DIR/bin:$PATH" run "$META_BIN" git update
    [ "$status" -eq 0 ]
    [[ "$output" == *"far isn't in .meta.yaml; it holds 1 tag no remote was asked about"* ]]
    [ ! -e "$TEST_DIR/ssh-ran" ]
    rm -rf shared
    PATH="$TEST_DIR/bin:$PATH" run "$META_BIN" --dry-run git update --prune-only
    [ "$status" -eq 0 ]
    [[ "$output" == *"Would remove clean (nothing unpushed)"* ]]
    [[ "$output" == *"Would keep unpushed: it holds 1 commit on no remote: "*"unpushed work"* ]]
    [[ "$output" != *"Would run"* && "$output" != *"Nothing to clone"* ]]
    [ -d clean/.git ]
    PATH="$TEST_DIR/bin:$PATH" run "$META_BIN" git update --prune-only
    [ "$status" -eq 0 ]
    [[ "$output" == *"Removed clean (nothing unpushed)"* ]]
    [[ "$output" == *"Kept unpushed: it holds 1 commit on no remote: "*"unpushed work"* ]]
    [[ "$output" == *"Kept tagged: it holds 1 tag on no remote: v9"* ]]
    # pruning asks: a remote that can't be asked keeps the repo
    [[ "$output" == *"Kept far: what it holds can't be told (remote origin can't be asked which tags it has: "* ]]
    [[ "$output" == *"Update completed (1 removed, 3 kept)"* ]]
    [ -d far/.git ]
    [ -d tagged/.git ]
    [ ! -e clean ]
    [ -d unpushed/.git ]
    [ ! -e shared ]
    # one or the other
    run "$META_BIN" git update --prune --prune-only
    [ "$status" -eq 2 ]
    [[ "$output" == *"--prune-only"*"--prune"* ]]
    [ -d unpushed/.git ]
    # a listed folder that holds something else is no failure: nothing is cloned
    mkdir shared && echo mine > shared/notes.txt
    run "$META_BIN" --json git update --prune-only
    [ "$status" -eq 0 ]
    jq -e '[.results[] | select(.data.action == "failed")] == []' <<<"$output"
}

@test "meta --json git update names a failed clone and a refused folder by their paths" {
    rm -rf shared
    mkdir -p frontend-notes && echo mine > frontend-notes/notes.txt
    echo '{"projects": {"api": {"repo": "file://'"$TEST_DIR"'/nowhere.git", "path": "services/api"}, "notes": {"repo": "u", "path": "frontend-notes"}}}' > .meta.yaml
    bats_require_minimum_version 1.5.0
    run --separate-stderr "$META_BIN" --json git update
    [ "$status" -eq 1 ]
    jq -e '.results[] | select(.project == "services/api") | .data.action == "failed" and (.error | type) == "object"' <<<"$output"
    jq -e '.results[] | select(.project == "frontend-notes") | .data == {"action": "failed", "reason": "not a git repository (folder exists, no .git)"} and .error.message == .data.reason' <<<"$output"
}

@test "meta git update --prune in a worktree set refuses instead of cloning into the workspace" {
    with_remotes
    "$META_BIN" git worktree create review --repo frontend --no-deps >/dev/null
    rm -rf "$TEST_DIR/shared"
    cd "$TEST_DIR/.worktrees/review/frontend"
    run "$META_BIN" git update --prune
    [ "$status" -ne 0 ]
    [[ "$output" == *"not worktree set 'review'"* ]]
    [ ! -e "$TEST_DIR/shared" ]
}

# ============ looped commands ============

@test "an option of meta's typed after the command gets a tip; a real git error doesn't" {
    run "$META_BIN" git status --recursive
    [ "$status" -eq 1 ]
    [[ "$output" == *"tip: --recursive is an option of meta itself: it goes before the command, as in meta --recursive git status"* ]]
    run "$META_BIN" git status --tag=v1
    [ "$status" -eq 1 ]
    [[ "$output" == *"tip: --tag is an option of meta itself: it goes before the command, as in meta '--tag=v1' git status"* ]]
    run "$META_BIN" git log --json
    [ "$status" -ne 0 ]
    [[ "$output" == *"tip: --json is an option of meta itself: it goes before the command, as in meta --json git log"* ]]
    # with another of meta's left after it, no line: git names only one
    run "$META_BIN" git status --recursive --json
    [[ "$output" == *"tip: --recursive is an option of meta itself: it goes before the command"* ]]
    [[ "$output" != *"as in meta"* ]]
    # typed before too: said once
    run sh -c '"$1" --json git status --json 2>/dev/null' _ "$META_BIN"
    [[ "$(jq -r .error.hint <<<"$output")" == *"as in meta --json git status" ]]
    # -s is meta's and git's: git rejected the other one
    run "$META_BIN" git status -s --no-such-option
    [ "$status" -eq 1 ]
    [[ "$output" != *"tip:"* ]]
    # rejected, but no option of meta's; and an error that is no option's
    run "$META_BIN" git status --no-such-option
    [ "$status" -eq 1 ]
    [[ "$output" != *"tip:"* ]]
    run "$META_BIN" git checkout no-such-branch
    [ "$status" -eq 1 ]
    [[ "$output" != *"tip:"* ]]
}

@test "a project that isn't cloned is a warning in looped git commands, and a failure in exec" {
    with_remotes
    teammate frontend
    rm -rf "$TEST_DIR/shared"
    run "$META_BIN" git pull
    [ "$status" -eq 0 ]
    [ "$(grep -c 'shared is not cloned' <<< "$output")" -eq 1 ]
    [[ "$output" == *"warning: shared is not cloned; \`meta git update\` clones it"* ]]
    [[ "$output" == *"3 commands complete"* ]]
    [ "$(head_of frontend)" = "$(head_of remotes/frontend.git)" ]
    run "$META_BIN" --strict git status
    [ "$status" -ne 0 ]
    # with nothing left to run, the warning still says why, and --json is
    # still one document
    run "$META_BIN" --include shared git status
    [ "$status" -eq 0 ]
    [[ "$output" == *"warning: shared is not cloned"* ]]
    run sh -c '"$1" --json --include shared git status 2>/dev/null' _ "$META_BIN"
    [[ "$(jq -r '.warnings[0].message' <<<"$output")" == "shared is not cloned"* ]]
    [ "$(jq '.results | length' <<<"$output")" -eq 0 ]
    run "$META_BIN" exec -- git status
    [ "$status" -eq 1 ]
    [[ "$output" == *"not cloned (run 'meta git update')"* ]]
}

@test "meta git update compares no origin for a project that is not cloned" {
    # the workspace's own origin is not the uncloned project's
    git -C "$TEST_DIR" remote add origin git@github.com:org/workspace.git
    rm -rf "$TEST_DIR/shared"
    mkdir "$TEST_DIR/shared"
    echo notes > "$TEST_DIR/shared/notes.txt"
    run "$META_BIN" git update
    # the folder is named, never cloned over
    [ "$status" -ne 0 ]
    [[ "$output" == *"shared: not a git repository (folder exists, no .git)"* ]]
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
    # named as no repository git can use, never cloned over
    [ "$status" -ne 0 ]
    [[ "$output" == *"backend: not a git repository"* ]]
    [[ "$output" != *"remote set-url"* ]]
    [ "$(git -C "$TEST_DIR" remote get-url origin)" = "git@github.com:org/workspace.git" ]
}

# ============ meta git clone (local) ============

@test "meta git clone numbers each repo once, after the header that announces the work" {
    remotes="$(cd "$(mktemp -d)" && pwd -P)"
    seed="$remotes/seed"
    mkdir -p "$seed"
    git -C "$seed" init --quiet --initial-branch=main
    printf 'projects:\n' > "$seed/.meta.yaml"
    for i in 1 2 3 4 5 6 7 8; do
        git init --quiet --bare --initial-branch=main "$remotes/a$i.git"
        git -C "$remotes" clone --quiet "$remotes/a$i.git" "w$i" 2>/dev/null
        git -C "$remotes/w$i" -c user.name=t -c user.email=t@t commit --quiet --allow-empty -m init
        git -C "$remotes/w$i" push --quiet origin main
        printf '  a%s: %s\n' "$i" "$remotes/a$i.git" >> "$seed/.meta.yaml"
    done
    git -C "$seed" add .meta.yaml
    git -C "$seed" -c user.name=t -c user.email=t@t commit --quiet -m ws
    git clone --quiet --bare "$seed" "$remotes/ws.git"
    # from the workspace, whose .meta/plugins holds meta-git
    run with_terminal "$META_BIN" git clone "$remotes/ws.git" "$remotes/ws"
    [ "$status" -eq 0 ]
    screen=$(printf '%s' "$output" | sed 's/\x1b\[[0-9;?]*[A-Za-z]//g' | tr '\r' '\n')
    # the number each repo finished with: 1..8, once each
    numbers=$(grep -ao '\[[0-9]/8\] *Cloned a[0-9]' <<<"$screen" | sort -u | grep -o '^\[[0-9]' | sort | tr -d '[' | tr -d '\n')
    [ "$numbers" = "12345678" ]
    # the header comes before the first repo's progress
    header=$(grep -an 'Cloning meta repository' <<<"$screen" | head -1 | cut -d: -f1)
    first=$(grep -an 'Cloned a' <<<"$screen" | head -1 | cut -d: -f1)
    [ -n "$header" ] && [ "$header" -lt "$first" ]
    rm -rf "$remotes"
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
    # Set default branch name explicitly
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
    # one planned command per repo, and git never ran
    [ "$(grep -c 'git status' <<<"$output")" -ge 4 ]
    [[ "$output" != *"On branch"* ]]
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
    # the repo reports; the folder is named, and git never runs in it
    [ "$status" -ne 0 ]
    [[ "$output" == *"frontend"* ]]
    [[ "$output" == *"not-a-repo"*"not a git repository (folder exists, no .git)"* ]]
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
    echo "# sub-lib" > "$TEST_DIR/frontend/sub-lib/README.md"
    git -C "$TEST_DIR/frontend/sub-lib" add README.md
    git -C "$TEST_DIR/frontend/sub-lib" commit --quiet -m "Init sub-lib"

    mkdir -p "$TEST_DIR/frontend/sub-lib/deep"
    git -C "$TEST_DIR/frontend/sub-lib/deep" init --quiet
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
    git -C "$TEST_DIR/backend/sub-service" init --quiet
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
    run "$META_BIN" --recursive git clone "file://$ROOT_BARE" "$CLONE_TARGET"
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
    run "$META_BIN" --recursive git clone "file://$ROOT_BARE" "$CLONE_TARGET"
    [ "$status" -eq 0 ]

    [ -d "$CLONE_TARGET/l1/.git" ]
    [ -d "$CLONE_TARGET/l1/l2/.git" ]
    [ -d "$CLONE_TARGET/l1/l2/l3/.git" ]

    rm -rf "$L3_BARE" "$L2_BARE" "$L1_BARE" "$ROOT_BARE" "$CLONE_TARGET"
}

@test "meta -r --depth limits how deep clone goes into nested meta repos" {
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
    # --depth 1 means: level 0 (root's children) + level 1 (l1's children)
    # l2's children (l3) should NOT be cloned
    run "$META_BIN" --recursive --depth 1 git clone "file://$ROOT_BARE" "$CLONE_TARGET"
    [ "$status" -eq 0 ]

    [ -d "$CLONE_TARGET/l1/.git" ]
    [ -d "$CLONE_TARGET/l1/l2/.git" ]
    # l3 should NOT be cloned (depth limit)
    [ ! -d "$CLONE_TARGET/l1/l2/l3" ]

    rm -rf "$L3_BARE" "$L2_BARE" "$L1_BARE" "$ROOT_BARE" "$CLONE_TARGET"
}

@test "clone routes nesting and history depth apart" {
    # root -> l1 (meta, two commits) -> l2 (meta) -> l3
    L3_BARE="$(mktemp -d)/l3.git"
    create_meta_bare_repo "$L3_BARE" '{"projects": {}}'
    L2_BARE="$(mktemp -d)/l2.git"
    create_meta_bare_repo "$L2_BARE" "{\"projects\": {\"l3\": \"file://$L3_BARE\"}}"
    L1_BARE="$(mktemp -d)/l1.git"
    create_meta_bare_repo "$L1_BARE" "{\"projects\": {\"l2\": {\"repo\": \"file://$L2_BARE\", \"meta\": true}}}"
    local work
    work="$(mktemp -d)"
    git clone --quiet "$L1_BARE" "$work/l1"
    git -C "$work/l1" commit --quiet --allow-empty -m second
    git -C "$work/l1" push --quiet origin main
    ROOT_BARE="$(mktemp -d)/root.git"
    create_meta_bare_repo "$ROOT_BARE" "{\"projects\": {\"l1\": {\"repo\": \"file://$L1_BARE\", \"meta\": true}}}"
    CLONE_TARGET="$(mktemp -d)/clone-depths"
    rm -rf "$CLONE_TARGET"

    cd "$TEST_DIR"
    # meta's --depth: one nested level; git clone's: one commit of history
    run "$META_BIN" -r --depth 1 git clone "file://$ROOT_BARE" "$CLONE_TARGET" --depth 1
    [ "$status" -eq 0 ]
    [ -d "$CLONE_TARGET/l1/l2/.git" ]
    [ ! -d "$CLONE_TARGET/l1/l2/l3" ]
    [ "$(git -C "$CLONE_TARGET/l1" rev-list --count HEAD)" -eq 1 ]
    # meta's -r after the command is refused, with the line to run
    run "$META_BIN" git clone "file://$ROOT_BARE" other -r
    [ "$status" -eq 2 ]
    [[ "$output" == *"as in meta -r git clone file://$ROOT_BARE other"* ]]
    [ ! -e other ]

    rm -rf "$L3_BARE" "$L2_BARE" "$L1_BARE" "$ROOT_BARE" "$CLONE_TARGET" "$work"
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
    META_MCP_BIN="$META_BIN_DIR/meta-mcp"
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

@test "arguments after -- that look like meta flags reach git" {
    echo x > "$TEST_DIR/frontend/--json"
    run "$META_BIN" --include frontend git add -- --json
    [ "$status" -eq 0 ]
    [ "$(git -C "$TEST_DIR/frontend" diff --cached --name-only)" = "--json" ]
}

@test "--help and -h after -- reach git as file names" {
    META_MCP_BIN="$META_BIN_DIR/meta-mcp"
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
        # in meta-git's own data dir, which meta makes private
        [[ "$path" == "$META_DATA_DIR/data/git/META_COMMIT_EDITMSG-"* ]]
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
    [[ "$output" == *"needs the terminal, but the terminal's foreground is another program"* ]]
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
    # this build's plugin, not one on PATH
    mkdir -p "$WS/.meta/plugins"
    cp "$META_BIN_DIR/meta-git" "$WS/.meta/plugins/"
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
    run "$META_BIN" -r git update
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
    bare_with_meta child '{"projects": {"x": {"repo": "u", "path": ".git/x"}}}'
    bare_with_meta root "{\"projects\": {\"child\": {\"repo\": \"$UP/child.git\", \"meta\": true}, \"gone\": \"$UP/gone.git\"}}"

    cd "$TEST_DIR"
    run "$META_BIN" --recursive git clone "$UP/root.git" "$UP/clone"
    [ "$status" -ne 0 ]
    [[ "$output" == *"Cannot read nested .meta.yaml"* ]]
    [[ "$output" == *"leads into .git"* ]]
    # A failed clone is still reported alongside it
    [[ "$output" == *"Failed to clone"*"gone"* ]]
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
    run "$META_BIN" git status --short
    [ "$status" -eq 0 ]
    [[ "$output" == *" M README.md"* ]]
    [[ "$output" != *"Changes not staged"* ]]
    # into a pipe uncoloured; colour forced, git colours it
    [[ "$output" != *$'\033['* ]]
    CLICOLOR_FORCE=1 run "$META_BIN" git status --short
    [[ "$output" == *$'\033['* ]]
    # meta's flag after the command is git's argument
    run "$META_BIN" git status --tag backend
    [ "$status" -ne 0 ]
    [[ "$output" == *"unknown option"*"tag"* ]]

    git clone --quiet --bare "$TEST_DIR/shared" "$TEST_DIR/shared.git"
    rm -rf "$TEST_DIR/shared"
    sed -i.bak "s|git@github.com:org/shared.git|file://$TEST_DIR/shared.git|" "$TEST_DIR/.meta.yaml"
    run "$META_BIN" git update --dry-run
    [ "$status" -ne 0 ]
    [[ "$output" == *"unexpected argument '--dry-run'"*"goes before the command, as in meta --dry-run git update"* ]]
    [ ! -e "$TEST_DIR/shared" ]
    run "$META_BIN" git update --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage: meta git update"* ]]
    run "$META_BIN" git status --help
    [ "$status" -eq 0 ]
    [[ "${lines[0]}" == '`meta git status` runs as typed (`git status`) in each selected repo it applies to.' ]]
}

@test "meta help never runs the command, even with -- in the words" {
    echo junk > "$TEST_DIR/frontend/junk"
    run "$META_BIN" help git clean -f -- .
    [ "$status" -eq 0 ]
    [ -f "$TEST_DIR/frontend/junk" ]
    [[ "$output" == *'runs as typed (`git clean -f`)'* ]]
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

@test "a failed clone of the meta repo shows git's error, its stderr being the user's" {
    cd "$TEST_DIR"
    run "$META_BIN" git clone "file://$TEST_DIR/nowhere.git" dest
    [ "$status" -ne 0 ]
    [[ "$output" == *"fatal: "*"nowhere.git"* ]]
    [ "$(tail -1 <<<"$output")" = "Error: Failed to clone meta repository" ]
    [ ! -e dest ]
}

@test "the summary shows why each command failed, from its stdout and stderr, in every mode" {
    with_remotes
    # git as it fails to reach a remote it has no access to, in frontend
    mkdir -p "$TEST_DIR/shim"
    real_git="$(command -v git)"
    cat > "$TEST_DIR/shim/git" <<EOF
#!/bin/sh
case "\$PWD \$* " in *frontend*" pull "*)
    printf '%s\n' 'ERROR: Repository not found.' 'fatal: Could not read from remote repository.' '' \
        'Please make sure you have the correct access rights' 'and the repository exists.' >&2
    exit 1;;
esac
exec "$real_git" "\$@"
EOF
    chmod +x "$TEST_DIR/shim/git"
    local said="✗ ./frontend: git pull (exit code 1)"$'\n'"    ERROR: Repository not found."$'\n'"    fatal: Could not read from remote repository."$'\n'"    Please make sure you have the correct access rights"$'\n'"    and the repository exists."
    # captured (several repos at once), and live: one after another, one
    # repo alone; live, git's lines are shown as they come too
    for mode in --parallel --sequential "--sequential --include frontend" "--include frontend"; do
        # shellcheck disable=SC2086
        PATH="$TEST_DIR/shim:$PATH" run "$META_BIN" $mode git pull
        [ "$status" -eq 1 ]
        [[ "${output#*Summary:}" == *"$said"* ]]
        [[ "${output%%Summary:*}" == *"Please make sure"* ]]
    done
    # a long output: its first lines, how many are left out, and its last;
    # eleven are all shown (a marker would hide two)
    run "$META_BIN" --parallel --include frontend,backend exec -- sh -c '[ "${PWD##*/}" = backend ] && exit 0; for i in $(seq 15); do echo "line $i" >&2; done; exit 2'
    [ "$status" -eq 1 ]
    local summary=${output#*Summary:}
    [[ "$summary" == *"    line 5"$'\n'"    … 6 more lines"$'\n'"    line 12"$'\n'"    line 13"$'\n'"    line 14"$'\n'"    line 15"* ]]
    [[ "$summary" != *"line 6"* && "$summary" != *"line 11"* ]]
    run "$META_BIN" --sequential --include frontend exec -- sh -c 'for i in $(seq 11); do echo "line $i"; done; exit 2'
    summary=${output#*Summary:}
    [[ "$summary" == *"    line 1"$'\n'*"    line 11"* && "$summary" != *"more lines"* ]]
    # a line is cut, colour codes go, and a line of codes alone takes no row
    run "$META_BIN" --parallel --include frontend,backend exec -- sh -c '[ "${PWD##*/}" = backend ] && exit 0; printf "\033[31merror:\033[0m red\n\033[0m\n"; head -c 1000 /dev/zero | tr "\0" x >&2; echo >&2; exit 2'
    summary=${output#*Summary:}
    [[ "$summary" == *"(exit code 2)"$'\n'"    error: red"$'\n'"    $(printf 'x%.0s' $(seq 300))… 700 more characters"* ]]
    # one that wrote nothing says so on its line; one that didn't start, why
    run "$META_BIN" --parallel --include frontend,backend exec -- sh -c '[ "${PWD##*/}" = backend ] || exit 2'
    [[ "$output" == *"✗ ./frontend: sh -c "*" (exit code 2): no output"* ]]
    run "$META_BIN" --sequential --include frontend exec -- ./no-such-program
    [[ "${output#*Summary:}" == *"✗ ./frontend: ./no-such-program (didn't start)"$'\n'"    Failed to run ./no-such-program: "* ]]
    # a merge conflict says why on stdout
    git -C frontend config pull.rebase false
    teammate frontend README.md theirs
    echo mine > frontend/README.md
    git -C frontend add README.md
    git -C frontend commit --quiet -m "mine"
    run "$META_BIN" --parallel git pull
    [ "$status" -eq 1 ]
    [[ "${output#*Summary:}" == *"✗ ./frontend: git pull (exit code 1)"*"    CONFLICT ("*"Merge conflict in README.md"* ]]
    git -C frontend merge --abort
    # git's real reason comes after its hints: a pull of a branch that has
    # diverged from its remote, as git words it
    git -C frontend config --unset pull.rebase
    run "$META_BIN" --parallel git pull
    [ "$status" -eq 1 ]
    summary=${output#*Summary:}
    [[ "$summary" == *"✗ ./frontend: git pull (exit code 128)"*"hint: "*" more lines"*"fatal: Need to specify how to reconcile divergent branches."* ]]
}

@test "a stopped run prints what finished and names what still ran" {
    with_remotes
    # SIGTERM: a background job starts with SIGINT ignored, so meta leaves
    # that alone; the same stop serves both
    "$META_BIN" --parallel --include frontend,backend exec -- sh -c '[ "${PWD##*/}" = backend ] || { echo "fatal: frontend broke" >&2; exit 1; }; exec sleep 30' > out.txt 2>&1 &
    local meta=$!
    sleep 2
    kill -TERM "$meta"
    local ended=0
    wait "$meta" || ended=$?
    [ "$ended" -eq 143 ]
    run cat out.txt
    [[ "$output" == *"Interrupted: 1 of 2 commands finished, 1 failed"* ]]
    [[ "$output" == *"✗ ./frontend: sh -c "*" (exit code 1)"$'\n'"    fatal: frontend broke"* ]]
    [[ "$output" == *"✗ Stopped while running: ./backend"* ]]
}

@test "a failed clone ends with git's error, as the summary shows a command's" {
    with_remotes
    rm -rf "$TEST_DIR/shared"
    # git that fails every clone, saying why on its last line
    mkdir -p "$TEST_DIR/shim"
    real_git="$(command -v git)"
    printf '#!/bin/sh\ncase " $* " in *" clone "*) printf "FIRST diagnostic\\nLAST diagnostic\\n" >&2; exit 1;; esac\nexec "%s" "$@"\n' "$real_git" > "$TEST_DIR/shim/git"
    chmod +x "$TEST_DIR/shim/git"
    for mode in "" "--silent"; do
        # shellcheck disable=SC2086
        PATH="$TEST_DIR/shim:$PATH" run "$META_BIN" $mode git update
        [ "$status" -ne 0 ]
        [[ "$output" == *"failed to clone: shared:"$'\n'"    FIRST diagnostic"$'\n'"    LAST diagnostic" ]]
    done
}
