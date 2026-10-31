//! Running git. Every git process meta starts comes from [`git`], so none
//! inherits the variables that tie git to one repository.

use crate::display::{escape, escape_path};
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

mod status;
mod url;

pub use status::{RepoStatus, StatusEntry};
pub use url::GitUrl;

/// The variables that tie git to one repository, which git itself clears
/// when it runs in another (`git rev-parse --local-env-vars`, less the
/// `-c`/`GIT_CONFIG_COUNT` settings git keeps for a submodule: they are the
/// user's, not the repository's). A git hook or alias sets some of them
/// (`GIT_DIR`, `GIT_INDEX_FILE`); left in place, git would act on that
/// repository from every directory meta runs it in.
pub const REPO_ENV_VARS: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_CONFIG",
    "GIT_DIR",
    "GIT_GRAFT_FILE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_INTERNAL_SUPER_PREFIX",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_OBJECT_DIRECTORY",
    "GIT_PREFIX",
    "GIT_REPLACE_REF_BASE",
    "GIT_SHALLOW_FILE",
    "GIT_WORK_TREE",
];

/// Remove [`REPO_ENV_VARS`] from what `command` will inherit.
pub fn clear_repo_env(command: &mut Command) -> &mut Command {
    for key in REPO_ENV_VARS {
        command.env_remove(key);
    }
    command
}

/// `git`, to run in `dir`, without [`REPO_ENV_VARS`].
pub fn git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(dir);
    clear_repo_env(&mut command);
    command
}

/// The output of `git <args>` in `dir`, without its trailing newline; when
/// git fails, an error that names the command, `dir` and git's message.
pub fn run<I, S>(dir: &Path, args: I) -> anyhow::Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<S> = args.into_iter().collect();
    let shown = || {
        args.iter()
            .map(|a| a.as_ref().to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let output = git(dir).args(&args).output().map_err(|e| {
        anyhow::anyhow!(
            "Cannot run git {} in {}: {e}",
            escape(&shown()),
            escape_path(dir)
        )
    })?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed in {}: {}",
            escape(&shown()),
            escape_path(dir),
            escape(String::from_utf8_lossy(&output.stderr).trim())
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(['\n', '\r'])
        .to_string())
}

/// The output of `git <args>` in `dir` (see [`run`]), or `None` if git fails.
pub fn query<I, S>(dir: &Path, args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run(dir, args).ok()
}

/// The branch checked out at `dir`: `None` when HEAD is detached or git
/// fails. Unlike [`RepoStatus::read`], it works in a bare repository.
pub fn current_branch(dir: &Path) -> Option<String> {
    query(dir, ["branch", "--show-current"]).filter(|branch| !branch.is_empty())
}

/// The URL configured for `origin` in the repository at `dir`, as written:
/// what `.meta.yaml` lists is compared with this, so `insteadOf` rewrites
/// don't apply.
pub fn remote_url(dir: &Path) -> Option<String> {
    query(dir, ["config", "--get", "remote.origin.url"]).filter(|url| !url.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variable_git_reports_is_cleared() {
        let reported = run(Path::new("."), ["rev-parse", "--local-env-vars"]).unwrap();
        let kept = ["GIT_CONFIG_COUNT", "GIT_CONFIG_PARAMETERS"];
        for name in reported.lines().filter(|name| !kept.contains(name)) {
            assert!(REPO_ENV_VARS.contains(&name), "{name} is not cleared");
        }
    }

    #[test]
    fn remote_url_is_the_configured_one() {
        let dir = tempfile::tempdir().unwrap();
        let configured = "https://example.invalid/org/repo.git";
        for args in [
            &["init", "-q"][..],
            &["remote", "add", "origin", configured],
            &[
                "config",
                "url.ssh://git@example.invalid/.insteadOf",
                "https://example.invalid/",
            ],
        ] {
            run(dir.path(), args).unwrap();
        }
        // what git would fetch from is rewritten; what is listed is not
        assert_ne!(
            run(dir.path(), ["remote", "get-url", "origin"]).unwrap(),
            configured
        );
        assert_eq!(remote_url(dir.path()).as_deref(), Some(configured));
        assert_eq!(remote_url(&dir.path().join("missing")), None);
    }

    #[test]
    fn run_names_the_failing_command() {
        let dir = tempfile::tempdir().unwrap();
        let err = run(dir.path(), ["rev-parse", "--verify", "nope"]).unwrap_err();
        assert!(
            err.to_string()
                .contains("git rev-parse --verify nope failed"),
            "{err}"
        );
        assert_eq!(query(dir.path(), ["rev-parse", "--verify", "nope"]), None);
    }
}
