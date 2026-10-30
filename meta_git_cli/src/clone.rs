use crate::clone_worker::clone_with_queue;
use crate::ssh_setup;
use console::style;
use indicatif::MultiProgress;
use meta_core::config;
use meta_git_lib::clone_queue::CloneQueue;
use meta_plugin_protocol::{CommandResult, PluginRequestOptions};
use std::process::Command;
use std::sync::Arc;

pub(crate) fn execute_git_clone(
    args: &[String],
    options: &PluginRequestOptions,
    cwd: &std::path::Path,
) -> anyhow::Result<CommandResult> {
    let dry_run = options.dry_run;

    // Default options - limit to 4 concurrent clones to avoid SSH multiplexing issues
    // Start with --recursive from CLI options (passed via PluginRequestOptions)
    let mut recursive = options.recursive;
    let mut parallel = 4_usize;
    let mut depth: Option<String> = options.depth.map(|d| d.to_string());
    let mut meta_depth: Option<usize> = None; // Limit recursion depth for nested .meta.yaml files

    let mut url = String::new();
    let mut dir_arg: Option<String> = None;
    let mut idx = 0;
    let mut git_clone_args: Vec<String> = Vec::new();

    while idx < args.len() {
        match args[idx].as_str() {
            "--recursive" | "-r" => {
                recursive = true;
                idx += 1;
            }
            "--meta-depth" => {
                if idx + 1 < args.len() {
                    meta_depth = args[idx + 1].parse().ok();
                    idx += 2;
                } else {
                    idx += 1;
                }
            }
            "--parallel" => {
                if idx + 1 < args.len() {
                    parallel = args[idx + 1].parse().unwrap_or(4);
                    idx += 2;
                } else {
                    idx += 1;
                }
            }
            "--depth" => {
                if idx + 1 < args.len() {
                    let d = args[idx + 1].clone();
                    depth = Some(d.clone());
                    git_clone_args.push("--depth".to_string());
                    git_clone_args.push(d);
                    idx += 2;
                } else {
                    idx += 1;
                }
            }
            s if s.starts_with('-') => {
                idx += 1; // skip unknown option
            }
            s => {
                if url.is_empty() {
                    url = s.to_string();
                } else if dir_arg.is_none() {
                    dir_arg = Some(s.to_string());
                }
                idx += 1;
            }
        }
    }

    if url.is_empty() {
        return Ok(CommandResult::Error(
            "No repository URL provided".to_string(),
        ));
    }

    // If depth was set from options but not added to git_clone_args yet, add it now
    if let Some(ref d) = depth {
        if !git_clone_args.contains(&"--depth".to_string()) {
            git_clone_args.push("--depth".to_string());
            git_clone_args.push(d.clone());
        }
    }

    // Derive directory name
    let clone_dir = if let Some(ref dir) = dir_arg {
        dir.clone()
    } else {
        url.trim_end_matches(".git")
            .rsplit('/')
            .next()
            .unwrap_or("meta")
            .to_string()
    };

    // Build the git clone command string for display/dry-run
    let mut clone_cmd_str = "git clone".to_string();
    for arg in &git_clone_args {
        clone_cmd_str.push(' ');
        clone_cmd_str.push_str(arg);
    }
    clone_cmd_str.push(' ');
    clone_cmd_str.push_str(&url);
    clone_cmd_str.push(' ');
    clone_cmd_str.push_str(&clone_dir);

    if dry_run {
        // Output what we know - just the meta repo clone command
        // (Child repos are in .meta.yaml file which hasn't been cloned yet)
        println!("{} Would clone meta repository:", style("[DRY RUN]").cyan());
        println!("  {}", meta_core::display::escape(&clone_cmd_str));
        return Ok(CommandResult::Message(String::new()));
    }

    // Set up SSH ControlMaster for the meta repo's host before cloning
    let mut ssh_cmd = match ssh_setup::establish_ssh_masters(&[url.as_str()]) {
        ssh_setup::SshMasters::OurSockets(dir) => Some(ssh_setup::git_ssh_command(&dir)),
        ssh_setup::SshMasters::UserManaged | ssh_setup::SshMasters::Failed => None,
    };

    println!(
        "Cloning meta repository: {}",
        meta_core::display::escape(&url)
    );
    let mut clone_cmd = Command::new("git");
    clone_cmd.arg("clone").args(&git_clone_args).arg(&url);
    if let Some(ref dir) = dir_arg {
        clone_cmd.arg(dir);
    }
    if let Some(ref ssh) = ssh_cmd {
        clone_cmd.env("GIT_SSH_COMMAND", ssh);
    }
    clone_cmd.current_dir(cwd);
    let status = clone_cmd.status()?;
    if !status.success() {
        return Ok(CommandResult::Error(
            "Failed to clone meta repository".to_string(),
        ));
    }

    // Parse meta config inside cloned repo
    let clone_dir_path = cwd.join(&clone_dir);
    if config::find_meta_config_in(&clone_dir_path)?.is_none() {
        return Ok(CommandResult::Message(
            "No .meta.yaml found in cloned repository".to_string(),
        ));
    }

    // Create the clone queue with depth settings
    // For non-recursive mode, set meta_depth to 0 (only first level)
    let effective_meta_depth = if recursive { meta_depth } else { Some(0) };
    let queue = Arc::new(CloneQueue::new(depth.clone(), effective_meta_depth));

    // Seed the queue with first-level children
    let initial_count = queue.push_from_meta(&clone_dir_path, 0)?;

    if initial_count == 0 {
        return Ok(CommandResult::Message(
            "No child repositories to clone".to_string(),
        ));
    }

    // Establish SSH masters for any additional hosts in the queue
    let queue_urls = queue.peek_urls();
    if !queue_urls.is_empty() {
        let url_refs: Vec<&str> = queue_urls.iter().map(|s| s.as_str()).collect();
        match ssh_setup::establish_ssh_masters(&url_refs) {
            ssh_setup::SshMasters::OurSockets(sockets_dir) => {
                ssh_cmd = Some(ssh_setup::git_ssh_command(&sockets_dir));
            }
            ssh_setup::SshMasters::UserManaged => {} // parallel OK, no override needed
            ssh_setup::SshMasters::Failed if ssh_cmd.is_none() => {
                log::warn!("SSH multiplexing setup failed, falling back to serial cloning");
                parallel = 1;
            }
            ssh_setup::SshMasters::Failed => {} // initial host works
        }
    }

    println!(
        "Cloning {} child repositories{}",
        initial_count,
        if recursive { " (recursive mode)" } else { "" }
    );

    let mp = MultiProgress::new();

    // Use the queue-based cloning system
    clone_with_queue(Arc::clone(&queue), parallel, &mp, ssh_cmd.as_deref())?;
    if let Some(error) = crate::clone_worker::report_failures(&queue) {
        return Ok(CommandResult::Error(error));
    }

    let (completed, total) = queue.get_counts();
    if total > initial_count {
        println!(
            "Meta-repo clone completed ({} repos cloned, {} discovered via nested .meta.yaml files)",
            completed,
            total - initial_count
        );
    } else {
        println!("Meta-repo clone completed ({completed} repos cloned)");
    }

    Ok(CommandResult::Message(String::new()))
}
