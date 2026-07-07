mod artifact_impl;

pub struct Dispatch;

#[cfg(test)]
mod tests {
    use super::*;
    use templar_contract_artifacts::ArtifactId;
    use templar_gateway_artifacts_spec::artifact::{GetArtifact, ListArtifacts};
    use templar_gateway_core::DispatchRead;

    #[tokio::test]
    async fn test_dispatch_get_artifact_returns_wasm_bytes() {
        // Given: a GetArtifact request for the registry artifact
        let request = GetArtifact {
            artifact: ArtifactId::Market,
        };
        // When: dispatched
        let result = Dispatch::dispatch(request, ()).await.unwrap();
        // Then: code is non-empty WASM bytes
        assert!(!result.code.0.is_empty());
        // Then: starts with WASM magic bytes
        assert_eq!(&result.code.0[0..4], b"\0asm");
        // Then: metadata matches the catalog
        let meta = ArtifactId::Market.metadata();
        assert_eq!(result.package_name, meta.package_name);
        assert_eq!(result.version, meta.version);
    }

    #[tokio::test]
    async fn test_dispatch_list_artifacts_returns_metadata_only_catalog() {
        let result = Dispatch::dispatch(ListArtifacts {}, ()).await.unwrap();

        assert_eq!(result.artifacts.len(), ArtifactId::ALL.len());
        let market = result
            .artifacts
            .iter()
            .find(|metadata| metadata.artifact == ArtifactId::Market)
            .unwrap();
        let market_catalog = ArtifactId::Market.metadata();
        assert_eq!(market.package_name, "templar-market-contract");
        assert_eq!(market.version, market_catalog.version);

        let json = serde_json::to_value(&result).unwrap();
        let first_artifact = json["artifacts"].as_array().unwrap().first().unwrap();
        assert!(first_artifact.get("code").is_none());
    }

    #[tokio::test]
    async fn test_dispatch_get_artifact_sha_matches() {
        // Given: a GetArtifact request for the vault artifact
        let request = GetArtifact {
            artifact: ArtifactId::Vault,
        };
        // When: dispatched
        let result = Dispatch::dispatch(request, ()).await.unwrap();
        // Then: SHA-256 hash matches the bytes
        let computed = templar_contract_artifacts::sha256_hex(&result.code.0);
        assert_eq!(result.sha256, computed);
        // Then: version key has the canonical format
        assert!(result
            .version_key
            .starts_with(&format!("{}@{}#", result.package_name, result.version)));
        assert_eq!(
            result.version_key.len(),
            format!("{}@{}#", result.package_name, result.version).len() + 64
        );
    }

    #[tokio::test]
    async fn test_dispatch_get_artifact_all_artifacts_have_bytes() {
        // Given: every catalogued artifact
        for artifact in ArtifactId::ALL {
            let meta = artifact.metadata();
            let request = GetArtifact { artifact };
            // When: dispatched
            let result = Dispatch::dispatch(request, ()).await.unwrap();
            // Then: code is non-empty and starts with WASM magic
            assert!(
                !result.code.0.is_empty(),
                "{} has empty WASM",
                meta.package_name
            );
            assert_eq!(
                &result.code.0[0..4],
                b"\0asm",
                "{} does not start with WASM magic",
                meta.package_name
            );
        }
    }

    #[tokio::test]
    async fn test_version_key_uses_catalog_version() {
        for artifact in ArtifactId::ALL {
            let meta = artifact.metadata();
            let result = Dispatch::dispatch(GetArtifact { artifact }, ())
                .await
                .unwrap();

            assert_eq!(result.version, meta.version);
            assert!(result
                .version_key
                .starts_with(&format!("{}@{}#", meta.package_name, meta.version)));
        }
    }
}
