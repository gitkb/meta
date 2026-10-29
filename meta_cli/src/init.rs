//! Initialize Claude Code integration for meta repositories.
//!
//! This module provides the `meta init claude` command which installs
//! Claude Code skill files, rules, and hook configuration into the current
//! project's `.claude/` directory.

use crate::config;
use anyhow::{Context, Result};
use colored::*;
use meta_core::display::escape_path;
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
    /// Older releases wrote real skill folders here, so a folder holding only a
    /// `SKILL.md` identical to the shipped one is the tool's own
    legacy: bool,
}

const AGENT_SKILL_DIRS: &[AgentDir] = &[
    AgentDir {
        path: ".claude/skills",
        always: true,
        legacy: true,
    },
    // Codex and other agents
    AgentDir {
        path: ".agents/skills",
        always: true,
        legacy: false,
    },
    AgentDir {
        path: ".cursor/skills",
        always: false,
        legacy: false,
    },
    AgentDir {
        path: ".opencode/skills",
        always: false,
        legacy: false,
    },
    AgentDir {
        path: ".pi/skills",
        always: false,
        legacy: false,
    },
    AgentDir {
        path: ".omp/skills",
        always: false,
        legacy: false,
    },
    AgentDir {
        path: ".grok/skills",
        always: false,
        legacy: false,
    },
];

/// First lines of every flat `.claude/skills/<name>.md` file older releases wrote
const FLAT_SKILL_HEADINGS: &[&str] = &[
    "# Meta Workspace Skill",
    "# Meta Git Skill",
    "# Meta Git Operations Skill",
    "# Meta Exec Skill",
    "# Meta Plugins Skill",
    "# Meta Worktree Skill",
    "# Meta Safety Skill",
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

/// Typed init subcommand, mirroring the clap-parsed structure from main.
pub enum InitCommand {
    /// No subcommand — show help
    None,
    /// Install Claude Code skills, rules, and hooks
    Claude {
        /// Refresh skills and rules, merge meta hooks into settings
        force: bool,
        /// Update skills and rules only, skip settings (preserves user customizations)
        update: bool,
    },
}

/// Handle the `meta init` subcommand with typed args.
pub fn handle_init_command(command: InitCommand, verbose: bool) -> Result<()> {
    match command {
        InitCommand::None => {
            print_init_help();
            Ok(())
        }
        InitCommand::Claude { force, update } => install_claude_integration(force, update, verbose),
    }
}

fn print_init_help() {
    println!("meta init - Initialize meta integrations");
    println!();
    println!("USAGE:");
    println!("    meta init <command>");
    println!();
    println!("COMMANDS:");
    println!("    claude    Install Claude Code skills, rules, and hooks for this meta repo");
    println!();
    println!("OPTIONS:");
    println!("    -f, --force     Refresh skills and rules, merge meta hooks into settings");
    println!("    -u, --update    Update skills and rules only, preserve settings");
    println!();
    println!("EXAMPLES:");
    println!("    meta init claude             Install Claude integration");
    println!("    meta init claude --update    Update skills/rules, keep settings");
    println!("    meta init claude --force     Refresh skills/rules and merge settings");
}

/// Install Claude Code skills and hook configuration
fn install_claude_integration(force: bool, update: bool, verbose: bool) -> Result<()> {
    let current_dir = std::env::current_dir()?;
    install_claude_integration_to(&current_dir, force, update, verbose)
}

/// Install Claude Code skills and hook configuration into a specific directory
fn install_claude_integration_to(
    target_dir: &Path,
    force: bool,
    update: bool,
    verbose: bool,
) -> Result<()> {
    let claude_dir = target_dir.join(".claude");
    let skills_dir = claude_dir.join("skills");
    let rules_dir = claude_dir.join("rules");

    // A config that can't be read is an error, not "no config"
    if config::find_meta_config_in(target_dir)?.is_none() {
        println!(
            "{}",
            "Warning: No .meta.yaml found in current directory.".yellow()
        );
        println!("This integration is designed for meta repositories.");
        println!();
    }

    // Build and validate settings before writing any skills or rules.
    let settings_path = claude_dir.join("settings.json");
    let settings_link = symlink_in(target_dir, Path::new(".claude/settings.json"));
    let settings_plan = if update || settings_link.is_some() {
        None
    } else {
        Some(prepare_settings(&settings_path)?)
    };

    // --force or --update: overwrite; default: skip existing
    let overwrite_content = force || update;
    let skills = install_skills(
        target_dir,
        "meta",
        SKILLS,
        AGENT_SKILL_DIRS,
        overwrite_content,
        verbose,
    )?;
    let mut installed = skills.written;
    let mut skipped = skills.skipped;

    // Like skills, rules are never written through a symlink
    let rules_link = symlink_in(target_dir, Path::new(".claude/rules"));
    if rules_link.is_none() {
        fs::create_dir_all(&rules_dir)
            .with_context(|| format!("Failed to create {}", escape_path(&rules_dir)))?;
    }

    // Older releases wrote flat `<name>.md` files, which Claude Code does not load.
    // Like other existing files, they are only removed with --update or --force,
    // and only when they start with the heading of a skill meta shipped.
    let skills_dir_linked = symlink_in(target_dir, Path::new(".claude/skills")).is_some();
    for (name, _) in SKILLS {
        if skills_dir_linked {
            break;
        }
        let flat_path = skills_dir.join(format!("{name}.md"));
        let is_file = fs::symlink_metadata(&flat_path).is_ok_and(|m| m.is_file());
        let shipped = is_file
            && fs::read_to_string(&flat_path).is_ok_and(|text| {
                text.lines()
                    .next()
                    .is_some_and(|l| FLAT_SKILL_HEADINGS.contains(&l))
            });
        if overwrite_content && shipped {
            fs::remove_file(&flat_path)
                .with_context(|| format!("Failed to remove {}", escape_path(&flat_path)))?;
            if verbose {
                println!("{} {}", "Removed".yellow(), escape_path(&flat_path));
            }
        } else if fs::symlink_metadata(&flat_path).is_ok() {
            println!(
                "{} {} ({})",
                "Skipped".yellow(),
                escape_path(&flat_path),
                if shipped {
                    "an older meta's file; --force or --update removes it"
                } else {
                    "not created by meta, left alone"
                }
            );
            skipped += 1;
        }
    }

    // Install rule files
    // --force or --update: overwrite; default: skip existing
    for (filename, content) in RULES {
        let target_path = rules_dir.join(filename);
        let rel = Path::new(".claude/rules").join(filename);
        if let Some(link) = rules_link.clone().or_else(|| symlink_in(target_dir, &rel)) {
            println!(
                "{} {} (reached through the symlink {}, not followed)",
                "Skipped".yellow(),
                escape_path(&rel),
                escape_path(&link)
            );
            skipped += 1;
            continue;
        }

        if target_path.exists() && !overwrite_content {
            if verbose {
                println!("{} {} (already exists)", "Skipped".yellow(), filename);
            }
            skipped += 1;
            continue;
        }

        write_file(&target_path, content, verbose)?;
        installed += 1;
    }

    // --update leaves settings untouched; --force uses the same merge as default.
    if update {
        println!("Skipped .claude/settings.json (--update preserves settings)");
        skipped += 1;
    } else if let Some(link) = settings_link {
        println!(
            "Skipped .claude/settings.json (reached through the symlink {}, not followed)",
            escape_path(&link)
        );
        skipped += 1;
    } else if let Some(plan) = settings_plan {
        if let Some(content) = plan.content {
            write_settings_atomic(&settings_path, &content)?;
            installed += 1;
        }
        println!("{} {}", plan.message, escape_path(&settings_path));
    }
    let present_hooks = fs::read_to_string(&settings_path)
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .map(|settings| installed_hooks(&settings))
        .unwrap_or_default();

    // Print summary
    println!();
    if installed > 0 {
        println!("{} Installed {} file(s)", "✓".green(), installed);
    }
    if skills.placed > 0 {
        println!(
            "{} Linked {} skill folder(s) into {}",
            "✓".green(),
            skills.placed,
            skills.agent_dirs.join(", ")
        );
    }
    if skills.copied > 0 {
        println!(
            "{} Copied {} skill folder(s) because symlinks could not be created \
             (on Windows, enable Developer Mode and rerun with --update to get links)",
            "•".yellow(),
            skills.copied
        );
    }
    if skipped > 0 {
        let hint = if update {
            "(--update preserves settings)"
        } else {
            "(use --force or --update to refresh skills and rules)"
        };
        println!(
            "{} Skipped {} existing file(s) {}",
            "•".yellow(),
            skipped,
            hint
        );
    }

    if installed > 0 {
        println!();
        println!("Claude Code is now configured for this meta repository:");
        println!(
            "  Skills:  .meta/skills/ ({} skills), linked into {}",
            SKILLS.len(),
            skills.agent_dirs.join(", ")
        );
        println!("  Rules:   .claude/rules/ ({} rule files)", RULES.len());
        if !present_hooks.is_empty() {
            println!(
                "  Hooks:   .claude/settings.json ({})",
                present_hooks.join(", ")
            );
        }
        println!("  Note:    PreToolUse hooks require the standalone agent CLI (`agent guard`).");
    }

    #[cfg(not(test))]
    if (installed > 0) && !agent_cli_available() {
        println!(
            "{}",
            "Warning: 'agent' CLI not found. PreToolUse hooks require the standalone agent tool."
                .yellow()
        );
        println!("Install from: https://github.com/gitkb/agent");
    }

    println!();
    println!("For global Claude plugin access, run:");
    println!("  claude plugin marketplace add gitkb/claude-plugins");
    println!("  claude plugin install meta@gitkb");

    Ok(())
}

/// What [`install_skills`] did.
#[derive(Default)]
struct SkillsReport {
    /// `SKILL.md` files written into the tool's folder
    written: usize,
    /// Existing entries left alone
    skipped: usize,
    /// Skill folders linked (or copied) into agent folders
    placed: usize,
    /// Of `placed`, how many are copies because a symlink could not be made
    copied: usize,
    /// Agent folders that got links
    agent_dirs: Vec<String>,
}

/// State of a skill entry in an agent skills folder, read without following links.
enum SkillEntry {
    Missing,
    /// The tool's own link
    Linked,
    /// Something the tool wrote that is not its link: a marked copy, a legacy
    /// folder (only the shipped `SKILL.md`, in a `legacy` agent folder), or the plain file git
    /// checks the link out as with `core.symlinks=false`
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
/// Foreign entries are never changed, and no symlink is followed or removed: a
/// symlink inside `.<tool>/skills/` is an error, and an agent folder reached through
/// a symlink is skipped.
/// Where a symlink can't be made (e.g. Windows without Developer Mode), the skill folder is
/// copied instead, with a `.<tool>-skill-copy` marker so later runs know it is theirs.
fn install_skills(
    root: &Path,
    tool: &str,
    skills: &[(&str, &str)],
    agent_dirs: &[AgentDir],
    overwrite: bool,
    verbose: bool,
) -> Result<SkillsReport> {
    install_skills_with(
        root,
        tool,
        skills,
        agent_dirs,
        overwrite,
        verbose,
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
    verbose: bool,
    make_link: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<SkillsReport> {
    let mut report = SkillsReport::default();
    let tool_dir = PathBuf::from(format!(".{tool}")).join("skills");
    let marker = format!(".{tool}-skill-copy");

    for (name, content) in skills {
        let rel = tool_dir.join(name).join("SKILL.md");
        if let Some(link) = symlink_in(root, &rel) {
            anyhow::bail!(
                "Refusing to write through the symlink {}",
                escape_path(&link)
            );
        }
        let path = root.join(&rel);
        if path.exists() && !overwrite {
            println!(
                "{} {} (already exists)",
                "Skipped".yellow(),
                escape_path(&path)
            );
            report.skipped += 1;
            continue;
        }
        let skill_dir = root.join(tool_dir.join(name));
        fs::create_dir_all(&skill_dir)
            .with_context(|| format!("Failed to create {}", escape_path(&skill_dir)))?;
        write_file(&path, content, verbose)?;
        report.written += 1;
    }

    for agent in agent_dirs {
        let agent_dir = Path::new(agent.path);
        let agent_exists = agent_dir.parent().is_some_and(|p| root.join(p).is_dir());
        if !agent.always && !agent_exists {
            continue;
        }
        if let Some(link) = symlink_in(root, agent_dir) {
            println!(
                "{} {} (reached through the symlink {}, not followed)",
                "Skipped".yellow(),
                escape_path(agent_dir),
                escape_path(&link)
            );
            report.skipped += 1;
            continue;
        }
        let skills_dir = root.join(agent_dir);
        fs::create_dir_all(&skills_dir)
            .with_context(|| format!("Failed to create {}", escape_path(&skills_dir)))?;
        report.agent_dirs.push(agent_dir.display().to_string());

        // One `..` per folder in `agent_dir`
        let up: PathBuf = agent_dir.components().map(|_| "..").collect();
        for (name, content) in skills {
            let link = skills_dir.join(name);
            let target = up.join(&tool_dir).join(name);
            let legacy = agent.legacy.then_some(*content);
            match skill_entry(&link, &target, &marker, legacy) {
                SkillEntry::Missing => {}
                SkillEntry::Linked => {
                    if verbose {
                        println!("Skipped {} (already linked)", escape_path(&link));
                    }
                    continue;
                }
                SkillEntry::Owned if overwrite => remove_owned(&link, &marker)?,
                SkillEntry::Owned => {
                    println!("Skipped {} (already exists)", escape_path(&link));
                    report.skipped += 1;
                    continue;
                }
                SkillEntry::Foreign => {
                    println!(
                        "{} {} (not created by {tool}, left alone)",
                        "Skipped".yellow(),
                        escape_path(&link)
                    );
                    report.skipped += 1;
                    continue;
                }
            }
            match make_link(&target, &link) {
                Ok(()) => {
                    if verbose {
                        println!(
                            "{} {} -> {}",
                            "Linked".green(),
                            escape_path(&link),
                            escape_path(&target)
                        );
                    }
                }
                // Windows needs Developer Mode or admin rights for symlinks; some
                // filesystems have none. Copy instead.
                Err(err) => {
                    let source = root.join(tool_dir.join(name)).join("SKILL.md");
                    copy_skill(&source, &link, &marker)?;
                    println!(
                        "{} {} (symlink failed: {err})",
                        "Copied".yellow(),
                        escape_path(&link)
                    );
                    report.copied += 1;
                }
            }
            report.placed += 1;
        }
    }

    Ok(report)
}

/// The first symlink (or Windows junction) on the way from `root` to `root/rel`, if any.
fn symlink_in(root: &Path, rel: &Path) -> Option<PathBuf> {
    let mut path = root.to_path_buf();
    for part in rel.components() {
        path.push(part);
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Some(path);
        }
    }
    None
}

/// Classify the entry at `link` without following it. `legacy` is the shipped
/// `SKILL.md` when older releases wrote real folders into this agent folder.
fn skill_entry(link: &Path, target: &Path, marker: &str, legacy: Option<&str>) -> SkillEntry {
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
    let shipped = |text: &str| fs::read_to_string(link.join("SKILL.md")).is_ok_and(|t| t == text);
    if names == [marker, "SKILL.md"] || (names == ["SKILL.md"] && legacy.is_some_and(shipped)) {
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

fn prepare_settings(path: &Path) -> Result<SettingsPlan> {
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
    let merged = merge_hooks_into_settings(existing, build_meta_hooks(), path)?;
    let added: Vec<_> = installed_hooks(&merged)
        .into_iter()
        .filter(|key| !before.contains(key))
        .collect();
    let message = if created {
        "Created".to_string()
    } else if added.is_empty() {
        "Already has meta's hooks (no write):".to_string()
    } else {
        format!("Added {} hooks to", added.join(", "))
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
    build_meta_hooks()
        .into_iter()
        .filter_map(|(key, groups)| {
            let existing = settings.get("hooks")?.get(&key)?.as_array()?;
            groups
                .as_array()?
                .iter()
                .all(|group| existing.iter().any(|found| same_group(found, group)))
                .then_some(key)
        })
        .collect()
}

/// Whether two hook groups run the same hooks for the same tool calls: the
/// same matcher and the same hook types, commands and prompts. Other fields,
/// such as a timeout the user changed, don't make a group another one.
fn same_group(a: &Value, b: &Value) -> bool {
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
    a.get("matcher") == b.get("matcher") && hooks(a) == hooks(b)
}

/// Build the meta hooks configuration as a JSON object
fn build_meta_hooks() -> Map<String, Value> {
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
    for (key, groups) in meta_hooks {
        let arr = hooks
            .entry(&key)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| invalid(&format!("hooks.{key}"), "an array"))?;
        if let Some(groups) = groups.as_array() {
            for group in groups {
                if !arr.iter().any(|found| same_group(found, group)) {
                    arr.push(group.clone());
                }
            }
        }
    }
    Ok(existing)
}

#[cfg(not(test))]
fn agent_cli_available() -> bool {
    use std::process::{Command, Stdio};

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
        println!("{} {}", "Wrote".green(), escape_path(path));
    } else {
        // Show the parent too: every skill file is named SKILL.md.
        let name = |p: &Path| p.file_name().unwrap_or_default().to_owned();
        let shown = Path::new(&path.parent().map(name).unwrap_or_default()).join(name(path));
        println!("  {} {}", "✓".green(), escape_path(&shown));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
                    install_claude_integration_to(dir.path(), force, false, false).unwrap_err();
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
        let mut hooks = build_meta_hooks();
        hooks["SessionStart"][0]["hooks"][0]["timeout"] = json!(30);
        fs::write(&settings, json!({ "hooks": hooks }).to_string()).unwrap();
        let plan = prepare_settings(&settings).unwrap();
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
        let plan = prepare_settings(&settings).unwrap();
        write_settings_atomic(&settings, &plan.content.unwrap()).unwrap();
        let mode = fs::metadata(&settings).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o644);
    }

    #[test]
    fn settings_plan_compares_matchers_and_reports_no_write() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut settings = json!({"hooks": build_meta_hooks(), "theme": "dark"});
        settings["hooks"]["PreToolUse"][0]["matcher"] = json!("Read");
        fs::write(&path, serde_json::to_string(&settings).unwrap()).unwrap();
        let plan = prepare_settings(&path).unwrap();
        assert_eq!(plan.message, "Added PreToolUse hooks to");
        let content = plan.content.unwrap();
        let merged: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(merged["theme"], "dark");
        assert_eq!(merged["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(merged["hooks"]["PreToolUse"][1]["matcher"], "Bash");
        write_settings_atomic(&path, &content).unwrap();
        let plan = prepare_settings(&path).unwrap();
        assert_eq!(plan.message, "Already has meta's hooks (no write):");
        assert!(plan.content.is_none());
        assert_eq!(fs::read_to_string(path).unwrap(), content);
    }

    #[test]
    fn test_install_creates_skills_rules_and_settings() {
        let dir = tempdir().unwrap();

        // Create a .meta.yaml file so it looks like a meta repo
        fs::write(dir.path().join(".meta.yaml"), "projects: {}\n").unwrap();

        install_claude_integration_to(dir.path(), false, false, false).unwrap();

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

        install_claude_integration_to(dir.path(), false, false, false).unwrap();

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

        install_claude_integration_to(dir.path(), true, false, false).unwrap();

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
        install_claude_integration_to(dir.path(), false, true, false).unwrap();

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

    #[test]
    fn test_update_replaces_flat_skill_files() {
        let dir = tempdir().unwrap();

        let skills_dir = dir.path().join(".claude").join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        let headings = [
            "# Meta Workspace Skill",
            "# Meta Git Operations Skill",
            "# Meta Exec Skill",
            "# Meta Plugins Skill",
            "# Meta Worktree Skill",
            "# Meta Safety Skill",
        ];
        for ((name, _), heading) in SKILLS.iter().zip(headings) {
            let old = format!("{heading}\nold flat skill");
            fs::write(skills_dir.join(format!("{name}.md")), old).unwrap();
        }
        let unrelated = skills_dir.join("my-skill.md");
        fs::write(&unrelated, "not ours").unwrap();

        install_claude_integration_to(dir.path(), false, true, false).unwrap();

        for (name, _) in SKILLS {
            assert!(!skills_dir.join(format!("{name}.md")).exists());
            assert!(skills_dir.join(name).join("SKILL.md").exists());
        }
        assert_eq!(fs::read_to_string(&unrelated).unwrap(), "not ours");
    }

    #[test]
    fn test_default_install_keeps_flat_skill_files() {
        let dir = tempdir().unwrap();

        let skills_dir = dir.path().join(".claude").join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        let flat = skills_dir.join("meta-git.md");
        fs::write(&flat, "custom flat skill").unwrap();

        install_claude_integration_to(dir.path(), false, false, false).unwrap();

        assert_eq!(fs::read_to_string(&flat).unwrap(), "custom flat skill");
        assert!(skills_dir.join("meta-git").join("SKILL.md").exists());
    }

    /// Assert `<agent_dir>/<name>` is meta's link, or its marked copy where links can't be made.
    fn assert_linked(root: &Path, agent_dir: &str, name: &str) {
        let link = root.join(agent_dir).join(name);
        let target = PathBuf::from("../..").join(".meta/skills").join(name);
        let meta = fs::symlink_metadata(&link).unwrap();
        if meta.file_type().is_symlink() {
            assert_eq!(fs::read_link(&link).unwrap(), target);
        } else if cfg!(windows) {
            assert!(link.join(".meta-skill-copy").is_file());
        } else {
            panic!("{} is not a link", link.display());
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

        install_claude_integration_to(dir.path(), false, false, false).unwrap();

        for (name, _) in SKILLS {
            assert_linked(dir.path(), ".cursor/skills", name);
        }
        assert!(!dir.path().join(".opencode").exists());
    }

    #[test]
    fn test_update_turns_skill_folders_into_links() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join(".claude/skills");
        // The layout meta wrote before links: real folders in .claude/skills/
        for (name, content) in SKILLS {
            fs::create_dir_all(skills_dir.join(name)).unwrap();
            fs::write(skills_dir.join(name).join("SKILL.md"), content).unwrap();
        }

        // Without --update they are kept
        install_claude_integration_to(dir.path(), false, false, false).unwrap();
        for (name, _) in SKILLS {
            assert!(!fs::symlink_metadata(skills_dir.join(name))
                .unwrap()
                .is_symlink());
            assert_linked(dir.path(), ".agents/skills", name);
        }

        install_claude_integration_to(dir.path(), false, true, false).unwrap();
        for (name, _) in SKILLS {
            assert_linked(dir.path(), ".claude/skills", name);
        }
    }

    #[test]
    fn test_update_replaces_marked_copies_with_links() {
        let dir = tempdir().unwrap();
        let copy = dir.path().join(".claude/skills/meta-git");
        fs::create_dir_all(&copy).unwrap();
        fs::write(copy.join("SKILL.md"), "old copy").unwrap();
        fs::write(copy.join(".meta-skill-copy"), "").unwrap();

        install_claude_integration_to(dir.path(), false, false, false).unwrap();
        assert_eq!(
            fs::read_to_string(copy.join("SKILL.md")).unwrap(),
            "old copy"
        );

        install_claude_integration_to(dir.path(), false, true, false).unwrap();
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
            install_claude_integration_to(dir.path(), force, update, false).unwrap();
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
    fn test_update_keeps_flat_files_meta_did_not_ship() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join(".claude/skills");
        fs::create_dir_all(&skills_dir).unwrap();
        let other = skills_dir.join("meta-git.md");
        // Another tool's file, and what git checks another tool's link out as
        fs::write(&other, "# Another tool\n").unwrap();
        fs::write(skills_dir.join("meta-exec.md"), "../../.other/meta-exec.md").unwrap();

        for (force, update) in [(false, true), (true, false)] {
            install_claude_integration_to(dir.path(), force, update, false).unwrap();
            assert_eq!(fs::read_to_string(&other).unwrap(), "# Another tool\n");
            assert!(skills_dir.join("meta-exec.md").is_file());
        }
    }

    #[test]
    fn test_update_keeps_skill_folders_meta_did_not_ship() {
        let dir = tempdir().unwrap();
        // Same name and layout as an old meta folder, but not meta's text
        let other = dir.path().join(".claude/skills/meta-git");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("SKILL.md"), "another tool").unwrap();

        for (force, update) in [(false, true), (true, false)] {
            install_claude_integration_to(dir.path(), force, update, false).unwrap();
            assert_eq!(
                fs::read_to_string(other.join("SKILL.md")).unwrap(),
                "another tool"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_never_writes_rules_through_symlinks() {
        let dir = tempdir().unwrap();
        let other = dir.path().join(".other/SKILL.md");
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        fs::write(&other, "other tool").unwrap();
        fs::create_dir_all(dir.path().join(".claude/rules")).unwrap();
        let rule = dir
            .path()
            .join(".claude/rules/meta-workspace-discipline.md");
        symlink_dir("../../.other/SKILL.md", &rule).unwrap();

        install_claude_integration_to(dir.path(), false, true, false).unwrap();

        assert_eq!(fs::read_to_string(&other).unwrap(), "other tool");
        assert!(fs::symlink_metadata(&rule).unwrap().is_symlink());
        let other_rule = dir
            .path()
            .join(".claude/rules/meta-destructive-commands.md");
        assert!(fs::read_to_string(other_rule).unwrap().contains("# "));
    }

    #[cfg(unix)]
    #[test]
    fn test_never_writes_settings_through_symlinks() {
        let dir = tempdir().unwrap();
        let other = dir.path().join(".other/settings.json");
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        fs::write(&other, r#"{"other": true}"#).unwrap();

        // A linked settings file
        fs::create_dir_all(dir.path().join(".claude")).unwrap();
        symlink_dir(
            "../.other/settings.json",
            dir.path().join(".claude/settings.json"),
        )
        .unwrap();
        for force in [false, true] {
            install_claude_integration_to(dir.path(), force, false, false).unwrap();
            assert_eq!(fs::read_to_string(&other).unwrap(), r#"{"other": true}"#);
        }

        // A linked .claude folder: nothing is written through it
        let linked = tempdir().unwrap();
        symlink_dir(dir.path().join(".other"), linked.path().join(".claude")).unwrap();
        install_claude_integration_to(linked.path(), true, false, false).unwrap();
        assert_eq!(fs::read_to_string(&other).unwrap(), r#"{"other": true}"#);
        assert_eq!(fs::read_dir(dir.path().join(".other")).unwrap().count(), 1);
    }

    #[test]
    fn test_update_keeps_single_file_skill_folders_outside_claude() {
        let dir = tempdir().unwrap();
        // Older releases never wrote real folders into .agents/skills/
        let other = dir.path().join(".agents/skills/meta-git");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("SKILL.md"), "another tool").unwrap();

        install_claude_integration_to(dir.path(), true, false, false).unwrap();

        assert_eq!(
            fs::read_to_string(other.join("SKILL.md")).unwrap(),
            "another tool"
        );
        assert_linked(dir.path(), ".claude/skills", "meta-git");
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

        install_claude_integration_to(dir.path(), false, true, false).unwrap();

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
            false,
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
            false,
            fail,
        )
        .unwrap();
        assert_eq!(report.copied, 0);
        install_claude_integration_to(dir.path(), false, true, false).unwrap();
        assert_linked(dir.path(), ".claude/skills", "meta-git");
    }

    #[cfg(unix)]
    #[test]
    fn test_never_writes_through_symlinked_folders() {
        let dir = tempdir().unwrap();
        let other = dir.path().join(".other/skills");
        fs::create_dir_all(other.join("meta-git")).unwrap();
        fs::write(other.join("meta-git/SKILL.md"), "other tool").unwrap();

        // An agent folder that is a link is skipped
        fs::create_dir(dir.path().join(".agents")).unwrap();
        symlink_dir("../.other/skills", dir.path().join(".agents/skills")).unwrap();
        install_claude_integration_to(dir.path(), true, false, false).unwrap();
        assert_eq!(
            fs::read_to_string(other.join("meta-git/SKILL.md")).unwrap(),
            "other tool"
        );
        assert_eq!(fs::read_dir(&other).unwrap().count(), 1);
        assert_linked(dir.path(), ".claude/skills", "meta-git");

        // A link inside .meta/skills/ is an error
        let skill_dir = dir.path().join(".meta/skills/meta-git");
        fs::remove_dir_all(&skill_dir).unwrap();
        symlink_dir("../../.other/skills/meta-git", &skill_dir).unwrap();
        let err = install_claude_integration_to(dir.path(), false, true, false).unwrap_err();
        assert!(err.to_string().contains("symlink"), "{err}");
        assert_eq!(
            fs::read_to_string(other.join("meta-git/SKILL.md")).unwrap(),
            "other tool"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_update_never_follows_or_removes_foreign_links() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join(".claude/skills");
        fs::create_dir_all(&skills_dir).unwrap();
        // Another tool's skill folder, with only SKILL.md, behind a link
        let other = dir.path().join(".other/skills/meta-git");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("SKILL.md"), "other tool").unwrap();
        symlink_dir("../../.other/skills/meta-git", skills_dir.join("meta-git")).unwrap();
        // Flat-file name, but a link
        symlink_dir(
            "../../.other/skills/meta-git/SKILL.md",
            skills_dir.join("meta-exec.md"),
        )
        .unwrap();

        install_claude_integration_to(dir.path(), true, false, false).unwrap();

        assert_eq!(
            fs::read_link(skills_dir.join("meta-git")).unwrap(),
            Path::new("../../.other/skills/meta-git")
        );
        assert_eq!(
            fs::read_to_string(other.join("SKILL.md")).unwrap(),
            "other tool"
        );
        assert!(fs::symlink_metadata(skills_dir.join("meta-exec.md")).is_ok());
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

        let meta_hooks = build_meta_hooks();
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

        let meta_hooks = build_meta_hooks();
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
        install_claude_integration_to(dir.path(), false, false, false).unwrap();

        // settings.local.json should be untouched
        let local_content = fs::read_to_string(&local_settings).unwrap();
        assert!(local_content.contains("permissions"));
        assert!(!local_content.contains("hooks"));
    }

    #[test]
    fn test_build_meta_hooks_structure() {
        let hooks = build_meta_hooks();

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
