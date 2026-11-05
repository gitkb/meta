//! The members of a worktree set, as its folders show them: the checkouts
//! git's linked-worktree layout and meta-git's records prove are the set's.
//! meta-git's own scan, the authority for what a set holds (meta's members
//! of a set are advisory).

use anyhow::{Context, Result};
use meta_core::gitdir;
use meta_plugin::display::{escape, escape_lines, escape_path};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Discovered information about a repo within a worktree set.
#[derive(Debug, Clone, Serialize)]
pub struct WorktreeRepoInfo {
    pub alias: String,
    pub branch: String,
    pub path: PathBuf,
    pub source_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_branch: Option<bool>,
}

/// Whether a checkout meta made for a set (a linked worktree with meta's
/// record, readable or not, or one whose git directory can't be found) may
/// be under `dir`, as in a set moved by hand; a part of `dir` that can't be
/// read may hold one too. Other tools' worktrees
/// have no record; plain clones (a `.git` directory, or a link to one) are
/// not looked inside.
pub fn holds_meta_checkout(dir: &Path) -> bool {
    let canonical_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    // Where discovery looks: not in `.git`, nor in the sets of a workspace
    // that is itself under `.worktrees` (its own nested `.worktrees`)
    let mut walk = walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || ScannedDirs::searched(entry.path()));
    while let Some(entry) = walk.next() {
        // What can't be read may hold one: can't tell is no plain clone
        let Ok(entry) = entry else { return true };
        if !entry.file_type().is_dir() {
            continue;
        }
        match std::fs::metadata(entry.path().join(".git")) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return true,
            Ok(meta) if meta.is_dir() => walk.skip_current_dir(),
            // a checkout of a set kept inside `dir` by a workspace inside
            // `dir` (one there with sets of its own, wherever its
            // worktrees_dir puts them) is no sign that `dir` is a set
            // (a record that can't be read says nothing, and is checked below)
            Ok(_)
                if meta_core::set_record::member(entry.path())
                    .ok()
                    .flatten()
                    .is_some_and(|(_, record)| {
                        let inside = |path: &Path| {
                            path.canonicalize()
                                .is_ok_and(|path| path.starts_with(&canonical_dir))
                        };
                        inside(&record.set)
                            && record.set.canonicalize().ok() != Some(canonical_dir.clone())
                            && inside(&record.project)
                    }) => {}
            // even one meta can no longer read: it was meta's. A checkout
            // whose git directory can't be found (moved, with relative paths)
            // may be meta's too: it is not taken for a plain clone
            Ok(_) => {
                let meta_made = gitdir::gitfile_target(entry.path())
                    .ok()
                    .flatten()
                    .is_none_or(|admin| {
                        // only a record that is surely absent says "not meta's"
                        !admin.is_dir()
                            || !matches!(
                                admin
                                    .join(meta_core::set_record::RECORD_FILE)
                                    .symlink_metadata(),
                                Err(e) if e.kind() == std::io::ErrorKind::NotFound
                            )
                    });
                if meta_made {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// [`discover_worktree_members`] with each one's branch.
pub fn discover_worktree_repos(
    task_dir: &Path,
    places: &[String],
) -> Result<Vec<WorktreeRepoInfo>> {
    Ok(discover_worktree_members(task_dir, places)?
        .into_iter()
        .map(with_branch)
        .collect())
}

fn with_branch(member: WorktreeMemberInfo) -> WorktreeRepoInfo {
    WorktreeRepoInfo {
        // a branch git can't read is shown as by git for a detached HEAD;
        // status and list say when a checkout can't be read
        branch: meta_core::git::current_branch(&member.path)
            .ok()
            .flatten()
            .unwrap_or_else(|| "HEAD".to_string()),
        alias: member.alias,
        path: member.path,
        source_path: member.source_path,
        created_branch: None,
    }
}

/// Every member of the set at `task_dir`, nested ones (`clients/desktop`
/// inside `clients`) too, without branch probes (command scoping needs only
/// their paths, and starts no git for each): the one member list, as a
/// listing of the set shows it. What can't be read is skipped; a checkout
/// found inside a member counts only if git's linked-worktree layout holds
/// (a submodule doesn't).
pub fn discover_worktree_members(
    task_dir: &Path,
    places: &[String],
) -> Result<Vec<WorktreeMemberInfo>> {
    let mut repos = Vec::new();

    // Check if the task dir itself is a worktree (the "." alias)
    let dot_git = task_dir.join(".git");
    if dot_git
        .symlink_metadata()
        .map(|m| m.is_file())
        .unwrap_or(false)
    {
        let source = source_repo(task_dir)?;
        repos.push(WorktreeMemberInfo {
            alias: ".".to_string(),
            path: task_dir.to_path_buf(),
            source_path: source,
        });
    }

    // Recursively scan subdirectories for worktrees.
    // Repos may be nested (e.g., "vendor/tree-sitter-markdown") when
    // dependencies have paths with intermediate directories.
    if task_dir.is_dir() {
        let scanned = ScannedDirs::of(task_dir);
        discover_repos_recursive(task_dir, task_dir, &scanned, places, &mut repos)?;
    }

    // Sort by alias for deterministic output ("." sorts first)
    repos.sort_by(|a, b| a.alias.cmp(&b.alias));
    repos.dedup_by(|a, b| a.alias == b.alias);

    Ok(repos)
}

/// Recursively scan `dir` for git worktree repos, recording aliases as
/// relative paths from `root` (the worktree task directory).
///
/// A directory with a `.git` file is a worktree repo: recorded, with the
/// repos nested in it ([`add_member`]). A directory without `.git` is an
/// intermediate directory (e.g., `vendor/`): recursed into.
fn discover_repos_recursive(
    root: &Path,
    dir: &Path,
    scanned: &ScannedDirs,
    places: &[String],
    repos: &mut Vec<WorktreeMemberInfo>,
) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            log::debug!(
                "Skipping unreadable directory {}: {}",
                escape_path(dir),
                escape_lines(&format!("{e:#}"))
            );
            return Ok(());
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let sub_path = entry.path();
        if !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
            continue;
        }
        if !ScannedDirs::searched(&sub_path) {
            continue;
        }
        match is_checkout(&sub_path) {
            true => add_member(root, &sub_path, scanned, places, false, repos)?,
            // an unreadable one is skipped there; a record that can't be
            // read is an error
            false => discover_repos_recursive(root, &sub_path, scanned, places, repos)?,
        }
    }
    Ok(())
}

/// Whether `dir` has a `.git` file: a worktree's (or a submodule's).
fn is_checkout(dir: &Path) -> bool {
    dir.join(".git")
        .symlink_metadata()
        .is_ok_and(|m| m.is_file())
}

/// Record the checkout at `dir` as a member when it is one, then the
/// members nested in it. A set lays its repos out as the workspace does, so
/// those sit where its own `.meta.yaml` puts its projects: only there is
/// looked, not through its whole tree (a build folder can hold thousands of
/// folders), and a worktree made by hand inside it is no member. `nested`:
/// `dir` is one of those, where only a checkout in git's linked-worktree
/// layout counts (a submodule doesn't).
fn add_member(
    root: &Path,
    dir: &Path,
    scanned: &ScannedDirs,
    places: &[String],
    nested: bool,
    repos: &mut Vec<WorktreeMemberInfo>,
) -> Result<()> {
    if nested && gitdir::linked(dir).is_err() {
        return Ok(());
    }
    if !scanned.member(root, dir)? {
        return Ok(());
    }
    let source = match source_repo(dir) {
        Ok(s) => s,
        Err(e) => {
            log::debug!(
                "Skipping {}: {}",
                escape_path(dir),
                escape_lines(&format!("{e:#}"))
            );
            return Ok(());
        }
    };
    repos.push(WorktreeMemberInfo {
        alias: dir
            .strip_prefix(root)
            .unwrap_or(dir)
            .to_string_lossy()
            .to_string(),
        path: dir.to_path_buf(),
        source_path: source.clone(),
    });
    // the places of the workspace's projects inside it (as meta read the
    // workspace's configs), and of the projects of its own .meta.yaml as
    // checked out and of its source's as it is on disk, which create planned
    // from (as meta reads them; one meta can't read lists none: the strict
    // scan says why): only a checkout there is one nested in it
    let Some(inside) = dir.strip_prefix(root).ok().map(|rel| {
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    }) else {
        return Ok(());
    };
    let listed = |checkout: &Path| -> Vec<String> {
        if !checkout.join(".meta.yaml").is_file() {
            return Vec::new();
        }
        crate::here::Listing::of(checkout, None)
            .ok()
            .flatten()
            .map(|listing| {
                listing
                    .projects
                    .into_iter()
                    .map(|p| format!("{inside}/{}", p.path))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut candidates: Vec<String> = places.to_vec();
    candidates.extend(listed(dir));
    candidates.extend(listed(&source));
    candidates.sort();
    candidates.dedup();
    for place in &candidates {
        let rest = match inside.as_str() {
            "" => continue,
            inside => place.strip_prefix(&format!("{inside}/")),
        };
        // the next checkout down, not one nested in another nested one; a
        // place outside the checkout is no repo nested in it
        let Some(rest) = rest.filter(|rest| {
            Path::new(rest)
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
                && !candidates.iter().any(|other| {
                    other != place
                        && place.starts_with(&format!("{other}/"))
                        && other.starts_with(&format!("{inside}/"))
                        && is_checkout(&root.join(other))
                })
        }) else {
            continue;
        };
        let nested = dir.join(rest);
        if is_checkout(&nested) {
            add_member(root, &nested, scanned, places, true, repos)?;
        }
    }
    Ok(())
}

/// Discover all realized worktrees, including repos nested beneath other repos.
/// Unlike legacy discovery, traversal errors are fatal. Under a hidden
/// directory only checkouts meta recorded in this set count (see
/// `ScannedDirs`); directory symlinks are not followed, so traversal never
/// leaves the set.
/// A finite entry budget prevents unbounded scans of generated directory trees.
pub fn discover_worktree_repos_strict(task_dir: &Path) -> Result<Vec<WorktreeRepoInfo>> {
    Ok(discover_worktree_members_strict(task_dir)?
        .into_iter()
        .map(with_branch)
        .collect())
}

/// Realized membership without branch, dirty-state or ahead/behind probes.
/// This uses the same strict bounded traversal and Git backlink validation as
/// recursive status, but never launches Git. Consumers can observe Git once.
#[derive(Debug, Clone, Serialize)]
pub struct WorktreeMemberInfo {
    pub alias: String,
    pub path: PathBuf,
    pub source_path: PathBuf,
}

/// The set, resolved, that meta-git's record says the checkout at `dir` was
/// made for; `None` without a record, or when the set is gone.
fn recorded_set(dir: &Path) -> Result<Option<PathBuf>> {
    Ok(meta_core::set_record::member(dir)?.and_then(|(_, record)| record.set.canonicalize().ok()))
}

/// Which checkouts in a set are its members. `.git` and a nested
/// `.worktrees` hold none. Under another hidden directory, only a checkout
/// meta's record says it made for this set counts: a project may live at
/// `.deps/x`, but a tool's own worktrees (`.claude/worktrees/x`) are not the
/// set's.
struct ScannedDirs {
    set: PathBuf,
}

impl ScannedDirs {
    fn of(task_dir: &Path) -> Self {
        ScannedDirs {
            set: task_dir
                .canonicalize()
                .unwrap_or_else(|_| task_dir.to_path_buf()),
        }
    }

    /// Whether to look for members inside `dir`.
    fn searched(dir: &Path) -> bool {
        dir.file_name()
            .is_none_or(|name| name != ".git" && name != ".worktrees")
    }

    /// Whether the checkout at `dir`, inside the set at `root`, is a member.
    fn member(&self, root: &Path, dir: &Path) -> Result<bool> {
        let hidden = dir.strip_prefix(root).is_ok_and(|place| {
            place
                .components()
                .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        });
        Ok(!hidden || recorded_set(dir)?.as_ref() == Some(&self.set))
    }
}

pub fn discover_worktree_members_strict(task_dir: &Path) -> Result<Vec<WorktreeMemberInfo>> {
    let mut repos = Vec::new();
    let scanned = ScannedDirs::of(task_dir);
    let entries = walkdir::WalkDir::new(task_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || ScannedDirs::searched(entry.path()));
    for (count, entry) in entries.enumerate() {
        anyhow::ensure!(
            count < 2_000_000,
            "Recursive worktree discovery exceeded 2000000 entries"
        );
        // walkdir's error names the path it failed on: kept on one line
        let entry = entry.map_err(|e| {
            anyhow::anyhow!(
                "Cannot scan worktree {}: {}",
                escape_path(task_dir),
                escape(&e.to_string())
            )
        })?;
        if !entry.file_type().is_dir() {
            continue;
        }
        let path = entry.path();
        let git_file = path.join(".git");
        let metadata = match git_file.symlink_metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Cannot inspect {}", escape_path(&git_file)))
            }
        };
        if !metadata.is_file() || !scanned.member(task_dir, path)? {
            continue;
        }
        gitdir::linked(path)?;
        let source_path = source_repo(path)?;
        let alias = if path == task_dir {
            ".".to_string()
        } else {
            path.strip_prefix(task_dir)?.to_string_lossy().to_string()
        };
        repos.push(WorktreeMemberInfo {
            alias,
            path: path.to_path_buf(),
            source_path,
        });
    }
    repos.sort_by(|a, b| a.alias.cmp(&b.alias));
    Ok(repos)
}

/// The checkout the linked worktree at `checkout` was made from: the
/// directory above the git directory that holds its admin directory
/// (`<repo>/.git/worktrees/<id>`). An absolute `gitdir:` is taken as written.
fn source_repo(checkout: &Path) -> Result<PathBuf> {
    let target = gitdir::read_gitfile(checkout)?
        .with_context(|| format!("{} has no .git file", escape_path(checkout)))?;
    let admin = if target.is_absolute() {
        target
    } else {
        checkout
            .join(&target)
            .canonicalize()
            .with_context(|| format!("Cannot resolve gitdir of {}", escape_path(checkout)))?
    };
    admin
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
        .with_context(|| {
            format!(
                "Cannot derive source repo from gitdir: {}",
                escape_path(&admin)
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use meta_plugin::testing::Sandbox;

    /// Create a real committed repository and linked worktree in the fixture.
    fn member(sandbox: &Sandbox, root: &Path, alias: &str) {
        let sources = if alias == "." {
            root.parent().unwrap().join(".sources")
        } else {
            root.join(".sources")
        };
        std::fs::create_dir_all(&sources).unwrap();
        let source = sources.join(format!(
            "repo{}",
            std::fs::read_dir(&sources).unwrap().count()
        ));
        sandbox.repo(&source);
        sandbox.git(
            &source,
            &[
                "worktree",
                "add",
                "-qb",
                "topic",
                root.join(alias).to_str().unwrap(),
            ],
        );
    }

    /// Normalize separators for platform-independent alias assertions.
    fn aliases(repos: Vec<WorktreeRepoInfo>) -> Vec<String> {
        repos
            .into_iter()
            .map(|repo| repo.alias.replace('\\', "/"))
            .collect()
    }

    #[test]
    /// Nested members have sorted, distinct, full aliases.
    fn members_include_nested_ones_with_sorted_full_aliases() {
        let sandbox = meta_plugin::testing::isolated!();
        let root = &sandbox.work().join("set");
        for alias in [".", "open-source", "open-source/atc", "other/atc"] {
            member(&sandbox, root, alias);
        }
        // a repo nested in a member is at a project's place
        let places = [
            "open-source/atc".to_string(),
            "open-source/absent".to_string(),
        ];
        std::fs::create_dir_all(root.join("configured-but-absent")).unwrap();
        // the strict scan finds the same members
        for repos in [
            discover_worktree_repos(root, &places),
            discover_worktree_repos_strict(root),
        ] {
            assert_eq!(
                aliases(repos.unwrap()),
                [".", "open-source", "open-source/atc", "other/atc"]
            );
        }
    }

    #[test]
    fn nested_members_are_found_without_collecting_branch_facts() {
        let sandbox = meta_plugin::testing::isolated!();
        let root = sandbox.work().join(".worktrees/example");
        for alias in [".", "parent", "parent/nested"] {
            member(&sandbox, &root, alias);
        }
        // a project's place in the workspace
        let places = ["parent/nested".to_string()];
        let paths: Vec<PathBuf> = discover_worktree_members(&root, &places)
            .unwrap()
            .into_iter()
            .map(|member| member.path)
            .collect();
        assert_eq!(
            paths,
            vec![
                root.clone(),
                root.join("parent"),
                root.join("parent/nested")
            ]
        );
        let metadata = discover_worktree_members_strict(&root).unwrap();
        assert_eq!(metadata.len(), 3);
        for row in metadata {
            let json = serde_json::to_value(row).unwrap();
            assert!(json.get("branch").is_none());
            assert!(json.get("dirty").is_none());
        }
    }

    #[test]
    /// Malformed nested Git files fail instead of silently omitting members.
    fn recursive_malformed_member_is_error_instead_of_partial_inventory() {
        let sandbox = meta_plugin::testing::isolated!();
        member(&sandbox, &sandbox.work(), "parent");
        let child = sandbox.work().join("parent/broken");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join(".git"), "invalid").unwrap();
        assert!(discover_worktree_repos_strict(sandbox.work().as_path()).is_err());
        assert_eq!(
            aliases(discover_worktree_repos(sandbox.work().as_path(), &[]).unwrap()),
            ["parent"]
        );
    }

    #[test]
    /// Missing roots cannot be represented as complete empty inventories.
    fn recursive_missing_root_is_error() {
        let sandbox = meta_plugin::testing::isolated!();
        assert!(discover_worktree_repos_strict(&sandbox.work().join("missing")).is_err());
    }

    #[cfg(unix)]
    #[test]
    /// Under a hidden directory, a checkout meta recorded for this set is a
    /// member; others (an agent's own worktrees), a nested set and symlink
    /// cycles are not.
    fn recursive_finds_recorded_hidden_members_only() {
        let sandbox = meta_plugin::testing::isolated!();
        let root = sandbox.work();
        member(&sandbox, &root, "visible");
        member(&sandbox, &root, ".deps/child");
        let gitfile = std::fs::read_to_string(root.join(".deps/child/.git")).unwrap();
        let admin = Path::new(gitfile.trim().strip_prefix("gitdir: ").unwrap());
        std::fs::write(
            admin.join(meta_core::set_record::RECORD_FILE),
            serde_json::json!({ "project": "/p", "set": root, "created_at": "2025-01-01T00:00:00Z" })
                .to_string(),
        )
        .unwrap();
        member(&sandbox, &root, ".claude/worktrees/agent");
        member(&sandbox, &root, ".worktrees/other");
        std::os::unix::fs::symlink(&root, root.join("visible/cycle")).unwrap();
        assert_eq!(
            aliases(discover_worktree_repos_strict(&root).unwrap()),
            [".deps/child", "visible"]
        );
    }
    /// Relative Git links resolve against the checkout, independently of cwd.
    #[test]
    fn relative_gitdir_reports_absolute_source() {
        let sandbox = meta_plugin::testing::isolated!();
        let root = sandbox.work().join("set");
        member(&sandbox, &root, ".");
        let source = sandbox
            .work()
            .join(".sources/repo0")
            .canonicalize()
            .unwrap();
        let git_file = root.join(".git");
        let content = std::fs::read_to_string(&git_file).unwrap();
        let admin = Path::new(content.trim().strip_prefix("gitdir: ").unwrap());
        let name = admin.file_name().unwrap().to_str().unwrap();
        std::fs::write(
            &git_file,
            format!("gitdir: ../.sources/repo0/.git/worktrees/{name}\n"),
        )
        .unwrap();
        assert_eq!(source_repo(&root).unwrap(), source);
        assert_eq!(
            discover_worktree_repos_strict(&root).unwrap()[0].source_path,
            source
        );
        assert_eq!(
            discover_worktree_repos(&root, &[]).unwrap()[0].source_path,
            source
        );
    }

    /// Dangling and incomplete admin links must not produce successful members.
    #[test]
    fn recursive_rejects_missing_worktree_metadata() {
        let sandbox = meta_plugin::testing::isolated!();
        for missing in ["HEAD", "commondir", "gitdir", "entire-admin"] {
            let root = sandbox.work().join(missing);
            member(&sandbox, &root, "child");
            let content = std::fs::read_to_string(root.join("child/.git")).unwrap();
            let admin = Path::new(content.trim().strip_prefix("gitdir: ").unwrap());
            if missing == "entire-admin" {
                std::fs::remove_dir_all(admin).unwrap();
            } else {
                std::fs::remove_file(admin.join(missing)).unwrap();
            }
            assert!(discover_worktree_repos_strict(&root).is_err(), "{missing}");
        }
    }
}
