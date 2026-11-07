//! Where worktree sets go has one answer: meta_core's, which meta sends in
//! the request and `meta git worktree list` reads, from every place a
//! command runs and whether or not `META_WORKTREES` is set.

#![expect(
    clippy::unwrap_used,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]

use meta_plugin::sets::WORKTREES_VAR;
use std::path::Path;
use std::process::Output;

/// What meta_core tells a plugin of sets for a command run from `cwd`, from
/// the config meta reads there: inside a set, the set's.
fn resolved(cwd: &Path) -> meta_plugin::Worktree {
    use meta_core::{config::Workspace, worktree};
    let location = worktree::detect_worktree_location(cwd).unwrap();
    let config = match &location {
        Some((_, set)) => worktree::set_config(set, None)
            .unwrap()
            .map(|path| Workspace::load(&path).unwrap()),
        None => Workspace::find(cwd, None).unwrap(),
    };
    let set = location.as_ref().map(|(_, dir)| dir.as_path());
    worktree::plugin_worktree(config.as_ref(), cwd, set)
}

/// `var` as the value of `META_WORKTREES` in this process; none unsets it.
fn set_var(var: Option<&str>) {
    match var {
        Some(var) => std::env::set_var(WORKTREES_VAR, var),
        None => std::env::remove_var(WORKTREES_VAR),
    }
}

#[test]
fn every_place_has_one_folder_of_sets() {
    let sandbox = meta_plugin::testing::isolated!();
    // paths as given, never canonical: git can't make a worktree at a
    // Windows `\\?\` path; compared as they really are
    let real = |path: &Path| path.canonicalize().unwrap();
    let work = sandbox.work();
    let ws = sandbox.workspace(&work.join("ws")).repo("app").build();
    let (elsewhere, outside) = (work.join("elsewhere"), work.join("outside"));
    for dir in [&elsewhere, &outside.join(".worktrees")] {
        std::fs::create_dir_all(dir).unwrap();
    }
    // meta and meta-git as built (`cargo build --workspace --bins` first)
    let bin = Path::new(env!("CARGO_BIN_EXE_meta-git")).parent().unwrap();
    let meta = bin.join(format!("meta{}", std::env::consts::EXE_SUFFIX));
    assert!(meta.is_file(), "{} is not built", meta.display());
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(bin.to_path_buf()).chain(std::env::split_paths(&path)),
    )
    .unwrap();
    let run = |cwd: &Path, var: Option<&str>, args: &[&str]| -> Output {
        let mut command = sandbox.command(&meta);
        command.args(args).current_dir(cwd).env("PATH", &path);
        if let Some(var) = var {
            command.env(WORKTREES_VAR, var);
        }
        command.output().unwrap()
    };
    let ran = |cwd: &Path, var: Option<&str>, args: &[&str]| {
        let out = run(cwd, var, args);
        assert!(out.status.success(), "meta {args:?} in {cwd:?}: {out:?}");
        out.stdout
    };
    let elsewhere_var = elsewhere.to_str().unwrap();
    // a set in the workspace's folder, and one made where the variable said
    ran(
        &ws,
        None,
        &["git", "worktree", "create", "s", "--repo", "app"],
    );
    let made = ["git", "worktree", "create", "t", "--repo", "app"];
    ran(&ws, Some(elsewhere_var), &made);
    let (in_s, in_t) = (ws.join(".worktrees/s/app"), elsewhere.join("t/app"));

    let at_home = ws.join(".worktrees");
    // a workspace of its own whose sets go in /wanted, in a `.worktrees`
    // folder, with s's owner entry copied beside it: made for another folder
    let (one, wanted) = (work.join("copy/.worktrees/one"), work.join("wanted"));
    std::fs::create_dir_all(one.parent().unwrap().join(".meta-sets")).unwrap();
    std::fs::create_dir_all(&wanted).unwrap();
    sandbox.git(
        &work,
        &[
            "clone",
            "-q",
            ws.join("app").to_str().unwrap(),
            one.join("app").to_str().unwrap(),
        ],
    );
    let config =
        serde_json::json!({"projects": {"app": "git@x:o/app.git"}, "worktrees_dir": wanted});
    std::fs::write(one.join(".meta.yaml"), config.to_string()).unwrap();
    std::fs::copy(
        at_home.join(".meta-sets/s.json"),
        one.parent().unwrap().join(".meta-sets/one.json"),
    )
    .unwrap();
    // where the command runs, META_WORKTREES, the folder its sets are in
    let table: [(&Path, Option<&str>, &Path); 8] = [
        (&ws, None, &at_home),
        (&ws, Some(elsewhere_var), &elsewhere),
        (&in_s, None, &at_home),
        (&in_s, Some(elsewhere_var), &elsewhere),
        // inside a set made elsewhere, the variable gone: its siblings, in
        // the folder it is in
        (&in_t, None, &elsewhere),
        (&outside, None, &outside.join(".worktrees")),
        (&outside, Some(elsewhere_var), &elsewhere),
        // an owner entry made for another folder is no proof of a set
        (&one, None, &wanted),
    ];
    for (cwd, var, folder) in table {
        set_var(var);
        let dir = resolved(cwd).dir.unwrap();
        assert_eq!(real(&dir), real(folder), "meta_core from {cwd:?}, {var:?}");
        // a set in that folder alone is the one `list` finds by name; one
        // of the same name in the workspace's folder is not
        let probe = folder.join("probe");
        let decoy = (folder != at_home).then(|| at_home.join("probe"));
        let app = ws.join("app");
        let checkouts: Vec<String> = std::iter::once(&probe)
            .chain(&decoy)
            .map(|set| set.join("app").display().to_string())
            .collect();
        for checkout in &checkouts {
            sandbox.git(&app, &["worktree", "add", "-q", "--detach", checkout]);
        }
        let listed = ran(cwd, var, &["--json", "git", "worktree", "list", "probe"]);
        let listed: serde_json::Value = serde_json::from_slice(&listed).unwrap();
        let root = listed["results"][0]["data"]["root"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            Path::new(root).canonicalize().ok(),
            Some(real(&probe)),
            "list from {cwd:?}, {var:?}: {listed}"
        );
        for checkout in &checkouts {
            sandbox.git(&app, &["worktree", "remove", "--force", checkout]);
        }
    }

    // an empty or relative META_WORKTREES is refused, naming it, before
    // anything is made
    for var in ["", "rel"] {
        for cwd in [&ws, &ws.join("app"), &in_s] {
            set_var(Some(var));
            let refused = resolved(cwd).refused.unwrap_or_default();
            assert!(
                refused.contains(&format!("'{var}', not an absolute path")),
                "meta_core from {cwd:?}, {var:?}: {refused}"
            );
            let create = ["git", "worktree", "create", "x", "--repo", "app"];
            let out = run(cwd, Some(var), &create);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(
                !out.status.success() && stderr.contains("not an absolute path"),
                "create from {cwd:?}, {var:?}: {out:?}"
            );
            assert!(!cwd.join(var).join("x").exists() && !cwd.join("x").exists());
        }
    }
    set_var(None);
}
