//! Pins: the revision a project builds a dependency at, written on that
//! `depends_on` edge (`- open-source/gitkb/core: <sha or tag>`).
//!
//! A pin is the consumer's, as a DEPS entry is the Chromium checkout's
//! (gclient) and a gitlink the superproject's (git submodules): another
//! consumer of the same project may need another revision. So pins apply in
//! a worktree set, which is made for one task, and never in the workspace's
//! own checkouts, which every consumer shares and which follow their
//! branches (`meta git update` pulls them). In a set, a repo starts at its
//! pin (`meta git worktree create` and `add`) and `meta git update` moves it
//! there when the pin changes; `meta git worktree status` says where it is.
//!
//! Moving a checkout protects what git protects and rewrites no branch of
//! the user's: see [`bring_to_pin`].

use anyhow::Result;
use meta_cli::dependency_graph::{DependencyGraph, ProjectDependencies};
use meta_core::config::Workspace;
use meta_core::display::{escape, escape_path, shell_quote};
use meta_core::git::{lookup, run};
use meta_git_lib::worktree::plan::Index;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

/// The pin a set applies to one of its repos.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Pin {
    /// A full commit SHA or a tag
    pub rev: String,
    /// The members of the set that pin it there, by place
    pub by: Vec<String>,
}

impl Pin {
    /// The pin as said to the user: `app-sdk-v0.1.39 (pinned by clients/desktop)`.
    pub(crate) fn shown(&self) -> String {
        format!(
            "{} (pinned by {})",
            escape(&self.short()),
            escape(&self.by.join(", "))
        )
    }

    /// A SHA's first 12 digits, or the tag.
    fn short(&self) -> String {
        match self.is_commit() {
            true => self.rev.chars().take(12).collect(),
            false => self.rev.clone(),
        }
    }

    /// Whether it is a commit SHA (else a tag: the config refuses anything
    /// else, see `pin_problem` in meta_core's config).
    fn is_commit(&self) -> bool {
        [40, 64].contains(&self.rev.len()) && self.rev.chars().all(|c| c.is_ascii_hexdigit())
    }
}

/// The pins on the repos of a set whose members are at `places` (from the
/// set's root): a repo is pinned when a member's `depends_on` pins it.
/// Members pinning one repo at different revisions are an error naming
/// each: like gclient's "specified more than once", never a pick (west
/// takes the first manifest's, which here would depend on walk order).
pub(crate) fn pins_in_set(
    graph: &DependencyGraph,
    places: &BTreeSet<String>,
) -> Result<BTreeMap<String, Pin>> {
    let mut pins = BTreeMap::new();
    let mut clashes = Vec::new();
    for place in places {
        let Some(on) = graph.pins_on(place) else {
            continue;
        };
        let by: BTreeMap<&String, &String> =
            on.iter().filter(|(c, _)| places.contains(*c)).collect();
        let revs: BTreeSet<&String> = by.values().copied().collect();
        match revs.into_iter().collect::<Vec<_>>().as_slice() {
            [] => {}
            [rev] => {
                pins.insert(
                    place.clone(),
                    Pin {
                        rev: rev.to_string(),
                        by: by.keys().map(|c| c.to_string()).collect(),
                    },
                );
            }
            _ => clashes.push(format!(
                "'{}' is pinned {}",
                escape(place),
                by.iter()
                    .map(|(c, rev)| format!("at {} by '{}'", escape(rev), escape(c)))
                    .collect::<Vec<_>>()
                    .join(" and ")
            )),
        }
    }
    anyhow::ensure!(
        clashes.is_empty(),
        "The repos of this worktree set disagree on pins: {}. A set has one checkout of \
         each repo; make their depends_on pin the same revision.",
        clashes.join("; ")
    );
    Ok(pins)
}

/// The commit `pin` names in the repository at `repo`, if it has it.
pub(crate) fn pinned_commit(repo: &Path, pin: &Pin) -> Result<Option<String>> {
    let rev = match pin.is_commit() {
        true => format!("{}^{{commit}}", pin.rev),
        false => format!("refs/tags/{}^{{commit}}", pin.rev),
    };
    lookup(repo, ["rev-parse", "--verify", "--quiet", &rev])
}

/// The commit `pin` names, fetched from `origin` into the repository at
/// `repo` if it isn't there: only that commit or tag (as west's `--fetch
/// smart` and repo's "already have persistent ref" skip the fetch for one
/// they have). A shallow repository stays shallow and a full one full:
/// `--depth` would make a full clone, and all its worktrees, shallow.
pub(crate) fn fetch_pin(repo: &Path, place: &str, pin: &Pin) -> Result<String> {
    if let Some(commit) = pinned_commit(repo, pin)? {
        return Ok(commit);
    }
    let shallow = run(repo, ["rev-parse", "--is-shallow-repository"])? == "true";
    let tag = format!("+refs/tags/{0}:refs/tags/{0}", pin.rev);
    let wanted = match pin.is_commit() {
        true => pin.rev.as_str(),
        false => tag.as_str(),
    };
    let mut args = vec!["fetch", "--quiet", "--no-tags"];
    if shallow {
        args.extend(["--depth", "1"]);
    }
    args.extend(["origin", wanted]);
    let fetched = run(repo, &args);
    match (fetched, pinned_commit(repo, pin)?) {
        (_, Some(commit)) => Ok(commit),
        (Err(e), None) => Err(e.context(format!(
            "Cannot fetch pin {} of '{}' from origin",
            pin.shown(),
            escape(place)
        ))),
        (Ok(_), None) => anyhow::bail!(
            "Pin {} of '{}' is not on origin",
            pin.shown(),
            escape(place)
        ),
    }
}

/// The commits reachable from `rev` that no remote-tracking branch, tag or
/// other local branch holds (`except`: the branch `rev` is, which doesn't
/// count), newest first, one line each: the commits git would call lost.
/// Commits only a reflog holds are not among them: git counts those
/// discarded, and expires them itself.
pub(crate) fn unpublished(repo: &Path, rev: &str, except: Option<&str>) -> Result<Vec<String>> {
    let exclude = except.map(|branch| format!("--exclude={branch}"));
    let mut args = vec!["log", "--oneline", rev, "--not", "--remotes", "--tags"];
    args.extend(exclude.as_deref());
    args.push("--branches");
    Ok(run(repo, &args)?.lines().map(str::to_string).collect())
}

/// What [`bring_to_pin`] did to a checkout.
#[derive(Debug, PartialEq)]
pub(crate) enum Moved {
    /// It was at the pin already
    AtPin,
    /// It was moved there from this commit
    From(String),
    /// In a preview: what a run would do
    Would(String),
    /// It was left where it is: why, and what to do
    Left(String),
}

/// Move the checkout at `checkout` (the set's member at `place`) to `pin`,
/// if that loses and rewrites nothing; `set_branch`: the branch the set
/// made for it, if it made one. Every rule refuses, naming the fix, rather
/// than guess:
///
/// - Changes to tracked files, or a merge, rebase or the like under way,
///   keep it where it is (as repo's `contains uncommitted changes` and
///   gclient's `_CheckClean`, without their `--force`). Untracked files
///   don't: git refuses to overwrite one, and moves past the rest.
/// - A branch the set didn't make is never moved (west leaves branches
///   alone too): it is the user's.
/// - The set's own branch is fast-forwarded when the pin is ahead of it, or
///   else moved to the pin when every commit on it is on a remote, a tag or
///   another branch: what `meta git worktree remove` would delete it under.
///   A detached HEAD is moved under that rule too. A commit only it holds
///   keeps it (west and git submodules leave such commits to the reflog;
///   meta, like `git branch -d`, refuses).
/// - A pin it doesn't have is fetched only once these local checks pass.
///
/// `preview`: change nothing, fetch nothing, and say what a run would do.
pub(crate) fn bring_to_pin(
    checkout: &Path,
    place: &str,
    pin: &Pin,
    set_branch: Option<&str>,
    preview: bool,
) -> Result<Moved> {
    let at = |rev: &str| lookup(checkout, ["rev-parse", "--verify", "--quiet", rev]);
    let head = at("HEAD^{commit}")?
        .ok_or_else(|| anyhow::anyhow!("{} has no commit checked out", escape_path(checkout)))?;
    if pinned_commit(checkout, pin)?.as_deref() == Some(head.as_str()) {
        return Ok(Moved::AtPin);
    }
    let quoted = escape(&shell_quote(&checkout.to_string_lossy())).into_owned();
    let branch = meta_core::git::current_branch(checkout)?;
    let short = |commit: &str| commit.chars().take(12).collect::<String>();
    let here = match &branch {
        Some(b) => format!("{} at {}", escape(b), short(&head)),
        None => format!("a detached HEAD at {}", short(&head)),
    };
    let left = |why: String, fix: String| Ok(Moved::Left(format!("{here}: {why}; {fix}")));
    if let Some(operation) = meta_git_lib::worktree::safety::operation_in_progress(checkout)? {
        return left(
            format!("a {operation} is under way"),
            format!("finish or abort it (git -C {quoted} status says how)"),
        );
    }
    if !run(checkout, ["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return left(
            "it has uncommitted changes".to_string(),
            "commit or discard them, then run `meta git update` again".to_string(),
        );
    }
    if let Some(b) = &branch {
        if set_branch != Some(b.as_str()) {
            return left(
                format!(
                    "the set didn't make branch {}, so meta leaves it alone",
                    escape(b)
                ),
                match set_branch {
                    Some(own) => format!(
                        "switch back to the set's branch (git -C {quoted} switch {}) and run \
                         `meta git update` again",
                        escape(own)
                    ),
                    None => format!(
                        "check out the pin yourself: git -C {quoted} switch --detach {}",
                        escape(&pin.rev)
                    ),
                },
            );
        }
    }
    let pinned = match (preview, pinned_commit(checkout, pin)?) {
        (_, Some(commit)) => commit,
        (true, None) => {
            return Ok(Moved::Would(
                "fetch the pin from origin, then move there unless that loses a commit".to_string(),
            ))
        }
        (false, None) => fetch_pin(checkout, place, pin)?,
    };
    // the pin is ahead when HEAD has no commit it lacks
    let ahead = run(
        checkout,
        ["rev-list", "--count", &format!("{pinned}..HEAD")],
    )? == "0";
    if !ahead {
        let lost = unpublished(checkout, "HEAD", branch.as_deref())?;
        if let Some(first) = lost.first() {
            return left(
                format!(
                    "{} on no remote, tag or other branch (first: {})",
                    match lost.len() {
                        1 => "1 commit".to_string(),
                        n => format!("{n} commits"),
                    },
                    escape(first)
                ),
                format!(
                    "push them or put them on a branch of their own (git -C {quoted} branch \
                     <name>), then run `meta git update` again"
                ),
            );
        }
    }
    if preview {
        return Ok(Moved::Would(match (&branch, ahead) {
            (Some(b), true) => format!("fast-forward {} to the pin", escape(b)),
            (Some(b), false) => format!("move {} to the pin", escape(b)),
            (None, _) => "check out the pin".to_string(),
        }));
    }
    match (&branch, ahead) {
        (Some(_), true) => run(checkout, ["merge", "--ff-only", "--quiet", &pinned])?,
        (Some(b), false) => run(checkout, ["checkout", "--quiet", "-B", b, &pinned])?,
        (None, _) => run(checkout, ["checkout", "--quiet", "--detach", &pinned])?,
    };
    Ok(Moved::From(head))
}

/// The dependency graph of the whole tree (`tree`, every project by its
/// place), named by place. A project's `depends_on` and `provides` are
/// qualified with the place of the meta repo that lists it, so they name
/// places too: `depends_on: [vendor/x]` of `open-source/gitkb/core` is
/// `open-source/gitkb/vendor/x`.
pub(crate) fn nested_dep_graph(
    tree: &HashMap<String, (std::path::PathBuf, meta_core::config::ProjectInfo)>,
) -> DependencyGraph {
    let mut project_deps = Vec::new();
    for (full_path, (_, info)) in tree {
        // full_path="open-source/gitkb/core", info.path="core" → "open-source/gitkb/"
        let parent_prefix = full_path
            .strip_suffix(info.path.as_str())
            .filter(|prefix| prefix.is_empty() || prefix.ends_with('/'))
            .unwrap_or("");
        let qualify = |name: &str| {
            if tree.contains_key(name) || name == "." {
                name.to_string()
            } else {
                format!("{parent_prefix}{name}")
            }
        };
        project_deps.push(ProjectDependencies {
            name: full_path.clone(),
            path: full_path.clone(),
            repo: info.repo.clone(),
            tags: info.tags.clone(),
            provides: info.provides.iter().map(|p| qualify(p)).collect(),
            depends_on: info.depends_on.iter().map(|d| qualify(d)).collect(),
            pins: info
                .pins
                .iter()
                .map(|(d, pin)| (qualify(d), pin.clone()))
                .collect(),
        });
    }
    // in the order of the places, so a run picks the provider the last did
    project_deps.sort_by(|a, b| a.name.cmp(&b.name));
    DependencyGraph::build(project_deps)
}

/// The pins on the members at `places` of a set of `workspace` (see
/// [`pins_in_set`]), read from every `.meta.yaml` of its tree: the pin
/// lives in the config that lists its consumer, as `depends_on` does.
pub(crate) fn of_set(
    workspace: &Workspace,
    places: impl IntoIterator<Item = String>,
) -> Result<BTreeMap<String, Pin>> {
    let index = Index::recursive(workspace)?;
    let graph = index
        .tree()
        .map(nested_dep_graph)
        .unwrap_or_else(|| DependencyGraph::build(Vec::new()));
    pins_in_set(&graph, &places.into_iter().collect())
}

/// Where a new branch for the set member at `place` (the checkout `source`,
/// on `branch`) starts because of its pin: the pinned commit, fetched if
/// `fetch` (else `None` for one it would fetch), and the pin as said. None
/// when it isn't pinned, or when `from_ref` was given, which starts every
/// new branch (said once by the caller). A branch that exists already is
/// checked out as it is (a pin starts new branches; it moves none in
/// create or add): a warning says so.
pub(crate) fn start(
    pins: &BTreeMap<String, Pin>,
    place: &str,
    source: &Path,
    branch: &str,
    from_ref: Option<&str>,
    fetch: bool,
) -> Result<Option<(Option<String>, String)>> {
    let Some(pin) = pins.get(place).filter(|_| from_ref.is_none()) else {
        return Ok(None);
    };
    for existing in [
        format!("refs/heads/{branch}"),
        format!("refs/remotes/origin/{branch}"),
    ] {
        if lookup(source, ["rev-parse", "--verify", "--quiet", &existing])?.is_some() {
            meta_core::out::warn(format!(
                "'{}': branch {} exists, so it is checked out as it is, not at pin {}",
                escape(place),
                escape(branch),
                pin.shown()
            ));
            return Ok(None);
        }
    }
    let commit = match fetch {
        true => Some(fetch_pin(source, place, pin)?),
        false => pinned_commit(source, pin)?,
    };
    Ok(Some((commit, pin.shown())))
}
