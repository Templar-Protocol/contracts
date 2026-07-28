//! Append a release to `contract/artifacts/releases.tsv`.
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

use std::{fmt::Write as _, process::ExitCode};

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

/// Relative to this crate's manifest directory.
const RELEASES: &str = "releases.tsv";

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

    if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{sha} is not a 64-char hex SHA-256"));
    }
    let sha = sha.to_ascii_lowercase();

    // A tab would split one field into two and shift every later column. These
    // land in a data file, not generated source, so nothing else needs policing.
    for (name, value) in [("version", version), ("tag", tag), ("asset", asset)] {
        if value.is_empty() || value.contains('\t') || value.contains('\n') {
            return Err(format!(
                "{name} `{value}` is empty or contains a tab/newline"
            ));
        }
    }

    // Releases are immutable, so re-recording one is either a no-op replay of
    // the same job or a genuine conflict. Never a rewrite.
    if let Some(existing) = artifact.metadata().release(version) {
        return if existing.sha256 == sha {
            Ok(format!("{artifact}@{version} is already recorded as {sha}"))
        } else {
            Err(format!(
                "{artifact}@{version} is already recorded as {}, refusing to \
                 rewrite it to {sha}. Released bytes are immutable.",
                existing.sha256,
            ))
        };
    }

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(RELEASES);
    let table = std::fs::read_to_string(&path).map_err(|e| format!("{path:?}: {e}"))?;
    let mut row = String::new();
    writeln!(row, "{artifact}\t{version}\t{tag}\t{asset}\t{sha}")
        .map_err(|e| format!("building the release row: {e}"))?;

    std::fs::write(&path, insert_row(&table, &artifact.to_string(), &row))
        .map_err(|e| format!("{path:?}: {e}"))?;

    Ok(format!("recorded {artifact}@{version} as {sha} ({tag})"))
}

/// Insert `row` after the artifact's existing rows, or at the end if it has
/// none.
///
/// Not a plain append: one release PR can tag several contracts, and each tag
/// runs this in its own branch off the same `dev`. Appending would put every
/// row at the same offset, so the second catalog PR to merge would conflict.
/// Grouped by artifact, they touch different regions and merge cleanly.
fn insert_row(table: &str, artifact: &str, row: &str) -> String {
    let mut lines = table.lines().map(str::to_owned).collect::<Vec<_>>();
    let after = lines
        .iter()
        .rposition(|line| line.split('\t').next() == Some(artifact))
        .map_or(lines.len(), |index| index + 1);
    lines.insert(after, row.trim_end().to_owned());
    lines
        .into_iter()
        .map(|line| line + "\n")
        .collect::<String>()
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
                .expect_err("should be rejected before touching releases.tsv");
            assert!(
                error.contains("empty or contains a tab/newline"),
                "{label}: {error}"
            );
        }
    }

    #[test]
    fn a_row_lands_in_its_own_artifact_group() {
        // Two contracts released in one batch each open a catalog PR off the
        // same `dev`; grouped inserts keep those PRs from conflicting.
        let table =
            "# header\nregistry\t1.0.0\ta\tb\tc\nmarket\t1.0.0\ta\tb\tc\nmarket\t1.1.0\ta\tb\tc\n";

        let with_registry = insert_row(table, "registry", "registry\t1.1.0\tt\tw\td\n");
        let rows: Vec<&str> = with_registry
            .lines()
            .map(|l| l.split('\t').next().unwrap())
            .collect();
        assert_eq!(
            rows,
            ["# header", "registry", "registry", "market", "market"]
        );

        // A first-ever release for an artifact still goes at the end.
        let with_vault = insert_row(table, "vault", "vault\t1.0.0\tt\tw\td\n");
        assert!(with_vault.ends_with("vault\t1.0.0\tt\tw\td\n"));
        assert_eq!(with_vault.lines().count(), 5);
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
