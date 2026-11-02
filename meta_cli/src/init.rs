//! Initialize Claude Code integration for meta repositories.
//!
//! This module provides the `meta init claude` command which installs
//! Claude Code skill files, rules, and hook configuration into the current
//! project's `.claude/` directory.

use anyhow::{Context, Result};
use colored::*;
use meta_core::config;
use meta_core::display::escape_path;
use meta_core::outln;
use meta_core::ExecutionMode;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::symlink as symlink_dir;
#[cfg(windows)]
use std::os::windows::fs::symlink_dir;

/// Embeds `.claude/skills/<name>/SKILL.md`, installed at the same path
macro_rules! skill {
    ($name:literal) => {
        (
            $name,
            include_str!(concat!("../.claude/skills/", $name, "/SKILL.md")),
        )
    };
}

/// All available skills with their names
const SKILLS: &[(&str, &str)] = &[
    skill!("meta-workspace"),
    skill!("meta-git"),
    skill!("meta-exec"),
    skill!("meta-plugins"),
    skill!("meta-worktree"),
    skill!("meta-safety"),
];

/// An agent skills folder each skill is linked into, named as in atc's agent registry.
struct AgentDir {
    path: &'static str,
    /// Always linked; otherwise only when the agent's folder (e.g. `.cursor/`) exists
    always: bool,
}

const AGENT_SKILL_DIRS: &[AgentDir] = &[
    AgentDir {
        path: ".claude/skills",
        always: true,
    },
    // Codex and other agents
    AgentDir {
        path: ".agents/skills",
        always: true,
    },
    AgentDir {
        path: ".cursor/skills",
        always: false,
    },
    AgentDir {
        path: ".opencode/skills",
        always: false,
    },
    AgentDir {
        path: ".pi/skills",
        always: false,
    },
    AgentDir {
        path: ".omp/skills",
        always: false,
    },
    AgentDir {
        path: ".grok/skills",
        always: false,
    },
];

/// Embedded rule files (always-loaded, survive compaction)
const RULE_WORKSPACE_DISCIPLINE: &str =
    include_str!("../.claude/rules/meta-workspace-discipline.md");
const RULE_DESTRUCTIVE_COMMANDS: &str =
    include_str!("../.claude/rules/meta-destructive-commands.md");

/// All available rules with their filenames
const RULES: &[(&str, &str)] = &[
    ("meta-workspace-discipline.md", RULE_WORKSPACE_DISCIPLINE),
    ("meta-destructive-commands.md", RULE_DESTRUCTIVE_COMMANDS),
];

/// `meta init claude`: `force` refreshes skills and rules and merges meta's
/// hooks into the settings; `update` updates skills and rules only, keeping
/// the settings.
pub fn init_claude(force: bool, update: bool, mode: ExecutionMode, verbose: bool) -> Result<()> {
    let init = Init {
        mode,
        verbose,
        guard: agent_cli_available(),
    };
    install_claude_integration(force, update, &init)
}

/// One run of `meta init claude`. Every change it makes to files goes
/// through here, so a preview (`--dry-run`) makes none: it says what it
/// would do instead.
struct Init {
    mode: ExecutionMode,
    verbose: bool,
    /// `agent` is installed, so the PreToolUse hook `agent guard` can run
    guard: bool,
}

impl Init {
    fn create_dir_all(&self, dir: &Path) -> Result<()> {
        if self.mode.is_preview() {
            return Ok(());
        }
        fs::create_dir_all(dir).with_context(|| format!("Failed to create {}", escape_path(dir)))
    }

    fn write_file(&self, path: &Path, content: &str) -> Result<()> {
        if self.mode.is_preview() {
            outln!("  would write {}", escape_path(path))?;
            return Ok(());
        }
        write_file(path, content, self.verbose)
    }

    fn write_settings(&self, path: &Path, content: &str) -> Result<()> {
        if self.mode.is_preview() {
            return Ok(());
        }
        write_settings_atomic(path, content)
    }

    fn remove_owned(&self, link: &Path, marker: &str) -> Result<()> {
        if self.mode.is_preview() {
            outln!("  would replace {}", escape_path(link))?;
            return Ok(());
        }
        remove_owned(link, marker)
    }

    /// Link `link` to `target` with `make_link`; where that fails, copy the
    /// skill at `source` there. Whether it was copied.
    fn link_skill(
        &self,
        target: &Path,
        link: &Path,
        source: &Path,
        marker: &str,
        make_link: impl Fn(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<bool> {
        if self.mode.is_preview() {
            outln!(
                "  would link {} -> {}",
                escape_path(link),
                escape_path(target)
            )?;
            return Ok(false);
        }
        match make_link(target, link) {
            Ok(()) => {
                if self.verbose {
                    outln!(
                        "{} {} -> {}",
                        "Linked".green(),
                        escape_path(link),
                        escape_path(target)
                    )?;
                }
                Ok(false)
            }
            // Windows needs Developer Mode or admin rights for symlinks; some
            // filesystems have none. Copy instead.
            Err(err) => {
                copy_skill(source, link, marker)?;
                outln!(
                    "{} {} (symlink failed: {err})",
                    "Copied".yellow(),
                    escape_path(link)
                )?;
                Ok(true)
            }
        }
    }
}

/// Install Claude Code skills and hook configuration at the root of the
/// workspace the current directory is in: the nearest `.meta.yaml` at or
/// above it.
fn install_claude_integration(force: bool, update: bool, init: &Init) -> Result<()> {
    let current_dir = std::env::current_dir()?;
    let Some(config) = config::find_meta_config(&current_dir, None)? else {
        anyhow::bail!(
            "No .meta.yaml in {} or any folder above it. `meta init claude` sets up \
             Claude Code in a meta workspace; run it inside one.",
            escape_path(&current_dir)
        );
    };
    let root = config.parent().unwrap_or(&current_dir);
    if root != current_dir {
        outln!(
            "Installing in the workspace at {} (the nearest .meta.yaml)",
            escape_path(root)
        )?;
    }
    install_claude_integration_to(root, force, update, init)
}

/// Install Claude Code skills and hook configuration into a specific directory
fn install_claude_integration_to(
    target_dir: &Path,
    force: bool,
    update: bool,
    init: &Init,
) -> Result<()> {
    let claude_dir = target_dir.join(".claude");
    let rules_dir = claude_dir.join("rules");
    let verbose = init.verbose;
    let preview = init.mode.is_preview();
    if preview {
        outln!("[DRY RUN] In {}:", escape_path(target_dir))?;
    }

    // Build and validate settings before writing any skills or rules.
    let settings_path = claude_dir.join("settings.json");
    let settings_plan = if update {
        None
    } else {
        Some(prepare_settings(&settings_path, init)?)
    };

    // --force or --update: overwrite; default: skip existing
    let overwrite_content = force || update;
    let skills = install_skills(
        target_dir,
        "meta",
        SKILLS,
        AGENT_SKILL_DIRS,
        overwrite_content,
        init,
    )?;
    let mut installed = skills.written;
    let mut skipped = skills.skipped;

    // Install rule files
    // --force or --update: overwrite; default: skip existing
    init.create_dir_all(&rules_dir)?;
    for (filename, content) in RULES {
        let target_path = rules_dir.join(filename);

        if target_path.exists() && !overwrite_content {
            if verbose {
                outln!("{} {} (already exists)", "Skipped".yellow(), filename)?;
            }
            skipped += 1;
            continue;
        }

        init.write_file(&target_path, content)?;
        installed += 1;
    }

    // --update leaves settings untouched; --force uses the same merge as default.
    if update {
        outln!("Skipped .claude/settings.json (--update preserves settings)")?;
    } else if let Some(plan) = settings_plan {
        if let Some(content) = plan.content {
            init.write_settings(&settings_path, &content)?;
            installed += 1;
        }
        outln!("{} {}", plan.message, escape_path(&settings_path))?;
    }
    let present_hooks = fs::read_to_string(&settings_path)
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .map(|settings| installed_hooks(&settings))
        .unwrap_or_default();
    // Without `agent`, Claude Code would run a hook that doesn't exist; one
    // already in the settings stays
    let guard_skipped =
        !init.guard && !update && !present_hooks.iter().any(|key| key == "PreToolUse");
    let guard_note = || -> Result<()> {
        outln!(
            "{} Skipped the PreToolUse hook `agent guard`: the agent CLI is not installed.",
            "•".yellow()
        )?;
        outln!(
            "  Install it from https://github.com/gitkb/agent, then run `meta init claude` again to add the hook."
        )?;
        Ok(())
    };

    if preview {
        outln!()?;
        outln!(
            "[DRY RUN] Would install {installed} file(s) and link {} skill folder(s); nothing was written.",
            skills.placed
        )?;
        if guard_skipped {
            guard_note()?;
        }
        return Ok(());
    }

    // Print summary
    outln!()?;
    if installed > 0 {
        outln!("{} Installed {} file(s)", "✓".green(), installed)?;
    }
    if skills.placed > 0 {
        outln!(
            "{} Linked {} skill folder(s) into {}",
            "✓".green(),
            skills.placed,
            skills.agent_dirs.join(", ")
        )?;
    }
    if skills.copied > 0 {
        outln!(
            "{} Copied {} skill folder(s) because symlinks could not be created \
             (on Windows, enable Developer Mode and rerun with --update to get links)",
            "•".yellow(),
            skills.copied
        )?;
    }
    // Only what --force or --update would refresh; every other skip has
    // its own line above
    if skipped > 0 {
        outln!(
            "{} Skipped {} existing file(s) (use --force or --update to refresh skills and rules)",
            "•".yellow(),
            skipped
        )?;
    }

    if installed > 0 {
        outln!()?;
        outln!("Claude Code is now configured for this meta repository:")?;
        outln!(
            "  Skills:  .meta/skills/ ({} skills), linked into {}",
            SKILLS.len(),
            skills.agent_dirs.join(", ")
        )?;
        outln!("  Rules:   .claude/rules/ ({} rule files)", RULES.len())?;
        if !present_hooks.is_empty() {
            outln!(
                "  Hooks:   .claude/settings.json ({})",
                present_hooks.join(", ")
            )?;
        }
    }
    if guard_skipped {
        guard_note()?;
    }

    outln!()?;
    outln!("For global Claude plugin access, run:")?;
    outln!("  claude plugin marketplace add gitkb/claude-plugins")?;
    outln!("  claude plugin install meta@gitkb")?;

    Ok(())
}

/// What [`install_skills`] did.
#[derive(Default)]
struct SkillsReport {
    /// `SKILL.md` files written into the tool's folder
    written: usize,
    /// Existing entries of the tool's own left alone, which `overwrite`
    /// would refresh
    skipped: usize,
    /// Skill folders linked (or copied) into agent folders
    placed: usize,
    /// Of `placed`, how many are copies because a symlink could not be made
    copied: usize,
    /// Agent folders that got links
    agent_dirs: Vec<String>,
}

/// State of a skill entry in an agent skills folder.
enum SkillEntry {
    Missing,
    /// The tool's own link
    Linked,
    /// Something the tool wrote that is not its link: a marked copy, or the plain
    /// file git checks the link out as with `core.symlinks=false`
    Owned,
    /// Anything else belongs to the user or another tool
    Foreign,
}

/// Install `skills` (name, `SKILL.md` content) as `.<tool>/skills/<name>/SKILL.md` under
/// `root` and link each into the agent skills folders, as atc and git-kb do:
/// `.claude/skills/<name> -> ../../.<tool>/skills/<name>`.
///
/// Without `overwrite`, existing files and folders are kept. With it, the tool's files
/// are rewritten and its own entries in agent folders are replaced by links.
/// Foreign entries (a file, folder or link meta didn't make) are left alone.
/// The folders on the way are followed wherever links put them.
/// Where a symlink can't be made (e.g. Windows without Developer Mode), the skill folder is
/// copied instead, with a `.<tool>-skill-copy` marker so later runs know it is theirs.
fn install_skills(
    root: &Path,
    tool: &str,
    skills: &[(&str, &str)],
    agent_dirs: &[AgentDir],
    overwrite: bool,
    init: &Init,
) -> Result<SkillsReport> {
    install_skills_with(
        root,
        tool,
        skills,
        agent_dirs,
        overwrite,
        init,
        |target, link| symlink_dir(target, link),
    )
}

/// [`install_skills`], making links with `make_link(target, link)`.
fn install_skills_with(
    root: &Path,
    tool: &str,
    skills: &[(&str, &str)],
    agent_dirs: &[AgentDir],
    overwrite: bool,
    init: &Init,
    make_link: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<SkillsReport> {
    let verbose = init.verbose;
    let mut report = SkillsReport::default();
    let tool_dir = PathBuf::from(format!(".{tool}")).join("skills");
    let marker = format!(".{tool}-skill-copy");
    let real_root = fs::canonicalize(root)
        .with_context(|| format!("Failed to resolve {}", escape_path(root)))?;

    for (name, content) in skills {
        let rel = tool_dir.join(name).join("SKILL.md");
        let path = root.join(&rel);
        if path.exists() && !overwrite {
            outln!(
                "{} {} (already exists)",
                "Skipped".yellow(),
                escape_path(&path)
            )?;
            report.skipped += 1;
            continue;
        }
        init.create_dir_all(&root.join(tool_dir.join(name)))?;
        init.write_file(&path, content)?;
        report.written += 1;
    }

    for agent in agent_dirs {
        let agent_dir = Path::new(agent.path);
        let agent_exists = agent_dir.parent().is_some_and(|p| root.join(p).is_dir());
        if !agent.always && !agent_exists {
            continue;
        }
        let skills_dir = root.join(agent_dir);
        init.create_dir_all(&skills_dir)?;
        report.agent_dirs.push(agent_dir.display().to_string());

        // Relative links are portable; a relocated folder needs the real source.
        let up: PathBuf = agent_dir.components().map(|_| "..").collect();
        let target_dir = if skills_dir.is_dir()
            && fs::canonicalize(&skills_dir)
                .with_context(|| format!("Failed to resolve {}", escape_path(&skills_dir)))?
                != real_root.join(agent_dir)
        {
            real_root.join(&tool_dir)
        } else {
            up.join(&tool_dir)
        };
        for (name, _) in skills {
            let link = skills_dir.join(name);
            let target = target_dir.join(name);
            match skill_entry(&link, &target, &marker) {
                SkillEntry::Missing => {}
                SkillEntry::Linked => {
                    if verbose {
                        outln!("Skipped {} (already linked)", escape_path(&link))?;
                    }
                    continue;
                }
                SkillEntry::Owned if overwrite => init.remove_owned(&link, &marker)?,
                SkillEntry::Owned => {
                    outln!("Skipped {} (already exists)", escape_path(&link))?;
                    report.skipped += 1;
                    continue;
                }
                SkillEntry::Foreign => {
                    outln!(
                        "{} {} (not created by {tool}, left alone)",
                        "Skipped".yellow(),
                        escape_path(&link)
                    )?;
                    continue;
                }
            }
            let source = root.join(tool_dir.join(name)).join("SKILL.md");
            if init.link_skill(&target, &link, &source, &marker, &make_link)? {
                report.copied += 1;
            }
            report.placed += 1;
        }
    }

    Ok(report)
}

/// Classify the entry at `link` without following it.
fn skill_entry(link: &Path, target: &Path, marker: &str) -> SkillEntry {
    let Ok(meta) = fs::symlink_metadata(link) else {
        return SkillEntry::Missing;
    };
    if meta.file_type().is_symlink() {
        return match fs::read_link(link) {
            Ok(found) if found == target => SkillEntry::Linked,
            _ => SkillEntry::Foreign,
        };
    }
    if meta.is_file() {
        // git writes a link as a file holding its target when it can't make links
        let placeholder = target.to_string_lossy().replace('\\', "/");
        return match fs::read_to_string(link) {
            Ok(text) if text == placeholder => SkillEntry::Owned,
            _ => SkillEntry::Foreign,
        };
    }
    let Ok(entries) = fs::read_dir(link) else {
        return SkillEntry::Foreign;
    };
    let mut names = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            return SkillEntry::Foreign;
        };
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            return SkillEntry::Foreign;
        }
        names.push(entry.file_name());
    }
    names.sort();
    if names == [marker, "SKILL.md"] {
        SkillEntry::Owned
    } else {
        SkillEntry::Foreign
    }
}

/// Remove an [`SkillEntry::Owned`] entry: only the files the tool wrote, then the folder.
fn remove_owned(link: &Path, marker: &str) -> Result<()> {
    if fs::symlink_metadata(link).is_ok_and(|m| m.is_file()) {
        return fs::remove_file(link)
            .with_context(|| format!("Failed to remove {}", escape_path(link)));
    }
    for file in ["SKILL.md", marker] {
        let path = link.join(file);
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove {}", escape_path(&path)))?;
        }
    }
    fs::remove_dir(link).with_context(|| format!("Failed to remove {}", escape_path(link)))
}

/// Copy the skill folder of `source` (a `SKILL.md`) to `dir` where it can't be linked,
/// marking the copy as the tool's own.
fn copy_skill(source: &Path, dir: &Path, marker: &str) -> Result<()> {
    fs::create_dir(dir).with_context(|| format!("Failed to create {}", escape_path(dir)))?;
    fs::copy(source, dir.join("SKILL.md"))
        .with_context(|| format!("Failed to copy {}", escape_path(source)))?;
    fs::write(
        dir.join(marker),
        "Copied by `meta init claude` because a symlink could not be created here.\n\
         `meta init claude --update` refreshes it.\n",
    )
    .with_context(|| format!("Failed to write {}", escape_path(dir)))?;
    Ok(())
}

struct SettingsPlan {
    content: Option<String>,
    message: String,
}

fn prepare_settings(path: &Path, init: &Init) -> Result<SettingsPlan> {
    let existing = match fs::read_to_string(path) {
        Ok(content) => Some(
            serde_json::from_str::<Value>(&content)
                .with_context(|| format!("Invalid JSON in {}", escape_path(path)))?,
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("Failed to read {}", escape_path(path))),
    };
    let created = existing.is_none();
    let existing = existing.unwrap_or_else(|| json!({}));
    let before = installed_hooks(&existing);
    let merged = merge_hooks_into_settings(existing, build_meta_hooks(init.guard), path)?;
    let added: Vec<_> = installed_hooks(&merged)
        .into_iter()
        .filter(|key| !before.contains(key))
        .collect();
    let preview = init.mode.is_preview();
    let message = if created {
        if preview { "Would create" } else { "Created" }.to_string()
    } else if added.is_empty() {
        "Already has meta's hooks (no write):".to_string()
    } else {
        let verb = if preview { "Would add" } else { "Added" };
        format!("{verb} {} hooks to", added.join(", "))
    };
    let content = if created || !added.is_empty() {
        Some(serde_json::to_string_pretty(&merged)?)
    } else {
        None
    };
    Ok(SettingsPlan { content, message })
}

fn write_settings_atomic(path: &Path, content: &str) -> Result<()> {
    use std::io::Write;
    // A new file has nothing to lose, and gets the umask's mode
    if !path.exists() {
        return fs::write(path, content)
            .with_context(|| format!("Failed to write {}", escape_path(path)));
    }
    // Rename beside the final target so stowed settings keep every link intact.
    let resolved = fs::canonicalize(path)
        .with_context(|| format!("Failed to resolve {}", escape_path(path)))?;
    let path = resolved.as_path();
    let parent = path.parent().context("Settings path has no parent")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "Failed to create temporary settings in {}",
            escape_path(parent)
        )
    })?;
    temp.write_all(content.as_bytes())?;
    // The temporary file is private; keep the mode settings.json had
    temp.as_file()
        .set_permissions(fs::metadata(path)?.permissions())?;
    temp.persist(path)
        .with_context(|| format!("Failed to write {}", escape_path(path)))?;
    Ok(())
}

fn installed_hooks(settings: &Value) -> Vec<String> {
    build_meta_hooks(true)
        .into_iter()
        .filter_map(|(key, groups)| {
            let existing = settings.get("hooks")?.get(&key)?.as_array()?;
            groups
                .as_array()?
                .iter()
                .all(|group| existing.iter().any(|found| runs_group(found, group)))
                .then_some(key)
        })
        .collect()
}

/// Whether the hook group `found` already runs every hook of `group` for the
/// same tool calls: the same matcher, and each of the group's hook types,
/// commands and prompts among its own. Other fields, such as a timeout the
/// user changed, and hooks the user added to it don't make it another group.
fn runs_group(found: &Value, group: &Value) -> bool {
    type Hook<'a> = (Option<&'a Value>, Option<&'a Value>, Option<&'a Value>);
    fn hooks(group: &Value) -> Option<Vec<Hook<'_>>> {
        let hooks = group.get("hooks")?.as_array()?;
        Some(
            hooks
                .iter()
                .map(|h| (h.get("type"), h.get("command"), h.get("prompt")))
                .collect(),
        )
    }
    let (Some(have), Some(want)) = (hooks(found), hooks(group)) else {
        return false;
    };
    found.get("matcher") == group.get("matcher") && want.iter().all(|hook| have.contains(hook))
}

/// Build the meta hooks configuration as a JSON object; the PreToolUse
/// `agent guard` hook only with `guard`
fn build_meta_hooks(guard: bool) -> Map<String, Value> {
    let mut hooks = Map::new();

    // SessionStart: inject workspace context at session start and after compaction
    hooks.insert(
        "SessionStart".to_string(),
        json!([{
            "hooks": [{
                "type": "command",
                "command": "meta context 2>/dev/null",
                "timeout": 10
            }]
        }]),
    );

    // PreToolUse: block destructive Bash commands
    if guard {
        hooks.insert(
            "PreToolUse".to_string(),
            json!([{
                "matcher": "Bash",
                "hooks": [{
                    "type": "command",
                    "command": "agent guard",
                    "timeout": 5
                }]
            }]),
        );
    }

    // PreCompact: capture workspace state before context compaction
    hooks.insert(
        "PreCompact".to_string(),
        json!([{
            "hooks": [{
                "type": "command",
                "command": "meta context 2>/dev/null",
                "timeout": 10
            }]
        }]),
    );

    hooks
}

/// Merge meta hooks into existing settings, preserving other settings
fn merge_hooks_into_settings(
    mut existing: Value,
    meta_hooks: Map<String, Value>,
    path: &Path,
) -> Result<Value> {
    let invalid = |key: &str, shape: &str| {
        anyhow::anyhow!(
            "Cannot merge {}: key '{}' must be {}",
            escape_path(path),
            key,
            shape
        )
    };
    let obj = existing.as_object_mut().ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot merge {}: it must hold a JSON object",
            escape_path(path)
        )
    })?;
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| invalid("hooks", "an object"))?;
    // Every event meta adds to has the shape Claude Code reads, whether or
    // not this run adds to it
    for key in build_meta_hooks(true).keys() {
        if hooks.get(key).is_some_and(|groups| !groups.is_array()) {
            return Err(invalid(&format!("hooks.{key}"), "an array"));
        }
    }
    for (key, groups) in meta_hooks {
        let arr = hooks
            .entry(&key)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| invalid(&format!("hooks.{key}"), "an array"))?;
        if let Some(groups) = groups.as_array() {
            for group in groups {
                if !arr.iter().any(|found| runs_group(found, group)) {
                    arr.push(group.clone());
                }
            }
        }
    }
    Ok(existing)
}

fn agent_cli_available() -> bool {
    use std::process::{Command, Stdio};

    #[expect(
        clippy::disallowed_methods,
        reason = "a probe for the agent CLI, in no repo"
    )]
    Command::new("agent")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn write_file(path: &Path, content: &str, verbose: bool) -> Result<()> {
    fs::write(path, content).with_context(|| format!("Failed to write {}", escape_path(path)))?;

    if verbose {
        outln!("{} {}", "Wrote".green(), escape_path(path))?;
    } else {
        // Show the parent too: every skill file is named SKILL.md.
        let name = |p: &Path| p.file_name().unwrap_or_default().to_owned();
        let shown = Path::new(&path.parent().map(name).unwrap_or_default()).join(name(path));
        outln!("  {} {}", "✓".green(), escape_path(&shown))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// A real run with `agent` installed
    const RUN: Init = Init {
        mode: ExecutionMode::Execute,
        verbose: false,
        guard: true,
    };

    #[test]
    fn invalid_settings_leave_every_file_unchanged_even_with_force() {
        fn snapshot(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
            let mut entries = Vec::new();
            for entry in fs::read_dir(root).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    entries.push((path.clone(), None));
                    entries.extend(snapshot(&path));
                } else {
                    entries.push((path.clone(), Some(fs::read(&path).unwrap())));
                }
            }
            entries.sort();
            entries
        }
        for (content, key) in [
            (r#"[{"user":"data"}]"#, "must hold a JSON object"),
            (r#"{"hooks":[],"theme":"dark"}"#, "key 'hooks'"),
            (r#"{"hooks":{"PreToolUse":{}}}"#, "key 'hooks.PreToolUse'"),
            ("{invalid json", "Invalid JSON"),
        ] {
            let dir = tempdir().unwrap();
            fs::create_dir_all(dir.path().join(".claude/rules")).unwrap();
            fs::write(
                dir.path()
                    .join(".claude/rules/meta-workspace-discipline.md"),
                "keep this rule",
            )
            .unwrap();
            let settings = dir.path().join(".claude").join("settings.json");
            fs::write(&settings, content).unwrap();
            let before = snapshot(dir.path());
            for force in [false, true] {
                let err =
                    install_claude_integration_to(dir.path(), force, false, &RUN).unwrap_err();
                let message = format!("{err:#}");
                assert!(message.contains(&escape_path(&settings)), "{message}");
                assert!(message.contains(key), "{message}");
                assert_eq!(snapshot(dir.path()), before);
            }
        }
    }

    #[test]
    fn a_changed_hook_timeout_is_still_metas_hook() {
        let dir = tempdir().unwrap();
        let settings = dir.path().join("settings.json");
        let mut hooks = build_meta_hooks(true);
        hooks["SessionStart"][0]["hooks"][0]["timeout"] = json!(30);
        fs::write(&settings, json!({ "hooks": hooks }).to_string()).unwrap();
        let plan = prepare_settings(&settings, &RUN).unwrap();
        assert!(plan.content.is_none(), "{}", plan.message);
    }

    #[cfg(unix)]
    #[test]
    fn merged_settings_keep_their_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let settings = dir.path().join("settings.json");
        fs::write(&settings, r#"{"theme":"dark"}"#).unwrap();
        fs::set_permissions(&settings, fs::Permissions::from_mode(0o644)).unwrap();
        let plan = prepare_settings(&settings, &RUN).unwrap();
        write_settings_atomic(&settings, &plan.content.unwrap()).unwrap();
        let mode = fs::metadata(&settings).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o644);
    }

    #[test]
    fn settings_plan_compares_matchers_and_reports_no_write() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut settings = json!({"hooks": build_meta_hooks(true), "theme": "dark"});
        settings["hooks"]["PreToolUse"][0]["matcher"] = json!("Read");
        fs::write(&path, serde_json::to_string(&settings).unwrap()).unwrap();
        let plan = prepare_settings(&path, &RUN).unwrap();
        assert_eq!(plan.message, "Added PreToolUse hooks to");
        let content = plan.content.unwrap();
        let merged: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(merged["theme"], "dark");
        assert_eq!(merged["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(merged["hooks"]["PreToolUse"][1]["matcher"], "Bash");
        write_settings_atomic(&path, &content).unwrap();
        let plan = prepare_settings(&path, &RUN).unwrap();
        assert_eq!(plan.message, "Already has meta's hooks (no write):");
        assert!(plan.content.is_none());
        assert_eq!(fs::read_to_string(path).unwrap(), content);
    }

    #[test]
    fn test_install_creates_skills_rules_and_settings() {
        let dir = tempdir().unwrap();

        // Create a .meta.yaml file so it looks like a meta repo
        fs::write(dir.path().join(".meta.yaml"), "projects: {}\n").unwrap();

        install_claude_integration_to(dir.path(), false, false, &RUN).unwrap();

        let claude_dir = dir.path().join(".claude");
        let rules_dir = claude_dir.join("rules");

        // Skills are real folders in .meta/skills/, linked into the agent folders
        for (name, _) in SKILLS {
            let skill =
                fs::read_to_string(dir.path().join(".meta/skills").join(name).join("SKILL.md"))
                    .unwrap();
            let mut lines = skill.lines();
            assert_eq!(lines.next(), Some("---"));
            assert_eq!(lines.next(), Some(format!("name: {name}").as_str()));
            assert!(lines.next().is_some_and(|l| l.starts_with("description: ")));
            assert_linked(dir.path(), ".claude/skills", name);
            assert_linked(dir.path(), ".agents/skills", name);
        }
        // Agents whose folder doesn't exist get no links
        assert!(!dir.path().join(".cursor").exists());

        // Rules should exist
        assert!(rules_dir.exists());
        assert!(rules_dir.join("meta-workspace-discipline.md").exists());
        assert!(rules_dir.join("meta-destructive-commands.md").exists());

        // Settings.json should exist with hook configuration
        let settings_path = claude_dir.join("settings.json");
        assert!(settings_path.exists());
        let settings_content = fs::read_to_string(&settings_path).unwrap();
        assert!(settings_content.contains("hooks"));
        assert!(settings_content.contains("SessionStart"));
        assert!(settings_content.contains("PreToolUse"));
        assert!(settings_content.contains("PreCompact"));
        assert!(settings_content.contains("meta context"));
        assert!(settings_content.contains("agent guard"));
    }

    #[test]
    fn test_install_skips_existing_files() {
        let dir = tempdir().unwrap();

        let claude_dir = dir.path().join(".claude");
        let skills_dir = claude_dir.join("skills");
        let rules_dir = claude_dir.join("rules");
        fs::create_dir_all(skills_dir.join("meta-workspace")).unwrap();
        fs::create_dir_all(&rules_dir).unwrap();

        // Create existing skill file
        let existing_skill = skills_dir.join("meta-workspace").join("SKILL.md");
        fs::write(&existing_skill, "custom content").unwrap();

        // Create existing rule file
        let existing_rule = rules_dir.join("meta-workspace-discipline.md");
        fs::write(&existing_rule, "custom rule").unwrap();

        // Create existing settings.json
        let existing_settings = claude_dir.join("settings.json");
        fs::write(&existing_settings, r#"{"custom": true}"#).unwrap();

        install_claude_integration_to(dir.path(), false, false, &RUN).unwrap();

        // Should not overwrite skill or rule
        let skill_content = fs::read_to_string(&existing_skill).unwrap();
        assert_eq!(skill_content, "custom content");

        let rule_content = fs::read_to_string(&existing_rule).unwrap();
        assert_eq!(rule_content, "custom rule");

        // Settings should be MERGED (not overwritten, not skipped)
        let settings_content = fs::read_to_string(&existing_settings).unwrap();
        let settings: Value = serde_json::from_str(&settings_content).unwrap();
        assert!(
            settings.get("custom").is_some(),
            "should preserve custom key"
        );
        assert!(settings.get("hooks").is_some(), "should add hooks");
    }

    #[test]
    fn test_install_force_overwrites() {
        let dir = tempdir().unwrap();

        let claude_dir = dir.path().join(".claude");
        let rules_dir = claude_dir.join("rules");
        let meta_skill_dir = dir.path().join(".meta/skills/meta-workspace");
        fs::create_dir_all(&meta_skill_dir).unwrap();
        fs::create_dir_all(&rules_dir).unwrap();

        // Create existing files with different content
        let existing_skill = meta_skill_dir.join("SKILL.md");
        fs::write(&existing_skill, "custom content").unwrap();

        let existing_rule = rules_dir.join("meta-workspace-discipline.md");
        fs::write(&existing_rule, "custom rule").unwrap();

        let existing_settings = claude_dir.join("settings.json");
        fs::write(&existing_settings, r#"{"custom": true}"#).unwrap();

        install_claude_integration_to(dir.path(), true, false, &RUN).unwrap();

        // Should overwrite with embedded content
        let skill_content = fs::read_to_string(&existing_skill).unwrap();
        assert!(skill_content.contains("Meta Workspace Skill"));

        let rule_content = fs::read_to_string(&existing_rule).unwrap();
        assert!(rule_content.contains("Meta Workspace Discipline"));

        // --force still merges settings.
        let settings_content = fs::read_to_string(&existing_settings).unwrap();
        let settings: Value = serde_json::from_str(&settings_content).unwrap();
        assert!(
            settings.get("custom").is_some(),
            "should preserve custom key with --force"
        );
        assert!(settings.get("hooks").is_some(), "should have hooks");
    }

    #[test]
    fn test_update_flag_refreshes_content_skips_settings() {
        let dir = tempdir().unwrap();

        let claude_dir = dir.path().join(".claude");
        let rules_dir = claude_dir.join("rules");
        let meta_skill_dir = dir.path().join(".meta/skills/meta-workspace");
        fs::create_dir_all(&meta_skill_dir).unwrap();
        fs::create_dir_all(&rules_dir).unwrap();

        // Create existing files
        let existing_skill = meta_skill_dir.join("SKILL.md");
        fs::write(&existing_skill, "old skill content").unwrap();

        let existing_rule = rules_dir.join("meta-workspace-discipline.md");
        fs::write(&existing_rule, "old rule content").unwrap();

        let existing_settings = claude_dir.join("settings.json");
        fs::write(
            &existing_settings,
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "prompt", "prompt": "custom"}]}]}}"#,
        )
        .unwrap();

        // Use --update flag
        install_claude_integration_to(dir.path(), false, true, &RUN).unwrap();

        // Skills and rules should be updated
        let skill_content = fs::read_to_string(&existing_skill).unwrap();
        assert!(
            skill_content.contains("Meta Workspace Skill"),
            "skill should be updated"
        );

        let rule_content = fs::read_to_string(&existing_rule).unwrap();
        assert!(
            rule_content.contains("Meta Workspace Discipline"),
            "rule should be updated"
        );

        // Settings should be UNTOUCHED
        let settings_content = fs::read_to_string(&existing_settings).unwrap();
        assert!(
            settings_content.contains("custom"),
            "settings should be preserved"
        );
        assert!(
            !settings_content.contains("SessionStart"),
            "meta hooks should NOT be added"
        );
    }

    /// Assert `<agent_dir>/<name>` is meta's link, or its marked copy where links can't be made.
    fn assert_linked(root: &Path, agent_dir: &str, name: &str) {
        let link = root.join(agent_dir).join(name);
        let target = PathBuf::from("../..").join(".meta/skills").join(name);
        let meta = fs::symlink_metadata(&link).unwrap();
        if meta.file_type().is_symlink() || !cfg!(windows) {
            assert_eq!(
                fs::read_link(&link).ok(),
                Some(target),
                "{}",
                link.display()
            );
        } else {
            assert!(link.join(".meta-skill-copy").is_file());
        }
        assert_eq!(
            fs::read_to_string(link.join("SKILL.md")).unwrap(),
            fs::read_to_string(root.join(".meta/skills").join(name).join("SKILL.md")).unwrap()
        );
    }

    #[test]
    fn test_install_links_into_existing_agent_folders() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".cursor")).unwrap();

        install_claude_integration_to(dir.path(), false, false, &RUN).unwrap();

        for (name, _) in SKILLS {
            assert_linked(dir.path(), ".cursor/skills", name);
        }
        assert!(!dir.path().join(".opencode").exists());
    }

    #[test]
    fn test_update_replaces_marked_copies_with_links() {
        let dir = tempdir().unwrap();
        let copy = dir.path().join(".claude/skills/meta-git");
        fs::create_dir_all(&copy).unwrap();
        fs::write(copy.join("SKILL.md"), "old copy").unwrap();
        fs::write(copy.join(".meta-skill-copy"), "").unwrap();

        install_claude_integration_to(dir.path(), false, false, &RUN).unwrap();
        assert_eq!(
            fs::read_to_string(copy.join("SKILL.md")).unwrap(),
            "old copy"
        );

        install_claude_integration_to(dir.path(), false, true, &RUN).unwrap();
        assert_linked(dir.path(), ".claude/skills", "meta-git");
    }

    #[test]
    fn test_update_leaves_foreign_entries_alone() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join(".claude/skills");
        // A folder with more than meta wrote, and a file
        fs::create_dir_all(skills_dir.join("meta-git")).unwrap();
        fs::write(skills_dir.join("meta-git/SKILL.md"), "mine").unwrap();
        fs::write(skills_dir.join("meta-git/notes.md"), "mine").unwrap();
        fs::write(skills_dir.join("meta-exec"), "mine").unwrap();

        for (force, update) in [(false, true), (true, false)] {
            install_claude_integration_to(dir.path(), force, update, &RUN).unwrap();
            assert_eq!(
                fs::read_to_string(skills_dir.join("meta-git/SKILL.md")).unwrap(),
                "mine"
            );
            assert_eq!(
                fs::read_to_string(skills_dir.join("meta-git/notes.md")).unwrap(),
                "mine"
            );
            assert_eq!(
                fs::read_to_string(skills_dir.join("meta-exec")).unwrap(),
                "mine"
            );
            assert_linked(dir.path(), ".claude/skills", "meta-safety");
        }
    }

    #[test]
    fn test_update_keeps_unmarked_skill_folders() {
        // A folder holding only a `SKILL.md`, without meta's marker, is not meta's,
        // even when it holds the text meta ships
        let shipped = SKILLS
            .iter()
            .find(|(name, _)| *name == "meta-git")
            .unwrap()
            .1;
        for text in ["another tool", shipped] {
            let dir = tempdir().unwrap();
            let other = dir.path().join(".claude/skills/meta-git");
            fs::create_dir_all(&other).unwrap();
            fs::write(other.join("SKILL.md"), text).unwrap();

            for (force, update) in [(false, true), (true, false)] {
                install_claude_integration_to(dir.path(), force, update, &RUN).unwrap();
                assert!(!fs::symlink_metadata(&other).unwrap().is_symlink());
                assert_eq!(fs::read_to_string(other.join("SKILL.md")).unwrap(), text);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_rules_and_files_follow_overwrite_policy() {
        for link_folder in [false, true] {
            let dir = tempdir().unwrap();
            let other = tempdir().unwrap();
            let target = other.path().join("meta-workspace-discipline.md");
            fs::write(&target, "custom rule").unwrap();
            fs::write(other.path().join("notes.md"), "keep me").unwrap();
            fs::create_dir(dir.path().join(".claude")).unwrap();
            let rules = dir.path().join(".claude/rules");
            let rule = dir
                .path()
                .join(".claude/rules/meta-workspace-discipline.md");
            let link = if link_folder {
                symlink_dir(other.path(), &rules).unwrap();
                &rules
            } else {
                fs::create_dir(&rules).unwrap();
                symlink_dir(&target, &rule).unwrap();
                &rule
            };
            install_claude_integration_to(dir.path(), false, false, &RUN).unwrap();
            assert_eq!(fs::read_to_string(&target).unwrap(), "custom rule");
            for (force, update) in [(true, false), (false, true)] {
                fs::write(&target, "custom rule").unwrap();
                install_claude_integration_to(
                    dir.path(),
                    force,
                    update,
                    &Init {
                        mode: ExecutionMode::Preview,
                        ..RUN
                    },
                )
                .unwrap();
                assert_eq!(fs::read_to_string(&target).unwrap(), "custom rule");
                install_claude_integration_to(dir.path(), force, update, &RUN).unwrap();
                assert_eq!(
                    fs::read_to_string(&target).unwrap(),
                    RULE_WORKSPACE_DISCIPLINE
                );
                assert!(fs::symlink_metadata(link).unwrap().is_symlink());
                assert_eq!(
                    fs::read_to_string(other.path().join("notes.md")).unwrap(),
                    "keep me"
                );
            }
            let other_rule = dir
                .path()
                .join(".claude/rules/meta-destructive-commands.md");
            assert_eq!(
                fs::read_to_string(other_rule).unwrap(),
                RULE_DESTRUCTIVE_COMMANDS
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_settings_merge_atomically() {
        use std::os::unix::fs::PermissionsExt;
        for link_folder in [false, true] {
            for force in [false, true] {
                let dir = tempdir().unwrap();
                let other = tempdir().unwrap();
                let target = other.path().join("settings.json");
                let original = r#"{"other":true,"hooks":{"Stop":[{"hooks":[{"type":"command","command":"custom"}]}]}}"#;
                fs::write(&target, original).unwrap();
                fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
                let old_inode = other.path().join("old-settings.json");
                fs::hard_link(&target, &old_inode).unwrap();
                let claude = dir.path().join(".claude");
                let settings = claude.join("settings.json");
                let link = if link_folder {
                    symlink_dir(other.path(), &claude).unwrap();
                    &claude
                } else {
                    fs::create_dir(&claude).unwrap();
                    symlink_dir("settings.json", other.path().join("stowed.json")).unwrap();
                    symlink_dir(other.path().join("stowed.json"), &settings).unwrap();
                    &settings
                };
                let destination = fs::read_link(link).unwrap();
                install_claude_integration_to(
                    dir.path(),
                    force,
                    false,
                    &Init {
                        mode: ExecutionMode::Preview,
                        ..RUN
                    },
                )
                .unwrap();
                assert_eq!(fs::read_to_string(&target).unwrap(), original);
                assert_eq!(fs::read_link(link).unwrap(), destination);
                install_claude_integration_to(dir.path(), force, true, &RUN).unwrap();
                assert_eq!(fs::read_to_string(&target).unwrap(), original);
                install_claude_integration_to(dir.path(), force, false, &RUN).unwrap();
                let merged: Value =
                    serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
                assert_eq!(installed_hooks(&merged).len(), 3);
                assert_eq!(merged["other"], true);
                assert_eq!(merged["hooks"]["Stop"][0]["hooks"][0]["command"], "custom");
                assert_eq!(fs::read_link(link).unwrap(), destination);
                assert_eq!(
                    fs::read_to_string(&old_inode).unwrap(),
                    original,
                    "replace the target atomically"
                );
                assert_eq!(
                    fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                    0o640
                );
                if !link_folder {
                    assert_eq!(
                        fs::read_link(other.path().join("stowed.json")).unwrap(),
                        Path::new("settings.json")
                    );
                }
                install_claude_integration_to(dir.path(), force, false, &RUN).unwrap();
                assert_eq!(
                    serde_json::from_str::<Value>(&fs::read_to_string(&target).unwrap()).unwrap(),
                    merged
                );
            }
        }
    }

    #[test]
    fn test_update_replaces_git_link_placeholders() {
        let dir = tempdir().unwrap();
        // What git checks a link out as with core.symlinks=false
        let placeholder = dir.path().join(".agents/skills/meta-git");
        fs::create_dir_all(placeholder.parent().unwrap()).unwrap();
        fs::write(&placeholder, "../../.meta/skills/meta-git").unwrap();
        let other = dir.path().join(".agents/skills/meta-exec");
        fs::write(&other, "../../.other/meta-exec").unwrap();

        install_claude_integration_to(dir.path(), false, true, &RUN).unwrap();

        assert_linked(dir.path(), ".agents/skills", "meta-git");
        assert_eq!(fs::read_to_string(other).unwrap(), "../../.other/meta-exec");
    }

    #[test]
    fn test_copies_installed_skill_when_links_fail() {
        let dir = tempdir().unwrap();
        let skill = dir.path().join(".meta/skills/meta-git/SKILL.md");
        fs::create_dir_all(skill.parent().unwrap()).unwrap();
        fs::write(&skill, "committed skill").unwrap();
        let fail = |_: &Path, _: &Path| Err(std::io::Error::other("no links"));

        let report = install_skills_with(
            dir.path(),
            "meta",
            SKILLS,
            AGENT_SKILL_DIRS,
            false,
            &RUN,
            fail,
        )
        .unwrap();

        assert_eq!(report.copied, 2 * SKILLS.len());
        let copy = dir.path().join(".claude/skills/meta-git");
        assert_eq!(
            fs::read_to_string(copy.join("SKILL.md")).unwrap(),
            "committed skill"
        );
        assert!(copy.join(".meta-skill-copy").is_file());

        // Later runs know the copies are meta's: kept by default, replaced with --update
        let report = install_skills_with(
            dir.path(),
            "meta",
            SKILLS,
            AGENT_SKILL_DIRS,
            false,
            &RUN,
            fail,
        )
        .unwrap();
        assert_eq!(report.copied, 0);
        install_claude_integration_to(dir.path(), false, true, &RUN).unwrap();
        assert_linked(dir.path(), ".claude/skills", "meta-git");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_agent_skill_dirs_create_working_links() {
        let dir = tempdir().unwrap();
        let other = tempdir().unwrap();
        for agent in AGENT_SKILL_DIRS {
            let link = dir.path().join(agent.path);
            let target = other.path().join(agent.path);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            fs::create_dir_all(&target).unwrap();
            symlink_dir(&target, &link).unwrap();
        }
        for update in [false, true] {
            install_claude_integration_to(dir.path(), false, update, &RUN).unwrap();
            for agent in AGENT_SKILL_DIRS {
                let folder = dir.path().join(agent.path);
                assert!(fs::symlink_metadata(&folder).unwrap().is_symlink());
                for (name, content) in SKILLS {
                    let link = folder.join(name);
                    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
                    assert_eq!(fs::read_to_string(link.join("SKILL.md")).unwrap(), *content);
                    assert_eq!(
                        fs::canonicalize(&link).unwrap(),
                        fs::canonicalize(dir.path().join(".meta/skills").join(name)).unwrap()
                    );
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_tool_skill_paths_follow_overwrite_policy() {
        for rel in [
            ".meta/skills",
            ".meta/skills/meta-git",
            ".meta/skills/meta-git/SKILL.md",
        ] {
            let dir = tempdir().unwrap();
            let other = tempdir().unwrap();
            let link = dir.path().join(rel);
            let suffix = Path::new(".meta/skills/meta-git/SKILL.md")
                .strip_prefix(rel)
                .unwrap();
            let target = other.path().join("target");
            let file = if suffix.as_os_str().is_empty() {
                target.clone()
            } else {
                target.join(suffix)
            };
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, "custom skill").unwrap();
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink_dir(&target, &link).unwrap();
            install_claude_integration_to(dir.path(), false, false, &RUN).unwrap();
            assert_eq!(fs::read_to_string(&file).unwrap(), "custom skill");
            install_claude_integration_to(dir.path(), false, true, &RUN).unwrap();
            assert!(fs::read_to_string(&file)
                .unwrap()
                .contains("Meta Git Skill"));
            assert_eq!(fs::read_link(&link).unwrap(), target);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_skill_link_meta_did_not_make_is_left_alone() {
        // another tool's skill linked in under meta's name: nothing of meta's
        // is behind it, so no mode writes there
        let dir = tempdir().unwrap();
        let other = tempdir().unwrap();
        for agent in AGENT_SKILL_DIRS {
            let skills = dir.path().join(agent.path);
            let target = other.path().join(agent.path);
            fs::create_dir_all(&skills).unwrap();
            fs::create_dir_all(&target).unwrap();
            fs::write(target.join("SKILL.md"), "custom skill").unwrap();
            symlink_dir(&target, skills.join("meta-git")).unwrap();
        }
        for (force, update) in [(false, false), (true, false), (false, true)] {
            install_claude_integration_to(dir.path(), force, update, &RUN).unwrap();
            for agent in AGENT_SKILL_DIRS {
                let target = other.path().join(agent.path);
                assert_eq!(
                    fs::read_to_string(target.join("SKILL.md")).unwrap(),
                    "custom skill"
                );
                assert_eq!(
                    fs::read_link(dir.path().join(agent.path).join("meta-git")).unwrap(),
                    target
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_invalid_settings_are_refused_before_writes() {
        let dir = tempdir().unwrap();
        let other = tempdir().unwrap();
        let target = other.path().join("settings.json");
        fs::write(&target, "{invalid json").unwrap();
        fs::create_dir(dir.path().join(".claude")).unwrap();
        let link = dir.path().join(".claude/settings.json");
        symlink_dir(&target, &link).unwrap();
        for force in [false, true] {
            let err = install_claude_integration_to(dir.path(), force, false, &RUN).unwrap_err();
            assert!(format!("{err:#}").contains("Invalid JSON"));
            assert_eq!(fs::read_to_string(&target).unwrap(), "{invalid json");
            assert_eq!(fs::read_link(&link).unwrap(), target);
            assert!(!dir.path().join(".meta").exists());
            assert!(!dir.path().join(".claude/rules").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn preview_and_dangling_skill_dirs_leave_links_intact() {
        let dir = tempdir().unwrap();
        let other = tempdir().unwrap();
        fs::create_dir(dir.path().join(".claude")).unwrap();
        let settings = other.path().join("settings.json");
        fs::write(&settings, "{}").unwrap();
        symlink_dir(&settings, dir.path().join(".claude/settings.json")).unwrap();
        for rel in [
            ".claude/rules",
            ".claude/skills",
            ".agents/skills",
            ".meta/skills/meta-git",
        ] {
            let link = dir.path().join(rel);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink_dir(other.path().join(rel), &link).unwrap();
        }
        let preview = Init {
            mode: ExecutionMode::Preview,
            ..RUN
        };
        install_claude_integration_to(dir.path(), true, false, &preview).unwrap();
        assert_eq!(fs::read_to_string(&settings).unwrap(), "{}");
        assert_eq!(fs::read_dir(other.path()).unwrap().count(), 1);
        assert_eq!(
            fs::read_dir(dir.path().join(".meta/skills"))
                .unwrap()
                .count(),
            1
        );
        // a folder that leads nowhere is an error, as for `mkdir -p`, named;
        // the links stay as they are
        let err = install_claude_integration_to(dir.path(), true, false, &RUN).unwrap_err();
        assert!(format!("{err:#}").contains("Failed to create"), "{err:#}");
        for rel in [".claude/rules", ".claude/skills", ".agents/skills"] {
            assert_eq!(
                fs::read_link(dir.path().join(rel)).unwrap(),
                other.path().join(rel)
            );
        }
    }

    #[test]
    fn test_merge_preserves_existing_hooks() {
        let existing = json!({
            "hooks": {
                "Stop": [{
                    "hooks": [{
                        "type": "prompt",
                        "prompt": "custom stop hook",
                        "timeout": 30
                    }]
                }]
            },
            "permissions": {
                "allow": ["Bash(git:*)"]
            }
        });

        let meta_hooks = build_meta_hooks(true);
        let merged =
            merge_hooks_into_settings(existing, meta_hooks, Path::new("settings.json")).unwrap();

        // Should preserve existing Stop hook
        assert!(merged["hooks"]["Stop"].is_array());
        let stop_hooks = merged["hooks"]["Stop"].as_array().unwrap();
        assert!(stop_hooks
            .iter()
            .any(|h| h["hooks"][0]["prompt"] == "custom stop hook"));

        // Should add meta hooks
        assert!(merged["hooks"]["SessionStart"].is_array());
        assert!(merged["hooks"]["PreToolUse"].is_array());
        assert!(merged["hooks"]["PreCompact"].is_array());

        // Should preserve other settings
        assert!(merged["permissions"]["allow"].is_array());
    }

    #[test]
    fn test_merge_avoids_duplicate_hooks() {
        // Create existing settings that already have meta hooks
        let existing = json!({
            "hooks": {
                "SessionStart": [{
                    "hooks": [{
                        "type": "command",
                        "command": "meta context 2>/dev/null",
                        "timeout": 10
                    }]
                }]
            }
        });

        let meta_hooks = build_meta_hooks(true);
        let merged =
            merge_hooks_into_settings(existing, meta_hooks, Path::new("settings.json")).unwrap();

        // Should NOT duplicate SessionStart hook
        let session_hooks = merged["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(session_hooks.len(), 1, "should not duplicate existing hook");

        // Should still add other hooks
        assert!(merged["hooks"]["PreToolUse"].is_array());
        assert!(merged["hooks"]["PreCompact"].is_array());
    }

    #[test]
    fn test_settings_does_not_affect_local_settings() {
        let dir = tempdir().unwrap();

        let claude_dir = dir.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        // Create a settings.local.json (personal config)
        let local_settings = claude_dir.join("settings.local.json");
        fs::write(
            &local_settings,
            r#"{"permissions": {"allow": ["Bash(git:*)"]}}"#,
        )
        .unwrap();

        fs::write(dir.path().join(".meta.yaml"), "projects: {}\n").unwrap();
        install_claude_integration_to(dir.path(), false, false, &RUN).unwrap();

        // settings.local.json should be untouched
        let local_content = fs::read_to_string(&local_settings).unwrap();
        assert!(local_content.contains("permissions"));
        assert!(!local_content.contains("hooks"));
    }

    #[test]
    fn test_build_meta_hooks_structure() {
        let hooks = build_meta_hooks(true);

        // Should have exactly 3 lifecycle hooks
        assert_eq!(hooks.len(), 3);
        assert!(hooks.contains_key("SessionStart"));
        assert!(hooks.contains_key("PreToolUse"));
        assert!(hooks.contains_key("PreCompact"));

        // SessionStart should call meta context
        let session = &hooks["SessionStart"];
        assert!(session[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("meta context"));

        // PreToolUse should have Bash matcher and call agent guard
        let pre_tool = &hooks["PreToolUse"];
        assert_eq!(pre_tool[0]["matcher"], "Bash");
        assert!(pre_tool[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("agent guard"));

        // PreCompact should call meta context
        let pre_compact = &hooks["PreCompact"];
        assert!(pre_compact[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("meta context"));
    }
}
