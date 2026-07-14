# Advanced Usage Guide

This guide covers advanced features of the `meta` CLI, including query DSL, snapshots, dependency tracking, and power-user workflows.

## Table of Contents

- [Query DSL](#query-dsl)
- [Workspace Snapshots](#workspace-snapshots)
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

## Workspace Snapshots

Snapshots capture the complete state of all repositories, enabling safe batch operations with rollback capability.

### Creating Snapshots

```bash
# Create a named snapshot
meta git snapshot create before-refactor

# Create with description
meta git snapshot create release-v2 --description "Pre-release state"
```

### Listing Snapshots

```bash
meta git snapshot list
```

Output shows snapshot name, creation time, and repo count.

### Restoring Snapshots

```bash
# Preview what restore would do
meta git snapshot restore before-refactor --dry-run

# Actually restore
meta git snapshot restore before-refactor

# Force restore even with uncommitted changes
meta git snapshot restore before-refactor --force
```

### What Snapshots Capture

Per repository:
- Current commit SHA
- Branch name
- Dirty status

On restore:
- Checks out the captured commit
- Auto-stashes uncommitted changes (restored after)
- Switches to the captured branch

### Safe Batch Workflow

```bash
# 1. Create snapshot before risky operation
meta git snapshot create before-changes

# 2. Make changes across repos
meta exec -- npm update

# 3. Test changes
meta exec -- npm test

# 4. If tests fail, restore
meta git snapshot restore before-changes

# 5. If tests pass, commit
meta git commit -m "chore: update dependencies"
```

### Deleting Snapshots

```bash
meta git snapshot delete before-refactor
```

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
meta_batch_execute --command "npm test" --atomic true
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
# Include only specific directories (plugin commands)
meta git status --include api-service,web-app

# Exclude directories - use exec for non-plugin commands
meta exec --exclude legacy-app -- npm install
```

### Combined Filtering

```bash
# Tag filter + directory filter
meta --tag backend git status --include api

# Cargo commands use the Rust plugin namespace directly
meta --tag backend --include api cargo test
```

**Filter precedence:**
1. `--tag` filters by project tags (meta level)
2. `--include` limits to specific directories (loop level)
3. `--exclude` removes directories (loop level)

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
meta --parallel cargo test
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
1. `.meta-plugins/` in current directory
2. `~/.meta-plugins/` in home directory
3. Executables named `meta-*` in PATH

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
```

---

## See Also

- [Plugin Development Guide](plugin_development.md)
- [Architecture Overview](architecture_overview.md)
- [MCP Server Documentation](mcp_server.md)
- [FAQ / Troubleshooting](faq_troubleshooting.md)
