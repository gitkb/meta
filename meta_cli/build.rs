//! The commit meta is built from, for `meta --version`: a build from a git
//! checkout (GitHub main) says which commit it is, and whether files git
//! tracks were changed since; one from elsewhere (a source archive) says
//! only the version.

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

fn main() -> Result<(), String> {
    meta_plugin::build::generate_to("meta-host.toml", "host.rs")?;
    // a new commit, a branch switch, a fetch that packs refs, staging: build again
    for name in ["HEAD", "packed-refs", "index"] {
        if let Some(path) = git(&["rev-parse", "--git-path", name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(head) = git(&["symbolic-ref", "--quiet", "HEAD"]) {
        if let Some(path) = git(&["rev-parse", "--git-path", &head]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    // and an edit of any file git tracks (target/ isn't one)
    if let Some(top) = git(&["rev-parse", "--show-toplevel"]) {
        for entry in git(&["ls-tree", "--name-only", "--full-tree", "HEAD"])
            .unwrap_or_default()
            .lines()
        {
            println!("cargo:rerun-if-changed={top}/{entry}");
        }
    }
    // (without writing the index, which would make every build run this again)
    let dirty = git(&[
        "--no-optional-locks",
        "status",
        "--porcelain",
        "--untracked-files=no",
    ])
    .map(|_| ", with uncommitted changes")
    .unwrap_or_default();
    let commit = git(&["log", "-1", "--format=%h %cs"])
        .map(|commit| format!(" (git {commit}{dirty})"))
        .unwrap_or_default();
    println!("cargo:rustc-env=META_BUILD_COMMIT={commit}");
    embed_skills().map_err(|e| format!("claude-plugin/skills: {e}"))
}

/// Write `SKILLS` for `init.rs`, in name order: the name and the `SKILL.md`
/// of each folder of `claude-plugin/skills/`.
fn embed_skills() -> std::io::Result<()> {
    let manifest =
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let skills = manifest.join("../claude-plugin/skills");
    println!("cargo:rerun-if-changed={}", skills.display());
    let mut names = std::fs::read_dir(&skills)?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<Vec<String>>>()?;
    names.sort();
    let entries: String = names
        .iter()
        .map(|name| {
            let file = skills.join(name).join("SKILL.md");
            format!(
                "({name:?}, include_str!({:?})),\n",
                file.display().to_string()
            )
        })
        .collect();
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default());
    std::fs::write(
        out.join("skills.rs"),
        format!("const SKILLS: &[(&str, &str)] = &[\n{entries}];\n"),
    )
}
