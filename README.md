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
- [Start a Workspace](#start-a-workspace)
- [How It Works](#how-it-works)
- [Installation](#installation)
- [Configuration](#configuration)
- [Filtering](#filtering)
- [Plugins](#plugins)
- [AI Integration](#ai-integration)
- [Architecture](#architecture)
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

0. **Install meta** from GitHub main (needs [rustup](https://rustup.rs) and `git`; see [Installation](#installation)):

   ```bash
   git clone https://github.com/gitkb/meta && make -C meta install
   ```

1. **Clone a meta repository:**

   ```bash
   meta git clone https://github.com/org/meta-repo.git
   ```

   This clones the meta repo and all child repositories defined in its meta config.

2. **Run commands across all repos:**

   ```bash
   meta git status
   meta git pull
   meta exec -- npm install
   meta exec -- cargo test
   ```

3. **Filter by tags** (meta's options go before the command):

   ```bash
   meta --tag backend git pull
   meta --tag frontend,shared exec -- npm test
   ```

4. **Commit before risky changes:**

   ```bash
   meta git add -A && meta git commit -m "wip: before refactor"
   meta --dry-run exec -- <command>  # preview before running
   ```

`meta --help` lists every command, the ones plugins add included; `meta help <command>` (`meta help git worktree create`) shows one.

---

## Start a Workspace

With no meta repo yet, make one: a git repo holding a `.meta.yaml` that lists the others.

```bash
mkdir my-workspace && cd my-workspace && git init
cat > .meta.yaml <<'EOF'
projects:
  api: git@github.com:org/api.git
  web: git@github.com:org/web.git
EOF
printf 'api/\nweb/\n' > .gitignore   # the child repos keep their own history
meta git clone-missing               # clones api and web
```

Commit `.meta.yaml` and `.gitignore` and push the workspace repo; others then get everything with `meta git clone <url>`.

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

### From GitHub main (Recommended)

With [rustup](https://rustup.rs) and `git` installed:

```bash
git clone https://github.com/gitkb/meta && make -C meta install
```

This builds meta from the main branch with the committed `Cargo.lock` and
installs `meta`, the `meta-git`, `meta-project` and `meta-cargo` plugins,
`meta-mcp` and `loop` into `~/.cargo/bin` (the crates in the Makefile's
`BIN_CRATES`), replacing any copies already there, with your default
stable toolchain. `meta --version` names the commit it was built from
(as in `0.2.22 (git 1c96c7e 2026-11-03)`). Building also
needs a C compiler and linker and `make`, which most machines that build
Rust already have (Xcode Command Line Tools on macOS, `build-essential` or
your distro's equivalent on Linux, Visual Studio Build Tools on Windows,
where the command runs in Git Bash or WSL).

To update, run from the same folder: `git -C meta pull && make -C meta install`.

A checkout from before the crates moved into this repo, with `meta_cli/` and
the rest as separate clones, must run the snippet in
[#135](https://github.com/gitkb/meta/pull/135) first: a plain pull writes over
those clones.
`agent` and `meta-plugins` in `.meta.yaml` are not needed for the build;
`meta git` commands report them as missing until you clone them too.

### Release builds (lag main)

These install the latest published release, which can be well behind main:
commands and fixes made since the release (`meta-cargo`, for one) are missing
from it. Use them only where you can't build from source.

**macOS/Linux:**
```bash
curl -fsSL https://raw.githubusercontent.com/gitkb/meta/main/install.sh | bash
```

**Windows (PowerShell):**
```powershell
irm https://raw.githubusercontent.com/gitkb/meta/main/install.ps1 | iex
```

**Homebrew (macOS/Linux):**
```bash
brew install gitkb/tap/meta-cli
```

The scripts download the release archive for your platform together with
its published `.sha256` and check it before installing anything; a
download error, a missing checksum file or a mismatch stops the install.
`install.sh` needs `curl`, `tar` and `sha256sum` or `shasum`.

`VERSION`, `INSTALL_DIR` and `META_DOWNLOAD_URL` (see
[Environment Variables](#environment-variables)) go on `bash`, not on `curl`:

```bash
curl -fsSL https://raw.githubusercontent.com/gitkb/meta/main/install.sh | VERSION=0.2.22 bash
```
```powershell
$env:VERSION = "0.2.22"; irm https://raw.githubusercontent.com/gitkb/meta/main/install.ps1 | iex
```

The checksum comes from the same place as the archive, so it catches a
corrupted or truncated download, not a compromised source: use a mirror you
trust, over https.

Copies the install scripts put in `~/.local/bin` or `%USERPROFILE%\.meta\bin`
(their defaults) shadow or are shadowed by a build from main, depending on
`PATH`: remove them once you build from main, or put `~/.cargo/bin` first in
`PATH`.

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
| `repo` | No | Git repository URL (without one the project is local-only: `meta git update` doesn't clone it) |
| `path` | No | Custom clone path (defaults to project name) |
| `tags` | No | Array of tags for filtering |
| `meta` | No | Set `true` if this project contains a nested `.meta.yaml` |
| `provides` | No | Array of capabilities this project provides |
| `depends_on` | No | Array of project names or provided capabilities this depends on; an entry `- name: <sha or tag>` pins it (see [Pins](#pins)) |

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

**A project is acted on only if its folder is its own repository.** `meta exec` and the per-repo `meta git` commands run in each selected project that is a git repository of its own, and name the others. `meta exec` then exits 1: `web: not cloned (run 'meta git clone-missing')`, or `r1: not a git repository (folder exists, no .git)`. The per-repo `meta git` commands (`meta git pull`, `meta git status`, …) name a project that isn't cloned yet as a warning ("web is not cloned; `meta git clone-missing` clones it"), which fails the command only with `--strict`, and exit 1 for a folder that is not a repository. git started for a project never looks above the project's folder, so it can't reach the workspace repository around it: meta sets `GIT_CEILING_DIRECTORIES` (added to any you set) for every command it runs in a project, the root included. `meta git update` clones a missing project, or into an empty folder, and names a folder that holds something else. `meta project check` names every project that is missing, not a repository, or cloned from another remote, and exits 1. `--json` prints `{"problems": [{"name", "path", "state", "reason", "repo"?, "actual"?}]}` on stdout, `state` being `missing`, `not_a_repo` or `wrong_remote` (`actual`: the checkout's origin, `null` without one).

You can mix simple and extended formats in the same file.

### Environment Variables

| Variable | Effect |
|----------|--------|
| `META_DATA_DIR` | meta's own folder, `~/.meta` by default: installed plugins, the registry config |
| `META_WORKTREES` | Where worktree sets go, instead of `worktrees_dir` or `.worktrees` |
| `META_PLATFORM` | The platform `meta plugin install` downloads for (`linux-x64`, `darwin-arm64`, ...) |
| `RUST_LOG` | Debug logging on stderr: `RUST_LOG=debug meta git status` |
| `GIT_EDITOR`, `VISUAL`, `EDITOR` | The editor `meta git commit --edit` opens (the one git would, `core.editor` included) |
| `NO_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE` | Colour off, off (`0`), or on even into a pipe (`1`); see [Advanced Usage](docs/advanced_usage.md#colour) |
| `VERSION`, `INSTALL_DIR`, `META_DOWNLOAD_URL` | For the install scripts: the release (default `latest`), the target folder (`~/.local/bin`; `%USERPROFILE%\.meta\bin` on Windows), a mirror of `https://github.com/gitkb/meta/releases` |

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
meta git clone -r https://github.com/org/top-level.git

# Pull what's cloned and clone what's missing, nested meta repos included
meta git update -r
```

Keeping the clones to what the `.meta.yaml` files list:

| Command | What it does |
|---------|--------------|
| `meta git update` | Pull the workspace repo, fast-forward every cloned repo (`git pull --ff-only`), clone the missing ones |
| `meta git clone-missing` | Clone the projects the `.meta.yaml` files on disk list that aren't cloned; change no repo that is |
| `meta git clone-prune` | Remove the repos no `.meta.yaml` lists that hold nothing unpushed; keep and name the others |
| `meta git clone-sync` | `clone-missing`, then `clone-prune` |

Each takes `-r` (or meta's `--recursive`) for nested meta repos, and `--dry-run` before the command. `clone-missing`, `clone-prune` and `clone-sync` read the `.meta.yaml` files as they are on disk and pull nothing; they take `--include`, `--exclude` and `--tag` (the projects cloned, the meta repos searched for leftovers) and `--json` (`{"dry_run", "repos": [{"path", "action", "reason"?}]}`, `action` being `cloned`, `removed`, `kept` or `failed`; a removed repo's `reason` names the ignored files deleted with it). `clone-prune` removes a repo only when git shows nothing in it that isn't on a remote: no uncommitted or untracked files, stashes, commits on no remote, linked worktrees or repos inside it (bare ones too). Files git ignores (`.env`, `target/`) are deleted with it and named first, as `git worktree remove` deletes them; commits only a reflog holds are not kept.

```bash
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

### Pins

A `depends_on` entry can pin the dependency at the revision this project builds against: a full commit SHA or a tag, never a branch or a short SHA.

```yaml
projects:
  desktop:
    repo: git@github.com:org/desktop.git
    depends_on:
      - app-atc                                              # any revision
      - open-source/gitkb/core: c2630d5bf95e6553d71fdc61199759024293ecec
      - open-source/app-sdk: app-sdk-v0.1.39                  # a tag
```

The pin belongs to the project that depends, as a Chromium `DEPS` entry or a git submodule's commit does, so it is written in the `.meta.yaml` that lists that project (for nested meta repos too), and another project may pin the same dependency elsewhere. Change it by editing the entry.

Pins apply in worktree sets, which are made for one task. The workspace's own checkouts serve every project that depends on them, so `meta git update` there keeps following their branches.

- `meta git worktree create` and `add` start a new branch of a repo at its pin when a repo of the set pins it: `core -> fix (new from c2630d5bf95e (pinned by clients/desktop))`. A pin the repo lacks is fetched from origin first (only that commit or tag; a shallow clone stays shallow). A commit-ish on the command line starts every new branch instead, and a branch that exists already is checked out as it is; both are said. When two repos of the set pin one repo at different revisions, or origin lacks a pin, no set is made and the error names them.
- `meta git update` in a set moves each pinned repo to its pin, after a pin changes on the set's branch. A clean checkout on the branch the set made for it, or on a detached HEAD, is fast-forwarded to the pin, or moved there when every commit on it is on a remote, a tag or another branch (the rule `meta git worktree remove` deletes the branch under). Anything else is left where it is, named with the reason and the fix, and the command exits 1: uncommitted changes to tracked files, a merge or rebase under way, a branch the set didn't make, or a commit nothing else holds. `meta --dry-run git update` says what it would do.
- `meta git worktree status` says whether each pinned repo is at its pin (`--json`: `"pin": {"rev", "by", "at_pin"}`, `at_pin` `null` when the pin isn't fetched yet).

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

Meta's own options go before the command: `meta --dry-run git update`, `meta --json git worktree list`. Everything after the command word belongs to the command, so `meta git pull --tag v1` gives `--tag v1` to git, and `--dry-run` after `meta exec` is refused with a tip to write `meta --dry-run exec -- rm x`. The commands that print data take their own `--json` too (`meta context --json`, `meta plugin list --json`, `meta git worktree list --json`, `meta project list --json`).

### Recursive Processing

```bash
meta --recursive git status
meta --recursive --depth 2 git pull
```

---

## Plugins

Plugins are standalone executables that extend meta: a program named `meta-<word>` serves `meta <word> ...`. meta finds them in the workspace's `.meta/plugins/` folders, then `~/.meta/plugins/` (where `meta plugin install <name>` puts them; `--local` installs into the workspace), then PATH; the first file with the name wins. See [docs/plugin_development.md](docs/plugin_development.md#discovery-and-naming) for the details.

A **meta plugin** (`meta-<word>`) adds or adapts `meta <word> ...`. A **tool's own extension** (`git-<name>`, `cargo-<name>`) is just part of that tool, and runs through meta as typed: with `git-<name>` on PATH, `meta git <name>` runs `git <name>` in each repo. `meta plugin list` shows both: the plugins meta found, then the extensions on PATH of each tool a plugin runs as typed.

Help says who answers each command. `meta --help` lists each plugin's own commands under its word. `meta git --help` lists the git commands meta adapts and says that every other one runs as typed. A command's help starts with the same answer:

```
$ meta git clone --help
`meta git clone` is adapted by meta-git: meta's own command, not git run as typed.
...
Git options: of git clone's own, only --depth, given to each clone.

$ meta git pull --help
`meta git pull` runs as typed (`git pull`) in each selected repo.
...
For git's own options: git pull --help
```

### Built-in Plugins

| Plugin | Description |
|--------|-------------|
| `git` | `clone`, `clone-missing`, `clone-prune`, `clone-sync`, `update`, `commit` and `worktree` sets; any other `meta git <command>` runs git in each repo |
| `project` | `list`, `check` (is every project cloned from its remote?), `dependents` |
| `cargo` | `meta cargo <cargo args>`: any cargo command in each repo that has a `Cargo.toml` |

`meta plugin search` and `meta plugin install <name>` read the plugin registry, `gitkb/meta-plugins` by default. To use others, list them in `~/.meta/config.yaml` (in `META_DATA_DIR` if set); `install` takes a plugin from the first one that has it:

```yaml
registries:
  - https://raw.githubusercontent.com/gitkb/meta-plugins/main
  - https://plugins.example.com
```

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

**Tools include:** project listing, git operations (status/pull/push/commit/diff/branch), build/test, code search, file tree discovery, a query language for repo state (`dirty:true AND tag:backend`), impact analysis, and batch execution.

See [docs/mcp_server.md](docs/mcp_server.md) for full documentation.

### Claude Code Skills

```bash
meta init claude
```

Installs purpose-built skills that teach Claude Code how to work with multi-repo codebases — using `meta git` instead of `git`, committing or stashing before risky changes, filtering by tags, and understanding plugin interception. It writes only inside the workspace, in the folder with its `.meta.yaml`: the skills go in `.meta/skills/` and are linked into `.claude/skills/`, `.agents/skills/` and the skills folder of each other agent whose folder exists, such as `.cursor/` (`meta --dry-run init claude` shows what it would write). It also prints the `claude plugin` commands that install the same skills for all your projects. See [docs/claude_code_skills.md](docs/claude_code_skills.md).

### JSON Output

`meta --json` before a command prints its result, or its error, as JSON on stdout (help and `meta init claude` stay text):

```bash
meta --json git status
meta --json exec -- git rev-parse HEAD
meta project list --json
```

See [Advanced Usage](docs/advanced_usage.md#json-output-mode) for the format.

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
- [x] Query DSL for agents (MCP `meta_query_repos`)
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
