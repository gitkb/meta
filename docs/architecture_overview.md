# Architecture Overview

This document provides a comprehensive overview of the `meta` CLI platform's architecture, including its core components, plugin system, and data flow.

## Table of Contents

- [System Overview](#system-overview)
- [Workspace Structure](#workspace-structure)
- [Core Components](#core-components)
- [Plugin System](#plugin-system)
- [Command Execution Flow](#command-execution-flow)
- [Data Models](#data-models)
- [Key Modules](#key-modules)
- [Integration Points](#integration-points)
- [See Also](#see-also)

---

## System Overview

`meta` is a modular, extensible multi-repository management CLI built in Rust. It follows a workspace-based architecture with 10 member crates:

```
meta/
├── meta_cli/              # Main CLI entry point (binary: meta)
├── meta_core/             # Core shared types and utilities
├── meta_plugin_protocol/  # Plugin communication protocol
├── meta_git_cli/          # Git plugin (binary: meta-git)
├── meta_git_lib/          # Git library utilities
├── meta_project_cli/      # Project management plugin (binary: meta-project)
├── meta_rust_cli/         # Rust/Cargo plugin (binary: meta-cargo)
├── meta_mcp/              # MCP server for AI integration (binary: meta-mcp)
├── loop_cli/              # Standalone loop CLI (binary: loop)
└── loop_lib/              # Core loop engine library
```

## Workspace Structure

The Cargo workspace enables shared dependencies and coordinated builds:

```toml
[workspace]
members = [
    "loop_cli",
    "loop_lib",
    "meta_cli",
    "meta_core",
    "meta_git_cli",
    "meta_git_lib",
    "meta_mcp",
    "meta_plugin_protocol",
    "meta_rust_cli",
    "meta_project_cli",
]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"
repository = "https://github.com/gitkb/meta"
```

## Core Components

### meta_cli

The main CLI entry point responsible for:
- Argument parsing (via clap)
- Configuration loading (`.meta.yaml`)
- Plugin discovery and dispatch
- Filter application (tags, include, exclude)
- Query DSL processing
- Dependency graph analysis

Key modules:
- `main.rs` - Entry point, argument parsing
- `meta_core::config` - Configuration loading and parsing
- `subprocess_plugins.rs` - Plugin discovery and execution
- `query.rs` - Query DSL implementation
- `dependency_graph.rs` - Dependency tracking and analysis
- `registry.rs` - Plugin registry client
- `init.rs` - Claude Code skills installation

### meta_core

Shared infrastructure for `~/.meta/` directory management:
- `data_dir` - Locate and create the `~/.meta/` data directory and namespaced files
- `store` - Atomic JSON read/write

### meta_plugin_protocol

Shared plugin protocol types for meta subprocess plugins:
- Defines communication protocol between the meta CLI host and subprocess plugins
- `PluginInfo` - Metadata about a plugin (name, version, commands)
- `PluginRequest` - Host-to-plugin request format (JSON on stdin)
- `PlanResponse` - Plugin-to-host response: the commands to run
- `PluginHelp` - Structured help information for plugin commands

### loop_lib

The core execution engine that runs commands across directories:
- Directory expansion and filtering
- Parallel/sequential execution
- Output aggregation
- Error handling and reporting

Used by both `meta_cli` and `loop_cli`.

### loop_cli

Standalone CLI for the loop engine:
- Direct command execution without meta overhead
- `.looprc` configuration support
- Useful for non-meta multi-directory operations

### meta_git_cli

Git plugin providing:
- Queue-based recursive cloning
- SSH multiplexing setup
- Standard git operation passthrough

### meta_git_lib

Shared git utilities:
- Repository status queries
- Branch management
- Clone operations

### meta_mcp

MCP (Model Context Protocol) server exposing 26 tools:
- Core operations (list, exec, config)
- Git operations (status, pull, push, commit, etc.)
- Build/test operations
- Discovery tools (search, file tree)
- AI-dominance features (query, impact analysis, batch execution)

### meta_project_cli

Project management plugin:
- Project health checks
- Configuration sync
- Missing repo detection

### meta_rust_cli

Rust/Cargo plugin (`meta cargo <cargo args>`):
- Runs any cargo command, as given, in each selected repo that has a `Cargo.toml`
- Lists the repos it skips (no `Cargo.toml`)

## Plugin System

### Discovery

Plugins are discovered from:
1. `.meta/plugins/` next to the nearest meta config at or above the current directory, then next to each parent meta config that lists that directory as a project (the same workspace), nearest first. A parent that doesn't list it is another workspace and is not read. Inside a worktree set meta made, a checkout's own `.meta/plugins/` comes first, then those of the checkout it stands for (the one it was made from) and its workspace. `meta plugin install <name> --local` installs into the nearest one.
2. `~/.meta/plugins/` (moved by `META_DATA_DIR`), where `meta plugin install <name>` installs.
3. Executables named `meta-*` on PATH.

A plugin named `meta-<word>` serves `meta <word> ...`, and the first file found with that name wins. For a command meta starts only that file; if it doesn't answer `--meta-plugin-info` with its info within 5 seconds, the command fails naming it. `meta`, `meta help`, `meta --help` and `meta plugin list` start every plugin; there a plugins folder meta can't read, and a plugin that doesn't answer within 5 seconds, are skipped with a warning naming them. So is a file in `.meta/plugins` or `~/.meta/plugins` that can't be started, isn't executable, or answers with anything but its info; a `meta-*` program on PATH that can't be started or isn't a plugin is just left out, as it may be any program. Project-local plugins are binaries: `--local` writes `.meta/plugins/.gitignore` (`*`) so they stay out of git as untracked files.

### Protocol

Plugins communicate via JSON over stdin/stdout:

**Plugin Info Request:**
```bash
meta-git --meta-plugin-info
```

**Response:**
```json
{
  "name": "git",
  "version": "0.1.0",
  "description": "Git operations for meta repositories",
  "commands": ["git clone", "git status", "git update", "git commit"]
}
```

**Execution Request:**
```bash
echo '{"command":"git status","args":[],"projects":[...],"cwd":"..."}' | meta-git --meta-plugin-exec
```

**Response** (a plan meta runs through `loop_lib`; a plugin may instead print plain text, which meta shows as is):
```json
{
  "plan": {
    "commands": [
      {"dir": "./api", "argv": ["git", "status"]}
    ],
    "parallel": false
  }
}
```

### Command Routing

When you run `meta <command>`:

1. **If `meta exec`** - Run the command as typed via `loop_lib` in all (filtered) repos, never through a plugin
2. **Check for plugin** - Is there a plugin file named `meta-<word>` for the first word (`meta-git` for `meta git ...`, `meta-cargo` for `meta cargo ...`)?
3. **If plugin exists** - Start only that plugin, with the project list and filters
4. **Otherwise** - Show "unrecognized command" error

**Important:** Bare commands like `meta npm install` are not supported. You must either:
- Use a plugin command: `meta git status`, `meta cargo build`
- Use explicit exec: `meta exec -- npm install`

Special case commands (like `meta git clone`) are fully handled by the plugin rather than passed through to all repos. Because exec never reaches a plugin, `meta exec -- git ...` is plain git in each repo (no meta-git handling, e.g. no prompt-free remote commands) and `meta exec -- git clone` doesn't bootstrap a workspace; use `meta git ...` for those.

## Command Execution Flow

```
User: meta --tag backend git status
              │
              ▼
    ┌─────────────────────┐
    │     meta_cli        │
    │  (parse arguments)  │
    └─────────────────────┘
              │
              ▼
    ┌─────────────────────┐
    │  Load Configuration │
    │    (.meta.yaml)     │
    └─────────────────────┘
              │
              ▼
    ┌─────────────────────┐
    │   Apply Filters     │
    │  (tags, include,    │
    │   exclude)          │
    └─────────────────────┘
              │
              ▼
    ┌─────────────────────┐
    │  Plugin Discovery   │
    │  (find meta-git)    │
    └─────────────────────┘
              │
              ▼
    ┌─────────────────────┐
    │  Plugin Execution   │
    │  (JSON over stdio)  │
    └─────────────────────┘
              │
              ▼
    ┌─────────────────────┐
    │  Output Aggregation │
    │  (human or JSON)    │
    └─────────────────────┘
```

For arbitrary commands via `meta exec`:

```
User: meta --tag frontend exec -- npm install
              │
              ▼
    ┌─────────────────────┐
    │     meta_cli        │
    │  (parse arguments,  │
    │   detect 'exec')    │
    └─────────────────────┘
              │
              ▼
    ┌─────────────────────┐
    │   Apply Filters     │
    │  (--tag frontend)   │
    └─────────────────────┘
              │
              ▼
    ┌─────────────────────┐
    │    loop_lib::run    │
    │  (parallel/serial   │
    │   execution)        │
    └─────────────────────┘
              │
              ▼
    ┌─────────────────────┐
    │  Per-Directory Exec │
    │  (npm install in    │
    │   each repo)        │
    └─────────────────────┘
```

**Note:** Bare commands like `meta npm install` are **not supported**. Use `meta exec -- <command>` for arbitrary commands, or use a plugin command like `meta git status`.

## Data Models

### Project Configuration

```rust
pub struct MetaConfig {
    pub projects: HashMap<String, ProjectEntry>,
}

pub enum ProjectEntry {
    Simple(String),  // Just URL
    Extended {
        repo: String,
        path: Option<String>,
        tags: Option<Vec<String>>,
        provides: Option<Vec<String>>,
        depends_on: Option<Vec<String>>,
    },
}
```

### Query Condition

```rust
pub enum QueryCondition {
    Dirty(bool),
    Branch(String),
    Tag(String),
    Ahead(bool),
    Behind(bool),
    ModifiedIn(Duration),
    Language(String),
}

pub struct Query {
    pub conditions: Vec<QueryCondition>,  // AND-combined
}
```

### Dependency Graph

```rust
pub struct DependencyGraph {
    pub projects: HashMap<String, ProjectDependencies>,
}

pub struct ProjectDependencies {
    pub provides: Vec<String>,
    pub depends_on: Vec<String>,
}
```

## Key Modules

| Module | Purpose |
|--------|---------|
| `meta_cli/src/main.rs` | CLI entry, arg parsing |
| `meta_core/src/config.rs` | Config loading |
| `meta_cli/src/subprocess_plugins.rs` | Plugin discovery/execution |
| `meta_cli/src/query.rs` | Query DSL |
| `meta_cli/src/dependency_graph.rs` | Dependency analysis |
| `meta_cli/src/registry.rs` | Plugin registry |
| `meta_cli/src/init.rs` | Skills installation |
| `meta_core/src/lib.rs` | Shared infrastructure (~/.meta/) |
| `meta_plugin_protocol/src/lib.rs` | Plugin protocol types |
| `loop_lib/src/lib.rs` | Core loop engine |
| `meta_git_cli/src/lib.rs` | Git plugin logic |
| `meta_mcp/src/main.rs` | MCP server |

## Integration Points

### meta_cli ↔ loop_lib

The main CLI uses loop_lib for directory iteration and command execution when `meta exec` is used. This is the only path to run arbitrary commands across repos—bare commands are not supported.

### meta_cli ↔ subprocess_plugins

Plugin manager discovers, validates, and executes plugins via subprocess communication.

### meta_cli ↔ query

Query module parses DSL strings and filters projects by git state.

### meta_cli ↔ dependency_graph

Dependency graph enables impact analysis and topological sorting for build ordering.

### meta_mcp ↔ All

MCP server exposes all functionality to AI agents via JSON-RPC protocol.

### meta_git_cli ↔ meta_git_lib

Git plugin uses shared library for repository operations.

### Plugins ↔ meta_plugin_protocol

All plugins (git, project, rust) use `meta_plugin_protocol` for standardized host communication. This ensures consistent request/response formats across the plugin ecosystem.

### meta_cli ↔ meta_core

The main CLI uses `meta_core` for managing the `~/.meta/` data directory and atomic JSON storage.

## See Also

- [Plugin Development Guide](plugin_development.md)
- [Advanced Usage Guide](advanced_usage.md)
- [MCP Server Documentation](mcp_server.md)
- [FAQ / Troubleshooting](faq_troubleshooting.md)
