---
name: meta-git
description: Git operations across multiple repositories — status, commit, push, pull, multi-commit.
---

# Meta Git Skill

Git operations across multiple repositories. One command operates on the root repo and every project in this meta level; add `-r` (`meta -r git status`) to include the meta repos nested under it, or `-A` for the whole workspace from anywhere.

In the commands below, `<repo>` is a repo's `.meta.yaml` name or its path from the nearest `.meta.yaml` (with `-r` or `-A`, from the workspace root: a repo of a nested meta repo needs one of them), and `<tag>` one of its tags; `meta -A --json project list` lists both. Other repo and tag names (`api`, `backend`) are examples.

**The primary checkout is pull-only.** Commit, stage, push and batch-edit inside a worktree set (the `meta-worktree` skill), where these commands act on the set's repos only. In the primary checkout, read, or run `meta -r git pull --ff-only` and `meta -r git update`.

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
# Clone meta repo and all child repos defined in .meta.yaml
meta git clone <url>

# Clone recursively (if child repos are also meta repos)
meta -r git clone <url>

# Control parallelism
meta git clone <url> --parallel 8

# Shallow clone
meta git clone <url> --depth 1
```

**How it works**: Meta clones the parent, reads `.meta.yaml`, then queues and clones all children in parallel. With meta's `-r`, it repeats for any child that has its own `.meta.yaml`.

## Updating All Repos

```bash
meta git pull --ff-only   # Fast-forward every cloned repo
meta git update           # Clone the missing ones; pulls nothing
```

| Command | What it does |
|---------|--------------|
| `meta git update` | Clone the projects the `.meta.yaml` files on disk list that aren't cloned; change no repo that is, and pull nothing |
| `meta git update --prune` | The same, then remove the repos no `.meta.yaml` lists that hold nothing unpushed; keep and name the others |
| `meta git update --prune-only` | Only remove them; clone nothing |

`update` reads the `.meta.yaml` files on disk and pulls nothing, the workspace repo included. `--prune` and `--prune-only` keep any repo that holds uncommitted or untracked files, stashes, commits or tags on no remote, linked worktrees or repos inside it, and says what it holds. Files git ignores (`.env`) are deleted with a removed repo, and named first; preview with `meta --dry-run git update --prune`. A project that isn't cloned yet is a warning in `meta git pull` and the other per-repo commands, not a failure.

To bring a workspace up to date, pull first, then clone what the pulled `.meta.yaml` files add: `meta -r git pull --ff-only`, then `meta -r git update`. A repo git can't fast-forward fails the pull; nothing is merged or rebased.

## Before Batch Operations (Critical)

Inside your set, before making sweeping changes, commit uncommitted work in every repo:

```bash
# See which repos have uncommitted changes
meta git status

# Commit them (`meta git commit` commits only staged changes; with nothing staged it says so and exits 0)
meta git add -A
meta git commit -m "wip: before refactor"

# Preview a batch command before running it
meta --dry-run exec -- <command>
```

Commit rather than stash: every worktree of a repo shares one stash, so another session's `git stash pop` can take yours. If you must stash, name it (`meta exec -- git stash push -u -m before-refactor`), find its commit with `git stash list --format='%H %gs'` in each repo, and restore that with `git stash apply --index <sha>` (`--index` keeps what was staged staged).

**Recovering**: run `git reflog` inside the set, in the affected repo: it lists the commits that worktree's `HEAD` pointed to, and `git switch -c recovered <sha>` brings one back. From the primary checkout you won't see them, and a set's reflog goes with the set, so recover before you remove it. `meta exec -- git reflog -n 20`, run in the set, looks across its repos (`meta --include <repo> exec -- …` for one). The reflog only recovers commits, not uncommitted changes. A backup branch keeps only commits.

**Undoing a commit**, in the set, one repo at a time with `--include`:

```bash
meta --include <repo> exec -- git reset --soft HEAD~1   # not pushed: the changes come back staged
meta --include <repo> exec -- git revert --no-edit HEAD # pushed: a new commit undoes it
```

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

Target specific repos, with meta's options before `git`. `--tag` selects the tagged repos, never the meta root (add it with `--include .`); `--include`/`--exclude` take whole repos (a `.meta.yaml` name or the path from the nearest `.meta.yaml`, with `-r` or `-A` from the workspace root; `.` for that root), never part of a path. A value or tag that names nothing is an error.

```bash
# By tag (-r: the tags of nested meta repos' projects too)
meta -r --tag <tag> git status

# By repo: .meta.yaml name or path from the root (`api` does not match `api-tests`)
meta --include <repo>,<repo> git push

# Exclude repos
meta --exclude <repo> git pull
```

## Workflow Patterns

### Starting Work
```bash
meta -r git pull --ff-only && meta -r git update  # In the primary checkout: get latest
meta git worktree create my-task --repo <repo>  # A set for this task (-r or -A for nested repos)
cd .worktrees/my-task
meta git status                                 # What's the current state?
```

### After Making Changes
```bash
meta git status                                 # In the set: review changes across repos
meta git add -A                                 # Stage in all repos
meta git commit -m "feat: ..."                  # Commit with shared message
meta --include <repo> git push -u origin HEAD   # Push the repos you changed
```

### Safe Batch Refactoring
```bash
meta git add -A && meta git commit -m "wip: before changes"  # In the set: save pending work first
# ... make changes across repos ...
meta git status              # Review
# If something went wrong, in the affected repo of the set:
git reflog                   # Find an earlier commit, then: git switch -c recovered <sha>
```

## MCP Tools for Git Operations

The meta MCP server (`meta-mcp`; see the `meta-workspace` skill for how to start it) gives structured JSON. Every tool has a CLI equivalent:

| Tool | Purpose | Without MCP |
|------|---------|-------------|
| `meta_git_multi_commit` | Per-repo commit messages in one call (for audit trails when changes differ) | `meta -r --include <repo> git commit -m "…"`, once per repo (the tool itself takes only this level's repos) |
| `meta_git_status` | Each repo's `git status` text, the root and nested repos included | `meta -r --json git status` |
| `meta_git_diff` | Structured diff output, this level's repos only | `meta git diff` |
| `meta_git_branch` | Branch info of the root and every nested repo | `meta -r exec -- git status -sb` |

## Efficiency Tips

- **Targeted commits**: `meta --include <repo>,<repo> git commit -m "msg"` commits only in the named repos
- **Tag-based push**: `meta -r --tag <tag> git push` pushes the tagged repos only
- **Per-repo messages**: Use `meta_git_multi_commit` when changes in different repos need different commit messages
- **One-call status**: `meta git status` replaces N individual `cd && git status` sequences
