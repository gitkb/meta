use colored::Colorize;
use meta_plugin::display::{escape, escape_lines};
use meta_plugin::outln;
use meta_plugin::{Plan, Request, Response, Step};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A repo with staged changes: its name in the workspace, its folder (as
/// the OS names it, never re-spelled) and its staged files.
struct Staged {
    name: String,
    dir: PathBuf,
    files: Vec<String>,
}

/// Commit what is staged, in every repo that has staged changes.
#[derive(clap::Parser)]
#[command(
    after_help = "Git options: of git commit's own, only -m (one message for every repo) and -e (an editor for each repo)."
)]
pub(crate) struct CommitArgs {
    /// Write each repo's message in your editor (needs a terminal)
    #[arg(short, long)]
    edit: bool,
    /// Use this message in every repo
    #[arg(short, long, allow_hyphen_values = true)]
    message: Option<String>,
}

/// Execute git commit with optional --edit flag for per-repo messages, in
/// the selected repos that have staged changes
pub(crate) fn execute_git_commit(args: CommitArgs, request: &Request) -> anyhow::Result<Response> {
    let (use_editor, message) = (args.edit, args.message);

    // Find repos with staged changes
    let mut repos_with_changes: Vec<Staged> = Vec::new();

    for project in &request.projects {
        let (dir, path) = (&project.path, &project.abs);
        // a folder with nothing git's in it (not cloned, or a workspace
        // that is no repository) has nothing staged; a .git git can't use is
        // the user's to know about
        match meta_core::gitdir::common_dir(path) {
            Ok(Some(_)) => {}
            Ok(None) => continue,
            Err(e) => anyhow::bail!("{}: not a git repository ({e:#})", escape(dir)),
        }
        let files = staged_files(path)?;
        if !files.is_empty() {
            repos_with_changes.push(Staged {
                name: dir.clone(),
                dir: path.clone(),
                files,
            });
        }
    }

    if repos_with_changes.is_empty() {
        return Ok(Response::message(
            "No staged changes found in any repository.",
        ));
    }

    if use_editor {
        // Open editor for per-repo messages (interactive, cannot use ExecutionPlan)
        execute_editor_commit(&repos_with_changes, request.meta.mode(), &request.data_dir)?;
    } else if let Some(msg) = message {
        // Apply same message to all repos: a plan, which meta previews
        let steps = repos_with_changes
            .iter()
            .map(|repo| {
                let argv = ["git", "commit", "-m", msg.as_str()].map(str::to_string);
                Step::new(Some(&repo.name), &repo.dir, argv.to_vec())
            })
            .collect();
        return Ok(Response::plan(Plan {
            steps,
            ..Plan::default()
        }));
    } else {
        // No message provided, show what would be committed
        outln!("Repositories with staged changes:")?;
        list_staged(&repos_with_changes)?;
        outln!()?;
        outln!(
            "Use {} to create per-repo commit messages",
            "--edit".yellow()
        )?;
        outln!(
            "Use {} to apply the same message to all",
            "-m \"message\"".yellow()
        )?;
    }

    Ok(Response::done())
}

/// Print each of `repos` with the number of its staged files.
fn list_staged(repos: &[Staged]) -> anyhow::Result<()> {
    for repo in repos {
        outln!(
            "  {} ({} files)",
            escape(&repo.name).to_string().cyan(),
            repo.files.len()
        )?;
    }
    Ok(())
}

/// The staged files of the repo at `path`.
fn staged_files(path: &std::path::Path) -> anyhow::Result<Vec<String>> {
    Ok(
        meta_core::git::run(path, ["diff", "--cached", "--name-only"])?
            .lines()
            .map(String::from)
            .collect(),
    )
}

/// Execute commit with editor for per-repo messages
fn execute_editor_commit(
    repos: &[Staged],
    mode: meta_plugin::ExecutionMode,
    data_dir: &std::path::Path,
) -> anyhow::Result<()> {
    use std::io::Write;

    // The editor git would use, from the first repo's config; asked first,
    // so a missing one or a missing terminal fails before anything is written
    let Some(first) = repos.first() else {
        return Ok(());
    };
    let first = first.dir.as_path();
    let editor = meta_core::git::run(first, ["var", "GIT_EDITOR"]).map_err(|e| {
        anyhow::anyhow!(
            "git has no editor to use ({e:#}): set GIT_EDITOR, core.editor, VISUAL or EDITOR, \
             or use -m <message>"
        )
    })?;
    // As in git, the editor `:` leaves the message as it is
    let terminal = match editor.as_str() {
        ":" => None,
        _ => Some(Terminal::open().map_err(|why| {
            anyhow::anyhow!(
                "The editor '{}' needs the terminal, but {why}. Use -m <message> instead.",
                escape(&editor)
            )
        })?),
    };
    // A preview checks what a run checks, and opens nothing
    if mode.is_preview() {
        outln!(
            "[DRY RUN] Would open the editor '{}' for a message for each of:",
            escape(&editor)
        )?;
        list_staged(repos)?;
        return Ok(());
    }

    // Create temp file with commit template
    let mut template = String::new();
    template.push_str("# Meta Multi-Commit\n");
    template.push_str("# Each section represents one repository.\n");
    template.push_str("# Edit the message below each header.\n");
    template.push_str("# Delete a section entirely or leave message empty to skip that repo.\n");
    template.push_str("#\n\n");

    for repo in repos {
        template.push_str(&format!("========== {} ==========\n", repo.name));
        let file_count = repo.files.len();
        let file_list = repo.files.join(", ");
        template.push_str(&format!("# {file_count} file(s) staged: {file_list}\n"));
        template.push('\n');
        template.push_str("# Enter commit message above this line\n\n");
    }

    // Write to a new temp file (created exclusively, mode 0600, removed on
    // drop) in the plugin's private data dir meta made, never in a shared
    // directory like /tmp; named absolutely, as the editor runs in the repo
    let dir = std::path::absolute(data_dir)?;
    let mut file = tempfile::Builder::new()
        .prefix("META_COMMIT_EDITMSG-")
        .tempfile_in(dir)?;
    file.write_all(template.as_bytes())?;
    file.flush()?;
    // Close it (some editors on Windows cannot replace an open file); the
    // path is still removed on drop
    let temp_file = file.into_temp_path();

    if let Some(terminal) = terminal {
        let status = editor_command(first, &editor, &temp_file)
            .stdin(terminal.input)
            .stdout(terminal.output)
            .stderr(terminal.errors)
            .status()
            .map_err(|e| anyhow::anyhow!("Cannot start the editor '{}': {e}", escape(&editor)))?;
        if !status.success() {
            anyhow::bail!(
                "The editor '{}' failed ({status}); nothing was committed",
                escape(&editor)
            );
        }
    }

    // Read and parse the edited file
    let content = std::fs::read_to_string(&temp_file)?;
    let commits = parse_multi_commit_file(&content);

    drop(temp_file);

    if commits.is_empty() {
        outln!("No commits to make (all messages were empty or deleted).")?;
        return Ok(());
    }
    commit_each(repos, &commits)
}

/// Commit each of `commits` (repo name, message) in its repo of `repos`;
/// an error if any failed, after trying all.
fn commit_each(repos: &[Staged], commits: &[(String, String)]) -> anyhow::Result<()> {
    let mut succeeded = 0;
    let mut failed = 0;

    for (repo_name, message) in commits {
        // a section commits only in a repo it was written for, never in
        // a folder its (edited) header happens to name
        let Some(repo) = repos.iter().find(|repo| repo.name == *repo_name) else {
            outln!(
                "{} No repo with staged changes is named {}; the message was:\n{}",
                "✗".red(),
                escape(repo_name).to_string().bold(),
                escape_lines(message)
            )?;
            failed += 1;
            continue;
        };

        outln!(
            "{} Committing {}...",
            "→".cyan(),
            escape(repo_name).to_string().bold()
        )?;

        let status = meta_core::git::git(&repo.dir)
            .args(["commit", "-m", message])
            .status();

        match status {
            Ok(s) if s.success() => {
                outln!(
                    "  {} {}",
                    "✓".green(),
                    escape(message.lines().next().unwrap_or(""))
                )?;
                succeeded += 1;
            }
            _ => {
                // the message is in no file any more: show it, to reuse
                outln!(
                    "  {} Failed to commit; the message was:\n{}",
                    "✗".red(),
                    escape_lines(message)
                )?;
                failed += 1;
            }
        }
    }

    outln!()?;
    if failed > 0 {
        anyhow::bail!("Committed {succeeded} repo(s), {failed} failed");
    }
    outln!("Committed {} repo(s)", succeeded.to_string().green())?;

    Ok(())
}

/// `editor` (as `git var GIT_EDITOR` gives it in `repo`) on `file`, run by
/// git as a shell alias: through git's shell or directly by git's own rule,
/// with git's environment (Git for Windows finds the editors it ships, such
/// as `nano.exe`, only on its own PATH), in the repo's top folder.
fn editor_command(repo: &Path, editor: &str, file: &Path) -> Command {
    let mut command = meta_core::git::git(repo);
    command
        .arg("-c")
        .arg(format!("alias.meta-commit-editor=!{editor}"))
        .arg("meta-commit-editor")
        .arg(file);
    command
}

/// The terminal meta runs in, for an editor: meta's own standard input is
/// the plugin request and its output is read by meta, so neither is one.
struct Terminal {
    input: std::fs::File,
    output: std::fs::File,
    errors: std::fs::File,
}

impl Terminal {
    /// The terminal, or why there is none to use: without one (as for an
    /// agent or a job), or in its background (as under `timeout`), an
    /// editor would wait for input that never comes.
    fn open() -> Result<Self, String> {
        let open = |path: &str| {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .map_err(|e| format!("there is none ({e})"))
        };
        #[cfg(windows)]
        let (input, output) = (open("CONIN$")?, open("CONOUT$")?);
        #[cfg(unix)]
        let (input, output) = {
            let tty = open("/dev/tty")?;
            let foreground = rustix::termios::tcgetpgrp(&tty)
                .map_err(|e| format!("its foreground can't be read ({e})"))?;
            if foreground != rustix::process::getpgrp() {
                return Err("the terminal's foreground is another program".to_string());
            }
            (tty.try_clone().map_err(|e| e.to_string())?, tty)
        };
        Ok(Self {
            errors: output.try_clone().map_err(|e| e.to_string())?,
            input,
            output,
        })
    }
}

/// Parse the multi-commit file content: each section's repo and message,
/// in order, for the sections whose message isn't empty
pub(crate) fn parse_multi_commit_file(content: &str) -> Vec<(String, String)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    for line in content.lines() {
        if line.starts_with("==========") && line.ends_with("==========") {
            let repo = line.trim_matches('=').trim();
            sections.push((repo.to_string(), String::new()));
        } else if let Some((_, message)) = sections.last_mut().filter(|_| !line.starts_with('#')) {
            // a line of the section's message; a comment is no part of it
            message.push_str(line);
            message.push('\n');
        }
    }
    sections
        .into_iter()
        .filter_map(|(repo, message)| {
            let message = message.trim();
            (!message.is_empty()).then(|| (repo, message.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use meta_plugin::{Answer, Project};
    use std::path::PathBuf;

    /// A commit request for the projects at `dirs` (each named by its
    /// folder's name), from the first.
    fn request(dirs: &[PathBuf]) -> Request {
        let mut request = Request::new("git commit", dirs.first().cloned().unwrap_or_default());
        request.projects = dirs
            .iter()
            .map(|dir| Project {
                name: dir.file_name().unwrap().to_string_lossy().into_owned(),
                path: dir.file_name().unwrap().to_string_lossy().into_owned(),
                abs: dir.clone(),
                present: dir.exists(),
                ..Project::default()
            })
            .collect();
        request
    }

    fn says_nothing_staged(result: anyhow::Result<Response>) -> bool {
        matches!(result.unwrap().answer, Answer::Message(message) if message.contains("No staged changes"))
    }

    #[test]
    fn commit_passes_over_folders_with_nothing_of_gits_but_reports_a_broken_one() {
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        sandbox.git(&dir, &["init", "-q"]);
        let args = || CommitArgs {
            edit: false,
            message: Some("x".to_string()),
        };
        let in_dir = |name: &str| request(&[dir.join(name)]);
        let result = execute_git_commit(args(), &in_dir("missing"));
        assert!(says_nothing_staged(result));

        // nothing git's in it: nothing staged there
        std::fs::create_dir(dir.join("plain")).unwrap();
        let result = execute_git_commit(args(), &in_dir("plain"));
        assert!(says_nothing_staged(result));
        // a .git git can't use is named, never taken for "nothing staged"
        std::fs::create_dir_all(dir.join("broken/.git")).unwrap();
        let result = execute_git_commit(args(), &in_dir("broken"));
        let error = result.expect_err("a broken repository must fail");
        let error = error.to_string();
        assert!(
            error.contains("broken") && error.contains("not a git repository"),
            "{error}"
        );
    }

    #[test]
    fn a_workspace_root_that_is_no_repository_is_passed_over() {
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        // whatever the config is called (`meta -c meta.yml`)
        let root = dir;
        std::fs::write(root.join("meta.yml"), "projects:\n  p: u\n").unwrap();
        let project = root.join("p");
        std::fs::create_dir(&project).unwrap();
        sandbox.git(&project, &["init", "-q"]);
        std::fs::write(project.join("new"), "x").unwrap();
        sandbox.git(&project, &["add", "new"]);
        // from the root, and from inside the project
        for cwd in [&root, &project] {
            let mut request = request(&[root.clone(), project.clone()]);
            request.cwd = cwd.clone();
            request.meta.dry_run = true;
            let args = CommitArgs {
                edit: false,
                message: Some("save".to_string()),
            };
            let result = execute_git_commit(args, &request);
            let steps = match result.unwrap().answer {
                Answer::Plan(plan) => plan.steps,
                _ => Vec::new(),
            };
            assert_eq!(steps.len(), 1, "{cwd:?}");
            assert_eq!(steps[0].dir, project, "{cwd:?}");
        }
    }

    // macOS and Windows file systems refuse a name that isn't UTF-8
    #[cfg(target_os = "linux")]
    #[test]
    fn a_project_folder_that_is_not_utf8_is_committed_in_as_named() {
        use std::os::unix::ffi::OsStrExt;

        let sandbox = meta_plugin::testing::isolated!();
        let project = sandbox
            .work()
            .join(std::ffi::OsStr::from_bytes(b"app-\xff"));
        std::fs::create_dir(&project).unwrap();
        sandbox.git(&project, &["init", "-q"]);
        std::fs::write(project.join("new"), "x").unwrap();
        sandbox.git(&project, &["add", "new"]);
        let args = CommitArgs {
            edit: false,
            message: Some("save".to_string()),
        };
        let result = execute_git_commit(args, &request(std::slice::from_ref(&project)));
        let steps = match result.unwrap().answer {
            Answer::Plan(plan) => plan.steps,
            _ => Vec::new(),
        };
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].dir, project);
    }

    #[test]
    fn a_staged_file_read_git_fails_is_an_error_not_nothing_staged() {
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        let repo = dir.join("broken");
        std::fs::create_dir(&repo).unwrap();
        sandbox.git(&repo, &["init", "-q"]);
        std::fs::write(repo.join(".git/index"), "not an index").unwrap();
        let args = CommitArgs {
            edit: false,
            message: Some("x".to_string()),
        };
        let result = execute_git_commit(args, &request(&[repo]));
        let error = format!("{:#}", result.expect_err("git failing is no empty answer"));
        assert!(
            error.contains("diff --cached --name-only") && error.contains("broken"),
            "{error}"
        );
    }

    #[test]
    fn a_failed_commit_fails_the_command_after_the_others() {
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        // Each its own repository, so git never finds one around the
        // temporary directory; "a" has nothing staged, so its commit fails
        let repo = |name: &str| {
            let path = dir.join(name);
            std::fs::create_dir(&path).unwrap();
            sandbox.git(&path, &["init", "-q"]);

            Staged {
                name: name.to_string(),
                dir: path,
                files: vec![],
            }
        };
        let repos = [repo("a"), repo("b")];
        std::fs::write(dir.join("b/file"), "x").unwrap();
        sandbox.git(&dir.join("b"), &["add", "file"]);
        let commits = [
            ("a".to_string(), "one".to_string()),
            ("b".to_string(), "two".to_string()),
        ];
        let err = commit_each(&repos, &commits).unwrap_err();
        assert_eq!(err.to_string(), "Committed 1 repo(s), 1 failed");
        let subject = sandbox.git(&dir.join("b"), &["log", "-1", "--format=%s"]);
        assert_eq!(subject.trim(), "two");
    }

    #[test]
    fn a_section_naming_no_staged_repo_commits_nowhere() {
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        let api = sandbox.repo(&dir.join("api"));
        // a repo with staged changes that wasn't selected
        let web = sandbox.repo(&dir.join("web"));
        std::fs::write(web.join("file"), "x").unwrap();
        sandbox.git(&web, &["add", "file"]);
        let repos = [Staged {
            name: "api".to_string(),
            dir: api,
            files: vec![],
        }];
        // a header edited to name another folder, by path
        let commits = [(web.display().to_string(), "taken".to_string())];
        let err = commit_each(&repos, &commits).unwrap_err();
        assert_eq!(err.to_string(), "Committed 0 repo(s), 1 failed");
        let subject = sandbox.git(&web, &["log", "-1", "--format=%s"]);
        assert_eq!(subject.trim(), "init");
    }
}
