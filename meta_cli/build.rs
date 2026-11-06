//! The commit meta is built from, for `meta --version` and `meta --version
//! --json`: a build from a git checkout (GitHub main) says which commit it
//! is, and whether files git tracks were changed since; one from elsewhere (a
//! source archive) says only the version and, in JSON, when it was built.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The root of meta's source, where its `.git` is if it has one.
fn root() -> PathBuf {
    let crate_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    crate_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or(crate_dir)
}

/// What git printed about meta's checkout, empty if that was nothing; `None`
/// if it failed or there is no checkout. Never another repo's answer: not a
/// `.git` left in this crate's folder by the days it was a repo of its own,
/// nor that of a repo around a source archive.
fn git_output(args: &[&str]) -> Option<String> {
    let root = root();
    if !root.join(".git").exists() {
        return None;
    }
    #[expect(
        clippy::disallowed_methods,
        reason = "the build script reads meta's own checkout, not a user's repo"
    )]
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    output.status.success().then(|| text.trim().to_string())
}

fn git(args: &[&str]) -> Option<String> {
    git_output(args).filter(|text| !text.is_empty())
}

/// Build again when `name`, a file of git's, changes. git answers relative to
/// the root and cargo reads a path relative to this crate.
fn watch_git_path(name: &str) {
    if let Some(path) = git(&["rev-parse", "--git-path", name]) {
        println!("cargo:rerun-if-changed={}", root().join(path).display());
    }
}

fn main() {
    // a new commit, a branch switch, a fetch that packs refs, staging: build again
    for name in ["HEAD", "packed-refs", "index"] {
        watch_git_path(name);
    }
    if let Some(head) = git(&["symbolic-ref", "--quiet", "HEAD"]) {
        watch_git_path(&head);
    }
    // and an edit of any file git tracks (target/ isn't one)
    for entry in git(&["ls-tree", "--name-only", "--full-tree", "HEAD"])
        .unwrap_or_default()
        .lines()
    {
        println!("cargo:rerun-if-changed={}", root().join(entry).display());
    }
    // (without writing the index, which would make every build run this again)
    let dirty = git_output(&[
        "--no-optional-locks",
        "status",
        "--porcelain",
        "--untracked-files=no",
    ])
    .map(|changes| !changes.is_empty());
    let changed = dirty.unwrap_or_default();
    let note = if changed {
        ", with uncommitted changes"
    } else {
        ""
    };
    let commit = git(&["log", "-1", "--format=%h %cs"])
        .map(|commit| format!(" (git {commit}{note})"))
        .unwrap_or_default();
    println!("cargo:rustc-env=META_BUILD_COMMIT={commit}");

    // not describe's own --dirty, which writes the index
    let suffix = if changed { "-dirty" } else { "" };
    let source = git(&["describe", "--tags", "--always", "--match", "v*"])
        .map(|source| format!("{source}{suffix}"));
    for (name, value) in [
        ("META_BUILD_SOURCE", source),
        ("META_BUILD_SHA", git(&["rev-parse", "HEAD"])),
        ("META_BUILD_BRANCH", git(&["branch", "--show-current"])),
        ("META_BUILD_DIRTY", dirty.map(|dirty| dirty.to_string())),
    ] {
        if let Some(value) = value {
            println!("cargo:rustc-env={name}={value}");
        }
    }
    let built = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    println!("cargo:rustc-env=META_BUILT={built}");
}
