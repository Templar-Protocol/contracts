//! Record a release under `contract/artifacts/releases/`.
//!
//! Run by `.github/workflows/release-artifacts.yml` once it has built and
//! uploaded a release's WASM, and again by `catalog-pr.yml` to replay the row
//! onto the branch that carries the whole batch up as one PR. Never edited by
//! hand — the catalog records what was *released*, which a `Cargo.toml` bump
//! cannot assert.
//!
//! ```bash
//! cargo run -p templar-contract-artifacts --features clap --bin record-release -- \
//!   proxy-oracle 0.3.0 templar-proxy-oracle-near-contract-v0.3.0 \
//!   templar_proxy_oracle_near_contract-0.3.0.wasm <sha256> <length>
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

    /// Byte length of those same bytes.
    length: usize,
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
        length,
    } = args;
    let sha = sha.to_ascii_lowercase();

    // `version` is the only field reaching the path. The row's shape is build.rs's
    // to judge, and the workflow rebuilds before committing.
    if version.is_empty() || version.contains(['/', '\\']) || version.contains("..") {
        return Err(format!(
            "version `{version}` is empty or would escape the releases directory"
        ));
    }

    let row = format!("{artifact}\t{version}\t{tag}\t{asset}\t{sha}\t{length}\n");
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
            // Compare the recorded fields rather than the raw line: a replay is
            // still a replay if the file was reformatted. Both are derived from
            // the released bytes, so a row agreeing on one and not the other
            // describes bytes that never existed — `fetch` would refuse the
            // asset, and the deposit guard would size a deploy from a length
            // nothing was ever built at.
            let existing =
                std::fs::read_to_string(&path).map_err(|error| format!("{path:?}: {error}"))?;
            let fields = existing.trim().split('\t').collect::<Vec<_>>();
            let recorded_sha = fields.get(4).unwrap_or(&"").to_ascii_lowercase();
            let recorded_length = *fields.get(5).unwrap_or(&"");

            if recorded_sha == sha && recorded_length == length.to_string() {
                Ok(format!("{artifact}@{version} is already recorded as {sha}"))
            } else {
                Err(format!(
                    "{artifact}@{version} is already recorded as {recorded_sha} \
                     ({recorded_length} bytes), refusing to rewrite it to {sha} \
                     ({length} bytes). Released bytes are immutable."
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
            "1",
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
        replay.length = release.length;

        let message = record(&replay).expect("a replay is not an error");
        assert!(message.contains("already recorded"), "{message}");
    }

    /// Digest and length describe the same bytes, so a replay agreeing on one
    /// and not the other is not a replay.
    #[test]
    fn recording_a_different_length_for_a_recorded_digest_is_refused() {
        let release = ArtifactId::ProxyOracle
            .metadata()
            .release("0.3.0")
            .expect("0.3.0 is catalogued");
        let mut replay = args("0.3.0", release.tag, release.asset);
        replay.sha256 = release.sha256.to_owned();
        replay.length = release.length + 1;

        let error = record(&replay).expect_err("the recorded length is immutable too");
        assert!(error.contains("refusing to rewrite"), "{error}");
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
