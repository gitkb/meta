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
The primary checkout is pull-only: it stays on the default branch and only
`meta -r git update` changes it (it fast-forwards every repo and clones the
missing ones, after a PR lands or at the start of a task). Never commit, switch
branches or leave edits there.

Read-only work (reading code, searching, answering questions) may stay in the
primary checkout.

Create one set per task, with only the repos it touches, and give each set its
own build output (for Cargo, a `CARGO_TARGET_DIR` of its own or the set's own
`target/`): builds shared between sets silently reuse each other's results.
Remove a set once the work is pushed; **before a session ends, remove every set
it created.** Inside a set, `meta git worktree` commands find sets where the
checkout the set was made from keeps them, so running them there acts on the
workspace's sets.

## Creating Worktrees

Run these from the workspace root. **Use `-r` when a repo you name, or one it
depends on, is nested in a meta repo**, and `-A` when the target is at another
nesting level than the folder you are in: without them a set holds only this
level's repos, and each dependency the set won't hold is named in a warning with
the `--repo` that adds it (with `--strict`, an error, and nothing is made);
`--no-deps` leaves dependencies out without a word. `-r`
follows a dependency across nesting levels, by name, path or what a repo
`provides`.

```bash
# Specific repos (the root repo and their dependencies are added automatically)
meta git worktree create auth-fix --repo backend --repo frontend

# A nested repo and its dependencies, across nesting levels
meta -r git worktree create sdk-fix --repo open-source/sdk/core

# Only the named repos (and, for a repo nested inside another project's path, that parent project)
meta git worktree create auth-fix --repo backend --no-deps

# Every repo configured at this meta level
meta git worktree create full-task --every-repo

# Branch name other than the set name
meta git worktree create my-task --repo api --branch feature/my-feature

# Per-repo branch
meta git worktree create review --repo api:release/2.3 --repo worker:develop

# Start from a tag, SHA or branch (positional commit-ish)
meta git worktree create incident-42 v2.3.1 --repo api

# Continue a GitHub PR on a child repo: checks out its head branch
meta git worktree create review --from-pr org/api#42 --repo api

# Aliases with -A are paths from the workspace root, from anywhere
# (-r starts at the nearest .meta.yaml, aliases from there)
meta -A git worktree create sdk-fix-2 --repo open-source/sdk/core

# Preview without creating anything
meta --dry-run git worktree create auth-fix --repo backend
```

`meta --json git worktree create …` reports the set as `results[0].data`: its
`root`, and `repos[]` with each repo's `alias`, `path` and `branch`. To see what
depends on a repo before you change it: `meta -r project dependents <repo>`.

Add a repo to an existing set with `meta git worktree add auth-fix --repo another-service`
(`meta git worktree add auth-fix v2.3.1 --repo another-service` starts it from a commit-ish).

A commit-ish starts a new branch. Without one, each repo's branch is checked out
if it exists locally, else made tracking `origin/<branch>` if this clone has
that ref. Otherwise meta asks origin (once per repo, up to 5 seconds, with no
prompts): a branch origin has is fetched and tracked, with its commits. If origin
can't answer, a warning names the repo and branch and the branch starts as new
(with `--strict`, an error before anything is made); a repo with no `origin` is
not asked. A new branch starts from the remote's default branch (`origin/HEAD`,
tracking nothing), never from whatever the workspace's checkout has checked
out; only a repo whose remote names no default branch starts from its HEAD,
with a warning. The output names the start of each new branch:
`backend -> auth-fix (new from origin/main)`. `--dry-run` asks origin too, says
the same start, and writes nothing; when a repo the set needs isn't cloned it stops
at naming the clones, and `--from-pr` shows a placeholder for the PR's branch.

A repo the set needs that isn't cloned (one asked for, a dependency, or a
parent on the way to a nested repo) is cloned first, as
`meta git clone-missing` would, and named: `Cloning 2 repos the set needs: web, db`.
`add` does the same for the repos it adds. Nothing else is cloned. When a clone
fails (no access, say), no set is made (the repos it did clone stay cloned), and
the error names every repo the set needs that still isn't there. `--every-repo` skips the projects that aren't cloned instead.

A set lays its repos out as the workspace does: each repo sits at its `path`
from the meta config, so relative references between repos keep working.

`--from-pr` uses the PR's head branch name and fetches it from the child repo
whose `origin` matches; it does not check out a fork's branch or pin the head
SHA. It never applies to the root repo. When no selected repo matches, or the
fetch fails, create fails and makes nothing. To continue a PR on the root repo,
check out its branch directly:

```bash
git fetch origin my-feature:my-feature   # create or fast-forward the local branch
meta --strict git worktree create review --repo .:my-feature --repo backend
```

Don't pass `origin/my-feature` as the commit-ish here: with a commit-ish, create
always makes a new branch, which fails when `my-feature` already exists locally.

With `--from-pr` on a child repo whose PR branch already exists locally, `create`
checks that branch out as it is (the fetch only updates `origin/<branch>`), so
pull it in the new worktree.

## Working in a Set

```bash
cd .worktrees/auth-fix
meta git status                  # only this set's repos
meta git add -A                  # `meta git commit` commits only what is staged
meta git commit -m "feat: add token refresh"
meta --include backend git push -u origin HEAD   # the repos you changed; a new branch has no upstream yet
```

With nothing staged, `meta git commit` says so and exits 0, having committed
nothing, so stage first. `--include` takes a project's name or its path in the
set (`platform/api`); leave it off and `push -u` publishes an empty branch in
every repo of the set.

## Adding a Repo to the Workspace

The config change goes through a set like any other change; the new repo is
cloned into the primary checkout once it lands:

```bash
meta git worktree create add-svc --repo .
cd .worktrees/add-svc
# add to .meta.yaml, under projects:   svc: git@github.com:org/svc.git
echo svc >> .gitignore           # or the root repo would stage it as an embedded repo
git add .meta.yaml .gitignore
git commit -m "chore: add svc to the workspace"
git push -u origin HEAD          # then land it, as a PR or however the repo takes changes
cd ../..
meta git worktree remove add-svc
meta -r git update               # once it is on the default branch: pulls the config, clones svc
```

For a repo nested in a meta repo, do the same in that meta repo's set
(`meta -r git worktree create add-svc --repo platform`, then edit
`platform/.meta.yaml` and `platform/.gitignore`, and use `git -C platform` for the
add, commit and push). `provides` and `depends_on`
are lists of names. `meta project` has no command that adds one. `meta git clone-missing`
refuses to run inside a set, and `meta git worktree add` knows only the
projects of the primary checkout's config, so a repo can't join a set before its
config has landed.

## Short-Lived and Automated Sets

```bash
# Expire after a TTL (30s, 5m, 1h, 2d, 1w); `prune` removes expired sets
meta git worktree create ci-check --every-repo --ttl 1h

# Mark as ephemeral and store metadata for tooling
meta git worktree create ci-check --every-repo --ephemeral --meta agent=review-bot --meta run_id=abc123

# Run a command in a set of its own
meta git worktree create tmp --repo a --branch b
meta git worktree exec tmp -- cargo test
meta git worktree remove tmp
```

`prune` removes sets whose TTL has expired, ephemeral sets whose create has
ended, and sets a stopped `create` left half-made; for a set whose directory
is gone it only forgets meta's record (git's entries stay: a worktree that was
moved can still be repaired). meta never drops git's entries itself: after a
worktree folder was deleted by hand, `create` and `add` at that path stop and
name the repository, the path and the one command to drop git's entry once
you've checked it holds nothing you need (`git -C <repo> worktree remove
<path>`). Once its repos are taken out of the meta config,
or the checkout it was made from is moved or deleted, a set is no longer the
workspace's: remove each member with `git worktree remove`, run in the repo it
was made from. A member whose repo was deleted with that checkout has no git
record left, so git can't remove it: copy out any files you want to keep,
then delete its folder by hand. `create --ephemeral` makes a set the next
`prune` removes. A TTL is a backstop for a
session that dies before removing its sets.

## Inspecting Worktrees

```bash
meta git worktree list                 # from the workspace root: every set; inside a set, just that one
meta git worktree list auth-fix
meta --json git worktree list
meta git worktree list --metadata-only # membership paths only, no Git status
meta git worktree status auth-fix      # per-repo branch and dirty state
meta git worktree diff auth-fix --base main   # committed changes: a summary, then the patch
meta git worktree diff auth-fix --stat        # the summary only
```

`list` and `status` show every repo of the set, those nested beneath other
worktree repos too (`-r` and `-A` change nothing there and are refused);
`meta --strict git worktree list` fails on what it can't read instead of
skipping it. Run `list` from the workspace root: a nested folder can't see a set
made with `-A`. Inside a set, `meta project list` and `meta context` show the whole workspace's
projects, not the set's members; `meta git status` or `meta git worktree list`
shows those.

## Running Commands

```bash
meta git worktree exec auth-fix -- cargo test
meta --include backend git worktree exec auth-fix -- make build
meta --exclude frontend git worktree exec auth-fix -- npm test
meta --parallel git worktree exec auth-fix -- cargo build
```

meta's `--include`/`--exclude`/`--tag`, before the command as always, choose
among the set's repos, nested ones included: a project's name or its path in
the set (`.` for the root repo), never part of a path. A value that names no
repo in the set is an error. The other worktree commands refuse them. The
command's arguments reach it as typed, with no shell; use `-- sh -c '...'`
when you want one.

## Context Detection

Inside `.worktrees/<name>/`, meta commands that run in repos scope themselves to
that set's repos (`meta project list` and `meta context` still list the workspace):

```bash
cd .worktrees/auth-fix/backend
meta exec -- cargo test       # runs in auth-fix's repos, not the primary checkout
meta git status
meta --tag cli exec -- pwd
```

A set is a folder meta knows by its place: one in the worktrees folder that has
an owner file in `.meta-sets/` beside it, or the
`<name>` folder of a `.worktrees/<name>` path. Its members are the workspace's
projects whose folders are in it, at their paths, those nested inside another
project of the set (e.g. `open-source/sdk/core` inside `open-source`) too, as
`list` shows them; what made a checkout (git, jj) doesn't matter. `-r` and `-A`
add nothing inside a set. The root repo is part of a set by default, so the set
has its own `.meta.yaml` config; otherwise meta reads the config of the workspace
the set was made from. A set with no members is an error, never a reason to run
in the workspace. `meta --config <file> <command>` naming a config outside the
set acts on that config's workspace instead. `meta --primary <command>` bypasses detection and acts on the
workspace the set was made from (its owner file says which), from wherever you
are in the set.

## Cleanup

`cd` out of a set before removing it: your shell would be left in a folder that
no longer exists.

```bash
meta git worktree remove auth-fix          # refuses a set with work it would lose
meta git worktree remove auth-fix --force  # discards that work
meta --dry-run git worktree remove auth-fix --force  # what it would delete
meta --dry-run git worktree prune          # preview
meta git worktree prune                    # remove expired (TTL), ephemeral, half-made and orphaned sets of this workspace
```

`remove` and `prune` delete only sets meta created, and check every repo in the
set first, nested ones included: uncommitted or untracked changes, commits on
no branch, and files outside every repo count as work. `remove` refuses such a
set unless `--force`; `prune` keeps it. Both act only on sets of the current
workspace: sets whose every worktree is recorded in one of the workspace's
repositories. Another workspace's set is never touched, even when both share a
`META_WORKTREES` directory; `remove` refuses it and names its workspace. Run
from inside a set, they act on the workspace the set was made from.
`--force` discards that work: commit and push first. A set's reflog goes with
the set: recover anything you reset away before removing it.

Like `git worktree remove`, files git ignores (`.env`, `target/`) don't count
as work and go with the set. `remove` then deletes
each branch create or add made for the set, in the workspace's repos, when
every commit on it is also on a remote, another branch or a tag, as `git
branch -d` would. Commits only a reflog holds (reset, amended or rebased away)
are not kept: git counts them discarded. It names each one it deletes, and keeps and names any branch with a commit nowhere
else (`kept branch 'auth-fix' in …/backend: 1 commit on no remote, other branch
or tag`). A branch the set only checked out (`--repo api:release/2.3`, an
existing one) is never deleted, nor are the branches of sets made by a meta
older than this; `remove` names those as left. `--json` lists each branch with
its action (deleted, kept, left), and for kept and left the reason.

## Lifecycle Hooks

Configure hooks in the meta config (`.meta.yaml`):

```yaml
worktree:
  hooks:
    post-create: my-tool worktree on-create
    post-destroy: my-tool worktree on-destroy   # runs on `remove`
    post-prune: my-tool worktree on-prune
```

Each hook runs in the directory of the config that names it (the workspace's,
also when run from inside a set) and gets a JSON description of the action on
stdin. Its stdout is discarded and its stderr goes to meta's. A hook runs
once the command's work is done (on the terminal its stderr, and a warning
about it, show before the command's report). A hook that fails is a warning
naming it and its exit code (or signal), and an error under `--strict`; either
way the work stays done. Any other hook name is a config error.

`create` keeps the worktrees folder out of the workspace repository's
`git status` by listing it in that repository's `.git/info/exclude`, which is
local and never committed; `.gitignore` is left as it is.

## Where Meta Keeps Its Records

There is no global state file. When meta creates a worktree it writes a record
into git's own directory for that worktree, `<repo>/.git/worktrees/<id>/meta-worktree.json`,
with the set's path, its project, when it was created, and its TTL, ephemeral
flag and custom metadata. Git deletes the record with the worktree. The record
is how `remove` and `prune` know meta made a set: a set made by hand, or by a
meta from before records, is refused, and so is a set holding a worktree meta
has no record of (one added by hand with `git worktree add`); remove those with
`git worktree remove`. While `create` runs, a lease beside the set,
`<worktrees dir>/.<set>.meta-lease` (and its `.lock`), names it: `remove` refuses a set
that is being created, and takes back one whose creator stopped part way,
worktrees not recorded yet included.
`list` scans this project's `.worktrees/` and shows each set's record.

## Strict Mode

`meta --strict git worktree create …` turns warnings such as a
missing commit-ish in one repo, or (with `--every-repo`) a repo that isn't cloned, into errors. A create that fails is rolled back:
the worktrees and branches it made are deleted again. If the rollback itself
can't delete something, the error says what was left behind. A failing hook
fails the command too, but only after the set was made (or removed): that is
not rolled back, so remove the set yourself if you don't want it.

## Reference

Every subcommand and option: `meta help git worktree` and `meta help git worktree <command>`.
