# Claude Code Skills for Meta

Meta includes six purpose-built Claude Code skills (`meta-workspace`, `meta-git`, `meta-exec`, `meta-plugins`, `meta-worktree`, `meta-safety`) that teach AI agents how to work effectively with multi-repository codebases. These skills are pedagogical—they explain HOW to work in a meta repo, not just WHAT commands exist.

## Installation

Install skills via the `meta init` command:

```bash
# Install skills to .meta/skills/, plus rules and hooks to .claude/
meta init claude

# Refresh skills and rules, keep settings.json
meta init claude --update

# Overwrite meta's skill and rule files, and merge settings.json
meta init claude --force
```

**Where.** `meta init claude` installs in the workspace you run it in: the nearest folder at or above the current one that has a `.meta.yaml` (a nested meta repo is a workspace of its own), so running it from inside a project repo or a subfolder still installs once, there (the output names it). Outside a workspace it stops with an error and writes nothing. `meta --dry-run init claude` lists what it would write and writes nothing.

**settings.json.** `meta init claude` adds meta's hooks (`SessionStart` and `PreCompact` run `meta context`, `PreToolUse` runs `agent guard` for Bash) to `.claude/settings.json`, keeping everything else in it. The `agent guard` hook is added only when the [agent](https://github.com/gitkb/agent) CLI is installed, since Claude Code would otherwise run a command that doesn't exist; without it, init says it skipped the hook, and running `meta init claude` again after installing agent adds it. A hook already in the file is never removed. A hook group you added your own hooks to still counts as meta's, so a rerun doesn't add it a second time. It never replaces the file, `--force` included. It merges only into the layout Claude Code documents: a JSON object whose `hooks` is an object, with `SessionStart`, `PreToolUse` and `PreCompact` (the events meta adds to) each an array. Anything else (invalid JSON, an array, `hooks` or one of those three events in another shape) stops the command with an error naming the file and the key, before anything is written, so you can fix the file and rerun. The output says what it did: created the file, added which hooks, or found meta's hooks already there. `--update` leaves settings.json alone.

**Plugin marketplace.** `meta init claude` writes only inside the workspace. To get the same skills in every project through Claude Code's plugin system, run the two commands it prints: `claude plugin marketplace add gitkb/claude-plugins`, then `claude plugin install meta@gitkb`.

Each skill is installed as `.meta/skills/<name>/SKILL.md` with `name`/`description` frontmatter, in meta's own `.meta/` folder, and linked into each agent's skills folder, the same way atc (`.atc/skills/`) and git-kb (`.kb/skills/`) install theirs:

- `.claude/skills/<name> -> ../../.meta/skills/<name>` (Claude Code)
- `.agents/skills/<name> -> ../../.meta/skills/<name>` (Codex and other agents)
- `.cursor/skills/`, `.opencode/skills/`, `.pi/skills/`, `.omp/skills/`, `.grok/skills/`: only when that agent's folder (e.g. `.cursor/`) already exists

Commit `.meta/skills/` and the links so everyone working in the repo gets the skills (on Windows, see below).

**Windows.** Symlinks need Developer Mode or admin rights there. Where a link can't be made, `meta init claude` copies the skill folder from `.meta/skills/` instead and puts a `.meta-skill-copy` marker in it; `meta init claude --update` replaces marked copies with links once links work. A clone made with `core.symlinks=false` (Git for Windows' default) checks each committed link out as a small file holding its target path, which agents can't load: run `meta init claude --update` to replace those files with links or marked copies, and don't commit that change.

**What meta replaces.** `meta init claude --update` (or `--force`) overwrites meta's own files: `.meta/skills/<name>/SKILL.md` and its rule files `.claude/rules/meta-workspace-discipline.md` and `meta-destructive-commands.md`. In the agent skills folders (`.claude/skills/`, `.agents/skills/`, ...) it only replaces what meta wrote there: a marked copy, or a Git link placeholder holding exactly meta's link target. Other regular files, folders and links there are left alone and reported as skipped, in every mode. Links on the way are followed and kept: a linked `.claude/rules` folder or rule file gets the rules where it leads, a linked `.claude/settings.json` is merged atomically at its target and stays a link (`--update` still leaves settings alone), and a linked agent skills folder (say `.claude/skills` stowed from dotfiles) gets meta's links, made to point back at the workspace's skills. A linked folder that leads nowhere is an error naming it, as for `mkdir -p`.

**Editing the skills.** Each skill has one source, `claude-plugin/skills/<name>/SKILL.md`, which is embedded in the `meta` binary and is also what the Claude Code plugin ships. In this repo, `.claude/skills/meta-*` and `.agents/skills/meta-*` are committed links to `../../claude-plugin/skills/<name>`, so agents working on meta load the text you edit (`tests/init.bats` checks the links and that `meta init claude` installs that text). On Windows, a contributor needs Developer Mode and `git config core.symlinks true` before checking out, or those links arrive as small files holding their target path. When skill text changes, bump the version in `claude-plugin/.claude-plugin/plugin.json` so plugin users get the update.

## Available Skills

| Skill | What it teaches |
|-------|-----------------|
| `meta-workspace` | The `.meta.yaml` format, tags, nested meta repos, and why `meta git` beats `git` in a workspace; for a first look at a meta repo |
| `meta-git` | `meta git clone` and `update`, committing or stashing before batch changes, recovering with `git reflog` |
| `meta-exec` | `meta exec -- <cmd>` (as typed, no shell), parallel and sequential runs, `--include`/`--exclude`/`--tag`, `--dry-run`, JSON output |
| `meta-plugins` | Plugin discovery, how commands are routed to plugins, the built-in git, project and cargo plugins, installing plugins |
| `meta-worktree` | Creating, using and removing multi-repo worktree sets with `meta git worktree`; for any change, a PR, or testing a tag or SHA |
| `meta-safety` | Session start, precise filtering, dependency order (the MCP tool `meta_execution_order`), `--strict` |

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
3. **Highlight safety patterns** - Commit or stash first, dry-run, filtered operations
4. **Keep it scannable** - Use tables, code blocks, and clear headers

## MCP Server

Skills guide the CLI; the [MCP server](mcp_server.md) gives agents the same workspace as structured tools.

## Updating Skills

Skills are embedded in the `meta` binary and updated with each release. To get the latest skills:

1. Update meta: `brew upgrade meta-cli` or reinstall
2. Refresh skills: `meta init claude --update`
