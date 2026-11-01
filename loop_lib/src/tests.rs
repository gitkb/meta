use super::*;
use std::fs;
use tempfile::TempDir;

/// `script` run by the platform shell, the way a user asks for one
fn sh(script: &str) -> Vec<String> {
    let shell: &[&str] = if cfg!(windows) {
        &["cmd", "/c"]
    } else {
        &["sh", "-c"]
    };
    shell
        .iter()
        .map(|s| s.to_string())
        .chain([script.to_string()])
        .collect()
}

/// Cross-platform command that always fails with exit code 1
fn fail_cmd() -> Vec<String> {
    sh("exit 1")
}

/// Returns a cross-platform touch command for the given path
#[cfg(windows)]
fn touch_cmd(path: &std::path::Path) -> Vec<String> {
    // Simple echo redirect works with cmd /c
    // The > creates the file if it doesn't exist
    sh(&format!("echo.>\"{}\"", path.display()))
}
#[cfg(not(windows))]
fn touch_cmd(path: &std::path::Path) -> Vec<String> {
    vec!["touch".to_string(), path.display().to_string()]
}

/// Captured output as text, for assertions
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn test_expand_directories() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path().to_path_buf();
    for dir in ["dir2", "dir1/subdir", ".git", ".github"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    let ignore = vec![".git".to_string()];

    // the root, then its children by name; not .git, not grandchildren
    let expanded = crate::expand_directories(std::slice::from_ref(&root), &ignore).unwrap();
    let names = [".github", "dir1", "dir2"].map(|name| root.join(name));
    assert_eq!(expanded, [&[root.clone()][..], &names].concat());

    // the root excluded, its children still run
    let root_name = root.to_str().unwrap().to_string();
    let expanded = crate::expand_directories(
        std::slice::from_ref(&root),
        &[root_name, ".git".to_string()],
    )
    .unwrap();
    assert_eq!(expanded, names);
}

#[test]
fn test_should_ignore() {
    let ignore = vec![".git".to_string()];
    assert!(should_ignore(Path::new("/some/path/.git"), &ignore));
    assert!(!should_ignore(Path::new("/some/path/normal"), &ignore));

    // whole names, not parts of one
    assert!(!should_ignore(Path::new("/w/.github"), &ignore));
    let ignore = vec!["a".to_string(), "vendor/old".to_string()];
    assert!(should_ignore(Path::new("./a"), &ignore));
    assert!(should_ignore(Path::new("a"), &["./a".to_string()]));
    // `.` is the directory loop runs in, not every directory
    let dot = vec![".".into()];
    assert!(should_ignore(Path::new("."), &dot));
    assert!(!should_ignore(Path::new("./b"), &dot));
    assert!(!should_ignore(Path::new("b"), &[String::new()]));
    assert!(!should_ignore(Path::new("/w/abc"), &ignore));
    assert!(!should_ignore(Path::new("/w/data"), &ignore));
    assert!(should_ignore(Path::new("/w/vendor/old"), &ignore));
    assert!(!should_ignore(Path::new("/w/old"), &ignore));
    // the directories above don't count
    assert!(!should_ignore(Path::new("/a/ws/b"), &ignore));
}

#[test]
fn test_run() {
    let temp_dir = TempDir::new().unwrap();
    let dir1 = temp_dir.path().join("dir1");
    let dir2 = temp_dir.path().join("dir2");
    fs::create_dir(&dir1).unwrap();
    fs::create_dir(&dir2).unwrap();

    let config = LoopConfig {
        directories: vec![dir1.to_path_buf(), dir2.to_path_buf()],
        ignore: vec![],
        verbose: false,
        silent: true,
        parallel: false,
        dry_run: false,
        json_output: false,
        spawn_stagger_ms: 0,
        env: None,
        max_parallel: None,
        root_dir: None,
        ..Default::default()
    };

    let result = run(&config, &sh("echo test"));
    assert!(result.is_ok());

    // Test with a failing command
    let result = run(&config, &fail_cmd());
    assert!(result.is_err()); // The function should return an error if any command fails
}

#[test]
fn test_execute_command_in_directory() {
    let config = LoopConfig {
        verbose: false,
        silent: true,
        ..Default::default()
    };
    let temp_dir = TempDir::new().unwrap();

    let result =
        execute_command_in_directory(temp_dir.path(), &sh("echo test"), &config, None).unwrap();
    assert!(result.success);
    assert_eq!(result.exit_code, Some(0));

    let result = execute_command_in_directory(temp_dir.path(), &fail_cmd(), &config, None).unwrap();
    assert!(!result.success);
    assert_eq!(result.exit_code, Some(1));
}
#[test]
fn test_run_without_looprc() {
    let temp_dir = TempDir::new().unwrap();
    let dir1 = temp_dir.path().join("dir1");
    let dir2 = temp_dir.path().join("dir2");
    fs::create_dir(&dir1).unwrap();
    fs::create_dir(&dir2).unwrap();

    // Run without a .looprc file
    let config = LoopConfig {
        directories: vec![temp_dir.path().to_path_buf()],
        ignore: vec![],
        verbose: false,
        silent: true,
        parallel: false,
        dry_run: false,
        json_output: false,
        spawn_stagger_ms: 0,
        env: None,
        max_parallel: None,
        root_dir: None,
        ..Default::default()
    };

    let result = run(&config, &sh("echo test"));
    assert!(result.is_ok());
}

#[test]
fn test_run_parallel() {
    let temp_dir = TempDir::new().unwrap();
    let dir1 = temp_dir.path().join("dir1");
    let dir2 = temp_dir.path().join("dir2");
    let dir3 = temp_dir.path().join("dir3");
    fs::create_dir(&dir1).unwrap();
    fs::create_dir(&dir2).unwrap();
    fs::create_dir(&dir3).unwrap();

    let config = LoopConfig {
        directories: vec![dir1.to_path_buf(), dir2.to_path_buf(), dir3.to_path_buf()],
        ignore: vec![],
        verbose: false,
        silent: true,
        parallel: true,
        dry_run: false,
        json_output: false,
        spawn_stagger_ms: 0,
        env: None,
        max_parallel: None,
        root_dir: None,
        ..Default::default()
    };

    let result = run(&config, &sh("echo test"));
    assert!(result.is_ok());

    // Test with a failing command in parallel mode
    let result = run(&config, &fail_cmd());
    assert!(result.is_err());
}

#[test]
fn test_execute_command_in_directory_capturing() {
    let config = LoopConfig {
        verbose: false,
        silent: true,
        ..Default::default()
    };
    let temp_dir = TempDir::new().unwrap();

    let result =
        execute_command_in_directory_capturing(temp_dir.path(), &sh("echo hello"), &config, None);
    assert!(result.success);
    assert_eq!(result.exit_code, Some(0));
    assert!(text(&result.stdout).contains("hello"));

    let result =
        execute_command_in_directory_capturing(temp_dir.path(), &fail_cmd(), &config, None);
    assert!(!result.success);
    assert_eq!(result.exit_code, Some(1));
}

#[test]
fn test_execute_command_in_directory_capturing_stderr() {
    let config = LoopConfig {
        verbose: false,
        silent: true,
        ..Default::default()
    };
    let temp_dir = TempDir::new().unwrap();

    // Command that writes to stderr
    let result = execute_command_in_directory_capturing(
        temp_dir.path(),
        &sh("echo error >&2"),
        &config,
        None,
    );
    assert!(result.success);
    assert!(text(&result.stderr).contains("error"));
}

#[test]
fn test_execute_command_nonexistent_directory() {
    let config = LoopConfig {
        verbose: false,
        silent: true,
        ..Default::default()
    };
    let nonexistent = Path::new("/nonexistent/path/that/does/not/exist");

    let result =
        execute_command_in_directory_capturing(nonexistent, &sh("echo test"), &config, None);
    assert!(!result.success);
    assert_eq!(result.exit_code, Some(1));
    assert!(text(&result.stderr).contains("does not exist"));
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}

/// What git receives as its arguments, quoted by git itself
fn git_sees(argv: &[&str], aliases: &BTreeMap<String, String>) -> String {
    let temp_dir = TempDir::new().unwrap();
    let config = LoopConfig {
        aliases: aliases.clone(),
        ..LoopConfig::default()
    };
    let result =
        execute_command_in_directory_capturing(temp_dir.path(), &strings(argv), &config, None);
    assert!(result.success, "{}", text(&result.stderr));
    assert!(fs::read_dir(temp_dir.path()).unwrap().next().is_none());
    text(&result.stdout).trim().to_string()
}

#[test]
fn arguments_reach_the_program_as_given() {
    let hostile = [
        "one arg",
        "$(touch PWNED)",
        "a;touch PWNED",
        "*",
        "%h %s",
        "'\"",
        "",
    ];
    let argv: Vec<&str> = ["git", "rev-parse", "--sq-quote"]
        .into_iter()
        .chain(hostile)
        .collect();
    let quoted: Vec<String> = hostile
        .iter()
        .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
        .collect();
    assert_eq!(git_sees(&argv, &BTreeMap::new()), quoted.join(" "));
}

#[test]
fn aliases_expand_into_words_not_shell() {
    let aliases = BTreeMap::from([(
        "q".to_string(),
        "git rev-parse --sq-quote 'two words' ;".to_string(),
    )]);
    assert_eq!(git_sees(&["q", "$(x)"], &aliases), "'two words' ';' '$(x)'");

    let broken = BTreeMap::from([("q".to_string(), "git 'unclosed".to_string())]);
    for dry_run in [false, true] {
        let config = LoopConfig {
            dry_run,
            aliases: broken.clone(),
            ..LoopConfig::default()
        };
        let result =
            execute_command_in_directory_capturing(Path::new("."), &strings(&["q"]), &config, None);
        assert!(!result.success);
        assert!(
            text(&result.stderr).contains("Alias 'q'"),
            "{}",
            text(&result.stderr)
        );
    }
}

#[test]
fn a_missing_program_fails_without_panicking() {
    let config = LoopConfig {
        directories: vec![".".into()],
        silent: true,
        ..LoopConfig::default()
    };
    assert!(run(&config, &strings(&["meta-no-such-program-xyz"])).is_err());
}

// ============================================================================
// Tests for new dry_run and json_output functionality
// ============================================================================

#[test]
fn test_dry_run_does_not_execute() {
    let temp_dir = TempDir::new().unwrap();
    let marker_file = temp_dir.path().join("marker.txt");

    let config = LoopConfig {
        directories: vec![temp_dir.path().to_path_buf()],
        dry_run: true,
        silent: true,
        ..Default::default()
    };

    // This command would create a file if executed
    let cmd = touch_cmd(&marker_file);
    let result = run(&config, &cmd);
    assert!(result.is_ok());

    // File should NOT exist because dry_run is true
    assert!(!marker_file.exists(), "dry_run should not execute commands");
}

#[test]
fn test_dry_run_returns_success() {
    let temp_dir = TempDir::new().unwrap();

    let config = LoopConfig {
        directories: vec![temp_dir.path().to_path_buf()],
        dry_run: true,
        silent: true,
        ..Default::default()
    };

    // Even a command that would fail should succeed in dry_run mode
    let result = run(&config, &fail_cmd());
    assert!(
        result.is_ok(),
        "a dry run of a command that would fail succeeds"
    );
}

#[test]
fn test_execute_command_in_directory_dry_run() {
    let config = LoopConfig {
        dry_run: true,
        silent: true,
        ..Default::default()
    };
    let temp_dir = TempDir::new().unwrap();

    let result = execute_command_in_directory(temp_dir.path(), &fail_cmd(), &config, None).unwrap();
    assert!(result.success, "dry_run should return success");
    assert_eq!(result.exit_code, Some(0));
}

#[test]
fn test_execute_command_in_directory_capturing_dry_run() {
    let config = LoopConfig {
        dry_run: true,
        silent: true,
        ..Default::default()
    };
    let temp_dir = TempDir::new().unwrap();

    let result =
        execute_command_in_directory_capturing(temp_dir.path(), &sh("echo hello"), &config, None);
    assert!(result.success);
    assert!(text(&result.stdout).contains("[DRY RUN]"));
    assert!(text(&result.stdout).contains("echo hello"));
}

#[test]
fn test_run_commands_empty_list() {
    let config = LoopConfig::default();
    let commands: Vec<DirCommand> = vec![];
    let result = run_commands(&config, &commands);
    assert!(result.is_ok());
}

#[test]
fn test_run_commands_sequential() {
    let temp_dir = TempDir::new().unwrap();
    let dir1 = temp_dir.path().join("dir1");
    let dir2 = temp_dir.path().join("dir2");
    fs::create_dir(&dir1).unwrap();
    fs::create_dir(&dir2).unwrap();

    let config = LoopConfig {
        parallel: false,
        silent: true,
        ..Default::default()
    };

    let commands = vec![
        DirCommand {
            dir: dir1.to_path_buf(),
            argv: sh("echo test1"),
            env: None,
            refused: None,
        },
        DirCommand {
            dir: dir2.to_path_buf(),
            argv: sh("echo test2"),
            env: None,
            refused: None,
        },
    ];

    let result = run_commands(&config, &commands);
    assert!(result.is_ok());
}

#[test]
fn test_run_commands_parallel() {
    let temp_dir = TempDir::new().unwrap();
    let dir1 = temp_dir.path().join("dir1");
    let dir2 = temp_dir.path().join("dir2");
    fs::create_dir(&dir1).unwrap();
    fs::create_dir(&dir2).unwrap();

    let config = LoopConfig {
        parallel: true,
        silent: true,
        ..Default::default()
    };

    let commands = vec![
        DirCommand {
            dir: dir1.to_path_buf(),
            argv: sh("echo test1"),
            env: None,
            refused: None,
        },
        DirCommand {
            dir: dir2.to_path_buf(),
            argv: sh("echo test2"),
            env: None,
            refused: None,
        },
    ];

    let result = run_commands(&config, &commands);
    assert!(result.is_ok());
}

#[test]
fn test_run_commands_with_different_commands() {
    let temp_dir = TempDir::new().unwrap();
    let dir1 = temp_dir.path().join("dir1");
    let dir2 = temp_dir.path().join("dir2");
    fs::create_dir(&dir1).unwrap();
    fs::create_dir(&dir2).unwrap();

    let config = LoopConfig {
        parallel: false,
        silent: true,
        ..Default::default()
    };

    // Use simple echo commands with different arguments to verify
    // that different commands can be executed in different directories
    let commands = vec![
        DirCommand {
            dir: dir1.to_path_buf(),
            argv: sh("echo command1"),
            env: None,
            refused: None,
        },
        DirCommand {
            dir: dir2.to_path_buf(),
            argv: sh("echo command2"),
            env: None,
            refused: None,
        },
    ];

    let result = run_commands(&config, &commands);
    assert!(
        result.is_ok(),
        "Different commands should execute in different directories"
    );
}

#[test]
fn test_run_commands_dry_run() {
    let temp_dir = TempDir::new().unwrap();
    let marker_file = temp_dir.path().join("marker.txt");

    let config = LoopConfig {
        dry_run: true,
        silent: true,
        ..Default::default()
    };

    let commands = vec![DirCommand {
        dir: temp_dir.path().to_path_buf(),
        argv: touch_cmd(&marker_file),
        env: None,
        refused: None,
    }];

    let result = run_commands(&config, &commands);
    assert!(result.is_ok());
    assert!(!marker_file.exists(), "dry_run should not execute commands");
}

#[test]
fn test_run_commands_failure_handling() {
    let temp_dir = TempDir::new().unwrap();
    let dir1 = temp_dir.path().join("dir1");
    fs::create_dir(&dir1).unwrap();

    let config = LoopConfig {
        parallel: false,
        silent: true,
        ..Default::default()
    };

    let commands = vec![DirCommand {
        dir: dir1.to_path_buf(),
        argv: fail_cmd(), // This command always fails
        env: None,
        refused: None,
    }];

    let result = run_commands(&config, &commands);
    assert!(result.is_err(), "Should return error when command fails");
}

#[test]
fn test_run_commands_failure_in_dry_run_succeeds() {
    let temp_dir = TempDir::new().unwrap();

    let config = LoopConfig {
        dry_run: true,
        silent: true,
        ..Default::default()
    };

    let commands = vec![DirCommand {
        dir: temp_dir.path().to_path_buf(),
        argv: fail_cmd(),
        env: None,
        refused: None,
    }];

    let result = run_commands(&config, &commands);
    assert!(
        result.is_ok(),
        "dry_run should succeed even for failing commands"
    );
}

#[test]
fn test_json_output_structures() {
    // Test JsonOutput serialization
    let output = JsonOutput {
        success: true,
        results: vec![JsonCommandResult {
            directory: "/test".to_string(),
            command: "echo hello".to_string(),
            success: true,
            exit_code: Some(0),
            signal: None,
            stdout: "hello\n".to_string(),
            stderr: String::new(),
        }],
        summary: JsonSummary {
            total: 1,
            succeeded: 1,
            failed: 0,
            dry_run: false,
        },
    };

    let json = serde_json::to_string(&output).unwrap();
    assert!(json.contains("\"success\":true"));
    assert!(json.contains("\"directory\":\"/test\""));
    assert!(json.contains("\"total\":1"));
}

#[test]
fn test_json_output_skips_empty_strings() {
    let result = JsonCommandResult {
        directory: "/test".to_string(),
        command: "echo".to_string(),
        success: true,
        exit_code: Some(0),
        signal: None,
        stdout: String::new(),
        stderr: String::new(),
    };

    let json = serde_json::to_string(&result).unwrap();
    // Empty stdout/stderr should be skipped due to skip_serializing_if
    assert!(!json.contains("\"stdout\":\"\""));
    assert!(!json.contains("\"stderr\":\"\""));
}

#[test]
fn test_display_name_is_path_from_root() {
    let root = Path::new("/ws/harmony");
    assert_eq!(display_name(root, Some(root)), ".");
    assert_eq!(
        display_name(
            Path::new("/ws/harmony/open-source/meta/loop_cli"),
            Some(root)
        ),
        "./open-source/meta/loop_cli"
    );
    // Two repos with the same basename stay distinguishable
    assert_ne!(
        display_name(Path::new("/ws/harmony/agent"), Some(root)),
        display_name(Path::new("/ws/harmony/open-source/meta/agent"), Some(root))
    );
    // Outside the root, the full path; without a root, the basename
    assert_eq!(
        display_name(Path::new("/ws/other/repo"), Some(root)),
        Path::new("/ws/other/repo").display().to_string()
    );
    assert_eq!(display_name(Path::new("./repo"), None), "repo");
    assert_eq!(result_name(root, Some(root)), ". (harmony)");
}

#[test]
fn test_display_names_escape_control_characters() {
    let root = Path::new("/ws/x\u{1b}]0;TITLE\u{7}");
    assert_eq!(result_name(root, Some(root)), ". (x\\u{1b}]0;TITLE\\u{7})");
    assert_eq!(
        display_name(&root.join("a\u{1b}[2J"), Some(root)),
        "./a\\u{1b}[2J"
    );
    assert_eq!(
        display_name(Path::new("/elsewhere/n\nfake"), Some(root)),
        "/elsewhere/n\\nfake"
    );
    assert_eq!(display_name(Path::new("b\u{9b}31m"), None), "b\\u{9b}31m");
}

#[test]
fn a_malformed_alias_is_named_escaped() {
    let aliases = BTreeMap::from([(
        "bad\u{1b}[2J".to_string(),
        "'unclosed\u{1b}[2J\nFORGED".to_string(),
    )]);
    let err = resolve_aliases(&["bad\u{1b}[2J".to_string()], &aliases).unwrap_err();
    let message = format!("{err:#}");
    assert!(
        !message.contains('\u{1b}') && !message.contains('\n'),
        "{message}"
    );
}

/// `contents` as a `.looprc`, parsed
fn looprc(contents: &str) -> Result<LoopConfig> {
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join(".looprc");
    fs::write(&path, contents).unwrap();
    parse_config(&path)
}

#[test]
fn a_looprc_is_one_format_with_one_default() {
    // a key left out takes the same default as no file at all
    assert_eq!(looprc("{}").unwrap().ignore, LoopConfig::default().ignore);
    assert_eq!(LoopConfig::default().ignore, [".git"]);

    let config = looprc(
        r#"{"directories": ["a"], "silent": true, "max_parallel": 2,
            "aliases": {"gs": "git status"}}"#,
    )
    .unwrap();
    assert_eq!(config.directories, [PathBuf::from("a")]);
    assert!(config.silent);
    assert_eq!(config.max_parallel.map(NonZeroUsize::get), Some(2));
    assert_eq!(config.aliases["gs"], "git status");

    // what loop can't use is an error naming the file, not ignored
    for (contents, says) in [
        (r#"{"concurrency": 4}"#, "unknown field `concurrency`"),
        (r#"{"max_parallel": 0}"#, "nonzero"),
        (r#"{"aliases": {"gs": 1}}"#, "invalid type"),
        (r#"{"add_aliases_to_global_looprc": true}"#, "unknown field"),
        // the file's text in the message is escaped
        (r#"{"\u001b[2J": 1}"#, "\\u{1b}[2J"),
    ] {
        let message = format!("{:#}", looprc(contents).unwrap_err());
        assert!(
            message.contains(".looprc") && message.contains(says),
            "{message}"
        );
    }

    let message = format!("{:#}", parse_config(Path::new("no-such.json")).unwrap_err());
    assert!(message.contains("no-such.json"), "{message}");
    assert!(parse_config_if_present(Path::new("no-such.json")).is_ok());
}

#[test]
fn starts_are_one_interval_apart_however_long_commands_take() {
    use std::cell::{Cell, RefCell};
    let interval = Duration::from_millis(10);
    let stagger = Stagger::new(interval);
    let clock = Cell::new(Instant::now());
    let slept = RefCell::new(Vec::new());
    let wait = |busy: Duration| {
        clock.set(clock.get() + busy);
        stagger.wait_with(
            || clock.get(),
            |d| {
                slept.borrow_mut().push(d);
                clock.set(clock.get() + d);
            },
        );
    };

    wait(Duration::ZERO); // the first starts at once
    wait(Duration::ZERO); // the next one interval later
    wait(Duration::from_millis(4)); // the rest of the interval
    wait(Duration::from_millis(50)); // the interval long past: at once
    assert_eq!(
        *slept.borrow(),
        [interval, Duration::from_millis(6), Duration::ZERO]
    );
}

#[test]
fn a_refused_command_is_a_failed_result_and_never_runs() {
    let temp_dir = TempDir::new().unwrap();
    let command = |refused: Option<&str>| DirCommand {
        dir: temp_dir.path().to_path_buf(),
        argv: sh("echo ran > ran"),
        env: None,
        refused: refused.map(str::to_string),
    };
    for parallel in [false, true] {
        let config = LoopConfig {
            parallel,
            json_output: true,
            ..Default::default()
        };
        let results = match parallel {
            true => run_parallel(&config, &[command(Some("not a repository"))]),
            false => run_sequential(&config, &[command(Some("not a repository"))]),
        }
        .unwrap();
        assert!(!results[0].success);
        assert_eq!(text(&results[0].stderr), "not a repository\n");
        assert!(!temp_dir.path().join("ran").exists());
    }
}
