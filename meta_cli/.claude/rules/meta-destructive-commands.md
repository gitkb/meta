# Meta Destructive Command Safety

Multi-repo workspaces amplify the impact of destructive commands.

## Before Destructive Operations

1. **Commit or stash first**
   ```
   meta git status
   meta git add -A && meta git commit -m "wip: before refactor"
   ```
   Or stash it in every repo: `meta exec -- git stash push -u -m before-refactor`.
   A backup branch (`git branch backup/<name>`) keeps only commits, not uncommitted changes.

2. **Preview with --dry-run**
   ```
   meta --dry-run exec -- rm -rf node_modules
   ```

3. **Target precisely** — avoid blanket operations
   ```
   meta --include api-service exec -- cargo clean
   ```

## Blocked by `agent guard`

These commands trigger PreToolUse denial:
- `git push --force` (use `--force-with-lease` instead)
- `git reset --hard` (commit or stash first; recover commits with `git reflog`)
- `git clean -fd` (dangerous file removal)
- `rm -rf` on repo roots or `.meta*` paths

## Recovery

If something goes wrong, run `git reflog` in the affected repo to find the previous commit, then restore it on a branch:
```
git switch -c recovered <sha>
```
To look across all repos:
```
meta exec -- git reflog -n 20
```
The reflog only recovers commits, not uncommitted changes.
