//! The workspace a command runs in, as meta resolved it (`Request::workspace`),
//! and the projects of a meta repo's config, as meta reads them for meta-git
//! (`meta --json --no-parent project list` in that folder): meta-git reads no
//! `.meta.yaml` itself.

use anyhow::{Context, Result};
use meta_plugin::display::{escape, escape_path};
use meta_plugin::{ErrorCode, ListedProject, Project, RepoUrl, Request};
use std::path::{Component, Path, PathBuf};

/// The workspace a command runs in.
pub(crate) struct Here<'a> {
    pub workspace: &'a meta_plugin::Workspace,
    /// Where its sets go and what runs around them
    pub worktree: &'a meta_plugin::Worktree,
    /// The folder of the config the command runs from: where its projects
    /// are listed
    pub dir: PathBuf,
    /// The folder whose sets these are and whose settings apply: inside a
    /// set, the checkout the set stands for, which outlives it; else `dir`
    pub home: PathBuf,
}

impl<'a> Here<'a> {
    /// The workspace `request` runs in; none outside one.
    pub fn of(request: &'a Request) -> Option<Here<'a>> {
        let workspace = request.workspace.as_ref()?;
        let dir = workspace
            .config
            .parent()
            .map_or_else(|| workspace.listed_root.clone(), Path::to_path_buf);
        let home = request.worktree.home.clone().unwrap_or_else(|| dir.clone());
        Some(Here {
            workspace,
            worktree: &request.worktree,
            dir,
            home,
        })
    }

    /// [`Here::of`], or the error outside a workspace.
    pub fn need(request: &'a Request) -> Result<Here<'a>> {
        Self::of(request)
            .ok_or_else(|| anyhow::anyhow!("Not inside a meta project (no .meta.yaml found)"))
    }

    /// The projects listed under `dir` (the config's folder), each at its
    /// place there, as far as meta could read the configs.
    pub fn tree(&self) -> Tree<'a> {
        Tree::of(
            &self.workspace.members,
            &self.dir,
            self.dir == self.workspace.root,
        )
    }

    /// Where the workspace's projects sit in a set: each one's path from the
    /// folder a set stands for (from the set, for a command run in one).
    pub fn places(&self) -> Vec<String> {
        let mut places: Vec<String> = self
            .workspace
            .members
            .iter()
            .filter_map(|m| {
                place(&m.abs, &self.home).or_else(|| place(&m.abs, &self.workspace.root))
            })
            .collect();
        places.sort();
        places.dedup();
        places
    }
}

/// Where sets go for `request`, as meta resolved it (`Request::worktree`);
/// when meta refused the place the config gives, meta's reason.
pub(crate) fn worktree_root(request: &Request) -> Result<PathBuf> {
    let worktree = &request.worktree;
    worktree.dir.clone().ok_or_else(|| match &worktree.refused {
        Some(why) => anyhow::anyhow!("{why}"),
        None => anyhow::anyhow!("The request names no folder for worktree sets"),
    })
}

/// `path`'s place below `dir` (`a/b`), when it is one: not `dir` itself.
pub(crate) fn place(path: &Path, dir: &Path) -> Option<String> {
    let rel = path.strip_prefix(dir).ok()?;
    let parts: Option<Vec<&str>> = rel
        .components()
        .map(|c| match c {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect();
    let parts = parts?;
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// One project of a [`Tree`].
#[derive(Debug, Clone)]
pub(crate) struct Node<'a> {
    pub project: &'a Project,
    /// Its path from the tree's folder
    pub place: String,
    /// Its path in the config that lists it
    pub own: String,
    /// How many meta repos are between the tree's folder and it (0: one
    /// the tree's own config lists)
    pub level: usize,
}

impl Node<'_> {
    pub fn repo(&self) -> Option<RepoUrl> {
        self.project.repo.clone().map(RepoUrl::from)
    }
}

/// The projects under a folder, nested meta repos' too, each with its
/// place and level.
pub(crate) struct Tree<'a> {
    pub dir: PathBuf,
    pub nodes: Vec<Node<'a>>,
}

impl<'a> Tree<'a> {
    /// The members under `dir`, as their meta repos list them; with `root`
    /// (`dir` is the workspace root), also those its config lists outside
    /// it (`path: ../sib`), each at its name, as a set places one.
    pub fn of(members: &'a [Project], dir: &Path, root: bool) -> Tree<'a> {
        let metas: Vec<&Path> = members
            .iter()
            .filter(|m| m.meta)
            .map(|m| m.abs.as_path())
            .collect();
        let mut nodes: Vec<Node<'a>> = members
            .iter()
            .filter_map(|project| {
                let Some(place) = place(&project.abs, dir) else {
                    let outside = root && project.path.starts_with("../");
                    return outside.then(|| Node {
                        project,
                        own: project.path.clone(),
                        place: project.name.clone(),
                        level: 0,
                    });
                };
                // the meta repos on the way to it, below `dir`
                let above: Vec<&Path> = metas
                    .iter()
                    .copied()
                    .filter(|m| {
                        *m != project.abs
                            && project.abs.starts_with(m)
                            && m.starts_with(dir)
                            && *m != dir
                    })
                    .collect();
                let parent = above
                    .iter()
                    .copied()
                    .max_by_key(|m| m.components().count())
                    .unwrap_or(dir);
                Some(Node {
                    project,
                    own: self::place(&project.abs, parent).unwrap_or_else(|| place.clone()),
                    place,
                    level: above.len(),
                })
            })
            .collect();
        nodes.sort_by(|a, b| a.place.cmp(&b.place));
        Tree {
            dir: dir.to_path_buf(),
            nodes,
        }
    }

    /// The ones `dir`'s own config lists.
    pub fn top(&self) -> impl Iterator<Item = &Node<'a>> {
        self.nodes.iter().filter(|n| n.level == 0)
    }

    /// The ones at most `depth` meta repos down (all of them with none).
    pub fn within(&self, depth: Option<usize>) -> impl Iterator<Item = &Node<'a>> {
        self.nodes
            .iter()
            .filter(move |n| depth.is_none_or(|d| n.level <= d))
    }

    /// The one at `place`.
    pub fn at(&self, place: &str) -> Option<&Node<'a>> {
        self.nodes.iter().find(|n| n.place == place)
    }

    /// What the tree's own config lists.
    pub fn listing(&self) -> Listing {
        Listing {
            asked: false,
            dir: self.dir.clone(),
            projects: self
                .top()
                .map(|node| Listed {
                    name: node.project.name.clone(),
                    path: node.own.clone(),
                    repo: node.repo(),
                    meta: node.project.meta,
                    // a request's members don't carry it: Listing::of does
                    branch: None,
                    branch_problem: None,
                })
                .collect(),
        }
    }
}

/// A project a meta repo's config lists, as meta read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Listed {
    pub name: String,
    /// Its path from the config's folder
    pub path: String,
    pub repo: Option<RepoUrl>,
    /// Declared `meta: true`
    pub meta: bool,
    /// The branch it starts from (`branch:`); none: its remote's default
    pub branch: Option<String>,
    /// Why its `branch:` names no branch: it isn't cloned
    pub branch_problem: Option<String>,
}

/// What a meta repo's config lists.
#[derive(Debug, Clone)]
pub(crate) struct Listing {
    /// Read back from meta (`project list`), which hides the credentials
    /// in a URL: such a URL is no URL to clone from. Else from the request,
    /// where each URL is as configured.
    pub asked: bool,
    pub dir: PathBuf,
    pub projects: Vec<Listed>,
}

impl Listing {
    /// The projects the config in `dir` lists (`config`, absolute: that one
    /// instead of the default), as meta reads them; `None` when `dir` holds
    /// none. One meta can't read is an error saying why.
    pub fn of(dir: &Path, config: Option<&Path>) -> Result<Option<Listing>> {
        let rows = match project_list(dir, config, false)? {
            Some(rows) => rows,
            None => return Ok(None),
        };
        let projects = rows
            .iter()
            .filter(|row| row.place.is_some())
            .map(|row| Listed {
                name: row.listed.name.clone(),
                path: row.place.clone().unwrap_or_default(),
                repo: row.listed.repo.clone(),
                // declared `meta: true` (a meta repo not cloned yet too)
                meta: row.listed.meta,
                branch: row.listed.branch.clone(),
                branch_problem: row.listed.branch_problem.clone(),
            })
            .collect();
        Ok(Some(Listing {
            asked: true,
            dir: dir.to_path_buf(),
            projects,
        }))
    }
}

/// A project's `branch:` as meta lists it, by its folder.
pub(crate) struct ListedBranch {
    pub abs: PathBuf,
    pub branch: Option<String>,
    pub problem: Option<String>,
}

/// Each project's `branch:` that the config in `dir` lists (`config`, absolute:
/// that one instead of the default; `meta project list` there, nested
/// configs unread).
pub(crate) fn branches(dir: &Path, config: Option<&Path>) -> Result<Vec<ListedBranch>> {
    let rows = project_list(dir, config, false)?.unwrap_or_default();
    Ok(rows
        .into_iter()
        .map(|row| ListedBranch {
            abs: row.abs,
            branch: row.listed.branch,
            problem: row.listed.branch_problem,
        })
        .collect())
}

/// Every folder of the whole workspace `dir` belongs to (`meta -A project
/// list` from there): its root first, then each project's, and what meta
/// couldn't read, with why.
pub(crate) fn whole_tree(dir: &Path) -> Result<(Vec<PathBuf>, Unreadable)> {
    let rows = project_list(dir, None, true)?.unwrap_or_default();
    let dirs = rows.iter().map(|row| row.abs.clone()).collect();
    let unreadable = rows
        .iter()
        .filter_map(|row| Some((row.abs.clone(), row.listed.error.clone()?)))
        .collect();
    Ok((dirs, unreadable))
}

/// The folders whose configs meta couldn't read, each with why.
pub(crate) type Unreadable = Vec<(PathBuf, String)>;

/// One row of `meta --json project list`, with where it is.
struct Row {
    listed: ListedProject,
    abs: PathBuf,
    /// Its path from the folder listed (none for that folder itself)
    place: Option<String>,
}

/// What meta's `project list` gives in `dir`, asked with `--json
/// --no-parent` (and `--config CONFIG` when given; with `all`, `-A` in place
/// of `--no-parent`): its rows, the folder's own first; `None` when `dir`
/// holds no config.
fn project_list(dir: &Path, config: Option<&Path>, all: bool) -> Result<Option<Vec<Row>>> {
    // -A climbs to the outermost config itself; it takes no --no-parent
    // (`dir` is a workspace's folder, never a set's)
    let mut args: Vec<std::ffi::OsString> = match all {
        true => vec!["-A".into()],
        false => vec!["--no-parent".into()],
    };
    if let Some(config) = config {
        args.extend(["--config".into(), config.as_os_str().to_owned()]);
    }
    args.extend(["project".into(), "list".into()]);
    let asked = || {
        format!(
            "Cannot ask meta for the projects listed in {}",
            escape_path(dir)
        )
    };
    let envelope = meta_plugin::client::Meta::from_env()
        .and_then(|meta| meta.cwd(dir).args(args).json())
        .and_then(meta_plugin::client::Running::wait)
        .with_context(asked)?;
    if let Some(error) = envelope.error {
        if error.code == ErrorCode::NotFound {
            return Ok(None);
        }
        anyhow::bail!("{}", error.message);
    }
    let mut rows = Vec::new();
    let mut listed: Option<PathBuf> = None;
    for result in envelope.results {
        let row: Option<ListedProject> = result
            .data
            .map(serde_json::from_value)
            .transpose()
            .with_context(asked)?;
        let row = row.with_context(|| {
            format!(
                "The call of meta gave a project without its row ({})",
                escape(result.project.as_deref().unwrap_or_default())
            )
        })?;
        let abs = result.path.map(PathBuf::from).with_context(|| {
            format!(
                "The call of meta named a project without its folder ({})",
                escape(&row.name)
            )
        })?;
        // the first row is the folder listed; the others are named from it
        let place = match &listed {
            None => {
                listed = Some(abs.clone());
                None
            }
            Some(top) => {
                Some(place(&abs, top).unwrap_or_else(|| abs.to_string_lossy().into_owned()))
            }
        };
        rows.push(Row {
            listed: row,
            abs,
            place,
        });
    }
    Ok(Some(rows))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Have meta-git call the `meta` built beside the tests (in the target
    /// profile folder), as meta would set `META_EXE` for it. Each
    /// test that reads a config runs in a process of its own (`isolated!`).
    pub(crate) fn use_built_meta() {
        let meta = meta_plugin::testing::built_program_dir()
            .join(format!("meta{}", std::env::consts::EXE_SUFFIX));
        assert!(
            meta.is_file(),
            "{} is not built: cargo build --workspace --bins first",
            meta.display()
        );
        std::env::set_var(meta_plugin::client::EXE_VAR, meta);
    }

    fn member(root: &Path, path: &str, meta: bool) -> Project {
        Project {
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            path: path.to_string(),
            abs: root.join(path),
            meta,
            present: true,
            ..Project::default()
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_rows_meta_lists_are_read_back_typed() {
        use std::os::unix::fs::PermissionsExt;
        use_built_meta();
        let sandbox = meta_plugin::testing::isolated!();
        let root = sandbox.work().canonicalize().unwrap();
        std::fs::write(
            root.join(".meta.yaml"),
            "projects:\n  full:\n    repo: git@x:o/full.git\n    tags: [a, b]\n    meta: true\n  \
             bare:\n    path: libs/bare\n",
        )
        .unwrap();
        // bare holds a config of its own; full can't be looked into
        std::fs::create_dir_all(root.join("libs/bare")).unwrap();
        std::fs::write(root.join("libs/bare/.meta.yaml"), "projects: {}\n").unwrap();
        std::fs::create_dir(root.join("full")).unwrap();
        let locked = std::fs::Permissions::from_mode(0o000);
        std::fs::set_permissions(root.join("full"), locked).unwrap();
        let rows = project_list(&root, None, false);
        let open = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(root.join("full"), open).unwrap();
        let rows: Vec<(ListedProject, Option<String>)> = rows
            .unwrap()
            .unwrap()
            .into_iter()
            .skip(1)
            .map(|row| (row.listed, row.place))
            .collect();
        let error = format!(
            "cannot look into {}: Permission denied (os error 13)",
            escape_path(&root.join("full"))
        );
        assert_eq!(
            rows,
            [
                (
                    ListedProject {
                        name: "bare".into(),
                        path: "libs/bare".into(),
                        repo: None,
                        tags: Vec::new(),
                        is_meta: true,
                        meta: false,
                        branch: None,
                        branch_problem: None,
                        error: None,
                        cwd: None,
                        worktree: None,
                    },
                    Some("libs/bare".into())
                ),
                (
                    ListedProject {
                        name: "full".into(),
                        path: "full".into(),
                        repo: Some(RepoUrl::from("git@x:o/full.git")),
                        tags: vec!["a".into(), "b".into()],
                        is_meta: false,
                        meta: true,
                        branch: None,
                        branch_problem: None,
                        error: Some(error),
                        cwd: None,
                        worktree: None,
                    },
                    Some("full".into())
                ),
            ]
        );
    }

    #[test]
    fn a_tree_names_each_project_by_its_place_and_its_meta_repos_above() {
        let root = Path::new("/w");
        let members = vec![
            member(root, ".", false),
            member(root, "app", false),
            member(root, "vendor", true),
            member(root, "vendor/libs/sdk", false),
            member(root, "vendor/sub", true),
            member(root, "vendor/sub/deep", false),
        ];
        let tree = Tree::of(&members, root, true);
        let shown: Vec<(&str, &str, usize)> = tree
            .nodes
            .iter()
            .map(|n| (n.place.as_str(), n.own.as_str(), n.level))
            .collect();
        assert_eq!(
            shown,
            [
                ("app", "app", 0),
                ("vendor", "vendor", 0),
                ("vendor/libs/sdk", "libs/sdk", 1),
                ("vendor/sub", "sub", 1),
                ("vendor/sub/deep", "deep", 2),
            ]
        );
        assert_eq!(tree.within(Some(1)).count(), 4);
        // from a nested config, its own projects are its top
        let nested = Tree::of(&members, &root.join("vendor"), false);
        let top: Vec<&str> = nested.top().map(|n| n.place.as_str()).collect();
        assert_eq!(top, ["libs/sdk", "sub"]);
    }
}
