#![allow(unknown_lints)]

pub mod artifacts;
pub mod cli;
pub mod commands;
pub mod manifest;
pub mod profile;
pub mod stellar;
pub mod types;

use clap::{error::ErrorKind, Parser};
use tracing_subscriber::{
    fmt, layer::SubscriberExt as _, util::SubscriberInitExt as _, EnvFilter, Layer as _,
};

const LOG_ENV: &str = "TEMPLAR_SOROBAN_VAULT_LOG";

pub fn run() -> anyhow::Result<()> {
    init_tracing();
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

fn init_tracing() {
    let filter = EnvFilter::try_from_env(LOG_ENV)
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("off"));

    let subscriber = tracing_subscriber::registry().with(
        fmt::layer()
            .compact()
            .with_writer(std::io::stderr)
            .with_target(false)
            .with_filter(filter),
    );

    let _ = subscriber.try_init();
}
