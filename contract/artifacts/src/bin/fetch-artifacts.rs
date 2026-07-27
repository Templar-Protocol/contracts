//! Manage the shared cache of released contract WASM.
//!
//! Run via `just artifacts-fetch` / `artifacts-cache-path` / `artifacts-clean`.
//! CI warms the cache before the sandbox tests so a network failure surfaces as
//! one clear step rather than scattered test failures.
//!
//! Every mode resolves the cache location through [`fetch`] rather than
//! re-deriving it, so shell callers cannot drift from the crate.

use std::process::ExitCode;

use templar_contract_artifacts::{fetch, ArtifactId};

const USAGE: &str = "usage: fetch-artifacts [--print-assets | --print-path | --clean]";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        None => prefetch(),
        // Lets shell tooling (the backfill script) enumerate the tag and asset
        // name of every pinned release without re-deriving the naming
        // convention and drifting from `fetch::asset_url`.
        Some("--print-assets") => print_assets(),
        Some("--print-path") => print_path(),
        Some("--clean") => clean(),
        Some(other) => {
            eprintln!("unrecognized argument `{other}`\n{USAGE}");
            ExitCode::FAILURE
        }
    }
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

fn print_assets() -> ExitCode {
    for artifact in ArtifactId::ALL {
        let metadata = artifact.metadata();
        for release in metadata.releases {
            println!(
                "{} {}-{}.wasm {} {}",
                fetch::release_tag(artifact, release.version),
                metadata.cargo_target_name,
                release.version,
                metadata.cargo_target_name,
                release.version,
            );
        }
    }
    ExitCode::SUCCESS
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
