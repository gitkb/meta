---
name: meta-workspace
description: Understanding and operating in meta repository workspaces with .meta/.meta.yaml configuration.
---

# Meta Workspace Skill

This project is a **meta repository** - a parent repo that manages a graph of child repositories. Understanding this structure is essential for effective operation.

## Starting a Session

When you begin working in a meta repo, discover its structure:

```bash
meta project list --recursive --json
```

This returns repos, paths, and tags. Use this to:
- Know which repos exist before making changes
- Identify available tags for filtering operations
- Understand the project graph

Run `meta git status` to see the current state of every repo at this level (`meta --recursive git status` for nested meta repos).

## Why This Matters for You (Claude)

In a meta repo, changes often span multiple repositories. Using `meta` commands instead of plain `git` lets you:
- **Operate on every repo with one command** - fewer tool calls, less context
- **Maintain consistency** - same commit message, same branch across repos
- **Never miss a repo** - meta tracks them all

## The .meta File

The `.meta` file defines child repositories:

```json
{
  "projects": {
    "api": "git@github.com:org/api.git",
    "web": {
      "repo": "git@github.com:org/web.git",
      "path": "./frontend/web",
      "tags": ["frontend", "typescript"]
    }
  }
}
```

**Simple format**: `"name": "git-url"` - clones to `./name`
**Extended format**: Object with `repo`, optional `path`, optional `tags`

`.meta.json`, `.meta.yaml` and `.meta.yml` are also supported.

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
# Frontend repos (the meta root stays included)
meta --tag frontend git status

# Multiple tags (comma-separated)
meta --tag backend,api exec -- make test

# Exclude specific repos
meta --exclude legacy-service git pull
```

## Nested Meta Repos

Meta repos can contain other meta repos. Use `--recursive` to operate on the entire graph:

```bash
# Clone a meta repo and ALL nested meta repos
meta git clone <url> --recursive

# Status across entire graph
meta --recursive git status
```

The global `--depth N` flag limits recursion depth. For `meta git clone`, `--depth` is the shallow-clone depth; use `--meta-depth N` there.

## Key Commands Quick Reference

| Command | What It Does |
|---------|--------------|
| `meta git status` | Git status in every repo |
| `meta git clone <url>` | Clone meta repo + all children |
| `meta exec -- <cmd>` | Run command in every repo |
| `meta project list` | List all child projects |
| `meta git worktree create <task> --repo <alias>` | Isolated worktree set for a change (see `meta-worktree`) |
| `meta init claude` | Install these skills |

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
- **Targeted operations**: `meta --include repo1,repo2 exec -- cmd` operates on only the named repos (`.meta` name or path from the workspace root)
- **Tag-based filtering**: `meta --tag backend exec -- cargo test` scopes to tagged repos
- **Dependency awareness**: `meta_analyze_impact <repo>` shows who depends on a repo before you modify it — prevents cascading fix-up commits
- **Session start**: Always run `meta project list --recursive --json` first to get the workspace map
