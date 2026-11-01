#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# A token in a repo URL reaches git as written, and meta prints it hidden

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

    # local bare repos: `lib`, and a meta repo whose config holds a token
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
    done

    WS="$TEST_DIR/ws"
    git init -q -b main "$WS"
    git -C "$WS" commit -q --allow-empty -m init
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

@test "a repo URL with credentials reaches git, the token never printed" {
    # git fetches each token URL from a local bare repo: the clone works only
    # if git gets the URL exactly as configured
    for name in lib meta; do
        git config --global "url.$TEST_DIR/remotes/$name.git.insteadOf" \
            "https://$TOKEN@example.invalid/org/$name.git"
    done
    git config --global "url.$TEST_DIR/remotes/other.git.insteadOf" \
        "https://alice:$TOKEN@example.invalid/org/svc.git"
    cat > .meta.yaml <<EOF
projects:
  lib: https://$TOKEN@example.invalid/org/lib.git
  svc:
    repo: https://alice:$TOKEN@example.invalid/org/svc.git
EOF
    meta_without_token --dry-run git update
    [ "$status" -eq 0 ]
    [[ "$output" == *"git clone -- 'https://***@example.invalid/org/lib.git'"* ]] || false
    meta_without_token project check --json
    [[ "$output" == *'"repo": "https://***@example.invalid/org/lib.git"'* ]] || false
    meta_without_token git update
    [ "$status" -eq 0 ]
    [ -d lib/.git ]
    [ -d svc/.git ]
    # insteadOf matches a prefix: the origin shows the URL git got
    [ "$(git -C lib config remote.origin.url)" = "https://$TOKEN@example.invalid/org/lib.git" ]
    [ "$(git -C svc config remote.origin.url)" = "https://alice:$TOKEN@example.invalid/org/svc.git" ]
    for command in "project list" "project list --json" "--recursive project list --json" \
        "project check" "context --json" "git status" "exec -- true"; do
        # shellcheck disable=SC2086
        meta_without_token $command
        [ "$status" -eq 0 ]
    done
    meta_without_token project list --json
    [[ "$output" == *'"repo": "https://***@example.invalid/org/svc.git"'* ]] || false

    # a checkout of another repository: the fix can't print the hidden URL
    git -C svc remote set-url origin "$TEST_DIR/remotes/lib.git"
    meta_without_token git update
    [[ "$output" == *"expected: https://***@example.invalid/org/svc.git"* ]] || false
    [[ "$output" == *"remote set-url origin <the repo URL in .meta.yaml>"* ]] || false
    # a rotated token: the same URL printed twice, and why they differ
    git -C svc remote set-url origin "https://alice:ghp_OLD@example.invalid/org/svc.git"
    meta_without_token git update
    [[ "$output" == *"actual:   https://***@example.invalid/org/svc.git"* ]] || false
    [[ "$output" == *"(they differ only in their hidden credentials)"* ]] || false
    [[ "$output" != *ghp_OLD* ]] || false

    cd "$TEST_DIR"
    RUST_LOG=debug meta_without_token --dry-run git clone "https://$TOKEN@example.invalid/org/meta.git" fresh
    [ "$status" -eq 0 ]
    [[ "$output" == *"git clone -- 'https://***@example.invalid/org/meta.git' fresh"* ]] || false
    # the meta repo, and the project its own config lists with a token
    RUST_LOG=debug meta_without_token git clone "https://$TOKEN@example.invalid/org/meta.git" fresh
    [ "$status" -eq 0 ]
    [ "$(git -C fresh config remote.origin.url)" = "https://$TOKEN@example.invalid/org/meta.git" ]
    [ "$(git -C fresh/lib config remote.origin.url)" = "https://$TOKEN@example.invalid/org/lib.git" ]
}

@test "a clone's remote with a token prints hidden" {
    # tokens put there by hand: in the workspace's origin, and in the origin
    # of a checkout that isn't the configured one
    git -C "$WS" remote add origin "https://alice:$TOKEN@example.invalid/org/ws.git"
    git clone -q "$TEST_DIR/remotes/other.git" lib
    git -C lib remote set-url origin "https://$TOKEN@example.invalid/org/other.git"
    printf 'projects:\n  lib: %s\n' "$TEST_DIR/remotes/lib.git" > .meta.yaml
    meta_without_token project list
    [ "$status" -eq 0 ]
    [[ "$output" == *". (https://***@example.invalid/org/ws.git)"* ]] || false
    meta_without_token project list --json
    [ "$status" -eq 0 ]
    [ "$(jq -r .repo <<< "$output")" = "https://***@example.invalid/org/ws.git" ]
    meta_without_token git update
    [[ "$output" == *"actual:   https://***@example.invalid/org/other.git"* ]] || false
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
  r3: ext::rendezvous r3 $remotes/lib.git
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
    [[ "$output" == *"updating one repository at a time"* ]]
    # the same for empty folders git clones into
    mkdir a b
    run "$META_BIN" git update
    [[ "$output" == *"SSH master for git@nohost.invalid (port 22) could not be started"* ]] || false
    [[ "$output" == *"updating one repository at a time"* ]]
}
