#![allow(unknown_lints)]

pub mod artifacts;
pub mod cli;
pub mod commands;
pub mod manifest;
pub mod profile;
pub mod stellar;
pub mod types;

use clap::{error::ErrorKind, Parser};

pub fn run() -> anyhow::Result<()> {
    let raw_args: Vec<String> = std::env::args().collect();
    let expanded_args = profile::expand_args(&raw_args)?;
    let cli = match cli::Cli::try_parse_from(expanded_args.clone()) {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            error.print()?;
            std::process::exit(error.exit_code());
        }
        Err(error)
            if expanded_args
                .iter()
                .any(|arg| arg == "--json" || arg == "--json-lines") =>
        {
            commands::print_parse_error(&expanded_args, &error)?;
            std::process::exit(error.exit_code());
        }
        Err(error) => return Err(error.into()),
    };
    match commands::run(&cli, &stellar::RealExecutor) {
        Ok(()) => Ok(()),
        Err(error) if cli.json || cli.json_lines => {
            commands::print_error(&cli, &error)?;
            std::process::exit(1);
        }
        Err(error) => Err(error),
    }
}
