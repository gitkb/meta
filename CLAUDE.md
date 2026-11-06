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
meta -r git update       # Pulls and clones nested meta repos too
meta -r project list     # Shows full tree
meta -r exec -- cargo test  # Runs across all nested repos
```

**What `meta` does:** Clones the repos a workspace lists, runs commands across them in parallel (`meta exec`), manages git worktrees across the entire workspace (`meta git worktree`), and provides project-level coordination.

## Contracts with other repos

Other repos run and read meta, so these are contracts: CLI arguments and option order, `--json` fields, exit codes, text a caller parses (`meta --dry-run git worktree create` output, `--metadata-only` in `meta git worktree list --help`), `.meta.yaml`, the `META_WORKTREES` variable, and the plugin protocol.

Before merging a change to one, find its callers and test the real pair:

1. Search every repo in the harmony checkout, scripts, workflows and skills included. Child repos are gitignored, so plain `rg` skips them, and it skips `.github` too: use `rg --no-ignore --hidden -g '!.git' -g '!**/.worktrees/**'`.
2. Additive change that callers can ignore (a new field or flag): say so in the PR.
3. Breaking change: put `!` in the PR title. List every caller repo in the PR body with its companion PR, or "none found" and the search you ran.
4. Each companion PR moves the caller's pin of the producer to this PR's head, so its CI runs the real pair. A caller with no pin runs against a build of this PR's head instead, and its PR shows the result. A fake `meta` script alone proves nothing.
5. Both PRs pass on that pair and merge together, meta first.
6. A failed bump of meta in a caller belongs to the author of the change it moved to, who fixes it or reverts it; for a bot bump, that means the author of the upstream change.

How-to and the known callers: the `stack-development` skill.

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
- `meta_rust_cli` - Rust plugin
- `loop_lib` - Command execution library

**Architecture:** Subprocess plugins initialize their own logger via `meta_plugin::run_plugin()` (the plugin SDK), inheriting `RUST_LOG` from the parent process.

## GitKB

This project uses GitKB for knowledge management.

@.kb/AGENTS.md
