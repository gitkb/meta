//! What the host sends a plugin: the invocation, resolved. The host finds
//! the workspace (honouring `--config` and `--primary`), applies meta's
//! selection options once, and sends the result; a plugin never reads
//! `.meta.yaml` to find its repos, never looks at the process's working
//! directory, and never sees meta's option words.

use crate::wire_path;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One plugin call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// The plugin command the user named (`git worktree create`): the
    /// words the plugin claims. Everything typed after them is in `args`.
    pub command: String,
    /// The words typed after the command, for the plugin to parse.
    #[serde(default)]
    pub args: Vec<String>,
    /// Where the command runs from: the directory meta was started in, or
    /// with `--primary` inside a worktree set, the same place in the
    /// workspace the set was made from.
    #[serde(with = "wire_path")]
    pub cwd: PathBuf,
    /// The workspace the command runs in; none when there is none (a
    /// clone, a command run outside any).
    #[serde(default)]
    pub workspace: Option<Workspace>,
    /// Where worktree sets go and what runs around them, as the host
    /// resolved them for this command: outside a workspace too.
    #[serde(default)]
    pub worktree: Worktree,
    /// What the host resolved for this command.
    pub scope: Scope,
    /// The worktree set the command runs in, when it runs inside one, or
    /// the one it names (`scope = named-set`).
    #[serde(default)]
    pub set: Option<Set>,
    /// The selected projects, in order: meta's selection (`-r`, `-A`,
    /// `--depth`, `--include`, `--exclude`, `--tag`) applied. Empty is a
    /// selection of none, never "find them yourself".
    #[serde(default)]
    pub projects: Vec<Project>,
    /// How many levels of nested meta repos the selection reaches below
    /// the workspace (0 without `-r`/`-A`, none for no limit): for a command
    /// that clones repos and then their nested ones.
    #[serde(default)]
    pub depth: Option<usize>,
    /// Whether the selection reaches into nested meta repos (`-r` or `-A`),
    /// even no levels deep (`--depth 0`)
    #[serde(default)]
    pub recursive: bool,
    /// A directory for the plugin's own files (`~/.meta/data/<plugin>`),
    /// made by the host, readable by the user alone.
    #[serde(with = "wire_path")]
    pub data_dir: PathBuf,
    /// What of meta's options a plugin may act on.
    pub meta: Meta,
    /// meta's options and what was typed of them, for
    /// [`crate::parse_args`]'s tip when one is typed after the command.
    #[cfg(feature = "parse")]
    #[serde(default)]
    pub meta_options: crate::args::MetaOptions,
    /// (read only with the `parse` feature)
    #[cfg(not(feature = "parse"))]
    #[expect(dead_code, reason = "sent for plugins that parse; this one doesn't")]
    #[serde(default, skip_serializing)]
    meta_options: serde_json::Value,
}

/// meta's options as a plugin may act on them, resolved. Selection is not
/// here: it reaches a plugin only as [`Request::projects`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    /// `--json`: the host prints one Envelope of what the plugin returns;
    /// the plugin's stdout goes to meta's stderr.
    #[serde(default)]
    pub json: bool,
    /// `--dry-run`: change nothing, say what would change.
    #[serde(default)]
    pub dry_run: bool,
    /// `--strict`: warnings are errors.
    #[serde(default)]
    pub strict: bool,
    #[serde(default)]
    pub verbose: bool,
    /// `--silent`: discard the output of the programs meta runs; a plugin
    /// command that answers for itself still prints.
    #[serde(default)]
    pub silent: bool,
    /// Whether output is coloured.
    #[serde(default)]
    pub color: bool,
    /// Whether meta's stdin is a terminal (a plugin may prompt).
    #[serde(default)]
    pub interactive: bool,
    /// How many steps run at once: 1 is one at a time (`--sequential`),
    /// none is as many as there are.
    #[serde(default)]
    pub jobs: Option<usize>,
}

impl Meta {
    /// `dry_run` as the mode a command runs in.
    pub fn mode(&self) -> crate::ExecutionMode {
        crate::ExecutionMode::from_dry_run(self.dry_run)
    }

    /// Whether steps may run at once.
    pub fn parallel(&self) -> bool {
        self.jobs != Some(1)
    }
}

/// What the host resolved before the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    /// No workspace needed (a clone makes one)
    None,
    /// The workspace, with no selection applied: the command finds what it
    /// acts on in `workspace.members` (worktree commands)
    Workspace,
    /// The workspace, and the projects selected in it
    Selection,
    /// The worktree set the command names (`set`), and the checkouts
    /// selected in it
    NamedSet,
}

/// A meta workspace, as the host read it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    /// The folder of the outermost `.meta.yaml` the workspace is nested in
    /// (itself, when it is not nested): projects' `path` starts here.
    #[serde(with = "wire_path")]
    pub root: PathBuf,
    /// The config the command runs from (`--config`'s, or the nearest)
    #[serde(with = "wire_path")]
    pub config: PathBuf,
    /// The folder of that config: where its projects are listed
    #[serde(with = "wire_path")]
    pub listed_root: PathBuf,
    /// Every project of the workspace's tree, nested meta repos' too, from
    /// its outermost config (as far as its configs can be read), whether
    /// selected or cloned or not: never narrowed by meta's selection options.
    /// Inside a set, every member of the set.
    #[serde(default)]
    pub members: Vec<Project>,
}

/// Where worktree sets go, and what runs around them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Worktree {
    /// The folder sets are made in: `META_WORKTREES`, else the workspace's
    /// `worktrees_dir`, else `.worktrees` in its folder; outside a workspace,
    /// `META_WORKTREES`, else `.worktrees` where the command runs. None when
    /// the config's place for them is refused (the worktree commands say why)
    #[serde(default, with = "wire_path::option")]
    pub dir: Option<PathBuf>,
    /// Why there is no `dir`: the config's place for sets was refused (a
    /// `.worktrees` link into `.git`, say), in words a command shows as its
    /// error
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// The shell command for each worktree event (`post-create`,
    /// `post-destroy`, `post-prune`), from `worktree.hooks`
    #[serde(default)]
    pub hooks: std::collections::BTreeMap<String, String>,
    /// The folder of the config these settings come from (inside a set, the
    /// workspace the set was made from): hooks run there
    #[serde(default, with = "wire_path::option")]
    pub home: Option<PathBuf>,
}

/// A worktree set: a folder holding a checkout of some of the workspace's
/// projects at their paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Set {
    pub name: String,
    #[serde(with = "wire_path")]
    pub dir: PathBuf,
    /// Its members: the workspace's projects whose folder is in it (at
    /// their path, `abs` inside `dir`), the root when the set's folder is a
    /// checkout of its own. Which of them are checkouts of a provider's, the
    /// provider tells; meta looks for folders only.
    #[serde(default)]
    pub members: Vec<Project>,
    /// The provider that made it (see [`crate::sets::Owner`]): `git` for a
    /// set with no owner file
    pub provider: String,
    /// Whether its creator holds its lease now
    #[serde(default)]
    pub leased: bool,
}

/// A project of the workspace.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// Its name in its `.meta.yaml` (`.` for a workspace's own repo)
    pub name: String,
    /// Its path from the workspace root (`.` for the root), as `--include`
    /// takes it and as the Envelope names it
    pub path: String,
    /// Its folder
    #[serde(with = "wire_path")]
    pub abs: PathBuf,
    /// Its `repo:` URL, when it has one
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// What it depends on, resolved by the host within the command's reach
    /// (the config's own projects; with `-r`/`-A`, the tree it walks): each
    /// entry of its config's `depends_on` as the `path` of the project it
    /// names (by name, by place, or through what a project `provides`), or,
    /// when it names none there, as written (qualified with the place of
    /// the meta repo that lists it, with `-r`/`-A`): no member's path
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// What resolving its `depends_on` ran into, one sentence each ("depends
    /// on 'db', which 'a' and 'b' provide; 'b' is used")
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependency_problems: Vec<String>,
    /// Dependencies that name a repo this config's create doesn't follow:
    /// said only when the set won't hold that repo.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left_out: Vec<LeftOut>,
    /// Whether its config says it is a meta repo (`meta: true`): one with a
    /// `.meta.yaml` of its own listing projects
    #[serde(default)]
    pub meta: bool,
    /// The branch it starts from, as its config's `branch:` names it; none
    /// for its remote's default branch
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Its dependency level, when the selection was ordered by dependency
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<usize>,
    /// Whether its folder exists (false: listed but not cloned)
    pub present: bool,
    /// The paths from the workspace root of every meta project under it,
    /// at any depth (for a project that is a meta repo itself)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nested: Vec<String>,
}

impl Request {
    /// A request for `command` from `cwd` with nothing resolved: no
    /// workspace, no projects, meta's defaults. Fill in what a test or a
    /// caller needs.
    pub fn new(command: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        Request {
            command: command.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            workspace: None,
            worktree: Worktree::default(),
            scope: Scope::None,
            set: None,
            projects: Vec::new(),
            depth: Some(0),
            recursive: false,
            data_dir: PathBuf::new(),
            meta: Meta::default(),
            #[cfg(feature = "parse")]
            meta_options: crate::args::MetaOptions::default(),
            #[cfg(not(feature = "parse"))]
            meta_options: serde_json::Value::Null,
        }
    }

    /// The folders of the selected projects, in order.
    pub fn dirs(&self) -> Vec<&Path> {
        self.projects.iter().map(|p| p.abs.as_path()).collect()
    }

    /// The workspace, or the error a command that needs one gives.
    pub fn workspace(&self) -> anyhow::Result<&Workspace> {
        self.workspace.as_ref().ok_or_else(|| {
            crate::ErrorInfo::new(
                crate::ErrorCode::NotFound,
                "Not a meta workspace (no .meta.yaml found)",
            )
            .into()
        })
    }
}

/// A dependency a create doesn't follow, which names one repo: that repo,
/// by its path from the workspace root, and what to say when the set won't
/// hold it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeftOut {
    pub repo: String,
    pub problem: String,
}
