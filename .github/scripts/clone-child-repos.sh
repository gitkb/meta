#!/usr/bin/env bash
set -euo pipefail

child_branch="${META_CHILD_BRANCH:-${GITHUB_HEAD_REF:-${GITHUB_REF_NAME:-}}}"

branch_candidates=()
if [[ -n "$child_branch" ]]; then
  branch_candidates+=("$child_branch")

  normalized_branch="${child_branch//\//-}"
  if [[ "$normalized_branch" != "$child_branch" ]]; then
    branch_candidates+=("$normalized_branch")
  fi
fi

clone_child() {
  local repo="$1"
  local url="https://github.com/gitkb/${repo}.git"
  local branch
  local ls_remote_output
  local ls_remote_status

  for branch in "${branch_candidates[@]}"; do
    set +e
    ls_remote_output="$(git ls-remote --exit-code --heads "$url" "$branch" 2>&1)"
    ls_remote_status=$?
    set -e

    if [[ $ls_remote_status -eq 0 ]]; then
      echo "Cloning ${repo} from branch ${branch}"
      git clone --depth 1 --branch "$branch" "$url"
      return
    fi

    if [[ $ls_remote_status -ne 2 ]]; then
      echo "Failed to query branch '${branch}' for ${repo} (status: ${ls_remote_status})" >&2
      if [[ -n "$ls_remote_output" ]]; then
        echo "$ls_remote_output" >&2
      fi
      exit "$ls_remote_status"
    fi
  done

  echo "Cloning ${repo} from default branch"
  git clone --depth 1 "$url"
}

clone_child loop_lib
clone_child loop_cli
clone_child meta_cli
clone_child meta_core
clone_child meta_git_cli
clone_child meta_git_lib
clone_child meta_mcp
clone_child meta_plugin_protocol
clone_child meta_rust_cli
clone_child meta_project_cli
