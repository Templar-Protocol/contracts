//! Record a release under `contract/artifacts/releases/`.
//!
//! Run by `.github/workflows/release-artifacts.yml` after it has built and
//! uploaded a release's WASM; the one-line diff goes up as a PR. Never edited by
//! hand — the catalog records what was *released*, which a `Cargo.toml` bump
//! cannot assert.
//!
//! ```bash
//! cargo run -p templar-contract-artifacts --features clap --bin record-release -- \
//!   proxy-oracle 0.3.0 templar-proxy-oracle-near-contract-v0.3.0 \
//!   templar_proxy_oracle_near_contract-0.3.0.wasm <sha256>
//! ```

use std::{
    process::ExitCode,
    sync::atomic::{AtomicU64, Ordering},
};

use clap::Parser;
use templar_contract_artifacts::ArtifactId;

#[derive(Debug, Parser)]
struct Args {
    /// Catalogued artifact the release belongs to.
    artifact: ArtifactId,

    /// Version released, as the crate's Cargo.toml states it.
    version: String,

    /// Git tag carrying the release, verbatim.
    tag: String,

    /// Filename of the WASM asset uploaded to that release.
    asset: String,

    /// SHA-256 of the released bytes, as 64 hex characters.
    sha256: String,
}

/// Directory of per-release files, relative to this crate's manifest.
const RELEASES: &str = "releases";

/// Distinguishes staged files within one process; the pid alone does not, and
/// the first call to finish would delete the other's.
static STAGED: AtomicU64 = AtomicU64::new(0);

fn main() -> ExitCode {
    match record(&Args::parse()) {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("record-release: {error}");
            ExitCode::FAILURE
        }
    }
}

fn record(args: &Args) -> Result<String, String> {
    let Args {
        artifact,
        version,
        tag,
        asset,
        sha256: sha,
    } = args;
    let sha = sha.to_ascii_lowercase();

    // `version` is the only field reaching the path. The row's shape is build.rs's
    // to judge, and the workflow rebuilds before committing.
    if version.is_empty() || version.contains(['/', '\\']) || version.contains("..") {
        return Err(format!(
            "version `{version}` is empty or would escape the releases directory"
        ));
    }

    let row = format!("{artifact}\t{version}\t{tag}\t{asset}\t{sha}\n");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(RELEASES)
        .join(format!("{artifact}@{version}.tsv"));

    // Write the row in full, then install it under its final name with
    // `hard_link`, which fails rather than clobbering. So the immutable path
    // only ever appears complete: `create_new` would have reserved the name
    // before the write, and an interrupted one leaves a truncated record that a
    // retry then refuses to replace and build.rs rejects — with the append-only
    // check standing in the way of repairing it.
    let staged = path.with_extension(format!(
        "{}.{}.staged",
        std::process::id(),
        STAGED.fetch_add(1, Ordering::Relaxed),
    ));
    std::fs::write(&staged, &row).map_err(|error| format!("{staged:?}: {error}"))?;
    let installed = std::fs::hard_link(&staged, &path);
    let _ = std::fs::remove_file(&staged);

    match installed {
        Ok(()) => Ok(format!("recorded {artifact}@{version} as {sha} ({tag})")),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // Compare the recorded digest rather than the raw line: a replay is
            // still a replay if the file was reformatted.
            let existing =
                std::fs::read_to_string(&path).map_err(|error| format!("{path:?}: {error}"))?;
            let recorded = existing
                .trim()
                .split('\t')
                .nth(4)
                .unwrap_or_default()
                .to_ascii_lowercase();
            if recorded == sha {
                Ok(format!("{artifact}@{version} is already recorded as {sha}"))
            } else {
                Err(format!(
                    "{artifact}@{version} is already recorded as {recorded}, \
                     refusing to rewrite it to {sha}. Released bytes are immutable."
                ))
            }
        }
        Err(error) => Err(format!("{path:?}: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(version: &str, tag: &str, asset: &str) -> Args {
        Args::try_parse_from([
            "record-release",
            "proxy-oracle",
            version,
            tag,
            asset,
            &"a".repeat(64),
        ])
        .expect("clap takes these as opaque strings")
    }

    #[test]
    fn a_version_cannot_escape_the_releases_directory() {
        for version in ["", "../evil", "a\\b", "1.0.0/../../etc"] {
            let error = record(&args(version, "t", "a.wasm"))
                .expect_err("should be rejected before touching releases/");
            assert!(error.contains("escape the releases directory"), "{error}");
        }
    }

    #[test]
    fn replaying_an_identical_release_is_a_no_op() {
        let release = ArtifactId::ProxyOracle
            .metadata()
            .release("0.3.0")
            .expect("0.3.0 is catalogued");
        let mut replay = args("0.3.0", release.tag, release.asset);
        replay.sha256 = release.sha256.to_owned();

        let message = record(&replay).expect("a replay is not an error");
        assert!(message.contains("already recorded"), "{message}");
    }

    #[test]
    fn recording_different_bytes_for_a_recorded_version_is_refused() {
        let release = ArtifactId::ProxyOracle
            .metadata()
            .release("0.3.0")
            .expect("0.3.0 is catalogued");
        let error = record(&args("0.3.0", release.tag, release.asset))
            .expect_err("released bytes are immutable");
        assert!(error.contains("refusing to rewrite"), "{error}");
    }
}
