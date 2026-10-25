# Agent Workflows with Meta

This guide covers common patterns for AI agents working with multi-repository codebases using Meta's MCP tools.

## Understanding Multi-Repo Context

When working with meta repositories, agents need to:
1. Understand the overall workspace structure
2. Track state across multiple repos
3. Execute operations safely with rollback capability
4. Respect dependency ordering

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
2. Commit them, or create a backup branch (meta_exec with git branch)
3. meta_query_repos → Find repos to modify
4. Make changes
5. meta_run_tests → Verify changes
6. If tests fail: revert with git (git restore, git reflog / git switch)
7. If tests pass: meta_git_commit → Commit changes
```

**Example:**
```json
// Check state, then create a backup branch
{"name": "meta_git_status", "arguments": {
  "tag": "frontend"
}}
{"name": "meta_exec", "arguments": {
  "command": "git branch backup/pre-dependency-update",
  "tag": "frontend"
}}

// Find affected repos
{"name": "meta_query_repos", "arguments": {
  "query": "tag:frontend"
}}

// After making changes, run tests
{"name": "meta_run_tests", "arguments": {
  "tag": "frontend"
}}

// If failed, discard the changes
{"name": "meta_exec", "arguments": {
  "command": "git restore .",
  "tag": "frontend"
}}
```

### 4. Dependency-Aware Builds

When building projects with dependencies:

```
1. meta_analyze_impact → Understand what's affected
2. meta_execution_order → Get correct build order
3. meta_build → Build in correct order
```

**Example:**
```json
// See impact of changing shared-utils
{"name": "meta_analyze_impact", "arguments": {
  "project": "shared-utils"
}}

// Get build order
{"name": "meta_execution_order", "arguments": {}}

// Build with dependencies
{"name": "meta_batch_execute", "arguments": {
  "command": ["npm", "run", "build"],
  "atomic": true
}}
```

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

For operations that must succeed together:

```json
{"name": "meta_batch_execute", "arguments": {
  "command": ["sh", "-c", "npm test && npm run build"],
  "tag": "frontend",
  "atomic": true
}}
```

If any repo fails:
- Operation stops
- All repos are rolled back to pre-execution state
- Error details are returned

## Query DSL Reference

The query DSL allows precise filtering:

| Query | Meaning |
|-------|---------|
| `dirty:true` | Repos with uncommitted changes |
| `dirty:false` | Clean repos |
| `branch:main` | Repos on main branch |
| `branch:feature/*` | Repos on feature branches |
| `tag:backend` | Repos with backend tag |
| `ahead:true` | Repos ahead of remote |
| `behind:true` | Repos behind remote |

Combine with AND:
```
dirty:true AND tag:backend AND branch:main
```

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

### 3. Commit or Branch Before Risky Operations
```json
{"name": "meta_exec", "arguments": {
  "command": "git branch backup/before-refactor"
}}
```

### 4. Use Atomic Mode for Critical Operations
```json
{"name": "meta_batch_execute", "arguments": {
  "command": ["npm", "publish"],
  "atomic": true
}}
```

### 5. Respect Dependency Order
Use `meta_execution_order` before building:
```json
{"name": "meta_execution_order", "arguments": {"tag": "backend"}}
```

### 6. Handle Failures Gracefully
Always check the `is_error` field in responses and handle appropriately.

## Error Recovery

When operations fail:

1. **Single repo failure**: Check error, fix issue, retry
2. **Multiple repo failure**: Revert with git in each affected repo (`git reflog` to find the previous commit, then `git switch -c recovered <sha>`; the reflog only recovers commits)
3. **Build order failure**: Use `meta_execution_order` to find dependencies
4. **Git conflicts**: Manual intervention may be required

## Example: Full Feature Development Workflow

```
1. Check status and commit pending work: meta_git_status, meta_git_commit
2. Create feature branch: meta_git_checkout with create=true
3. Find relevant code: meta_search_code
4. Make changes across repos
5. Check impact: meta_analyze_impact
6. Get build order: meta_execution_order
7. Build in order: meta_batch_execute
8. Run tests: meta_run_tests
9. If failed: revert with git (git reflog / git switch), fix issues
10. If passed: meta_git_add, meta_git_commit, meta_git_push
```

This workflow ensures safe, coordinated changes across multiple repositories with the ability to rollback if anything goes wrong.
