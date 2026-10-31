# Meta MCP Server

The Meta MCP (Model Context Protocol) server provides AI agents with structured access to multi-repository operations. It exposes 26 tools across core operations, git workflows, build/test orchestration, code discovery, and AI-dominance features.

## Installation

The `meta-mcp` binary is included in the standard release package. Install via:

```bash
# Via install script
curl -fsSL https://raw.githubusercontent.com/gitkb/meta/main/install.sh | bash

# Or via Homebrew
brew install gitkb/tap/meta-cli
```

## Usage

The MCP server runs as a stdio-based JSON-RPC server:

```bash
meta-mcp
```

Configure in Claude Desktop's `claude_desktop_config.json`:

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

## Tool Reference

Each tool takes only the arguments listed for it. A misspelled argument, or a value of the wrong type (`"atomic": "true"` for `true`), is an error that names the tool, the argument and the value, and nothing runs. `null` is the same as leaving an argument out.

A `tag` parameter takes one tag or a comma-separated list, and selects the projects that have any of them, as `meta --tag` does. A blank list is no filter.

A `project` parameter is a project's name in `.meta.yaml`, or `.` for the workspace root. It is never a path, so no tool can be pointed outside the workspace; a name the config doesn't list is an error. `meta_git_multi_commit` and `meta_git_diff` refuse a project folder that is not a git repository of its own (git would use the repository around it); the tools that run `meta` act in it as `meta` does. A tool that takes both `project` and `tag` refuses them together.

Tools that act in several repositories report each one: its directory or name, whether it succeeded, and its output. The result is an error (`isError: true`) only when the tool itself failed or every repository failed; for the rest, an entry with `success: false` or an `error` field is a repository that failed (`meta_git_diff` and `meta_search_code` entries carry only `error`).

### Core Tools

#### meta_list_projects
Lists all projects in the meta repository.

**Parameters:**
- `tag` (optional): Filter by tag

**Example:**
```json
{"name": "meta_list_projects", "arguments": {"tag": "backend"}}
```

#### meta_exec
Execute a command across all projects.

Runs `meta exec` with the given program and arguments.

**Parameters:**
- `command` (required): The program and its arguments, one per item. They reach the program as given, with no shell; for a shell, ask for one: `["sh", "-c", "..."]`
- `tag` (optional): Filter by tag

**Example:**
```json
{"name": "meta_exec", "arguments": {"command": ["git", "log", "--format=%h %s", "-1"], "tag": "frontend"}}
```

#### meta_get_config
Get the raw meta configuration file.

**Parameters:** None

#### meta_get_project_path
Get the absolute path for a specific project.

**Parameters:**
- `project` (required): Project name

---

### Git Tools

#### meta_git_status
Get git status across all repositories.

**Parameters:**
- `project` (optional): Filter to specific project
- `tag` (optional): Filter by tag

#### meta_git_pull
Pull changes in all repositories.

**Parameters:**
- `tag` (optional): Filter by tag
- `rebase` (optional): Use --rebase

#### meta_git_push
Push commits across repositories.

**Parameters:**
- `tag` (optional): Filter by tag

#### meta_git_fetch
Fetch from remotes.

**Parameters:**
- `tag` (optional): Filter by tag

#### meta_git_diff
Get diffs across repositories.

**Parameters:**
- `project` (optional): Filter to specific project
- `tag` (optional): Filter by tag
- `staged` (optional): Show staged changes only

#### meta_git_branch
Get branch information (current, tracking, ahead/behind).

**Parameters:**
- `tag` (optional): Filter by tag

#### meta_git_add
Stage files across repositories.

**Parameters:**
- `files` (optional): Paths to stage, one per item, passed to git as given (default: `["."]`)
- `project` (optional): Stage only in this project
- `tag` (optional): Filter by tag

#### meta_git_commit
Create commits across repositories, in each one with staged changes.

**Parameters:**
- `message` (required): Commit message
- `project` (optional): Commit only in this project
- `tag` (optional): Filter by tag

#### meta_git_checkout
Switch branches across repositories.

**Parameters:**
- `branch` (required): Branch name. One `git check-ref-format --branch` refuses (such as `--orphan=x`) is an error, and it is never taken for a file to restore.
- `create` (optional): Create new branch
- `tag` (optional): Filter by tag

#### meta_git_multi_commit
Create commits with different messages for each repository. Useful for tailored commit messages.

**Parameters:**
- `commits` (required): Array of commit objects

**Commit Object:**
```json
{
  "project": "project-name",  // Use "." for root repo
  "message": "Commit message for this project"
}
```

Every `project` is checked before anything is committed: one `.meta.yaml` doesn't list is an error, and no commit is made. An entry that fails has git's reason in `error` (`nothing to commit`, say).

**Example:**
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

**Returns:**
```json
{
  "results": [
    {"project": ".", "success": true, "message": "feat: add new meta feature"},
    {"project": "api-service", "success": true, "message": "fix: resolve API bug"},
    {"project": "web-app", "success": false, "message": "chore: update dependencies", "error": "nothing to commit"}
  ],
  "summary": {"total": 3, "succeeded": 2, "failed": 1}
}
```

---

### Build/Test Tools

#### meta_detect_build_systems
Detect build systems (Cargo, npm, go, make, etc.) in each project.

**Parameters:**
- `tag` (optional): Filter by tag

#### meta_run_tests
Run tests across projects with auto-detection of test commands.

**Parameters:**
- `project` (optional): Filter to specific project
- `tag` (optional): Filter by tag

#### meta_build
Build all projects.

**Parameters:**
- `release` (optional): Build in release mode
- `tag` (optional): Filter by tag

#### meta_clean
Clean build artifacts.

**Parameters:**
- `tag` (optional): Filter by tag

---

### Discovery Tools

#### meta_search_code
Search for patterns across all repositories, those nested in meta projects too, with `git grep`: tracked files, and untracked files that `.gitignore` doesn't ignore; never `.git`. Each project is listed by its path in the workspace, a nested one too (`platform/api`), and is searched once: a project's search leaves out the projects nested in it. A nested `.meta.yaml` that can't be read is its project's error, reported whatever the tag filter; the projects beside it are still searched. When git reports a problem, the project's entry has an `error`, beside any `matches` git still found (an unreadable file, say). A project gives at most 50 matches; an entry cut there has `"truncated": true`.

**Parameters:**
- `pattern` (required): Search pattern, an extended regular expression (as `grep -E`)
- `file_pattern` (optional): Filter by file name pattern, in any directory (e.g., "*.rs")
- `tag` (optional): Filter by tag

#### meta_get_file_tree
Get file tree structure for projects.

**Parameters:**
- `project` (optional): Filter to specific project
- `tag` (optional): Filter by tag
- `depth` (optional): Max depth, a non-negative integer (default: 3)

A symbolic link is listed as `{"type": "symlink", "name": ...}` and not followed.

#### meta_list_plugins
List installed meta plugins.

**Parameters:** None

---

### AI-Dominance Tools

These tools provide enhanced capabilities for AI agents managing multi-repo workflows.

#### meta_query_repos
Query repositories by state using a DSL.

**Parameters:**
- `query` (required): Query string

**Query Syntax:**
- `dirty:true` - Projects with uncommitted changes
- `branch:main` - Projects on specific branch
- `tag:backend` - Projects with tag
- `ahead:true` - Projects ahead of remote
- `behind:true` - Projects behind remote

Combine with AND (in any case): `dirty:true AND tag:backend AND branch:main`. There is no `OR` or `NOT`, and a value holds no spaces: anything else in a query, or a condition with no value (`tag:`), is an error naming it.

**Example:**
```json
{"name": "meta_query_repos", "arguments": {"query": "dirty:true AND tag:backend"}}
```

#### meta_workspace_state
Get a summary of workspace state.

**Parameters:** None

**Returns:**
```json
{
  "total_projects": 10,
  "dirty_projects": 2,
  "clean_projects": 8,
  "ahead_of_remote": 1,
  "behind_remote": 0,
  "projects_by_branch": {"main": 8, "feature/x": 2},
  "projects_by_tag": {"backend": 5, "frontend": 3}
}
```

#### meta_analyze_impact
Analyze what would be affected if a project changes (dependency tracking).

**Parameters:**
- `project` (required): Project to analyze

**Returns:**
```json
{
  "project": "shared-utils",
  "direct_dependents": ["api-service", "auth-service"],
  "transitive_dependents": ["web-app"],
  "total_affected": 3
}
```

#### meta_execution_order
Get topologically sorted build/test order based on dependencies.

**Parameters:**
- `tag` (optional): Filter by tag

**Returns:**
```json
{
  "execution_order": ["shared-utils", "auth-service", "api-service", "web-app"],
  "count": 4
}
```

#### meta_batch_execute
Execute command across repos with optional atomic mode (auto-rollback on failure).

**Parameters:**
- `command` (required): The program and its arguments to run in each project, one per item, with no shell
- `tag` (optional): Filter by tag
- `atomic` (optional): Enable atomic mode with rollback on failure

**Example:**
```json
{
  "name": "meta_batch_execute",
  "arguments": {
    "command": ["npm", "test"],
    "tag": "frontend",
    "atomic": true
  }
}
```

---

## Extended .meta.yaml Configuration

For dependency tracking features, extend your `.meta.yaml` with `provides` and `depends_on`:

```yaml
projects:
  shared-utils:
    repo: git@github.com:org/shared-utils.git
    tags: [lib]
    provides: [utils-v2]
    depends_on: []

  auth-service:
    repo: git@github.com:org/auth-service.git
    tags: [backend]
    provides: [auth-api]
    depends_on:
      - shared-utils

  api-service:
    repo: git@github.com:org/api-service.git
    tags: [backend]
    provides: [api-v2]
    depends_on:
      - auth-service
      - utils-v2  # Can reference provided items

  web-app:
    repo: git@github.com:org/web-app.git
    tags: [frontend]
    depends_on:
      - api-v2
```

---

## Error Handling

All tools return structured responses. Errors include:
- `-32600`: Invalid request
- `-32601`: Method not found
- `-32602`: Invalid params
- `-32603`: Internal error

A tool that fails answers with its result's `isError: true`; its text gives the error and what caused it.

## Ping and cancellation

`ping` is answered with `{}`. Tool calls run one at a time, in the order they arrive; `initialize`, `ping` and `tools/list` are answered meanwhile.

`notifications/cancelled` stops a call: one still waiting never runs, and the program a running one started is stopped with what remains in its process group (on Unix its process group gets SIGTERM, then SIGKILL after 2 seconds; on Windows only the program itself is killed). What the call already did stays done. On Unix, a signal that ends the server (SIGTERM, SIGINT, SIGHUP) first stops the running call's programs the same way, and starts no call still waiting; the server ends once that call has (an atomic batch's rollback included). A second signal ends it at once. The git status reads of `meta_git_branch`, `meta_query_repos` and `meta_workspace_state` are not stopped: those calls end when their reads do. On Unix a tool's programs run in a process group of their own, so one that asks on the terminal (an ssh passphrase or host-key prompt) waits there until the call is cancelled: use an ssh agent and known hosts. A cancelled call gets no reply; what it returned goes to the server's stderr, so an atomic batch's rollback (with its stash) is still on record.
