#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# A token in a repo URL reaches git, and nothing meta prints

TOKEN=ghp_SECRETTOKEN123

setup() {
    clear_git_local_env
    isolate_user_env
    META_BIN="$BATS_TEST_DIRNAME/../target/debug/meta"
    if [ ! -f "$META_BIN" ] || [ ! -f "$BATS_TEST_DIRNAME/../target/debug/meta-git" ]; then
        cargo build --workspace --quiet
    fi
    export GIT_AUTHOR_NAME=Test GIT_AUTHOR_EMAIL=test@example.com
    export GIT_COMMITTER_NAME=Test GIT_COMMITTER_EMAIL=test@example.com

    TEST_DIR="$(cd "$BATS_TEST_TMPDIR" && pwd -P)"
    mkdir -p "$TEST_DIR/remotes" "$TEST_DIR/bin"
    for plugin in meta-git meta-project; do
        cp "$BATS_TEST_DIRNAME/../target/debug/$plugin" "$TEST_DIR/bin/"
    done
    export PATH="$TEST_DIR/bin:$PATH"

    # `lib` and the meta repo live in local bare repos; git reaches them
    # only from the exact URL with the token in it (insteadOf)
    for name in lib meta other; do
        git init -q -b main "$TEST_DIR/seed-$name"
        git -C "$TEST_DIR/seed-$name" commit -q --allow-empty -m init
    done
    cat > "$TEST_DIR/seed-meta/.meta.yaml" <<EOF
projects:
  lib: https://$TOKEN@example.invalid/org/lib.git
EOF
    git -C "$TEST_DIR/seed-meta" add .meta.yaml
    git -C "$TEST_DIR/seed-meta" commit -q -m meta
    for name in lib meta other; do
        git clone -q --bare "$TEST_DIR/seed-$name" "$TEST_DIR/remotes/$name.git"
        git config --global "url.$TEST_DIR/remotes/$name.git.insteadOf" \
            "https://$TOKEN@example.invalid/org/$name.git"
    done

    # The workspace: its own origin, an unreachable project, one to clone,
    # and a checkout whose origin isn't the one configured
    WS="$TEST_DIR/ws"
    git init -q -b main "$WS"
    git -C "$WS" commit -q --allow-empty -m init
    git -C "$WS" remote add origin "https://alice:$TOKEN@example.invalid/org/ws.git"
    git clone -q "$TEST_DIR/remotes/other.git" "$WS/moved"
    # and one whose origin differs from the configured URL only in its token
    git clone -q "$TEST_DIR/remotes/other.git" "$WS/rotated"
    git -C "$WS/rotated" remote set-url origin "https://ghp_OLD@example.invalid/org/rotated.git"
    cat > "$WS/.meta.yaml" <<EOF
projects:
  svc:
    repo: https://alice:$TOKEN@127.0.0.1:9/org/svc.git
  lib: https://$TOKEN@example.invalid/org/lib.git
  moved: https://$TOKEN@example.invalid/org/elsewhere.git
  rotated: https://$TOKEN@example.invalid/org/rotated.git
EOF
    cd "$WS"
}

# Run meta with `args`; its stdout and stderr must not hold the token
meta_without_token() {
    run "$META_BIN" "$@"
    if [[ "$output" == *"$TOKEN"* ]]; then
        echo "meta $* printed the token:"
        echo "$output"
        return 1
    fi
}

@test "no command prints the credentials in a repo URL" {
    meta_without_token --dry-run git update
    [ "$status" -eq 0 ]
    [[ "$output" == *"git clone https://***@127.0.0.1:9/org/svc.git"* ]] || false

    meta_without_token project list
    [ "$status" -eq 0 ]
    [[ "$output" == *". (https://***@example.invalid/org/ws.git)"* ]] || false
    [[ "$output" == *"svc (https://***@127.0.0.1:9/org/svc.git)"* ]] || false
    meta_without_token project list --json
    [ "$status" -eq 0 ]
    [ "$(jq -r .repo <<< "$output")" = "https://***@example.invalid/org/ws.git" ]
    meta_without_token --recursive project list --json
    [ "$(jq -r '.projects[] | select(.name == "lib") | .repo' <<< "$output")" = "https://***@example.invalid/org/lib.git" ]
    meta_without_token project check
    [[ "$output" == *"URL: https://***@127.0.0.1:9/org/svc.git"* ]] || false
    # commands meta shows, as typed
    meta_without_token "https://$TOKEN@example.invalid/org/x.git"
    [[ "$output" == *"unrecognized command 'https://***@example.invalid/org/x.git'"* ]] || false
    # errors and plans repeating a URL typed where a name or spec goes
    meta_without_token --include "https://$TOKEN@example.invalid/org/x.git" exec -- true
    [ "$status" -ne 0 ]
    [[ "$output" == *"--include 'https://***@example.invalid/org/x.git'"* ]] || false
    # log records (a plugin download URL, logged at info)
    RUST_LOG=info meta_without_token plugin install "https://u:$TOKEN@127.0.0.1:9/x.tar.gz"
    [[ "$output" == *"Downloading from: https://***@127.0.0.1:9/x.tar.gz"* ]] || false
    meta_without_token --config "https://$TOKEN@example.invalid/org/x.yaml" exec -- true
    [ "$status" -ne 0 ]
    [[ "$output" == *"'https://***@example.invalid/org/x.yaml'"* ]] || false
    meta_without_token --verbose --tag "https://$TOKEN@example.invalid/org/x" exec -- true
    [[ "$output" == *'Filtering projects by tags: ["https://***@example.invalid/org/x"]'* ]] || false
    # a comma in a password: the value is split there
    meta_without_token --verbose --tag "https://a,$TOKEN@example.invalid/org/x" exec -- true
    meta_without_token --include "https://a,$TOKEN@example.invalid/org/x" exec -- true
    [ "$status" -ne 0 ]
    meta_without_token --exclude "https://a:$TOKEN,tail@example.invalid/org/x" exec -- true
    [ "$status" -ne 0 ]
    [[ "$output" == *"--exclude 'https://***'"* ]] || false
    # a name with an @ and no URL shows as typed
    meta_without_token --include "lib@v2" exec -- true
    [[ "$output" == *"--include 'lib@v2'"* ]] || false
    meta_without_token project "https://$TOKEN@example.invalid/org/x.git"
    [[ "$output" == *"unrecognized command 'project https://***@example.invalid/org/x.git'"* ]] || false
    meta_without_token git worktree create x --repo moved --from-pr "https://$TOKEN@127.0.0.1:9/o/r/pull/zz"
    [ "$status" -ne 0 ]
    [[ "$output" == *"https://***@127.0.0.1:9/o/r/pull/zz"* ]] || false
    meta_without_token --dry-run git worktree create x --repo moved --from-pr "https://$TOKEN@127.0.0.1:9/o/r#5"
    [[ "$output" == *"origin 'https://***@127.0.0.1:9/o/r'"* ]] || false
    meta_without_token --dry-run git worktree create x --repo "moved:https://$TOKEN@127.0.0.1:9/b"
    meta_without_token --verbose git worktree create vb --repo moved --branch "https://$TOKEN@127.0.0.1:9/b"
    meta_without_token --dry-run git worktree create x "https://$TOKEN@127.0.0.1:9/r" --repo moved \
        --meta "origin=https://$TOKEN@127.0.0.1:9/r" --meta "https://$TOKEN@127.0.0.1:9/r"
    [[ "$output" == *"ref 'https://***@127.0.0.1:9/r' not found"* ]] || false
    meta_without_token --dry-run git worktree create x main --repo moved --meta "origin=https://$TOKEN@127.0.0.1:9/r" \
        --meta "h=foo::https://alice:p w$TOKEN@127.0.0.1:9/r"
    # metadata prints in no fixed order
    [[ "$output" == *"origin=https://***@127.0.0.1:9/r"* ]] || false
    [[ "$output" == *"h=foo::https://***@127.0.0.1:9/r"* ]] || false
    # metadata as created and as listed later (meta keeps it as typed)
    meta_without_token --json git worktree create mt --repo moved --meta "src=https://$TOKEN@127.0.0.1:9/r" \
        --meta "h=foo::https://alice:p w$TOKEN@127.0.0.1:9/r"
    [ "$status" -eq 0 ]
    meta_without_token git worktree list --recursive --metadata-only --json
    [[ "$output" == *'"src": "https://***@127.0.0.1:9/r"'* ]] || false
    [[ "$output" == *'"h": "foo::https://***@127.0.0.1:9/r"'* ]] || false
    meta_without_token git worktree diff mt --base "https://$TOKEN@127.0.0.1:9/r"
    [[ "$output" == *"mt vs https://***@127.0.0.1:9/r"* ]] || false
    # a branch typed as a URL, when adding a repo to the set
    meta_without_token --dry-run git worktree add mt --repo "rotated:https://$TOKEN@127.0.0.1:9/b"
    [ "$status" -eq 0 ]
    [[ "$output" == *"(branch: https://***@127.0.0.1:9/b, new)"* ]] || false
    meta_without_token --dry-run --json git worktree add mt --repo "rotated:https://$TOKEN@127.0.0.1:9/b"
    [ "$status" -eq 0 ]
    [[ "$output" == *'"branch": "https://***@127.0.0.1:9/b"'* ]] || false
    meta_without_token --verbose git worktree add mt --repo "rotated:https://$TOKEN@127.0.0.1:9/b"
    [[ "$output" == *"(branch: https://***@127.0.0.1:9/b)"* ]] || false
    meta_without_token git worktree "https://$TOKEN@example.invalid/org/x.git"
    [[ "$output" == *"unrecognized worktree subcommand 'https://***@example.invalid/org/x.git'"* ]] || false
    meta_without_token --dry-run git remote set-url origin "https://$TOKEN@example.invalid/org/x.git"
    [[ "$output" == *"git remote set-url origin 'https://***@example.invalid/org/x.git'"* ]] || false
    meta_without_token git fetch "https://$TOKEN@example.invalid/org/x.git" --help
    [ "$status" -eq 0 ]
    meta_without_token git "https://$TOKEN@example.invalid/org/x.git" --help
    [[ "$output" == *"meta git https://***@example.invalid/org/x.git"* ]] || false
    meta_without_token context --json --no-cache
    [ "$(jq -r '.repos[] | select(.name == "svc") | .repo' <<< "$output")" = "https://***@127.0.0.1:9/org/svc.git" ]
    # the cache it writes, and what it shows from it
    meta_without_token context --json
    [ "$status" -eq 0 ]
    meta_without_token context --json
    [ "$(jq -r '.repos[] | select(.name == "svc") | .repo' <<< "$output")" = "https://***@127.0.0.1:9/org/svc.git" ]
    run grep -rlq "127.0.0.1:9/org/svc.git" "$META_DATA_DIR"
    [ "$status" -eq 0 ]
    run grep -rq "$TOKEN" "$META_DATA_DIR"
    [ "$status" -eq 1 ]

    # svc can't be reached; lib is cloned with its token
    RUST_LOG=debug meta_without_token git update
    [ "$status" -ne 0 ]
    [[ "$output" == *"Failed to clone svc (https://***@127.0.0.1:9/org/svc.git)"* ]] || false
    [[ "$output" == *"expected: https://***@example.invalid/org/elsewhere.git"* ]] || false
    [[ "$output" == *"remote set-url origin <"*"not shown"* ]] || false
    [[ "$output" == *"(they differ only in their credentials)"* ]] || false
    [[ "$output" != *"ghp_OLD"* ]] || false
    [ "$(git -C lib config remote.origin.url)" = "https://$TOKEN@example.invalid/org/lib.git" ]
    # a set of one project, where --tag matches nothing
    git -C lib worktree add -q -b wt2 "$WS/.worktrees/wt2/lib"
    (cd .worktrees/wt2 && meta_without_token --tag "https://$TOKEN@example.invalid/x" exec -- true &&
        [[ "$output" == *"no projects match tag filter 'https://***@example.invalid/x'"* ]])

    cd "$TEST_DIR"
    meta_without_token --dry-run git clone "https://$TOKEN@example.invalid/org/meta.git" fresh
    [ "$status" -eq 0 ]
    [[ "$output" == *"https://***@example.invalid/org/meta.git"* ]] || false
    # a helper other than ext:: gets its URL as typed, a space in its
    # password included
    meta_without_token --dry-run git clone "foo::https://alice:p w$TOKEN@127.0.0.1:9/r" fresh2
    [ "$status" -eq 0 ]
    [[ "$output" == *"foo::https://***@127.0.0.1:9/r"* ]] || false
    # a helper's URL with no path names no clone directory after its token
    meta_without_token --dry-run git clone "persistent-https::https://$TOKEN@example.invalid/"
    [ "$status" -eq 0 ]
    RUST_LOG=debug meta_without_token git clone "https://$TOKEN@example.invalid/org/meta.git" fresh
    [ "$status" -eq 0 ]
    [ -d fresh/lib/.git ]
}

@test "remote helper URLs are no ssh hosts" {
    # each helper waits up to 10 s for all three to start: they clone only
    # if meta runs them at once, as it didn't when it read `ext::` as ssh
    git config --global protocol.ext.allow always
    cat > "$TEST_DIR/bin/rendezvous" <<EOF
#!/bin/sh
touch "$TEST_DIR/started-\$1"
for _ in \$(seq 100); do
    [ -e "$TEST_DIR/started-r1" ] && [ -e "$TEST_DIR/started-r2" ] &&
        [ -e "$TEST_DIR/started-r3" ] && exec git upload-pack "\$2"
    sleep 0.1
done
exit 1
EOF
    chmod +x "$TEST_DIR/bin/rendezvous"
    # `% ` is a space inside one of ext's arguments
    remotes="${TEST_DIR// /% }/remotes"
    cat > .meta.yaml <<EOF
projects:
  r1: ext::rendezvous r1 $remotes/lib.git
  r2: ext::rendezvous r2 $remotes/other.git
  r3: ext::rendezvous r3 $remotes/meta.git
EOF
    # an ssh that would show it was run
    printf '#!/bin/sh\ntouch "%s/ssh-ran"\nexit 255\n' "$TEST_DIR" > "$TEST_DIR/bin/ssh"
    chmod +x "$TEST_DIR/bin/ssh"
    run "$META_BIN" git update
    [ "$status" -eq 0 ]
    [ ! -e "$TEST_DIR/ssh-ran" ]
    [[ "$output" != *"SSH"* ]] || false
    [ -d r1/.git ]
    [ -d r2/.git ]
    [ -d r3/.git ]
}

@test "a remote helper's own output shows no credentials" {
    # a helper that prints the address it got, without `foo::` (so a space
    # in the password ends no scan): whole, then in a line meta's read cuts
    # inside a second copy, after the password's second space. An `@` in the
    # password too
    cat > "$TEST_DIR/bin/git-remote-foo" <<'EOF'
#!/bin/sh
echo "fatal: cannot open $2" >&2
printf 'fatal: %s %03945d retry %s\n' "$2" 0 "$2" >&2
exit 1
EOF
    chmod +x "$TEST_DIR/bin/git-remote-foo"
    url="foo::https://alice:p@s $TOKEN w vvvvvvvvvvvvvvvvvvvv@127.0.0.1:9/owner/svc.git"
    printf 'projects:\n  svc: "%s"\n' "$url" > .meta.yaml
    meta_without_token git update
    [ "$status" -ne 0 ]
    [[ "$output" == *"fatal: cannot open https://***@127.0.0.1:9/owner/svc.git"* ]] || false
    [[ "$output" != *"SECRET"* ]] || false
    # the same, when the --from-pr fetch fails
    git init -q -b main svc
    git -C svc commit -q --allow-empty -m init
    git -C svc remote add origin "$url"
    printf '#!/bin/sh\necho feature\n' > "$TEST_DIR/bin/gh"
    chmod +x "$TEST_DIR/bin/gh"
    meta_without_token git worktree create pr --repo svc --from-pr owner/svc#1
    [ "$status" -ne 0 ]
    [[ "$output" == *"fatal: cannot open https://***@127.0.0.1:9/owner/svc.git"* ]] || false
    [[ "$output" != *"SECRET"* ]] || false
}

@test "a failed ssh setup says so" {
    printf '#!/bin/sh\nexit 255\n' > "$TEST_DIR/bin/ssh"
    chmod +x "$TEST_DIR/bin/ssh"
    cat > .meta.yaml <<EOF
projects:
  a: git@nohost.invalid:org/a.git
  b: git@nohost.invalid:org/b.git
EOF
    run "$META_BIN" git update
    [[ "$output" == *"SSH master for git@nohost.invalid (port 22) could not be started"* ]] || false
    [[ "$output" == *"cloning one repository at a time"* ]]
}
