---
name: meta-worktree
description: Create, use and remove isolated multi-repo git worktree sets with `meta git worktree`. Use before changing code in a meta workspace, to continue a PR, or to test a tag or SHA in isolation.
---

# Meta Worktree Skill

A worktree set is a named group of git worktrees, one per repo, on one branch
by default. It lets a change span several repos without touching the primary
checkout, and lets several tasks run side by side.

```
.worktrees/
  auth-fix/           # set name; also the default branch name
    backend/          # git worktree of the primary checkout's backend/
    frontend/
```

## When to Use Worktrees

**Every change gets its own worktree set**, even a one-line fix in one repo.
Keep the primary checkout on the default branch and only pull it; never commit,
switch branches or leave edits there.

Read-only work (reading code, searching, answering questions) may stay in the
primary checkout.

Create one set per task, with only the repos it touches, and remove it once the
work is pushed. Before a session ends, remove every set it created. Run
`meta git worktree` commands from the primary checkout: inside a set they look
for sets under that set (only `list` understands the current set).

## Creating Worktrees

```bash
# Specific repos (the root repo and their dependencies are added automatically)
meta git worktree create auth-fix --repo backend --repo frontend

# Only the named repos (and, for nested aliases, their parent repos)
meta git worktree create auth-fix --repo backend --no-deps

# Every repo configured at this meta level
meta git worktree create full-task --all

# Branch name other than the set name
meta git worktree create my-task --repo api --branch feature/my-feature

# Per-repo branch
meta git worktree create review --repo api:release/2.3 --repo worker:develop

# Start from a tag, SHA or branch (positional commit-ish)
meta git worktree create incident-42 v2.3.1 --repo api

# Continue a GitHub PR on a child repo: checks out its head branch
meta git worktree create review --from-pr org/api#42 --repo api

# Nested meta repos: resolve across the whole tree, aliases are root-relative paths
meta git worktree create sdk-fix --recursive --repo open-source/sdk/core

# Preview without creating anything
meta git worktree create auth-fix --repo backend --dry-run
```

Add a repo to an existing set with `meta git worktree add auth-fix --repo another-service`.

`--from-pr` uses the PR's head branch name and fetches it from the child repo
whose `origin` matches; it does not check out a fork's branch or pin the head
SHA. It never applies to the root repo. When no repo matches it only warns and
keeps the set's own branch; when the fetch fails it warns and still uses a branch
named after the PR, without the PR's commits (add `--strict` to make either an
error). To continue a PR on the root repo, check out its branch directly:

```bash
git fetch origin my-feature:my-feature   # create or fast-forward the local branch
meta git worktree create review --repo .:my-feature --repo backend --strict
```

Add at least one child repo (here `backend`, on the set's own branch): `prune`
treats a set that holds only the root repo as orphaned and force-removes it.
Don't pass `origin/my-feature` as the commit-ish here: with a commit-ish, create
always makes a new branch, which fails when `my-feature` already exists locally
(and without `--strict` that failure is only a warning).

With `--from-pr` on a child repo whose PR branch already exists locally, `create`
checks that branch out as it is (the fetch only updates `origin/<branch>`), so
pull it in the new worktree.

## Short-Lived and Automated Sets

```bash
# Expire after a TTL (30s, 5m, 1h, 2d, 1w); `prune` removes expired sets
meta git worktree create ci-check --all --ttl 1h

# Mark as ephemeral and store metadata for tooling
meta git worktree create ci-check --all --ephemeral --meta agent=review-bot --meta run_id=abc123

# Create, run a command, then force-remove, in one step
meta git worktree exec --ephemeral lint-check --all -- make lint
```

`prune` removes sets whose TTL has expired and sets it considers orphaned (for
example: the set's directory or project is gone, none of its repos are in the
project any more, or the set holds only the root repo);
`--ephemeral` alone only records metadata. A TTL is a backstop for a session that
dies before removing its sets.

## Inspecting Worktrees

```bash
meta git worktree list                 # every set; inside a set, just that one
meta git worktree list auth-fix
meta --json git worktree list
meta git worktree list --metadata-only # membership paths only, no Git status
meta git worktree status auth-fix      # per-repo branch and dirty state
meta git worktree diff auth-fix --base main   # committed changes only, as a stat summary
```

`list` and `status` take `--recursive` to include repos nested beneath other
worktree repos.

## Running Commands

```bash
meta git worktree exec auth-fix -- cargo test
meta git worktree exec auth-fix --include backend -- make build
meta git worktree exec auth-fix --exclude legacy -- npm test
meta git worktree exec auth-fix --parallel -- cargo build
```

`--include`/`--exclude` match text anywhere in each repo's absolute path, and
every path contains `.worktrees/<set>/`: pick filter text that isn't in the set
name (`--include auth` in set `auth-fix` matches every repo).

## Context Detection

Inside `.worktrees/<name>/`, plain meta commands scope themselves to that set's
repos:

```bash
cd .worktrees/auth-fix/backend
meta exec -- cargo test       # runs in auth-fix's repos, not the primary checkout
meta git status
meta --tag cli exec -- pwd
```

Detection relies on the `.worktrees/<name>` path. It, and
`meta git worktree exec`, only find repos down to the first nested repo: repos
created with `--recursive` inside another repo of the set (e.g.
`open-source/sdk/core` inside `open-source`) are skipped. `list --recursive`
shows them; run the tool itself (`cargo`, `git`) in their directory, not `meta exec`, which still scopes to the set's top-level repos there. The root repo is part of a set
by default, so the set has its own `.meta` config; otherwise meta walks up to
the primary checkout's config. `meta --primary <command>` bypasses detection;
run it from the primary checkout to be sure it uses the primary repos.

## Cleanup

```bash
meta git worktree remove auth-fix          # refuses top-level repos with uncommitted changes
meta git worktree remove auth-fix --force  # discards uncommitted changes
meta git worktree prune --dry-run          # preview
meta git worktree prune                    # remove expired (TTL) and orphaned sets
```

`remove` only checks the set's top-level repos; repos nested inside them (added
with `--recursive`) are deleted without that check. `prune` works on the machine-wide store, so it can remove expired sets from
other projects too. `--force`, `exec --ephemeral` and `prune` all discard
uncommitted work: commit and push first.

## Lifecycle Hooks

Configure hooks in the meta config (`.meta`, `.meta.json`, `.meta.yaml` or `.meta.yml`):

```yaml
worktree:
  hooks:
    post-create: my-tool worktree on-create
    post-destroy: my-tool worktree on-destroy   # runs on `remove` and ephemeral cleanup
    post-prune: my-tool worktree on-prune
```

Each hook gets a JSON description of the action on stdin. Hook failures don't
block the operation; they are logged as warnings, shown only with `RUST_LOG=warn`.

## Centralized Store

Worktree metadata lives in `~/.meta/worktree.json` (the directory can be moved
with `META_DATA_DIR`). It records TTL, ephemeral and custom metadata for every
set on the machine. `list` scans this project's `.worktrees/` and adds the
store's metadata; `prune` only acts on sets in the store.

## Strict Mode

`--strict` on `create` (or global `meta --strict`) turns warnings such as a
missing commit-ish in one repo into errors. Repos already created are not rolled
back, and the half-made set is not in the store, so `prune` won't find it: clean
it up with `remove`.

## Command Reference

| Command | Description |
|---------|-------------|
| `create <name> [<commit-ish>]` | Create a new worktree set |
| `add <name>` | Add repo(s) to an existing set |
| `list [<name>]` | List sets (the current one inside a set) |
| `status <name>` | Per-repo status |
| `diff <name>` | Committed changes vs a base branch |
| `exec <name> -- <cmd>` | Run a command in the set's repos |
| `remove <name>` | Remove a set |
| `prune` | Remove expired or orphaned sets |

### Create Options

| Option | Description |
|--------|-------------|
| `[<commit-ish>]` | Start from a tag, SHA or branch (positional) |
| `--repo <alias>[:<branch>]` | Add specific repo(s) |
| `--all` | Add all repos at this meta level |
| `--no-deps` | Skip automatic root and dependency selection (nested aliases still get their parents) |
| `-r, --recursive` | Resolve across nested meta repos (root-relative aliases) |
| `--branch <name>` | Override the branch name |
| `--from-pr <owner/repo#N>` | Start a child repo from a PR's head branch |
| `--dry-run` | Preview without creating anything |
| `--ephemeral` | Mark as ephemeral |
| `--ttl <duration>` | Make `prune` remove it after this long |
| `--meta <key=value>` | Store custom metadata |
| `--strict` | Fail instead of warning on missing refs |

### Exec Options

| Option | Description |
|--------|-------------|
| `--include <text>` | Only repos whose path contains it (comma-separated) |
| `--exclude <text>` | Skip repos whose path contains it (comma-separated) |
| `--parallel` | Run concurrently |
| `--ephemeral` | Create, run, then remove (takes `--repo`/`--all`, `--branch`, `--from-pr`, `--meta`, a commit-ish) |
