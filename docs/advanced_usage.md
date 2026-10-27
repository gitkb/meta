# Advanced Usage Guide

This guide covers advanced features of the `meta` CLI, including query DSL, safe batch changes, dependency tracking, and power-user workflows.

## Table of Contents

- [Query DSL](#query-dsl)
- [Safe Batch Changes](#safe-batch-changes)
- [Dependency Tracking](#dependency-tracking)
- [Filtering Commands](#filtering-commands)
- [Scripting with Meta](#scripting-with-meta)
- [JSON Output Mode](#json-output-mode)
- [Parallel Execution](#parallel-execution)
- [Environment Variables](#environment-variables)
- [Advanced Plugin Usage](#advanced-plugin-usage)
- [See Also](#see-also)

---

## Query DSL

Meta includes a powerful query language for filtering repositories by their git state.

### Query Syntax

```bash
meta query "<condition>"
meta query "<condition> AND <condition>"
```

### Available Conditions

| Condition | Description | Example |
|-----------|-------------|---------|
| `dirty:true/false` | Has uncommitted changes | `dirty:true` |
| `branch:<name>` | On specific branch | `branch:main` |
| `tag:<name>` | Has specific tag | `tag:backend` |
| `ahead:true/false` | Ahead of remote | `ahead:true` |
| `behind:true/false` | Behind remote | `behind:true` |
| `modified_in:<time>` | Modified within timeframe | `modified_in:24h` |
| `language:<lang>` | Uses specific language/build system | `language:rust` |

### Combining Conditions

Use `AND` to combine multiple conditions:

```bash
# Find dirty backend repos on main branch
meta query "dirty:true AND tag:backend AND branch:main"

# Find repos that need pulling
meta query "behind:true AND branch:main"

# Find recently modified frontend repos
meta query "modified_in:24h AND tag:frontend"
```

### Use Cases

**Pre-commit checks:**
```bash
# See all repos with uncommitted changes
meta query "dirty:true"
```

**Release preparation:**
```bash
# Ensure all repos are on main and clean
meta query "branch:main AND dirty:false"
```

**Sync assessment:**
```bash
# Find repos that need attention
meta query "ahead:true" && meta query "behind:true"
```

---

## Safe Batch Changes

Before changing many repositories at once, commit or stash uncommitted work in every repo, so you can get back to it with plain git. A backup branch alone keeps only commits, not uncommitted changes.

### Before Changing Anything

```bash
# See which repos have uncommitted changes
meta git status

# Commit them (`meta git commit` commits only staged changes)
meta git add -A
meta git commit -m "wip: before refactor"

# Or stash them in every repo (untracked files included)
meta exec -- git stash push -u -m before-refactor
```

### Preview and Target

```bash
# Preview what would run, without running it
meta --dry-run exec -- npm update

# Limit the change to specific repos
meta --include api-service,web-app exec -- npm update
```

### Safe Batch Workflow

```bash
# 1. Commit pending work before the risky operation
meta git add -A && meta git commit -m "wip: before changes"

# 2. Make changes across repos
meta exec -- npm update

# 3. Test changes
meta exec -- npm test

# 4. If tests fail, discard the batch's changes in the affected repos.
#    Everything from before was committed in step 1, so this only drops
#    what the batch changed: git restore --staged --worktree . (tracked
#    files, staged or not) and git clean -nd, then -fd, for new files

# 5. If tests pass, stage and commit
meta git add -A && meta git commit -m "chore: update dependencies"
```

### Recovering Committed Work

`git reflog` lists every commit a repo's `HEAD` pointed to, so you can find and restore an earlier state:

```bash
# Look across all repos
meta exec -- git reflog -n 20

# In the affected repo, branch from the earlier commit
git switch -c recovered <sha>
```

The reflog only recovers commits. Uncommitted changes that were discarded are gone, so commit or stash first.

---

## Dependency Tracking

For complex workspaces, meta supports dependency declarations for impact analysis and execution ordering.

### Configuration

Extend your `.meta.yaml` with `provides` and `depends_on`:

```yaml
projects:
  shared-utils:
    repo: git@github.com:org/shared-utils.git
    provides: [utils-api, logger-api]
    depends_on: []

  auth-service:
    repo: git@github.com:org/auth-service.git
    provides: [auth-api]
    depends_on:
      - shared-utils

  api-service:
    repo: git@github.com:org/api-service.git
    depends_on:
      - auth-api      # Reference by provided name
      - utils-api
```

### Impact Analysis

See what's affected when a project changes:

```bash
# Via MCP tool
meta_analyze_impact --project shared-utils

# Returns:
# - Direct dependents
# - Transitive dependents
# - Total affected count
```

### Execution Ordering

Get topologically sorted build/test order:

```bash
# Via MCP tool
meta_execution_order

# Returns projects in correct dependency order
```

### Atomic Batch Execution

Execute commands with automatic rollback on failure:

```bash
# Via MCP tool
meta_batch_execute {"command": ["npm", "test"], "atomic": true}
```

If any repo fails, all repos are rolled back to pre-execution state.

---

## Filtering Commands

### By Tag

```bash
# Single tag (plugin commands work directly)
meta git pull --tag backend

# Multiple tags (OR logic) - use exec for non-plugin commands
meta exec --tag frontend,shared -- npm test
```

### By Directory

```bash
# Only these repos
meta --include api-service,web-app git status

# Skip a repo
meta --exclude legacy-app exec -- npm install
```

A value names a whole repo: its `.meta` name or its path from the workspace root (`.` for the root repo), never part of a path, so `api` does not select `api-legacy`. A value that names no repo is an error, so a typo never runs a command in a repo you meant to skip. Inside a worktree set, a repo is named by its alias (its path in the set). A value starting with `./` is a path only; use it when a name is ambiguous (a value that names several repos is an error too).

### Combined Filtering

```bash
# Tag filter + repo filter
meta --tag backend --include api git status
meta --tag backend --include api exec -- cargo test
```

**Filter precedence:**
1. `--tag` filters by project tags (meta level)
2. `--include` limits to the named repos
3. `--exclude` removes the named repos

---

## Scripting with Meta

### Chaining Commands

```bash
# Chain plugin commands and exec commands
meta git pull && meta exec -- npm install && meta exec -- npm test
```

### Capturing Output

```bash
# Get commit hashes
meta exec -- git rev-parse HEAD | grep -v "^>"
```

### Using with xargs

```bash
# List projects and process
meta project list | xargs -I{} echo "Project: {}"
```

### Exit Codes

Meta propagates exit codes from failed commands, making it suitable for CI/CD:

```bash
meta exec -- npm test || echo "Tests failed in at least one repo"
```

---

## JSON Output Mode

Get structured output for parsing and automation:

```bash
meta --json git status
meta --json exec -- git rev-parse HEAD
```

### Output Format

```json
{
  "success": true,
  "results": [
    {
      "directory": "./api",
      "command": "git rev-parse HEAD",
      "success": true,
      "exit_code": 0,
      "stdout": "abc123...\n"
    }
  ],
  "summary": {
    "total": 5,
    "succeeded": 5,
    "failed": 0
  }
}
```

### Parsing with jq

```bash
# Get only successful repos
meta --json git status | jq '.results[] | select(.success) | .directory'

# Count failures
meta --json npm test | jq '.summary.failed'
```

---

## Parallel Execution

By default, commands run sequentially with live output. Use `--parallel` for concurrent execution:

```bash
meta git status --parallel
meta exec --parallel -- cargo test
```

### Parallel Mode Behavior

- Uses thread pool for bounded concurrency
- Shows progress spinners (if TTY)
- Captures output, displays grouped after completion
- Faster for I/O-bound operations

### When to Use Parallel

**Good for:**
- `git fetch/pull/push`
- `git status`
- Read-only operations
- Independent builds

**Use sequential for:**
- Operations with shared state
- Commands with interactive output
- Debugging (easier to read)

---

## Environment Variables

| Variable | Description |
|----------|-------------|
| `META_DEBUG=1` | Enable debug output |
| `META_CONFIG` | Custom config file path |
| `META_SILENT=1` | Suppress output |

---

## Advanced Plugin Usage

### Plugin Locations

Plugins are discovered from:
1. `.meta/plugins/` next to the nearest meta config at or above the current directory, then next to each parent meta config that lists that directory as a project (the same workspace), nearest first. A parent that doesn't list it is another workspace and is not read. Inside a worktree set meta made, a checkout's own `.meta/plugins/` comes first, then those of the checkout it stands for (the one it was made from) and its workspace. `meta plugin install <name> --local` installs into the nearest one.
2. `~/.meta/plugins/` (moved by `META_DATA_DIR`), where `meta plugin install <name>` installs.
3. Executables named `meta-*` on PATH.

The first plugin found with a given name wins. A plugins folder meta can't read, and a plugin that doesn't answer `--meta-plugin-info` within 5 seconds, are skipped with a warning naming them. Project-local plugins are binaries: `--local` writes `.meta/plugins/.gitignore` (`*`) so they stay out of git as untracked files.

### Plugin Protocol

Plugins receive requests via stdin and respond via stdout:

**Request:**
```json
{
  "command": "status",
  "args": [],
  "projects": [{"name": "api", "path": "./api", "tags": ["backend"]}],
  "filters": {"tags": ["backend"]}
}
```

**Response:**
```json
{
  "success": true,
  "results": [...]
}
```

### Plugin Help

```bash
meta git --help
meta project --help
```

### Installing Plugins

```bash
# Search registry
meta plugin search docker

# Install
meta plugin install meta-docker

# List installed
meta plugin list

# This meta repo only (.meta/plugins/ next to the nearest meta config)
meta plugin install meta-docker --local
meta plugin list --local
```

---

## See Also

- [Plugin Development Guide](plugin_development.md)
- [Architecture Overview](architecture_overview.md)
- [MCP Server Documentation](mcp_server.md)
- [FAQ / Troubleshooting](faq_troubleshooting.md)


## Selecting a worktree set and choosing observation detail

`list` summarizes worktree sets; `status NAME` provides detailed Git state
(modified files, untracked counts, and ahead/behind information).

```sh
# Outside a worktree: list all sets, with branch and dirty/clean summaries.
meta git worktree list --recursive

# Select one set explicitly.
meta git worktree list example --recursive

# Inside .worktrees/example/ or any descendant: select the enclosing set.
meta git worktree list --recursive
```

An explicit name overrides the enclosing set. Outside a worktree, an omitted
name retains all-set listing. A selected set that is missing, empty or invalid
returns an error rather than an empty successful result. All-set recursive
listing continues to return readable sets and reports unavailable sets in
`warnings` (stderr for human output).

### Metadata-only listing

Background consumers that collect their own Git facts should request topology
without branch or dirty-state observation:

```sh
meta git worktree list example --recursive --metadata-only --json

# From inside the worktree, omit the name:
meta git worktree list --recursive --metadata-only --json
```

Both forms retain the list envelope, `worktrees`, with one selected set:

```json
{
  "worktrees": [{
    "name": "example",
    "root": "/work/harmony/.worktrees/example",
    "has_meta_root": true,
    "repos": [{
      "alias": ".",
      "path": "/work/harmony/.worktrees/example",
      "source_path": "/work/harmony"
    }]
  }]
}
```

Paths above are illustrative. Each repository has an alias, physical checkout
path and canonical source path; branch, dirty-state and ahead/behind fields are
absent. Set lifecycle metadata is included when available. Without
`--metadata-only`, existing branch/dirty fields are retained. The flag controls
observation detail independently of set selection and recursion; outside a
worktree, no name lists metadata for all sets.

`--recursive` includes realized repositories nested beneath other repositories.
Metadata-only without recursion retains legacy traversal boundaries. Recursive
metadata discovery launches no Git probes and uses strict bounded traversal:
invalid Git metadata, backlink mismatches and traversal failures are errors for
a selected set. All-set listing reports failed sets as warnings, so consumers
must check warnings before treating an aggregate inventory as complete.

Implicit worktree scoping uses metadata-only paths rather than branch probes
before dispatch. Membership discovery errors stop dispatch instead of silently
falling back to ordinary project configuration. Plugin list selection uses the
protocol request directory, even when the plugin process starts elsewhere. Membership still traverses the realized tree with a finite
entry budget. Poll sparingly, retain last-observed data as such, and back off on
failure. These changes reduce workload; they do not establish a fix for a macOS
kernel panic.
