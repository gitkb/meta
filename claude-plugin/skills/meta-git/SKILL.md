---
name: meta-git
description: Git operations across multiple repositories — status, commit, push, pull, multi-commit.
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

Any other git subcommand passes through to git in each repo. Meta passes the arguments it receives to git unchanged, with no shell in between: `meta git log --format='%h %s'` and a commit message containing `$(...)` or `;` are passed as-is. Your own shell still expands `$(...)` inside double quotes before meta runs, so put untrusted text in single quotes.

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

`meta git update` only clones missing repos (nested meta repos included); it does not pull existing ones. `meta git pull --recursive` also pulls nested meta repos.

## Before Batch Operations (Critical)

Before making sweeping changes, commit your work or create a backup branch in every repo:

```bash
# See which repos have uncommitted changes
meta git status

# Commit them, or create a backup branch in every repo at this meta level (add --recursive for nested ones)
meta git commit -m "wip: before refactor"
meta exec -- git branch backup/before-refactor

# Preview a batch command before running it
meta --dry-run exec -- <command>
```

**Recovering**: `git reflog` in the affected repo lists the commits its `HEAD` pointed to; `git switch -c recovered <sha>` brings one back. Use `meta --include <repo> exec -- git reflog -n 20` to look across repos. The reflog only recovers commits, not uncommitted changes, so commit or branch first.

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

Target specific repos. With `--tag`, the meta root repo stays included even when it isn't tagged; `--include`/`--exclude` take whole repos (a `.meta` name or the path from the workspace root, `.` for the root), never part of a path, and can drop the root. A value that names no repo is an error.

```bash
# By tag
meta --tag backend git status

# By repo: .meta name or path from the root (`api` does not match `api-tests`)
meta --include api,web git push

# Exclude repos
meta --exclude legacy git pull
```

## Workflow Patterns

### Starting Work
```bash
meta git status              # What's the current state?
meta git commit -m "wip"     # Commit pending work before changes
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
meta exec -- git branch backup/before-changes
# ... make changes across repos ...
meta git status              # Review
# If something went wrong, in the affected repo:
git reflog                   # Find an earlier commit, then: git switch -c recovered <sha>
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

- **Targeted commits**: `meta --include repo1,repo2 git commit -m "msg"` commits only in the named repos
- **Tag-based push**: `meta --tag backend git push` pushes tagged repos (and the root)
- **Per-repo messages**: Use `meta_git_multi_commit` when changes in different repos need different commit messages
- **One-call status**: `meta git status` replaces N individual `cd && git status` sequences
