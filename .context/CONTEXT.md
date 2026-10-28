# Context for meta

## Who You Are

You are an expert Rust developer specializing in high-performance, cross-platform CLI tools, plugin architectures, and extensible developer tooling.

## Project Vision

`meta` is a **powerful, extensible multi-repository management CLI** built in Rust. It enables engineers to **run any command across many repositories** with ease, and extend functionality via a flexible plugin system.

**Current Status:** Production-ready with all core features implemented.

---

## Key Features (Implemented)

### Core CLI
- Run any command across multiple repositories
- Project tags for filtering (`--tag backend,api`)
- YAML and JSON configuration support (`.meta`, `.meta.json`, `.meta.yaml`, `.meta.yml`)
- Nested meta repos with `--recursive` flag
- JSON output mode for scripting (`--json`)
- Directory filtering (`--include`, `--exclude`)

### Plugin System
- **Subprocess-based plugins** - executables that communicate via JSON over stdin/stdout
- Plugin auto-discovery from the workspace's `.meta/plugins/` directories, `META_DATA_DIR` and system PATH
- Built-in plugins: `git`, `project`, `rust`
- Structured help system with usage, commands, examples

### MCP Server (AI Integration)
- 26 tools for AI agent integration via Model Context Protocol
- Core, Git, Build, Discovery, and AI-specific tools
- Enables autonomous multi-repo operations by AI agents

### Distribution
- Cross-platform binaries (macOS x64/arm64, Linux x64/arm64, Windows x64)
- Homebrew formula (`brew install harmony-labs/tap/meta-cli`)
- Install script for quick setup
- GitHub Actions release workflow

---

## Key Principles

- Write clear, idiomatic, and efficient Rust code
- Design modular, reusable libraries following Rust best practices
- Create intuitive, powerful CLI interfaces (using `clap`)
- Prioritize cross-platform compatibility and performance
- Use expressive naming and idiomatic Rust conventions
- Leverage Rust's type system and ownership for safety and concurrency
- Implement robust error handling (`Result`, `Option`, `thiserror`, `anyhow`)
- Provide clear, helpful error messages and documentation

---

## Architecture

### Crate Structure

| Crate | Purpose |
|-------|---------|
| `meta_cli` | Main CLI binary and plugin orchestration |
| `meta_git_cli` | Git plugin (clone, status, update, commit, setup-ssh) |
| `meta_git_lib` | Shared git library used by plugins and MCP |
| `meta_project_cli` | Project management plugin |
| `meta_rust_cli` | Rust/Cargo plugin |
| `meta_mcp` | MCP server for AI agent integration |
| `loop_cli` | Loop engine CLI |
| `loop_lib` | Core loop engine library |

### Plugin Protocol

Plugins are subprocess executables that respond to:
- `--meta-plugin-info` → Returns JSON with name, version, commands, description, help
- `--meta-plugin-exec` → Receives JSON request on stdin, executes command

```json
// Plugin Info Response
{
  "name": "git",
  "version": "0.1.0",
  "commands": ["git clone", "git status", "git update"],
  "description": "Git operations for meta repositories",
  "help": {
    "usage": "meta git <command> [args...]",
    "commands": {"clone": "Clone a meta repository"},
    "examples": ["meta git clone <url>"],
    "note": "Optional note"
  }
}
```

---

## Configuration

### Extended Format (Recommended)
```yaml
projects:
  api-service:
    repo: git@github.com:org/api-service.git
    path: services/api  # Optional custom path
    tags: [backend, rust]
  web-app:
    repo: git@github.com:org/web-app.git
    tags: [frontend, typescript]
```

### Simple Format (Legacy)
```json
{
  "projects": {
    "api-service": "git@github.com:org/api-service.git",
    "web-app": "git@github.com:org/web-app.git"
  }
}
```

One config file per directory: two of `.meta`, `.meta.json`, `.meta.yaml`, `.meta.yml` in one directory is an error.

---

## Reference

For roadmap and future plans, see [VISION_PLAN.md](./VISION_PLAN.md).
