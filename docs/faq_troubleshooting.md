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

# From source: git pull in meta and each child repo, then
make install

# Script install
curl -fsSL https://raw.githubusercontent.com/gitkb/meta/main/install.sh | bash
```

---

## Configuration

### Q: Why does meta not find my repos?

**A:** Check your `.meta` or `.meta.yaml` file:

1. Ensure it's in the repository root
2. Verify JSON/YAML syntax
3. Check project paths are relative to the meta file location

```bash
# Validate JSON
cat .meta | jq .

# Validate YAML
cat .meta.yaml | python3 -c "import yaml,sys; yaml.safe_load(sys.stdin)"
```

### Q: Which config file takes priority?

**A:** File priority: `.meta.yaml` > `.meta.yml` > `.meta`

### Q: How do I add a new project?

**A:** Edit your config file:

```yaml
# .meta.yaml
projects:
  new-service:
    repo: git@github.com:org/new-service.git
    tags: [backend]
```

Then clone the missing repo:

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
# By tag (plugin commands)
meta --tag backend git status

# By repo: its .meta name or its path from the workspace root
meta --include api,web git status

# Exclude specific repos
meta --exclude legacy-service exec -- npm install
```

### Q: Commands are running slowly. How can I speed them up?

**A:** Use parallel execution:

```bash
meta git status --parallel
meta exec --parallel -- npm install
```

### Q: How do I see what would happen without executing?

**A:** Use dry-run mode:

```bash
meta git pull --dry-run
meta exec --dry-run -- rm -rf node_modules
```

### Q: How do I get JSON output for scripting?

**A:** Use the `--json` flag:

```bash
meta --json git status | jq '.results[] | select(.success == false)'
```

---

## Git Operations

### Q: `meta git clone` only clones the parent repo?

**A:** Check that your `.meta` file is properly formatted and committed. The clone process:

1. Clones the parent repo
2. Reads `.meta` or `.meta.yaml`
3. Queues and clones all child repos

If child repos aren't cloning, the `.meta` file may be missing or malformed.

### Q: Git operations are slow over SSH?

**A:** Set up SSH multiplexing:

```bash
meta git setup-ssh
```

This configures connection reuse for faster parallel operations.

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

This opens an editor for each dirty repo.

---

## Plugins

### Q: How do I add a new plugin?

**A:** Place the plugin in one of these locations:

1. `.meta-plugins/` in your project
2. `~/.meta-plugins/` in your home directory
3. System PATH (binary named `meta-<name>`)

```bash
# Example: install to home directory
cp meta-docker ~/.meta-plugins/
chmod +x ~/.meta-plugins/meta-docker
```

### Q: My plugin is not detected. What should I check?

**A:**

1. **Executable permission**: `chmod +x meta-plugin`
2. **Naming**: Must start with `meta-`
3. **Location**: One of the discovery paths
4. **Debug**: `META_DEBUG=1 meta plugin list`

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

1. **Use parallel mode**: `--parallel`
2. **Set up SSH multiplexing**: `meta git setup-ssh`
3. **Filter to relevant repos**: `--tag`, `--include`
4. **Use shallow clones for initial setup**: `meta git clone <url> --depth 1`

### Q: Parallel operations are failing?

**A:** Some operations don't work well in parallel:

- Operations that modify shared state
- Interactive commands
- Commands that need sequential output

Try sequential execution for these cases.

---

## Debugging

### Q: How do I enable debug output?

**A:**

```bash
META_DEBUG=1 meta git status
```

### Q: How do I see what meta is doing?

**A:** Use verbose mode:

```bash
meta --verbose git status
```

### Q: How do I check which config file is being used?

**A:**

```bash
META_DEBUG=1 meta project list 2>&1 | grep -i config
```

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
meta git --help
meta project --help
```

---

## See Also

- [Installation Guide](../README.md#installation)
- [Configuration Guide](../README.md#configuration)
- [Advanced Usage](advanced_usage.md)
- [Plugin Development](plugin_development.md)
- [Architecture Overview](architecture_overview.md)
