# Meta MCP Server

`meta-mcp` is an MCP (Model Context Protocol) server that gives AI agents structured access to a meta workspace through 26 tools: core operations, git, build and test, code discovery, and workspace analysis.

## Installation

`meta-mcp` ships with meta: the install script, Homebrew (`brew install gitkb/tap/meta-cli`) and `make install` all install it next to `meta`.

## Usage

The server speaks JSON-RPC over stdin and stdout, one message per line. It acts on the workspace of the directory it starts in: the nearest `.meta.yaml` at or above it. Configure it in your client, for example Claude Desktop's `claude_desktop_config.json`:

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

Claude Code's `meta` plugin (`claude plugin install meta@gitkb`) registers it for you.

## Tool Reference

`tools/list` gives each tool's description and the schema of its arguments. Each tool takes only the arguments listed for it. A misspelled argument, or a value of the wrong type (`"atomic": "true"` for `true`), is an error that names the tool, the argument and the value, and nothing runs. `null` is the same as leaving an argument out.

A `tag` argument takes one tag or a comma-separated list, and selects the projects that have any of them, as `meta --tag` does. A blank list is no filter. The tools that run `meta` (`meta_exec` and the `meta git` ones) act where `meta` would, the workspace root included; the others act on the projects of `.meta.yaml`, and on the root only when `project` is `.` (`meta_build` always includes it).

A `project` argument is a project's name in `.meta.yaml`, or `.` for the workspace root. It is never a path, so a tool goes only where the config puts a project (which may be outside the workspace); a name the config doesn't list is an error. `meta_git_multi_commit` and `meta_git_diff` refuse a project folder that is not a git repository of its own (git would use the repository around it); the tools that run `meta` act in it as `meta` does. A tool that takes both `project` and `tag` refuses them together.

Tools that act in several repositories report each one: its directory or name, whether it succeeded, and its output. The result is an error (`isError: true`) only when the tool itself failed or every repository failed; for the rest, an entry with `success: false` or an `error` field is a repository that failed (`meta_git_diff` and `meta_search_code` entries carry only `error`). `meta_git_branch` is never an error as a whole: a repository it can't read is an entry with `branch: null` and an `error`.

### Core

| Tool | Arguments | What it does |
|------|-----------|--------------|
| `meta_list_projects` | `tag` | Lists the projects, with their paths and tags |
| `meta_exec` | `command`, `tag` | Runs `meta exec` with `command`: the program and its arguments, one per item, with no shell (for one: `["sh", "-c", "..."]`) |
| `meta_get_config` | none | The config file's path and its text, as it is |
| `meta_get_project_path` | `project` | `{"project", "path", "exists"}`: the project's absolute path |

```json
{"name": "meta_exec", "arguments": {"command": ["git", "log", "--format=%h %s", "-1"], "tag": "frontend"}}
```

### Git

| Tool | Arguments | What it does |
|------|-----------|--------------|
| `meta_git_status` | `project`, `tag` | `meta git status` |
| `meta_git_pull` | `tag`, `rebase` | `meta git pull` (`--rebase` with `rebase: true`) |
| `meta_git_push` | `tag` | `meta git push` |
| `meta_git_fetch` | `tag` | `meta git fetch` |
| `meta_git_diff` | `project`, `tag`, `staged` | `git diff` (`--staged`) in each repository; only those with a diff are listed |
| `meta_git_branch` | `tag` | Each repository's branch, tracking branch, and commits ahead and behind |
| `meta_git_add` | `files`, `project`, `tag` | `meta git add` with `files`, passed to git as given (default `["."]`) |
| `meta_git_commit` | `message`, `project`, `tag` | `meta git commit -m`: commits what is staged, in each repository with staged changes |
| `meta_git_checkout` | `branch`, `create`, `tag` | `meta git checkout` (`-b` with `create: true`). A name `git check-ref-format --branch` refuses (such as `--orphan=x`) is an error, and it is never taken for a file to restore |
| `meta_git_multi_commit` | `commits` | Commits what is staged with a different message in each project |

`meta_git_multi_commit` takes a list of `{"project", "message"}` (`"."` for the root). Every `project` is checked before anything is committed: one `.meta.yaml` doesn't list is an error, and no commit is made. An entry that fails has git's reason in `error` (`nothing to commit`, say).

```json
{
  "name": "meta_git_multi_commit",
  "arguments": {
    "commits": [
      {"project": ".", "message": "feat: add new meta feature"},
      {"project": "api-service", "message": "fix: resolve API bug"},
      {"project": "web-app", "message": "chore: update dependencies"}
    ]
  }
}
```

```json
{
  "summary": {"total": 3, "succeeded": 2, "failed": 1},
  "results": [
    {"project": ".", "success": true, "message": "feat: add new meta feature"},
    {"project": "api-service", "success": true, "message": "fix: resolve API bug"},
    {"project": "web-app", "success": false, "message": "chore: update dependencies", "error": "nothing to commit"}
  ]
}
```

### Build and test

| Tool | Arguments | What it does |
|------|-----------|--------------|
| `meta_detect_build_systems` | `tag` | Which of cargo, npm, make, go, maven, gradle and python each project uses, by its files |
| `meta_run_tests` | `project`, `tag` | `cargo test`, `npm test`, `go test ./...` or `make test`, by the first of `Cargo.toml`, `package.json`, `go.mod`, `Makefile` a project has |
| `meta_build` | `release`, `tag` | `cargo build` (`--release`), `npm run build`, `go build ./...` or `make`, the same way, in the workspace root and then each selected project |
| `meta_clean` | `tag` | `cargo clean`, `go clean` or `make clean`; npm projects are skipped |

They run one project at a time, in project-name order (`meta_build` the root first), and skip a project with none of those files. `meta_build` always includes the root, even with a `tag`, and doesn't follow `meta_execution_order`: for dependency order, build each project in the order that tool gives.

### Discovery

`meta_search_code` (`pattern`, `file_pattern`, `tag`) searches with `git grep`: tracked files, and untracked files that `.gitignore` doesn't ignore; never `.git`. `pattern` is an extended regular expression (as `grep -E`); `file_pattern` matches file names in any directory (`"*.rs"`). Projects nested in meta projects are searched too. Each project is listed by its path in the workspace, a nested one too (`platform/api`), and is searched once: a project's search leaves out the projects nested in it. A nested `.meta.yaml` that can't be read is its project's error, reported whatever the tag filter; the projects beside it are still searched. When git reports a problem, the project's entry has an `error`, beside any `matches` git still found (an unreadable file, say). A project gives at most 50 matches; an entry cut there has `"truncated": true`.

`meta_get_file_tree` (`project`, `tag`, `depth`, a non-negative integer, default 3) gives each project's files and folders. Hidden entries, `node_modules` and `target` are left out; a symbolic link is listed as `{"type": "symlink", "name": ...}` and not followed.

`meta_list_plugins` (no arguments) gives `meta plugin list --json`.

### Workspace analysis

`meta_query_repos` (`query`) lists the projects that match every condition of a query, with each one's state (branch, dirty, ahead and behind, last commit, build systems):

| Condition | Matches |
|-----------|---------|
| `dirty:true`, `dirty:false` | Projects with or without uncommitted changes |
| `branch:main` | Projects on that branch |
| `tag:backend` | Projects with that tag |
| `ahead:true`, `behind:true` | Projects ahead of or behind their upstream (`has_unpushed:true` is `ahead:true`) |
| `modified_in:24h` | Projects with a commit in that time (`30s`, `5m`, `2h`, `1d`, `1w`) |
| `language:cargo` | Projects with that build system (`cargo`, `npm`, `go`, `make`, `maven`, `gradle`, `python`, `cmake`, `meson`) |

Join conditions with AND (in any case): `dirty:true AND tag:backend AND branch:main`. There is no `OR` or `NOT`, and a value holds no spaces: anything else in a query, or a condition with no value (`tag:`), is an error naming it.

`meta_workspace_state` (no arguments) sums the workspace up:

```json
{
  "total_projects": 10,
  "dirty_projects": 2,
  "clean_projects": 8,
  "unknown_projects": 0,
  "ahead_of_remote": 1,
  "behind_remote": 0,
  "projects_by_branch": {"main": 8, "feature/x": 2},
  "projects_by_tag": {"backend": 5, "frontend": 3},
  "projects_by_build_system": {"cargo": 4, "npm": 6}
}
```

`meta_analyze_impact` (`project`) gives what depends on a project, from `depends_on` and `provides` in `.meta.yaml`:

```json
{
  "project": "shared-utils",
  "direct_dependents": ["api-service", "auth-service"],
  "transitive_dependents": ["web-app"],
  "total_affected": 3
}
```

`meta_execution_order` (`tag`) orders the projects so that dependencies come before their dependents; a dependency cycle is an error:

```json
{
  "execution_order": ["shared-utils", "auth-service", "api-service", "web-app"],
  "count": 4,
  "tag_filter": null
}
```

`meta_batch_execute` (`command`, `tag`, `atomic`) runs `command` (the program and its arguments, one per item, with no shell) in each project, one at a time in project-name order, and reports each one's `success`, `exit_code`, `stdout` and `stderr`. With `atomic: true` it first refuses to run unless every project is the root of a clean git repository. On the first failure it stops and puts each project it reached back where it was (on its branch, or on the commit a detached HEAD was at); their uncommitted changes and untracked files are saved in a git stash. `rollback.status` is `success` when every one was put back; `rollback.restored` names each one's stash and the commits it replaced, and `rollback.failed` each one it couldn't put back, with git's reason. The rollback is git only: ignored files, submodules, other refs and anything the command did outside the repositories (a push, `npm publish`) are not undone. An atomic batch that failed anywhere is an error as a whole.

```json
{"name": "meta_batch_execute", "arguments": {"command": ["npm", "test"], "tag": "frontend", "atomic": true}}
```

## Dependencies in .meta.yaml

`meta_analyze_impact` and `meta_execution_order` read `depends_on` and `provides` from the extended project entries. A `depends_on` names a project, or something a project `provides`:

```yaml
projects:
  shared-utils:
    repo: git@github.com:org/shared-utils.git
    tags: [lib]
    provides: [utils-v2]

  auth-service:
    repo: git@github.com:org/auth-service.git
    tags: [backend]
    provides: [auth-api]
    depends_on: [shared-utils]

  api-service:
    repo: git@github.com:org/api-service.git
    tags: [backend]
    provides: [api-v2]
    depends_on: [auth-service, utils-v2]

  web-app:
    repo: git@github.com:org/web-app.git
    tags: [frontend]
    depends_on: [api-v2]
```

## Errors

A message the server can't take gets a JSON-RPC error: `-32700` for a line that isn't JSON, `-32600` for an invalid request, `-32601` for an unknown method, `-32603` for an internal error. A tool that fails answers with its result's `isError: true`; its text gives the error and what caused it.

## Ping and cancellation

`ping` is answered with `{}`. Tool calls run one at a time, in the order they arrive; `initialize`, `ping` and `tools/list` are answered meanwhile.

`notifications/cancelled` stops a call: one still waiting never runs, and the program a running one started is stopped with what remains in its process group (on Unix its process group gets SIGTERM, then SIGKILL after 2 seconds; on Windows only the program itself is killed). What the call already did stays done. A cancelled call gets no reply; what it returned goes to the server's stderr, so an atomic batch's rollback (with its stash) is still on record. The git status reads of `meta_git_branch`, `meta_query_repos` and `meta_workspace_state` are not stopped: those calls end when their reads do.

When the client stops reading the server's output, the server ends once it finds that out: if that is while answering something else, the running call is cancelled the same way; calls still waiting are dropped, each with a `request N not started` line on stderr, and a tool's reply it can't send goes to stderr. On Unix, a signal that ends the server (SIGTERM, SIGINT, SIGHUP, even when started ignoring them, as `nohup` does) first stops the running call's programs the same way and starts no call still waiting; the server ends once that call has (an atomic batch's rollback included) and its reply is written. A second signal ends it at once.

On Unix a tool's programs run in a process group of their own, so one that asks on the terminal (an ssh passphrase or host-key prompt) waits there until the call is cancelled: use an ssh agent and known hosts.
