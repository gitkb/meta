# FAQ / Troubleshooting Guide

This guide addresses common questions and issues encountered when using the `meta` CLI platform.

## Table of Contents

- [Installation](#installation)
- [Configuration](#configuration)
- [Commands & Execution](#commands--execution)
- [Git Operations](#git-operations)
- [Plugins](#plugins)
- [MCP Server](#mcp-server)
- [Safe Batch Changes](#safe-batch-changes)
- [Performance](#performance)
- [Debugging](#debugging)
- [Getting Help](#getting-help)
- [See Also](#see-also)

---

## Installation

### Q: `meta` command not found?

**A:** Ensure the installation directory is in your PATH:

```bash
# For cargo install
export PATH="$HOME/.cargo/bin:$PATH"

# For Homebrew
# Usually automatic, but verify:
brew --prefix  # Should show installation prefix
```

### Q: How do I install on Windows?

**A:** Use the PowerShell installer:

```powershell
irm https://raw.githubusercontent.com/gitkb/meta/main/install.ps1 | iex
```

Or build it from source in Git Bash or WSL (see [From Source](../README.md#from-source)):
those steps need `bash`, `git`, `make` and Rust.

### Q: How do I update meta?

**A:** Depends on your installation method:

```bash
# Homebrew
brew upgrade meta-cli

# From source: git pull in your meta checkout, then
# make install. A checkout with meta_cli/ and the rest as separate clones
# must first run the snippet in https://github.com/gitkb/meta/pull/135.
make install

# Script install
curl -fsSL https://raw.githubusercontent.com/gitkb/meta/main/install.sh | bash
```

---

## Configuration

### Q: Why does meta not find my repos?

**A:** meta reads the `.meta.yaml` in the current directory or the nearest one above it, and stops with an error naming the file and the project when an entry is wrong. A relative `path` is relative to that file's folder. To see what it found:

```bash
meta project list     # the projects it reads
meta project check    # the ones missing, not a repository, or cloned from another remote
```

### Q: How do I add a new project?

**A:** Edit your config file:

```yaml
# .meta.yaml
projects:
  new-service:
    repo: git@github.com:org/new-service.git
    tags: [backend]
```

Then clone the missing repo (this also fast-forwards the workspace repo and every cloned repo, `git pull --ff-only`):

```bash
meta git update
```

### Q: Can I use HTTPS URLs instead of SSH?

**A:** Yes, both formats work:

```yaml
projects:
  api: https://github.com/org/api.git
  web: git@github.com:org/web.git
```

---

## Commands & Execution

### Q: How do I run a command only in some repos?

**A:** Use filtering options:

```bash
# By tag
meta --tag backend git status

# By repo: its .meta.yaml name or its path from the workspace root
meta --include api,web git status

# Exclude specific repos
meta --exclude legacy-service exec -- npm install
```

### Q: Commands are running slowly. How can I speed them up?

**A:** `meta exec` and `meta git` already run in all repos at once, unless `.meta.yaml` sets `defaults: {parallel: false}` (then `--parallel` overrides it). Run in fewer repos with `--tag` or `--include`.

### Q: How do I see what would happen without executing?

**A:** Use dry-run mode:

```bash
meta --dry-run git pull
meta --dry-run exec -- rm -rf node_modules
meta --dry-run git worktree remove my-set --force
```

meta's own commands change nothing in preview: they print what they would do. A command that can't preview (such as `meta plugin install`) refuses `--dry-run` with an error.

### Q: How do I get JSON output for scripting?

**A:** Use the `--json` flag:

```bash
meta --json git status | jq '.results[] | select(.success == false)'
```

---

## Git Operations

### Q: `meta git clone` only clones the parent repo?

**A:** Check that your meta config is properly formatted and committed. The clone process:

1. Clones the parent repo
2. Reads its meta config
3. Clones each repo it lists; the repos of nested meta repos (`meta: true`) only with `meta git clone -r <url>`

If child repos aren't cloning, the meta config may be missing or malformed. `meta git update` in the clone clones whatever is still missing.

### Q: Git operations are slow over SSH?

**A:** Nothing to set up: for parallel clones, pulls and other remote git commands meta shares one SSH connection per host, and when it can't open one it runs them one at a time. Load your key into `ssh-agent` so git doesn't ask for its passphrase.

### Q: How do I create the same branch in all repos?

**A:**

```bash
meta git checkout -b feature/new-feature
```

### Q: How do I commit with different messages per repo?

**A:** Use the edit mode:

```bash
meta git commit --edit
```

This opens the editor git uses (`GIT_EDITOR`, `core.editor`, `VISUAL`, `EDITOR`) once, with a section for each repo that has staged changes. It needs a terminal: without one (for example in an agent or a CI job) it stops with an error; use `meta git commit -m "message"` there. On Windows only a missing console is detected, so a hidden one still opens the editor.

---

## Plugins

### Q: How do I add a new plugin?

**A:** Install it with `meta plugin install <name>` (add `--local` for this workspace only), or place an executable named `meta-<name>` where meta looks for plugins (see [Plugins](../README.md#plugins)):

```bash
# Example: install by hand for every workspace
cp meta-docker ~/.meta/plugins/
chmod +x ~/.meta/plugins/meta-docker
```

### Q: My plugin is not detected. What should I check?

**A:**

1. **Executable permission**: `chmod +x meta-plugin`
2. **Naming**: Must start with `meta-`
3. **Location**: One of the discovery paths
4. **Warnings**: `meta plugin list` names each file in `.meta/plugins` or `~/.meta/plugins` it skipped, and why; a `meta-*` program on PATH that isn't a plugin is left out silently

### Q: How do I see available plugins?

**A:**

```bash
meta plugin list
```

### Q: How do I get help for a specific plugin?

**A:**

```bash
meta git --help
meta project --help
```

---

## MCP Server

### Q: How do I set up the MCP server for Claude Desktop?

**A:** Add to Claude Desktop's `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "meta": {
      "command": "meta-mcp",
      "args": []
    }
  }
}
```

### Q: Where is the Claude Desktop config file?

**A:**

- macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`
- Windows: `%APPDATA%\Claude\claude_desktop_config.json`
- Linux: `~/.config/Claude/claude_desktop_config.json`

### Q: MCP server isn't connecting?

**A:**

1. Verify `meta-mcp` is in PATH: `which meta-mcp`
2. Test manually: `meta-mcp` (should wait for JSON-RPC input)
3. Check Claude Desktop logs for errors

---

## Safe Batch Changes

### Q: How do I protect my work before risky changes?

**A:** Commit it, or stash it, in every repo (a backup branch alone keeps only commits):

```bash
meta git status
meta git add -A && meta git commit -m "wip: before changes"
# or: meta exec -- git stash push -u -m before-changes
```

Preview the change with `--dry-run` and limit it with `--include` before running it.

### Q: How do I get back to an earlier state?

**A:** Use `git reflog` in the affected repo to find the previous commit, then branch from it:

```bash
# Look across all repos
meta exec -- git reflog -n 20

# In the affected repo
git switch -c recovered <sha>
```

### Q: Can I recover uncommitted changes?

**A:** No. The reflog only records commits, so commit or stash before batch changes.

---

## Performance

### Q: How can I speed up operations?

**A:**

1. **Filter to relevant repos**: `--tag`, `--include`
2. **Use shallow clones for initial setup**: `meta git clone <url> --depth 1`

### Q: Parallel operations are failing?

**A:** Some operations don't work well in parallel:

- Operations that modify shared state
- Interactive commands
- Commands that need sequential output

Run these with `meta --sequential …`.

---

## Debugging

### Q: How do I enable debug output?

**A:**

```bash
RUST_LOG=debug meta git status
```

`RUST_LOG=meta_git_cli=debug` limits it to the git plugin.

### Q: How do I see what meta is doing?

**A:** Use verbose mode:

```bash
meta --verbose git status
```

### Q: How do I check which config file is being used?

**A:** The nearest `.meta.yaml` at or above the current directory (`--config <name>` looks for another name the same way). `meta project list --json` prints its folder as `root`.

---

## Getting Help

### Q: Where can I report bugs?

**A:** [GitHub Issues](https://github.com/gitkb/meta/issues)

### Q: Where can I ask questions?

**A:** [GitHub Discussions](https://github.com/gitkb/meta/discussions)

### Q: How do I get command help?

**A:**

```bash
meta --help
meta help git
meta help git worktree create
```

---

## See Also

- [Installation Guide](../README.md#installation)
- [Configuration Guide](../README.md#configuration)
- [Advanced Usage](advanced_usage.md)
- [Plugin Development](plugin_development.md)
- [Architecture Overview](architecture_overview.md)
