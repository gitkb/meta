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
    git -C "$WS" remote add origin "https://alice:$TOKEN@example.invalid/org/ws.git"
    git clone -q "$TEST_DIR/remotes/other.git" "$WS/moved"
    cat > "$WS/.meta.yaml" <<EOF
projects:
  svc:
    repo: https://alice:$TOKEN@127.0.0.1:9/org/svc.git
  lib: https://$TOKEN@example.invalid/org/lib.git
  moved: https://$TOKEN@example.invalid/org/elsewhere.git
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
    [[ "$output" == *"git clone https://***@127.0.0.1:9/org/svc.git"* ]]

    meta_without_token project list
    [ "$status" -eq 0 ]
    [[ "$output" == *". (https://***@example.invalid/org/ws.git)"* ]]
    [[ "$output" == *"svc (https://***@127.0.0.1:9/org/svc.git)"* ]]
    meta_without_token project list --json
    [ "$status" -eq 0 ]
    [ "$(jq -r .repo <<< "$output")" = "https://***@example.invalid/org/ws.git" ]
    meta_without_token --recursive project list --json
    [ "$(jq -r '.projects[] | select(.name == "lib") | .repo' <<< "$output")" = "https://***@example.invalid/org/lib.git" ]
    meta_without_token project check
    [[ "$output" == *"URL: https://***@127.0.0.1:9/org/svc.git"* ]]
    meta_without_token context --json --no-cache
    [ "$(jq -r '.repos[] | select(.name == "svc") | .repo' <<< "$output")" = "https://***@127.0.0.1:9/org/svc.git" ]
    meta_without_token context --json
    [ "$status" -eq 0 ]

    # svc can't be reached; lib is cloned with its token
    RUST_LOG=debug meta_without_token git update
    [ "$status" -ne 0 ]
    [[ "$output" == *"Failed to clone svc (https://***@127.0.0.1:9/org/svc.git)"* ]]
    [[ "$output" == *"expected: https://***@example.invalid/org/elsewhere.git"* ]]
    [[ "$output" == *"remote set-url origin <its repo URL in .meta.yaml, not shown: it holds credentials>"* ]]
    [ "$(git -C lib config remote.origin.url)" = "https://$TOKEN@example.invalid/org/lib.git" ]

    cd "$TEST_DIR"
    meta_without_token --dry-run git clone "https://$TOKEN@example.invalid/org/meta.git" fresh
    [ "$status" -eq 0 ]
    [[ "$output" == *"https://***@example.invalid/org/meta.git"* ]]
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
    cat > .meta.yaml <<EOF
projects:
  r1: ext::rendezvous r1 $TEST_DIR/remotes/lib.git
  r2: ext::rendezvous r2 $TEST_DIR/remotes/other.git
  r3: ext::rendezvous r3 $TEST_DIR/remotes/meta.git
EOF
    # an ssh that would show it was run
    printf '#!/bin/sh\ntouch "%s/ssh-ran"\nexit 255\n' "$TEST_DIR" > "$TEST_DIR/bin/ssh"
    chmod +x "$TEST_DIR/bin/ssh"
    run "$META_BIN" git update
    [ "$status" -eq 0 ]
    [ ! -e "$TEST_DIR/ssh-ran" ]
    [[ "$output" != *"SSH"* ]]
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
    [[ "$output" == *"SSH master for git@nohost.invalid (port 22) could not be started"* ]]
    [[ "$output" == *"cloning one repository at a time"* ]]
}
