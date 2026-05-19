# Meta Destructive Command Safety

Multi-repo workspaces amplify the impact of destructive commands.

## Before Destructive Operations

1. **Create a snapshot first**
   ```
   meta git snapshot create before-refactor
   ```

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
- `git reset --hard` (use `meta git snapshot` instead)
- `git clean -fd` (dangerous file removal)
- `rm -rf` on repo roots or `.meta*` paths

## Recovery

If something goes wrong:
```
meta git snapshot restore before-refactor
```
