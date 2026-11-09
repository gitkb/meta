---
name: meta-safety
description: Multi-repo safety discipline — precision operations, dependency checking, and efficiency tips.
---

# Meta Safety Skill

Multi-repo workspaces are powerful but require awareness. Meta gives you precision tools to operate on only what you need, saving turns and avoiding unintended changes. A destructive command reaches every repo meta selects, so the primary checkout is pull-only: change things, and run anything below, inside a worktree set (the `meta-worktree` skill).

In the commands below, `<repo>` is a repo's `.meta.yaml` name or its path from the nearest `.meta.yaml` (with `-r` or `-A`, from the workspace root: a repo of a nested meta repo needs one of them), and `<tag>` one of its tags; `meta -A --json project list` lists both. Other names (`api`, `legacy`) are examples.

## Session Start

1. `meta -A --json project list` — the whole workspace map, nested meta repos included, from wherever you are, in one call
2. `meta git status` — see state of every repo at once (`meta -r git status`: the meta repos nested below here too; `meta -A git status`: the whole workspace)
3. Note which repos provide shared dependencies (check `.meta.yaml` for `provides`/`depends_on`)

## Precision Operations

Instead of cd-ing into repos one by one, use meta flags to target what you need. Include/exclude take whole repos (a `.meta.yaml` name or the path from the nearest `.meta.yaml`, with `-r` or `-A` from the workspace root), never part of a path, and a value that names no repo is an error; `--tag` selects only the tagged repos, never the meta root (`--include .` adds it):

```bash
# Target specific repos
meta --include <repo>,<repo> exec -- <command>

# Target by tag (-r: the tags of nested meta repos' projects too)
meta -r --tag <tag> exec -- cargo test

# Exclude repos
meta --exclude <repo> exec -- npm update

# Combine: tagged repos, excluding one, one at a time
meta -r --tag <tag> --exclude <repo> --sequential exec -- make deploy
```

## Before Destructive Operations

1. **Commit first**, inside your set:
   ```bash
   meta git status
   meta git add -A && meta git commit -m "wip: before refactor"
   ```
   `meta git commit` commits only what is staged. Commit rather than stash: every worktree of a repo shares one stash, so another session's `git stash pop` can take yours. A backup branch (`git branch backup/<name>`) keeps only commits, not uncommitted changes.
2. **Preview with --dry-run**: `meta --dry-run exec -- rm -rf node_modules`
3. **Target precisely**, not every repo: `meta --include <repo> exec -- cargo clean`
4. **Know what can't be undone**:
   - Push with `--force-with-lease`, not `--force`: it refuses to overwrite commits your last fetch didn't bring. A background fetch can defeat that, so look at the remote branch first.
   - Commit before `git reset --hard`: it discards uncommitted work for good.
   - `git clean -fd` removes untracked files for good; `git clean -nd` previews it.

Where gitkb's `agent guard` runs as a hook (`meta init skills` adds it to Claude Code's settings when gitkb's agent CLI is installed), it refuses these by default (a project can change them). It is a safety net, not a lock: when the agent fails, the command runs.

- `git push` with `--force` or `-f`, unless the line has `--force-with-lease`
- `git reset --hard`
- `git clean` with both `-f` and `-d`
- `git checkout .` and `git checkout -- .`
- `git branch -D`
- `git stash drop` and `git stash clear`
- `rm -rf` on `.`, `..`, `/`, `~`, `$HOME`, `.meta`, `.meta.yaml`, `.meta.yml`, `*`, `./*`, `../*` or `./`

## Recovery

Run `git reflog` inside the set, in the affected repo, to find the previous commit, then restore it on a branch there: `git switch -c recovered <sha>`. A set's commits are in that worktree's reflog, not the primary checkout's, and the reflog goes with the set: recover before removing it. To look across the set's repos, from inside it: `meta exec -- git reflog -n 20`. The reflog only recovers commits, not uncommitted changes.

To undo a commit, in the set, one repo at a time:

```bash
meta --include <repo> exec -- git reset --soft HEAD~1
meta --include <repo> exec -- git revert --no-edit HEAD
```

The first is for a commit not pushed (its changes come back staged), the second for one already pushed (a new commit undoes it).

## Before Modifying Shared Code

When modifying a repo that other repos depend on:

1. **Check dependents**: `meta -r project dependents <repo>` lists what depends on it directly; the MCP tool `meta_analyze_impact` (argument `project`) adds transitive dependents
2. **Plan cascading changes**: when a repo changes, the repos that depend on it may need updates
3. **Build in order**: there is no CLI flag for dependency order; get it from the MCP tool `meta_execution_order` (see the `meta-workspace` skill for starting the server), or read `depends_on` in `.meta.yaml`, and build the repos one at a time in that order

## Efficient Commits

```bash
# In a worktree set: commit only in these repos
meta --include <repo>,<repo> git commit -m "feat: update shared API"

# Push the tagged repos (not the root; add it with --include .)
meta -r --tag <tag> git push

# Per-repo commit messages (when changes differ)
# Use the meta_git_multi_commit MCP tool (this level's repos only), or `meta -r --include <repo> git commit -m "…"` once per repo
```

## Query DSL (MCP)

The `meta_query_repos` MCP tool (argument `query`) filters repos by state, the root and nested repos included. Without MCP: `meta -r --json context` has no row for the root's git state, so add `meta -r git status` (in a set with no root checkout, `context` lists the workspace's repos, not the set's: use `meta git status` there).

| Query | Result |
|-------|--------|
| `dirty:true` | Repos with uncommitted changes |
| `tag:<tag>` | Repos with that tag |
| `dirty:true AND tag:<tag>` | Combine filters |
| `branch:<branch>` | Repos on a specific branch |

## Global Strict Mode

Use `meta --strict` to turn worktree warnings into errors for CI/automation. It applies to the `meta git worktree` checks listed below (and `meta --strict git worktree list` and `status` fail on what they can't read), and to the per-repo `meta git` commands (`pull`, `status`, …), where a project that isn't cloned fails the command; not to `meta exec`. It is not a transaction: repos already changed before the error stay changed.

```bash
meta --strict git worktree create <set> <commit-ish> --every-repo
meta --strict git worktree prune
```

**What becomes an error in strict mode:**
- Missing refs when using a commit-ish (worktree create)
- The root repo skipped because a commit-ish is missing there (worktree create)
- Invalid `--meta` format values (worktree create)
- A `depends_on` of the set that names no project, or that two projects provide (worktree create)
- A failing hook (worktree create, remove and prune)
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

For `meta git worktree create`, meta's `--strict` goes before the command like every meta option: `meta --strict git worktree create <set> <commit-ish> --every-repo`.

### Running a Command in a Short-Lived Set

```bash
meta git worktree create tmp --repo <repo> --branch <branch>
meta git worktree exec tmp -- cargo test
meta git worktree remove tmp
```

## Efficiency Tips

- One `meta git status` replaces N individual `git status` calls
- One `meta -r --tag <tag> exec -- <command>` replaces N `cd && <command>` sequences
- `meta -r project dependents <repo>` before modifying providers prevents cascading fix-up commits
- `meta --dry-run exec -- <command>` previews before executing
