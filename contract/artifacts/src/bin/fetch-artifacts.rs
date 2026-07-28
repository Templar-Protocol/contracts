//! Manage the shared cache of released contract WASM.
//!
//! Run via `just artifacts-fetch` / `artifacts-cache-path` / `artifacts-clean`.
//! Cache location is resolved through [`fetch`] so shell callers cannot drift
//! from the crate.

use std::process::ExitCode;

use clap::Parser;
use templar_contract_artifacts::fetch;

/// With no flags, downloads every pinned release in the catalog.
#[derive(Debug, Parser)]
struct Args {
    /// Print the resolved cache directory and exit.
    #[arg(long)]
    print_path: bool,

    /// Delete every cached artifact. Entries are immutable release assets, so
    /// the only cost is a re-download.
    #[arg(long, conflicts_with = "print_path")]
    clean: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();

    if args.print_path {
        return print_path();
    }
    if args.clean {
        return clean();
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

fn clean() -> ExitCode {
    match fetch::clean() {
        Ok(removed) if removed.files == 0 => {
            println!("artifact cache already empty ({})", root_label());
            ExitCode::SUCCESS
        }
        Ok(removed) => {
            println!(
                "removed {} cached artifact(s), {} freed from {}",
                removed.files,
                human_bytes(removed.bytes),
                root_label(),
            );
            println!("re-warm with `just artifacts-fetch`");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to clean the artifact cache: {error}");
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

fn human_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    const KIB: f64 = 1024.0;

    #[expect(
        clippy::cast_precision_loss,
        reason = "display only, and a cache this size cannot lose a meaningful digit"
    )]
    let bytes = bytes as f64;

    if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_are_mutually_exclusive_and_trailing_arguments_are_rejected() {
        for argv in [
            vec!["fetch-artifacts", "--clean", "--print-path"],
            vec!["fetch-artifacts", "--clean", "stray"],
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
        assert!(!args.print_path && !args.clean);
    }

    #[test]
    fn human_bytes_switches_unit_at_each_boundary() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1024 * 1024 - 1), "1024.0 KiB");
        assert_eq!(human_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(human_bytes(6_710_886), "6.4 MiB");
        // Far past any plausible cache, but the formatter must not wrap or panic.
        assert_eq!(human_bytes(u64::MAX), "17592186044416.0 MiB");
    }
}
