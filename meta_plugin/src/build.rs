//! Rust types from a manifest, for a plugin's build script:
//!
//! ```no_run
//! // build.rs
//! fn main() -> Result<(), String> {
//!     meta_plugin::build::generate("meta-plugin.toml")
//! }
//! ```
//!
//! and `meta_plugin::include_commands!();` where the types belong: one
//! struct of typed options per command, and an enum `Command` over them
//! that [`Command::decode`](#decode) fills from a request's options. A
//! manifest change is a compile error where the option is used. The
//! generated file is plain Rust in `OUT_DIR`, and reads well.

use crate::manifest::{Command, Manifest, Opt, Type};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;

/// Write the types of `manifest` (a path from the crate's folder) to
/// `commands.rs` in `OUT_DIR`. A manifest with a problem is the error: a
/// plugin never builds from a declaration the host would disable.
pub fn generate(manifest: &str) -> Result<(), String> {
    generate_to(manifest, "commands.rs")
}

/// [`generate`], to `file` in `OUT_DIR`.
pub fn generate_to(manifest: &str, file: &str) -> Result<(), String> {
    #[expect(
        clippy::print_stdout,
        reason = "a build script talks to cargo on stdout"
    )]
    {
        println!("cargo:rerun-if-changed={manifest}");
    }
    let dir = std::env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?;
    let out = std::env::var("OUT_DIR").map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(Path::new(&dir).join(manifest))
        .map_err(|e| format!("{manifest}: {e}"))?;
    let parsed = Manifest::parse(&text).map_err(|e| format!("{manifest}: {e}"))?;
    let code = render(&parsed).map_err(|e| format!("{manifest}: {e}"))?;
    std::fs::write(Path::new(&out).join(file), code).map_err(|e| format!("{file}: {e}"))
}

/// The Rust source for `manifest`.
pub fn render(manifest: &Manifest) -> Result<String, String> {
    let mut problems = manifest.problems();
    problems.extend(crate::manifest::option_set_problems(&manifest.meta_options));
    for (name, command) in &manifest.commands {
        for problem in manifest.command_problems(name, command) {
            problems.push(format!("{name}: {problem}"));
        }
    }
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    let mut names = Names::default();
    let mut code = String::from("// Generated from the manifest by meta_plugin::build.\n");
    if !manifest.meta_options.is_empty() {
        let doc = "meta's own options, which go before the command";
        names.take("MetaOptions")?;
        code.push_str(&structure(
            "MetaOptions",
            doc,
            &manifest.meta_options,
            false,
        )?);
    }
    let mut variants = Vec::new();
    for (name, command) in &manifest.commands {
        let variant = variant(name);
        let structure_name = format!("{variant}Options");
        names.take(&structure_name)?;
        code.push_str(&command_structure(&structure_name, command)?);
        variants.push((name.as_str(), variant, structure_name));
    }
    if !variants.is_empty() {
        code.push_str(&command_enum(&variants));
    }
    Ok(code)
}

/// The type names taken so far: two names that come out the same are an
/// error, not a build failure in generated code.
#[derive(Default)]
struct Names(HashSet<String>);

impl Names {
    fn take(&mut self, name: &str) -> Result<(), String> {
        match self.0.insert(name.to_string()) {
            true => Ok(()),
            false => Err(format!("two declarations both make the Rust name {name}")),
        }
    }
}

fn command_structure(name: &str, command: &Command) -> Result<String, String> {
    let mut options = command.options.clone();
    // a raw command's words are one list, by the name `plan` knows them
    if command.raw {
        options.push(Opt {
            positional: Some("raw".to_string()),
            rest: true,
            help: "The words after the command, as typed".to_string(),
            ..Opt::default()
        });
    }
    structure(name, &command.summary, &options, true)
}

fn structure(name: &str, doc: &str, options: &[Opt], strict: bool) -> Result<String, String> {
    let mut code = String::new();
    let mut fields = Names::default();
    let _ = writeln!(code, "\n#[doc = {doc:?}]");
    let _ = writeln!(
        code,
        "#[derive(Debug, Clone, PartialEq, ::serde::Deserialize)]"
    );
    if strict {
        let _ = writeln!(code, "#[serde(deny_unknown_fields)]");
    }
    let _ = writeln!(code, "pub struct {name} {{");
    for option in options {
        let field = field(option.id());
        fields.take(&field)?;
        let (kind, attributes) = field_type(option);
        let id = option.id();
        let _ = writeln!(code, "    #[doc = {:?}]", option.help);
        let _ = writeln!(code, "    #[serde(rename = {id:?}{attributes})]");
        let _ = writeln!(code, "    pub {field}: {kind},");
    }
    code.push_str("}\n");
    Ok(code)
}

/// The Rust type of `option`, and what its field needs from serde.
fn field_type(option: &Opt) -> (String, String) {
    if option.is_flag() {
        return ("bool".to_string(), ", default".to_string());
    }
    let base = match option.kind {
        Type::String if option.is_words() => "::meta_plugin::Word",
        Type::String | Type::Bool => "::std::string::String",
        Type::Int => "i64",
        Type::Uint => "usize",
        Type::Path => "::std::path::PathBuf",
        Type::Duration => "::meta_plugin::duration::Seconds",
    };
    let path = option.kind == Type::Path;
    let with = |module: &str| match path {
        true => format!(", with = \"::meta_plugin::wire_path{module}\""),
        false => String::new(),
    };
    if option.is_list() {
        let attributes = format!(", default{}", with("::list"));
        return (format!("::std::vec::Vec<{base}>"), attributes);
    }
    match option.required || option.default.is_some() {
        true => (base.to_string(), with("")),
        false => {
            let attributes = format!(", default{}", with("::option"));
            (format!("::std::option::Option<{base}>"), attributes)
        }
    }
}

fn command_enum(variants: &[(&str, String, String)]) -> String {
    let mut code = String::from(
        "\n/// A command of this manifest, with its options.\n\
         #[derive(Debug, Clone, PartialEq)]\npub enum Command {\n",
    );
    for (name, variant, structure) in variants {
        let _ = writeln!(code, "    #[doc = {name:?}]");
        let _ = writeln!(code, "    {variant}({structure}),");
    }
    code.push_str(
        "}\n\nimpl Command {\n    \
         /// The command `name` with the options a request carries; `None` for a\n    \
         /// name this manifest doesn't declare.\n    \
         pub fn decode(\n        name: &str,\n        options: ::serde_json::Value,\n    \
         ) -> ::std::result::Result<::std::option::Option<Self>, ::serde_json::Error> {\n        \
         match name {\n",
    );
    for (name, variant, _) in variants {
        let _ = writeln!(
            code,
            "            {name:?} => ::serde_json::from_value(options).map(|o| Some(Self::{variant}(o))),"
        );
    }
    code.push_str("            _ => Ok(None),\n        }\n    }\n}\n");
    code
}

/// `git worktree-list` as `GitWorktreeList`.
fn variant(name: &str) -> String {
    let mut pascal = String::new();
    for word in name.split(|c: char| !c.is_alphanumeric()) {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            pascal.extend(first.to_uppercase());
            pascal.push_str(chars.as_str());
        }
    }
    let starts_badly = pascal.chars().next().is_none_or(|c| !c.is_alphabetic());
    match starts_badly || pascal == "Self" {
        true => format!("Command{pascal}"),
        false => pascal,
    }
}

/// An option's id as a field: hyphens become underscores, and a keyword is
/// escaped, or has `_` added where Rust has no escape for it.
fn field(id: &str) -> String {
    const NO_RAW_FORM: [&str; 4] = ["self", "Self", "super", "crate"];
    const KEYWORDS: [&str; 47] = [
        "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "do",
        "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl", "in",
        "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
        "return", "static", "struct", "trait", "true", "try", "type", "typeof", "unsafe",
        "unsized", "use", "virtual", "where", "while",
    ];
    let snake: String = id
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let snake = match snake.chars().next().is_none_or(|c| c.is_numeric()) {
        true => format!("_{snake}"),
        false => snake,
    };
    match (
        NO_RAW_FORM.contains(&snake.as_str()),
        KEYWORDS.contains(&snake.as_str()),
    ) {
        (true, _) => format!("{snake}_"),
        (_, true) => format!("r#{snake}"),
        _ => snake,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(text: &str) -> Manifest {
        Manifest::parse(text).unwrap()
    }

    #[test]
    fn names_are_rust_whatever_the_manifest_says() {
        assert_eq!(variant("git worktree-list"), "GitWorktreeList");
        assert_eq!(variant("2fa"), "Command2fa");
        assert_eq!(variant("self"), "CommandSelf");
        assert_eq!(field("every-repo"), "every_repo");
        assert_eq!(field("type"), "r#type");
        assert_eq!(field("self"), "self_");
        assert_eq!(field("2fa"), "_2fa");
    }

    #[test]
    fn two_names_that_come_out_the_same_are_an_error() {
        let commands = "name = \"a\"\nrun = [\"p\"]\n[commands.\"x-y\"]\n[commands.\"x y\"]";
        assert!(render(&manifest(commands))
            .unwrap_err()
            .contains("XYOptions"));
        let options = "name = \"a\"\nrun = [\"p\"]\n[commands.x]\n\
                       options = [{ long = \"a-b\" }, { long = \"a_b\" }]";
        assert!(render(&manifest(options)).unwrap_err().contains("a_b"));
    }

    #[test]
    fn a_manifest_the_host_would_disable_builds_nothing() {
        let both = "name = \"a\"\n[commands.x]\nrun = [\"p\"]\nplan = [\"q\"]";
        assert!(render(&manifest(both))
            .unwrap_err()
            .contains("both run and plan"));
    }
}
