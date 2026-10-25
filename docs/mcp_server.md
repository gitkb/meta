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
- `tag` (optional): Filter by tag

#### meta_git_commit
Create commits across repositories.

**Parameters:**
- `message` (required): Commit message
- `tag` (optional): Filter by tag

#### meta_git_checkout
Switch branches across repositories.

**Parameters:**
- `branch` (required): Branch name
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
Search for patterns across all repositories.

**Parameters:**
- `pattern` (required): Search pattern (regex)
- `file_pattern` (optional): Filter by file pattern (e.g., "*.rs")
- `tag` (optional): Filter by tag

#### meta_get_file_tree
Get file tree structure for projects.

**Parameters:**
- `project` (optional): Filter to specific project
- `tag` (optional): Filter by tag
- `depth` (optional): Max depth (default: 3)

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

Combine with AND: `dirty:true AND tag:backend AND branch:main`

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

## Extended .meta Configuration

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

Error responses include the tool result with `is_error: true`.
