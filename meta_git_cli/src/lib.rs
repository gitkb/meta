//! meta-git library
//!
//! Provides git operations optimized for meta repositories.

mod clone;
mod clone_missing;
mod clone_queue;
mod clone_worker;
mod commit;
mod git_env;
mod here;
mod ssh;
mod update;

mod commands;
mod worktree;

use log::debug;
use meta_plugin::display::escape;
use meta_plugin::{
    has_help_token, Ctx, Plan, PluginHelp, PluginInfo, ProjectResult, Request, Response, Step,
    Warning,
};

/// `meta git`: the git commands meta adapts; any other runs as typed.
#[derive(clap::Parser)]
// `meta git help log` is git's
#[command(name = "meta git", disable_help_subcommand = true)]
struct GitCli {
    #[command(subcommand)]
    command: Option<GitCommand>,
    /// Any other git command, as typed. Never read: `request.args` is
    /// what runs.
    #[arg(hide = true, trailing_var_arg = true, allow_hyphen_values = true)]
    as_typed: Vec<String>,
}

/// The git commands meta adapts, in the order help shows them.
#[derive(clap::Subcommand)]
enum GitCommand {
    Clone(clone::CloneArgs),
    CloneMissing(clone_missing::CloneMissingArgs),
    ClonePrune(clone_missing::ClonePruneArgs),
    CloneSync(clone_missing::CloneSyncArgs),
    Commit(commit::CommitArgs),
    Update(update::UpdateArgs),
    Worktree(commands::worktree::WorktreeParser),
}

/// What meta-git says about itself: the commands it routes and their
/// summaries, read from [`GitCli`], and its help.
pub fn plugin_info() -> PluginInfo {
    use clap::CommandFactory;
    let cli = GitCli::command();
    let summary = |c: &clap::Command| c.get_about().map(ToString::to_string).unwrap_or_default();
    PluginInfo {
        name: "git".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        commands: routed("git", &cli),
        description: Some("Git operations for meta repositories".to_string()),
        help: Some(PluginHelp {
            usage: "Usage: meta git <COMMAND> [OPTIONS]".to_string(),
            commands: cli
                .get_subcommands()
                .map(|c| (c.get_name().to_string(), summary(c)))
                .collect(),
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

/// Every command under `cli`, spelled from `words`: `git worktree`,
/// `git worktree create`, ...
fn routed(words: &str, cli: &clap::Command) -> Vec<String> {
    cli.get_subcommands()
        .flat_map(|c| {
            let words = format!("{words} {}", c.get_name());
            std::iter::once(words.clone()).chain(routed(&words, c))
        })
        .collect()
}

/// Answer `request`: a command meta-git adapts, or any other git command,
/// as typed, in each selected repo.
pub fn execute_command(ctx: &Ctx, request: &Request) -> meta_plugin::Result<Response> {
    let command = request.command.as_str();
    let args = &request.args;
    debug!("[meta_git_cli] Plugin invoked with command: '{command}'");
    debug!(
        "[meta_git_cli] {} projects, depth {:?}",
        request.projects.len(),
        request.depth
    );

    // The words after `git`, as typed
    let words: Vec<String> = command
        .split_whitespace()
        .skip(1)
        .map(str::to_string)
        .chain(args.iter().cloned())
        .collect();
    let meta = &request.meta_options;
    let parse = |words: &[String]| meta_plugin::parse_args::<GitCli>("git", words, meta);
    let cli = match parse(&words) {
        Ok(cli) => cli,
        Err(response) => return Ok(*response),
    };
    let help_requested = has_help_token(command, args);
    match cli.command {
        Some(GitCommand::Clone(args)) => clone::execute_git_clone(args, ctx, request),
        Some(GitCommand::Update(args)) => update::execute_git_update(args, ctx, request),
        Some(GitCommand::CloneMissing(args)) => clone_missing::execute(args.into(), ctx, request),
        Some(GitCommand::ClonePrune(args)) => clone_missing::execute(args.into(), ctx, request),
        Some(GitCommand::CloneSync(args)) => clone_missing::execute(args.into(), ctx, request),
        // meta sends a help request without its repos: whatever clap made
        // of the arguments, it never commits
        Some(GitCommand::Commit(_)) if help_requested => {
            let help = ["commit".to_string(), "--help".to_string()];
            Ok(parse(&help).map_or_else(|response| *response, |_| Response::done()))
        }
        Some(GitCommand::Commit(args)) => commit::execute_git_commit(args, request),
        Some(GitCommand::Worktree(worktree)) => {
            commands::worktree::handle_worktree_command(worktree.command, ctx, request)
        }
        // meta answers the help of a command it runs as typed itself
        None if help_requested => Ok(Response::message(meta_plugin::help_text(
            "git",
            &plugin_info(),
        ))),
        // Every other git command runs as typed in each repo
        None => execute_raw_git_command(ctx, request),
    }
}

/// Records a command printed itself, for one that failed with `failure`
/// after printing them: under `--json` the records, the failure among them
/// (as a record of its own unless one of them is a failure), so the
/// Envelope has both; in text, the failure, said as before.
pub(crate) fn printed(
    ctx: &Ctx,
    mut records: Vec<ProjectResult>,
    failure: Option<anyhow::Error>,
) -> meta_plugin::Result<Response> {
    let Some(failure) = failure else {
        return Ok(Response::printed(records));
    };
    if !ctx.meta().json || records.is_empty() {
        return Err(failure);
    }
    if !records.iter().any(ProjectResult::is_failure) {
        records.push(ProjectResult {
            error: Some(meta_plugin::envelope::info_of(&failure)),
            ..ProjectResult::default()
        });
    }
    Ok(Response::printed(records))
}

/// Run the command `request` is again, from the top, through meta (as typed,
/// meta's options first), for a command whose plan changed under it (it
/// cloned a meta repo whose config lists more): meta reads the configs
/// anew. In text it shows as it runs; under `--json` its rows are this
/// call's.
pub(crate) fn again(ctx: &Ctx, request: &Request) -> anyhow::Result<Response> {
    let words: Vec<String> = request
        .command
        .split_whitespace()
        .map(str::to_string)
        .chain(request.args.iter().cloned())
        .collect();
    through_meta(ctx, request, &request.cwd, &words)
}

/// Run meta with its options as typed, then `words`, from `cwd`, as [`again`]
/// does: in text as it shows, under `--json` its rows this call's.
pub(crate) fn through_meta(
    ctx: &Ctx,
    request: &Request,
    cwd: &std::path::Path,
    words: &[String],
) -> anyhow::Result<Response> {
    let json = ctx.meta().json;
    let args: Vec<String> = request
        .meta_options
        .typed
        .iter()
        .filter(|word| !(json && *word == "--json"))
        .cloned()
        .chain(words.iter().cloned())
        .collect();
    let meta = meta_plugin::client::Meta::from_env()?.cwd(cwd).args(args);
    if !json {
        let status = meta.inherit()?;
        return match status.success() {
            true => Ok(Response::done()),
            false => Err(meta_plugin::exit::ExitWith::of(status).into()),
        };
    }
    let envelope = meta.json()?.wait()?;
    for warning in envelope.warnings {
        ctx.warn(warning);
    }
    match envelope.error {
        Some(error) => printed(ctx, envelope.results, Some(error.into())),
        None => Ok(Response::printed(envelope.results)),
    }
}

/// [`printed`], for a worktree command: under `--json` its records, then
/// the failure (the command's error), then a row for each repo whose git
/// command failed, with git's stderr. In text the failure says it all,
/// git's stderr under the repo it names.
pub(crate) fn printed_with_git(
    ctx: &Ctx,
    request: &Request,
    mut records: Vec<ProjectResult>,
    failure: Option<anyhow::Error>,
) -> meta_plugin::Result<Response> {
    let Some(failure) = failure else {
        return Ok(Response::printed(records));
    };
    let root = request
        .workspace
        .as_ref()
        .map_or(request.cwd.as_path(), |w| w.root.as_path());
    let rows = worktree::git_ops::failure_rows(&failure, root);
    if !ctx.meta().json || rows.is_empty() {
        return printed(ctx, records, Some(failure));
    }
    records.push(ProjectResult {
        error: Some(meta_plugin::envelope::info_of(&failure)),
        ..ProjectResult::default()
    });
    records.extend(rows);
    Ok(Response::printed(records))
}

/// Check if a git command accesses remote repositories
fn is_remote_command(argv: &[String]) -> bool {
    let remote_commands = ["push", "pull", "fetch", "clone", "ls-remote"];
    argv.get(1)
        .is_some_and(|subcommand| remote_commands.contains(&subcommand.as_str()))
}

/// Plan a raw git command in each selected repo (any git command meta-git
/// doesn't adapt)
fn execute_raw_git_command(ctx: &Ctx, request: &Request) -> meta_plugin::Result<Response> {
    // The git argv: the command words (e.g. "git" or "git add") and the
    // user's args exactly as given
    let argv: Vec<String> = request
        .command
        .split_whitespace()
        .map(str::to_string)
        .chain(request.args.iter().cloned())
        .collect();
    // Set git-specific environment variables (pager, colors, prompts)
    let git_env: std::collections::BTreeMap<String, String> =
        git_env::git_env().into_iter().collect();

    // A listed project that isn't cloned yet is named, not counted as a
    // failure: cloning it is `meta git clone-missing`'s (with --strict, an
    // error)
    let mut steps = Vec::new();
    for project in &request.projects {
        if !project.present {
            ctx.warn_or_bail(Warning::about(
                &project.path,
                format!(
                    "{} is not cloned; `meta git clone-missing` clones it",
                    escape(&project.path)
                ),
            ))?;
            continue;
        }
        let mut step = Step::new(Some(&project.path), &project.abs, argv.clone());
        step.env = git_env.clone();
        steps.push(step);
    }
    let plan = |steps, parallel: Option<bool>, max_parallel, stagger_ms| {
        Ok(Response::plan(Plan {
            steps,
            parallel,
            max_parallel,
            stagger_ms,
        }))
    };

    // For remote commands running in parallel, establish SSH ControlMaster
    // connections and inject GIT_SSH_COMMAND into each step. A preview
    // connects nowhere.
    if !request.meta.parallel() || ctx.dry_run() || !is_remote_command(&argv) {
        return plan(steps, None, None, None);
    }
    let urls = ssh::ssh_urls(&request.projects);
    // HTTPS-only workspaces don't need SSH multiplexing
    if urls.is_empty() {
        return plan(steps, None, None, None);
    }
    let (ssh_env, parallel_ok) = match ssh::establish_ssh_masters(&urls) {
        ssh::SshMasters::OurSockets(dir) => (Some(ssh::git_ssh_command(&dir)), true),
        ssh::SshMasters::UserManaged => (None, true),
        ssh::SshMasters::Failed => {
            log::debug!("SSH multiplexing could not be set up: running one repository at a time");
            (None, false)
        }
    };
    if let Some(ssh) = &ssh_env {
        for step in &mut steps {
            step.env.insert("GIT_SSH_COMMAND".to_string(), ssh.clone());
        }
    }
    // SSH ControlMaster has a default MaxSessions limit of 10 (server-side).
    const SSH_MAX_SESSIONS: usize = 10;
    // Stagger spawns by 25ms to prevent SSH socket saturation.
    const SSH_SPAWN_STAGGER_MS: u64 = 25;
    match parallel_ok {
        true => plan(
            steps,
            Some(true),
            Some(SSH_MAX_SESSIONS),
            Some(SSH_SPAWN_STAGGER_MS),
        ),
        // SSH setup failed — fall back to sequential
        false => plan(steps, Some(false), None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meta_plugin::testing::Sandbox;

    fn argv(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }
    use commit::parse_multi_commit_file;
    use meta_plugin::{Answer, Meta, Project};

    /// `meta git <command> <args>` answered in `cwd`, with `projects`
    fn answer(
        command: &str,
        args: &str,
        cwd: &std::path::Path,
        projects: Vec<Project>,
    ) -> Response {
        let mut request = Request::new(command, cwd);
        request.args = argv(args);
        request.projects = projects;
        execute_command(&Ctx::new(Meta::default()), &request).unwrap()
    }

    #[test]
    fn a_line_clap_has_no_command_for_is_git_as_typed() {
        use clap::{CommandFactory, Parser};
        GitCli::command().debug_assert();
        let parse = |line: &str| GitCli::try_parse_from(argv(&format!("meta-git {line}")));
        for line in [
            "-c key=value status",
            "--no-pager log -1",
            "-C d status",
            "log --oneline -3",
            "pull -r",
            "help log",
            "status -- x",
            "-C d worktree remove s",
        ] {
            let cli = parse(line).unwrap();
            assert!(cli.command.is_none(), "{line}");
            assert_eq!(cli.as_typed, argv(line), "{line}");
        }
        let cli = parse("worktree remove s").unwrap();
        assert!(matches!(cli.command, Some(GitCommand::Worktree(_))));
        assert!(cli.as_typed.is_empty());
    }

    #[test]
    fn each_command_meta_git_adapts_names_its_git_options() {
        let sandbox = Sandbox::new();
        for verb in plugin_info().help.unwrap().commands.keys() {
            let response = answer(&format!("git {verb}"), "--help", &sandbox.work(), vec![]);
            let help = match response.answer {
                Answer::Message(help) => help,
                _ => String::new(),
            };
            assert!(help.contains("\nGit options: "), "git {verb}: {help}");
        }
    }

    #[test]
    fn a_raw_command_runs_in_each_selected_repo_and_names_one_not_cloned() {
        let sandbox = Sandbox::new();
        let root = sandbox.work();
        let project = |path: &str, present: bool| Project {
            name: path.to_string(),
            path: path.to_string(),
            abs: root.join(path),
            present,
            ..Project::default()
        };
        // Unknown git subcommands fall through to raw git, as typed
        let response = answer(
            "git unknown",
            "-x",
            &root,
            vec![project(".", true), project("gone", false)],
        );
        let steps = match response.answer {
            Answer::Plan(plan) => plan.steps,
            _ => Vec::new(),
        };
        let steps: Vec<_> = steps
            .iter()
            .map(|s| (s.project.as_deref(), s.argv.join(" ")))
            .collect();
        assert_eq!(steps, [(Some("."), "git unknown -x".to_string())]);
        let ctx = Ctx::new(Meta::default());
        let mut request = Request::new("git status", &root);
        request.projects = vec![project("gone", false)];
        let warned = ctx
            .finish(execute_command(&ctx, &request).unwrap())
            .warnings;
        assert_eq!(warned.len(), 1);
        assert!(
            warned[0].message.contains("gone is not cloned"),
            "{warned:?}"
        );
        // with --strict, an error
        let strict = Ctx::new(Meta {
            strict: true,
            ..Meta::default()
        });
        assert!(execute_command(&strict, &request).is_err());
        // no selection is no repo, never "find them yourself"
        let response = answer("git status", "", &root, vec![]);
        assert!(matches!(response.answer, Answer::Plan(plan) if plan.steps.is_empty()));
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
