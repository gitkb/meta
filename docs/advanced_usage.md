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

If any repo fails, the command stops and each repo it reached is put back on its branch and commit from before; read `rollback.status`, and `rollback.failed` for any repo it couldn't fully put back (see [Agent Workflows](agent_workflows.md)).

---

## Filtering Commands

### By Tag

```bash
# Single tag
meta --tag backend git pull

# Multiple tags (OR logic)
meta --tag frontend,shared exec -- npm test
```

### By Directory

```bash
# Only these repos
meta --include api-service,web-app git status

# Skip a repo
meta --exclude legacy-app exec -- npm install
```

A value names a whole repo: its `.meta.yaml` name or its path from the workspace root (`.` for the root repo), never part of a path, so `api` does not select `api-legacy`. A value that names no repo is an error, so a typo never runs a command in a repo you meant to skip. Inside a worktree set the same holds: a repo sits at its project's path, so it is named by its `.meta.yaml` name or that path, from the set's root. A value starting with `./` is a path only; use it when a name is ambiguous (a value that names several repos is an error too).

### Combined Filtering

```bash
# The backend repos and the root repo
meta --tag backend --include . git status
# The backend repos except one
meta --tag backend --exclude legacy exec -- cargo test
```

**Filters:** `--include` and `--tag` each add repos: the ones `--include` names, and the ones with any of the `--tag` tags (with neither, every repo). `--exclude` then removes the repos it names. The root repo has no tags, so `--tag` alone never selects it; add it with `--include .`. A tag no project has is an error that lists the tags there are. With `--recursive`, a nested project is selected by its own tags, whatever its parent's.

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

| Code | Meaning |
|------|---------|
| 0 | Done |
| 1 | Something failed: a command in at least one repo (the summary names them), or meta itself (`Error: …` on stderr) |
| 2 | The command line is wrong: an unknown command, option or value |
| 141 | Its output was cut off: stdout closed early, as in `meta exec -- git log \| head` |
| 128 + n | Stopped by signal n (Ctrl-C: 130), or a plugin was |

A plugin's own exit code passes through. `loop` and `meta-mcp` exit the same way.

### Colour

Output is coloured only on a terminal: never into a pipe, a file or `--json`. `NO_COLOR` turns colour off, `CLICOLOR_FORCE=1` turns it on anyway (not under `--json`), and `TERM=dumb` or `CLICOLOR=0` turn it off. A command meta captures (in parallel mode, or for JSON) is told the same, through `CLICOLOR_FORCE`/`FORCE_COLOR`; git gets `color.ui=always` after your own `GIT_CONFIG_*` entries, never in place of them. Each command's stdout goes to meta's stdout and its stderr to meta's stderr.

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

A command a signal ended has `"exit_code": null` and the signal's number in
`"signal"` (`9` for `SIGKILL`); `signal` is left out otherwise.

With `--json`, stdout holds only JSON: one document, or the documents a plugin writes. A command line clap can't parse is reported before meta knows about `--json`, so it is text on stderr (exit 2). After that, an error before any output is `{"success": false, "error": "…"}` (an unknown command exits 2, other errors 1); one after a report was printed goes to stderr, so the report stays the only document. A plugin's output that isn't JSON comes as `{"success": …, "output": "…"}`, and a plugin that fails without printing anything leaves `{"success": false, "error": "plugin … failed"}`. Warnings and `--verbose` lines go to stderr.

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
meta --parallel git status
meta --parallel exec -- cargo test
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

A plugin named `meta-<word>` serves `meta <word> ...`, and the first file found with that name wins. For a command meta starts only that file; if it doesn't answer `--meta-plugin-info` with its info within 5 seconds, the command fails naming it. `meta`, `meta help`, `meta --help` and `meta plugin list` start every plugin; there a plugins folder meta can't read, and a plugin that doesn't answer within 5 seconds, are skipped with a warning naming them. So is a file in `.meta/plugins` or `~/.meta/plugins` that can't be started, isn't executable, or answers with anything but its info; a `meta-*` program on PATH that can't be started or isn't a plugin is just left out, as it may be any program. Project-local plugins are binaries: `--local` writes `.meta/plugins/.gitignore` (`*`) so they stay out of git as untracked files.

### Plugin Protocol

Plugins receive requests via stdin and respond via stdout:

**Request:**
```json
{
  "command": "status",
  "args": [],
  "projects": ["/ws/api"],
  "cwd": "/ws",
  "options": {"tag_filters": ["backend"], "include_filters": null, "exclude_filters": null}
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
a selected set. All-set listing (recursive or not, with or without
`--metadata-only`) reports failed sets as warnings, in the text listing too, so
consumers must check warnings before treating an aggregate inventory as
complete. Without `--metadata-only`, a member git can't read has `"branch":
null` and `"dirty": null` (unknown, never clean). `meta git worktree status`
shows the same unknowns, and names members whose checkout is gone in
`"missing"`, exiting 1.

Implicit worktree scoping uses metadata-only paths rather than branch probes
before dispatch. Membership discovery errors stop dispatch instead of silently
falling back to ordinary project configuration. Plugin list selection uses the
protocol request directory, even when the plugin process starts elsewhere. Membership still traverses the realized tree with a finite
entry budget. Poll sparingly, retain last-observed data as such, and back off on
failure. These changes reduce workload; they do not establish a fix for a macOS
kernel panic.
