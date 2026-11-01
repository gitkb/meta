---
name: meta-plugins
description: How the meta plugin system intercepts and enhances commands.
---

# Meta Plugins Skill

Meta uses a plugin system to intercept commands and provide enhanced behavior.

## How Plugins Work

A plugin is an executable named `meta-<word>`, and it serves `meta <word>`.
When you run `meta <word> ...`, meta looks for a file with that name and
starts only it:

1. **`meta-<word>` found** → that plugin executes with special logic
2. **None** → unrecognized command (use `meta exec` for arbitrary commands)

Example:
- `meta git status` → git plugin runs `git status` in every repo at this meta level (`--recursive` for nested ones)
- `meta git clone <url>` → git plugin clones parent + all children from `.meta.yaml`
- `meta npm install` → unrecognized, shows help; use `meta exec npm install`

## Built-in Plugins

### Git Plugin (`meta-git`)

Handles all `meta git *` commands with special cases:

| Command | Behavior |
|---------|----------|
| `meta git clone <url>` | Clone parent, read `.meta.yaml`, clone all children |
| `meta git update` | Fast-forward cloned repos (`git pull --ff-only`) and clone missing ones; `-r` for nested meta repos |
| `meta git worktree *` | Isolated multi-repo worktree sets (see the `meta-worktree` skill) |
| `meta git <other>` | Pass through to every repo at this meta level |

### Project Plugin (`meta-project`)

Workspace management:

```bash
meta project list      # List projects from .meta.yaml
meta project check     # Verify all repos exist
meta project dependents <name>  # Projects that depend on one
```

### Cargo Plugin (`meta-cargo`)

Runs any cargo command in each selected repo that has a `Cargo.toml`, skipping the rest (listed on stderr). `meta exec -- cargo ...` runs cargo in every repo instead.

```bash
meta cargo build --release   # cargo build --release in each Rust repo
meta cargo fmt --check       # any cargo subcommand passes through
meta --include api cargo clippy --all-targets -- -D warnings
```

## Plugin Discovery

Plugins are discovered from:
1. `.meta/plugins/` next to the nearest meta config, then next to each parent
   meta config that lists it as a project (one workspace), nearest first; a
   parent that doesn't list it is not read. Inside a worktree set, a checkout's
   own plugins come first, then those of the checkout it stands for (the one
   it was made from) and its workspace. `meta plugin install <name> --local`
   installs into the nearest one
2. `~/.meta/plugins/` (moved by `META_DATA_DIR`), where `meta plugin install` installs
3. Executables named `meta-*` in PATH

The first file found with a given name wins. `meta <word>` fails naming its
plugin if that plugin doesn't answer `--meta-plugin-info` with its info within
5 seconds.
`meta`, `meta help`, `meta --help` and `meta plugin list` start every plugin;
there a plugins folder meta can't read, and a plugin that doesn't answer within
5 seconds, are skipped with a warning naming them; one that answers with
anything but its info is left out.

Project-local plugins are binaries: `--local` writes `.meta/plugins/.gitignore`
(`*`) so they stay out of git as untracked files.

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

# The same for this meta repo only (.meta/plugins/ next to the nearest meta config)
meta plugin install <name> --local
meta plugin list --local
```

`--local` outside a meta workspace fails with "not in a meta workspace".

## Understanding Command Flow

```
meta git status
  │
  ├─ Is there a file named meta-git? Yes
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
