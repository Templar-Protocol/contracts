//! Warm the shared artifact cache with every pinned release in the catalog.
//!
//! Run via `just artifacts-fetch`. CI runs it before the sandbox tests so a
//! network failure surfaces as one clear step rather than scattered test
//! failures.

use std::process::ExitCode;

use templar_contract_artifacts::{fetch, ArtifactId};

fn main() -> ExitCode {
    // `--print-assets` lets shell tooling (the backfill script) enumerate the
    // tag and asset name of every pinned release without re-deriving the
    // naming convention and drifting from `fetch::asset_url`.
    if std::env::args().nth(1).as_deref() == Some("--print-assets") {
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
        return ExitCode::SUCCESS;
    }

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
            let root = fetch::cache_root().map_or_else(
                |error| format!("<unknown: {error}>"),
                |path| path.display().to_string(),
            );
            println!("{count} released artifact(s) cached under {root}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to populate the artifact cache: {error}");
            ExitCode::FAILURE
        }
    }
}
