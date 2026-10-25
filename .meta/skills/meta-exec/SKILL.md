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

# Any program with its arguments
meta exec -- ls -la
meta exec -- npm install --save-dev typescript
```

Each argument meta receives reaches the program unchanged: meta runs the program directly, with no shell in between. Quotes, spaces, `$()`, `;`, `|`, `&&`, globs and `%` in an argument stay literal, so text from an issue, a commit message or a file name can't run commands once meta has it. Your own shell still parses the command line before meta runs (`"$(...)"` in double quotes runs there), so pass untrusted text in single quotes or as a separate argv element (as the MCP tools take it). When you want a shell (pipes, `&&`, redirects, globs), ask for one:

```bash
meta exec -- sh -c 'npm ci && npm test'
```

A `.looprc` alias for the program is expanded into words (POSIX quoting rules, nothing expanded or run). On Windows meta does not wrap commands in `cmd /c` either. Batch files such as `npm.cmd` need their extension (`meta exec -- npm.cmd install`); Windows runs those through `cmd.exe`, and Rust escapes their arguments for it and refuses any it cannot pass safely.

The `--` separates meta options from the command to execute. The `--` is optional unless your command starts with `-`.

```bash
# These are equivalent:
meta exec make test
meta exec -- make test

# Use -- when command starts with dash:
meta exec -- --version
```

## Parallel vs Sequential

Commands run in parallel by default (a `.meta` config can set `defaults.parallel: false`; it is only read when you run meta from the directory holding that `.meta`). Use `--sequential` for live output in order, or `--parallel` to force concurrency:

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

Control which repos run the command:

```bash
# Only these repos: each value is a repo's .meta name or its path from the workspace root
meta --include api,worker git status
meta --include libs/core,. git status   # `.` is the root repo

# Skip these repos
meta --exclude legacy-service git push

# Filter by tag (applied before --include/--exclude)
meta --tag backend exec -- make deploy

# Combine: tag filter + directory filter
meta --tag backend --include api git status
```

**Filter precedence:**
1. `--tag` filters projects by tag (meta level)
2. `--include` limits to the named repos
3. `--exclude` removes the named repos

A value names a whole repo, never part of a path: `api` does not match `api-tests`. A value that names no repo is an error, so a typo never runs a command in the repo you meant to skip. Inside a worktree set, a repo is named by its alias (its path in the set). A value starting with `./` is a path only; use it when a name is ambiguous (a value that names several repos is an error too). The meta root repo stays included even when it doesn't match a tag.

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
| `--include <repos>` | Only these repos (`.meta` name or path from the root) |
| `--exclude <repos>` | Skip these repos (`.meta` name or path from the root) |
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
meta --exclude large-monorepo exec -- cargo update
```

### Find Files Across Repos
```bash
# Arguments go to find as typed: "*.rs" is find's pattern, not a shell glob
meta exec -- find . -name '*.rs' -type f | head -20
```

### Clean Build Artifacts
```bash
meta exec -- cargo clean
meta exec -- rm -rf node_modules dist
```

## When to Use Exec vs Plugins

| Use `meta exec` | Use Plugin (e.g., `meta git`) |
|-----------------|-------------------------------|
| Generic commands | Git operations |
| Build/test commands | Commands needing special handling |
| One-off scripts | Operations with meta-specific logic |
| npm/cargo/make | Clone, update, worktree |

`meta exec -- <cmd>` always runs the command as typed, never through a plugin: `meta exec -- git status` runs plain `git status` in each repo, while `meta git status` uses the git plugin. Prefer `meta git <cmd>` for git: any git subcommand passes through it, and it runs remote commands (`fetch`, `pull`, `push`) without credential prompts that would hang parallel runs. Plugins intercept only their own subcommands (`meta git ...`, `meta cargo ...`) and provide enhanced behavior. `meta git clone` doesn't run `git clone` in each repo—it reads `.meta` and clones the entire graph.

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
