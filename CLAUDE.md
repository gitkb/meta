## Project Layout

All of meta's crates (`meta_cli/`, `meta_core/`, `loop_lib/` and the rest) are folders in this one git repo and members of the root Cargo workspace (`Cargo.toml`). A change across crates is one commit and one PR here. A new crate is a new folder added to the workspace `members`.

The repo is also a meta workspace: `.meta.yaml` lists two separate repos, `agent` and `meta-plugins`, which are cloned into gitignored folders and keep their own history. Neither is needed to build the workspace.

**Project config (`.meta.yaml`):**
```yaml
projects:
  agent:
    repo: git@github.com:gitkb/agent.git
  meta-plugins:
    repo: git@github.com:gitkb/meta-plugins.git
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
meta git update -r       # Pulls and clones nested meta repos too
meta project list -r     # Shows full tree
meta -r exec -- cargo test  # Runs across all nested repos
```

**What `meta` does:** Clones the repos a workspace lists, runs commands across them in parallel (`meta exec`), manages git worktrees across the entire workspace (`meta git worktree`), and provides project-level coordination.

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
RUST_LOG=meta_git_cli::ssh=debug meta git push
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
