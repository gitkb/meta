# Task: Plugin Help System & Multi-Commit Support

## Overview

Two major usability gaps in meta:

1. **Help System** - `meta git --help` runs `git --help` in all repos instead of showing meta-git plugin help
2. **Multi-Commit** - No way to create different commit messages for different repos in one operation

---

## Feature 1: Plugin Help System

### Problem

Currently:
```bash
$ meta git --help
# Runs `git --help` in every repo - not useful
```

Expected:
```bash
$ meta git --help
meta-git - Git operations across all meta repositories

USAGE:
    meta git <command> [args...]

COMMANDS:
    clone       Clone all repositories defined in .meta.yaml
    status      Show git status for all repos
    pull        Pull changes in all repos
    push        Push commits in all repos
    ...

To run raw git commands across repos, use:
    meta exec -- git --help
```

### Design

#### 1.1 Plugin Help Protocol

Extend the subprocess plugin protocol to require help text:

```json
{
  "type": "info",
  "name": "git",
  "version": "0.1.0",
  "commands": ["clone", "status", "pull", "push", "fetch", "branch", "checkout"],
  "description": "Git operations across meta repositories",
  "help": {
    "usage": "meta git <command> [args...]",
    "commands": {
      "clone": "Clone all repositories defined in .meta.yaml",
      "status": "Show git status for all repos",
      "pull": "Pull changes in all repos",
      "push": "Push commits in all repos",
      "fetch": "Fetch from remotes in all repos",
      "branch": "Show/create branches across repos",
      "checkout": "Switch branches across repos"
    },
    "examples": [
      "meta git status",
      "meta git pull --rebase",
      "meta git checkout -b feature/new-api"
    ],
    "note": "To run raw git commands: meta exec -- git <command>"
  }
}
```

#### 1.2 Help Detection

Meta should intercept `--help` and `-h` before passing to plugins:

```rust
// In meta_cli/src/main.rs
fn main() {
    let cli = Cli::parse();

    // Check if user wants plugin help
    if let Some(first_arg) = cli.command.first() {
        if cli.command.contains(&"--help".to_string()) ||
           cli.command.contains(&"-h".to_string()) {
            // Check if this is a plugin command
            if let Some(plugin) = find_plugin(first_arg) {
                return show_plugin_help(plugin);
            }
        }
    }

    // ... normal flow
}
```

#### 1.3 Plugin Help Request

New protocol message for help:

```json
// Request
{
  "type": "help",
  "command": "status"  // Optional - for command-specific help
}

// Response
{
  "type": "help",
  "content": "... formatted help text ..."
}
```

#### 1.4 Fallback for Legacy Plugins

If a plugin doesn't support the help protocol, generate basic help from the `info` response:

```
meta-git - Git operations across meta repositories

Commands: clone, status, pull, push, fetch, branch, checkout

Note: This plugin doesn't provide detailed help.
      Run with --verbose to see plugin location.
```

#### 1.5 Main Meta Help

`meta --help` should also list available plugins:

```
Usage: meta [OPTIONS] [COMMAND]...

OPTIONS:
    --json          Output in JSON format
    -t, --tag TAG   Filter by project tag
    -r, --recursive Process nested meta repos
    ...

BUILT-IN COMMANDS:
    exec            Execute command across all repos
    plugin          Manage plugins (install, uninstall, list)

INSTALLED PLUGINS:
    git             Git operations across repos (meta-git)
    project         Project management (meta-project)
    rust            Rust/Cargo operations (meta-rust)

Run 'meta <plugin> --help' for plugin-specific help.
```

### Implementation Files

| File | Changes |
|------|---------|
| `meta_cli/src/subprocess_plugins.rs` | Add `help` message type, update protocol |
| `meta_cli/src/main.rs` | Intercept `--help`, show plugin help |
| `meta_git_cli/src/main.rs` | Implement help protocol |
| `meta_project_cli/src/main.rs` | Implement help protocol |
| `meta_rust_cli/src/main.rs` | Implement help protocol |

---

## Feature 2: Multi-Commit with Different Messages

### Problem

Currently:
```bash
$ meta git commit -m "Update dependencies"
# Same message for ALL repos - often not what you want
```

Needed:
```bash
$ meta git commit --interactive
# Opens editor/UI for per-repo commit messages
```

### Design Options

#### Option A: Interactive TUI (Recommended)

```bash
$ meta git commit -i
# or
$ meta git commit --interactive

┌─────────────────────────────────────────────────────────┐
│ Multi-Repo Commit                                       │
├─────────────────────────────────────────────────────────┤
│ ● meta_cli (3 files changed)                            │
│   Message: [feat: add query DSL for filtering repos   ] │
│                                                         │
│ ● meta_mcp (2 files changed)                            │
│   Message: [feat: add 8 new MCP tools                 ] │
│                                                         │
│ ○ loop_lib (1 file changed)                             │
│   Message: [chore: apply formatting fixes             ] │
│                                                         │
│ [Skip] [Commit Selected] [Commit All]                   │
└─────────────────────────────────────────────────────────┘
```

#### Option B: YAML/JSON Input File

```bash
$ meta git commit --from commits.yaml

# commits.yaml
commits:
  meta_cli:
    message: "feat: add query DSL for filtering repos"
  meta_mcp:
    message: "feat: add 8 new MCP tools"
  loop_lib:
    message: "chore: apply formatting fixes"
```

#### Option C: Editor-Based (like git rebase -i)

```bash
$ meta git commit --edit

# Opens $EDITOR with:
# -------- meta_cli (3 files staged) --------
feat: add query DSL for filtering repos

# -------- meta_mcp (2 files staged) --------
feat: add 8 new MCP tools

# -------- loop_lib (1 file staged) --------
chore: apply formatting fixes

# Lines starting with # are ignored.
# Save and close to commit, or delete a section to skip.
```

### Recommended: Option C (Editor-Based)

Reasons:
- Familiar to git users (like `git rebase -i`)
- Works in any terminal, no TUI library needed
- Allows complex, multi-line commit messages
- Can be piped/scripted for automation

### Implementation

#### 2.1 New Command Flag

```rust
// meta_git_cli
#[derive(Parser)]
struct CommitArgs {
    #[arg(short, long)]
    message: Option<String>,

    #[arg(short = 'e', long)]
    edit: bool,  // Opens editor for per-repo messages

    #[arg(long)]
    from: Option<PathBuf>,  // Read from YAML file
}
```

#### 2.2 Editor Format

```
# Meta Multi-Commit
# Each section represents one repository.
# Edit the message below each header.
# Delete a section entirely to skip that repo.
# Empty messages will skip the repo.

========== meta_cli ==========
# 3 files staged: src/query.rs, src/lib.rs, Cargo.toml

feat: add query DSL for filtering repos

========== meta_mcp ==========
# 2 files staged: src/main.rs, Cargo.toml

feat: add 8 new MCP tools

========== loop_lib ==========
# 1 file staged: src/lib.rs

chore: apply formatting fixes
```

#### 2.3 Parsing Logic

```rust
fn parse_multi_commit_file(content: &str) -> Vec<(String, String)> {
    let mut commits = Vec::new();
    let mut current_repo: Option<String> = None;
    let mut current_message = String::new();

    for line in content.lines() {
        if line.starts_with("==========") && line.ends_with("==========") {
            // Save previous
            if let Some(repo) = current_repo.take() {
                let msg = current_message.trim().to_string();
                if !msg.is_empty() {
                    commits.push((repo, msg));
                }
            }
            // Parse new repo name
            let repo = line.trim_matches('=').trim().to_string();
            current_repo = Some(repo);
            current_message.clear();
        } else if !line.starts_with('#') && current_repo.is_some() {
            current_message.push_str(line);
            current_message.push('\n');
        }
    }

    // Don't forget last one
    if let Some(repo) = current_repo {
        let msg = current_message.trim().to_string();
        if !msg.is_empty() {
            commits.push((repo, msg));
        }
    }

    commits
}
```

#### 2.4 MCP Tool Extension

Add new MCP tool for AI agents:

```json
{
  "name": "meta_git_multi_commit",
  "description": "Create commits with different messages per repo",
  "inputSchema": {
    "type": "object",
    "properties": {
      "commits": {
        "type": "array",
        "items": {
          "type": "object",
          "properties": {
            "project": { "type": "string" },
            "message": { "type": "string" }
          },
          "required": ["project", "message"]
        }
      }
    },
    "required": ["commits"]
  }
}
```

### Implementation Files

| File | Changes |
|------|---------|
| `meta_git_cli/src/main.rs` | Add `--edit` flag, implement editor flow |
| `meta_git_cli/src/lib.rs` | Add `multi_commit()` function |
| `meta_mcp/src/main.rs` | Add `meta_git_multi_commit` tool |

---

## Implementation Order

### Phase 1: Plugin Help System
1. Update `subprocess_plugins.rs` with help protocol
2. Update `meta_cli/src/main.rs` to intercept --help
3. Update `meta_git_cli` with help responses
4. Update `meta_project_cli` with help responses
5. Update `meta_rust_cli` with help responses
6. Add plugin listing to main `meta --help`

### Phase 2: Multi-Commit
1. Add `--edit` flag to meta-git commit
2. Implement editor file generation
3. Implement editor file parsing
4. Execute commits per-repo
5. Add `meta_git_multi_commit` MCP tool

### Phase 3: Documentation & Distribution
1. Update all documentation
2. Test release workflow
3. Create release

---

## Success Criteria

### Help System
- [x] `meta git --help` shows plugin help, not raw git help
- [x] `meta --help` lists installed plugins
- [x] Plugins that don't support help show basic fallback
- [x] Help format is consistent across all plugins

### Multi-Commit
- [x] `meta git commit --edit` opens editor with staged files per repo
- [x] Can create different commit messages for each repo
- [x] Deleting a section skips that repo
- [x] Empty message skips that repo
- [x] MCP tool allows AI agents to create multi-commits

---

## Questions for User

1. **Help format**: Should plugin help use the same format as clap (USAGE, COMMANDS, OPTIONS) or a simpler format?

2. **Multi-commit default**: Should `meta git commit` (no message) default to `--edit` mode, or require explicit `-e`?

3. **Skip behavior**: When a repo has staged changes but user provides no message, should we:
   - Skip silently
   - Warn and skip
   - Error and abort all

4. **Co-author lines**: Should multi-commit automatically add co-author lines, or only when explicitly requested?
