# Task: Implement `meta snapshot` for Workspace State Management

## Overview

Add workspace snapshot functionality that records the git state of all repos, enabling safe batch operations with rollback capability. Leverages git reflog for safety - all operations are reversible.

## Goal

Enable users and AI agents to safely perform batch operations across many repos with the ability to restore to a known good state if something goes wrong.

---

## Key Design Decision: Recursive by Default

Snapshots capture ALL repos in the workspace (root + nested) by default. No `--recursive` flag needed.

Rationale:
- The point of a snapshot is to capture the *entire* workspace state
- Partial snapshots would be confusing
- Users expect `meta snapshot restore` to fully restore their workspace

Optional filtering via `--include`, `--exclude`, or `--tag` if needed.

---

## Commands

```bash
meta snapshot create <name>       # Record current state of ALL repos
meta snapshot list                # List all snapshots
meta snapshot show <name>         # Show details of a snapshot
meta snapshot restore <name>      # Restore ALL repos to snapshot state
meta snapshot delete <name>       # Delete a snapshot
```

---

## Storage

**Location:** `.meta-snapshots/<name>.json` in the meta root directory

**Schema:**
```json
{
  "name": "before-upgrade",
  "created": "2026-01-03T10:30:00Z",
  "repos": {
    ".": {
      "sha": "a1b2c3d4e5f6...",
      "branch": "main",
      "dirty": false
    },
    "api-service": {
      "sha": "x9y8z7w6v5u4...",
      "branch": "feature/auth",
      "dirty": true,
      "stash_created": true
    }
  }
}
```

---

## Implementation

### Phase 1: Core snapshot module

**File:** `meta_git_lib/src/snapshot.rs` (new)

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoState {
    pub sha: String,
    pub branch: Option<String>,  // None if detached HEAD
    pub dirty: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stash_created: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub name: String,
    pub created: DateTime<Utc>,
    pub repos: HashMap<String, RepoState>,
}

// Core functions:
pub fn capture_repo_state(repo_path: &Path) -> Result<RepoState>
pub fn restore_repo_state(repo_path: &Path, state: &RepoState, force: bool) -> Result<()>
pub fn save_snapshot(meta_root: &Path, snapshot: &Snapshot) -> Result<()>
pub fn load_snapshot(meta_root: &Path, name: &str) -> Result<Snapshot>
pub fn list_snapshots(meta_root: &Path) -> Result<Vec<SnapshotInfo>>
pub fn delete_snapshot(meta_root: &Path, name: &str) -> Result<()>
```

### Phase 2: Git operations

**Capture state** (per repo):
```bash
git rev-parse HEAD                         # Get current SHA
git symbolic-ref --short HEAD 2>/dev/null  # Get branch (empty if detached)
git status --porcelain                     # Check if dirty
```

**Restore state** (per repo):
```bash
git stash push -m "meta-snapshot-auto-stash"  # If dirty
git checkout <sha>                             # Checkout to snapshot
git checkout -B <branch> <sha>                 # Restore branch if was on one
```

### Phase 3: CLI integration

**File:** `meta_git_cli/src/lib.rs` - add to `execute_command()`:
```rust
"snapshot create" => execute_snapshot_create(&args, &projects),
"snapshot list" => execute_snapshot_list(),
"snapshot show" => execute_snapshot_show(&args),
"snapshot restore" => execute_snapshot_restore(&args, &projects),
"snapshot delete" => execute_snapshot_delete(&args),
```

**File:** `meta_git_cli/src/main.rs` - register commands in PluginInfo

---

## Safety & Edge Cases

| Scenario | Behavior |
|----------|----------|
| Dirty repo on create | Record `dirty: true`, warn user |
| Dirty repo on restore | Auto-stash, set `stash_created: true` |
| Missing repo | Skip with warning |
| Detached HEAD | Record SHA only, no branch |
| SHA no longer exists | Error with reflog hint |
| Restore confirmation | Always prompt (unless `--force`) |

**Restore confirmation:**
```
$ meta snapshot restore before-upgrade
This will restore 25 repos to snapshot 'before-upgrade':
  - 23 repos will checkout to their recorded SHA
  - 2 repos have uncommitted changes (will be stashed)

Proceed? [y/N] y
```

**Flags:**
- `--force` - Skip confirmation
- `--dry-run` - Preview without changes

---

## Files to Modify

| File | Action |
|------|--------|
| `meta_git_lib/src/snapshot.rs` | Create - core logic |
| `meta_git_lib/src/lib.rs` | Add `pub mod snapshot;` |
| `meta_git_lib/Cargo.toml` | Add `chrono` dependency |
| `meta_git_cli/src/lib.rs` | Add command handlers |
| `meta_git_cli/src/main.rs` | Register commands |

---

## Example Usage

```bash
# Before risky operation
$ meta snapshot create before-upgrade
Captured state of 25 repos
  2 repos have uncommitted changes (recorded as dirty)
Snapshot saved: .meta-snapshots/before-upgrade.json

# Do batch operations
$ meta exec -- sh -c 'git pull && npm install'
# ... something breaks ...

# Restore
$ meta snapshot restore before-upgrade
Restoring 25 repos...
  api-service: a1b2c3d -> main
  frontend: x9y8z7w -> feature/auth (stashed dirty changes)
Restored 25 repos

# Clean up
$ meta snapshot delete before-upgrade
```

---

## MCP Tools (for AI agents)

| Tool | Description |
|------|-------------|
| `meta_snapshot_create` | Create snapshot, returns name and repo count |
| `meta_snapshot_list` | List snapshots with dates and repo counts |
| `meta_snapshot_restore` | Restore to snapshot, returns per-repo status |

---

## Testing

1. Create snapshot with clean repos
2. Create snapshot with dirty repos (verify warning)
3. Make changes, then restore
4. Verify reflog preserves pre-restore state
5. Test with missing repos
6. Test `--force` flag
7. Test `--dry-run` flag

---

## Why This Matters

- **Safety net** for batch operations across 50+ repos
- **Reversible** - git reflog preserves all state
- **AI-friendly** - agents can safely experiment
- **Simple mental model** - snapshot = save point
