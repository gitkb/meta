//! meta-cargo library
//!
//! Runs any cargo command across the Rust repos of a meta workspace.

use meta_plugin::{Ctx, Plan, Project, Request, Response, Skipped, Step, Warning};

/// Plan `cargo <args>` in every selected repo that has a `Cargo.toml`. A
/// repo that isn't cloned is a warning (with `--strict`, an error), the
/// others are skipped, and with none left the command is done. A Cargo
/// workspace that spans repos runs in each of them; plain `cargo` in its
/// root runs it once.
pub fn execute_command(ctx: &Ctx, request: &Request) -> meta_plugin::Result<Response> {
    let mut cloned = Vec::new();
    for project in &request.projects {
        if project.present {
            cloned.push(project);
        } else {
            ctx.warn_or_bail(Warning::not_cloned(&project.path))?;
        }
    }
    let (rust, other): (Vec<&Project>, Vec<&Project>) = cloned
        .into_iter()
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
        return Ok(
            Response::message("No Rust projects found (no Cargo.toml files)").skipping(skipped),
        );
    }
    let argv: Vec<String> = std::iter::once("cargo".to_string())
        .chain(request.args.iter().cloned())
        .collect();
    let steps = rust
        .into_iter()
        .map(|project| Step::new(Some(&project.path), &project.abs, argv.clone()))
        .collect();
    Ok(Response::plan(Plan {
        steps,
        ..Plan::default()
    })
    .skipping(skipped))
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

    fn project(dir: &Path) -> Project {
        Project {
            name: dir.display().to_string(),
            path: dir.display().to_string(),
            abs: dir.to_path_buf(),
            present: dir.exists(),
            ..Project::default()
        }
    }

    fn request(args: &[&str], projects: &[&Path]) -> Request {
        let mut request = Request::new("cargo", ".");
        request.args = args.iter().map(|a| a.to_string()).collect();
        request.projects = projects.iter().map(|dir| project(dir)).collect();
        request
    }

    /// The planned steps as `dir: argv`, or `None` for any other answer.
    fn plan(args: &[&str], projects: &[&Path]) -> Option<Vec<String>> {
        let ctx = Ctx::new(meta_plugin::Meta::default());
        match execute_command(&ctx, &request(args, projects))
            .unwrap()
            .answer
        {
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
    fn a_repo_not_cloned_is_a_warning_and_with_strict_an_error() {
        let temp = TempDir::new().unwrap();
        let (rust, docs, gone) = (
            temp.path().join("rust"),
            temp.path().join("docs"),
            temp.path().join("gone"),
        );
        write(&rust, "Cargo.toml", "[package]\nname = \"rust\"\n");
        write(&docs, "README.md", "");
        let request = request(&["build"], &[&rust, &docs, &gone]);

        let ctx = Ctx::new(meta_plugin::Meta::default());
        let response = ctx.finish(execute_command(&ctx, &request).unwrap());
        let gone = gone.display().to_string();
        assert_eq!(response.warnings, [Warning::not_cloned(&gone)]);
        let skipped: Vec<_> = response.skipped.iter().map(|s| &s.reason).collect();
        assert_eq!(skipped, ["no Cargo.toml"]);
        assert!(
            matches!(&response.answer, meta_plugin::Answer::Plan(plan) if plan.steps.len() == 1)
        );

        let strict = Ctx::new(meta_plugin::Meta {
            strict: true,
            ..meta_plugin::Meta::default()
        });
        let error = execute_command(&strict, &request).unwrap_err().to_string();
        assert!(error.contains(&format!("{gone} is not cloned")), "{error}");
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
