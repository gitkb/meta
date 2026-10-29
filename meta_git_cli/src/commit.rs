use console::style;
use meta_core::config;
use meta_core::display::{escape, escape_path};
use meta_plugin_protocol::{CommandResult, PlannedCommand, PluginRequestOptions};
use std::process::Command;

/// Execute git commit with optional --edit flag for per-repo messages
pub(crate) fn execute_git_commit(
    args: &[String],
    projects: &[String],
    options: &PluginRequestOptions,
    cwd: &std::path::Path,
) -> anyhow::Result<CommandResult> {
    // Parse arguments
    let mut use_editor = false;
    let mut message: Option<String> = None;
    let mut idx = 0;

    while idx < args.len() {
        match args[idx].as_str() {
            "--edit" | "-e" => {
                use_editor = true;
                idx += 1;
            }
            "-m" | "--message" => {
                if idx + 1 < args.len() {
                    message = Some(args[idx + 1].clone());
                    idx += 2;
                } else {
                    idx += 1;
                }
            }
            // Skip other args like "git", "commit"
            _ => idx += 1,
        }
    }

    // Get list of directories to check for staged changes
    let dirs_to_check: Vec<String> = if !projects.is_empty() {
        // Use projects from meta_cli (supports --recursive)
        projects.to_vec()
    } else {
        // Fall back to reading local meta config
        let Some(meta_path) = config::find_meta_config_in(cwd)? else {
            return Ok(CommandResult::Message(format!(
                "No .meta.yaml found in {}",
                escape_path(cwd)
            )));
        };
        let (projects, _) = config::parse_meta_config(&meta_path)?;

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

        if path.exists() && has_staged_changes(&path.to_string_lossy()) {
            let staged = get_staged_files(&path.to_string_lossy());
            repos_with_changes.push((dir.clone(), path.to_string_lossy().to_string(), staged));
        }
    }

    if repos_with_changes.is_empty() {
        return Ok(CommandResult::Message(
            "No staged changes found in any repository.".to_string(),
        ));
    }

    if use_editor {
        // Open editor for per-repo messages (interactive, cannot use ExecutionPlan)
        execute_editor_commit(&repos_with_changes)?;
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

/// Check if a repo has staged changes
fn has_staged_changes(path: &str) -> bool {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["diff", "--cached", "--quiet"])
        .status();

    match output {
        Ok(status) => !status.success(), // Non-zero exit means there are changes
        Err(_) => false,
    }
}

/// Get list of staged files in a repo
fn get_staged_files(path: &str) -> Vec<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["diff", "--cached", "--name-only"])
        .output();

    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(String::from)
            .collect(),
        _ => vec![],
    }
}

/// Execute commit with editor for per-repo messages
fn execute_editor_commit(repos: &[(String, String, Vec<String>)]) -> anyhow::Result<()> {
    use std::io::Write;

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
    // drop) in meta's private data dir, never in a shared directory like /tmp
    let mut file = tempfile::Builder::new()
        .prefix("META_COMMIT_EDITMSG-")
        .tempfile_in(meta_core::data_dir::data_subdir("tmp")?)?;
    file.write_all(template.as_bytes())?;
    file.flush()?;
    // Close it (some editors on Windows cannot replace an open file); the
    // path is still removed on drop
    let temp_file = file.into_temp_path();

    // Get editor from environment
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());

    // Open editor
    let status = Command::new(&editor).arg(&temp_file).status()?;

    if !status.success() {
        anyhow::bail!("Editor exited with non-zero status");
    }

    // Read and parse the edited file
    let content = std::fs::read_to_string(&temp_file)?;
    let commits = parse_multi_commit_file(&content);

    drop(temp_file);

    if commits.is_empty() {
        println!("No commits to make (all messages were empty or deleted).");
        return Ok(());
    }

    // Execute commits
    let mut succeeded = 0;
    let mut failed = 0;

    for (repo_name, message) in &commits {
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

        let status = Command::new("git")
            .arg("-C")
            .arg(path)
            .arg("commit")
            .arg("-m")
            .arg(message)
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
                println!("  {} Failed to commit", style("✗").red());
                failed += 1;
            }
        }
    }

    println!();
    if failed > 0 {
        println!(
            "Committed {} repo(s), {} failed",
            style(succeeded).green(),
            style(failed).red()
        );
    } else {
        println!("Committed {} repo(s)", style(succeeded).green());
    }

    Ok(())
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
