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

use std::process::ExitCode;

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

    // `create_new` is the one-call create-or-fail primitive, which makes "never
    // rewrite a release" a property of the syscall rather than a check. An
    // interrupted write can leave a truncated row, but only in an uncommitted
    // working tree: the workflow rebuilds the crate before staging, and build.rs
    // rejects a malformed row, so the job fails and nothing reaches the repo.
    let created = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, row.as_bytes()));

    match created {
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
