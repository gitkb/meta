# Agent Workflows with Meta

This guide covers common patterns for AI agents working with multi-repository codebases using Meta's MCP tools. [mcp_server.md](mcp_server.md) describes each tool, its arguments and its results, and the query language of `meta_query_repos`.

## Common Workflows

### 1. Initial Workspace Assessment

When starting work in a new meta repository:

```
1. meta_workspace_state → Get overall summary
2. meta_list_projects → See all projects with tags
3. meta_git_status → Check for uncommitted changes
4. meta_detect_build_systems → Understand build tools
```

**Example sequence:**
```json
// Step 1: Get workspace summary
{"name": "meta_workspace_state", "arguments": {}}

// Step 2: List all projects
{"name": "meta_list_projects", "arguments": {}}

// Step 3: Check git status
{"name": "meta_git_status", "arguments": {}}
```

### 2. Finding Code Across Repos

When searching for implementation details:

```
1. meta_search_code → Find pattern across all repos
2. meta_get_file_tree → Explore relevant project structure
3. Read specific files as needed
```

**Example:**
```json
// Find all API endpoints
{"name": "meta_search_code", "arguments": {
  "pattern": "@(Get|Post|Put|Delete)Mapping",
  "file_pattern": "*.java"
}}

// Or find TypeScript handlers
{"name": "meta_search_code", "arguments": {
  "pattern": "export.*Handler",
  "file_pattern": "*.ts",
  "tag": "backend"
}}
```

### 3. Safe Multi-Repo Updates

When making changes across repositories:

```
1. meta_git_status → Check for uncommitted changes
2. meta_git_add, then meta_git_commit → Commit them (meta_git_commit commits only what is staged)
3. meta_query_repos → Find repos to modify
4. Make changes
5. meta_run_tests → Verify changes
6. If tests fail: discard the changes with git (everything from before is committed)
7. If tests pass: meta_git_add, then meta_git_commit → Commit changes
```

**Example:**
```json
// Check state, then commit pending work
{"name": "meta_git_status", "arguments": {"tag": "frontend"}}
{"name": "meta_git_add", "arguments": {"tag": "frontend"}}
{"name": "meta_git_commit", "arguments": {"message": "wip: before dependency update", "tag": "frontend"}}

// Find affected repos
{"name": "meta_query_repos", "arguments": {"query": "tag:frontend"}}

// After making changes, run tests
{"name": "meta_run_tests", "arguments": {"tag": "frontend"}}

// If they failed, discard the changes; git clean -nd then lists
// new files, to remove by name
{"name": "meta_exec", "arguments": {
  "command": ["git", "restore", "--staged", "--worktree", "."],
  "tag": "frontend"
}}
```

### 4. Dependency-Aware Builds

`meta_build` and `meta_batch_execute` run in `.meta.yaml` order, not in dependency order. To build in dependency order:

```
1. meta_analyze_impact → See what a change affects
2. meta_execution_order → Get the order: dependencies before their dependents
3. Build each project in that order
```

**Example:**
```json
// See impact of changing shared-utils
{"name": "meta_analyze_impact", "arguments": {"project": "shared-utils"}}

// Get build order
{"name": "meta_execution_order", "arguments": {}}
```

Then build one project at a time in that order, for example from a shell with `meta --include shared-utils exec -- npm run build`, then the next.

### 5. Syncing Branches Across Repos

When synchronizing branches:

```
1. meta_git_branch → Check current branches
2. meta_git_fetch → Fetch latest
3. meta_git_checkout → Switch branches
4. meta_git_pull → Pull changes
```

**Example:**
```json
// Check branch status
{"name": "meta_git_branch", "arguments": {}}

// Fetch all remotes
{"name": "meta_git_fetch", "arguments": {}}

// Switch to feature branch
{"name": "meta_git_checkout", "arguments": {
  "branch": "feature/new-api",
  "create": true
}}
```

### 6. Atomic Operations with Rollback

For local changes that must succeed together:

```json
{"name": "meta_batch_execute", "arguments": {
  "command": ["sh", "-c", "npm test && npm run build"],
  "tag": "frontend",
  "atomic": true
}}
```

Atomic mode runs only when every selected project is a clean git repository. If any repo fails:
- The command stops, and each repo it reached is put back on its branch and commit from before; what the command left uncommitted is saved in a stash
- `rollback.status` is `success` when every one of them was; otherwise `rollback.failed` lists each repo it couldn't fully put back, with git's reason. Such a repo may be partly restored: its uncommitted changes may already be in a stash, which the reason then names
- `rollback.restored` names, for each repo put back, the stash holding its uncommitted changes and the commits it replaced

The rollback restores local git state only. It can't undo what the command did elsewhere: a `git push`, an `npm publish`, a deploy. Don't use it as a safety net for those.

## Best Practices

### 1. Always Check State First
Before any operation, query workspace state:
```json
{"name": "meta_workspace_state", "arguments": {}}
```

### 2. Use Tags for Scoping
Don't operate on all repos when you mean a subset:
```json
{"name": "meta_run_tests", "arguments": {"tag": "backend"}}
```

### 3. Commit or Stash Before Risky Operations
```json
{"name": "meta_exec", "arguments": {
  "command": ["git", "stash", "push", "-u", "-m", "before-refactor"]
}}
```

### 4. Handle Failures Gracefully
A tool's result has `isError: true` when the tool itself failed, or when every repository it acted in failed. When only some failed, `isError` is not set, so also check each repository's entry: `success: false` or an `error` field means that repository failed. An atomic `meta_batch_execute` that failed anywhere is an error as a whole: read its `rollback` before retrying.

## Error Recovery

When operations fail:

1. **Single repo failure**: Check error, fix issue, retry
2. **Multiple repo failure**: In each affected repo, discard uncommitted edits with `git restore --staged --worktree .`, list new files with `git clean -nd` and remove those by name (this assumes earlier work was committed or stashed first), and go back to an earlier commit with `git reflog` and `git switch -c recovered <sha>` (the reflog only recovers commits; switching carries uncommitted edits along)
3. **Build order failure**: Use `meta_execution_order` to find dependencies
4. **Git conflicts**: Manual intervention may be required

## Example: Full Feature Development Workflow

```
1. Check status and commit pending work: meta_git_status, meta_git_add, meta_git_commit
2. Create feature branch: meta_git_checkout with create=true
3. Find relevant code: meta_search_code
4. Make changes across repos
5. Check impact: meta_analyze_impact
6. Get build order: meta_execution_order, then build each project in that order
7. Run tests: meta_run_tests
8. If failed: discard the edits (`git restore --staged --worktree .`; `git clean -nd` lists new files to remove by name) or go back to an earlier commit (git reflog / git switch), fix issues
9. If passed: meta_git_add, meta_git_commit, meta_git_push
```
