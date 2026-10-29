//! Integration tests for meta CLI and plugin behavior
//!
//! Note: Some tests are commented out because they require the meta binary to be built.
//! Uncomment them when running full integration tests.

use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn touch(path: &Path) -> Vec<String> {
    vec!["touch".to_string(), path.display().to_string()]
}

/// Test that loop_lib properly respects dry_run flag throughout the execution chain
#[test]
fn test_dry_run_integration_loop_lib() {
    use loop_lib::{run, run_commands, DirCommand, LoopConfig};

    let dir = tempdir().unwrap();
    let marker_file = dir.path().join("should_not_exist.txt");

    // Test dry_run with run() function
    let config = LoopConfig {
        directories: vec![dir.path().to_str().unwrap().to_string()],
        dry_run: true,
        silent: true,
        ..Default::default()
    };

    let cmd = touch(&marker_file);
    let result = run(&config, &cmd);
    assert!(result.is_ok(), "dry_run should succeed");
    assert!(
        !marker_file.exists(),
        "dry_run should NOT create file via run()"
    );

    // Test dry_run with run_commands() function
    let marker_file2 = dir.path().join("also_should_not_exist.txt");
    let commands = vec![DirCommand {
        dir: dir.path().to_str().unwrap().to_string(),
        argv: touch(&marker_file2),
        env: None,
    }];

    let result = run_commands(&config, &commands);
    assert!(result.is_ok(), "dry_run should succeed");
    assert!(
        !marker_file2.exists(),
        "dry_run should NOT create file via run_commands()"
    );
}

/// Test that LoopConfig properly serializes/deserializes with all new fields
#[test]
fn test_loop_config_full_serialization_round_trip() {
    use loop_lib::LoopConfig;

    let config = LoopConfig {
        directories: vec!["dir1".to_string(), "dir2".to_string()],
        ignore: vec![".git".to_string()],
        verbose: true,
        silent: false,
        add_aliases_to_global_looprc: false,
        parallel: true,
        dry_run: true,
        json_output: true,
        spawn_stagger_ms: 0,
        env: None,
        max_parallel: Some(5),
        root_dir: None,
    };

    let json = serde_json::to_string(&config).unwrap();
    let restored: LoopConfig = serde_json::from_str(&json).unwrap();

    assert_eq!(restored.directories, config.directories);
    assert_eq!(restored.ignore, config.ignore);
    assert_eq!(restored.verbose, config.verbose);
    assert_eq!(restored.silent, config.silent);
    assert_eq!(restored.parallel, config.parallel);
    assert_eq!(restored.dry_run, config.dry_run);
    assert_eq!(restored.json_output, config.json_output);
    assert_eq!(restored.max_parallel, config.max_parallel);
}

/// Test that DirCommand allows different commands per directory
#[test]
fn test_different_commands_per_directory() {
    use loop_lib::{run_commands, DirCommand, LoopConfig};

    let dir = tempdir().unwrap();
    let dir1 = dir.path().join("project_a");
    let dir2 = dir.path().join("project_b");
    fs::create_dir(&dir1).unwrap();
    fs::create_dir(&dir2).unwrap();

    let file1 = dir1.join("file_a.txt");
    let file2 = dir2.join("file_b.txt");

    let config = LoopConfig {
        parallel: false,
        silent: true,
        ..Default::default()
    };

    let commands = vec![
        DirCommand {
            dir: dir1.to_str().unwrap().to_string(),
            argv: touch(&file1),
            env: None,
        },
        DirCommand {
            dir: dir2.to_str().unwrap().to_string(),
            argv: touch(&file2),
            env: None,
        },
    ];

    let result = run_commands(&config, &commands);
    assert!(result.is_ok());
    assert!(file1.exists(), "First command should create file_a.txt");
    assert!(file2.exists(), "Second command should create file_b.txt");
}

/// Test that execution plan JSON format matches what subprocess_plugins expects
#[test]
fn test_execution_plan_protocol_compatibility() {
    use meta_cli::subprocess_plugins::PluginResponse;

    // This JSON format is what meta_git_cli produces
    let plugin_output = r#"{
        "plan": {
            "commands": [
                {"dir": "./repo1", "argv": ["git", "status"]},
                {"dir": "./repo2", "argv": ["git", "status"]}
            ],
            "parallel": false
        }
    }"#;

    // subprocess_plugins should be able to parse it
    let response: PluginResponse = serde_json::from_str(plugin_output).unwrap();

    assert_eq!(response.plan.commands.len(), 2);
    assert_eq!(response.plan.commands[0].dir, "./repo1");
    assert_eq!(response.plan.commands[0].argv, ["git", "status"]);
    assert_eq!(response.plan.commands[1].dir, "./repo2");
    assert_eq!(response.plan.commands[1].argv, ["git", "status"]);
    assert_eq!(response.plan.parallel, Some(false));
}

/// Test that dry_run properly propagates through the plugin request options
#[test]
fn test_plugin_request_options_dry_run_propagation() {
    use meta_cli::subprocess_plugins::PluginRequestOptions;

    // Simulate what happens when --dry-run is passed to meta CLI
    let options = PluginRequestOptions {
        json_output: false,
        verbose: false,
        parallel: false,
        dry_run: true, // User passed --dry-run
        ..Default::default()
    };

    // Serialize as JSON (this is what gets sent to plugin stdin)
    let json = serde_json::to_string(&options).unwrap();
    assert!(json.contains("\"dry_run\":true"));
    assert!(json.contains("\"json_output\":false"));
    assert!(json.contains("\"verbose\":false"));
    assert!(json.contains("\"parallel\":false"));

    // Verify the JSON can be parsed as generic Value (plugin side validation)
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed["dry_run"].as_bool().unwrap());
    assert!(!parsed["json_output"].as_bool().unwrap());
}

/// Test that silent and filter options propagate correctly
#[test]
fn test_plugin_request_options_silent_and_filters() {
    use meta_cli::subprocess_plugins::PluginRequestOptions;

    let options = PluginRequestOptions {
        silent: true,
        include_filters: Some(vec!["frontend".to_string()]),
        exclude_filters: Some(vec!["tests".to_string()]),
        ..Default::default()
    };

    let json = serde_json::to_string(&options).unwrap();
    assert!(json.contains("\"silent\":true"));
    assert!(json.contains("\"include_filters\""));
    assert!(json.contains("frontend"));
    assert!(json.contains("\"exclude_filters\""));
    assert!(json.contains("tests"));
}

/// Test that parallel execution works correctly with different commands
#[test]
fn test_parallel_execution_with_different_commands() {
    use loop_lib::{run_commands, DirCommand, LoopConfig};

    let dir = tempdir().unwrap();
    let dirs: Vec<_> = (0..5)
        .map(|i| {
            let d = dir.path().join(format!("dir{i}"));
            fs::create_dir(&d).unwrap();
            d
        })
        .collect();

    let config = LoopConfig {
        parallel: true,
        silent: true,
        ..Default::default()
    };

    // Each directory gets a different command
    let commands: Vec<DirCommand> = dirs
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let file = d.join(format!("file{i}.txt"));
            DirCommand {
                dir: d.to_str().unwrap().to_string(),
                argv: touch(&file),
                env: None,
            }
        })
        .collect();

    let result = run_commands(&config, &commands);
    assert!(result.is_ok());

    // Verify all files were created
    for (i, d) in dirs.iter().enumerate() {
        let file = d.join(format!("file{i}.txt"));
        assert!(
            file.exists(),
            "File {} should exist after parallel execution",
            file.display()
        );
    }
}

/// Test that empty command list succeeds without errors
#[test]
fn test_empty_execution_plan() {
    use loop_lib::{run_commands, DirCommand, LoopConfig};

    let config = LoopConfig::default();
    let commands: Vec<DirCommand> = vec![];

    let result = run_commands(&config, &commands);
    assert!(result.is_ok(), "Empty command list should succeed");
}

/// Test backward compatibility - old configs without new fields should work
#[test]
fn test_backward_compatibility_config_parsing() {
    use loop_lib::LoopConfig;

    // Old config format without dry_run, json_output, parallel
    let old_config_json = r#"{
        "directories": ["dir1", "dir2"],
        "ignore": [".git"],
        "verbose": false,
        "silent": true,
        "add_aliases_to_global_looprc": false
    }"#;

    let config: LoopConfig = serde_json::from_str(old_config_json).unwrap();

    // New fields should have defaults
    assert!(!config.dry_run);
    assert!(!config.json_output);
    assert!(!config.parallel);
}

/*
// These tests require the meta binary to be built
// Uncomment when running full integration tests with `cargo nextest run`

use std::env;
use std::process::Command;

#[test]
fn test_meta_git_clone_invalid_repo() {
    let dir = tempdir().unwrap();
    let meta_path = dir.path().join(".meta.yaml");
    fs::write(
        &meta_path,
        r#"{
            "projects": {}
        }"#,
    )
    .unwrap();

    let output = Command::new(env::var("CARGO_BIN_EXE_meta").expect("CARGO_BIN_EXE_meta not set"))
        .current_dir(dir.path())
        .args(&["git", "clone", "invalid_repo_url"])
        .output()
        .expect("failed to execute meta git clone");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal") || !output.status.success(),
        "Expected git clone to fail on invalid repo"
    );
}

#[test]
fn test_meta_dry_run_flag() {
    let dir = tempdir().unwrap();
    let meta_path = dir.path().join(".meta.yaml");
    fs::write(
        &meta_path,
        r#"{
            "projects": {}
        }"#,
    )
    .unwrap();

    let marker_file = dir.path().join("marker.txt");

    let output = Command::new(env::var("CARGO_BIN_EXE_meta").expect("CARGO_BIN_EXE_meta not set"))
        .current_dir(dir.path())
        .args(&["--dry-run", "exec", &format!("touch {}", marker_file.display())])
        .output()
        .expect("failed to execute meta with --dry-run");

    assert!(output.status.success(), "meta --dry-run should succeed");
    assert!(!marker_file.exists(), "--dry-run should NOT create file");
}
*/

/// A malformed selected worktree must fail before ordinary config fallback or exec.
#[cfg(unix)]
#[test]
fn malformed_worktree_membership_is_reported_before_dispatch() {
    let temp = tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let worktree = root.join(".worktrees/broken");
    fs::create_dir_all(&worktree).unwrap();
    fs::write(root.join(".meta.yaml"), r#"{"projects":{}}"#).unwrap();
    fs::write(worktree.join(".meta.yaml"), r#"{"projects":{}}"#).unwrap();
    fs::write(worktree.join(".git"), "malformed metadata").unwrap();
    let marker = root.join("unexpected-dispatch");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_meta"))
        .args(["exec", "--", "/usr/bin/touch", marker.to_str().unwrap()])
        .current_dir(&worktree)
        .env("HOME", &root)
        .env("XDG_CONFIG_HOME", root.join("xdg"))
        .env("GIT_CONFIG_GLOBAL", root.join("gitconfig"))
        .env("META_DATA_DIR", root.join("data"))
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Invalid .git file format"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !marker.exists(),
        "invalid worktree dispatched through ordinary config fallback"
    );
}
