//! Plugins stay plugins: a first-party plugin depends on the SDK
//! (`meta_plugin`) and on no part of meta itself, so a third-party plugin
//! can do all it does. A ratchet: what a plugin may still use, until the PR
//! named, shrinks to nothing.

#![expect(
    clippy::string_slice,
    clippy::disallowed_methods,
    reason = "test helpers: a panic is a failed test, and the test runs cargo"
)]

use std::collections::{BTreeMap, BTreeSet};

/// Each plugin package, and the meta crates it may still depend on.
const ALLOWED: &[(&str, &[&str])] = &[
    // meta_core's git, gitdir and set_record move into meta-git in PR 7
    ("meta_git_cli", &["meta_core"]),
    // deleted in PR 5a
    ("meta_rust_cli", &[]),
    // becomes a plugin built on the index in PR 10
    ("meta-mcp", &["meta_core"]),
];

/// meta's own crates, none of which a plugin may reach but as allowed.
const META: &[&str] = &["meta", "meta_core", "loop_lib"];

/// The meta_core modules meta-git may use until PR 7.
const GIT_MODULES: &[&str] = &["git", "gitdir", "set_record"];

#[test]
fn plugins_depend_only_on_the_sdk() {
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/../Cargo.toml");
    let out = std::process::Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--manifest-path",
            manifest,
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let name_of: BTreeMap<String, String> = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["id"].as_str().unwrap().into(),
                p["name"].as_str().unwrap().into(),
            )
        })
        .collect();
    // each package's normal dependencies, by name
    let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for node in metadata["resolve"]["nodes"].as_array().unwrap() {
        let normal = node["deps"].as_array().unwrap().iter().filter(|dep| {
            dep["dep_kinds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|kind| kind["kind"].is_null())
        });
        deps.insert(
            name_of[node["id"].as_str().unwrap()].clone(),
            normal
                .map(|dep| name_of[dep["pkg"].as_str().unwrap()].clone())
                .collect(),
        );
    }
    for (plugin, allowed) in ALLOWED {
        // everything it reaches, through anything
        let mut reached = BTreeSet::new();
        let mut todo = vec![plugin.to_string()];
        while let Some(name) = todo.pop() {
            for dep in deps.get(&name).into_iter().flatten() {
                if reached.insert(dep.clone()) && !allowed.contains(&dep.as_str()) {
                    todo.push(dep.clone());
                }
            }
        }
        assert!(
            reached.contains("meta_plugin"),
            "{plugin} doesn't use the SDK"
        );
        let bad: Vec<&str> = META
            .iter()
            .copied()
            .filter(|krate| reached.contains(*krate) && !allowed.contains(krate))
            .collect();
        assert!(bad.is_empty(), "{plugin} depends on {bad:?}");
    }
    // meta-git uses three modules of meta_core, no more
    let src = concat!(env!("CARGO_MANIFEST_DIR"), "/../meta_git_cli/src");
    let mut used = BTreeSet::new();
    let mut dirs = vec![std::path::PathBuf::from(src)];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for (at, word) in text.match_indices("meta_core::") {
                let rest = &text[at + word.len()..];
                let names: &str = match rest.strip_prefix('{') {
                    Some(list) => list.split('}').next().unwrap(),
                    None => rest
                        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .next()
                        .unwrap(),
                };
                for name in names.split(',') {
                    let module = name.trim().split("::").next().unwrap().trim();
                    if !module.is_empty() {
                        used.insert(module.to_string());
                    }
                }
            }
        }
    }
    let extra: Vec<&String> = used
        .iter()
        .filter(|m| !GIT_MODULES.contains(&m.as_str()))
        .collect();
    assert!(extra.is_empty(), "meta-git uses meta_core::{extra:?}");
}
