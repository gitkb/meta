//! `defaults` come from the config meta runs with, wherever it was found.

#![expect(
    clippy::disallowed_methods,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]

/// `defaults: {parallel: false}` holds when meta runs below the config, and
/// when `--config` names it over a `.meta.yaml` that runs in parallel: two
/// repos holding one lock at once fail, as `--parallel` shows.
#[cfg(unix)]
#[test]
fn sequential_default_holds_wherever_the_config_is() {
    let projects = "projects:\n  a:\n    path: a\n  b:\n    path: b\n";
    let sequential = format!("defaults:\n  parallel: false\n{projects}");
    for (default_config, chosen, run_in, args, collides) in [
        (sequential.as_str(), None, "a", &["exec"][..], false),
        (
            projects,
            Some("sequential.yaml"),
            ".",
            &["--config", "sequential.yaml", "exec"],
            false,
        ),
        // the control: run in parallel, the two repos collide
        (
            sequential.as_str(),
            None,
            "a",
            &["--parallel", "exec"],
            true,
        ),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let tmp = tmp.path().canonicalize().unwrap();
        let workspace = tmp.join("workspace");
        for project in ["a", "b"] {
            std::fs::create_dir_all(workspace.join(project)).unwrap();
            meta_core::git::run(&workspace.join(project), ["init", "-q"]).unwrap();
        }
        std::fs::write(workspace.join(".meta.yaml"), default_config).unwrap();
        if let Some(name) = chosen {
            std::fs::write(workspace.join(name), &sequential).unwrap();
        }
        let lock = tmp.join("lock");
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_meta"))
            .args(args)
            .args([
                "--",
                "sh",
                "-c",
                r#"mkdir "$1" && sleep 0.5 && rmdir "$1""#,
                "sh",
            ])
            .arg(&lock)
            .current_dir(workspace.join(run_in))
            .env("RAYON_NUM_THREADS", "2")
            .env("HOME", &tmp)
            .env("XDG_CONFIG_HOME", tmp.join("xdg"))
            .env("GIT_CONFIG_GLOBAL", tmp.join("no-gitconfig"))
            .env("META_DATA_DIR", tmp.join("meta-data"))
            .output()
            .unwrap();
        assert_eq!(
            !out.status.success(),
            collides,
            "{args:?}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
