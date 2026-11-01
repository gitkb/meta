use anyhow::Result;
use clap::{CommandFactory, Parser};
use colored::*;
use loop_lib::{
    display_command, expand_directories, import_aliases, parse_config, parse_config_if_present,
    run, should_ignore, with_global_aliases,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "loop", author, version, about, long_about = None)]
struct Cli {
    #[arg(
        long,
        help = "Add shell aliases to the global .looprc file, enabling aliases within commands"
    )]
    add_aliases_to_global_looprc: bool,

    #[arg(trailing_var_arg = true)]
    command: Vec<String>,

    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    #[arg(
        short,
        long,
        value_delimiter = ',',
        help = "Specify directories to exclude (adds to config file exclusions)"
    )]
    exclude: Option<Vec<String>>,

    #[arg(
        short,
        long,
        value_delimiter = ',',
        help = "Specify directories to include (overrides config file)"
    )]
    include: Option<Vec<PathBuf>>,

    #[arg(short, long, help = "Enable silent mode (suppress all output)")]
    silent: bool,

    #[arg(short, long, help = "Enable verbose output")]
    verbose: bool,
}

fn main() -> Result<()> {
    let cli = meta_core::args::parse::<Cli>();

    // A file named with -c must exist; ./.looprc may not
    let (config_path, mut config) = match cli.config {
        Some(path) => {
            let config = parse_config(&path)?;
            (path, config)
        }
        None => {
            let path = PathBuf::from(".looprc");
            (path.clone(), parse_config_if_present(&path)?)
        }
    };

    if cli.add_aliases_to_global_looprc {
        anyhow::ensure!(
            cli.command.is_empty(),
            "--add-aliases-to-global-looprc imports your shell's aliases and runs no command"
        );
        return import_aliases(config.dry_run);
    }
    if cli.command.is_empty() {
        Cli::command().print_help()?;
        std::process::exit(0);
    }

    if cli.verbose {
        println!("{}", "Verbose mode enabled".green());
        println!(
            "\nResolved config file path: {}",
            meta_core::display::escape_path(&std::env::current_dir()?.join(&config_path))
        );
        println!(
            "Executing command: {}",
            meta_core::display::escape(&display_command(&cli.command))
        );
    }

    // Flags only turn settings on: one the file sets stays when they're absent
    if let Some(include) = cli.include {
        config.directories = include;
    }
    if let Some(exclude) = cli.exclude {
        config.ignore.extend(exclude);
    }
    config.verbose |= cli.verbose;
    config.silent |= cli.silent;
    config.aliases = with_global_aliases(config.aliases)?;

    // If no directories specified, use current and all child directories;
    // exclusions hold for directories named outright too
    if config.directories.is_empty() {
        config.directories = expand_directories(&[PathBuf::from(".")], &config.ignore)?;
    } else {
        config
            .directories
            .retain(|dir| !should_ignore(dir, &config.ignore));
    }
    anyhow::ensure!(
        !config.directories.is_empty(),
        "No directories left after exclusions"
    );

    run(&config, &cli.command)?;

    Ok(())
}
