---
name: meta-plugins
description: How the meta plugin system intercepts and enhances commands.
---

# Meta Plugins Skill

Meta uses a plugin system to intercept commands and provide enhanced behavior.

## How Plugins Work

When you run `meta <command>`, meta checks if a plugin handles that command pattern:

1. **Plugin matches** → Plugin executes with special logic
2. **No plugin** → Shows help (use `meta exec` for arbitrary commands)

Example:
- `meta git status` → git plugin runs `git status` in every repo at this meta level (`--recursive` for nested ones)
- `meta git clone <url>` → git plugin clones parent + all children from `.meta`
- `meta npm install` → unrecognized, shows help; use `meta exec npm install`

## Built-in Plugins

### Git Plugin (`meta-git`)

Handles all `meta git *` commands with special cases:

| Command | Behavior |
|---------|----------|
| `meta git clone <url>` | Clone parent, read `.meta`, clone all children |
| `meta git update` | Clone missing repos (use `meta git pull` to pull existing ones) |
| `meta git snapshot *` | Create/restore workspace state |
| `meta git worktree *` | Isolated multi-repo worktree sets (see the `meta-worktree` skill) |
| `meta git <other>` | Pass through to every repo at this meta level |

### Project Plugin (`meta-project`)

Workspace management:

```bash
meta project list      # List projects from .meta
meta project check     # Verify all repos exist
meta project dependents <name>  # Projects that depend on one
```

### Rust Plugin (`meta-rust`, if installed)

Cargo workspace awareness. Check `meta --help` for it; without it, use `meta exec -- cargo build`.

```bash
meta rust build        # cargo build in each repo with a Cargo.toml
meta rust test         # cargo test in each repo with a Cargo.toml (no dependency ordering)
```

## Plugin Discovery

Plugins are discovered from:
1. `.meta/plugins/` in the current directory and each parent
2. `~/.meta/plugins/` (moved by `META_DATA_DIR`)
3. Executables named `meta-*` in PATH

The first plugin found with a given name wins.

## Plugin Management

```bash
# List installed plugins
meta plugin list

# Search registry for plugins
meta plugin search <query>

# Install from registry
meta plugin install <name>

# Uninstall
meta plugin uninstall <name>
```

## Understanding Command Flow

```
meta git status
  │
  ├─ Is there a 'git' plugin? Yes (meta-git)
  │
  ├─ Plugin receives: command="git status", projects=[list]
  │
  ├─ Plugin returns: ExecutionPlan with commands per repo
  │
  └─ Meta executes plan via loop engine
```

For commands with special handling (like `clone`), the plugin does the work directly instead of returning an execution plan.

## Why This Matters

Plugins let you:
- **Extend meta** with domain-specific behavior
- **Intercept patterns** like `git clone` to add meta-aware logic
- **Provide help text** via `meta <plugin> --help`

When you see a command behave "magically" (like `meta git clone` cloning multiple repos), a plugin is handling it.
