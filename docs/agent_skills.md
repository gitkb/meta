# Agent Skills for Meta

Meta includes six purpose-built agent skills (`meta-workspace`, `meta-git`, `meta-exec`, `meta-plugins`, `meta-worktree`, `meta-safety`) that teach AI agents how to work effectively with multi-repository codebases. These skills are pedagogical—they explain HOW to work in a meta repo, not just WHAT commands exist.

## Installation

Install skills via the `meta init` command:

```bash
# Install skills to ~/.meta/skills/, linked for every agent, plus Claude Code's hooks in this workspace
meta init skills

# Refresh the skills, keep settings.json
meta init skills --update

# Overwrite meta's skill files, and merge settings.json
meta init skills --force

# Install in this workspace instead of the home folder, to be committed
meta init skills --local

# Take meta's skills out of this workspace
meta init skills --remove-local
```

**Where.** `meta init skills` installs the skills once, in your home folder, so every project and every worktree set has them and they match the `meta` you have installed; run it again with `--update` after upgrading meta. It works from any folder. No workspace carries a copy of meta's skills. Only Claude Code's hooks go in a workspace: the one you run it in, which is the nearest folder at or above the current one that has a `.meta.yaml` (a nested meta repo is a workspace of its own). Outside a workspace it installs the skills and says it added no hooks. `meta --dry-run init skills` lists what it would write and writes nothing.

**In the workspace instead (`--local`).** `meta init skills --local` installs the skills in the workspace (`.meta/skills/`, linked into `.agents/skills/` and `.claude/skills/`) and nothing in your home folder, as older releases always did. Use it to keep meta's skills out of the projects of yours that aren't meta workspaces, or to give them to everyone who clones the repo: commit `.meta/skills/` and the links. Refresh them with `meta init skills --local --update` after upgrading meta. Each skill's description says it applies only in a meta workspace, so with the home install an agent in another project has six lines of descriptions in its list and no reason to load one.

**Home and workspace both.** A workspace that has its own copies (from `--local` or an older meta) while your home folder has them too makes agents there list each skill twice. A home install never changes a workspace's copies; it says how many there are. `meta init skills --remove-local` takes them out of the workspace: each link meta made, and each `.meta/skills/<name>/SKILL.md` while it is byte for byte one meta wrote, with the flat skill files and rule files of older releases. A skill file you edited is kept with its links and named; your own skills, other tools' and `.claude/settings.json` are never touched. It installs nothing. Commit the deletions.

**settings.json (Claude Code only).** The skills are the guidance, for every agent. As a convenience for Claude Code, `meta init skills` also adds meta's hooks (`SessionStart` and `PreCompact` run `meta -r context`, the workspace with its nested meta repos' projects; `PreToolUse` runs `agent guard` for Bash) to the workspace's `.claude/settings.json`, keeping everything else in it; it writes `~/.claude/settings.json` only when your home folder is itself the workspace (it has the `.meta.yaml`). Commit that file so everyone working in the repo gets the hooks. The `agent guard` hook is added only when gitkb's [agent](https://github.com/gitkb/agent) CLI is installed (`agent --help` says "Claude Code agent toolkit"; another program named `agent` doesn't count). The hook itself checks that again on each call and always exits 0, so a missing, foreign or failing `agent` never blocks a Bash call; gitkb's guard refuses a command by printing its decision. A hook an older meta wrote (`agent guard` alone) is rewritten to this one by `meta init skills`, and so is an older context hook (`meta context`, which left nested repos out), so a rerun leaves one context hook per event; without it, init says it skipped the hook, and running `meta init skills` again after installing agent adds it. A hook already in the file is never removed. A hook group you added your own hooks to still counts as meta's, so a rerun doesn't add it a second time. It never replaces the file, `--force` included. It merges only into the layout Claude Code documents: a JSON object whose `hooks` is an object, with `SessionStart`, `PreToolUse` and `PreCompact` (the events meta adds to) each an array. Anything else (invalid JSON, an array, `hooks` or one of those three events in another shape) stops the command with an error naming the file and the key, before anything is written, so you can fix the file and rerun. The output says what it did: created the file, added which hooks, or found meta's hooks already there. `--update` leaves settings.json alone.

**Plugin marketplace.** Claude Code can also get the skills as a plugin, with meta's MCP server: `claude plugin marketplace add gitkb/claude-plugins`, then `claude plugin install meta@gitkb`. Use one or the other: with both, Claude Code lists each skill twice.

Each skill is installed as `~/.meta/skills/<name>/SKILL.md` with `name`/`description` frontmatter, in meta's own folder in your home folder, and linked into the two user-level skills folders agents read:

- `~/.agents/skills/<name> -> ../../.meta/skills/<name>`: read by Codex, and by the other agents that read a user-level `.agents/skills`
- `~/.claude/skills/<name> -> ../../.meta/skills/<name>`: read by Claude Code

Older releases installed all of this in the workspace instead, as `--local` does. They also linked into `.cursor/skills/`, `.opencode/skills/`, `.pi/skills/`, `.omp/skills/` and `.grok/skills/`, and installed two rule files in `.claude/rules/` (`meta-workspace-discipline.md` and `meta-destructive-commands.md`), which only Claude Code read. Their guidance is now in the `meta-workspace` and `meta-safety` skills. `--local --update`, `--local --force` and `--remove-local` remove those links and marked copies meta made, and each rule file while it is byte for byte one meta wrote; anything else there (your own skills and rules, an edited rule) stays, and an edited rule is named.

**Windows.** Symlinks need Developer Mode or admin rights there. Where a link can't be made, `meta init skills` copies the skill folder from `~/.meta/skills/` instead and puts a `.meta-skill-copy` marker in it; `meta init skills --update` replaces marked copies with links once links work. With `--local`, a clone made with `core.symlinks=false` (Git for Windows' default) checks each committed link out as a small file holding its target path, which agents can't load: run `meta init skills --local --update` to replace those files with links or marked copies, and don't commit that change.

**What meta replaces.** `meta init skills --update` (or `--force`) overwrites meta's own files (in the workspace with `--local`, where the paths below have no `~/`): `~/.meta/skills/<name>/SKILL.md` while it is byte for byte a version some meta wrote (an edited one is kept and named, so a change of yours is never lost; move it away to get meta's version). In the agent skills folders (`~/.agents/skills/`, `~/.claude/skills/`) it only replaces what meta wrote there: a marked copy, or a Git link placeholder holding exactly meta's link target. Other regular files, folders and links there are left alone and reported as skipped, in every mode. Older releases wrote each skill as a flat `.claude/skills/<name>.md` file in the workspace, which Claude Code doesn't load: `--local --update`, `--local --force` and `--remove-local` delete such a file only while it is byte for byte one an older meta wrote, and name one that was changed since, leaving it. Links on the way are followed and kept: the workspace's `.claude/settings.json`, when it is a link, is merged atomically at its target and stays a link (`--update` still leaves settings alone), and a linked agent skills folder (say `~/.claude/skills` stowed from dotfiles) gets meta's links, made to point back at `~/.meta/skills`. A linked folder that leads nowhere is an error naming it, as for `mkdir -p`.

**The one place.** Everything an agent needs to use meta is in these skills, so every agent gets the same guidance in every workspace from one `meta init skills`. A repo's own AGENTS.md or skills hold only that repo's policy (which repos, which build commands), never how meta works.

**Editing the skills.** Each skill has one source, `claude-plugin/skills/<name>/SKILL.md`, which is embedded in the `meta` binary and is also what the Claude Code plugin ships (`claude-plugin/` is that plugin's folder). In this repo, `.claude/skills/meta-*` and `.agents/skills/meta-*` are committed links to `../../claude-plugin/skills/<name>`, so agents working on meta load the text you edit (`tests/init.bats` checks the links and that `meta init skills` installs that text). On Windows, a contributor needs Developer Mode and `git config core.symlinks true` before checking out, or those links arrive as small files holding their target path. When skill text changes, add the new text's line to `meta_cli/src/shipped.txt` (a unit test fails and names the line): that list is how `--update` tells a file meta wrote from one you edited.

## Available Skills

| Skill | What it teaches |
|-------|-----------------|
| `meta-workspace` | The `.meta.yaml` format, tags, nested meta repos, the pull-only primary checkout, and why `meta git` and `meta exec` beat `git` and `cd` in a workspace; for a first look at a meta repo |
| `meta-git` | `meta git clone` and `update`, committing before batch changes, recovering with `git reflog`, undoing a commit |
| `meta-exec` | `meta exec -- <cmd>` (as typed, no shell), parallel and sequential runs, `--include`/`--exclude`/`--tag`, `--dry-run`, JSON output |
| `meta-plugins` | Plugin discovery, how commands are routed to plugins, the built-in git, project and cargo plugins, installing plugins |
| `meta-worktree` | Creating, using and removing multi-repo worktree sets with `meta git worktree`; for any change, a PR, adding a repo, or testing a tag or SHA |
| `meta-safety` | Session start, precise filtering, what to do before destructive commands and what `agent guard` refuses, recovery, dependency order (the MCP tool `meta_execution_order`), `--strict` |

## Creating Custom Skills

Add a custom skill for one project as `.agents/skills/<name>/SKILL.md` in it (and `.claude/skills/<name>/SKILL.md` for Claude Code), or for all your projects in the same folders under your home folder. Keep custom skills out of `~/.meta/skills/`, which meta manages:

```markdown
---
name: my-skill
description: What the skill does and when an agent should use it
---

# Skill Name

Instructions for the agent...
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
2. Refresh skills: `meta init skills --update`
