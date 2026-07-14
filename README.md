# meta

[![Build Status](https://img.shields.io/github/actions/workflow/status/gitkb/meta/release.yml?branch=main)](https://github.com/gitkb/meta/actions)
[![Version](https://img.shields.io/github/v/release/gitkb/meta)](https://github.com/gitkb/meta/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

**Meta makes multi-repo architectures feel like monorepos** — without the downsides of monorepos. Keep your repositories independent (their own git history, CI, ownership boundaries), but operate on them as a cohesive unit.

Most multi-repo tools solve the *cloning* problem. Meta solves the *working* problem. A `.meta` file declares your repos. Everything else flows from that: clone them all at once, run any command across them, query their state, snapshot and rollback, understand their dependency graph — and let AI agents do it autonomously.

---

## Table of Contents

- [Why Meta](#why-meta)
- [Quick Start](#quick-start)
- [How It Works](#how-it-works)
- [Installation](#installation)
- [Configuration](#configuration)
- [Commands](#commands)
- [Filtering](#filtering)
- [Snapshots](#snapshots)
- [Plugins](#plugins)
- [AI Integration](#ai-integration)
- [Architecture](#architecture)
- [CLI Reference](#cli-reference)
- [Documentation](#documentation)
- [Roadmap](#roadmap)
- [Contributing](#contributing)
- [License](#license)

---

## Why Meta

| Problem | How Meta Solves It |
|---------|-------------------|
| **Git submodules** are painful — detached HEADs, nested `.git` state, no batch operations | Meta treats repos as peers, not hierarchy. Parallel execution built-in. |
| **Lerna/Nx/Turborepo** assume JavaScript | Meta is language-agnostic. Run `cargo test`, `npm install`, or `make build` — it doesn't care. |
| **Google's `repo` tool** is rigid | Meta has an extensible plugin system in any language. |
| **AI agents struggle** with multi-repo codebases | Meta is AI-native: MCP server, Claude Code skills, JSON output, query DSL — all designed for agents from day one. |

---

## Quick Start

1. **Clone a meta repository:**

   ```bash
   meta git clone https://github.com/org/meta-repo.git
   ```

   This clones the meta repo and all child repositories defined in its `.meta` file.

2. **Run workspace commands:**

   ```bash
   meta git status
   meta git pull
   meta exec npm install
   meta cargo test
   ```

3. **Filter by tags:**

   ```bash
   meta git pull --tag backend
   meta exec npm test --tag frontend,shared
   ```

4. **Query repo state:**

   ```bash
   meta query "dirty:true AND tag:backend"
   ```

5. **Snapshot before risky changes:**

   ```bash
   meta git snapshot create before-refactor
   # ... make changes ...
   meta git snapshot restore before-refactor  # rollback if needed
   ```

---

## How It Works

Meta is a **command router on top of a loop engine**, built in three layers:

**1. Loop Engine** — The foundation. Takes a list of directories and a command, runs it in each one (sequential or parallel), aggregates output. Simple, fast, reliable.

**2. Meta CLI** — The brain. Reads your `.meta` config, applies filters (tags, include/exclude), discovers plugins, and routes commands. When no plugin claims a command, it falls through to the loop engine via `meta exec`.

**3. Plugins** — The specialists. They don't *execute* — they *plan*. A plugin receives a request and returns an execution plan (a list of directory/command pairs). The loop engine does the actual work. This means plugins are pure functions, and execution is always consistent.

```
User runs: meta git status --tag backend
    │
    ▼
Meta CLI: parse args → load .meta → filter by tags → find plugin
    │
    ▼
Plugin (meta-git): "here's the plan" → [{dir: "./api", cmd: "git status"}, ...]
    │
    ▼
Loop Engine: execute plan → aggregate output → report results
```

Plugins communicate via JSON over stdin/stdout — language-agnostic, process-isolated, easy to debug. You could write a plugin in Python, Go, Rust, or anything else.

---

## Installation

### Via Install Script (Recommended)

**macOS/Linux:**
```bash
curl -fsSL https://raw.githubusercontent.com/gitkb/meta/main/install.sh | bash
```

**Windows (PowerShell):**
```powershell
irm https://raw.githubusercontent.com/gitkb/meta/main/install.ps1 | iex
```

### Via Homebrew (macOS/Linux)

```bash
brew install gitkb/tap/meta-cli
```

### Via cargo-binstall

```bash
cargo binstall meta-cli
```

### From Source

```bash
cargo install --git https://github.com/gitkb/meta
```

---

## Configuration

Meta projects are configured via a `.meta` file (JSON) or `.meta.yaml`/`.meta.yml` (YAML) in the repository root.

### Simple Format

Map project names to repository URLs:

```json
{
  "projects": {
    "api-service": "git@github.com:org/api-service.git",
    "web-app": "git@github.com:org/web-app.git",
    "shared-utils": "git@github.com:org/shared-utils.git"
  }
}
```

### Extended Format

Add tags, custom paths, and dependency tracking:

```yaml
projects:
  api-service:
    repo: git@github.com:org/api-service.git
    tags: [backend, rust]

  web-app:
    repo: git@github.com:org/web-app.git
    path: apps/web
    tags: [frontend, typescript]

  shared-utils:
    repo: git@github.com:org/shared-utils.git
    tags: [shared]
```

**Extended format fields:**

| Field | Required | Description |
|-------|----------|-------------|
| `repo` | Yes | Git repository URL |
| `path` | No | Custom clone path (defaults to project name) |
| `tags` | No | Array of tags for filtering |
| `meta` | No | Set `true` if this project contains a nested `.meta.yaml` |
| `provides` | No | Array of capabilities this project provides |
| `depends_on` | No | Array of project names or provided capabilities this depends on |

**File priority:** `.meta.yaml` > `.meta.yml` > `.meta`

You can mix simple and extended formats in the same file.

### Nested Meta Repos

Meta repos can contain other meta repos, enabling hierarchical organization of large codebases:

```yaml
projects:
  infrastructure:
    repo: git@github.com:org/infrastructure.git
    meta: true  # This directory has its own .meta.yaml

  apps:
    repo: git@github.com:org/apps.git
    meta: true
```

When `meta: true` is set, commands with `--recursive` will descend into the nested meta repo and process its children too:

```bash
# Clone everything, including nested meta repos
meta git clone https://github.com/org/top-level.git

# Update all repos recursively
meta git update --recursive

# List entire tree
meta project list --recursive
```

### Dependency Tracking

For impact analysis and build ordering:

```yaml
projects:
  shared-utils:
    repo: git@github.com:org/shared-utils.git
    provides: [utils-api]

  auth-service:
    repo: git@github.com:org/auth-service.git
    depends_on: [shared-utils]

  api-service:
    repo: git@github.com:org/api-service.git
    depends_on: [auth-service, utils-api]
```

This enables impact analysis ("what breaks if I change shared-utils?"), topological build ordering, and transitive dependency resolution.

---

## Commands

### Core

| Command | Description |
|---------|-------------|
| `meta exec <command>` | Run any command across all repositories |
| `meta git status` | Git status for all repos |
| `meta git pull` | Pull latest changes |
| `meta git push` | Push commits |
| `meta query "<expr>"` | Query repos by state |

### Git Plugin

| Command | Description |
|---------|-------------|
| `meta git clone <url>` | Clone meta repo + all child repos |
| `meta git update` | Pull changes and clone missing repos |
| `meta git setup-ssh` | Configure SSH multiplexing |
| `meta git commit --edit` | Per-repo commit messages via editor |
| `meta git commit -m "msg"` | Same message across all repos |
| `meta git snapshot create <name>` | Capture workspace state |
| `meta git snapshot list` | List snapshots |
| `meta git snapshot restore <name>` | Restore workspace to snapshot |

### Rust/Cargo Plugin

`meta cargo` is an explicit plugin namespace. The Rust plugin selects directories that contain a
`Cargo.toml` and runs the requested command in each one; Cargo remains responsible for validating
built-in commands, aliases, and installed `cargo-*` extensions. `meta rust` is an alias that
produces the same canonical `cargo` command.

```bash
meta cargo clean
meta cargo check --all-targets
meta cargo clippy --all-targets -- -D warnings
meta cargo nextest run
meta rust test
```

Put Meta options before the `cargo` or `rust` namespace:

```bash
meta --include api --dry-run cargo check
meta --dry-run --sequential cargo clean --recursive
```

In this first pass, `--recursive` before Cargo's `--` remains a Meta scope control, including
postfix usage such as `meta cargo clean --recursive`.

The Rust plugin treats `-h` and `--help` before Cargo's `--` separator as side-effect-free
namespace help. For command-specific Cargo help, run
`cargo help <command>` directly (for example, `cargo help check`). Meta never inspects or removes
arguments after Cargo's `--` separator.

### Plugin Management

| Command | Description |
|---------|-------------|
| `meta plugin list` | List installed plugins |
| `meta plugin search <query>` | Search plugin registry |
| `meta plugin install <name>` | Install a plugin |
| `meta plugin uninstall <name>` | Remove a plugin |

---

## Filtering

### By Tag

```bash
meta git pull --tag backend
meta exec npm test --tag frontend,shared
```

### By Directory

```bash
meta git status --include api-service,web-app
meta exec npm install --exclude legacy-app
```

### Recursive Processing

```bash
meta git status --recursive
meta git pull --recursive --depth 2
```

### Query DSL

```bash
meta query "dirty:true"
meta query "dirty:true AND tag:backend AND branch:main"
meta query "ahead:true"
```

**Conditions:** `dirty`, `branch`, `tag`, `ahead`, `behind` — combined with `AND`.

---

## Snapshots

Capture the complete state of all repositories for safe batch operations:

```bash
meta git snapshot create before-refactor
meta git snapshot list
meta git snapshot restore before-refactor --dry-run
meta git snapshot restore before-refactor
```

Snapshots record each repo's commit SHA, branch, and dirty status. Dirty repos are auto-stashed on restore.

---

## Plugins

Plugins are standalone executables that extend meta. They're discovered automatically from:

- `.meta-plugins/` in the current or parent directories
- `~/.meta-plugins/`
- System PATH (binaries named `meta-*`)

### Built-in Plugins

| Plugin | Description |
|--------|-------------|
| `git` | Clone, status, update, commit, snapshots, SSH multiplexing |
| `project` | Project health checks and sync |
| `rust` | Cargo commands across Rust projects; `meta rust` aliases `meta cargo` |

### Writing Plugins

Plugins communicate via JSON over stdin/stdout. Any language works:

```bash
# Meta asks: "What can you do?"
meta-myplugin --meta-plugin-info
# → {"name": "myplugin", "version": "1.0", "commands": ["myplugin run"]}

# Meta asks: "Execute this command"
echo '{"command":"myplugin run","args":[],"projects":[...]}' | meta-myplugin --meta-plugin-exec
# → {"plan": {"commands": [{"dir": "./repo1", "cmd": "..."}]}}
```

See [docs/plugin_development.md](docs/plugin_development.md) for the full guide.

---

## AI Integration

Meta is designed for AI agents from day one. Three integration points:

### MCP Server (29 tools)

```json
{
  "mcpServers": {
    "meta": {
      "command": "meta-mcp",
      "args": []
    }
  }
}
```

**Tools include:** project listing, git operations (status/pull/push/commit/diff/branch), build/test, code search, file tree discovery, query DSL, impact analysis, workspace snapshots, and batch execution.

See [docs/mcp_server.md](docs/mcp_server.md) for full documentation.

### Claude Code Skills

```bash
meta init claude
```

Installs purpose-built skills that teach Claude Code how to work with multi-repo codebases — using `meta git` instead of `git`, creating snapshots before risky changes, filtering by tags, and understanding plugin interception.

### JSON Output

Every command supports `--json` for structured, machine-readable output:

```bash
meta git status --json
meta query "dirty:true" --json
```

---

## Architecture

Meta is a Rust workspace of 10 crates:

| Crate | Purpose |
|-------|---------|
| `meta_cli` | Main CLI — config loading, plugin routing, filtering |
| `loop_lib` | Core execution engine — runs commands across directories |
| `loop_cli` | Standalone loop CLI (usable without meta) |
| `meta_core` | Shared infrastructure — `~/.meta/` directory, lockfile, atomic store |
| `meta_plugin_protocol` | Shared types for the plugin contract |
| `meta_git_cli` | Git plugin — clone, update, status, commit, snapshots |
| `meta_git_lib` | Shared git library utilities |
| `meta_project_cli` | Project management plugin |
| `meta_rust_cli` | Rust/Cargo plugin |
| `meta_mcp` | MCP server for AI agent integration |

Each plugin is both a workspace member and a child repo in the `.meta` manifest — meta manages itself with itself.

See [docs/architecture_overview.md](docs/architecture_overview.md) for the full design.

---

## Loop System

The **loop** engine is the foundation that runs commands across directories. It can be used standalone:

```bash
loop git status
loop npm install
loop cargo test
```

Configure with `.looprc`. See [docs/loop.md](docs/loop.md) for details.

---

## CLI Reference

```
Usage: meta [OPTIONS] [COMMAND]...

Arguments:
  [COMMAND]...  Command to run across repositories

Options:
  -c, --config <FILE>       Path to .meta config file
  -t, --tag <TAGS>          Filter by tag(s), comma-separated
  -i, --include <DIRS>      Include only these directories
  -e, --exclude <DIRS>      Exclude these directories
  -r, --recursive           Process nested meta repos
      --depth <N>           Max recursion depth
      --json                Output in JSON format
  -v, --verbose             Verbose output
  -s, --silent              Silent mode
  -h, --help                Print help
  -V, --version             Print version
```

---

## Documentation

- **[Architecture Overview](docs/architecture_overview.md)** — System design
- **[MCP Server Guide](docs/mcp_server.md)** — AI agent integration
- **[Plugin Development](docs/plugin_development.md)** — Writing plugins
- **[Claude Code Skills](docs/claude_code_skills.md)** — AI workflow skills
- **[Advanced Usage](docs/advanced_usage.md)** — Power-user features
- **[Loop System](docs/loop.md)** — Loop engine details
- **[FAQ / Troubleshooting](docs/faq_troubleshooting.md)** — Common issues

---

## Roadmap

- [x] Core CLI + plugin system
- [x] Git plugin with parallel clone
- [x] YAML configuration support
- [x] Project tags and filtering
- [x] JSON output mode
- [x] Nested meta repos (`--recursive`)
- [x] MCP server for AI agents (29 tools)
- [x] Multi-commit support (`meta git commit --edit`)
- [x] Plugin help system
- [x] Query DSL for advanced filtering
- [x] Workspace snapshots with rollback
- [x] Dependency tracking and impact analysis
- [x] Claude Code skills integration
- [x] Windows support (PowerShell installer)
- [ ] Dependency graph visualization (CLI)
- [ ] GUI for visual management

See [.context/VISION_PLAN.md](.context/VISION_PLAN.md) for full details.

---

## Contributing

Contributions welcome! Please see [CONTRIBUTING.md](CONTRIBUTING.md).

---

## Community & Support

- **Issues:** [GitHub Issues](https://github.com/gitkb/meta/issues)
- **Discussions:** [GitHub Discussions](https://github.com/gitkb/meta/discussions)

---

## License

MIT License. See [LICENSE](LICENSE).
