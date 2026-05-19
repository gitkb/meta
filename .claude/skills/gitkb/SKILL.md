---
name: gitkb
description: Manage GitKB knowledge base for project documentation, tasks, and context. Use when working with KB documents, viewing tasks, updating progress, or managing project knowledge.
allowed-tools:
  - mcp__gitkb__*
  - Bash(git kb:*)
---

# GitKB Knowledge Base Skill

GitKB is a distributed knowledge base with a git-like CLI. Documents are stored in a local database and materialized to `.kb/workspace/` for editing.

## Common Gotchas

1. **Command syntax**: Use `git kb` (space, git subcommand), NOT `git-kb` (hyphen)
2. **No type-specific subcommands**: Use `git kb show <slug>`, NOT `git kb task show`
3. **Batch fetching**: Use `kb_show` with `slugs: [...]` array for multiple documents
4. **Board rendering**: Use CLI `git kb board` for ASCII, MCP `kb_board` for JSON
5. **Always check numbering**: Run `git kb list <type> --all` before creating new documents

## When to Use CLI vs MCP

**Use CLI (`git kb`) for:**
- ASCII board display (`git kb board`)
- Template management (`git kb templates`)
- Service control (`git kb service`)
- Interactive editing (opens $EDITOR)

**Use MCP tools for:**
- All document CRUD operations
- Workspace operations (checkout, commit, status, diff)
- Batch operations (multiple slugs)
- Programmatic access from Claude Code

## Complete MCP Tool Reference

### Document Operations

| Tool | Description | Key Parameters |
|------|-------------|----------------|
| `kb_list` | List documents with filtering | `type`, `status`, `tag`, `limit`, `offset` |
| `kb_show` | Get document(s) by slug | `slug` or `slugs: [...]` for batch |
| `kb_create` | Create new document | `title`, `type`, `slug`, `content`, `status`, `tags` |
| `kb_update` | Update document content/metadata | `slug`, `title`, `content`, `status`, `tags` |
| `kb_delete` | Delete document | `slug` |
| `kb_search` | Full-text search | `query`, `doc_type`, `limit` |
| `kb_set` | Quick metadata update (auto-commits) | `slug`, `status`, `priority`, `tags`, `assignee` |

### Relationship Operations

| Tool | Description | Key Parameters |
|------|-------------|----------------|
| `kb_link` | Add document to container | `child`, `container`, `position` |
| `kb_unlink` | Remove from container | `child`, `container` |
| `kb_graph` | Get relationship graph | `slug`, `depth`, `direction` |

### Workspace Operations

| Tool | Description | Key Parameters |
|------|-------------|----------------|
| `kb_checkout` | Materialize documents to workspace | `slugs`, `type`, `status`, `force` |
| `kb_status` | Show pending changes | (none) |
| `kb_commit` | Commit workspace changes | `message`, `author` |
| `kb_diff` | Show line-level diffs | (none) |
| `kb_stash` | Stash/restore changes | `action`, `message`, `index` |
| `kb_reset` | Discard workspace changes | `pathspecs` |
| `kb_clear` | Remove from workspace | `pathspecs`, `force`, `dry_run` |

### Additional Operations

| Tool | Description | Key Parameters |
|------|-------------|----------------|
| `kb_board` | Kanban board (JSON) | `type`, `tags`, `all` |
| `kb_context` | Bootstrap context bundle | `task`, `include_content`, `commit_limit` |
| `kb_mv` | Move/rename document | `source`, `dest`, `force` |
| `kb_log` | Commit history | `slug`, `limit` |
| `kb_export` | Export documents | `format`, `type`, `status`, `tag` |
| `kb_backup` | Create full backup | (none) |
| `kb_restore` | Restore from backup | `data`, `skip_documents`, `skip_commits` |

### Code Intelligence

| Tool | Description | Key Parameters |
|------|-------------|----------------|
| `kb_symbols` | Query indexed code symbols | `file_path`, `kind`, `language`, `search`, `limit` |
| `kb_index` | Index source code symbols from files | `paths`, `force`, `dry_run`, `language`, `prune` |
| `kb_callers` | Find all callers of a function/method | `symbol`, `depth`, `limit` |
| `kb_callees` | Find all functions/methods called by a symbol | `symbol`, `depth`, `limit` |
| `kb_impact` | Analyze change blast radius via call graph | `file_path`, `depth` |
| `kb_dead_code` | Find potentially dead code (no callers) | `file_path`, `kind`, `include_tests` |
| `kb_symbol_refs` | Find KB documents referencing a code symbol | `symbol` |
| `kb_semantic` | Semantic search across docs and code | `query`, `scope`, `doc_type`, `language` |
| `kb_embed` | Generate embeddings for docs/code | `scope`, `force` |

**Skills for code intelligence workflows:**
- `/understand <file|symbol>` — Analyze structure and dependencies
- `/before-refactor <symbol>` — Safety check: callers, callees, impact analysis
- `/explore <query>` — Semantic search to find relevant code and docs

**Prerequisite:** Code must be indexed first with `git kb index`. For semantic search, embeddings must be generated with `git kb embed`.

### CLI Commands (for features not available in MCP)

```bash
# ASCII Kanban board view
git kb board                    # Tasks only (default)
git kb board --all              # All document types

# Template management
git kb templates list           # List available templates
git kb templates show <name>    # Show template content

# Service control
git kb service start            # Start daemon
git kb service stop             # Stop daemon
git kb service status           # Check status
```

## Workflows

### Starting Work on a Task

1. View available tasks:
   ```
   kb_list with type: "task", status: "active"
   ```

2. Checkout task to workspace:
   ```
   kb_checkout with slugs: ["tasks/my-task"]
   ```

3. Read the file at `.kb/workspace/tasks/my-task.md`

4. Make changes and commit:
   ```
   kb_commit with message: "Progress on feature", author: "Claude <claude@anthropic.com>"
   ```

### Completing a Task or Incident

**IMPORTANT**: Before changing status to `completed`, `done`, or `resolved`:

1. **Update the document content** with completion evidence:
   - Mark acceptance criteria as checked (`- [x]`)
   - Add "Completion Evidence" section with commit hashes, changes made
   - Document any follow-up items or remaining work

2. **Then** update the status:
   ```
   kb_set with slug: "tasks/my-task", status: "completed"
   ```

**Never mark a document complete without first updating its content to reflect completion.**

### Quick Metadata Updates

Use `kb_set` for metadata changes (auto-commits):
```
kb_set with slug: "tasks/my-task", status: "active"
kb_set with slug: "tasks/my-task", priority: "high"
```

### Creating a New Task

```
kb_create with:
  title: "Implement feature X"
  type: "task"
  slug: "tasks/my-task"
  status: "backlog"
  tags: ["feature"]
  content: "## Overview\n\nDescription here..."
```

### Viewing Project Context

1. List context documents:
   ```
   kb_list with type: "context"
   ```

2. Show specific context:
   ```
   kb_show with slug: "context/overridable/active"
   ```

### Batch Document Fetching (Efficient Bootstrap)

When loading multiple documents, use `kb_show` with `slugs` array to fetch them in a single call:

```
kb_show with slugs: ["context/architecture", "context/patterns", "tasks/my-task"]
```

This is more efficient than multiple individual `kb_show` calls.

**JSON Response Format** (always wrapped):
```json
{
  "count": 2,
  "documents": [
    { "slug": "...", "title": "...", "content": "...", ... },
    { "slug": "...", "title": "...", "content": "...", ... }
  ],
  "not_found": ["missing-slug"]
}
```

- `count`: Number of documents found
- `documents`: Array of document objects (in requested order)
- `not_found`: Slugs that couldn't be resolved (partial success allowed)

## Key Concepts

| Term | Definition |
|------|------------|
| **Workspace** | `.kb/workspace/` - Files materialized for editing |
| **Checkout** | Materialize document from DB to workspace |
| **Commit** | Sync workspace changes back to database |
| **Slug** | Human-readable document ID (e.g., `tasks/my-task`) |
| **Wikilink** | `[[slug]]` reference between documents; also `[[commit:repo@sha]]` for commit refs |

## Document Types

- `task` - Work items with status tracking
- `note` - General documentation
- `spec` - Technical specifications
- `context` - Project context documents
- `brief` - Project brief
- `architecture` - Architecture documentation
- `patterns` - Design patterns

## Status Values

- `draft` - Initial state
- `backlog` - Queued for work
- `active` - Currently in progress
- `blocked` - Waiting on dependency
- `completed` - Finished
- `done` - Archived/closed

## Multi-Agent Tracing

When `GITKB_AGENT_ID` env var is set, KB commits automatically append `Agent: <id>` to the commit message. This enables multi-agent traceability.

## Commit References

Use `[[commit:repo@sha]]` wikilinks in KB documents to link to code commits:
```markdown
## Implementation Commits
- [[commit:core@8d56c6f]] - Added feature X
- [[commit:frontend@abc1234]] - Updated UI
```

These are parsed by WikilinkExtractor into `references_commit` graph edges, creating bidirectional traceability between KB documents and code commits across repositories.

## Document Naming Conventions

**Always check existing documents before creating** to ensure consistent numbering:
```
git kb list <type> --all
```

| Type | Pattern | Example |
|------|---------|---------|
| Task | `tasks/{prefix}-{N}` | `tasks/my-project-1` |
| Incident | `incidents/inc-{NNN}-{slug}` | `incidents/inc-001-auth-timeout` |
| Context | `context/{category}/{name}` | `context/overridable/active` |
| Note | `notes/{slug}` | `notes/api-design` |
| Spec | `specs/{slug}` | `specs/federation-protocol` |

**Context categories:**
- `immutable/` - Core project docs that rarely change
- `extensible/` - Growing reference docs
- `overridable/` - Frequently updated status docs
