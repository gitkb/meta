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
2. **None** → an error that suggests the nearest command (for `gti status`, `meta git status`), else `meta exec -- <words>`

Example:
- `meta git status` → git plugin runs `git status` in every repo at this meta level (`--recursive` for nested ones)
- `meta git clone <url>` → git plugin clones parent + all children from `.meta.yaml`
- npm has no plugin: run it with `meta exec -- npm install`

## Built-in Plugins

### Git Plugin (`meta-git`)

Handles all `meta git *` commands with special cases:

| Command | Behavior |
|---------|----------|
| `meta git clone <url>` | Clone parent, read `.meta.yaml`, clone its children (`meta -r git clone <url>`: nested meta repos' too) |
| `meta git update` | Fast-forward cloned repos (`git pull --ff-only`) and clone missing ones; `meta -r git update` for nested meta repos |
| `meta git clone-missing` | Clone the missing projects; change no repo that is cloned |
| `meta git clone-prune` | Remove the repos no `.meta.yaml` lists that hold nothing unpushed |
| `meta git clone-sync` | `clone-missing`, then `clone-prune` |
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
5 seconds, are skipped with a warning naming them. So is a file in
`.meta/plugins` or `~/.meta/plugins` that can't be started, isn't executable,
or answers with anything but its info; a `meta-*` program on PATH that can't
be started or isn't a plugin is just left out, as it may be any program.

Project-local plugins are binaries: `--local` writes `.meta/plugins/.gitignore`
(`*`) so they stay out of git as untracked files.

## Plugin Management

A meta plugin (`meta-<word>`) adds or adapts `meta <word> ...`. A tool's own
extension (`git-<name>` on PATH) is just part of that tool and runs through
meta as typed: `meta git <name>` runs `git <name>` in each repo.
`meta plugin list` shows both.

To learn who answers a command, read the first line of its help:
`meta git clone --help` says it is adapted by meta-git (and which of git's
options it takes); `meta git pull --help` says it runs as typed. `meta --help`
lists each plugin's own commands.

```bash
# List installed plugins, then the tool extensions on PATH
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
  ├─ Plugin receives: command="git", args=["status"], projects=[list]
  │
  ├─ Plugin returns: ExecutionPlan with commands per repo
  │
  └─ Meta executes plan via loop engine
```

For commands with special handling (like `clone`), the plugin does the work directly instead of returning an execution plan.

meta's own options (`-r`, `--json`, `--dry-run`, `--include`, …) go before the command and reach a plugin in the request's `options`, never in `args` (`meta -r --json project list`). A plugin defines no option of meta's; it parses its arguments with `meta_plugin_protocol::parse_args` and refuses the rest with exit 2, so a meta option typed after its command gets the tip with the line that has it before (see `docs/plugin_development.md`).

## Why This Matters

Plugins let you:
- **Extend meta** with domain-specific behavior
- **Intercept patterns** like `git clone` to add meta-aware logic
- **Provide help text** via `meta <plugin> --help`

When you see a command behave "magically" (like `meta git clone` cloning multiple repos), a plugin is handling it.
