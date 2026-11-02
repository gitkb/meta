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
}
