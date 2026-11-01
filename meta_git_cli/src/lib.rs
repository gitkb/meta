//! meta-git library
//!
//! Provides git operations optimized for meta repositories.

mod clone;
mod clone_worker;
mod commit;
mod git_env;
mod helpers;
mod ssh;
mod update;

mod commands;

use log::debug;
use meta_plugin_protocol::{has_help_token, CommandResult, PlannedCommand, PluginRequestOptions};
use std::path::Path;

use helpers::get_project_directories_with_fallback;

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
        // They find their repos in the set; worktree exec takes its own filters
        if options.filtered() {
            return CommandResult::Error(format!(
                "meta {command} does not take --include/--exclude/--tag before it; use `meta git worktree exec <name> --include <repo> -- <command>`"
            ));
        }
        return commands::worktree::execute_worktree_command(command, args, options, cwd);
    }

    // These act on repos they find themselves, not on the selected ones
    if matches!(command, "git update" | "git clone") && options.filtered() {
        return CommandResult::Error(format!(
            "meta {command} finds its repos itself (in the .meta.yaml files); it does not take --include/--exclude/--tag"
        ));
    }

    // Commands meta adapts parse their own arguments, help included
    match command {
        "git clone" => {
            return match meta_plugin_protocol::parse_args(command, args) {
                Ok(args) => finish(clone::execute_git_clone(args, options, cwd)),
                Err(result) => result,
            }
        }
        "git update" => {
            return match meta_plugin_protocol::parse_args::<update::UpdateArgs>(command, args) {
                Ok(update::UpdateArgs {}) => {
                    finish(update::execute_git_update(projects, options.mode(), cwd))
                }
                Err(result) => result,
            }
        }
        "git commit" => {
            // meta sends a help request without its repos: whatever clap
            // makes of the arguments, it never commits
            let help = ["--help".to_string()];
            let args = if help_requested { &help[..] } else { args };
            return match meta_plugin_protocol::parse_args(command, args) {
                Ok(args) => finish(commit::execute_git_commit(args, projects, options, cwd)),
                Err(result) => result,
            };
        }
        _ => {}
    }

    if help_requested {
        if let Some(help) = build_passthrough_git_help(command, args, projects, cwd) {
            return CommandResult::Message(help);
        }
        return CommandResult::ShowHelp(None);
    }

    // `git status` and every other git command run as typed in each repo
    execute_raw_git_command(command, args, projects, options, cwd)
}

fn finish(result: anyhow::Result<CommandResult>) -> CommandResult {
    result.unwrap_or_else(|e| CommandResult::Error(format!("{e:#}")))
}

/// Check if a git command accesses remote repositories
fn is_remote_command(argv: &[String]) -> bool {
    let remote_commands = ["push", "pull", "fetch", "clone", "ls-remote"];
    argv.get(1)
        .is_some_and(|subcommand| remote_commands.contains(&subcommand.as_str()))
}

fn build_passthrough_git_help(
    command: &str,
    args: &[String],
    projects: &[String],
    cwd: &Path,
) -> Option<String> {
    let command_words: Vec<&str> = command
        .split_whitespace()
        .filter(|word| *word != "--help" && *word != "-h")
        .collect();

    if command_words.first().copied() != Some("git") {
        return None;
    }

    let adapted_commands = ["clone", "update", "commit", "worktree"];
    let git_args: Vec<String> = command_words
        .iter()
        .skip(1)
        .map(|s| s.to_string())
        .chain(
            args.iter()
                .filter(|arg| arg.as_str() != "--help" && arg.as_str() != "-h")
                .cloned(),
        )
        .collect();
    let subcommand = git_args.first()?;

    if adapted_commands.contains(&subcommand.as_str()) {
        return None;
    }

    let git_argv: Vec<String> = std::iter::once("git".to_string())
        .chain(git_args.iter().cloned())
        .collect();
    let display_command = loop_lib::display_command(&git_argv);
    let scope = match get_project_directories_with_fallback(projects, cwd) {
        Ok(dirs) => {
            if dirs.is_empty() {
                "Current scope: no repos were discovered.".to_string()
            } else {
                format!(
                    "Current scope: {} repo(s): {}",
                    dirs.len(),
                    dirs.iter()
                        .map(|d| d.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        Err(_) => "Current scope: unable to resolve repos from the current directory.".to_string(),
    };

    Some(format!(
        "meta git {subcommand} - Pass-through git command\n\n\
Usage: meta [META_OPTIONS] git {subcommand} [GIT_ARGS...]\n\n\
Runs `{display_command}` in each repo in the current meta workspace.\n\n\
{scope}\n\n\
Meta options (before `git`):\n\
  --dry-run             Show the command that would run in each repo\n\
  --recursive           Include repos from nested meta workspaces\n\
  --depth <N>           Limit recursive traversal depth\n\
  --include <REPOS>     These repos (and those --tag selects; .meta.yaml name or path from the root)\n\
  --exclude <REPOS>     Skip these repos (.meta.yaml name or path from the root)\n\
  --tag <TAGS>          Filter by project tag(s), comma-separated\n\
  --parallel            Run repo commands concurrently\n\
  --sequential          Run repo commands one at a time\n\n\
Examples:\n\
  meta git {subcommand}\n\
  meta --dry-run git {subcommand}\n\
  meta --recursive --dry-run git {subcommand}\n\n\
Git help: run `git {subcommand} --help` directly for upstream git options."
    ))
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
            return CommandResult::Plan(commands, Some(true));
        }

        let (ssh_env, parallel_ok) = match ssh::establish_ssh_masters(&urls) {
            ssh::SshMasters::OurSockets(dir) => (Some(ssh::git_ssh_command(&dir)), true),
            ssh::SshMasters::UserManaged => (None, true),
            ssh::SshMasters::Failed => {
                ssh::warn("SSH multiplexing could not be set up: running one repository at a time");
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
            CommandResult::FullPlan(ExecutionPlan {
                pre_commands: vec![],
                commands,
                post_commands: vec![],
                parallel: Some(true),
                max_parallel: Some(SSH_MAX_SESSIONS),
                spawn_stagger_ms: Some(SSH_SPAWN_STAGGER_MS),
            })
        } else {
            // SSH setup failed — fall back to sequential
            CommandResult::Plan(commands, Some(false))
        }
    } else {
        CommandResult::Plan(commands, Some(options.parallel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }
    use commit::parse_multi_commit_file;
    use helpers::get_project_directories;
    use meta_plugin_protocol::{ExecutionPlan, PlanResponse, PlannedCommand};
    use tempfile::TempDir;

    #[test]
    fn test_execute_git_status_no_meta_file() {
        let temp_dir = TempDir::new().unwrap();
        let options = PluginRequestOptions::default();

        let result = execute_command("git status", &[], &[], &options, temp_dir.path());

        // Should succeed (returns a Plan, not an Error)
        assert!(!matches!(result, CommandResult::Error(_)));
    }

    #[test]
    fn test_meta_config_parsing() {
        let dir = tempfile::tempdir().unwrap();
        let meta_path = dir.path().join(".meta.yaml");
        std::fs::write(
            &meta_path,
            r#"{"projects": {"foo": "git@github.com:org/foo.git", "bar": "git@github.com:org/bar.git"}}"#,
        )
        .unwrap();
        let projects = meta_core::config::Workspace::load(&meta_path)
            .unwrap()
            .projects;
        assert_eq!(projects.len(), 2);
        let names: std::collections::HashSet<_> =
            projects.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains("foo"));
        assert!(names.contains("bar"));
    }

    #[test]
    fn test_unknown_command_falls_through_to_raw_git() {
        // Unknown git subcommands should fall through to raw git execution
        let options = PluginRequestOptions::default();
        let result = execute_command("git unknown", &[], &[], &options, Path::new("."));
        // Should return a Plan (to run `git unknown` in all repos), not ShowHelp
        assert!(matches!(result, CommandResult::Plan(_, _)));
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

    // ============ Execution Plan Tests ============

    #[test]
    fn test_execution_plan_serialization() {
        let plan = ExecutionPlan {
            pre_commands: vec![],
            commands: vec![
                PlannedCommand {
                    dir: "./repo1".to_string(),
                    argv: argv("git status"),
                    env: None,
                },
                PlannedCommand {
                    dir: "./repo2".to_string(),
                    argv: argv("git status"),
                    env: None,
                },
            ],
            post_commands: vec![],
            parallel: Some(false),
            max_parallel: None,
            spawn_stagger_ms: None,
        };

        let json = serde_json::to_string(&plan).unwrap();
        assert!(json.contains("\"dir\":\"./repo1\""));
        assert!(json.contains(r#""argv":["git","status"]"#));
        assert!(json.contains("\"parallel\":false"));
    }

    #[test]
    fn test_execution_plan_without_parallel() {
        let plan = ExecutionPlan {
            pre_commands: vec![],
            commands: vec![PlannedCommand {
                dir: ".".to_string(),
                argv: argv("ls"),
                env: None,
            }],
            post_commands: vec![],
            parallel: None,
            max_parallel: None,
            spawn_stagger_ms: None,
        };

        let json = serde_json::to_string(&plan).unwrap();
        // parallel and max_parallel should be omitted when None due to skip_serializing_if
        assert!(!json.contains("parallel"));
        assert!(!json.contains("max_parallel"));
        assert!(!json.contains("spawn_stagger_ms"));
    }

    #[test]
    fn test_planned_command_serialization() {
        let cmd = PlannedCommand {
            dir: "/absolute/path".to_string(),
            argv: argv("git pull --rebase"),
            env: None,
        };

        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"dir\":\"/absolute/path\""));
        assert!(json.contains(r#""argv":["git","pull","--rebase"]"#));
    }

    #[test]
    fn test_plan_response_serialization() {
        let response = PlanResponse {
            plan: ExecutionPlan {
                pre_commands: vec![],
                commands: vec![PlannedCommand {
                    dir: "project".to_string(),
                    argv: argv("make build"),
                    env: None,
                }],
                post_commands: vec![],
                parallel: Some(true),
                max_parallel: None,
                spawn_stagger_ms: None,
            },
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"plan\":"));
        assert!(json.contains("\"commands\":"));
        assert!(json.contains("\"dir\":\"project\""));
        assert!(json.contains(r#""argv":["make","build"]"#));
        assert!(json.contains("\"parallel\":true"));
    }

    #[test]
    fn test_plan_response_structure() {
        // Test that the JSON structure matches what subprocess_plugins expects
        let response = PlanResponse {
            plan: ExecutionPlan {
                pre_commands: vec![],
                commands: vec![
                    PlannedCommand {
                        dir: "a".to_string(),
                        argv: argv("cmd1"),
                        env: None,
                    },
                    PlannedCommand {
                        dir: "b".to_string(),
                        argv: argv("cmd2"),
                        env: None,
                    },
                ],
                post_commands: vec![],
                parallel: Some(false),
                max_parallel: None,
                spawn_stagger_ms: None,
            },
        };

        let json = serde_json::to_string(&response).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Verify structure matches what shim expects
        assert!(parsed.get("plan").is_some());
        let plan = parsed.get("plan").unwrap();
        assert!(plan.get("commands").is_some());
        let commands = plan.get("commands").unwrap().as_array().unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].get("dir").unwrap().as_str().unwrap(), "a");
        assert_eq!(commands[0]["argv"], serde_json::json!(["cmd1"]));
        assert!(!plan.get("parallel").unwrap().as_bool().unwrap());
    }

    #[test]
    fn test_execution_plan_empty_commands() {
        let plan = ExecutionPlan {
            pre_commands: vec![],
            commands: vec![],
            post_commands: vec![],
            parallel: None,
            max_parallel: None,
            spawn_stagger_ms: None,
        };

        let json = serde_json::to_string(&plan).unwrap();
        assert!(json.contains("\"commands\":[]"));
    }

    #[test]
    fn test_planned_command_with_special_chars() {
        let cmd = PlannedCommand {
            dir: "./path with spaces".to_string(),
            argv: vec![
                "git".to_string(),
                "commit".to_string(),
                "-m".to_string(),
                "feat: \"add\" feature".to_string(),
            ],
            env: None,
        };

        let json = serde_json::to_string(&cmd).unwrap();
        // JSON should properly escape the string
        assert!(json.contains("path with spaces"));
        assert!(json.contains("\"feat: \\\"add\\\" feature\""));
    }

    #[test]
    fn test_planned_command_git_clone() {
        let cmd = PlannedCommand {
            dir: "/home/user/workspace".to_string(),
            argv: argv("git clone git@github.com:org/repo.git my-repo"),
            env: None,
        };

        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""git","clone""#));
        assert!(json.contains("git@github.com:org/repo.git"));
    }

    #[test]
    fn test_execution_plan_many_commands() {
        let commands: Vec<PlannedCommand> = (0..100)
            .map(|i| PlannedCommand {
                dir: format!("./repo_{i}"),
                argv: argv("git status"),
                env: None,
            })
            .collect();

        let plan = ExecutionPlan {
            pre_commands: vec![],
            commands,
            post_commands: vec![],
            parallel: Some(true),
            max_parallel: None,
            spawn_stagger_ms: None,
        };

        let json = serde_json::to_string(&plan).unwrap();
        assert!(json.contains("./repo_0"));
        assert!(json.contains("./repo_99"));
        assert!(json.contains("\"parallel\":true"));
    }

    // ============ get_project_directories Tests ============

    #[test]
    fn test_get_project_directories_no_meta_file() {
        let temp_dir = TempDir::new().unwrap();

        let dirs = get_project_directories(temp_dir.path()).unwrap();

        // Should return just "." when no .meta.yaml file
        assert_eq!(dirs, vec!["."]);
    }

    #[test]
    fn test_get_project_directories_refuses_a_bad_meta_file() {
        // A refused config is an error, not "just the root"
        let temp_dir = TempDir::new().unwrap();
        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
            r#"{"projects": {"evil": {"repo": "u", "path": "../../x"}}}"#,
        )
        .unwrap();

        let err = get_project_directories(temp_dir.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("must not contain '..'"),
            "{err:#}"
        );
    }

    #[test]
    fn test_get_project_directories_with_meta_file() {
        let temp_dir = TempDir::new().unwrap();

        // Create .meta.yaml file
        let meta_content = r#"{"projects": {"alpha": "url1", "beta": "url2", "gamma": "url3"}}"#;
        std::fs::write(temp_dir.path().join(".meta.yaml"), meta_content).unwrap();

        let dirs = get_project_directories(temp_dir.path()).unwrap();

        // Should return "." plus sorted project names
        assert_eq!(dirs.len(), 4);
        assert_eq!(dirs[0], ".");
        assert_eq!(dirs[1], "alpha");
        assert_eq!(dirs[2], "beta");
        assert_eq!(dirs[3], "gamma");
    }

    #[test]
    fn test_get_project_directories_sorted() {
        let temp_dir = TempDir::new().unwrap();

        // Create .meta.yaml file with unsorted projects
        let meta_content = r#"{"projects": {"zebra": "url1", "alpha": "url2", "middle": "url3"}}"#;
        std::fs::write(temp_dir.path().join(".meta.yaml"), meta_content).unwrap();

        let dirs = get_project_directories(temp_dir.path()).unwrap();

        // Projects should be sorted alphabetically
        assert_eq!(dirs[1], "alpha");
        assert_eq!(dirs[2], "middle");
        assert_eq!(dirs[3], "zebra");
    }

    #[test]
    fn test_get_project_directories_from_inside_a_project() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
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
        let temp_dir = TempDir::new().unwrap();

        // Create .meta.yaml file with no projects
        let meta_content = r#"{"projects": {}}"#;
        std::fs::write(temp_dir.path().join(".meta.yaml"), meta_content).unwrap();

        let dirs = get_project_directories(temp_dir.path()).unwrap();

        // Should return just "."
        assert_eq!(dirs, vec!["."]);
    }

    // ============ Integration-like Tests ============

    #[test]
    fn test_git_status_returns_execution_plan_format() {
        // We can't easily test the actual output, but we can verify
        // the plan structure by creating one and checking its JSON format
        let dirs = vec![".".to_string(), "repo1".to_string(), "repo2".to_string()];

        let commands: Vec<PlannedCommand> = dirs
            .into_iter()
            .map(|dir| PlannedCommand {
                dir,
                argv: argv("git status"),
                env: None,
            })
            .collect();

        let response = PlanResponse {
            plan: ExecutionPlan {
                pre_commands: vec![],
                commands,
                post_commands: vec![],
                parallel: Some(false),
                max_parallel: None,
                spawn_stagger_ms: None,
            },
        };

        let json = serde_json::to_string(&response).unwrap();

        // Verify it can be parsed as expected by the shim
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["plan"]["commands"].is_array());
        assert_eq!(parsed["plan"]["commands"].as_array().unwrap().len(), 3);
        assert!(!parsed["plan"]["parallel"].as_bool().unwrap());
    }

    #[test]
    fn test_git_update_plan_for_missing_repos() {
        // Simulate what execute_git_update would return for missing repos
        let missing_repos = [
            ("repo1", "git@github.com:org/repo1.git"),
            ("repo2", "git@github.com:org/repo2.git"),
        ];

        let cwd = "/home/user/workspace";
        let commands: Vec<PlannedCommand> = missing_repos
            .iter()
            .map(|(name, url)| PlannedCommand {
                dir: cwd.to_string(),
                argv: argv(&format!("git clone {url} {name}")),
                env: None,
            })
            .collect();

        let response = PlanResponse {
            plan: ExecutionPlan {
                pre_commands: vec![],
                commands,
                post_commands: vec![],
                parallel: Some(false),
                max_parallel: None,
                spawn_stagger_ms: None,
            },
        };

        let json = serde_json::to_string(&response).unwrap();

        // Verify clone commands are in the plan
        assert!(json.contains(r#""git","clone""#));
        assert!(json.contains("repo1"));
        assert!(json.contains("repo2"));
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

    #[test]
    fn test_execution_plan_max_parallel_serialization() {
        let plan = ExecutionPlan {
            pre_commands: vec![],
            commands: vec![PlannedCommand {
                dir: ".".to_string(),
                argv: argv("git push"),
                env: None,
            }],
            post_commands: vec![],
            parallel: Some(true),
            max_parallel: Some(10),
            spawn_stagger_ms: Some(25),
        };

        let json = serde_json::to_string(&plan).unwrap();
        assert!(json.contains("\"max_parallel\":10"));
        assert!(json.contains("\"spawn_stagger_ms\":25"));

        // Verify round-trip
        let restored: ExecutionPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.max_parallel, Some(10));
        assert_eq!(restored.spawn_stagger_ms, Some(25));
    }

    #[test]
    fn test_execution_plan_max_parallel_omitted_when_none() {
        let plan = ExecutionPlan {
            pre_commands: vec![],
            commands: vec![PlannedCommand {
                dir: ".".to_string(),
                argv: argv("git status"),
                env: None,
            }],
            post_commands: vec![],
            parallel: Some(true),
            max_parallel: None,
            spawn_stagger_ms: None,
        };

        let json = serde_json::to_string(&plan).unwrap();
        // max_parallel and spawn_stagger_ms should be omitted when None due to skip_serializing_if
        assert!(!json.contains("max_parallel"));
        assert!(!json.contains("spawn_stagger_ms"));
    }
}
