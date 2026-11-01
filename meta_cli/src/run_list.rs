//! What a per-project command runs in: each selected project folder only if
//! it is its own repository, and git there kept to that repository.

use loop_lib::DirCommand;
use meta_core::gitdir::Checkout;
use std::path::{Path, PathBuf};

/// `commands`, each in a selected project folder (one of `projects`, other
/// than `root`) that is not its own repository refused with why, which loop
/// reports as a failed result without running it; every command in a
/// project folder, `root`'s included, gets [`meta_core::git::ceiling`].
pub fn in_repos(
    mut commands: Vec<DirCommand>,
    projects: &[PathBuf],
    root: Option<&Path>,
) -> Vec<DirCommand> {
    use rayon::prelude::*;
    commands.par_iter_mut().for_each(|command| {
        if !projects.contains(&command.dir) {
            return;
        }
        if root != Some(command.dir.as_path()) {
            command.refused = Checkout::of(&command.dir).problem();
        }
        command.env.get_or_insert_with(Default::default).extend(
            meta_core::git::ceiling(&command.dir)
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string_lossy().into_owned())),
        );
    });
    commands
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_commands_get_the_ceiling_and_others_are_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let plain = root.join("plain");
        std::fs::create_dir(&plain).unwrap();
        let command = |dir: &Path| DirCommand {
            dir: dir.to_path_buf(),
            argv: vec!["git".into(), "status".into()],
            env: None,
            refused: None,
        };
        let elsewhere = root.join("elsewhere");
        let commands = in_repos(
            vec![command(&root), command(&plain), command(&elsewhere)],
            &[root.clone(), plain.clone()],
            Some(&root),
        );
        let ceiling = |c: &DirCommand| {
            c.env
                .as_ref()
                .is_some_and(|env| env.contains_key("GIT_CEILING_DIRECTORIES"))
        };
        // the root runs, bounded; the plain folder is refused, bounded too;
        // a plugin's own folder is none of meta's business
        assert!(ceiling(&commands[0]) && commands[0].refused.is_none());
        assert!(ceiling(&commands[1]) && commands[1].refused.is_some());
        assert!(!ceiling(&commands[2]) && commands[2].refused.is_none());
    }
}
