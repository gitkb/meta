## Project Architecture: Meta-Repo (NOT a monorepo)

This is a **meta-repo** — a workspace of independent git repositories managed by the `meta` CLI tool. Each directory listed in `.meta` (the project config) is a **separate git repo** with its own remote, commits, and history.

**Critical rules:**
- NEVER assume this is a monorepo. There is no single shared git history.
- Each workspace member (e.g., `meta_cli/`, `meta_core/`, `loop_lib/`) is its own git repo cloned from its own GitHub remote.
- The root `.gitignore` ignores all child repos because they are NOT part of the parent repo.
- The root `Cargo.toml` defines a Rust workspace for local development convenience, but each member builds and publishes independently.
- When creating a new crate/package, it MUST be initialized as a separate git repo, pushed to GitHub under `gitkb/`, and added to both `.meta` (projects config) and `.gitignore`.

**Project config (`.meta.yaml`):**
```yaml
projects:
  meta_cli:
    repo: git@github.com:gitkb/meta_cli.git
  meta_core:
    repo: git@github.com:gitkb/meta_core.git
```

**Nested meta repos:** Use `meta: true` when a child project contains its own `.meta.yaml`:
```yaml
projects:
  open-source:
    repo: git@github.com:org/open-source.git
    meta: true  # This directory has its own .meta.yaml
```

This enables recursive operations:
```bash
meta git update          # Clones nested meta repos automatically
meta project list -r     # Shows full tree
meta exec -r cargo test  # Runs across all nested repos
```

**What `meta` does:** Clones all child repos, runs commands across them in parallel (`meta exec`), manages git worktrees across the entire workspace (`meta git worktree`), and provides project-level coordination.

## Logging & Debugging

The meta CLI uses the `log` crate with `env_logger`. Use `RUST_LOG` to control debug output:

```bash
# Debug the host CLI
RUST_LOG=meta=debug meta git push

# Debug the git plugin (subprocess)
RUST_LOG=meta_git_cli=debug meta git push

# Debug everything
RUST_LOG=debug meta git push

# Debug specific modules
RUST_LOG=meta_git_cli::ssh_setup=debug meta git push
```

**Crate names for RUST_LOG:**
- `meta` - Host CLI (meta_cli)
- `meta_git_cli` - Git plugin
- `meta_project_cli` - Project plugin
- `meta_rust_cli` - Rust plugin
- `meta_git_lib` - Git library
- `loop_lib` - Command execution library

**Architecture:** Subprocess plugins initialize their own logger via `meta_plugin_protocol::run_plugin()`, inheriting `RUST_LOG` from the parent process.

## GitKB

This project uses GitKB for knowledge management.

@.kb/AGENTS.md
