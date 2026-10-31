//! `meta_batch_execute`: a command in each project, and the rollback of an
//! atomic batch that failed.

use crate::tools::{BatchArgs, Reply, Scope};
use crate::McpServer;
use anyhow::{Context, Result};
use meta_core::git::git;
use std::process::Command;

impl McpServer {
    pub(crate) fn batch_execute(&self, args: BatchArgs) -> Result<Reply> {
        let Some((program, program_args)) = args.command.split_first() else {
            anyhow::bail!("'command' is empty: name the program to run");
        };
        let atomic = args.atomic == Some(true);
        let targets = self.workspace()?.select(&Scope::of(None, args.tag)?)?;

        // Atomic: every target must be a clean git repository before anything
        // runs, so a rollback discards only what the command changed.
        let mut before = Vec::new();
        if atomic {
            let mut refused = Vec::new();
            for project in &targets {
                let path = &project.dir;
                // The branch is empty for a detached HEAD.
                let head = self
                    .repo_git(path, &["rev-parse", "--verify", "HEAD"])
                    .and_then(|commit| {
                        let branch = self.repo_git(path, &["branch", "--show-current"])?;
                        Ok(((!branch.is_empty()).then_some(branch), commit))
                    });
                match (head, self.is_clean(path)) {
                    (Ok(head), Ok(true)) => before.push(head),
                    (Ok(_), Ok(false)) => refused.push(format!(
                        "{}: has uncommitted changes or untracked files",
                        project.name
                    )),
                    _ => refused.push(format!(
                        "{}: not the root of a git repository with a commit",
                        project.name
                    )),
                }
            }
            if !refused.is_empty() {
                anyhow::bail!(
                    "Atomic batch refused, nothing was run: {}",
                    refused.join("; ")
                );
            }
        }

        let mut results = Vec::new();
        let mut has_failure = false;

        for project in &targets {
            if !project.dir.exists() {
                results.push(serde_json::json!({
                    "project": project.name,
                    "success": false,
                    "error": "Path does not exist"
                }));
                has_failure = true;
                if atomic {
                    break;
                }
                continue;
            }

            // In each project's own repository, whatever a git hook around
            // this server set
            let mut command = Command::new(program);
            meta_core::git::clear_repo_env(&mut command)
                .args(program_args)
                .current_dir(&project.dir);
            match self.calls.output(&mut command) {
                Ok(out) => {
                    let success = out.status.success();
                    has_failure |= !success;
                    results.push(serde_json::json!({
                        "project": project.name,
                        "success": success,
                        "exit_code": out.status.code(),
                        "stdout": String::from_utf8_lossy(&out.stdout),
                        "stderr": String::from_utf8_lossy(&out.stderr)
                    }));
                }
                Err(e) => {
                    has_failure = true;
                    results.push(serde_json::json!({
                        "project": project.name,
                        "success": false,
                        "error": format!("Cannot run {program}: {e}")
                    }));
                }
            }

            // If atomic and failure, stop and rollback
            if atomic && has_failure {
                break;
            }
        }

        // Roll back only the projects the command reached: the first
        // `results.len()` targets. The ones after them are untouched. This
        // runs even when the call was cancelled: the rollback's git is not
        // stopped with it.
        let rollback = (atomic && has_failure).then(|| {
            let mut restored = Vec::new();
            let mut failed = Vec::new();
            for (project, (branch, commit)) in targets.iter().zip(&before).take(results.len()) {
                match self.restore_repo(&project.dir, branch.as_deref(), commit) {
                    Ok((stash, replaced)) => restored.push(serde_json::json!({
                        "project": project.name,
                        "stash": stash,
                        "replaced_commits": replaced
                    })),
                    Err(e) => failed.push(serde_json::json!({
                        "project": project.name,
                        "error": format!("{e:#}")
                    })),
                }
            }
            serde_json::json!({
                "status": if failed.is_empty() { "success" } else { "failed" },
                "restored": restored,
                "failed": failed
            })
        });

        // An atomic batch fails as a whole; otherwise only when nothing worked
        let all_failed = !results.is_empty() && results.iter().all(|r| r["success"] == false);
        Ok(Reply {
            is_error: if atomic { has_failure } else { all_failed },
            text: serde_json::to_string_pretty(&serde_json::json!({
                "command": args.command,
                "results": results,
                "has_failure": has_failure,
                "rollback": rollback
            }))?,
        })
    }

    /// Runs git on the repository whose root is `path` (its `.git` is
    /// named, so git never falls back to an enclosing repository, and it
    /// never recurses into submodules, which are repositories of their own)
    /// and returns the trimmed stdout, or an error with git's stderr.
    fn repo_git(&self, path: &std::path::Path, args: &[&str]) -> Result<String> {
        let mut git_dir = std::ffi::OsString::from("--git-dir=");
        git_dir.push(path.join(".git"));
        let mut work_tree = std::ffi::OsString::from("--work-tree=");
        work_tree.push(path);
        let output = git(path)
            .args(["-c", "submodule.recurse=false"])
            .arg(git_dir)
            .arg(work_tree)
            .args(args)
            .output()
            .with_context(|| {
                format!(
                    "Failed to run git {args:?} in {}",
                    meta_core::display::escape_path(path)
                )
            })?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(anyhow::anyhow!(
                "git {} failed in {}: {}",
                args.join(" "),
                meta_core::display::escape_path(path),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    /// Whether the repository at `path` has no uncommitted changes and no
    /// untracked files, whatever `status.showUntrackedFiles` says.
    fn is_clean(&self, path: &std::path::Path) -> Result<bool> {
        let status = self.repo_git(path, &["status", "--porcelain", "--untracked-files=normal"])?;
        Ok(status.is_empty())
    }

    /// Puts the repository at `path` back on `branch` (detached when `None`)
    /// at `commit`, recreating the branch if it is gone. Uncommitted changes
    /// and untracked files are first saved in a stash entry; ignored files are
    /// not. Returns that stash's commit id and the commits HEAD and the branch
    /// were on, other than `commit`, so all of them can be recovered.
    fn restore_repo(
        &self,
        path: &std::path::Path,
        branch: Option<&str>,
        commit: &str,
    ) -> Result<(Option<String>, Vec<String>)> {
        let commit = self.repo_git(
            path,
            &["rev-parse", "--verify", &format!("{commit}^{{commit}}")],
        )?;
        let mut replaced = vec![self.repo_git(path, &["rev-parse", "--verify", "HEAD"])?];
        if let Some(branch) = branch {
            // Empty when the branch is gone.
            let tip = format!("refs/heads/{branch}");
            replaced.push(self.repo_git(path, &["for-each-ref", "--format=%(objectname)", &tip])?);
        }
        replaced.retain(|replaced| !replaced.is_empty() && *replaced != commit);
        replaced.dedup();

        // `stash list` is empty, not an error, when there is no stash.
        let stash_head = || self.repo_git(path, &["stash", "list", "-n1", "--format=%H"]);
        let old_stash = stash_head()?;
        self.repo_git(
            path,
            &[
                "stash",
                "push",
                "--include-untracked",
                "--quiet",
                "-m",
                "meta-restore-backup",
            ],
        )?;
        let stash = stash_head()
            .context("uncommitted changes may be in a new stash entry, see `git stash list`")?;
        let stash = (stash != old_stash).then_some(stash);
        let checkout = match branch {
            Some(branch) => ["checkout", "--quiet", "-B", branch, &commit, "--"].to_vec(),
            None => ["checkout", "--quiet", "--detach", &commit, "--"].to_vec(),
        };
        self.repo_git(path, &checkout)
            .with_context(|| match &stash {
                Some(stash) => format!("uncommitted changes are saved in stash {stash}"),
                None => "there were no uncommitted changes to save".to_string(),
            })?;
        Ok((stash, replaced))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{call, git, init_repo, json, workspace};
    use serde_json::json;

    /// `meta_batch_execute`; a string `command` is a script, run with
    /// `sh -c` the way a user asks for a shell.
    fn batch(server: &McpServer, mut args: serde_json::Value) -> Result<serde_json::Value> {
        if let Some(script) = args["command"].as_str().map(str::to_string) {
            args["command"] = json!(["sh", "-c", script]);
        }
        Ok(serde_json::from_str(
            &call(server, "meta_batch_execute", args)?.text,
        )?)
    }

    #[test]
    fn test_batch_runs_its_argv_with_no_shell() {
        let (server, tmp) = workspace();
        let args = json!({"command": ["git", "rev-parse", "--sq-quote", "$(touch PWNED)", "a b"]});
        let result = json(call(&server, "meta_batch_execute", args));
        assert_eq!(
            result["results"][0]["stdout"], " '$(touch PWNED)' 'a b'\n",
            "{result}"
        );
        for dir in [
            tmp.path().to_path_buf(),
            tmp.path().join("a"),
            tmp.path().join("b"),
        ] {
            assert!(!dir.join("PWNED").exists());
        }
    }

    #[test]
    fn test_atomic_batch_never_touches_projects_it_does_not_target() {
        let (server, tmp) = workspace();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        std::fs::write(b.join("README"), "my edit").unwrap();
        let head = git(&a, &["rev-parse", "HEAD"]);

        let result = batch(
            &server,
            json!({
                "command": "echo changed > README && git -c user.name=t -c user.email=t@example.com commit -qam c && touch new && false",
                "tag": "x",
                "atomic": true
            }),
        )
        .unwrap();

        assert_eq!(result["rollback"]["status"], "success", "{result}");
        let stash = result["rollback"]["restored"][0]["stash"].as_str().unwrap();
        assert_eq!(git(&a, &["show", &format!("{stash}^3:new")]), "");
        let replaced = &result["rollback"]["restored"][0]["replaced_commits"];
        assert_eq!(replaced.as_array().unwrap().len(), 1, "{replaced}");
        let replaced = replaced[0].as_str().unwrap();
        assert_eq!(
            git(&a, &["show", &format!("{replaced}:README")]),
            "changed\n"
        );
        assert!(!a.join("new").exists());
        assert_eq!(git(&a, &["rev-parse", "HEAD"]), head);
        assert_eq!(git(&a, &["branch", "--show-current"]), "main\n");
        assert_eq!(std::fs::read_to_string(a.join("README")).unwrap(), "a");
        assert_eq!(
            std::fs::read_to_string(b.join("README")).unwrap(),
            "my edit"
        );
        assert_eq!(git(&b, &["stash", "list"]), "");
    }

    #[test]
    fn test_atomic_batch_refuses_a_dirty_target_and_runs_nothing() {
        let (server, tmp) = workspace();
        std::fs::write(tmp.path().join("b/README"), "my edit").unwrap();

        let err = batch(&server, json!({"command": "touch ran", "atomic": true})).unwrap_err();

        assert!(
            err.to_string().contains("b: has uncommitted changes"),
            "{err}"
        );
        assert!(!tmp.path().join("a/ran").exists());
        assert_eq!(git(&tmp.path().join("b"), &["stash", "list"]), "");
    }

    #[test]
    fn test_atomic_batch_refuses_hidden_untracked_files_and_non_repos() {
        let (server, tmp) = workspace();
        let a = tmp.path().join("a");
        git(&a, &["config", "status.showUntrackedFiles", "no"]);
        std::fs::write(a.join("mine"), "my file").unwrap();
        // `c` is a plain directory inside the workspace root's repository,
        // not a repository of its own. (A project nested inside another
        // project, such as `a/sub`, is refused when .meta.yaml is parsed.)
        init_repo(tmp.path());
        std::fs::create_dir(tmp.path().join("c")).unwrap();
        std::fs::write(
            tmp.path().join(".meta.yaml"),
            r#"{"projects":{"a":"x","c":"x"}}"#,
        )
        .unwrap();

        let err = batch(&server, json!({"command": "touch ran", "atomic": true})).unwrap_err();

        assert!(
            err.to_string().contains("a: has uncommitted changes"),
            "{err}"
        );
        assert!(
            err.to_string()
                .contains("c: not the root of a git repository"),
            "{err}"
        );
        assert!(!a.join("ran").exists());
    }

    #[test]
    fn test_atomic_batch_reports_a_failed_rollback() {
        let (server, tmp) = workspace();
        // The workspace root is a repository too, with work of its own.
        init_repo(tmp.path());
        std::fs::write(tmp.path().join("README"), "root edit").unwrap();

        let result = batch(
            &server,
            json!({"command": "rm -rf .git && false", "atomic": true}),
        )
        .unwrap();

        assert_eq!(result["rollback"]["status"], "failed", "{result}");
        assert_eq!(result["rollback"]["failed"][0]["project"], "a");
        assert_eq!(result["rollback"]["restored"], json!([]));
        assert_eq!(result["results"].as_array().unwrap().len(), 1);
        assert_eq!(git(tmp.path(), &["stash", "list"]), "");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("README")).unwrap(),
            "root edit"
        );
    }

    #[test]
    fn test_atomic_batch_stops_at_a_project_that_disappeared() {
        let (server, tmp) = workspace();
        init_repo(&tmp.path().join("c"));
        std::fs::write(
            tmp.path().join(".meta.yaml"),
            r#"{"projects":{"a":"x","b":"x","c":"x"}}"#,
        )
        .unwrap();

        let result = batch(
            &server,
            json!({"command": "rm -rf ../b && touch ran", "atomic": true}),
        )
        .unwrap();

        assert_eq!(result["results"].as_array().unwrap().len(), 2, "{result}");
        assert!(!tmp.path().join("c/ran").exists());
        assert_eq!(result["rollback"]["status"], "failed");
        assert_eq!(result["rollback"]["restored"][0]["project"], "a");
        assert_eq!(result["rollback"]["failed"][0]["project"], "b");
        assert!(!tmp.path().join("a/ran").exists());
    }

    #[test]
    fn test_rollback_reports_the_branch_tip_and_recreates_a_deleted_branch() {
        let (server, tmp) = workspace();
        let a = tmp.path().join("a");
        std::fs::write(tmp.path().join(".meta.yaml"), r#"{"projects":{"a":"x"}}"#).unwrap();
        let head = git(&a, &["rev-parse", "HEAD"]);
        let commit =
            "echo x > README && git -c user.name=t -c user.email=t@example.com commit -qam x";

        // HEAD is back on the old commit, but the branch tip has moved on.
        let result = batch(
            &server,
            json!({"command": format!("{commit} && git checkout -q HEAD~1 && false"), "atomic": true}),
        )
        .unwrap();
        let replaced = result["rollback"]["restored"][0]["replaced_commits"][0]
            .as_str()
            .unwrap();
        assert_eq!(git(&a, &["show", &format!("{replaced}:README")]), "x\n");
        assert_eq!(git(&a, &["branch", "--show-current"]), "main\n");
        assert_eq!(git(&a, &["rev-parse", "HEAD"]), head);

        let result = batch(
            &server,
            json!({"command": "git checkout -q -b tmp && git branch -q -D main && false", "atomic": true}),
        )
        .unwrap();
        assert_eq!(result["rollback"]["status"], "success", "{result}");
        assert_eq!(git(&a, &["branch", "--show-current"]), "main\n");
        assert_eq!(git(&a, &["rev-parse", "HEAD"]), head);
    }

    #[test]
    fn test_rollback_never_moves_a_submodule() {
        let (server, tmp) = workspace();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        let b = b.to_str().unwrap();
        git(
            &a,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                b,
                "sub",
            ],
        );
        git(&a, &["commit", "-qm", "sub"]);
        git(&a, &["config", "submodule.recurse", "true"]);
        std::fs::write(tmp.path().join(".meta.yaml"), r#"{"projects":{"a":"x"}}"#).unwrap();

        // The command moves the submodule on and commits that in `a`.
        let id = "git -c user.name=t -c user.email=t@example.com";
        let command = format!(
            "{id} -C sub commit -q --allow-empty -m y && git add sub && {id} commit -qm bump && false"
        );
        let result = batch(&server, json!({"command": command, "atomic": true})).unwrap();

        assert_eq!(result["rollback"]["status"], "success", "{result}");
        let sub = a.join("sub");
        assert_eq!(git(&sub, &["log", "-1", "--format=%s"]), "y\n");
        assert_eq!(git(&sub, &["branch", "--show-current"]), "main\n");
    }

    #[test]
    fn a_batch_fails_when_every_project_failed_or_an_atomic_one_rolled_back() {
        let (server, tmp) = workspace();
        let a = tmp.path().join("a");
        std::fs::write(a.join("marker"), "").unwrap();
        git(&a, &["add", "marker"]);
        git(&a, &["commit", "-qm", "marker"]);
        let reply = |script: &str, atomic: bool| {
            let args = json!({"command": ["sh", "-c", script], "atomic": atomic});
            call(&server, "meta_batch_execute", args).unwrap()
        };
        let some = reply("test -f marker", false);
        assert!(!some.is_error, "{}", some.text);
        let result: serde_json::Value = serde_json::from_str(&some.text).unwrap();
        assert_eq!(result["results"][1]["exit_code"], 1, "{result}");
        assert!(reply("exit 1", false).is_error);
        assert!(reply("test -f marker", true).is_error);
        assert!(!reply("exit 0", true).is_error);
    }
}
