//! Record a release under `contract/artifacts/releases/`.
//!
//! Run by `.github/workflows/release-artifacts.yml` after it has built and
//! uploaded a release's WASM; the one-line diff goes up as a PR. Never edited by
//! hand — the catalog records what was *deployed*, which a `Cargo.toml` bump
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

/// Three numeric components, nothing else — the shape `build.rs` sorts by.
fn is_major_minor_patch(version: &str) -> bool {
    let mut parts = version.split('.');
    let numeric = |part: Option<&str>| part.is_some_and(|p| p.parse::<u64>().is_ok());
    numeric(parts.next())
        && numeric(parts.next())
        && numeric(parts.next())
        && parts.next().is_none()
}

fn record(args: &Args) -> Result<String, String> {
    let Args {
        artifact,
        version,
        tag,
        asset,
        sha256: sha,
    } = args;

    if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{sha} is not a 64-char hex SHA-256"));
    }
    let sha = sha.to_ascii_lowercase();

    // A tab would split one field into two and shift every later column.
    for (name, value) in [("version", version), ("tag", tag), ("asset", asset)] {
        if value.is_empty() || value.contains(['\t', '\n']) {
            return Err(format!(
                "{name} `{value}` is empty or contains a tab/newline"
            ));
        }
    }
    // Only `version` reaches the path — tags legitimately contain slashes.
    if version.contains(['/', '\\']) {
        return Err(format!("version `{version}` cannot contain a slash"));
    }
    // build.rs rejects this too, but only on the next compile — after CI has
    // committed the file, by which point every build fails.
    if !is_major_minor_patch(version) {
        return Err(format!(
            "version `{version}` is not `major.minor.patch`; releases sort by it"
        ));
    }

    let row = format!("{artifact}\t{version}\t{tag}\t{asset}\t{sha}\n");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(RELEASES)
        .join(format!("{artifact}@{version}.tsv"));

    // `create_new` makes "never rewrite a release" structural rather than a
    // check that a later edit could bypass.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            std::io::Write::write_all(&mut file, row.as_bytes())
                .map_err(|error| format!("{path:?}: {error}"))?;
            Ok(format!("recorded {artifact}@{version} as {sha} ({tag})"))
        }
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
    fn a_field_cannot_break_the_column_layout() {
        for (label, version, tag, asset) in [
            ("tab in version", "1.0.0\tevil", "t", "a.wasm"),
            ("newline in tag", "1.0.0", "t\nevil", "a.wasm"),
            ("empty asset", "1.0.0", "t", ""),
        ] {
            let error = record(&args(version, tag, asset))
                .expect_err("should be rejected before touching releases/");
            assert!(
                error.contains("empty or contains a tab/newline"),
                "{label}: {error}"
            );
        }
    }

    #[test]
    fn a_version_cannot_escape_the_releases_directory() {
        // `version` is the only field that reaches the path.
        for version in ["../evil", "a\\b"] {
            let error = record(&args(version, "t", "a.wasm"))
                .expect_err("should be rejected before touching releases/");
            assert!(
                error.contains("cannot contain a slash"),
                "{version}: {error}"
            );
        }
    }

    #[test]
    fn an_unsortable_version_is_rejected_before_it_reaches_the_catalog() {
        for version in ["1.4.0-rc1", "1.4", "1.4.0.1", "v1.4.0"] {
            let error = record(&args(version, "t", "a.wasm"))
                .expect_err("build.rs would reject this only on the next compile");
            assert!(error.contains("major.minor.patch"), "{version}: {error}");
        }
    }

    #[test]
    fn a_bad_digest_is_rejected() {
        let mut bad = args("1.0.0", "t", "a.wasm");
        bad.sha256 = "not-a-digest".to_owned();
        let error = record(&bad).expect_err("should be rejected");
        assert!(error.contains("64-char hex"), "{error}");
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
