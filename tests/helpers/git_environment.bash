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
