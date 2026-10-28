# Vision and Architecture Plan for meta

## Overview

`meta` is a **powerful, extensible multi-repository management CLI** built in Rust. It enables engineers to **run any command across many repositories** with ease, and extend functionality via a flexible plugin system.

**Current Status:** Production-ready (v0.1.x) with core features complete.

---

## Implementation Status

### Completed Features

| Feature | Status | Notes |
|---------|--------|-------|
| Core CLI | ✅ Done | Run commands across repos |
| Loop engine | ✅ Done | Directory iteration, filtering |
| JSON output (`--json`) | ✅ Done | Scripting support |
| Project tags (`--tag`) | ✅ Done | Filter by tags |
| YAML configuration | ✅ Done | `.meta.yaml` / `.meta.yml` (one config per directory) |
| Nested meta repos (`--recursive`) | ✅ Done | Process nested repos |
| Subprocess plugin system | ✅ Done | JSON over stdin/stdout |
| Plugin discovery | ✅ Done | `.meta/plugins/` of the workspace's meta dirs, `META_DATA_DIR`, PATH |
| Plugin help system | ✅ Done | Structured help info |
| Git plugin | ✅ Done | clone, status, update, commit, setup-ssh |
| Project plugin | ✅ Done | check, sync, update |
| Rust plugin | ✅ Done | cargo build, cargo test |
| Multi-commit support | ✅ Done | Per-repo commit messages |
| MCP server | ✅ Done | 26 tools for AI agents |
| GitHub Actions release | ✅ Done | All platforms |
| Homebrew formula | ✅ Done | `meta-cli` to avoid conflicts |
| Install script | ✅ Done | Cross-platform |

---

## Architecture

### High-Level Design

```
┌─────────────────────────────────────────────────────────────┐
│                        meta CLI                              │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────────────┐ │
│  │ Parsing │→ │ Config  │→ │ Filter  │→ │ Plugin Manager  │ │
│  │ (clap)  │  │ (.meta.yaml) │  │ (tags)  │  │ (subprocess)    │ │
│  └─────────┘  └─────────┘  └─────────┘  └─────────────────┘ │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│                      Loop Engine                             │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │ Directory   │→ │ Filter      │→ │ Parallel Execution  │  │
│  │ Discovery   │  │ Application │  │ (per directory)     │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│                     Plugins (Subprocess)                     │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌─────────────┐  │
│  │ meta-git │  │ meta-    │  │ meta-    │  │ Custom      │  │
│  │          │  │ project  │  │ rust     │  │ Plugins     │  │
│  └──────────┘  └──────────┘  └──────────┘  └─────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### Crate Structure

```
meta/                          # Meta repository (this repo)
├── meta_cli/                  # Main CLI binary
├── meta_git_cli/              # Git plugin binary
├── meta_git_lib/              # Shared git library
├── meta_project_cli/          # Project plugin binary
├── meta_rust_cli/             # Rust plugin binary
├── meta_mcp/                  # MCP server binary
├── loop_cli/                  # Loop engine CLI
└── loop_lib/                  # Core loop engine library
```

### Plugin Protocol

Plugins communicate with meta via subprocess execution and JSON:

**Discovery:**
```bash
meta-git --meta-plugin-info
```

**Response:**
```json
{
  "name": "git",
  "version": "0.1.0",
  "commands": ["git clone", "git status", "git update", "git commit"],
  "description": "Git operations for meta repositories",
  "help": {
    "usage": "meta git <command> [args...]",
    "commands": {
      "clone": "Clone a meta repository and all child repos",
      "status": "Show git status for all repos",
      "update": "Pull latest changes and clone missing repos",
      "commit": "Commit changes with per-repo messages"
    },
    "examples": [
      "meta git clone https://github.com/org/meta-repo.git",
      "meta git status",
      "meta git commit --edit"
    ],
    "note": "To run raw git commands: meta exec -- git <command>"
  }
}
```

**Execution:**
```bash
echo '{"command":"status","args":[],"projects":["a","b"],"cwd":"/path","options":{}}' | meta-git --meta-plugin-exec
```

---

## MCP Server (AI Integration)

The MCP server exposes 26 tools for AI agent integration:

### Core Tools
- `meta_list_projects` - List all projects in workspace
- `meta_exec` - Execute command across repos
- `meta_get_config` - Get meta configuration
- `meta_get_project_path` - Get absolute path for project

### Git Tools
- `meta_git_status`, `meta_git_pull`, `meta_git_push`, `meta_git_fetch`
- `meta_git_diff`, `meta_git_branch`, `meta_git_add`, `meta_git_commit`
- `meta_git_checkout`, `meta_git_multi_commit`

### Build Tools
- `meta_detect_build_systems` - Detect Cargo/npm/make per repo
- `meta_run_tests` - Run tests across repos
- `meta_build`, `meta_clean` - Build/clean projects

### Discovery Tools
- `meta_search_code` - Search code across all repos
- `meta_get_file_tree` - Get file tree for repos
- `meta_list_plugins` - List installed plugins

### AI Features
- `meta_query_repos` - Query repos by state/criteria
- `meta_workspace_state` - Current workspace state summary
- `meta_analyze_impact` - Impact analysis for changes
- `meta_execution_order` - Topological sort for builds
- `meta_batch_execute` - Batch command execution

---

## Future Roadmap

### Near-Term Priorities

1. **Query DSL** - Advanced filtering with state-aware queries
   - `meta query "dirty:true AND tag:backend"`
   - `meta query "modified_after:24h AND branch:main"`

2. **Dependency Tracking** - Understand project relationships
   - Extended `.meta.yaml` schema with `provides`/`depends_on`
   - Impact analysis before changes
   - Topological sort for build ordering

3. **Documentation** - Comprehensive guides
   - MCP tools reference
   - Plugin development guide
   - Agent workflow patterns

### Long-Term Vision

1. **Smart Execution Engine**
   - Dependency-aware ordering
   - Adaptive parallelism
   - Automatic retry with backoff
   - Failure isolation

2. **GUI Development**
   - Visual project management
   - Dependency graph visualization
   - Real-time status monitoring

3. **Ecosystem Expansion**
   - Plugin registry/marketplace
   - Community plugins
   - Cloud integrations

---

## Why meta Beats Alternatives

### vs Git Submodules
| Problem | meta's Advantage |
|---------|------------------|
| Detached HEAD complexity | Simple linear model - all repos are peers |
| Nested .git state issues | No nested state to track |
| No batch operations | Parallel execution built-in |

### vs Lerna/Nx/Turborepo
| Limitation | meta's Advantage |
|-----------|------------------|
| JS/TS ecosystem only | Language-agnostic by design |
| Complex dependency graphs | Simple repos + tags model |
| Build-centric philosophy | Git-first, builds are plugins |
| Heavy configuration | Minimal .meta.yaml config |

### meta's Unique Position
1. **Conceptual simplicity** - Easy mental model for users and AI agents
2. **Language agnostic** - Plugins in any language
3. **MCP-native** - Designed for AI from day one
4. **Git-centric** - Build on universally known concepts

---

## Key Design Decisions

### Why Subprocess Plugins (not Dynamic Libraries)

The original vision called for compiled Rust dynamic libraries (dlopen). We chose subprocess-based plugins instead because:

1. **Simplicity** - No ABI compatibility concerns
2. **Language agnostic** - Plugins can be written in any language
3. **Safety** - Plugins run in separate processes
4. **Debugging** - Easier to test and debug plugins independently
5. **Distribution** - Plugins are standalone executables

### Why JSON Protocol

1. **Universal** - Every language has JSON support
2. **Debuggable** - Human-readable for troubleshooting
3. **Extensible** - Easy to add new fields without breaking compatibility
4. **Typed** - Can use serde for strong typing in Rust

---

## Reference

- [README.md](../README.md) - User documentation
- [docs/mcp_server.md](../docs/mcp_server.md) - MCP server reference
- [docs/plugin_development.md](../docs/plugin_development.md) - Plugin development guide
