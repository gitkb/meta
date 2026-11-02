# Advanced Usage Guide

This guide covers advanced features of the `meta` CLI, including safe batch changes, dependency tracking, and power-user workflows.

## Table of Contents

- [Safe Batch Changes](#safe-batch-changes)
- [Dependency Tracking](#dependency-tracking)
- [Filtering Commands](#filtering-commands)
- [Scripting with Meta](#scripting-with-meta)
- [JSON Output Mode](#json-output-mode)
- [Parallel Execution](#parallel-execution)
- [Advanced Plugin Usage](#advanced-plugin-usage)
- [Listing Worktree Sets](#listing-worktree-sets)
- [See Also](#see-also)

---

## Safe Batch Changes

Before changing many repositories at once, commit or stash uncommitted work in every repo, so you can get back to it with plain git. A backup branch alone keeps only commits, not uncommitted changes.

### Before Changing Anything

```bash
# See which repos have uncommitted changes
meta git status

# Commit them (`meta git commit` commits only staged changes)
meta git add -A
meta git commit -m "wip: before refactor"

# Or stash them in every repo (untracked files included)
meta exec -- git stash push -u -m before-refactor
```

### Preview and Target

```bash
# Preview what would run, without running it
meta --dry-run exec -- npm update

# Limit the change to specific repos
meta --include api-service,web-app exec -- npm update
```

### Safe Batch Workflow

```bash
# 1. Commit pending work before the risky operation
meta git add -A && meta git commit -m "wip: before changes"

# 2. Make changes across repos
meta exec -- npm update

# 3. Test changes
meta exec -- npm test

# 4. If tests fail, discard the batch's changes in the affected repos.
#    Everything from before was committed in step 1, so this only drops
#    what the batch changed: git restore --staged --worktree . (tracked
#    files, staged or not) and git clean -nd, then -fd, for new files

# 5. If tests pass, stage and commit
meta git add -A && meta git commit -m "chore: update dependencies"
```

### Recovering Committed Work

`git reflog` lists every commit a repo's `HEAD` pointed to, so you can find and restore an earlier state:

```bash
# Look across all repos
meta exec -- git reflog -n 20

# In the affected repo, branch from the earlier commit
git switch -c recovered <sha>
```

The reflog only recovers commits. Uncommitted changes that were discarded are gone, so commit or stash first.

---

## Dependency Tracking

For complex workspaces, meta supports dependency declarations for impact analysis and execution ordering.

### Configuration

Extend your `.meta.yaml` with `provides` and `depends_on`:

```yaml
projects:
  shared-utils:
    repo: git@github.com:org/shared-utils.git
    provides: [utils-api, logger-api]
    depends_on: []

  auth-service:
    repo: git@github.com:org/auth-service.git
    provides: [auth-api]
    depends_on:
      - shared-utils

  api-service:
    repo: git@github.com:org/api-service.git
    depends_on:
      - auth-api      # Reference by provided name
      - utils-api
```

The [MCP server](mcp_server.md) reads them: `meta_analyze_impact` lists the projects a change to one affects, `meta_execution_order` puts projects in build order, and `meta_batch_execute` with `"atomic": true` runs a command across repos and, if any repo fails, stops and puts each repo it reached back where it was (on its branch, or on the commit a detached HEAD was at) (see [Agent Workflows](agent_workflows.md)). On the command line, `meta project dependents <project>` names the projects that depend on one.

---

## Filtering Commands

### Combined Filtering

```bash
# The backend repos and the root repo
meta --tag backend --include . git status
# The backend repos except one
meta --tag backend --exclude legacy exec -- cargo test
```

`--include` and `--tag` each add repos, and `--exclude` then removes some (see [Filtering](../README.md#filtering)). With `--recursive`, a nested project is selected by its own tags, whatever its parent's.

---

## Scripting with Meta

### Chaining Commands

```bash
# Chain plugin commands and exec commands
meta git pull && meta exec -- npm install && meta exec -- npm test
```

### Capturing Output

```bash
# Each repo's commit
meta --json exec -- git rev-parse HEAD | jq -j '.results[] | .directory + " " + (.stdout // "\n")'

# Project names, one per line
meta project list --json | jq -r '.projects[].name'
```

### Exit Codes

Meta propagates exit codes from failed commands, making it suitable for CI/CD:

```bash
meta exec -- npm test || echo "Tests failed in at least one repo"
```

| Code | Meaning |
|------|---------|
| 0 | Done |
| 1 | Something failed: a command in at least one repo (the summary names them), or meta itself (`Error: …` on stderr) |
| 2 | The command line is wrong: an unknown command, or an option or value clap rejects |
| 141 | Its output was cut off: stdout closed early, as in `meta exec -- git log \| head` |
| 128 + n | Stopped by signal n (Ctrl-C: 130), or a plugin was |

A plugin's own exit code passes through. `loop` and `meta-mcp` exit the same way.

### Colour

Output is coloured only on a terminal: never into a pipe, a file or `--json`. `NO_COLOR` turns colour off, whatever else is set. Otherwise `CLICOLOR_FORCE=1` turns it on even into a pipe (not under `--json`), and `TERM=dumb` or `CLICOLOR=0` turn it off. A command meta captures (in parallel mode, or for JSON) is told the same, through `CLICOLOR_FORCE`/`FORCE_COLOR`; git gets `color.ui=always` after your own `GIT_CONFIG_*` entries, never in place of them. Each command's stdout goes to meta's stdout and its stderr to meta's stderr.

---

## JSON Output Mode

Get structured output for parsing and automation:

```bash
meta --json git status
meta --json exec -- git rev-parse HEAD
```

### Output Format

```json
{
  "success": true,
  "results": [
    {
      "directory": "/work/ws/api",
      "command": "git rev-parse HEAD",
      "success": true,
      "exit_code": 0,
      "stdout": "abc123...\n"
    }
  ],
  "summary": {
    "total": 5,
    "succeeded": 5,
    "failed": 0,
    "dry_run": false
  }
}
```

A result has `"stderr"` when the command wrote any. A command a signal ended has `"exit_code": null` and the signal's number in
`"signal"` (`9` for `SIGKILL`); `signal` is left out otherwise.

With `--json`, a command with a JSON result prints only JSON on stdout: one document, or the documents a plugin writes (help and `meta init claude` have no JSON form). A command line clap can't parse is reported before meta knows about `--json`, so it is text on stderr (exit 2). After that, an error before any output is `{"success": false, "error": "…"}` (an unknown command exits 2, other errors 1); one after a report was printed goes to stderr, so the report stays the only document. A plugin's output that isn't JSON comes as `{"success": …, "output": "…"}`, and a plugin that fails without printing anything leaves `{"success": false, "error": "plugin … failed"}`. Warnings and `--verbose` lines go to stderr.

### Parsing with jq

```bash
# Get only successful repos
meta --json git status | jq '.results[] | select(.success) | .directory'

# Count failures
meta --json exec -- npm test | jq '.summary.failed'
```

---

## Parallel Execution

`meta exec` and the `meta git` commands that run git as typed run in every repo at once by default (`defaults.parallel: true` in `.meta.yaml`). Each repo's output is captured and shown after all are done, the root first; on a terminal a spinner per repo shows progress. `--sequential`, or `defaults: {parallel: false}`, runs one repo after another with live output: use it for commands that ask for input, share state, or that you are debugging. `--parallel` wins over both. `meta git clone` and `meta git update` clone and pull several repos at once whatever these say (`meta git clone --parallel 8` sets how many) and show their own progress. `loop` runs sequentially by default.

```bash
meta --sequential exec -- npm install
```

---

## Advanced Plugin Usage

Where meta finds plugins, and how they talk to it: [Plugins](../README.md#plugins) and the [Plugin Development Guide](plugin_development.md).

### Plugin Help

```bash
meta help git
meta git worktree create --help
```

### Installing Plugins

```bash
# Search registry
meta plugin search docker

# Install
meta plugin install meta-docker

# List installed
meta plugin list

# This meta repo only (.meta/plugins/ next to the nearest meta config)
meta plugin install meta-docker --local
meta plugin list --local
```

---

## Listing Worktree Sets

`list` summarizes worktree sets; `status NAME` provides detailed Git state
(modified files, untracked counts, and ahead/behind information).

```sh
# Outside a worktree: list all sets, with branch and dirty/clean summaries.
meta git worktree list --recursive

# Select one set explicitly.
meta git worktree list example --recursive

# Inside .worktrees/example/ or any descendant: select the enclosing set.
meta git worktree list --recursive
```

An explicit name overrides the enclosing set. `--recursive` includes
repositories nested beneath other repositories of the set.

### Metadata-only listing

Tools that collect their own git facts can ask for the set's members only:

```sh
meta git worktree list example --recursive --metadata-only --json

# From inside the worktree, omit the name:
meta git worktree list --recursive --metadata-only --json
```

Both print the usual `worktrees` list, with the one set:

```json
{
  "worktrees": [{
    "name": "example",
    "root": "/work/harmony/.worktrees/example",
    "has_meta_root": true,
    "repos": [{
      "alias": ".",
      "path": "/work/harmony/.worktrees/example",
      "source_path": "/work/harmony"
    }]
  }]
}
```

Paths above are illustrative. Each repository has an alias, its checkout's
path and the path of the checkout it was made from; branch, dirty-state and
ahead/behind fields are left out. Without
`--metadata-only`, a member git can't read has `"branch": null` and
`"dirty": null` (unknown, never clean). `meta git worktree status` shows the
same unknowns, and names members whose checkout is gone in `"missing"`,
exiting 1.

A selected set that is missing, empty or invalid is an error. Listing all sets
reports the ones it can't read in `warnings` (on stderr in the text listing),
so check them before treating the list as complete.

---

## See Also

- [Plugin Development Guide](plugin_development.md)
- [Architecture Overview](architecture_overview.md)
- [MCP Server Documentation](mcp_server.md)
- [FAQ / Troubleshooting](faq_troubleshooting.md)
