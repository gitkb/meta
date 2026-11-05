//! Paths inside a folder: where a relative path really leads, through the
//! links that exist now, refused when one leads out.

use crate::display::{escape, escape_path};
use std::path::{Path, PathBuf};

/// Resolve `rel` below `base_dir` through the symlinks that exist now, and
/// refuse it if one leads outside `base_dir` or if it ends at `base_dir`.
/// Returns the project's location: the real path of the part that exists,
/// followed by the part not created yet.
pub fn resolve_inside(base_dir: &Path, rel: &str) -> Result<PathBuf, String> {
    resolve_inside_after(base_dir, rel, 0)
}

/// Links followed for a missing target before giving up, as the OS does.
const MAX_LINK_HOPS: usize = 40;

/// [`resolve_inside`], `hops` links into following missing targets.
fn resolve_inside_after(base_dir: &Path, rel: &str, hops: usize) -> Result<PathBuf, String> {
    let base = base_dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", escape_path(base_dir)))?;
    if rel.starts_with('/') || rel.split('/').any(|part| matches!(part, "" | "." | "..")) {
        return Err(format!(
            "path '{}' must be a plain relative path",
            escape(rel)
        ));
    }
    let mut current = base.clone();
    let mut exists = true;
    for part in rel.split('/') {
        current.push(part);
        if !exists {
            continue;
        }
        match current.symlink_metadata() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => exists = false,
            // What can't be examined (no permission, say) may hold a
            // symlink out, so it is refused rather than taken as missing
            Err(e) => {
                return Err(format!(
                    "path '{}' cannot be checked at '{}': {e}",
                    escape(rel),
                    escape_path(&current)
                ))
            }
            Ok(meta) if meta.file_type().is_symlink() => match current.canonicalize() {
                Ok(target) if target.starts_with(&base) => current = target,
                // A link to what isn't there yet (a repo a checkout links
                // to, before its own checkout): checked as the path it names
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    current = dangling_target(&base, &current, rel, hops + 1)?;
                    // the rest is checked too: part of it may exist
                    exists = current.symlink_metadata().is_ok();
                }
                Ok(_) => {
                    return Err(format!(
                        "path '{}' leads outside {} through the symlink '{}'",
                        escape(rel),
                        escape_path(&base),
                        escape_path(&current)
                    ))
                }
                // Its target may not exist yet, but where it would lead
                // cannot be checked, so it is refused
                Err(e) => {
                    return Err(format!(
                        "path '{}' goes through the symlink '{}', which cannot be resolved: {e}",
                        escape(rel),
                        escape_path(&current)
                    ))
                }
            },
            Ok(_) => {}
        }
    }
    if current == base {
        return Err(format!(
            "path '{}' leads to the directory of the .meta.yaml file itself",
            escape(rel)
        ));
    }
    Ok(current)
}

/// Where the symlink `link` inside `base` leads when its target is missing:
/// the target's path, with each link on the way followed before a `..`
/// after it, then checked with [`resolve_inside`].
fn dangling_target(base: &Path, link: &Path, rel: &str, hops: usize) -> Result<PathBuf, String> {
    let unresolved = |at: &Path, e: std::io::Error| {
        format!(
            "path '{}' goes through the symlink '{}', which cannot be resolved: {e}",
            escape(rel),
            escape_path(at)
        )
    };
    if hops > MAX_LINK_HOPS {
        return Err(unresolved(
            link,
            std::io::Error::other("too many levels of symbolic links"),
        ));
    }
    let target = std::fs::read_link(link).map_err(|e| unresolved(link, e))?;
    let mut path = link.parent().unwrap_or(base).to_path_buf();
    for part in target.components() {
        match part {
            // `..` leaves a directory only if it is there, as the OS has it
            std::path::Component::ParentDir => {
                if !path.is_dir() {
                    return Err(unresolved(
                        link,
                        std::io::Error::from(std::io::ErrorKind::NotFound),
                    ));
                }
                path.pop();
            }
            std::path::Component::CurDir => {}
            // What exists is taken as the OS resolves it (links followed,
            // and in the base's form: `\\?\` on Windows, which an absolute
            // target doesn't carry); a link to what is missing is followed here
            std::path::Component::Normal(_) => {
                path.push(part);
                match path.canonicalize() {
                    Ok(real) => path = real,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        if path
                            .symlink_metadata()
                            .is_ok_and(|meta| meta.file_type().is_symlink())
                        {
                            path = dangling_target(base, &path, rel, hops + 1)?;
                        }
                    }
                    Err(e) => return Err(unresolved(&path, e)),
                }
            }
            // The root of an absolute target
            std::path::Component::Prefix(_) | std::path::Component::RootDir => path.push(part),
        }
    }
    let outside = || {
        format!(
            "path '{}' leads outside {} through the symlink '{}'",
            escape(rel),
            escape_path(base),
            escape_path(link)
        )
    };
    let place = path.strip_prefix(base).map_err(|_| outside())?;
    let place: Vec<_> = place
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect();
    if place.is_empty() {
        return Err(outside());
    }
    resolve_inside_after(base, &place.join("/"), hops)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn an_absolute_link_to_what_is_not_there_yet_stays_inside() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        // the target as a user writes it, without `\\?\`
        let plain = dir.path().join("real");
        // creating links needs a privilege or developer mode
        if std::os::windows::fs::symlink_dir(&plain, base.join("link")).is_err() {
            eprintln!("skipped: this runner can't create symbolic links");
            return;
        }
        assert_eq!(resolve_inside(&base, "link"), Ok(base.join("real")));
        let outside = tempfile::tempdir().unwrap();
        std::os::windows::fs::symlink_dir(outside.path().join("x"), base.join("out")).unwrap();
        let err = resolve_inside(&base, "out").unwrap_err();
        assert!(err.contains("leads outside"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_what_is_not_there_yet_leads_where_it_names() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        std::os::unix::fs::symlink("real", base.join("link")).unwrap();
        assert_eq!(resolve_inside(&base, "link"), Ok(base.join("real")));
        assert_eq!(resolve_inside(&base, "link/x"), Ok(base.join("real/x")));

        std::os::unix::fs::symlink("../elsewhere", base.join("out")).unwrap();
        let err = resolve_inside(&base, "out").unwrap_err();
        assert!(err.contains("leads outside"), "{err}");
        // through a link that does exist, out of the base
        std::os::unix::fs::symlink("/", base.join("root")).unwrap();
        std::os::unix::fs::symlink("root/nowhere-meta-test", base.join("via")).unwrap();
        let err = resolve_inside(&base, "via").unwrap_err();
        assert!(err.contains("leads outside"), "{err}");

        // `..` climbs from where the link before it leads
        std::fs::create_dir_all(base.join("real/base")).unwrap();
        std::os::unix::fs::symlink("real/base", base.join("hop")).unwrap();
        std::os::unix::fs::symlink("hop/../app", base.join("back")).unwrap();
        assert_eq!(resolve_inside(&base, "back"), Ok(base.join("real/app")));

        // `..` out of a directory that isn't there is no way through, and a
        // link after a followed one is still checked
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(base.join("b")).unwrap();
        std::os::unix::fs::symlink(outside.path(), base.join("b/x")).unwrap();
        std::os::unix::fs::symlink("missing/../b", base.join("skip")).unwrap();
        assert!(resolve_inside(&base, "skip/x").is_err());
        std::os::unix::fs::symlink("b", base.join("tob")).unwrap();
        std::os::unix::fs::symlink("nothing/../tob/x/y", base.join("deep")).unwrap();
        assert!(resolve_inside(&base, "deep").is_err());
        std::os::unix::fs::symlink("missing/../self", base.join("self")).unwrap();
        assert!(resolve_inside(&base, "self").is_err());
        // names from the filesystem are escaped in the error
        std::os::unix::fs::symlink(outside.path(), base.join("bad\nFORGED")).unwrap();
        std::os::unix::fs::symlink("bad\nFORGED/missing", base.join("forge")).unwrap();
        let err = resolve_inside(&base, "forge").unwrap_err();
        assert!(!err.contains('\n'), "{err}");
        // a chain of links to missing targets ends
        std::os::unix::fs::symlink("ping/z", base.join("pong")).unwrap();
        std::os::unix::fs::symlink("pong/z", base.join("ping")).unwrap();
        assert!(resolve_inside(&base, "ping").is_err());
    }
}
