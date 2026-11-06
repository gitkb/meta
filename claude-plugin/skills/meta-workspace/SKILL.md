---
name: meta-workspace
description: Understanding and operating in meta repository workspaces with .meta.yaml configuration.
---

# Meta Workspace Skill

This project is a **meta repository** - a parent repo that manages a graph of child repositories. Understanding this structure is essential for effective operation.

## Starting a Session

When you begin working in a meta repo, discover its structure:

```bash
meta -A --json project list
```

This returns repos, paths, and tags. Use this to:
- Know which repos exist before making changes
- Identify available tags for filtering operations
- Understand the project graph

Run `meta git status` to see the current state of every repo at this level (`meta -r git status`: the meta repos nested below it too; `meta -A git status`: the whole workspace, from anywhere).

## Why This Matters for You (Claude)

In a meta repo, changes often span multiple repositories. Using `meta` commands instead of plain `git` lets you:
- **Operate on every repo with one command** - fewer tool calls, less context
- **Maintain consistency** - same commit message, same branch across repos
- **Never miss a repo** - meta tracks them all

## The Meta Config

One config file in the workspace root, `.meta.yaml`, defines the child
repositories.

```yaml
projects:
  api: git@github.com:org/api.git
  web:
    repo: git@github.com:org/web.git
    path: frontend/web
    tags: [frontend, typescript]
```

**Simple format**: `name: git-url` - clones to `./name`
**Extended format**: a mapping; every key is optional. The fields of the config:

<!-- generated:config-fields -->
| Field | Meaning |
|-------|---------|
| `projects` | The projects, by name: each a repo URL, or a mapping of the `projects.<name>` fields |
| `defaults` | Settings for every command |
| `worktrees_dir` | Where worktree sets go, instead of `.worktrees` |
| `worktree` | Settings for worktree sets |
| `projects.<name>.path` | Where the project lives (defaults to its name) |
| `projects.<name>.repo` | Git repository URL (without one the project is local-only: `meta git update` doesn't clone it) |
| `projects.<name>.tags` | Array of tags for filtering |
| `projects.<name>.provides` | Array of capabilities this project provides |
| `projects.<name>.depends_on` | Array of project names or provided capabilities this depends on |
| `projects.<name>.meta` | Set `true` if this project contains a nested `.meta.yaml` |
| `defaults.parallel` | Run commands in parallel (`true` unless set) |
| `worktree.hooks` | The shell command to run at each worktree event, by event: `post-create`, `post-destroy`, `post-prune` |
<!-- /generated -->

## Discovering What's Here

```bash
# List all projects in this workspace
meta project list

# See git status across every repo at once
meta git status
```

## Filtering by Tag

When projects have tags, filter operations:

```bash
# Frontend repos (not the root: it has no tags; add it with --include .;
# -r: the tags of nested meta repos' projects too)
meta -r --tag frontend git status

# Multiple tags (comma-separated)
meta -r --tag backend,api exec -- make test

# Exclude specific repos
meta --exclude legacy-service git pull
```

## Nested Meta Repos

Meta repos can contain other meta repos. meta reaches them at three levels, chosen before the command: no option, the nearest `.meta.yaml`; `-r`, that one and the meta repos nested under it; `-A`, the whole workspace from its outermost `.meta.yaml`, wherever you are (at the root, `-r` and `-A` are the same). With `-r` or `-A`, repos are named by their path from the workspace root.

```bash
# Clone a meta repo and ALL nested meta repos
meta -r git clone <url>

# Status across the meta repos nested below here
meta -r git status

# Status across the whole workspace, from anywhere
meta -A git status
```

meta's `--depth N`, before the command, limits how deep into nested meta repos it goes (`meta -r --depth 1 git clone <url>`); after `git clone`, `--depth N` is git's history depth.

## MCP Tools for Workspace Discovery

When the meta MCP server is available, these tools provide structured JSON for programmatic workspace operations:

| Tool | Purpose |
|------|---------|
| `meta_workspace_state` | Full workspace state in one call (projects, branches, dirty status) |
| `meta_query_repos` | Filter repos by state: `dirty:true`, `tag:backend`, `branch:main` |
| `meta_analyze_impact` | Check transitive dependents before modifying a repo |
| `meta_execution_order` | Get topological build/deploy order respecting dependency graph |

## Efficiency Tips

- **One command, all repos**: `meta git status` replaces N individual `cd && git status` calls
- **Targeted operations**: `meta --include repo1,repo2 exec -- cmd` operates on only the named repos (`.meta.yaml` name or path from the workspace root)
- **Tag-based filtering**: `meta -r --tag backend exec -- cargo test` scopes to tagged repos
- **Dependency awareness**: `meta_analyze_impact <repo>` shows who depends on a repo before you modify it — prevents cascading fix-up commits
- **Session start**: Always run `meta -A --json project list` first to get the workspace map
