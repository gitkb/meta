---
name: meta-git
description: Git operations across multiple repositories — status, commit, push, pull, snapshots, multi-commit.
---

# Meta Git Skill

Git operations across multiple repositories. One command operates on the root repo and every project in this meta level; add `--recursive` (`meta --recursive git status`) to include nested meta repos.

## The Core Insight

**Use `meta git` instead of `git`** when you want to operate across the workspace:

```bash
# Instead of running git status in each repo...
meta git status    # Shows status for every repo at once

# Instead of committing in each repo...
meta git commit -m "feat: update"    # Commits in every repo with staged changes
```

## Cloning a Meta Repo

```bash
# Clone meta repo and all child repos defined in .meta
meta git clone <url>

# Clone recursively (if child repos are also meta repos)
meta git clone <url> --recursive

# Control parallelism
meta git clone <url> --parallel 8

# Shallow clone
meta git clone <url> --depth 1
```

**How it works**: Meta clones the parent, reads `.meta`, then queues and clones all children in parallel. With `--recursive`, it repeats for any child that has its own `.meta`.

## Updating All Repos

```bash
meta git pull      # Pull every existing repo
meta git update    # Clone repos listed in .meta that are missing locally
```

`meta git update` only clones missing repos; it does not pull existing ones. Add `--recursive` to either for nested meta repos.

## Snapshots (Critical for Batch Operations)

Before making sweeping changes, create a snapshot:

```bash
# Save current state of ALL repos
meta git snapshot create before-refactor

# See what snapshots exist
meta git snapshot list

# Preview what restore would do
meta --dry-run git snapshot restore before-refactor

# Actually restore (auto-stashes uncommitted tracked changes)
meta git snapshot restore before-refactor
```

**What snapshots capture per repo:**
- Current SHA
- Branch name
- Dirty status

**On restore**: If a repo has uncommitted changes to tracked files, meta stashes them before checking out the snapshot state. Untracked files are not stashed and can block the checkout.

## Common Git Operations

All standard git commands work:

```bash
meta git pull
meta git push
meta git fetch
meta git checkout -b feature/new-thing
meta git add .
meta git diff
meta git log --oneline -5
```

## Filtering

Target specific repos. With `--tag`, the meta root repo stays included even when it isn't tagged; `--include`/`--exclude` match substrings of directory paths and can drop the root.

```bash
# By tag
meta --tag backend git status

# By directory (substring match: `api` also matches `api-tests`)
meta --include api,web git push

# Exclude repos
meta --exclude legacy git pull
```

## Workflow Patterns

### Starting Work
```bash
meta git status              # What's the current state?
meta git snapshot create wip # Save state before changes
meta git pull                # Get latest
```

### After Making Changes
```bash
meta git status              # Review changes across repos
meta git add .               # Stage in all repos
meta git commit -m "feat: ..." # Commit with shared message
meta git push                # Push all repos
```

### Safe Batch Refactoring
```bash
meta git snapshot create before-changes
# ... make changes across repos ...
meta git status              # Review
# If something went wrong:
meta git snapshot restore before-changes
```

## MCP Tools for Git Operations

When the meta MCP server is available, these tools provide structured JSON output:

| Tool | Purpose |
|------|---------|
| `meta_git_multi_commit` | Per-repo commit messages in one call (for audit trails when changes differ) |
| `meta_git_status` | Structured git status across all repos |
| `meta_git_diff` | Structured diff output |
| `meta_git_branch` | Branch info across repos |

## Efficiency Tips

- **Targeted commits**: `meta --include repo1,repo2 git commit -m "msg"` commits only in repos whose path matches
- **Tag-based push**: `meta --tag backend git push` pushes tagged repos (and the root)
- **Per-repo messages**: Use `meta_git_multi_commit` when changes in different repos need different commit messages
- **One-call status**: `meta git status` replaces N individual `cd && git status` sequences
