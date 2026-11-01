# meta

[![Build Status](https://img.shields.io/github/actions/workflow/status/gitkb/meta/ci.yml?branch=main)](https://github.com/gitkb/meta/actions)
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
   meta --tag backend git pull
   meta --tag frontend,shared exec -- npm test
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

**2. Meta CLI** — The brain. Reads your meta config, applies filters (tags, include/exclude), discovers plugins, and routes commands. `meta exec -- <cmd>` hands a command to the loop engine as typed, never to a plugin; any other command `meta <word> ...` goes to the plugin file named `meta-<word>`, or fails as unrecognized.

**3. Plugins** — The specialists. They don't *execute* — they *plan*. A plugin receives a request and returns an execution plan (a list of directory/command pairs). The loop engine does the actual work. This means plugins are pure functions, and execution is always consistent.

```
User runs: meta --tag backend git status
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

Clone this repo, which holds all of meta's crates, then install with the
committed `Cargo.lock`:

```bash
git clone https://github.com/gitkb/meta && cd meta
make install
```

This installs `meta`, the `meta-git`, `meta-project` and `meta-cargo`
plugins, `meta-mcp` and `loop` into `~/.cargo/bin` (the crates in the
Makefile's `BIN_CRATES`, the binaries a release ships), replacing any
copies already there. To update, `git pull` and run `make install` again.
A checkout from before the crates moved into this repo, with `meta_cli/` and
the rest as separate clones, must run the snippet in
[#135](https://github.com/gitkb/meta/pull/135) first: a plain pull writes over
those clones.
Copies the install scripts put in `~/.local/bin` or `%USERPROFILE%\.meta\bin`
(their defaults) stay; remove them or put `~/.cargo/bin` first in `PATH`.
`agent` and `meta-plugins` in `.meta.yaml` are not needed for the build;
`meta git` commands report them as missing until you clone them too.

---

## Configuration

Meta projects are configured by one YAML file at the top of the workspace: `.meta.yaml`. It is the only name meta looks for, in the current directory and then each one above it; `--config <FILE>` looks for another name the same way.

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

A token in a `repo` URL (`https://user:token@host/org/repo.git`) works, and meta's own messages show it as `***`, but it is stored in plain text in `.meta.yaml` and in the clone's `.git/config`; prefer a git credential helper, such as `gh auth setup-git`.

**Rules for names and values.** meta checks every entry when it reads the config and stops with an error that names the file and the project:

- `path` (the name when it is omitted) is where the project lives, anywhere on disk: relative to the config's directory (`../shared-lib` is fine), absolute, or through a symlink that leads elsewhere. It may not be the config's directory itself or one around it, lead into `.git`, or lie under `.worktrees` or a `worktrees_dir` inside the workspace. In a relative path `\` reads as `/`, and `./`, `//` and a trailing `/` are dropped. In a worktree set, a project outside the workspace sits at its name.
- A project name is usually one word; a nested alias such as `vendor/lib` is fine. It must be a plain relative path in normal form.
- Names use only ASCII letters, digits, space and `._-+@`, so one config names the same projects on every platform (no Unicode that macOS or Windows would treat as another name, no `C:`, `<>:"|?*` or short names like `PROGRA~1`). They have no trailing dot or space, are not Windows device names (`CON`, `NUL`, `COM1`, ...) or `.git`, and do not start with `.worktrees` or lie under the configured `worktrees_dir`.
- No two projects share a path, and no project's path lies inside another's. Paths are compared after following existing symlinks, and without regard to case (some filesystems ignore it). Project names may not be equal or nested either, also without regard to case. A repo inside another repo goes in a nested meta config (`meta: true`).
- No name, path, repo, tag, `provides` or `depends_on` value is empty or contains a control character (such as an escape or a newline), a bidirectional-text control, or an invisible format character (such as a zero-width space).
- `repo` does not start with `-`.
- Tags contain no whitespace and no `,`.
- `worktrees_dir`, if set, may be anywhere (relative, `..`, absolute, or `.worktrees` a symlink to another disk), but not the config's directory itself, not inside a project and not in `.git`. `META_WORKTREES` overrides it.
- A key meta doesn't know at the top level, under `defaults` or under `worktree` is an error, so a misspelling isn't ignored. Project entries may carry keys of their own for other tools.
- `worktree.hooks` names only `post-create`, `post-destroy` and `post-prune`, and each value is a command string (`false` or `123` is refused, not run).

The same rules apply to every meta config that a recursive command reads, including one inside a cloned project: a refused nested config stops the command rather than silently leaving out that part of the tree.

YAML anchors and merge keys (`<<: *defaults`) work as YAML defines them.

**A project is acted on only if its folder is its own repository.** `meta exec` and the per-repo `meta git` commands run in each selected project that is a git repository of its own, and name the others, then exit 1: `web: not cloned (run 'meta git update')`, or `r1: not a git repository (folder exists, no .git)`. git started for a project never looks above the project's folder, so it can't reach the workspace repository around it: meta sets `GIT_CEILING_DIRECTORIES` (added to any you set) for every command it runs in a project, the root included. `meta git update` clones a missing project, or into an empty folder, and names a folder that holds something else. `meta project check` names every project that is missing, not a repository, or cloned from another remote, and exits 1. `--json` prints `{"problems": [{"name", "path", "state", "reason", "repo"?, "actual"?}]}` on stdout, `state` being `missing`, `not_a_repo` or `wrong_remote` (`actual`: the checkout's origin, `null` without one).

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

# Pull what's cloned and clone what's missing, nested meta repos included
meta git update -r

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
| `meta git update` | Fast-forward each cloned repo (`git pull --ff-only`) and clone each missing one; the workspace repo is pulled first, so projects added upstream are cloned too. A repo that can't fast-forward is named and the command exits 1; one on no branch or without an upstream is named and left alone. `-r` for nested meta repos |
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
meta --tag backend git pull
meta --tag frontend,shared exec -- npm test
```

### By Directory

```bash
meta --include api-service,web-app git status
meta --exclude legacy-app exec -- npm install
```

A value names a whole repo: its `.meta.yaml` name or its path from the workspace root (`.` for the root repo), never part of a path, so `api` does not select `api-legacy`. A value that names no repo is an error, so a typo never runs a command in a repo you meant to skip. Inside a worktree set the same holds: a repo sits at its project's path, so it is named by its `.meta.yaml` name or that path, from the set's root. A value starting with `./` is a path only; use it when a name is ambiguous (a value that names several repos is an error too).

**Filters:** `--include` and `--tag` each add repos: the ones `--include` names, and the ones with any of the `--tag` tags (with neither, every repo). `--exclude` then removes the repos it names. The root repo has no tags, so `--tag` alone never selects it; add it with `--include .`. A tag no project has is an error that lists the tags there are.

```bash
meta --tag backend --include . git checkout -b release   # the backend repos and the root
meta -r --tag cli-tools exec -- cargo build               # nested projects by their own tags
```

### Arguments

Meta runs every command as a program with its arguments, never through a shell. Each argument meta receives reaches the program unchanged, so quotes, spaces, `$()`, `;`, globs and `%` stay literal (`meta git log --format='%h %s'` works, and text pasted into a commit message can't run commands). Your own shell still parses the line before meta runs, so `"$(...)"` in double quotes runs there: pass untrusted text in single quotes. When you want a shell, ask for one: `meta exec -- sh -c 'npm ci && npm test'`. `meta git commit`, `clone`, `update` and `worktree` are meta's own commands with their own options; for git's native behavior, `meta exec -- git ...`. Every other `meta git` command, `status` included, passes its arguments to git.

Meta's own options go before the command: `meta --dry-run git update`, `meta --json git worktree list`. Everything after the command word belongs to the command, so `meta git pull --tag v1` gives `--tag v1` to git, and `meta exec --dry-run -- rm x` is refused with a tip to write `meta --dry-run exec -- rm x`. The commands that print data take their own `--json` too (`meta context --json`, `meta plugin list --json`, `meta git worktree list --json`, `meta project list --json`).

### Recursive Processing

```bash
meta --recursive git status
meta --recursive --depth 2 git pull
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

A plugin named `meta-<word>` serves `meta <word> ...`, and the first file found with that name wins. For a command meta starts only that file; if it doesn't answer `--meta-plugin-info` with its info within 5 seconds, the command fails naming it. `meta`, `meta help`, `meta --help` and `meta plugin list` start every plugin; there a plugins folder meta can't read, and a plugin that doesn't answer within 5 seconds, are skipped with a warning naming them. So is a file in `.meta/plugins` or `~/.meta/plugins` that can't be started, isn't executable, or answers with anything but its info; a `meta-*` program on PATH that can't be started or isn't a plugin is just left out, as it may be any program. Project-local plugins are binaries: `--local` writes `.meta/plugins/.gitignore` (`*`) so they stay out of git as untracked files.

### Built-in Plugins

| Plugin | Description |
|--------|-------------|
| `git` | Clone, status, update, commit, SSH multiplexing |
| `project` | Project health checks and sync |
| `cargo` | `meta cargo <cargo args>`: any cargo command in each repo that has a `Cargo.toml` |

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

Installs purpose-built skills that teach Claude Code how to work with multi-repo codebases — using `meta git` instead of `git`, committing or stashing before risky changes, filtering by tags, and understanding plugin interception. It writes only inside the workspace, in the folder with its `.meta.yaml` (`meta --dry-run init claude` shows what it would write), and prints the `claude plugin` commands that install the same skills for all your projects. See [docs/claude_code_skills.md](docs/claude_code_skills.md).

### JSON Output

Every command supports `--json` for structured, machine-readable output:

```bash
meta --json git status
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

All of these crates live in this repo as members of one Cargo workspace. `.meta.yaml` lists the separate repos `agent` and `meta-plugins`, so meta still manages its own workspace with itself.

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
Usage: meta [OPTIONS] [COMMAND]

Commands:
  context  Show workspace context summary
  exec     Run a command in each selected repo, as typed (no shell, no plugin)
  help     Print help for meta or for a command (meta help git worktree create)
  init     Initialize meta integrations
  plugin   Manage plugins

Options:
  -h, --help                          Print help, with the commands plugins add
      --add-aliases-to-global-looprc  Add shell aliases to the global .looprc file
  -c, --config <FILE>                 Read FILE instead of .meta.yaml, looked for in the current directory and the ones above it
  -e, --exclude <EXCLUDE>             Skip these repos: each value is a repo's .meta.yaml name or its whole path from the workspace root (. for the root), never part of one
  -i, --include <INCLUDE>             Run in these repos (and in those --tag selects): each value is a repo's .meta.yaml name or its whole path from the workspace root (. for the root), never part of one
      --json                          Output results in JSON format
  -s, --silent                        Enable silent mode
  -v, --verbose                       Enable verbose output
  -t, --tag <TAGS>                    Run in the repos with any of these tags, comma-separated (the root has none: add it with -i .)
  -r, --recursive                     Recursively process nested meta repos
      --depth <N>                     Maximum depth for recursive processing (default: unlimited)
      --dry-run                       Show what would change, and change nothing (commands that can't preview refuse it)
      --parallel                      Run commands in parallel
      --sequential                    Run commands sequentially (overrides config default)
      --primary                       Inside a worktree set, act on the workspace it was made from instead
      --strict                        Convert warnings to errors for all-or-nothing behavior (CI/automation)
  -V, --version                       Print version

meta's options go before the command (meta --dry-run git update); what follows the command belongs to it.
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
