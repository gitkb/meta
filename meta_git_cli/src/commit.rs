use console::style;
use meta_core::config;
use meta_core::display::{escape, escape_lines, escape_path};
use meta_plugin_protocol::{CommandResult, PlannedCommand, PluginRequestOptions};
use std::path::Path;
use std::process::Command;

/// Commit what is staged, in every repo that has staged changes.
#[derive(clap::Parser)]
pub(crate) struct CommitArgs {
    /// Write each repo's message in your editor
    #[arg(short, long)]
    edit: bool,
    /// Use this message in every repo
    #[arg(short, long, allow_hyphen_values = true)]
    message: Option<String>,
}

/// Execute git commit with optional --edit flag for per-repo messages
pub(crate) fn execute_git_commit(
    args: CommitArgs,
    projects: &[String],
    options: &PluginRequestOptions,
    cwd: &std::path::Path,
) -> anyhow::Result<CommandResult> {
    let (use_editor, message) = (args.edit, args.message);

    // Get list of directories to check for staged changes
    let dirs_to_check: Vec<String> = if !projects.is_empty() {
        // Use projects from meta_cli (supports --recursive)
        projects.to_vec()
    } else {
        // Fall back to reading local meta config
        let Some(workspace) = config::Workspace::in_dir(cwd)? else {
            return Ok(CommandResult::Message(format!(
                "No .meta.yaml found in {}",
                escape_path(cwd)
            )));
        };
        let projects = workspace.projects;

        let mut dirs = vec![".".to_string()];
        dirs.extend(projects.iter().map(|p| p.path.clone()));
        dirs
    };

    // Find repos with staged changes
    let mut repos_with_changes: Vec<(String, String, Vec<String>)> = Vec::new();

    for dir in &dirs_to_check {
        let path = if dir == "." {
            cwd.to_path_buf()
        } else {
            cwd.join(dir)
        };

        let staged = staged_files(&path);
        if !staged.is_empty() {
            repos_with_changes.push((dir.clone(), path.to_string_lossy().to_string(), staged));
        }
    }

    if repos_with_changes.is_empty() {
        return Ok(CommandResult::Message(
            "No staged changes found in any repository.".to_string(),
        ));
    }

    if use_editor {
        if options.mode().is_preview() {
            println!("[DRY RUN] Would open the editor for a message for each of:");
            for (name, _path, files) in &repos_with_changes {
                println!("  {} ({} files)", style(escape(name)).cyan(), files.len());
            }
        } else {
            // Open editor for per-repo messages (interactive, cannot use ExecutionPlan)
            execute_editor_commit(&repos_with_changes)?;
        }
    } else if let Some(msg) = message {
        // Apply same message to all repos - use ExecutionPlan for proper dry-run support
        let commands: Vec<PlannedCommand> = repos_with_changes
            .iter()
            .map(|(name, path, _files)| {
                // For "." use "." as dir, otherwise use the path
                let dir = if name == "." {
                    ".".to_string()
                } else {
                    path.clone()
                };
                PlannedCommand {
                    dir,
                    argv: ["git", "commit", "-m", msg.as_str()]
                        .map(str::to_string)
                        .to_vec(),
                    env: None,
                }
            })
            .collect();

        return Ok(CommandResult::Plan(commands, Some(options.parallel)));
    } else {
        // No message provided, show what would be committed
        println!("Repositories with staged changes:");
        for (name, _path, files) in &repos_with_changes {
            println!("  {} ({} files)", style(escape(name)).cyan(), files.len());
        }
        println!();
        println!(
            "Use {} to create per-repo commit messages",
            style("--edit").yellow()
        );
        println!(
            "Use {} to apply the same message to all",
            style("-m \"message\"").yellow()
        );
    }

    Ok(CommandResult::Message(String::new()))
}

/// The staged files of the repo at `path` (none if git can't tell).
fn staged_files(path: &std::path::Path) -> Vec<String> {
    meta_core::git::query(path, ["diff", "--cached", "--name-only"])
        .map(|out| out.lines().map(String::from).collect())
        .unwrap_or_default()
}

/// Execute commit with editor for per-repo messages
fn execute_editor_commit(repos: &[(String, String, Vec<String>)]) -> anyhow::Result<()> {
    use std::io::Write;

    // The editor git would use, from the first repo's config; asked first,
    // so a missing one or a missing terminal fails before anything is written
    let Some((_, first, _)) = repos.first() else {
        return Ok(());
    };
    let first = Path::new(first);
    let Some(editor) = meta_core::git::query(first, ["var", "GIT_EDITOR"]) else {
        anyhow::bail!(
            "git has no editor to use: set GIT_EDITOR, core.editor, VISUAL or EDITOR, or use -m <message>"
        );
    };
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

    // Create temp file with commit template
    let mut template = String::new();
    template.push_str("# Meta Multi-Commit\n");
    template.push_str("# Each section represents one repository.\n");
    template.push_str("# Edit the message below each header.\n");
    template.push_str("# Delete a section entirely or leave message empty to skip that repo.\n");
    template.push_str("#\n\n");

    for (name, _path, files) in repos {
        template.push_str(&format!("========== {name} ==========\n"));
        let file_count = files.len();
        let file_list = files.join(", ");
        template.push_str(&format!("# {file_count} file(s) staged: {file_list}\n"));
        template.push('\n');
        template.push_str("# Enter commit message above this line\n\n");
    }

    // Write to a new temp file (created exclusively, mode 0600, removed on
    // drop) in meta's private data dir, never in a shared directory like /tmp;
    // named absolutely, as the editor runs in the repo
    let dir = std::path::absolute(meta_core::data_dir::data_subdir("tmp")?)?;
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
        println!("No commits to make (all messages were empty or deleted).");
        return Ok(());
    }
    commit_each(repos, &commits)
}

/// Commit each of `commits` (repo name, message) in its repo of `repos`;
/// an error if any failed, after trying all.
fn commit_each(
    repos: &[(String, String, Vec<String>)],
    commits: &[(String, String)],
) -> anyhow::Result<()> {
    let mut succeeded = 0;
    let mut failed = 0;

    for (repo_name, message) in commits {
        // Find the path for this repo
        let path = repos
            .iter()
            .find(|(name, _, _)| name == repo_name)
            .map(|(_, path, _)| path.as_str())
            .unwrap_or(repo_name);

        println!(
            "{} Committing {}...",
            style("→").cyan(),
            style(escape(repo_name)).bold()
        );

        let status = meta_core::git::git(Path::new(path))
            .args(["commit", "-m", message])
            .status();

        match status {
            Ok(s) if s.success() => {
                println!(
                    "  {} {}",
                    style("✓").green(),
                    escape(message.lines().next().unwrap_or(""))
                );
                succeeded += 1;
            }
            _ => {
                // the message is in no file any more: show it, to reuse
                println!(
                    "  {} Failed to commit; the message was:\n{}",
                    style("✗").red(),
                    escape_lines(message)
                );
                failed += 1;
            }
        }
    }

    println!();
    if failed > 0 {
        anyhow::bail!("Committed {succeeded} repo(s), {failed} failed");
    }
    println!("Committed {} repo(s)", style(succeeded).green());

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
                return Err("meta runs in its background".to_string());
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

/// Parse the multi-commit file content
pub(crate) fn parse_multi_commit_file(content: &str) -> Vec<(String, String)> {
    let mut commits = Vec::new();
    let mut current_repo: Option<String> = None;
    let mut current_message = String::new();

    for line in content.lines() {
        if line.starts_with("==========") && line.ends_with("==========") {
            // Save previous repo if it had a message
            if let Some(repo) = current_repo.take() {
                let msg = current_message.trim().to_string();
                if !msg.is_empty() {
                    commits.push((repo, msg));
                }
            }
            // Parse new repo name
            let repo = line
                .trim_start_matches('=')
                .trim_end_matches('=')
                .trim()
                .to_string();
            current_repo = Some(repo);
            current_message.clear();
        } else if !line.starts_with('#') && current_repo.is_some() {
            // Add non-comment lines to current message
            current_message.push_str(line);
            current_message.push('\n');
        }
    }

    // Don't forget the last repo
    if let Some(repo) = current_repo {
        let msg = current_message.trim().to_string();
        if !msg.is_empty() {
            commits.push((repo, msg));
        }
    }

    commits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_commit_fails_the_command_after_the_others() {
        let dir = tempfile::tempdir().unwrap();
        // Each its own repository, so git never finds one around the
        // temporary directory; "a" has nothing staged, so its commit fails
        let repo = |name: &str| {
            let path = dir.path().join(name);
            std::fs::create_dir(&path).unwrap();
            meta_core::git::run(&path, ["init", "-q"]).unwrap();
            meta_core::git::run(&path, ["config", "user.name", "Test"]).unwrap();
            meta_core::git::run(&path, ["config", "user.email", "t@example.invalid"]).unwrap();
            // whatever the developer's global config signs or hooks
            meta_core::git::run(&path, ["config", "commit.gpgsign", "false"]).unwrap();
            meta_core::git::run(&path, ["config", "core.hooksPath", "no-hooks"]).unwrap();
            (name.to_string(), path.display().to_string(), vec![])
        };
        let repos = [repo("a"), repo("b")];
        std::fs::write(dir.path().join("b/file"), "x").unwrap();
        meta_core::git::run(&dir.path().join("b"), ["add", "file"]).unwrap();
        let commits = [
            ("a".to_string(), "one".to_string()),
            ("b".to_string(), "two".to_string()),
        ];
        let err = commit_each(&repos, &commits).unwrap_err();
        assert_eq!(err.to_string(), "Committed 1 repo(s), 1 failed");
        let subject = meta_core::git::run(&dir.path().join("b"), ["log", "-1", "--format=%s"]);
        assert_eq!(subject.unwrap(), "two");
    }
}
