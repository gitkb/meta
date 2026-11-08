# Meta Destructive Command Safety

Multi-repo workspaces amplify the impact of destructive commands. The primary checkout is pull-only: change things, and run anything below, inside a worktree set (the `meta-worktree` skill).

## Before Destructive Operations

1. **Commit first**, inside your set
   ```
   meta git status
   meta git add -A && meta git commit -m "wip: before refactor"
   ```
   `meta git commit` commits only what is staged. Commit rather than stash: every worktree of a repo shares one stash, so another session's `git stash pop` can take yours. A backup branch (`git branch backup/<name>`) keeps only commits, not uncommitted changes.

2. **Preview with --dry-run**
   ```
   meta --dry-run exec -- rm -rf node_modules
   ```

3. **Target precisely** — avoid blanket operations
   ```
   meta --include api-service exec -- cargo clean
   ```

4. **Know what can't be undone**
   - Push with `--force-with-lease`, not `--force`: it refuses to overwrite commits you haven't seen.
   - Commit before `git reset --hard`: it discards uncommitted work for good.
   - `git clean -fd` removes untracked files for good; `git clean -nd` previews it.

## Blocked by `agent guard`

gitkb's `agent guard`, the PreToolUse hook, refuses these, as `.claude/agent-guard.toml` lists them. It is a safety net, not a lock: when the agent fails, the command runs.

- `git push` with `--force` or `-f`, unless the line has `--force-with-lease`
- `git reset --hard`
- `git clean` with both `-f` and `-d`
- `git checkout .` and `git checkout -- .`
- `git branch -D`
- `git stash drop` and `git stash clear`
- `rm -rf` on `.`, `..`, `/`, `~`, `$HOME`, `.meta`, `.meta.yaml`, `.meta.yml`, `*`, `./*`, `../*` or `./`

## Recovery

Run `git reflog` inside the set, in the affected repo, to find the previous commit, then restore it on a branch there:
```
git switch -c recovered <sha>
```
A set's commits are in that worktree's reflog, not the primary checkout's, and the reflog goes with the set: recover before removing it. To look across the set's repos, from inside it:
```
meta exec -- git reflog -n 20
```
The reflog only recovers commits, not uncommitted changes.

To undo a commit, in the set, one repo at a time:
```
meta --include <repo> exec -- git reset --soft HEAD~1
meta --include <repo> exec -- git revert --no-edit HEAD
```
The first is for a commit not pushed (its changes come back staged), the second for one already pushed (a new commit undoes it).
