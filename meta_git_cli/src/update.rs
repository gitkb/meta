use crate::clone_worker::clone_with_queue;
use crate::ssh;
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

    // The meta roots to update: in recursive mode (projects list provided
    // by meta_cli) each of its directories that has a .meta.yaml, else cwd.
    // A config that can't be read is an error, not "no nested repos".
    let dirs: Vec<PathBuf> = if recursive {
        projects
            .iter()
            .map(|p| {
                if p == "." {
                    cwd.to_path_buf()
                } else {
                    cwd.join(p)
                }
            })
            .collect()
    } else {
        vec![cwd.to_path_buf()]
    };
    let mut roots = Vec::new();
    for dir in dirs {
        roots.extend(config::Workspace::in_dir(&dir)?);
    }

    for workspace in &roots {
        crate::ssh::warn_remote_mismatches(workspace);
    }

    // Repos that exist locally but are not in .meta.yaml
    for workspace in &roots {
        let dir = &workspace.dir;
        let config_projects: HashSet<&str> =
            workspace.projects.iter().map(|p| p.path.as_str()).collect();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if path.is_dir()
                && matches!(meta_core::gitdir::common_dir(&path), Ok(Some(_)))
                && !name.starts_with('.')
                && !config_projects.contains(name)
            {
                let relative_path = if dir.as_path() == cwd {
                    name.to_string()
                } else {
                    dir.join(name).to_string_lossy().to_string()
                };
                eprintln!(
                    "{} {} exists locally but is not in .meta.yaml. To remove: rm -rf {}",
                    style("⚠").yellow(),
                    style(escape(&relative_path)).yellow().bold(),
                    escape(&meta_core::display::shell_quote(&relative_path))
                );
            }
        }
    }

    // Create the clone queue - unlimited depth for recursive mode
    let meta_depth = if recursive { None } else { Some(0) };
    let queue = Arc::new(CloneQueue::new(None, meta_depth)); // No git depth for update

    // Seed the queue from all known .meta.yaml files
    for workspace in &roots {
        let dir = &workspace.dir;
        // Determine relative depth based on whether it's the cwd or nested
        let depth_level = if dir.as_path() == cwd { 0 } else { 1 };
        queue.push_from_workspace(workspace, depth_level)?;
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
                task.url,
                escape_path(&task.target_path)
            );
        }
        return Ok(CommandResult::Message(String::new()));
    }

    // Establish SSH multiplexing before parallel clones
    let queue_urls = queue.peek_urls();
    let mut parallel = 4_usize;
    let ssh_cmd = if !queue_urls.is_empty() {
        match ssh::establish_ssh_masters(&queue_urls) {
            ssh::SshMasters::OurSockets(dir) => Some(ssh::git_ssh_command(&dir)),
            ssh::SshMasters::UserManaged => None, // parallel OK
            ssh::SshMasters::Failed => {
                ssh::warn("SSH multiplexing could not be set up: cloning one repository at a time");
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
