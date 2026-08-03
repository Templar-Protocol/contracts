//! Manage the shared cache of released contract WASM.
//!
//! Run via `just artifacts-fetch` and `just artifacts-cache-path`. Deleting the
//! cache is `just artifacts-clean`, which removes the directory this prints —
//! the cache is disposable, so it needs no code of its own.

use std::process::ExitCode;

use clap::Parser;
use templar_contract_artifacts::fetch;

/// With no flags, downloads every pinned release in the catalog.
#[derive(Debug, Parser)]
struct Args {
    /// Print the resolved cache directory and exit.
    #[arg(long)]
    print_path: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();

    if args.print_path {
        return print_path();
    }
    prefetch()
}

fn prefetch() -> ExitCode {
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

    match runtime.block_on(fetch::prefetch_all()) {
        Ok(count) => {
            println!("{count} released artifact(s) cached under {}", root_label());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to populate the artifact cache: {error}");
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
