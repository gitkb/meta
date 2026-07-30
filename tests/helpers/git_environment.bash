clear_git_local_env() {
    local git_env_var
    while IFS= read -r git_env_var; do
        if [[ "$git_env_var" =~ ^[A-Z0-9_]+$ ]]; then
            unset "$git_env_var"
        fi
    done < <(git rev-parse --local-env-vars)
}
