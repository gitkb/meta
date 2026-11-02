//! Running git. Every git process meta starts comes from [`git`], so none
//! inherits the variables that tie git to one repository.

use crate::display::{escape, escape_path, program_output};
use std::ffi::{OsStr, OsString};
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use std::process::Command;

mod status;
mod url;

pub use status::{Operation, RepoStatus, StatusEntry};
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

/// `program`, to run in the repository folder `dir` with [`REPO_ENV_VARS`]
/// cleared: each folder is its own repository, so a `GIT_DIR` from a git
/// hook around meta must not point the git the program starts at the
/// hook's. Every program meta starts in a repo starts here (or through
/// [`git`]).
pub fn in_repo(program: impl AsRef<std::ffi::OsStr>, dir: &Path) -> Command {
    #[expect(
        clippy::disallowed_methods,
        reason = "the helper every program in a repo starts through"
    )]
    let mut command = Command::new(program);
    command.current_dir(dir);
    clear_repo_env(&mut command);
    command
}

/// The variable that keeps git started in the project folder `dir` to the
/// repository there: `GIT_CEILING_DIRECTORIES` set to the parent of the
/// folder's real path (git resolves the links in a ceiling but not in the
/// folder it runs in). A folder that isn't its own repository then gives
/// git none, never the workspace's around it. `None` where git can't be
/// given that parent (its path holds the path-list separator, which git
/// splits on without unquoting): git then looks up as it always did, and
/// [`crate::gitdir::common_dir`] still tells a folder's own repository from
/// the one around it.
pub fn ceiling(dir: &Path) -> Option<(&'static str, OsString)> {
    let real = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let Some(parent) = real.parent() else {
        return Some(("GIT_CEILING_DIRECTORIES", OsString::new()));
    };
    // git reads `C:\...` and `\\host\share`, not the `\\?\` forms
    // canonicalize gives on Windows
    #[cfg(windows)]
    let parent = &parent.to_str().map_or_else(
        || parent.to_path_buf(),
        |p| match (p.strip_prefix(r"\\?\UNC\"), p.strip_prefix(r"\\?\")) {
            (Some(unc), _) => PathBuf::from(format!(r"\\{unc}")),
            (None, Some(drive)) if drive.as_bytes().get(1) == Some(&b':') => PathBuf::from(drive),
            _ => parent.to_path_buf(),
        },
    );
    let separator = if cfg!(windows) { ';' } else { ':' };
    if parent.to_string_lossy().contains(separator) {
        return None;
    }
    // the user's own ceilings still hold
    let mut ceiling = parent.as_os_str().to_owned();
    if let Some(theirs) = std::env::var_os("GIT_CEILING_DIRECTORIES").filter(|v| !v.is_empty()) {
        ceiling.push(separator.to_string());
        ceiling.push(theirs);
    }
    Some(("GIT_CEILING_DIRECTORIES", ceiling))
}

/// `git`, to run in `dir` and the repository there only (see [`ceiling`]),
/// without [`REPO_ENV_VARS`] and without optional locks, so a read such as `git
/// status` never rewrites the index (meta's reads change nothing, previews
/// included).
pub fn git(dir: &Path) -> Command {
    let mut command = in_repo("git", dir);
    command.env("GIT_OPTIONAL_LOCKS", "0").envs(ceiling(dir));
    command
}

/// The output of `git <args>` in `dir`, without its trailing newline; when
/// git fails, an error that names the command, `dir` and git's message.
pub fn run<I, S>(dir: &Path, args: I) -> anyhow::Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    answer(dir, args, false).map(Option::unwrap_or_default)
}

/// What `git <args>` in `dir` prints (see [`run`]), for a command that
/// exits 1 or prints nothing when what it reads is absent: `rev-parse
/// --verify --quiet`, `config --get`, `branch --show-current`. Then `None`;
/// any other failure is an error.
pub fn lookup<I, S>(dir: &Path, args: I) -> anyhow::Result<Option<String>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Ok(answer(dir, args, true)?.filter(|out| !out.is_empty()))
}

/// [`run`], with exit status 1 read as `None` when `absent_on_1`.
fn answer<I, S>(dir: &Path, args: I, absent_on_1: bool) -> anyhow::Result<Option<String>>
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
    // a child this program owns: stopping it stops git and its hooks too
    let output = crate::children::output(git(dir).args(&args)).map_err(|e| {
        anyhow::anyhow!(
            "Cannot run git {} in {}: {e}",
            escape(&shown()),
            escape_path(dir)
        )
    })?;
    if absent_on_1 && output.status.code() == Some(1) {
        return Ok(None);
    }
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed in {}:{}",
            escape(&shown()),
            escape_path(dir),
            program_output(&output.stderr)
        );
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout)
            .trim_end_matches(['\n', '\r'])
            .to_string(),
    ))
}

/// The branch checked out at `dir`; `None` when HEAD is detached.
pub fn current_branch(dir: &Path) -> anyhow::Result<Option<String>> {
    lookup(dir, ["branch", "--show-current"])
}

/// The URL configured for `origin` in the repository at `dir`, as written:
/// what `.meta.yaml` lists is compared with this, so `insteadOf` rewrites
/// don't apply. Every config git reads there counts (the repository's,
/// its worktree's, included files). `None` without one; ask only about a
/// folder that is a repository (see [`crate::gitdir::common_dir`]).
pub fn remote_url(dir: &Path) -> anyhow::Result<Option<GitUrl>> {
    Ok(lookup(dir, ["config", "--get", "remote.origin.url"])?.map(GitUrl::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variable_git_reports_is_cleared() {
        let _sandbox = meta_test_support::isolated!();
        let reported = run(Path::new("."), ["rev-parse", "--local-env-vars"]).unwrap();
        let kept = ["GIT_CONFIG_COUNT", "GIT_CONFIG_PARAMETERS"];
        for name in reported.lines().filter(|name| !kept.contains(name)) {
            assert!(REPO_ENV_VARS.contains(&name), "{name} is not cleared");
        }
    }

    #[test]
    fn git_in_a_project_folder_never_reaches_the_repository_around_it() {
        let sandbox = meta_test_support::isolated!();
        let dir = sandbox.work();
        let workspace = dir.join("ws");
        std::fs::create_dir_all(workspace.join("plain")).unwrap();
        sandbox.git(&workspace, &["init", "-q", "-b", "main"]);
        let toplevel = ["rev-parse", "--show-toplevel"];
        assert!(run(&workspace.join("plain"), toplevel).is_err());
        // nor through a link to it from elsewhere, nor for any program the
        // folder runs that starts git (`sh -c 'git ...'`)
        #[cfg(unix)]
        {
            let link = dir.join("link");
            std::os::unix::fs::symlink(workspace.join("plain"), &link).unwrap();
            assert!(run(&link, toplevel).is_err());
            let shell = sandbox
                .command("sh")
                .args(["-c", "git rev-parse --show-toplevel"])
                .current_dir(&link)
                .envs(ceiling(&link))
                .output()
                .unwrap();
            assert!(!shell.status.success(), "{shell:?}");
        }
        // a repository of its own still is one
        sandbox.git(&workspace.join("plain"), &["init", "-q", "-b", "main"]);
        assert!(run(&workspace.join("plain"), toplevel).is_ok());
    }

    #[test]
    fn a_path_list_separator_in_the_path_still_keeps_git_inside() {
        let sandbox = meta_test_support::isolated!();
        let separator = if cfg!(windows) { ";" } else { ":" };
        let dir = sandbox.work();
        let workspace = dir.join(format!("client{separator}acme"));
        for folder in ["plain", "cloned/sub"] {
            std::fs::create_dir_all(workspace.join(folder)).unwrap();
        }
        sandbox.git(&workspace, &["init", "-q", "-b", "main"]);
        sandbox.git(&workspace.join("cloned"), &["init", "-q", "-b", "main"]);
        let toplevel = ["rev-parse", "--show-toplevel"];
        // a clone is still its own repository, from any folder in it
        let real = |p: &Path| p.canonicalize().unwrap();
        let shown = run(&workspace.join("cloned"), toplevel).unwrap();
        assert_eq!(real(Path::new(&shown)), real(&workspace.join("cloned")));
        let mut sub = git(&workspace.join("cloned"));
        sub.current_dir(workspace.join("cloned/sub")).args(toplevel);
        assert!(sub.output().unwrap().status.success());
        // and the checkout state of a folder inside the workspace is still
        // its own
        std::fs::create_dir(workspace.join("plain/.git")).unwrap();
        assert!(crate::gitdir::common_dir(&workspace.join("plain")).is_err());
        assert_eq!(
            crate::gitdir::common_dir(&workspace.join("cloned/sub")).unwrap(),
            None
        );
    }

    #[test]
    fn remote_url_is_the_configured_one() {
        let sandbox = meta_test_support::isolated!();
        let dir = sandbox.work();
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
            sandbox.git(&dir, args);
        }
        // what git would fetch from is rewritten; what is listed is not
        assert_ne!(
            run(dir.as_path(), ["remote", "get-url", "origin"]).unwrap(),
            configured
        );
        assert_eq!(remote_url(dir.as_path()).unwrap(), Some(configured.into()));
        // the repository's config includes what it includes
        sandbox.git(dir.as_path(), &["remote", "remove", "origin"]);
        let included = dir.join(".git/remote.conf");
        std::fs::write(
            &included,
            format!("[remote \"origin\"]\n\turl = {configured}\n"),
        )
        .unwrap();
        sandbox.git(dir.as_path(), &["config", "include.path", "remote.conf"]);
        assert_eq!(remote_url(dir.as_path()).unwrap(), Some(configured.into()));
        sandbox.git(dir.as_path(), &["config", "--unset", "include.path"]);
        // and so does a worktree's own config
        sandbox.git(
            dir.as_path(),
            &["config", "extensions.worktreeConfig", "true"],
        );
        sandbox.git(
            dir.as_path(),
            &["config", "--worktree", "remote.origin.url", configured],
        );
        assert_eq!(remote_url(dir.as_path()).unwrap(), Some(configured.into()));
        sandbox.git(
            dir.as_path(),
            &["config", "--worktree", "--unset", "remote.origin.url"],
        );
        assert_eq!(remote_url(dir.as_path()).unwrap(), None);
    }

    #[test]
    fn run_names_the_failing_command() {
        let sandbox = meta_test_support::isolated!();
        let dir = sandbox.work();
        let err = run(dir.as_path(), ["rev-parse", "--verify", "nope"]).unwrap_err();
        assert!(
            err.to_string()
                .contains("git rev-parse --verify nope failed"),
            "{err}"
        );
        // absent, as `--verify --quiet` says by exiting 1; a git failure
        // is still an error
        sandbox.git(dir.as_path(), &["init", "-q"]);
        let absent = lookup(dir.as_path(), ["rev-parse", "--verify", "--quiet", "nope"]);
        assert_eq!(absent.unwrap(), None);
        assert!(lookup(&dir.join("missing"), ["rev-parse", "HEAD"]).is_err());
    }
}
