//! Where worktree sets go has one answer: meta_core's, which meta sends in
//! the request and `meta git worktree list` reads, from every place a
//! command runs and whether or not `META_WORKTREES` is set.

#![expect(
    clippy::unwrap_used,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]

use meta_plugin::sets::WORKTREES_VAR;
use std::path::{Path, PathBuf};

/// The folder of sets meta_core resolves for a command run from `cwd`, from
/// the config meta reads there: inside a set, the set's.
fn resolved(cwd: &Path) -> PathBuf {
    use meta_core::{config::Workspace, worktree};
    let config = match worktree::detect_worktree_location(cwd).unwrap() {
        Some((_, set)) => worktree::set_config(&set, None)
            .unwrap()
            .map(|path| Workspace::load(&path).unwrap()),
        None => Workspace::find(cwd, None).unwrap(),
    };
    worktree::plugin_worktree(config.as_ref(), cwd).dir.unwrap()
}

#[test]
fn every_place_has_one_folder_of_sets() {
    let sandbox = meta_plugin::testing::isolated!();
    let work = sandbox.work().canonicalize().unwrap();
    let ws = sandbox.workspace(&work.join("ws")).repo("app").build();
    let (elsewhere, outside) = (work.join("elsewhere"), work.join("outside"));
    std::fs::create_dir_all(&outside).unwrap();
    // meta and meta-git as built (`cargo build --workspace --bins` first)
    let bin = Path::new(env!("CARGO_BIN_EXE_meta-git")).parent().unwrap();
    let meta = bin.join(format!("meta{}", std::env::consts::EXE_SUFFIX));
    assert!(meta.is_file(), "{} is not built", meta.display());
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(bin.to_path_buf()).chain(std::env::split_paths(&path)),
    )
    .unwrap();
    let run = |cwd: &Path, var: Option<&Path>, args: &[&str]| {
        let mut command = sandbox.command(&meta);
        command.args(args).current_dir(cwd).env("PATH", &path);
        if let Some(var) = var {
            command.env(WORKTREES_VAR, var);
        }
        let out = command.output().unwrap();
        assert!(out.status.success(), "meta {args:?} in {cwd:?}: {out:?}");
        out.stdout
    };
    // a set in the workspace's folder, and one made where the variable said
    run(
        &ws,
        None,
        &["git", "worktree", "create", "s", "--repo", "app"],
    );
    run(
        &ws,
        Some(&elsewhere),
        &["git", "worktree", "create", "t", "--repo", "app"],
    );
    let (in_s, in_t) = (ws.join(".worktrees/s/app"), elsewhere.join("t/app"));

    let at_home = ws.join(".worktrees");
    // where the command runs, META_WORKTREES, the folder its sets are in
    let table: [(&Path, Option<&Path>, &Path); 7] = [
        (&ws, None, &at_home),
        (&ws, Some(&elsewhere), &elsewhere),
        (&in_s, None, &at_home),
        (&in_s, Some(&elsewhere), &elsewhere),
        // a set made elsewhere, the variable gone: its workspace's folder
        (&in_t, None, &at_home),
        (&outside, None, &outside.join(".worktrees")),
        (&outside, Some(&elsewhere), &elsewhere),
    ];
    for (cwd, var, folder) in table {
        match var {
            Some(var) => std::env::set_var(WORKTREES_VAR, var),
            None => std::env::remove_var(WORKTREES_VAR),
        }
        assert_eq!(resolved(cwd), folder, "meta_core from {cwd:?}, {var:?}");
        // a set in that folder alone is the one `list` finds by name
        let probe = folder.join("probe");
        let checkout = probe.join("app").display().to_string();
        let app = ws.join("app");
        sandbox.git(&app, &["worktree", "add", "-q", "--detach", &checkout]);
        let listed = run(cwd, var, &["--json", "git", "worktree", "list", "probe"]);
        sandbox.git(&app, &["worktree", "remove", "--force", &checkout]);
        let listed: serde_json::Value = serde_json::from_slice(&listed).unwrap();
        assert_eq!(
            listed["results"][0]["data"]["root"],
            probe.display().to_string(),
            "list from {cwd:?}, {var:?}: {listed}"
        );
    }
}
