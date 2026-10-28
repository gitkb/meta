# meta

[![Build Status](https://img.shields.io/github/actions/workflow/status/gitkb/meta/release.yml?branch=main)](https://github.com/gitkb/meta/actions)
[![Version](https://img.shields.io/github/v/release/gitkb/meta)](https://github.com/gitkb/meta/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

**Meta makes multi-repo architectures feel like monorepos** — without the downsides of monorepos. Keep your repositories independent (their own git history, CI, ownership boundaries), but operate on them as a cohesive unit.

Most multi-repo tools solve the *cloning* problem. Meta solves the *working* problem. A meta config file (`.meta.yaml`) declares your repos. Everything else flows from that: clone them all at once, run any command across them, query their state, understand their dependency graph — and let AI agents do it autonomously.

---

## Table of Contents

- [Why Meta](#why-meta)
- [Quick Start](#quick-start)
- [How It Works](#how-it-works)
- [Installation](#installation)
- [Configuration](#configuration)
- [Commands](#commands)
- [Filtering](#filtering)
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

   This clones the meta repo and all child repositories defined in its meta config.

2. **Run commands across all repos:**

   ```bash
   meta git status
   meta git pull
   meta exec npm install
   meta exec cargo test
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

5. **Commit before risky changes:**

   ```bash
   meta git add -A && meta git commit -m "wip: before refactor"
   meta --dry-run exec -- <command>  # preview before running
   ```

---

## How It Works

Meta is a **command router on top of a loop engine**, built in three layers:

**1. Loop Engine** — The foundation. Takes a list of directories and a command, runs it in each one (sequential or parallel), aggregates output. Simple, fast, reliable.

**2. Meta CLI** — The brain. Reads your meta config, applies filters (tags, include/exclude), discovers plugins, and routes commands. `meta exec -- <cmd>` hands a command to the loop engine as typed, never to a plugin; any other command goes to the plugin that claims it, or fails as unrecognized.

**3. Plugins** — The specialists. They don't *execute* — they *plan*. A plugin receives a request and returns an execution plan (a list of directory/command pairs). The loop engine does the actual work. This means plugins are pure functions, and execution is always consistent.

```
User runs: meta git status --tag backend
    │
    ▼
Meta CLI: parse args → load .meta.yaml → filter by tags → find plugin
    │
    ▼
Plugin (meta-git): "here's the plan" → [{dir: "./api", argv: ["git", "status"]}, ...]
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

The scripts download the release archive for your platform together with
its published `.sha256` and check it before installing anything; a
download error, a missing checksum file or a mismatch stops the install.
`install.sh` needs `curl`, `tar` and `sha256sum` or `shasum`.

Set `VERSION` for a specific release, `INSTALL_DIR` for the target
directory, or `META_DOWNLOAD_URL` to install from a mirror of
`https://github.com/gitkb/meta/releases`. The variables go on `bash`, not
on `curl`:

```bash
curl -fsSL https://raw.githubusercontent.com/gitkb/meta/main/install.sh | VERSION=0.2.22 bash
```
```powershell
$env:VERSION = "0.2.22"; irm https://raw.githubusercontent.com/gitkb/meta/main/install.ps1 | iex
```

The checksum comes from the same place as the archive, so it catches a
corrupted or truncated download, not a compromised source: use a mirror you
trust, over https.

### Via Homebrew (macOS/Linux)

```bash
brew install gitkb/tap/meta-cli
```

### From Source

Prerequisites: [rustup](https://rustup.rs) (it installs the toolchain
pinned in `rust-toolchain.toml`), a C compiler and linker (Xcode Command
Line Tools on macOS, `build-essential` or your distro's equivalent on
Linux, Visual Studio Build Tools on Windows), `git` and `make`.

meta's crates live in their own repos, which this repo's Cargo workspace
expects as sibling directories. Clone them all, then install with the
committed `Cargo.lock`:

```bash
git clone https://github.com/gitkb/meta && cd meta
for p in loop_cli loop_lib meta_cli meta_core meta_git_cli meta_git_lib \
         meta_mcp meta_plugin_protocol meta_project_cli meta_rust_cli; do
  git clone "https://github.com/gitkb/$p"
done
make install
```

This installs `meta`, the `meta-git`, `meta-project` and `meta-rust`
plugins, `meta-mcp` and `loop` into `~/.cargo/bin` (the crates in the
Makefile's `BIN_CRATES`, the binaries a release ships), replacing any
copies already there. To update, `git pull` in each directory and run
`make install` again. Copies the install scripts put in `~/.local/bin` or
`%USERPROFILE%\.meta\bin` (their defaults) stay; remove them or put
`~/.cargo/bin` first in `PATH`. `agent` and `meta-plugins` in `.meta.yaml`
are not needed for the build; `meta git` commands report them as missing
until you clone them too.

---

## Configuration

Meta projects are configured by one YAML file in the repository root: `.meta.yaml` (or `.meta.yml`). Two config files in one directory are an error that names both; keep one.

### Simple Format

Map project names to repository URLs:

```yaml
projects:
  api-service: git@github.com:org/api-service.git
  web-app: git@github.com:org/web-app.git
  shared-utils: git@github.com:org/shared-utils.git
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

**Rules for names and values.** meta checks every entry when it reads the config and stops with an error that names the file and the project:

- `path` (the name when it is omitted) is relative and stays inside the directory of the config file: no `..`, no absolute paths, and no symlink that leads outside it or into `.git` or `.worktrees`. `\` reads as `/`, and `./`, `//` and a trailing `/` are dropped.
- A project name is usually one word; a nested alias such as `vendor/lib` is fine. It follows the same rules as `path` and must already be in plain form.
- Names and path components use only ASCII letters, digits, space and `._-+@`, so one config names the same directories on every platform (no Unicode that macOS or Windows would treat as another name, no `C:`, `<>:"|?*` or short names like `PROGRA~1`). They have no trailing dot or space, are not Windows device names (`CON`, `NUL`, `COM1`, ...) or `.git`, and do not start with `.worktrees` or lie under the configured `worktrees_dir`.
- No two projects share a path, and no project's path lies inside another's. Paths are compared after following existing symlinks, and without regard to case (some filesystems ignore it). Project names may not be equal or nested either, also without regard to case. A repo inside another repo goes in a nested meta config (`meta: true`).
- No name, path, repo, tag, `provides` or `depends_on` value is empty or contains a control character (such as an escape or a newline), a bidirectional-text control, or an invisible format character (such as a zero-width space).
- `repo` does not start with `-`.
- Tags contain no whitespace and no `,`.
- `worktrees_dir`, if set, lies inside the workspace (relative, or absolute but inside), not inside a project and not in `.git`. For a directory elsewhere, set `META_WORKTREES`.

The same rules apply to every meta config that a recursive command reads, including one inside a cloned project: a refused nested config stops the command rather than silently leaving out that part of the tree.

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
| `meta exec -- <command>` | Run any command across all repositories (no shell; `sh -c '...'` for one) |
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

### Plugin Management

| Command | Description |
|---------|-------------|
| `meta plugin list` | List installed plugins |
| `meta plugin search <query>` | Search plugin registry |
| `meta plugin install <name>` | Install a plugin |
| `meta plugin uninstall <name>` | Remove a plugin |

Add `--local` to install, list, uninstall or update the plugins of this meta repo only (`.meta/plugins/` next to the nearest meta config).

---

## Filtering

### By Tag

```bash
meta git pull --tag backend
meta exec npm test --tag frontend,shared
```

### By Directory

```bash
meta --include api-service,web-app git status
meta --exclude legacy-app exec -- npm install
```

A value names a whole repo: its `.meta.yaml` name or its path from the workspace root (`.` for the root repo), never part of a path, so `api` does not select `api-legacy`. A value that names no repo is an error, so a typo never runs a command in a repo you meant to skip. Inside a worktree set the same holds: a repo sits at its project's path, so it is named by its `.meta.yaml` name or that path, from the set's root. A value starting with `./` is a path only; use it when a name is ambiguous (a value that names several repos is an error too).

### Arguments

Meta runs every command as a program with its arguments, never through a shell. Each argument meta receives reaches the program unchanged, so quotes, spaces, `$()`, `;`, globs and `%` stay literal (`meta git log --format='%h %s'` works, and text pasted into a commit message can't run commands). Your own shell still parses the line before meta runs, so `"$(...)"` in double quotes runs there: pass untrusted text in single quotes. When you want a shell, ask for one: `meta exec -- sh -c 'npm ci && npm test'`. `meta git status`, `commit`, `clone`, `update` and `worktree` are meta's own commands with their own options; for git's native behavior, `meta exec -- git ...`. Meta still reads its own `--json`, `--verbose`, `--silent`, `--recursive`, `--strict` and `--primary` after a plugin command (`meta git status --json`); put arguments that look like those after `--` (`meta git add -- --json`).

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

## Plugins

Plugins are standalone executables that extend meta. They're discovered automatically from:

1. `.meta/plugins/` next to the nearest meta config at or above the current directory, then next to each parent meta config that lists that directory as a project (the same workspace), nearest first. A parent that doesn't list it is another workspace and is not read. Inside a worktree set meta made, a checkout's own `.meta/plugins/` comes first, then those of the checkout it stands for (the one it was made from) and its workspace. `meta plugin install <name> --local` installs into the nearest one.
2. `~/.meta/plugins/` (moved by `META_DATA_DIR`), where `meta plugin install <name>` installs.
3. Executables named `meta-*` on PATH.

The first plugin found with a given name wins. A plugins folder meta can't read, and a plugin that doesn't answer `--meta-plugin-info` within 5 seconds, are skipped with a warning naming them. Project-local plugins are binaries: `--local` writes `.meta/plugins/.gitignore` (`*`) so they stay out of git as untracked files.

### Built-in Plugins

| Plugin | Description |
|--------|-------------|
| `git` | Clone, status, update, commit, SSH multiplexing |
| `project` | Project health checks and sync |
| `rust` | `meta cargo <cargo args>`: any cargo command in each repo that has a `Cargo.toml` |

### Writing Plugins

Plugins communicate via JSON over stdin/stdout. Any language works:

```bash
# Meta asks: "What can you do?"
meta-myplugin --meta-plugin-info
# → {"name": "myplugin", "version": "1.0", "commands": ["myplugin run"]}

# Meta asks: "Execute this command"
echo '{"command":"myplugin run","args":[],"projects":[...]}' | meta-myplugin --meta-plugin-exec
# → {"plan": {"commands": [{"dir": "./repo1", "argv": ["make", "lint"]}]}}
```

See [docs/plugin_development.md](docs/plugin_development.md) for the full guide.

---

## AI Integration

Meta is designed for AI agents from day one. Three integration points:

### MCP Server (26 tools)

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

**Tools include:** project listing, git operations (status/pull/push/commit/diff/branch), build/test, code search, file tree discovery, query DSL, impact analysis, and batch execution.

See [docs/mcp_server.md](docs/mcp_server.md) for full documentation.

### Claude Code Skills

```bash
meta init claude
```

Installs purpose-built skills that teach Claude Code how to work with multi-repo codebases — using `meta git` instead of `git`, committing or stashing before risky changes, filtering by tags, and understanding plugin interception. It writes only inside the repo, and prints the `claude plugin` commands that install the same skills for all your projects. See [docs/claude_code_skills.md](docs/claude_code_skills.md).

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
| `meta_core` | Shared infrastructure — `~/.meta/` directory, atomic JSON store |
| `meta_plugin_protocol` | Shared types for the plugin contract |
| `meta_git_cli` | Git plugin — clone, update, status, commit |
| `meta_git_lib` | Shared git library utilities |
| `meta_project_cli` | Project management plugin |
| `meta_rust_cli` | Rust/Cargo plugin |
| `meta_mcp` | MCP server for AI agent integration |

Each plugin is both a workspace member and a child repo in the `.meta.yaml` manifest — meta manages itself with itself.

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
  -c, --config <FILE>       Path to .meta.yaml config file
  -t, --tag <TAGS>          Filter by tag(s), comma-separated
  -i, --include <REPOS>     Only these repos (.meta.yaml name or path from the root)
  -e, --exclude <REPOS>     Skip these repos (.meta.yaml name or path from the root)
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
- [x] MCP server for AI agents (26 tools)
- [x] Multi-commit support (`meta git commit --edit`)
- [x] Plugin help system
- [x] Query DSL for advanced filtering
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
