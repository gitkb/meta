# Claude Code Skills for Meta

Meta includes six purpose-built Claude Code skills (`meta-workspace`, `meta-git`, `meta-exec`, `meta-plugins`, `meta-worktree`, `meta-safety`) that teach AI agents how to work effectively with multi-repository codebases. These skills are pedagogical—they explain HOW to work in a meta repo, not just WHAT commands exist.

## Installation

Install skills via the `meta init` command:

```bash
# Install skills to .meta/skills/, plus rules and hooks to .claude/
meta init claude

# Refresh skills and rules, keep settings.json
meta init claude --update
```

Each skill is installed as `.meta/skills/<name>/SKILL.md` with `name`/`description` frontmatter, in meta's own `.meta/` folder, and linked into each agent's skills folder, the same way atc (`.atc/skills/`) and git-kb (`.kb/skills/`) install theirs:

- `.claude/skills/<name> -> ../../.meta/skills/<name>` (Claude Code)
- `.agents/skills/<name> -> ../../.meta/skills/<name>` (Codex and other agents)
- `.cursor/skills/`, `.opencode/skills/`, `.pi/skills/`, `.omp/skills/`, `.grok/skills/`: only when that agent's folder (e.g. `.cursor/`) already exists

Commit `.meta/skills/` and the links so everyone working in the repo gets the skills (on Windows, see below).

**Windows.** Symlinks need Developer Mode or admin rights there. Where a link can't be made, `meta init claude` copies the skill folder from `.meta/skills/` instead and puts a `.meta-skill-copy` marker in it; `meta init claude --update` replaces marked copies with links once links work. A clone made with `core.symlinks=false` (Git for Windows' default) checks each committed link out as a small file holding its target path, which agents can't load: run `meta init claude --update` to replace those files with links or marked copies, and don't commit that change.

**Upgrading.** Older releases wrote flat `.claude/skills/meta-*.md` files (which Claude Code never loaded) and later real `.claude/skills/meta-*/` folders. `meta init claude --update` removes both and links the new layout. It only replaces what meta wrote: in `.claude/skills/`, a skill folder holding just the `SKILL.md` meta shipped (unchanged) and regular flat `meta-*.md` files starting with the heading meta shipped (e.g. `# Meta Git Skill`); anywhere, a marked copy or a Git link placeholder holding exactly meta's link target. Other files, folders and links, including another tool's link with a meta skill's name, are left alone and reported as skipped. A link is never followed or removed: an agent skills folder, rule file or `settings.json` reached through a link is skipped, and a link inside `.meta/skills/` stops the install with an error.

**Legacy `.meta` config file.** A repo whose config is the legacy `.meta` file can't also have the `.meta/` folder, so `meta init claude` refuses and asks you to rename the config first: `git mv .meta .meta.json` (same content), or convert it to `.meta.yaml`. Everywhere else, meta reads a `.meta` config only when it is a file, so the `.meta/` folder is never taken for config.

The skill text lives in `meta_cli/.claude/skills/` and is embedded in the `meta` binary. This repo's `.meta/skills/` (linked from `.claude/skills/` and `.agents/skills/`) and `claude-plugin/skills/` are copies: edit meta_cli, then run `make skills` (`tests/init.bats` fails when they differ). CI checks against meta_cli `main` (or the branch of the same name), so after a meta_cli skill change merges, run `make skills` here too, including on the automatic child-update sync PR. Bump `claude-plugin/.claude-plugin/plugin.json`'s version when the plugin's skills change, so plugin users get the update.

## Available Skills

### meta-workspace - Understanding Meta Repos

**Purpose:** Teaches Claude about meta repository structure and why multi-repo commands matter.

**Key concepts covered:**
- The `.meta` file structure (JSON and YAML formats)
- Simple vs extended project formats
- Tag-based filtering
- Nested meta repos and recursive operations
- Why `meta git` is better than `git` in workspaces

**When Claude should reference this skill:**
- When first entering a meta repository
- When asked about project structure
- When confused about repo relationships

### meta-git - Git Operations

**Purpose:** Multi-repo git operations with special attention to safe batch changes.

**Key concepts covered:**
- `meta git clone` and its queue-based recursive cloning
- `meta git update` for syncing workspaces
- Committing or branching before batch changes, and recovering with `git reflog`
- Safe workflow patterns (status → commit/branch → work → commit)

**When Claude should reference this skill:**
- Before any multi-repo git operation
- When making batch changes across repos
- When needing to undo workspace-wide changes

### meta-exec - Execution Model

**Purpose:** How commands run across repos, filtering options, and output modes.

**Key concepts covered:**
- `meta exec -- <cmd>` syntax for running arbitrary commands
- Plugin commands work directly (e.g., `meta git status`)
- Bare commands like `meta npm install` are **not supported**—use `meta exec -- npm install`
- Parallel vs sequential execution
- `--include`, `--exclude`, `--tag` filtering
- `--dry-run` for previewing operations
- JSON output mode for scripting
- Filter precedence (tag → include → exclude)

**When Claude should reference this skill:**
- When running arbitrary commands across repos
- When needing to filter target repositories
- When building scripts that use meta

### meta-plugins - Plugin System

**Purpose:** How plugins intercept commands and extend meta's behavior.

**Key concepts covered:**
- Plugin discovery locations
- Command routing: `meta exec --` runs commands as typed; other commands go to a plugin or error (no automatic fallback for bare commands)
- Built-in plugins (git, project, rust)
- Plugin management commands
- Why some commands behave "magically" (e.g., `meta git clone`)
- Understanding "unrecognized command" errors—use `meta exec --` for non-plugin commands

**When Claude should reference this skill:**
- When a command behaves unexpectedly
- When extending meta with custom behavior
- When troubleshooting command routing

### meta-worktree - Isolated Worktree Sets

**Purpose:** Create, use and remove multi-repo git worktree sets with `meta git worktree`.

**Key concepts covered:**
- Every change gets its own set; read-only work may stay in the primary checkout
- `create` with `--repo`, `--recursive`, `--no-deps`, a commit-ish or `--from-pr` (child repos only), plus a root-PR recipe
- `list`, `status`, `diff`, `exec`, `remove`, `prune`, TTLs and `--strict`
- Context detection inside `.worktrees/<name>/` and its limits

**When Claude should reference this skill:**
- Before changing code in a meta workspace
- When continuing a PR or testing a tag or SHA in isolation

### meta-safety - Multi-Repo Safety

**Purpose:** Precision operations and dependency awareness across repos.

**Key concepts covered:**
- Session start: `meta project list --recursive --json`, `meta git status`
- Filtering with `--include`/`--exclude` (whole repos: `.meta` name or path from the root) and `--tag`
- Dependency order via the MCP tool `meta_execution_order`
- Global and worktree `--strict` mode

**When Claude should reference this skill:**
- Before batch changes or commits across repos
- Before modifying a repo other repos depend on

## Skill Design Philosophy

These skills were designed with specific principles:

1. **Pedagogical, not encyclopedic** - Skills teach workflows and decision-making, not just command lists

2. **Context-aware** - Skills explain WHEN to use techniques, not just HOW

3. **Safety-first** - Committing first and dry-run are emphasized for batch operations

4. **AI-optimized** - Written for how AI agents process instructions, with clear patterns and examples

## Creating Custom Skills

Add a custom skill as `.claude/skills/<name>/SKILL.md` (or `.agents/skills/<name>/SKILL.md` for Codex). Keep custom skills out of `.meta/skills/`, which meta manages:

```markdown
---
name: my-skill
description: What the skill does and when Claude should use it
---

# Skill Name

Instructions for Claude...
```

### Best Practices for Custom Skills

1. **Focus on workflow, not reference** - Explain when and why, not just what
2. **Include concrete examples** - Show actual command sequences
3. **Highlight safety patterns** - Commit or branch first, dry-run, filtered operations
4. **Keep it scannable** - Use tables, code blocks, and clear headers

## Best Practices for AI Agents

1. **Use tags** - Filter operations by tag to avoid affecting unrelated projects
2. **Check status first** - Run `meta git status` before bulk operations
3. **Use atomic mode** - For risky operations, use atomic mode for auto-rollback
4. **Leverage queries** - Use query DSL to find specific repos by state
5. **Commit or branch first** - Before major changes, commit or create a backup branch so git can recover the work

## Integration with MCP Server

While skills provide guidance for CLI usage, Claude can also use the Meta MCP server for programmatic access. The MCP server exposes 26 tools:

| Skill Domain | Related MCP Tools |
|--------------|-------------------|
| meta-workspace | `meta_list_projects`, `meta_get_config`, `meta_workspace_state` |
| meta-git | `meta_git_*` |
| meta-exec | `meta_exec`, `meta_batch_execute`, `meta_query_repos` |
| meta-plugins | `meta_list_plugins` |

See [mcp_server.md](mcp_server.md) for full MCP documentation.

## Updating Skills

Skills are embedded in the `meta` binary and updated with each release. To get the latest skills:

1. Update meta: `brew upgrade meta-cli` or reinstall
2. Refresh skills: `meta init claude --update`

`--update` overwrites the skill and rule files with the latest versions, turns older layouts into links, and keeps `settings.json`; `--force` also overwrites `settings.json`.
