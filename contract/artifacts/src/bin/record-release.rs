//! Append a release to the artifact catalog.
//!
//! Run by `.github/workflows/release-artifacts.yml` after it has built and
//! uploaded a release's WASM, against the hash of the bytes it just built. The
//! resulting diff goes up as a PR for a human to merge.
//!
//! Nobody edits the release list by hand. The catalog records what was
//! *deployed*, and deployment is not something a `Cargo.toml` bump can assert:
//! contract versions have repeatedly been bumped during development and never
//! shipped. Letting CI append after the fact is what keeps the catalog honest —
//! and it means there is no step for a developer to forget.
//!
//! ```bash
//! cargo run -p templar-contract-artifacts --bin record-release -- proxy-oracle 0.4.0 <sha256>
//! ```

use std::process::ExitCode;

use templar_contract_artifacts::ArtifactId;

fn main() -> ExitCode {
    match run() {
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

fn run() -> Result<String, String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let [artifact, version, sha] = args.as_slice() else {
        return Err("usage: record-release <artifact> <version> <sha256>".to_owned());
    };

    if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{sha} is not a 64-char hex SHA-256"));
    }
    let sha = sha.to_ascii_lowercase();

    let artifact = artifact
        .parse::<ArtifactId>()
        .map_err(|error| error.to_string())?;
    let metadata = artifact.metadata();

    // Releases are immutable, so re-recording one is either a no-op replay of
    // the same job or a genuine conflict. Never a rewrite.
    if let Some(existing) = metadata.release(version) {
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

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ids.rs");
    let source = std::fs::read_to_string(&path).map_err(|error| format!("{path:?}: {error}"))?;
    let patched = append(&source, metadata.package_name, version, &sha)?;
    std::fs::write(&path, patched).map_err(|error| format!("{path:?}: {error}"))?;

    Ok(format!("recorded {artifact}@{version} as {sha}"))
}

/// Marks the start of the catalog. Package names also appear above it, in the
/// `clap` value aliases on `ArtifactId`, so the search has to start here or the
/// uniqueness check below never holds.
const CATALOG_MARKER: &str = "macro_rules! entry {";

/// Append `("<version>", "<sha>"),` to one artifact's release list.
///
/// Scoped to the artifact's own catalog block, located by its package name —
/// unique within the catalog — so artifacts sharing a version number cannot be
/// confused. A surprising shape fails loudly rather than corrupting the
/// catalog; `cargo fmt` tidies the result afterwards.
fn append(source: &str, package_name: &str, version: &str, sha: &str) -> Result<String, String> {
    let catalog_at = source
        .find(CATALOG_MARKER)
        .ok_or_else(|| format!("ids.rs has no `{CATALOG_MARKER}`; catalog layout changed"))?;
    let catalog = &source[catalog_at..];

    let package_literal = format!("\"{package_name}\"");
    let block_start = find_unique(catalog, &package_literal)
        .map(|offset| catalog_at + offset)
        .ok_or_else(|| {
            format!("{package_literal} does not appear exactly once in the ids.rs catalog")
        })?;
    let block_end = source[block_start..]
        .find(");")
        .map(|offset| block_start + offset)
        .ok_or_else(|| format!("no end of catalog block for {package_name}"))?;

    // The release list is the last `]` inside the entry! invocation.
    let close = source[block_start..block_end]
        .rfind(']')
        .map(|offset| block_start + offset)
        .ok_or_else(|| format!("no release list in the {package_name} block"))?;

    let mut patched = String::with_capacity(source.len() + sha.len() + 32);
    patched.push_str(&source[..close]);
    patched.push_str("(\"");
    patched.push_str(version);
    patched.push_str("\", \"");
    patched.push_str(sha);
    patched.push_str("\"),");
    patched.push_str(&source[close..]);
    Ok(patched)
}

fn find_unique(haystack: &str, needle: &str) -> Option<usize> {
    let first = haystack.find(needle)?;
    if haystack[first + needle.len()..].contains(needle) {
        return None;
    }
    Some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors the real file closely enough to matter: the `clap` alias above
    /// the catalog repeats the package name, which is what makes a whole-file
    /// uniqueness check wrong.
    const SOURCE: &str = r#"
pub enum ArtifactId {
    #[cfg_attr(feature = "clap", value(alias = "templar-proxy-oracle-near-contract"))]
    ProxyOracle,
}

macro_rules! entry {
    ($id:ident, $pkg:expr, $target:expr, $src:expr, [$(($ver:expr, $sha:expr)),* $(,)?]) => {};
}

static PROXY_ORACLE_METADATA: ArtifactMetadata = entry!(
    ProxyOracle,
    "templar-proxy-oracle-near-contract",
    "templar_proxy_oracle_near_contract",
    "contract/proxy-oracle/near/contract",
    [("0.3.0", "aa"),]
);
static VAULT_METADATA: ArtifactMetadata = entry!(
    Vault,
    "templar-vault-contract",
    "templar_vault_contract",
    "contract/vault/near",
    []
);
"#;

    #[test]
    fn appends_to_the_named_artifacts_release_list() {
        let patched = append(SOURCE, "templar-proxy-oracle-near-contract", "0.4.0", "ff").unwrap();
        assert!(
            patched.contains(r#"[("0.3.0", "aa"),("0.4.0", "ff"),]"#),
            "{patched}"
        );
        // The other artifact must be untouched.
        assert!(patched.contains("\"contract/vault/near\",\n    []"));
    }

    #[test]
    fn seeds_an_empty_release_list() {
        let patched = append(SOURCE, "templar-vault-contract", "1.0.0", "ff").unwrap();
        assert!(patched.contains(r#"[("1.0.0", "ff"),]"#), "{patched}");
        // proxy-oracle keeps exactly its one release.
        assert!(patched.contains(r#"[("0.3.0", "aa"),]"#));
    }

    #[test]
    fn refuses_an_unknown_package() {
        let error =
            append(SOURCE, "templar-nope-contract", "1.0.0", "ff").expect_err("not in the catalog");
        assert!(error.contains("exactly once"), "{error}");
    }
}
