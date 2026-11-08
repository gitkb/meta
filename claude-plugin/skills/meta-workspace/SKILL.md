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

For what depends on what, read `depends_on` and `provides` in `.meta.yaml`, or ask `meta -r project dependents <repo>`.

Run `meta git status` to see the current state of every repo at this level (`meta -r git status`: the meta repos nested below it too; `meta -A git status`: the whole workspace, from anywhere).

Keep `.meta/skills`, `.claude` and `.agents` committed in the root repo (`meta init claude` writes them, untracked), or a worktree set made from `origin/main` won't have this guidance.

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
**Extended format**: a mapping; every key is optional (`repo`, `path`, `tags`, `provides`, `depends_on`, `meta: true` for a nested meta repo)

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

The meta MCP server gives structured JSON. Start it with `meta-mcp`, which speaks MCP over stdio for the workspace or set it starts in, or install the Claude Code plugin, which registers it. To register it yourself, in `.mcp.json`:

```json
{"mcpServers": {"meta": {"command": "meta-mcp", "args": []}}}
```

The tools below, `meta_git_status` and `meta_git_branch` see what `meta -r` sees: the root and nested repos, and inside a set that set's members only. The exceptions: `meta_git_diff` and `meta_execution_order` see this level's repos only, and in a set `meta_analyze_impact` also names projects the config lists that the set doesn't hold.

| Tool | Purpose | Without MCP |
|------|---------|-------------|
| `meta_workspace_state` | Counts: repos, dirty, clean, ahead, behind, and repos per branch, tag and build system | `meta -r git status` |
| `meta_query_repos` | Filter repos by state (`query`: `dirty:true`, `tag:backend`, `branch:main`) | `meta -r --json context` (no row for the root's git state: add `meta -r git status`) |
| `meta_analyze_impact` | Direct and transitive dependents of a repo (`project`: name, or nested path) | `meta -r project dependents <repo>` (direct only) |
| `meta_execution_order` | Build/deploy order of this level's repos, from their `depends_on` | none: read `depends_on` in `.meta.yaml` |

## Efficiency Tips

- **One command, all repos**: `meta git status` replaces N individual `cd && git status` calls
- **Targeted operations**: `meta --include repo1,repo2 exec -- cmd` operates on only the named repos (`.meta.yaml` name or path from the workspace root)
- **Tag-based filtering**: `meta -r --tag backend exec -- cargo test` scopes to tagged repos
- **Dependency awareness**: `meta -r project dependents <repo>` (or the MCP tool `meta_analyze_impact`) shows who depends on a repo before you modify it — prevents cascading fix-up commits
- **Session start**: Always run `meta -A --json project list` first to get the workspace map
