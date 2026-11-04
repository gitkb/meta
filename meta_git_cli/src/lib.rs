//! meta-git library
//!
//! Provides git operations optimized for meta repositories.

mod clone;
mod clone_missing;
mod clone_worker;
mod commit;
mod git_env;
mod helpers;
mod ssh;
mod update;

mod commands;

use indexmap::IndexMap;
use log::debug;
use meta_plugin_protocol::{
    has_help_token, CommandResult, PlannedCommand, PluginHelp, PluginInfo, PluginRequestOptions,
};
use std::path::Path;

use helpers::get_project_directories_with_fallback;

/// meta's options that choose repos, which commands that find their repos
/// themselves never take
pub(crate) const FILTERS: [&str; 3] = ["--include", "--exclude", "--tag"];

/// What meta-git says about itself: the commands it routes, and its help.
pub fn plugin_info() -> PluginInfo {
    // The git commands meta adapts; every other one runs as typed
    let mut adapted = IndexMap::new();
    adapted.insert(
        "clone".to_string(),
        "Clone a meta repo and the repos its .meta.yaml lists (meta -r: nested ones too)"
            .to_string(),
    );
    adapted.insert(
        "clone-missing".to_string(),
        "Clone the projects in .meta.yaml that aren't cloned; change nothing else".to_string(),
    );
    adapted.insert(
        "clone-prune".to_string(),
        "Remove the repos no .meta.yaml lists that hold nothing unpushed".to_string(),
    );
    adapted.insert(
        "clone-sync".to_string(),
        "clone-missing, then clone-prune".to_string(),
    );
    adapted.insert(
        "commit".to_string(),
        "Commit changes with optional per-repo messages".to_string(),
    );
    adapted.insert(
        "update".to_string(),
        "Fast-forward cloned repos (git pull --ff-only) and clone missing ones".to_string(),
    );
    adapted.insert(
        "worktree".to_string(),
        "Create isolated worktree sets for multi-repo branches".to_string(),
    );

    PluginInfo {
        name: "git".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        commands: vec![
            "git clone".to_string(),
            "git clone-missing".to_string(),
            "git clone-prune".to_string(),
            "git clone-sync".to_string(),
            "git update".to_string(),
            "git commit".to_string(),
            "git worktree".to_string(),
            "git worktree create".to_string(),
            "git worktree add".to_string(),
            "git worktree remove".to_string(),
            "git worktree list".to_string(),
            "git worktree status".to_string(),
            "git worktree diff".to_string(),
            "git worktree exec".to_string(),
            "git worktree prune".to_string(),
        ],
        description: Some("Git operations for meta repositories".to_string()),
        help: Some(PluginHelp {
            usage: "Usage: meta git <COMMAND> [OPTIONS]".to_string(),
            commands: adapted,
            runs_as_typed: Some("git".to_string()),
            examples: vec![
                "meta git clone https://github.com/org/meta-repo.git".to_string(),
                "meta git commit --edit              # Per-repo messages".to_string(),
                "meta git worktree create feature-x --repo api --repo web".to_string(),
                "meta git status                     # git status in each repo".to_string(),
                "meta git log --oneline -3           # git log, as typed".to_string(),
            ],
            note: None,
        }),
    }
}

/// Execute a command for meta repositories.
///
/// The `projects` parameter is the list of project directories passed from meta_cli.
/// When meta_cli runs with `--recursive`, it discovers nested .meta.yaml files and passes
/// all project directories here. If `projects` is empty, we fall back to reading
/// the local .meta.yaml file via `get_project_directories()`.
pub fn execute_command(
    command: &str,
    args: &[String],
    projects: &[String],
    options: &PluginRequestOptions,
    cwd: &Path,
) -> CommandResult {
    debug!("[meta_git_cli] Plugin invoked with command: '{command}'");
    debug!(
        "[meta_git_cli] Options: parallel={}, mode={:?}, verbose={}, recursive={}, depth={:?}",
        options.parallel,
        options.mode(),
        options.verbose,
        options.recursive,
        options.depth
    );
    debug!("[meta_git_cli] Projects from meta_cli: {projects:?}");

    let help_requested = has_help_token(command, args);

    // Worktree commands (meta worktree * and meta git worktree *)
    // Checked before --help intercept so worktree subcommands can handle their own --help via clap
    if command.starts_with("worktree") || command.starts_with("git worktree") {
        return commands::worktree::execute_worktree_command(command, args, options, cwd);
    }

    // These act on repos they find themselves, not on the selected ones
    if matches!(command, "git update" | "git clone") && options.filtered() {
        return CommandResult::Error(format!(
            "meta {command} finds its repos itself (in the .meta.yaml files); it does not take --include/--exclude/--tag"
        ));
    }

    // Commands meta adapts parse their own arguments, help included
    let meta = &options.meta_options;
    let all_but_filters = meta.refusing(&FILTERS);
    match command {
        "git clone" => {
            return match meta_plugin_protocol::parse_args(command, args, &all_but_filters) {
                Ok(args) => finish(clone::execute_git_clone(args, options, cwd)),
                Err(result) => result,
            }
        }
        "git update" => {
            return match meta_plugin_protocol::parse_args(command, args, &all_but_filters) {
                Ok(args) => finish(update::execute_git_update(args, options, cwd)),
                Err(result) => result,
            };
        }
        "git clone-missing" => {
            return match meta_plugin_protocol::parse_args::<clone_missing::CloneMissingArgs>(
                command, args, meta,
            ) {
                Ok(args) => finish(clone_missing::execute(args.into(), projects, options, cwd)),
                Err(result) => result,
            };
        }
        "git clone-prune" => {
            return match meta_plugin_protocol::parse_args::<clone_missing::ClonePruneArgs>(
                command, args, meta,
            ) {
                Ok(args) => finish(clone_missing::execute(args.into(), projects, options, cwd)),
                Err(result) => result,
            };
        }
        "git clone-sync" => {
            return match meta_plugin_protocol::parse_args::<clone_missing::CloneSyncArgs>(
                command, args, meta,
            ) {
                Ok(args) => finish(clone_missing::execute(args.into(), projects, options, cwd)),
                Err(result) => result,
            };
        }
        "git commit" => {
            // meta sends a help request without its repos: whatever clap
            // makes of the arguments, it never commits
            let help = ["--help".to_string()];
            let args = if help_requested { &help[..] } else { args };
            return match meta_plugin_protocol::parse_args(command, args, meta) {
                Ok(args) => finish(commit::execute_git_commit(args, projects, options, cwd)),
                Err(result) => result,
            };
        }
        _ => {}
    }

    // meta answers the help of a command it runs as typed itself
    if help_requested {
        return CommandResult::ShowHelp(None);
    }

    // Every other git command runs as typed in each repo
    execute_raw_git_command(command, args, projects, options, cwd)
}

fn finish(result: anyhow::Result<CommandResult>) -> CommandResult {
    result.unwrap_or_else(Into::into)
}

/// Check if a git command accesses remote repositories
fn is_remote_command(argv: &[String]) -> bool {
    let remote_commands = ["push", "pull", "fetch", "clone", "ls-remote"];
    argv.get(1)
        .is_some_and(|subcommand| remote_commands.contains(&subcommand.as_str()))
}

/// Execute a raw git command across all repos (fallback for unrecognized subcommands)
fn execute_raw_git_command(
    command: &str,
    args: &[String],
    projects: &[String],
    options: &PluginRequestOptions,
    cwd: &Path,
) -> CommandResult {
    use meta_plugin_protocol::ExecutionPlan;

    // Get project directories
    let dirs = match get_project_directories_with_fallback(projects, cwd) {
        Ok(d) => d,
        Err(e) => return CommandResult::Error(format!("Failed to get project directories: {e:#}")),
    };
    // A listed project that isn't cloned yet is named, not counted as a
    // failure: cloning it is `meta git clone-missing`'s (with --strict, an
    // error)
    let (missing, dirs): (Vec<String>, Vec<String>) = dirs.into_iter().partition(|dir| {
        matches!(
            meta_core::gitdir::Checkout::of(Path::new(dir)),
            meta_core::gitdir::Checkout::Missing
        )
    });
    let mut warnings = Vec::new();
    for dir in &missing {
        let shown = Path::new(dir);
        let shown = meta_core::display::escape_path(shown.strip_prefix(cwd).unwrap_or(shown));
        let warning = format!("{shown} is not cloned; `meta git clone-missing` clones it");
        if options.strict {
            return CommandResult::Error(format!("{warning} (strict mode)"));
        }
        // meta shows them, or lists them in its --json report
        warnings.push(warning);
    }
    let plan = |commands, parallel, max_parallel, spawn_stagger_ms| {
        CommandResult::FullPlan(Box::new(ExecutionPlan {
            pre_commands: vec![],
            commands,
            post_commands: vec![],
            parallel: Some(parallel),
            max_parallel,
            spawn_stagger_ms,
            warnings: warnings.clone(),
        }))
    };

    // The git argv: the command words (e.g. "git" or "git add") and the
    // user's args exactly as given
    let argv: Vec<String> = command
        .split_whitespace()
        .map(str::to_string)
        .chain(args.iter().cloned())
        .collect();

    // Set git-specific environment variables (pager, colors, prompts)
    let git_env = Some(git_env::git_env());

    let commands: Vec<PlannedCommand> = dirs
        .into_iter()
        .map(|dir| PlannedCommand {
            dir,
            argv: argv.clone(),
            env: git_env.clone(),
        })
        .collect();

    // For remote commands running in parallel, establish SSH ControlMaster
    // connections and inject GIT_SSH_COMMAND into each planned command. A
    // preview connects nowhere.
    if options.parallel && !options.mode().is_preview() && is_remote_command(&argv) {
        let urls = meta_core::config::Workspace::find(cwd, None)
            .ok()
            .flatten()
            .map(|workspace| ssh::ssh_urls(&workspace.projects))
            .unwrap_or_default();

        // HTTPS-only workspaces don't need SSH multiplexing
        if urls.is_empty() {
            return plan(commands, true, None, None);
        }

        let (ssh_env, parallel_ok) = match ssh::establish_ssh_masters(&urls) {
            ssh::SshMasters::OurSockets(dir) => (Some(ssh::git_ssh_command(&dir)), true),
            ssh::SshMasters::UserManaged => (None, true),
            ssh::SshMasters::Failed => {
                log::debug!(
                    "SSH multiplexing could not be set up: running one repository at a time"
                );
                (None, false)
            }
        };

        // Inject GIT_SSH_COMMAND into every planned command's env
        let commands: Vec<PlannedCommand> = commands
            .into_iter()
            .map(|mut cmd| {
                if let Some(ref ssh) = ssh_env {
                    let env = cmd.env.get_or_insert_with(Default::default);
                    env.insert("GIT_SSH_COMMAND".to_string(), ssh.clone());
                }
                cmd
            })
            .collect();

        // SSH ControlMaster has a default MaxSessions limit of 10 (server-side).
        const SSH_MAX_SESSIONS: usize = 10;

        // Stagger spawns by 25ms to prevent SSH socket saturation.
        const SSH_SPAWN_STAGGER_MS: u64 = 25;

        if parallel_ok {
            plan(
                commands,
                true,
                Some(SSH_MAX_SESSIONS),
                Some(SSH_SPAWN_STAGGER_MS),
            )
        } else {
            // SSH setup failed — fall back to sequential
            plan(commands, false, None, None)
        }
    } else {
        plan(commands, options.parallel, None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meta_test_support::Sandbox;

    fn argv(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }
    use commit::parse_multi_commit_file;
    use helpers::get_project_directories;

    #[test]
    fn the_commands_meta_git_adapts_are_the_ones_it_routes_and_each_names_its_git_options() {
        let info = plugin_info();
        let adapted: std::collections::BTreeSet<&str> = info
            .help
            .as_ref()
            .unwrap()
            .commands
            .keys()
            .map(String::as_str)
            .collect();
        let routed = info
            .commands
            .iter()
            .filter_map(|command| command.split_whitespace().nth(1))
            .collect();
        assert_eq!(adapted, routed);
        let sandbox = Sandbox::new();
        for verb in adapted {
            let options = PluginRequestOptions::default();
            let result = execute_command(
                &format!("git {verb}"),
                &argv("--help"),
                &[],
                &options,
                &sandbox.work(),
            );
            let help = match result {
                CommandResult::Message(help) => help,
                _ => String::new(),
            };
            assert!(help.contains("\nGit options: "), "git {verb}: {help}");
        }
    }

    #[test]
    fn test_execute_git_status_no_meta_file() {
        let sandbox = Sandbox::new();
        let temp_dir = sandbox.work();
        let options = PluginRequestOptions::default();

        let result = execute_command("git status", &[], &[], &options, &temp_dir);

        // Should succeed (returns a Plan, not an Error)
        assert!(!matches!(result, CommandResult::Error(_)));
    }

    #[test]
    fn test_unknown_command_falls_through_to_raw_git() {
        // Unknown git subcommands should fall through to raw git execution
        let options = PluginRequestOptions::default();
        let result = execute_command("git unknown", &[], &[], &options, Path::new("."));
        // Should return a plan (to run `git unknown` in all repos), not ShowHelp
        assert!(matches!(result, CommandResult::FullPlan(_)));
    }

    #[test]
    fn test_parse_multi_commit_file() {
        let content = r#"# Meta Multi-Commit
# Each section represents one repository.

========== meta_cli ==========
# 3 file(s) staged: src/main.rs, src/lib.rs, Cargo.toml

feat: add new feature

========== meta_mcp ==========
# 2 file(s) staged: src/main.rs, Cargo.toml

fix: fix bug in MCP server

========== empty_repo ==========
# 1 file(s) staged: file.rs

# No message here - should be skipped

"#;

        let commits = parse_multi_commit_file(content);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].0, "meta_cli");
        assert_eq!(commits[0].1, "feat: add new feature");
        assert_eq!(commits[1].0, "meta_mcp");
        assert_eq!(commits[1].1, "fix: fix bug in MCP server");
    }

    #[test]
    fn test_parse_multi_commit_file_multiline_message() {
        let content = r#"========== repo1 ==========
# 1 file staged

feat: add feature

This is a longer description
that spans multiple lines.

- bullet point 1
- bullet point 2
"#;

        let commits = parse_multi_commit_file(content);
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].0, "repo1");
        assert!(commits[0].1.contains("feat: add feature"));
        assert!(commits[0].1.contains("bullet point 1"));
    }

    #[test]
    fn test_parse_multi_commit_file_empty() {
        let content = "";
        let commits = parse_multi_commit_file(content);
        assert!(commits.is_empty());
    }

    #[test]
    fn test_parse_multi_commit_file_only_comments() {
        let content = r#"# Meta Multi-Commit
# Each section represents one repository.
# This file has only comments
"#;
        let commits = parse_multi_commit_file(content);
        assert!(commits.is_empty());
    }

    #[test]
    fn test_parse_multi_commit_file_whitespace_only_message() {
        let content = r#"========== repo1 ==========
# 1 file staged



========== repo2 ==========
# 1 file staged

valid message
"#;
        let commits = parse_multi_commit_file(content);
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].0, "repo2");
        assert_eq!(commits[0].1, "valid message");
    }

    #[test]
    fn test_parse_multi_commit_file_special_characters_in_repo_name() {
        let content = r#"========== my-repo_v2.0 ==========
# 1 file staged

fix: handle special chars
"#;
        let commits = parse_multi_commit_file(content);
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].0, "my-repo_v2.0");
    }

    #[test]
    fn test_parse_multi_commit_file_preserves_message_whitespace() {
        let content = r#"========== repo ==========
# staged files

first line
  indented line
    more indent

last line
"#;
        let commits = parse_multi_commit_file(content);
        assert_eq!(commits.len(), 1);
        assert!(commits[0].1.contains("  indented line"));
        assert!(commits[0].1.contains("    more indent"));
    }

    #[test]
    fn test_parse_multi_commit_file_deleted_section() {
        // Simulates user deleting a section entirely
        let content = r#"========== repo1 ==========
# 1 file staged

first commit

========== repo3 ==========
# 1 file staged

third commit
"#;
        let commits = parse_multi_commit_file(content);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].0, "repo1");
        assert_eq!(commits[1].0, "repo3");
    }

    #[test]
    fn test_parse_multi_commit_file_single_repo() {
        let content = r#"========== only_repo ==========
# 5 files staged

the only commit message
"#;
        let commits = parse_multi_commit_file(content);
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].0, "only_repo");
        assert_eq!(commits[0].1, "the only commit message");
    }

    // ============ get_project_directories Tests ============

    #[test]
    fn test_get_project_directories_no_meta_file() {
        let sandbox = Sandbox::new();
        let temp_dir = sandbox.work();

        let dirs = get_project_directories(&temp_dir).unwrap();

        // Should return just "." when no .meta.yaml file
        assert_eq!(dirs, vec!["."]);
    }

    #[test]
    fn test_get_project_directories_refuses_a_bad_meta_file() {
        // A refused config is an error, not "just the root"
        let sandbox = Sandbox::new();
        let temp_dir = sandbox.work();
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"{"projects": {"evil": {"repo": "u", "path": ".git/x"}}}"#,
        )
        .unwrap();

        let err = get_project_directories(&temp_dir).unwrap_err();
        assert!(format!("{err:#}").contains("leads into .git"), "{err:#}");
    }

    #[test]
    fn test_get_project_directories_with_meta_file() {
        let sandbox = Sandbox::new();
        let temp_dir = sandbox.work();

        // Create .meta.yaml file
        let meta_content = r#"{"projects": {"alpha": "url1", "beta": "url2", "gamma": "url3"}}"#;
        std::fs::write(temp_dir.join(".meta.yaml"), meta_content).unwrap();

        let dirs = get_project_directories(&temp_dir).unwrap();

        // Should return "." plus sorted project names
        assert_eq!(dirs.len(), 4);
        assert_eq!(dirs[0], ".");
        assert_eq!(dirs[1], "alpha");
        assert_eq!(dirs[2], "beta");
        assert_eq!(dirs[3], "gamma");
    }

    #[test]
    fn test_get_project_directories_sorted() {
        let sandbox = Sandbox::new();
        let temp_dir = sandbox.work();

        // Create .meta.yaml file with unsorted projects
        let meta_content = r#"{"projects": {"zebra": "url1", "alpha": "url2", "middle": "url3"}}"#;
        std::fs::write(temp_dir.join(".meta.yaml"), meta_content).unwrap();

        let dirs = get_project_directories(&temp_dir).unwrap();

        // Projects should be sorted alphabetically
        assert_eq!(dirs[1], "alpha");
        assert_eq!(dirs[2], "middle");
        assert_eq!(dirs[3], "zebra");
    }

    #[test]
    fn test_get_project_directories_from_inside_a_project() {
        let sandbox = Sandbox::new();
        let temp_dir = sandbox.work();
        let root = &temp_dir;
        std::fs::write(
            root.join(".meta.yaml"),
            r#"{"projects": {"a": "u", "b": "u"}}"#,
        )
        .unwrap();
        std::fs::create_dir(root.join("a")).unwrap();

        // every repo the command runs in, not just the one cwd is in
        let dirs = get_project_directories(&root.join("a")).unwrap();
        let full = |path: &std::path::Path| path.to_string_lossy().into_owned();
        assert_eq!(dirs, [full(root), ".".to_string(), full(&root.join("b"))]);
    }

    #[test]
    fn test_get_project_directories_empty_projects() {
        let sandbox = Sandbox::new();
        let temp_dir = sandbox.work();

        // Create .meta.yaml file with no projects
        let meta_content = r#"{"projects": {}}"#;
        std::fs::write(temp_dir.join(".meta.yaml"), meta_content).unwrap();

        let dirs = get_project_directories(&temp_dir).unwrap();

        // Should return just "."
        assert_eq!(dirs, vec!["."]);
    }

    // ============ Remote Command Detection Tests ============

    #[test]
    fn test_is_remote_command_push() {
        assert!(super::is_remote_command(&argv("git push")));
        assert!(super::is_remote_command(&argv("git push origin main")));
    }

    #[test]
    fn test_is_remote_command_pull() {
        assert!(super::is_remote_command(&argv("git pull")));
        assert!(super::is_remote_command(&argv("git pull --rebase")));
    }

    #[test]
    fn test_is_remote_command_fetch() {
        assert!(super::is_remote_command(&argv("git fetch")));
        assert!(super::is_remote_command(&argv("git fetch --all")));
    }

    #[test]
    fn test_is_remote_command_clone() {
        assert!(super::is_remote_command(&argv("git clone")));
        assert!(super::is_remote_command(&argv(
            "git clone https://github.com/org/repo.git"
        )));
    }

    #[test]
    fn test_is_remote_command_ls_remote() {
        assert!(super::is_remote_command(&argv("git ls-remote")));
    }

    #[test]
    fn test_is_remote_command_local_commands() {
        assert!(!super::is_remote_command(&argv("git status")));
        assert!(!super::is_remote_command(&argv("git add .")));
        assert!(!super::is_remote_command(&argv("git commit -m \"msg\"")));
        assert!(!super::is_remote_command(&argv("git log")));
        assert!(!super::is_remote_command(&argv("git diff")));
        assert!(!super::is_remote_command(&argv("git branch")));
        assert!(!super::is_remote_command(&argv("git checkout main")));
    }
}
