//! meta-rust library
//!
//! Runs any cargo command across the Rust repos of a meta workspace.

pub use meta_plugin_protocol::{CommandResult, PlannedCommand, PluginRequestOptions};
use std::path::Path;

/// Plan `cargo <args>` in every selected repo that has a `Cargo.toml`.
///
/// `projects` is the repo list from meta_cli (it includes the meta root);
/// when it is empty the plan runs in `cwd` only. A Cargo workspace that spans
/// repos runs in each of them; plain `cargo` in its root runs it once.
pub fn execute_command(
    args: &[String],
    options: &PluginRequestOptions,
    projects: &[String],
    cwd: &Path,
) -> CommandResult {
    // `meta cargo` and any `--help`/`-h` show this plugin's help instead of
    // running cargo once per repo. meta_cli sends help requests without the
    // repo list, so cargo's own help could not reach each repo anyway.
    if args.is_empty() || meta_plugin_protocol::has_help_token("", args) {
        return CommandResult::ShowHelp(None);
    }

    let dirs: Vec<String> = if projects.is_empty() {
        vec![".".to_string()]
    } else {
        projects.to_vec()
    };
    let (rust_dirs, other_dirs): (Vec<String>, Vec<String>) = dirs
        .into_iter()
        .partition(|dir| cwd.join(dir).join("Cargo.toml").is_file());

    if !options.silent && !other_dirs.is_empty() {
        // meta_cli sends only the selected repos, so name them from the cwd
        let names: Vec<String> = other_dirs.iter().map(|dir| display(dir, cwd)).collect();
        eprintln!("meta-rust: skipping {} (no Cargo.toml)", names.join(", "));
    }

    if rust_dirs.is_empty() {
        return CommandResult::Message("No Rust projects found (no Cargo.toml files)".to_string());
    }

    let commands = rust_dirs
        .into_iter()
        .map(|dir| PlannedCommand {
            dir,
            argv: std::iter::once("cargo".to_string())
                .chain(args.iter().cloned())
                .collect(),
            env: None,
        })
        .collect();
    CommandResult::Plan(commands, Some(options.parallel))
}

fn display(dir: &str, root: &Path) -> String {
    match root.join(dir).strip_prefix(root) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => rel.display().to_string(),
        Err(_) => dir.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &Path, file: &str, contents: &str) {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// The planned commands as `dir: argv`, or `None` for any other result.
    fn plan(
        args: &[&str],
        projects: &[&Path],
        options: &PluginRequestOptions,
    ) -> Option<Vec<String>> {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let projects: Vec<String> = projects.iter().map(|p| p.display().to_string()).collect();
        match execute_command(&args, options, &projects, Path::new(".")) {
            CommandResult::Plan(commands, _) => Some(
                commands
                    .into_iter()
                    .map(|c| format!("{}: {:?}", c.dir, c.argv))
                    .collect(),
            ),
            _ => None,
        }
    }

    #[test]
    fn any_cargo_command_runs_in_rust_repos_only() {
        let temp = TempDir::new().unwrap();
        let (root, rust, docs) = (
            temp.path(),
            temp.path().join("rust"),
            temp.path().join("docs"),
        );
        write(&rust, "Cargo.toml", "[package]\nname = \"rust\"\n");
        write(&docs, "README.md", "");

        let args = ["fmt", "--check", "--", "--config", "a b"];
        let commands = plan(&args, &[root, &rust, &docs], &Default::default()).unwrap();

        let expected = format!(
            "{}: {:?}",
            rust.display(),
            ["cargo", "fmt", "--check", "--", "--config", "a b"]
        );
        assert_eq!(commands, [expected]);
    }

    #[test]
    fn workspace_members_run_in_their_own_repos() {
        let temp = TempDir::new().unwrap();
        let (root, member) = (temp.path(), temp.path().join("member"));
        write(root, "Cargo.toml", "[workspace]\nmembers = [\"member\"]\n");
        write(&member, "Cargo.toml", "[package]\nname = \"member\"\n");

        // meta_cli sends only the repos --include/--exclude select
        let all = plan(&["run"], &[root, &member], &Default::default()).unwrap();
        let only_member = plan(&["run"], &[&member], &Default::default()).unwrap();

        let run_in = |dir: &Path| format!("{}: {:?}", dir.display(), ["cargo", "run"]);
        assert_eq!(all, [run_in(root), run_in(&member)]);
        assert_eq!(only_member, [run_in(&member)]);
    }

    #[test]
    fn help_after_double_dash_goes_to_cargo() {
        let temp = TempDir::new().unwrap();
        write(temp.path(), "Cargo.toml", "[package]\nname = \"x\"\n");
        let commands = plan(
            &["test", "--", "--help"],
            &[temp.path()],
            &Default::default(),
        )
        .unwrap();
        let expected = format!(
            "{}: {:?}",
            temp.path().display(),
            ["cargo", "test", "--", "--help"]
        );
        assert_eq!(commands, [expected]);
    }

    #[test]
    fn bare_or_help_shows_plugin_help() {
        for args in [&[][..], &["test", "--help"][..]] {
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            let result = execute_command(&args, &Default::default(), &[], Path::new("."));
            assert!(matches!(result, CommandResult::ShowHelp(None)));
        }
    }
}
