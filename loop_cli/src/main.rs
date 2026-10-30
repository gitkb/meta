use anyhow::Result;
use clap::{CommandFactory, Parser};
use colored::*;
use loop_lib::{display_command, expand_directories, parse_config, run, LoopConfig};
use std::path::PathBuf;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
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
        help = "Specify directories to exclude (adds to config file exclusions)"
    )]
    exclude: Option<Vec<String>>,

    #[arg(
        short,
        long,
        help = "Specify directories to include (overrides config file)"
    )]
    include: Option<Vec<String>>,

    #[arg(short, long, help = "Enable silent mode (suppress all output)")]
    silent: bool,

    #[arg(short, long, help = "Enable verbose output")]
    verbose: bool,
}

fn main() -> Result<()> {
    let cli = parse_cli();

    let config_path = cli.config.unwrap_or_else(|| PathBuf::from(".looprc"));

    if cli.command.is_empty() {
        Cli::command().print_help()?;
        std::process::exit(0);
    }

    // Parse the .looprc file
    let absolute_path = std::env::current_dir()?.join(&config_path);

    if cli.verbose {
        println!("{}", "Verbose mode enabled".green());
        println!(
            "\nResolved config file path: {}",
            meta_core::display::escape_path(&absolute_path)
        );
        println!(
            "Executing command: {}",
            meta_core::display::escape(&display_command(&cli.command))
        );
    }

    let mut config = if config_path.exists() {
        parse_config(&config_path)?
    } else {
        LoopConfig::default()
    };

    // Update config with CLI options
    if let Some(include) = cli.include {
        config.directories = include;
    }
    if let Some(exclude) = cli.exclude {
        config.ignore.extend(exclude);
    }

    config.verbose = cli.verbose;
    config.silent = cli.silent;
    config.add_aliases_to_global_looprc = cli.add_aliases_to_global_looprc;

    // If no directories specified, use current and all child directories
    if config.directories.is_empty() {
        let dirs = expand_directories(&[".".to_string()], &config.ignore)?;
        config.directories = dirs;
    }

    run(&config, &cli.command)?;

    Ok(())
}

/// Parse the command line. Clap quotes rejected values as given, so an error
/// is shown from the escaped arguments: the same error, with values that
/// can't move the cursor or forge a line.
fn parse_cli() -> Cli {
    use clap::error::ErrorKind;
    Cli::try_parse().unwrap_or_else(|e| match e.kind() {
        // clap's invalid UTF-8 error doesn't echo the value
        ErrorKind::DisplayHelp
        | ErrorKind::DisplayVersion
        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        | ErrorKind::InvalidUtf8 => e.exit(),
        _ => {
            let shown = std::env::args_os()
                .map(|arg| meta_core::display::escape(&arg.to_string_lossy()).into_owned());
            match Cli::try_parse_from(shown) {
                Err(shown) => shown.exit(),
                Ok(_) => Cli::command()
                    .error(
                        ErrorKind::InvalidValue,
                        "an argument contains a control character",
                    )
                    .exit(),
            }
        }
    })
}
