---
name: meta-exec
description: Execute commands across all repos — parallel, filtered, dry-run modes.
---

# Meta Exec Skill

Execute any command in the root repo and every project at this meta level (add `--recursive` for nested meta repos). Meta extends `loop`, a tool for running commands across directories.

## The Execution Model

`meta exec` runs a command in each repo directory:

```bash
# Run 'make build' in every repo
meta exec -- make build

# Run any shell command
meta exec -- ls -la

# Commands with arguments
meta exec -- npm install --save-dev typescript
```

The `--` separates meta options from the command to execute. The `--` is optional unless your command starts with `-`.

```bash
# These are equivalent:
meta exec make test
meta exec -- make test

# Use -- when command starts with dash:
meta exec -- --version
```

## Parallel vs Sequential

Commands run in parallel by default (a `.meta` config can set `defaults.parallel: false`). Use `--sequential` for live output in order, or `--parallel` to force concurrency:

```bash
# Parallel (default) - faster, grouped output after completion
meta exec -- cargo build

# Sequential - see output as it happens
meta --sequential exec -- cargo build
```

Parallel mode:
- Uses rayon thread pool for bounded concurrency
- Shows spinners during execution (if TTY)
- Captures and displays output grouped by repo after completion

## Filtering Options

Control which repos run the command. These options come from `loop`:

```bash
# Only directories whose path contains one of these (substring match)
meta --include api,worker git status

# Skip directories whose path contains one of these
meta --exclude legacy-service git push

# Filter by tag (meta-specific, applied before loop filtering)
meta --tag backend exec -- make deploy

# Combine: tag filter + directory filter
meta --tag backend --include api git status
```

**Filter precedence:**
1. `--tag` filters projects by tag (meta level)
2. `--include` limits to specific directories (loop level)
3. `--exclude` removes directories (loop level)

Include/exclude match substrings of directory paths, so `api` also matches `api-tests`. The meta root repo stays included even when it doesn't match a tag.

## Dry Run

Preview what would happen without executing:

```bash
meta --dry-run exec -- rm -rf node_modules
```

Shows which repos would run the command, with `[DRY RUN]` prefix.

## JSON Output

Get structured output for parsing:

```bash
meta --json exec -- git rev-parse HEAD
```

Returns JSON with:
```json
{
  "success": true,
  "results": [
    {
      "directory": "/path/to/workspace/api",
      "command": "git rev-parse HEAD",
      "success": true,
      "exit_code": 0,
      "stdout": "abc123...\n"
    }
  ],
  "summary": {
    "total": 5,
    "succeeded": 5,
    "failed": 0,
    "dry_run": false
  }
}
```

## Silent Mode

Suppress command output (only meta's final failure message remains):

```bash
meta --silent exec -- npm install
```

## Global Options Reference

| Option | Description |
|--------|-------------|
| `--parallel` | Run commands concurrently (the default) |
| `--sequential` | Run one repo at a time |
| `--include <dirs>` | Only directories whose path contains these |
| `--exclude <dirs>` | Skip directories whose path contains these |
| `--tag <tags>` | Filter by project tag(s) |
| `--dry-run` | Preview without executing |
| `--json` | Structured JSON output |
| `--silent` | Suppress output |
| `--verbose` | Show detailed execution info |
| `--recursive` | Include nested meta repos |

## Practical Examples

### Build Everything
```bash
meta exec -- cargo build --release
```

### Run Tests
```bash
meta exec -- cargo test
```

### Update Dependencies Selectively
```bash
# Repos tagged frontend (plus the root)
meta --tag frontend exec -- npm update

# Exclude slow repos (meta options go before `exec`)
meta --exclude large-monorepo exec -- cargo update   # fails if meta-rust is installed (see below)
```

### Find Files Across Repos
```bash
# Quote the whole command: meta joins its arguments with spaces for the shell
meta exec -- 'find . -name "*.rs" -type f' | head -20
```

### Clean Build Artifacts
```bash
meta exec -- cargo clean   # fails if meta-rust is installed (see below)
meta exec -- rm -rf node_modules dist
```

## When to Use Exec vs Plugins

| Use `meta exec` | Use Plugin (e.g., `meta git`) |
|-----------------|-------------------------------|
| Generic shell commands | Git operations |
| Build/test commands | Commands needing special handling |
| One-off scripts | Operations with meta-specific logic |
| npm/cargo/make | Clone, update, snapshot |

`meta exec -- <cmd>` still offers the command to plugins first: `meta exec -- git ...` goes to the git plugin, and every `meta exec -- cargo ...` goes to meta-rust when it is installed. meta-rust only handles `build` and `test`, so with it installed other cargo commands (`cargo update`, `cargo clean`) fail. Plugins intercept command patterns and provide enhanced behavior. `meta git clone` doesn't run `git clone` in each repo—it reads `.meta` and clones the entire graph.

## Worktree Context Detection

When your cwd is inside a `.worktrees/<name>/` directory, `meta exec` automatically scopes to the worktree's repos instead of the primary checkout:

```bash
cd .worktrees/auth-fix/backend
meta exec -- cargo test    # runs in auth-fix's repos

# --primary skips worktree detection; config is still found from the cwd,
# so run it from the primary checkout to target the primary repos
meta --primary exec -- cargo test
```

This is filesystem-based detection—no store dependency. It skips repos nested inside another repo of the set. See the `meta-worktree` skill for full worktree management.

## Efficiency Tips

- **Target precisely**: Use `--include`/`--exclude`/`--tag` to run commands in only the repos you need — avoids running commands you'll have to undo
- **Dependency order**: There is no CLI flag for dependency order. Get it from the MCP tool `meta_execution_order` (from `depends_on` in `.meta.yaml`), then run repos one at a time in that order with `--include`
- **Cross-repo dependencies**: Use `--sequential` when one repo's step needs another's output; parallel is the default
- **Dry run first**: `meta --dry-run exec -- dangerous-command` shows what would happen before committing
