clear_git_local_env() {
    local git_env_var git_local_env_vars
    git_local_env_vars="$(git rev-parse --local-env-vars)" || {
        echo "Error: Could not determine repository-local Git environment" >&2
        return 1
    }
    while IFS= read -r git_env_var; do
        if [[ ! "$git_env_var" =~ ^[A-Z0-9_]+$ ]]; then
            echo "Error: Refusing invalid Git environment variable name: $git_env_var" >&2
            return 1
        fi
        unset "$git_env_var"
    done <<< "$git_local_env_vars"
}

# Give each test its own home, config and meta data dir, so a run never reads
# or writes the user's real ones (their global plugins included). Cargo and
# rustup keep their real homes, so a test that builds still finds them.
isolate_user_env() {
    export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
    export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
    local user_env="$BATS_TEST_TMPDIR/user-env"
    mkdir -p "$user_env/home" "$user_env/config" "$user_env/data" "$user_env/meta"
    : > "$user_env/gitconfig"
    export HOME="$user_env/home"
    export XDG_CONFIG_HOME="$user_env/config"
    export XDG_DATA_HOME="$user_env/data"
    export META_DATA_DIR="$user_env/meta"
    export GIT_CONFIG_GLOBAL="$user_env/gitconfig"
    # Tests that want these set them themselves
    unset META_WORKTREES RUST_LOG NO_COLOR CLICOLOR CLICOLOR_FORCE FORCE_COLOR
    unset META_GITHUB_URL META_PLATFORM GIT_TEMPLATE_DIR GIT_SSH_COMMAND GIT_EDITOR
}

# Every test's setup starts here: a home, config, git config, meta data dir
# and temp dir of the test's own (all under $BATS_TEST_TMPDIR, so nothing
# outlives the test), no system git config, new repos on main, and a commit
# identity. TEST_DIR is an empty folder for the test's workspace, as its
# physical path (macOS temp dirs sit behind a symlink, and meta and git print
# the real path).
# The binaries under test are in META_BIN_DIR (setup_suite.bash).
sandbox() {
    # isolated before git first runs: the user's git config could be broken
    isolate_user_env
    export GIT_CONFIG_NOSYSTEM=1
    clear_git_local_env || return 1
    mkdir -p "$BATS_TEST_TMPDIR/user-env/runtime" "$BATS_TEST_TMPDIR/tmp" "$BATS_TEST_TMPDIR/ws"
    chmod 700 "$BATS_TEST_TMPDIR/user-env/runtime"
    # whatever the machine's git defaults to, a new repo starts on main
    git config --file "$GIT_CONFIG_GLOBAL" init.defaultBranch main
    export XDG_RUNTIME_DIR="$BATS_TEST_TMPDIR/user-env/runtime"
    export TMPDIR="$BATS_TEST_TMPDIR/tmp"
    export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.com
    export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.com
    TEST_DIR="$(cd "$BATS_TEST_TMPDIR/ws" && pwd -P)"
    META_BIN="$META_BIN_DIR/meta"
    META_GIT_BIN="$META_BIN_DIR/meta-git"
}

# make_workspace [--root-repo] [--init-only] [--plugins "git project ..."] REPO...
# Each REPO becomes a repo in $TEST_DIR on branch main with one commit of a
# README.md (--init-only: no commit), and a project of .meta.yaml, written
# unless the test wrote its own first. --root-repo makes $TEST_DIR a repo too,
# with .meta.yaml committed. --plugins puts meta-<word> in .meta/plugins.
# Every step must succeed: setup runs under set -e.
make_workspace() {
    local root_repo="" init_only="" plugins="" repo word
    while [ $# -gt 0 ]; do
        case "$1" in
            --root-repo) root_repo=1 ;;
            --init-only) init_only=1 ;;
            --plugins) plugins="$2"; shift ;;
            *) break ;;
        esac
        shift
    done
    for word in $plugins; do
        mkdir -p "$TEST_DIR/.meta/plugins"
        cp "$META_BIN_DIR/meta-$word" "$TEST_DIR/.meta/plugins/meta-$word"
    done
    for repo in "$@"; do
        git init --quiet -b main "$TEST_DIR/$repo"
        echo "# $repo" > "$TEST_DIR/$repo/README.md"
        if [ -z "$init_only" ]; then
            git -C "$TEST_DIR/$repo" add README.md
            git -C "$TEST_DIR/$repo" commit --quiet -m "Initial commit for $repo"
        fi
    done
    if [ ! -e "$TEST_DIR/.meta.yaml" ]; then
        {
            echo "projects:"
            for repo in "$@"; do echo "  $repo: git@github.com:org/$repo.git"; done
        } > "$TEST_DIR/.meta.yaml"
    fi
    if [ -n "$root_repo" ]; then
        git init --quiet -b main "$TEST_DIR"
        git -C "$TEST_DIR" add .meta.yaml
        git -C "$TEST_DIR" commit --quiet -m "Initial meta commit"
    fi
}
