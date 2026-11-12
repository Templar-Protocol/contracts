//! Manage the shared cache of released contract WASM.
//!
//! Run via `just artifacts-fetch` and `just artifacts-cache-path`. Deleting the
//! cache is `just artifacts-clean`, which removes the directory this prints —
//! the cache is disposable, so it needs no code of its own.

use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use templar_contract_artifacts::fetch;

/// With no flags, downloads every pinned release in the catalog.
#[derive(Debug, Parser)]
#[command(args_conflicts_with_subcommands = true)]
struct Args {
    /// Print the resolved cache directory and exit.
    #[arg(long)]
    print_path: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Download a pinned external WASM without adding a contract catalog entry.
    Download {
        #[arg(long)]
        url: String,
        #[arg(long)]
        sha256: String,
        #[arg(long)]
        length: usize,
        #[arg(long)]
        output: PathBuf,
    },
}

fn main() -> ExitCode {
    let args = Args::parse();

    if args.print_path {
        return print_path();
    }
    fetch(args.command)
}

fn fetch(command: Option<Command>) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("failed to start tokio runtime: {error}");
            return ExitCode::FAILURE;
        }
    };

    let result = runtime.block_on(async {
        match command {
            Some(Command::Download {
                url,
                sha256,
                length,
                output,
            }) => fetch::download_pinned(&url, length, &sha256, &output)
                .await
                .map(|_| format!("Verified WASM saved to {}", output.display())),
            None => fetch::prefetch_all()
                .await
                .map(|count| format!("{count} released artifact(s) cached under {}", root_label())),
        }
    });
    match result {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to fetch artifact: {error}");
            ExitCode::FAILURE
        }
    }
}

fn print_path() -> ExitCode {
    match fetch::cache_root() {
        Ok(root) => {
            println!("{}", root.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn root_label() -> String {
    fetch::cache_root().map_or_else(
        |error| format!("<unknown: {error}>"),
        |path| path.display().to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stray_and_unknown_arguments_are_rejected() {
        for argv in [
            vec!["fetch-artifacts", "stray"],
            vec!["fetch-artifacts", "--unknown"],
        ] {
            assert!(
                Args::try_parse_from(&argv).is_err(),
                "{argv:?} should have been rejected",
            );
        }
    }

    #[test]
    fn no_arguments_means_prefetch() {
        let args = Args::try_parse_from(["fetch-artifacts"]).expect("no arguments is valid");
        assert!(!args.print_path);
    }
}
