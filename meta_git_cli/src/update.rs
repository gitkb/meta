use crate::clone_worker::clone_with_queue;
use crate::ssh_setup;
use console::style;
use indicatif::MultiProgress;
use meta_core::config;
use meta_core::display::{escape, escape_path};
use meta_git_lib::clone_queue::CloneQueue;
use meta_plugin_protocol::CommandResult;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) fn execute_git_update(
    projects: &[String],
    dry_run: bool,
    cwd: &std::path::Path,
) -> anyhow::Result<CommandResult> {
    // Determine if we're in recursive mode (projects list provided by meta_cli)
    let recursive = !projects.is_empty();

    // Build list of directories to check for .meta.yaml files
    let dirs_to_check: Vec<PathBuf> = if recursive {
        // In recursive mode, check each directory that has a .meta.yaml file
        // A .meta.yaml that can't be examined is an error, not "no nested repos"
        let mut dirs = Vec::new();
        for p in projects {
            let path = if p == "." {
                cwd.to_path_buf()
            } else {
                cwd.join(p)
            };
            if config::find_meta_config_in(&path)?.is_some() {
                dirs.push(path);
            }
        }
        dirs
    } else {
        // Normal mode - just check current directory
        vec![cwd.to_path_buf()]
    };

    // Check for remote URL mismatches in all meta roots
    for dir in &dirs_to_check {
        crate::ssh::warn_remote_mismatches(dir);
    }

    // First pass: check for orphaned repos and warn user
    for dir in &dirs_to_check {
        let Some(meta_path) = config::find_meta_config_in(dir)? else {
            continue;
        };

        let (projects, _) = match config::parse_meta_config(&meta_path) {
            Ok(result) => result,
            Err(_) => continue,
        };

        // Check for orphaned repositories (exist locally but not in .meta.yaml)
        let config_projects: HashSet<String> = projects.iter().map(|p| p.path.clone()).collect();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    // Check if it's a git repo and not in config
                    if path.join(".git").exists()
                        && !name.starts_with('.')
                        && !config_projects.contains(name)
                    {
                        let relative_path = if dir.as_path() == cwd {
                            name.to_string()
                        } else {
                            dir.join(name).to_string_lossy().to_string()
                        };
                        let shown = escape(&relative_path);
                        eprintln!(
                            "{} {} exists locally but is not in .meta.yaml. To remove: rm -rf {}",
                            style("⚠").yellow(),
                            style(&shown).yellow().bold(),
                            shown
                        );
                    }
                }
            }
        }
    }

    // Create the clone queue - unlimited depth for recursive mode
    let meta_depth = if recursive { None } else { Some(0) };
    let queue = Arc::new(CloneQueue::new(None, meta_depth)); // No git depth for update

    // Seed the queue from all known .meta.yaml files
    for dir in &dirs_to_check {
        // Determine relative depth based on whether it's the cwd or nested
        let depth_level = if dir.as_path() == cwd { 0 } else { 1 };
        queue.push_from_meta(dir, depth_level)?;
    }

    let (_, initial_count) = queue.get_counts();

    if initial_count == 0 {
        println!("All repositories are already cloned.");
        return Ok(CommandResult::Message(String::new()));
    }

    if dry_run {
        println!(
            "{} Would clone {} missing repositories:",
            style("[DRY RUN]").cyan(),
            initial_count
        );
        let tasks = queue.drain_all();
        for task in tasks {
            println!(
                "  git clone {} {}",
                escape(&task.url),
                escape_path(&task.target_path)
            );
        }
        return Ok(CommandResult::Message(String::new()));
    }

    // Establish SSH multiplexing before parallel clones
    let queue_urls = queue.peek_urls();
    let mut parallel = 4_usize;
    let ssh_cmd = if !queue_urls.is_empty() {
        let url_refs: Vec<&str> = queue_urls.iter().map(|s| s.as_str()).collect();
        match ssh_setup::establish_ssh_masters(&url_refs) {
            ssh_setup::SshMasters::OurSockets(dir) => Some(ssh_setup::git_ssh_command(&dir)),
            ssh_setup::SshMasters::UserManaged => None, // parallel OK
            ssh_setup::SshMasters::Failed => {
                log::warn!("SSH multiplexing setup failed, falling back to serial cloning");
                parallel = 1;
                None
            }
        }
    } else {
        None
    };

    println!(
        "Cloning {} missing repositories{}",
        initial_count,
        if recursive { " (recursive mode)" } else { "" }
    );

    let mp = MultiProgress::new();

    clone_with_queue(Arc::clone(&queue), parallel, &mp, ssh_cmd.as_deref())?;
    if let Some(error) = crate::clone_worker::report_failures(&queue) {
        return Ok(CommandResult::Error(error));
    }

    let (completed, total) = queue.get_counts();
    if total > initial_count {
        println!(
            "Update completed ({} repos cloned, {} discovered via nested .meta.yaml files)",
            completed,
            total - initial_count
        );
    } else {
        println!("Update completed ({completed} repos cloned)");
    }

    Ok(CommandResult::Message(String::new()))
}
