//! Optional `clap` integration — parsing helpers for contract artifact IDs.
//!
//! Enabled via the `clap` feature.

use crate::{artifact_catalog, ArtifactMetadata, ContractArtifact};

/// Parse a [`ContractArtifact`] from a string argument.
///
/// Accepts either the human-readable artifact name (e.g. `"market"`,
/// case-insensitive) or the Cargo package name (e.g.
/// `"templar-market-contract"`, case-sensitive).
///
/// # Errors
///
/// Returns the input and a list of valid names if no match is found.
pub fn parse_artifact_id(s: &str) -> Result<ContractArtifact, String> {
    if let Some(id) = parse_by_friendly_name(s) {
        return Ok(id);
    }
    if let Some(artifact) = crate::find_by_package_name(s) {
        return Ok(artifact.id);
    }
    let valid_names: Vec<_> = artifact_catalog()
        .iter()
        .map(|a| format!("  {} ({})", friendly_name(a.id), a.package_name))
        .collect();
    Err(format!(
        "Unknown artifact '{s}'. Valid values:\n{}",
        valid_names.join("\n")
    ))
}

/// Human-readable name for an artifact (e.g. `"market"`).
pub fn friendly_name(id: ContractArtifact) -> &'static str {
    id.friendly_name()
}

/// Find an artifact by its human-readable friendly name.
fn parse_by_friendly_name(s: &str) -> Option<ContractArtifact> {
    s.parse().ok()
}

/// Return a list of all supported friendly names for help text / completion.
pub fn friendly_names() -> Vec<&'static str> {
    artifact_catalog()
        .iter()
        .map(|a| friendly_name(a.id))
        .collect()
}

/// Return an artifact's metadata from a value parsed by [`parse_artifact_id`].
pub fn metadata_for(id: ContractArtifact) -> Option<&'static ArtifactMetadata> {
    id.metadata()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_by_friendly_name_known() {
        assert_eq!(
            parse_artifact_id("market").unwrap(),
            ContractArtifact::Market
        );
        assert_eq!(
            parse_artifact_id("MARKET").unwrap(),
            ContractArtifact::Market
        );
    }

    #[test]
    fn test_parse_by_package_name() {
        assert_eq!(
            parse_artifact_id("templar-market-contract").unwrap(),
            ContractArtifact::Market
        );
    }

    #[test]
    fn test_parse_unknown() {
        assert!(parse_artifact_id("no-such-contract").is_err());
    }

    #[test]
    fn test_friendly_names_unique() {
        let names = friendly_names();
        let mut deduped = names.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(names.len(), deduped.len(), "friendly names must be unique");
    }

    #[test]
    fn test_metadata_for() {
        let meta = metadata_for(ContractArtifact::Market).unwrap();
        assert_eq!(meta.package_name, "templar-market-contract");
        assert_eq!(meta.source_path, "contract/market");
    }
}
