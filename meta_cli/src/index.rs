//! The command index: every command there is, read from manifests. It
//! answers help, `meta --json help`, completions and the unknown-command
//! list; what runs a command is still the router in `main.rs`.
//!
//! A bad manifest or a collision disables only the commands it touches, and
//! reading the index never fails: what went wrong is in its `problems`.

use crate::subprocess_plugins::SubprocessPluginManager;
use meta_plugin::manifest::{
    Index, Indexed, Manifest, PluginFile, Problem, Provider, Source, FILE,
};
use std::path::{Path, PathBuf};

/// The first-party manifests, as the last place to look: an install has no
/// manifest files yet, and `meta --json help git …` must have an answer.
const EMBEDDED: [&str; 2] = [
    include_str!("../../meta_git_cli/meta-plugin.toml"),
    include_str!("../../plugins/cargo/meta-plugin.toml"),
];

/// Commands no provider may replace, whatever their effect.
const FIXED: [&str; 2] = ["help", "plugin"];

/// A manifest as found: where, and what it holds.
struct Found {
    source: Source,
    path: Option<PathBuf>,
    manifest: Result<Manifest, String>,
}

/// Read every manifest there is, and `plugins`' files.
pub fn load(plugins: &SubprocessPluginManager) -> Index {
    let mut problems = Vec::new();
    let mut found = Vec::new();
    for (source, dir) in folders(&mut problems) {
        found.extend(manifests_in(source, &dir, &mut problems));
    }
    let on_disk = |name: &str| {
        found.iter().any(|f: &Found| {
            f.source == Source::Release && f.manifest.as_ref().is_ok_and(|m| m.name == name)
        })
    };
    let embedded: Vec<Found> = EMBEDDED
        .iter()
        .map(|text| Found {
            source: Source::Embedded,
            path: None,
            manifest: Manifest::parse(text),
        })
        .filter(|f| !f.manifest.as_ref().is_ok_and(|m| on_disk(&m.name)))
        .collect();
    found.extend(embedded);
    found.sort_by_key(|f| f.source);
    let mut index = build(without_copies(found));
    index.problems.extend(problems);
    index.files = files(&index, plugins);
    for held in index.commands.iter_mut() {
        let word = held.name.split(' ').next();
        let file = index
            .files
            .iter()
            .find(|file| Some(file.word.as_str()) == word);
        if held.provider.source != Source::Host {
            held.file = file.map(|file| file.path.clone());
        }
    }
    index
}

/// `found` without a manifest the same as one nearer in the same scope: a
/// worktree set holds a copy of what its workspace commits, and neither is a
/// second provider.
fn without_copies(found: Vec<Found>) -> Vec<Found> {
    let mut kept: Vec<Found> = Vec::new();
    for manifest in found {
        let copy = kept.iter().any(|near| {
            near.source == manifest.source
                && near.manifest.is_ok()
                && near.manifest == manifest.manifest
        });
        if !copy {
            kept.push(manifest);
        }
    }
    kept
}

/// The folders that hold plugin folders, with the scope of each.
fn folders(problems: &mut Vec<Problem>) -> Vec<(Source, PathBuf)> {
    let mut folders: Vec<(Source, PathBuf)> = Vec::new();
    let mut problem = |place: String, e: anyhow::Error| {
        problems.push(Problem {
            place,
            command: None,
            message: format!("{e:#}"),
        })
    };
    let workspace = std::env::current_dir()
        .map_err(anyhow::Error::from)
        .and_then(|cwd| meta_core::config::nearest_meta_dir(&cwd))
        .and_then(|nearest| {
            nearest.map_or(Ok(Vec::new()), |dir| {
                meta_core::config::workspace_meta_dirs(&dir)
            })
        });
    match workspace {
        Ok(dirs) => folders.extend(
            dirs.into_iter()
                .map(|dir| (Source::Workspace, dir.join(".meta/plugins"))),
        ),
        Err(e) => problem("the workspace".to_string(), e),
    }
    match meta_core::meta_dir() {
        Ok(dir) => folders.push((Source::User, dir.join("plugins"))),
        Err(e) => problem("your meta folder".to_string(), e),
    }
    folders.extend(release_dir().map(|dir| (Source::Release, dir)));
    if let Some(dirs) = std::env::var_os("META_PLUGIN_DIRS") {
        folders.extend(std::env::split_paths(&dirs).map(|dir| (Source::Release, dir)));
    }
    // one folder by two names (a workspace in the home folder) is read once
    let mut seen = std::collections::HashSet::new();
    folders.retain(|(_, dir)| seen.insert(dir.canonicalize().unwrap_or_else(|_| dir.clone())));
    folders
}

/// `<prefix>/lib/meta/plugins`, for meta at `<prefix>/bin/meta`, through any
/// links to it (Homebrew's).
fn release_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    Some(exe.parent()?.parent()?.join("lib/meta/plugins"))
}

/// The manifests of the plugin folders in `dir`, by folder name.
fn manifests_in(source: Source, dir: &Path, problems: &mut Vec<Problem>) -> Vec<Found> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            problems.push(Problem {
                place: dir.to_string_lossy().into_owned(),
                command: None,
                message: e.to_string(),
            });
            return Vec::new();
        }
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path().join(FILE))
        .filter(|path| path.is_file())
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| Found {
            source,
            manifest: Manifest::load(&path),
            path: Some(path),
        })
        .collect()
}

/// The index of meta's own commands and `found` (farthest first).
fn build(found: Vec<Found>) -> Index {
    let host = Manifest::parse(crate::HOST_MANIFEST).unwrap_or_default();
    let mut index = Index {
        meta_options: host.meta_options.clone(),
        ..Index::default()
    };
    add(&mut index, Source::Host, None, host);
    for Found {
        source,
        path,
        manifest,
    } in found
    {
        let place = place_of(&path);
        match manifest {
            Ok(manifest) => {
                let mut wrong = manifest.problems();
                if !manifest.meta_options.is_empty() {
                    wrong.push("meta_options belong to meta alone".to_string());
                }
                match wrong.is_empty() {
                    true => add(&mut index, source, path, manifest),
                    false => index
                        .problems
                        .extend(wrong.into_iter().map(|message| Problem {
                            place: place.clone(),
                            command: None,
                            message,
                        })),
                }
            }
            Err(message) => index.problems.push(Problem {
                place,
                command: None,
                message,
            }),
        }
    }
    index
}

fn place_of(path: &Option<PathBuf>) -> String {
    match path {
        Some(path) => path.to_string_lossy().into_owned(),
        None => "built into meta".to_string(),
    }
}

/// The commands of `manifest`, as the precedence rule lets them in.
fn add(index: &mut Index, source: Source, path: Option<PathBuf>, manifest: Manifest) {
    let provider = Provider {
        name: manifest.name.clone(),
        version: manifest
            .version
            .clone()
            .or_else(|| (source <= Source::Embedded).then(|| crate::VERSION.trim().to_string())),
        source,
        path,
    };
    let place = place_of(&provider.path);
    for (name, command) in &manifest.commands {
        let wrong = manifest.command_problems(name, command);
        let problem = |index: &mut Index, message: String| {
            index.problems.push(Problem {
                place: place.clone(),
                command: Some(name.clone()),
                message,
            })
        };
        let held = holder(index, name, &provider);
        if let Some(at) = held {
            // a mistake disables the one who made it, never the command it
            // tried to take; an override that keeps the rules waits, as what
            // runs a command doesn't read the index yet
            let refused = index
                .commands
                .get(at)
                .and_then(|held| refusal(held, name, &provider, manifest.overrides.contains(name)));
            match refused.or_else(|| (!wrong.is_empty()).then(|| wrong.join("; "))) {
                Some(message) => problem(index, message),
                None => {
                    if let Some(held) = index.commands.get_mut(at) {
                        held.waiting.push(provider.clone());
                    }
                }
            }
            continue;
        }
        let disabled = (!wrong.is_empty()).then(|| wrong.join("; "));
        if let Some(message) = &disabled {
            problem(index, message.clone());
        }
        index.commands.push(Indexed {
            name: name.clone(),
            provider: provider.clone(),
            disabled,
            file: None,
            waiting: Vec::new(),
            command: command.clone(),
        });
    }
}

/// Where in the index another provider holds `name`: the command itself,
/// or the longest raw command whose words reach it (raw words belong to
/// their provider).
fn holder(index: &Index, name: &str, provider: &Provider) -> Option<usize> {
    let exact = index.commands.iter().position(|held| held.name == name);
    let nearest_raw = || {
        let under = |held: &&Indexed| held.command.raw && is_under(name, &held.name);
        let longest = index
            .commands
            .iter()
            .filter(under)
            .max_by_key(|h| h.name.len())?;
        index
            .commands
            .iter()
            .position(|held| held.name == longest.name)
    };
    exact.or_else(nearest_raw).filter(|at| {
        index
            .commands
            .get(*at)
            .is_some_and(|held| held.provider != *provider)
    })
}

/// Whether the words of `name` start with all those of `command`, and more.
fn is_under(name: &str, command: &str) -> bool {
    name.strip_prefix(command)
        .is_some_and(|rest| rest.starts_with(' '))
}

/// Why `provider` can't take `name` from `held`, if it can't.
fn refusal(held: &Indexed, name: &str, provider: &Provider, overrides: bool) -> Option<String> {
    let owner = format!("{} ({})", held.provider.name, place_of(&held.provider.path));
    let first = name.split(' ').next().unwrap_or_default();
    if FIXED.contains(&first) || held.command.effects == meta_plugin::manifest::Effect::Destructive
    {
        return Some(format!(
            "`{name}` can't be overridden; it stays with {owner}"
        ));
    }
    if !overrides {
        return Some(format!(
            "`{name}` belongs to {owner}: to replace it, declare overrides = [\"{name}\"]"
        ));
    }
    (provider.source <= held.provider.source).then(|| {
        format!("`{name}` belongs to {owner}, in a scope as near: an override must be nearer")
    })
}

/// `plugins`' files: each with no manifest behind it, and the command that
/// keeps it from running. Only meta's own commands do that today: the
/// router runs the file of any other word.
fn files(index: &Index, plugins: &SubprocessPluginManager) -> Vec<PluginFile> {
    plugins
        .files_by_word()
        .into_iter()
        .map(|(word, path)| {
            let shadowed_by = index
                .commands
                .iter()
                .find(|held| {
                    held.provider.source == Source::Host
                        && held.name.split(' ').next() == Some(word.as_str())
                })
                .map(|held| held.name.split(' ').next().unwrap_or_default().to_string());
            PluginFile {
                word,
                path,
                shadowed_by,
            }
        })
        .collect()
}

/// The command `words` reach, and how many of them name it: the longest
/// declared command they start with, by name or by the alias of its last
/// word.
pub fn resolve<'a>(index: &'a Index, words: &[String]) -> Option<(&'a Indexed, usize)> {
    (1..=words.len()).rev().find_map(|len| {
        let typed = words.get(..len)?;
        let held = index.commands.iter().find(|held| names(held, typed))?;
        Some((held, len))
    })
}

fn names(held: &Indexed, typed: &[String]) -> bool {
    let declared: Vec<&str> = held.name.split(' ').collect();
    let Some(((last, words), (last_typed, typed))) = declared.split_last().zip(typed.split_last())
    else {
        return false;
    };
    words.len() == typed.len()
        && words.iter().zip(typed).all(|(a, b)| a == b)
        && (last == last_typed || held.command.alias.contains(last_typed))
}

/// The commands under `words`, by name.
pub fn under<'a>(index: &'a Index, words: &[String]) -> Vec<&'a Indexed> {
    let prefix = words.join(" ");
    index
        .commands
        .iter()
        .filter(|held| is_under(&held.name, &prefix))
        .collect()
}

/// The first word of every command that shows, sorted, each once.
pub fn top_words(index: &Index) -> Vec<&str> {
    let mut words: Vec<&str> = index
        .commands
        .iter()
        .filter(|held| !held.command.hidden)
        .filter_map(|held| held.name.split(' ').next())
        .collect();
    words.sort_unstable();
    words.dedup();
    words
}

/// The index cut down to what `words` ask about: the command, or the
/// commands under them.
pub fn about(index: &Index, words: &[String]) -> Option<Index> {
    let found = resolve(index, words).filter(|(held, len)| *len == words.len() || held.command.raw);
    let mut commands: Vec<Indexed> = found.iter().map(|(held, _)| (*held).clone()).collect();
    if found.is_none_or(|(_, len)| len == words.len()) {
        commands.extend(under(index, words).into_iter().cloned());
    }
    (!commands.is_empty()).then(|| Index {
        commands,
        ..Index::default()
    })
}

/// The `answered by:` line of a command only a manifest declares: who
/// declares it, and what help should say of it.
pub fn answered_by(held: &Indexed) -> String {
    let provider = &held.provider;
    let version = provider
        .version
        .as_deref()
        .map(|version| format!(" {version}"))
        .unwrap_or_default();
    let place = place_of(&provider.path);
    let line = format!("answered by: {}{version} ({place})", provider.name);
    meta_plugin::display::escape(&format!("{line}{}", noted(held))).into_owned()
}

/// What help should say of the command, after who answers it.
pub fn noted(held: &Indexed) -> String {
    notes(held)
        .iter()
        .map(|note| format!(" · {note}"))
        .collect()
}

fn notes(held: &Indexed) -> Vec<String> {
    let command = &held.command;
    let mut notes: Vec<String> = held
        .disabled
        .iter()
        .map(|why| format!("disabled: {why}"))
        .collect();
    if command.raw {
        notes.push("runs as typed".to_string());
    }
    if !command.only_if.is_empty() {
        notes.push(format!(
            "only where there is {}",
            command.only_if.join(" or ")
        ));
    }
    if command.network {
        notes.push("uses the network".to_string());
    }
    notes
}

/// The providers with a manifest of their own and no `meta-<word>` file
/// among `plugins`, one line each in `meta plugin list`. meta itself isn't
/// one, nor is a manifest built into meta: that says what a plugin would
/// answer, not that it is installed.
pub fn manifest_only(
    index: &Index,
    plugins: &[crate::subprocess_plugins::SubprocessPlugin],
) -> Vec<Provider> {
    let mut providers: Vec<Provider> = Vec::new();
    for held in &index.commands {
        let provider = &held.provider;
        let file = plugins.iter().any(|plugin| plugin.word == provider.name);
        if provider.source > Source::Embedded && !file && !providers.contains(provider) {
            providers.push(provider.clone());
        }
    }
    providers
}

/// `problem`, as one line.
pub fn problem_line(problem: &Problem) -> String {
    use meta_plugin::display::escape;
    let command = problem
        .command
        .as_deref()
        .map(|command| format!(" `{}`", escape(command)))
        .unwrap_or_default();
    format!(
        "Problem in {}{command}: {}",
        escape(&problem.place),
        escape(&problem.message)
    )
}

/// What help says after the commands: each file that never runs, and each
/// problem. Empty when there is nothing to say.
pub fn notices(index: &Index) -> String {
    use meta_plugin::display::{escape, escape_path};
    let mut text = String::new();
    for file in &index.files {
        if let Some(command) = &file.shadowed_by {
            text.push_str(&format!(
                "\n{} never runs: `meta {}` is meta's own command. Delete the file.\n",
                escape_path(&file.path),
                escape(command)
            ));
        }
    }
    for problem in &index.problems {
        text.push_str(&format!("\n{}\n", problem_line(problem)));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(source: Source, place: &str, text: &str) -> Found {
        Found {
            source,
            path: Some(PathBuf::from(place)),
            manifest: Manifest::parse(text),
        }
    }

    const GIT: &str = r#"
name = "git"
run = ["meta-git"]
[commands.git]
raw = true
[commands."git pull"]
effects = "write"
[commands."git clean"]
raw = true
effects = "destructive"
"#;

    fn first_party() -> Found {
        found(Source::Embedded, "first", GIT)
    }

    fn provider_of<'a>(index: &'a Index, name: &str) -> &'a str {
        let held = index.commands.iter().find(|held| held.name == name);
        held.map_or("nobody", |held| held.provider.name.as_str())
    }

    fn other(commands: &str) -> String {
        format!("name = \"other\"\nrun = [\"o\"]\n{commands}")
    }

    #[test]
    fn a_second_provider_without_overrides_is_disabled_and_the_first_keeps_the_command() {
        let text = other("[commands.\"git pull\"]");
        let index = build(vec![first_party(), found(Source::Workspace, "w", &text)]);
        assert_eq!(provider_of(&index, "git pull"), "git");
        assert_eq!(index.problems.len(), 1, "{:?}", index.problems);
        assert_eq!(index.problems[0].place, "w");
        assert!(index.problems[0].message.contains("overrides"));
    }

    #[test]
    fn raw_words_belong_to_their_provider() {
        let plain = other("[commands.\"git foo\"]");
        let index = build(vec![first_party(), found(Source::Workspace, "w", &plain)]);
        assert_eq!(provider_of(&index, "git foo"), "nobody");
        assert!(index.problems[0].message.contains("overrides"));

        let declared = format!("overrides = [\"git foo\"]\n{plain}");
        let index = build(vec![
            first_party(),
            found(Source::Workspace, "w", &declared),
        ]);
        // allowed, and waiting: what runs `git foo` is still git's
        assert_eq!(provider_of(&index, "git foo"), "nobody");
        assert!(index.problems.is_empty(), "{:?}", index.problems);
        let held = index.commands.iter().find(|h| h.name == "git");
        assert_eq!(held.unwrap().waiting[0].name, "other");
    }

    #[test]
    fn an_override_is_nearer_or_refused_and_waits_without_taking_the_command() {
        let text = format!(
            "overrides = [\"git pull\"]\n{}",
            other("[commands.\"git pull\"]")
        );
        let index = build(vec![first_party(), found(Source::User, "u", &text)]);
        assert_eq!(provider_of(&index, "git pull"), "git");
        let held = index.commands.iter().find(|h| h.name == "git pull");
        assert_eq!(held.unwrap().waiting[0].name, "other");

        let index = build(vec![first_party(), found(Source::Embedded, "same", &text)]);
        assert_eq!(provider_of(&index, "git pull"), "git");
        assert!(index.problems[0].message.contains("nearer"));
    }

    #[test]
    fn destructive_commands_help_and_plugin_are_never_overridden() {
        for name in ["git clean", "git clean foo", "help", "plugin list"] {
            let command = format!("overrides = [\"{name}\"]\n");
            let text = format!("{command}{}", other(&format!("[commands.\"{name}\"]")));
            let index = build(vec![first_party(), found(Source::Workspace, "w", &text)]);
            assert_ne!(provider_of(&index, name), "other", "{name}");
            assert!(index.commands.iter().all(|held| held.waiting.is_empty()));
            assert!(index.problems[0].message.contains("can't be overridden"));
        }
    }

    #[test]
    fn a_bad_manifest_or_command_disables_only_itself() {
        let half = other("[commands.good]\n[commands.bad]\nrun = [\"p\"]\nplan = [\"q\"]");
        let index = build(vec![
            first_party(),
            found(Source::Workspace, "broken", "name ="),
            found(Source::Workspace, "half", &half),
            found(
                Source::Workspace,
                "theirs",
                "name = \"x\"\nmeta_options = [{ long = \"y\" }]",
            ),
        ]);
        assert_eq!(provider_of(&index, "git"), "git");
        assert_eq!(provider_of(&index, "good"), "other");
        let held = index.commands.iter().find(|h| h.name == "bad").unwrap();
        assert!(held
            .disabled
            .as_deref()
            .unwrap()
            .contains("both run and plan"));
        let places: Vec<&str> = index.problems.iter().map(|p| p.place.as_str()).collect();
        assert_eq!(places, ["broken", "half", "theirs"]);
    }

    #[test]
    fn the_same_manifest_twice_in_a_scope_is_one_provider() {
        let text = other("[commands.mine]");
        let (near, far) = (
            found(Source::Workspace, "near", &text),
            found(Source::Workspace, "far", &text),
        );
        let index = build(without_copies(vec![near, far]));
        assert!(index.problems.is_empty(), "{:?}", index.problems);
        let held = index.commands.iter().find(|h| h.name == "mine").unwrap();
        assert_eq!(held.provider.path.as_deref(), Some(Path::new("near")));
    }

    #[test]
    fn the_longest_declared_command_answers_and_an_alias_reaches_it() {
        let index = build(vec![first_party()]);
        let words = |line: &str| line.split(' ').map(str::to_string).collect::<Vec<_>>();
        let name = |line| resolve(&index, &words(line)).map(|(h, n)| (h.name.clone(), n));
        assert_eq!(name("git pull --rebase"), Some(("git pull".to_string(), 2)));
        assert_eq!(name("git log -3"), Some(("git".to_string(), 1)));
        assert_eq!(name("project ls"), Some(("project list".to_string(), 2)));
        assert_eq!(name("nonesuch"), None);
    }

    #[test]
    fn the_first_party_manifests_load_with_no_problem() {
        let found = EMBEDDED.iter().map(|text| Found {
            source: Source::Embedded,
            path: None,
            manifest: Manifest::parse(text),
        });
        let index = build(found.collect());
        assert!(index.problems.is_empty(), "{:?}", index.problems);
        assert!(index.commands.iter().all(|held| held.disabled.is_none()));
    }
}
