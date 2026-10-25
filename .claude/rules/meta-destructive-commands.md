# Meta Destructive Command Safety

Multi-repo workspaces amplify the impact of destructive commands.

## Before Destructive Operations

1. **Commit or branch first**
   ```
   meta git status
   meta exec -- git branch backup/before-refactor
   ```
   Commit uncommitted work, or create a backup branch in every repo.

2. **Preview with --dry-run**
   ```
   meta --dry-run exec -- rm -rf node_modules
   ```

3. **Target precisely** — avoid blanket operations
   ```
   meta --include api-service exec -- git reset --hard
   ```

## Blocked by `agent guard`

These commands trigger PreToolUse denial:
- `git push --force` (use `--force-with-lease` instead)
- `git reset --hard` (commit or branch first; recover commits with `git reflog`)
- `git clean -fd` (dangerous file removal)
- `rm -rf` on repo roots or `.meta*` paths

## Recovery

If something goes wrong, run `git reflog` in the affected repo to find the previous commit, then restore it on a branch:
```
git switch -c recovered <sha>
```
To look across repos:
```
meta --include <repo> exec -- git reflog -n 20
```
The reflog only recovers commits, not uncommitted changes.
