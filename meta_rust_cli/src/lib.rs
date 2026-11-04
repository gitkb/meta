//! meta-cargo library
//!
//! Runs any cargo command across the Rust repos of a meta workspace.

use meta_plugin::{Plan, Project, Request, Response, Skipped, Step};

/// Plan `cargo <args>` in every selected repo that has a `Cargo.toml`; the
/// others are skipped, and with none left the command is done. A Cargo
/// workspace that spans repos runs in each of them; plain `cargo` in its
/// root runs it once.
pub fn execute_command(request: &Request) -> Response {
    let (rust, other): (Vec<&Project>, Vec<&Project>) = request
        .projects
        .iter()
        .partition(|project| project.abs.join("Cargo.toml").is_file());
    let skipped = other
        .into_iter()
        .map(|project| Skipped {
            project: project.path.clone(),
            reason: "no Cargo.toml".to_string(),
            quiet: false,
        })
        .collect();
    if rust.is_empty() {
        return Response::message("No Rust projects found (no Cargo.toml files)").skipping(skipped);
    }
    let argv: Vec<String> = std::iter::once("cargo".to_string())
        .chain(request.args.iter().cloned())
        .collect();
    let steps = rust
        .into_iter()
        .map(|project| Step::new(Some(&project.path), &project.abs, argv.clone()))
        .collect();
    Response::plan(Plan {
        steps,
        ..Plan::default()
    })
    .skipping(skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    fn write(dir: &Path, file: &str, contents: &str) {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// The planned steps as `dir: argv`, or `None` for any other answer.
    fn plan(args: &[&str], projects: &[&Path]) -> Option<Vec<String>> {
        let mut request = Request::new("cargo", ".");
        request.args = args.iter().map(|a| a.to_string()).collect();
        request.projects = projects
            .iter()
            .map(|dir| Project {
                name: dir.display().to_string(),
                path: dir.display().to_string(),
                abs: dir.to_path_buf(),
                present: true,
                ..Project::default()
            })
            .collect();
        match execute_command(&request).answer {
            meta_plugin::Answer::Plan(plan) => Some(
                plan.steps
                    .into_iter()
                    .map(|s| format!("{}: {:?}", s.dir.display(), s.argv))
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
        let commands = plan(&args, &[root, &rust, &docs]).unwrap();

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
        let all = plan(&["run"], &[root, &member]).unwrap();
        let only_member = plan(&["run"], &[&member]).unwrap();

        let run_in = |dir: &Path| format!("{}: {:?}", dir.display(), ["cargo", "run"]);
        assert_eq!(all, [run_in(root), run_in(&member)]);
        assert_eq!(only_member, [run_in(&member)]);
    }

    #[test]
    fn help_after_double_dash_goes_to_cargo() {
        let temp = TempDir::new().unwrap();
        write(temp.path(), "Cargo.toml", "[package]\nname = \"x\"\n");
        let commands = plan(&["test", "--", "--help"], &[temp.path()]).unwrap();
        let expected = format!(
            "{}: {:?}",
            temp.path().display(),
            ["cargo", "test", "--", "--help"]
        );
        assert_eq!(commands, [expected]);
    }
}
