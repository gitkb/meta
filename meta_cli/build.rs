//! The commit meta is built from, for `meta --version`: a build from a git
//! checkout (GitHub main) says which commit it is; one from elsewhere (a
//! source archive) says only the version.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the build script reads meta's own checkout, not a user's repo"
    )]
    let output = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (output.status.success() && !text.is_empty()).then(|| text.to_string())
}

fn main() {
    // a new commit, a branch switch or a fetch that packs refs: build again
    for name in ["HEAD", "packed-refs"] {
        if let Some(path) = git(&["rev-parse", "--git-path", name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(head) = git(&["symbolic-ref", "--quiet", "HEAD"]) {
        if let Some(path) = git(&["rev-parse", "--git-path", &head]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let commit = git(&["log", "-1", "--format=%h %cs"])
        .map(|commit| format!(" (git {commit})"))
        .unwrap_or_default();
    println!("cargo:rustc-env=META_BUILD_COMMIT={commit}");
}
