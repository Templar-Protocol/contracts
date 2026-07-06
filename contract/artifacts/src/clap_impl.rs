use crate::{artifact_catalog, ArtifactId};

pub fn artifact_value_parser(s: &str) -> Result<ArtifactId, String> {
    if let Ok(id) = s.parse::<ArtifactId>() {
        return Ok(id);
    }
    let valid_names: Vec<_> = artifact_catalog()
        .iter()
        .map(|artifact| format!("  {} ({})", artifact.id, artifact.package_name))
        .collect();
    Err(format!(
        "Unknown artifact '{s}'. Valid values:\n{}",
        valid_names.join("\n")
    ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_by_artifact_id() {
        assert_eq!(artifact_value_parser("market").unwrap(), ArtifactId::Market);
        assert_eq!(artifact_value_parser("MARKET").unwrap(), ArtifactId::Market);
    }

    #[test]
    fn test_parse_by_package_name() {
        assert_eq!(
            artifact_value_parser("templar-market-contract").unwrap(),
            ArtifactId::Market
        );
    }

    #[test]
    fn test_parse_unknown() {
        assert!(artifact_value_parser("no-such-contract").is_err());
    }

    #[test]
    fn test_artifact_ids_unique() {
        let names = artifact_catalog()
            .iter()
            .map(|artifact| artifact.id.as_str())
            .collect::<Vec<_>>();
        let mut deduped = names.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(names.len(), deduped.len(), "artifact IDs must be unique");
    }
}
