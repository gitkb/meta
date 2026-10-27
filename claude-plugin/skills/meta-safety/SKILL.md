---
name: meta-safety
description: Multi-repo safety discipline — precision operations, dependency checking, and efficiency tips.
---

# Meta Safety Skill

Multi-repo workspaces are powerful but require awareness. Meta gives you precision tools to operate on only what you need, saving turns and avoiding unintended changes.

## Session Start

1. `meta project list --recursive --json` — workspace map, including nested meta repos, in one call
2. `meta git status` — see state of every repo at once (`meta --recursive git status` for nested ones)
3. Note which repos provide shared dependencies (check `.meta.yaml` for `provides`/`depends_on`)

## Precision Operations

Instead of cd-ing into repos one by one, use meta flags to target what you need. Include/exclude take whole repos (a `.meta` name or the path from the workspace root), never part of a path, and a value that names no repo is an error; `--tag` keeps the meta root even when it isn't tagged:

```bash
# Target specific repos
meta --include repo1,repo2 exec -- command

# Target by tag
meta --tag backend exec -- cargo test

# Exclude repos
meta --exclude legacy exec -- npm update

# Combine: tagged repos, excluding one, one at a time
meta --tag backend --exclude legacy --sequential exec -- make deploy
```

## Before Modifying Shared Code

When modifying a repo that other repos depend on:

1. **Check dependents**: Use `meta_analyze_impact <repo-name>` (MCP tool) to see transitive dependents
2. **Plan cascading changes**: If `meta_core` changes, repos that depend on it may need updates
3. **Build in order**: there is no CLI flag for dependency order; get it from the MCP tool `meta_execution_order` and build the repos one at a time in that order

## Efficient Commits

```bash
# Commit only in repos repo1 and repo2 (.meta name or path from the root)
meta --include repo1,repo2 git commit -m "feat: update shared API"

# Push tagged repos (and the root)
meta --tag backend git push

# Per-repo commit messages (when changes differ)
# Use meta_git_multi_commit MCP tool
```

## Query DSL (MCP)

The `meta_query_repos` MCP tool filters repos by state:

| Query | Result |
|-------|--------|
| `dirty:true` | Repos with uncommitted changes |
| `tag:backend` | Repos tagged "backend" |
| `dirty:true AND tag:backend` | Combine filters |
| `branch:feature-x` | Repos on a specific branch |

## Global Strict Mode

Use `meta --strict` to turn worktree warnings into errors for CI/automation. It applies to the `meta git worktree` checks listed below, not to `meta exec`. It is not a transaction: repos already changed before the error stay changed.

```bash
meta --strict git worktree create feature-test v2.0.0 --all
meta --strict git worktree prune
```

**What becomes an error in strict mode:**
- Missing refs when using a commit-ish (worktree create)
- The root repo skipped because a commit-ish is missing there (worktree create)
- Invalid `--meta` format values (worktree create)
- Failed directory removal (worktree prune)
- Any set prune keeps: one holding work, a worktree meta has no record of, a worktree from another workspace, or a record it can't read (worktree prune)

**When to use --strict:**
- CI pipelines testing a specific release tag across all repos
- Automated scripts that require consistent state
- Debugging scenarios where partial state would be misleading
- Any context where silent warnings could mask failures

**When NOT to use --strict:**
- Interactive development (warnings are usually sufficient)
- Working with repos that legitimately don't have a ref
- Exploratory work where partial context is acceptable

### Ephemeral Cleanup Behavior

`meta git worktree exec --ephemeral` creates and cleans up in best-effort mode (`strict=false`), even if you specified `--strict`. Cleanup only runs if creation succeeded. This ensures:

- The command result (success/failure) reflects the actual executed command, not cleanup issues
- Cleanup failures are logged as warnings, not errors
- Partial cleanup succeeds even if some repos have open file handles or permissions issues

If you need to ensure cleanup completed fully, follow up with `meta git worktree list` to verify.

### Worktree-specific --strict

`meta git worktree create` also has a local `--strict` flag:

```bash
# Local --strict on worktree create only
meta git worktree create feature-test v2.0.0 --all --strict
```

Both flags work together - if either global `--strict` or local `--strict` is set, strict mode is enabled.

## Efficiency Tips

- One `meta git status` replaces N individual `git status` calls
- One `meta --tag X exec -- cmd` replaces N `cd && cmd` sequences
- `meta_analyze_impact` before modifying providers prevents cascading fix-up commits
- `meta --dry-run exec -- dangerous-cmd` previews before executing
