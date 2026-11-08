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

`--include` and `--tag` each add repos, and `--exclude` then removes some (see [Filtering](../README.md#filtering)). With `-r` or `-A`, a nested project is selected by its own tags, whatever its parent's, and by its path from the workspace root.

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
meta --json exec -- git rev-parse HEAD | jq -j '.results[] | .project + " " + .stdout'

# Project names, one per line
meta --json project list | jq -r '.results[].data.name'

# Only the paths, as meta selects the fields
meta --fields project --json project list
```

### Exit Codes

Meta propagates exit codes from failed commands, making it suitable for CI/CD:

```bash
meta exec -- npm test || echo "Tests failed in at least one repo"
```

<!-- generated:exit-codes -->
| Code | Meaning |
|------|---------|
| 0 | Done |
| 1 | Something failed: a command in at least one repo (the summary names them), or meta itself (`Error: …` on stderr; with `--json`, the Envelope's `error`) |
| 2 | The command line is wrong: an unknown command, or an option or value clap rejects |
| 141 | Its output was cut off: stdout closed early, as in `meta exec -- git log \| head` |
| 128 + n | Stopped by signal n (Ctrl-C: 130), or a plugin was |
<!-- /generated -->

A plugin's own exit code passes through. `loop` and `meta-mcp` exit the same way.

### Colour

Output is coloured only on a terminal: never into a pipe, a file or `--json`. `NO_COLOR` turns colour off, whatever else is set. Otherwise `CLICOLOR_FORCE=1` turns it on even into a pipe (not under `--json`), and `TERM=dumb` or `CLICOLOR=0` turn it off. A command meta captures (in parallel mode, or for JSON) is told the same, through `CLICOLOR_FORCE`/`FORCE_COLOR`; git gets `color.ui=always` after your own `GIT_CONFIG_*` entries, never in place of them. Each command's stdout goes to meta's stdout and its stderr to meta's stderr.

---

## JSON Output Mode

Every command prints one JSON document with `--json` before it: the same envelope, whatever the command. `meta --version --json` isn't a command and prints no envelope, just the build, as `{"version", "source", "commit", "branch", "dirty", "built"}`.

```bash
meta --json git status
meta --json exec -- git rev-parse HEAD
```

### Output Format

`meta --json exec -- git rev-parse HEAD` in a workspace of one repo:

```json
{
  "success": true,
  "command": "exec",
  "results": [
    {
      "project": ".",
      "path": "/work/ws",
      "argv": ["git", "rev-parse", "HEAD"],
      "exit_code": 0,
      "stdout": "abc123...\n",
      "stderr": ""
    },
    {
      "project": "api",
      "path": "/work/ws/api",
      "argv": ["git", "rev-parse", "HEAD"],
      "exit_code": 0,
      "stdout": "def456...\n",
      "stderr": ""
    }
  ],
  "skipped": [],
  "warnings": []
}
```

- `success` is false exactly when `error` is there: `{"code", "message", "hint"?, "rejected"?, "retry_after"?}`, with `code` one of `usage` (exit 2), `not-found`, `conflict`, `dirty`, `plugin-failed`, `tool-failed` (a command failed in at least one repo), `cancelled`, `too-much-output` (a program's captured output passed 16 MiB, so it was stopped), `auth`, `network`, `rate-limited` and `failed` (anything else). `hint` is the line to run instead, or the tip for an option of meta's typed after the command.
- `results` has one row per repo a command ran in, or per record a command returned (a worktree set, a project of `project list`, a repo of `context`); a repo can have several. `project` names the repo by its path from the workspace root as `--include` takes it (`.` for the root; without `-r` or `-A`, from the nearest `.meta.yaml`); a row about the workspace as a whole has none. `path` is its folder, and `path_bytes` its bytes in base64 when they aren't UTF-8. A row that ran a program has `argv`, `exit_code` (null when a signal ended it: see `signal`; absent in a dry run, which runs nothing), `stdout` and `stderr`; a record has the command's own `data`. A row that failed has its own `error`.
- `skipped` lists repos a command left out on purpose (`{"project", "reason", "quiet"}`); all of them skipped is still success. `warnings` lists what the text output warns of (`{"message", "project"?}`).
- `message` is what a command that answers in words said (a preview, help).

With `--json`, stdout holds the envelope alone: what a command or plugin prints as text goes to stderr. A command line that can't be parsed is an envelope too (code `usage`, exit 2) when `--json` is among meta's options before the command; without, it is text on stderr. `--fields a,b.c` keeps only those fields of each row (`b.c` reaches into a field, as `data.name`).

### Streaming

`--json-stream` prints the same envelope as the last line of a stream (`{"envelope": …}`), after one line per event as it happens (`{"event": …}`): `step` when a step of a plan ends (its row, as in `results`), `record` when a command sends one of its records early (meta-grep's matches, say), `warning` as it is found, and `progress` (`{"done", "total"?, "project"?, "message"?}`). Under `--json` the same records and warnings are in the envelope, in the order they came, and progress is left out; in text they are shown as they come (progress only on a terminal).

```bash
meta --json-stream exec -- make test | jq -c 'select(.event) | .event | {project, exit_code}'
```

### Parsing with jq

```bash
# The repos where the command failed
meta --json exec -- npm test | jq -r '.results[] | select(.error) | .project'

# How many failed
meta --json exec -- npm test | jq '[.results[] | select(.error)] | length'
```

---

## Parallel Execution

`meta exec` and the `meta git` commands that run git as typed run in every repo at once by default (`defaults.parallel: true` in `.meta.yaml`). Each repo's output is captured and shown after all are done, the root first; on a terminal a spinner per repo shows progress. `--sequential`, or `defaults: {parallel: false}`, runs one repo after another with live output: use it for commands that ask for input, share state, or that you are debugging. Output meta captures (in parallel mode and with `--json`) is held until the command ends, so meta keeps at most 16 MiB of it per command: one that writes more is stopped and fails with `too-much-output`, and none of its output is shown. Live output is the command's own, on your terminal, file or pipe, with no limit. After the commands, a summary names each repo where one failed and how it ended, and, for output meta captured, the last line of its stderr (`no stderr` when it wrote none). `--silent` sends the stdout and stderr of the programs meta runs for `exec`, raw git, `cargo`, `git commit -m` and `git worktree exec` to nowhere, in every order, and meta keeps none of it, under `--json` too: its rows keep each command's status, with empty output. Commands that answer for themselves (`git clone`, `git update`, `git commit --edit`, `git worktree` other than `exec`) still print. A command `meta exec` silences is never stopped for what it writes. If any failed, the closing error names the repos and nothing they wrote (`Error: 2 commands failed: app, platform`); to see why, run it again without `--silent`. `--parallel` wins over both. `meta git clone` and `meta git update` clone and pull several repos at once whatever these say (`meta git clone --parallel 8` sets how many) and show their own progress. `loop` runs sequentially by default.

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
meta git worktree list

# Select one set explicitly.
meta git worktree list example

# Inside .worktrees/example/ or any descendant: select the enclosing set.
meta git worktree list

# Fail on what can't be read instead of skipping it.
meta --strict git worktree list
```

An explicit name overrides the enclosing set. Every repository of the set is
listed, those nested beneath other repositories of the set (`clients/desktop`
inside `clients`) too, so `-r` and `-A` change nothing here and are refused, before or
after the command. meta's `--strict` fails on a folder that can't be scanned,
a checkout whose git files are broken, or a repository git can't read, instead
of skipping it or marking it unknown.

### Metadata-only listing

Tools that collect their own git facts can ask for the set's members only:

```sh
meta --strict --json git worktree list example --metadata-only

# From inside the worktree, omit the name:
meta --strict --json git worktree list --metadata-only
```

Both print the usual envelope, with one row for the set:

```json
{
  "success": true,
  "command": "git worktree list",
  "results": [{
    "data": {
      "name": "example",
      "root": "/work/harmony/.worktrees/example",
      "has_meta_root": true,
      "repos": [{
        "alias": ".",
        "path": "/work/harmony/.worktrees/example",
        "source_path": "/work/harmony"
      }]
    }
  }],
  "skipped": [],
  "warnings": []
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

### Previewing a worktree set

`meta --json --dry-run git worktree create` returns the plan as one row, whose
`data` is `{"repos": [{"alias", "source", "path"}]}`: each repo the create
would make a worktree of, the checkout it comes from, and where it would go.
With `-r` or `-A`, the repos of nested meta repos are in that one list, their
aliases paths from where the `.meta.yaml` is read. A path that isn't UTF-8 is
`{"lossy", "bytes"}` in place of a string, `bytes` being its base64. A preview
that first has to clone something can't know the plan, and returns no rows: it
succeeds, and `skipped` lists each repo to clone (`project`) and why (`reason`).

---

## See Also

- [Plugin Development Guide](plugin_development.md)
- [Architecture Overview](architecture_overview.md)
- [MCP Server Documentation](mcp_server.md)
- [FAQ / Troubleshooting](faq_troubleshooting.md)
