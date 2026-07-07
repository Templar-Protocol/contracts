use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::registry::DeployMode;
use templar_contract_artifacts::ArtifactId;
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::{Base64Bytes, NearToken};

/// Get contract artifact bytes and metadata.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "artifact.get", output = GetArtifactResult)]
pub struct GetArtifact {
    pub artifact: ArtifactId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetArtifactResult {
    pub artifact: ArtifactId,
    pub package_name: String,
    pub cargo_target_name: String,
    pub source_path: String,
    pub version: String,
    pub code: Base64Bytes,
    pub sha256: String,
    pub version_key: String,
}

/// List known contract artifacts and metadata, excluding code bytes.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "artifact.list", output = ListArtifactsResult)]
pub struct ListArtifacts {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListArtifactsResult {
    pub artifacts: Vec<ArtifactMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactMetadata {
    pub artifact: ArtifactId,
    pub package_name: String,
    pub cargo_target_name: String,
    pub source_path: String,
    pub version: String,
}

impl From<&templar_contract_artifacts::ArtifactMetadata> for ArtifactMetadata {
    fn from(metadata: &templar_contract_artifacts::ArtifactMetadata) -> Self {
        Self {
            artifact: metadata.id,
            package_name: metadata.package_name.to_owned(),
            cargo_target_name: metadata.cargo_target_name.to_owned(),
            source_path: metadata.source_path.to_owned(),
            version: metadata.version.to_owned(),
        }
    }
}

/// Add a contract artifact version to a registry.
///
/// Resolves the artifact's embedded WASM bytes from the contract-artifacts
/// catalog and computes the version key from the artifact's package name,
/// version, and SHA-256 hash.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "registry.addArtifactVersion")]
pub struct AddArtifactVersion {
    pub registry_id: AccountId,
    pub artifact: ArtifactId,
    pub deploy_mode: DeployMode,
    pub deposit: NearToken,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_artifact_kebab_case_input() {
        // Given: JSON with kebab-case artifact field
        let json = r#"{"artifact":"market"}"#;
        // When: deserialized
        let req: GetArtifact = serde_json::from_str(json).unwrap();
        // Then: artifact is correctly parsed
        assert_eq!(req.artifact, ArtifactId::Market);
    }

    #[test]
    fn test_get_artifact_all_variants_roundtrip() {
        for artifact in ArtifactId::ALL {
            let req = GetArtifact { artifact };
            let json = serde_json::to_string(&req).unwrap();
            let parsed: GetArtifact = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.artifact, artifact);
        }
    }

    #[test]
    fn test_list_artifacts_result_has_metadata_without_code() {
        // Given: a metadata-only list result
        let result = ListArtifactsResult {
            artifacts: vec![ArtifactMetadata::from(ArtifactId::Market.metadata())],
        };
        // When: serialized to JSON
        let json = serde_json::to_value(&result).unwrap();
        let artifact = json["artifacts"].as_array().unwrap().first().unwrap();
        // Then: metadata is present and WASM bytes are absent
        assert_eq!(artifact["artifact"], "market");
        assert!(artifact.get("code").is_none());
    }

    #[test]
    fn test_add_artifact_version_serde() {
        // Given: a full AddArtifactVersion spec
        let spec = AddArtifactVersion {
            registry_id: "registry.near".parse().unwrap(),
            artifact: ArtifactId::Market,
            deploy_mode: DeployMode::Normal,
            deposit: NearToken::from_yoctonear(1),
        };
        // When: round-tripped through JSON
        let json = serde_json::to_string(&spec).unwrap();
        let parsed: AddArtifactVersion = serde_json::from_str(&json).unwrap();
        // Then: fields match
        assert_eq!(parsed.registry_id, spec.registry_id);
        assert_eq!(parsed.artifact, spec.artifact);
        assert_eq!(parsed.deploy_mode, spec.deploy_mode);
        assert_eq!(parsed.deposit, spec.deposit);
    }
}
