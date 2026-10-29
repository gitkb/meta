use anyhow::Result;
use indicatif::ProgressBar;
use meta_core::display::escape_path;
use std::path::Path;
use std::process::{Command, Stdio};
pub mod clone_queue;
pub mod missing;
pub mod ssh_multiplexing;
pub mod worktree;
use console::style;
pub use missing::print_missing_repo;
pub use ssh_multiplexing::{
    ensure_ssh_sockets_dir, extract_ssh_host, get_remote_url, is_ssh_rate_limit_error,
    normalize_git_url, ssh_sockets_dir, urls_match,
};

/// Clone a git repository into the target directory, with progress bar.
pub fn clone_repo_with_progress(
    url: &str,
    target_dir: &Path,
    pb: Option<&ProgressBar>,
) -> Result<()> {
    if target_dir.exists() {
        if let Some(pb) = pb {
            pb.finish_with_message(format!(
                "{}: already exists, skipping",
                escape_path(target_dir)
            ));
        } else {
            println!("{}: already exists, skipping", escape_path(target_dir));
        }
        return Ok(());
    }
    if let Some(pb) = pb {
        pb.set_message(format!("Cloning {url}"));
    } else {
        println!("Cloning {} into {}", url, escape_path(target_dir));
    }
    let mut child = Command::new("git")
        .arg("clone")
        .arg(url)
        .arg(target_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let status = child.wait()?;
    if let Some(pb) = pb {
        if status.success() {
            pb.finish_with_message(format!("{} ✓", style(escape_path(target_dir)).green()));
        } else {
            pb.finish_with_message(format!(
                "Failed to clone {} into {}",
                url,
                escape_path(target_dir)
            ));
        }
    } else if status.success() {
        println!("{} ✓", style(escape_path(target_dir)).green());
    } else {
        println!("Failed to clone {} into {}", url, escape_path(target_dir));
    }
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("Failed to clone {} into {}", url, escape_path(target_dir))
    }
}
