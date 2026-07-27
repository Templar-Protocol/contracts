mod artifact_impl;

pub struct Dispatch;

#[cfg(test)]
mod tests {
    use super::*;
    use templar_contract_artifacts::ArtifactId;
    use templar_gateway_artifacts_spec::artifact::{GetArtifact, ListArtifacts};
    use templar_gateway_core::{DispatchRead, GatewayError};

    /// Artifacts whose bytes this dispatcher can serve: the ones that have
    /// actually been released. Everything else is a precondition failure.
    fn servable() -> impl Iterator<Item = ArtifactId> {
        ArtifactId::ALL
            .into_iter()
            .filter(|artifact| artifact.metadata().current().is_some())
    }

    /// Serving bytes means downloading them, so these tests need a warm
    /// artifact cache (`just artifacts-fetch`, which CI runs before the suite)
    /// or network access.
    #[tokio::test]
    async fn test_dispatch_get_artifact_returns_wasm_bytes() {
        // Given: a GetArtifact request for the market artifact
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
        assert_eq!(result.metadata.package_name, meta.package_name);
        assert_eq!(result.metadata.version.as_deref(), meta.version());
    }

    #[tokio::test]
    async fn test_dispatch_list_artifacts_returns_metadata_only_catalog() {
        let result = Dispatch::dispatch(ListArtifacts {}, ()).await.unwrap();

        // Listing is metadata-only, so it covers unreleased artifacts too —
        // they simply carry no version.
        assert_eq!(result.artifacts.len(), ArtifactId::ALL.len());
        let market = result
            .artifacts
            .iter()
            .find(|metadata| metadata.artifact == ArtifactId::Market)
            .unwrap();
        let market_catalog = ArtifactId::Market.metadata();
        assert_eq!(market.package_name, "templar-market-contract");
        assert_eq!(market.version.as_deref(), market_catalog.version());

        let mock = result
            .artifacts
            .iter()
            .find(|metadata| metadata.artifact == ArtifactId::MockFt)
            .unwrap();
        assert_eq!(mock.version, None, "mocks are never released");

        let json = serde_json::to_value(&result).unwrap();
        let first_artifact = json["artifacts"].as_array().unwrap().first().unwrap();
        assert!(first_artifact.get("code").is_none());
    }

    #[tokio::test]
    async fn test_dispatch_get_artifact_sha_matches() {
        // Given: a GetArtifact request for a released artifact (not the vault —
        // no NEAR vault has shipped, so it has no bytes to serve)
        let request = GetArtifact {
            artifact: ArtifactId::Registry,
        };
        // When: dispatched
        let result = Dispatch::dispatch(request, ()).await.unwrap();
        // Then: SHA-256 hash matches the bytes
        let computed = templar_contract_artifacts::sha256_hex(&result.code.0);
        assert_eq!(result.sha256, computed);
        // Then: version key has the canonical format
        let version = result.metadata.version.as_deref().unwrap();
        let prefix = format!("{}@{version}#", result.metadata.package_name);
        assert!(result.version_key.starts_with(&prefix));
        assert_eq!(result.version_key.len(), prefix.len() + 64);
    }

    #[tokio::test]
    async fn test_dispatch_get_artifact_all_released_artifacts_have_bytes() {
        for artifact in servable() {
            let meta = artifact.metadata();
            let result = Dispatch::dispatch(GetArtifact { artifact }, ())
                .await
                .unwrap();
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
        for artifact in servable() {
            let meta = artifact.metadata();
            let result = Dispatch::dispatch(GetArtifact { artifact }, ())
                .await
                .unwrap();

            assert_eq!(result.metadata.version.as_deref(), meta.version());
            assert!(result.version_key.starts_with(&format!(
                "{}@{}#",
                meta.package_name,
                meta.version().unwrap()
            )));
        }
    }

    /// A mock has no released version, so there are no canonical bytes to
    /// serve. This is a precondition failure, not an upstream fetch failure —
    /// and it needs no network to establish.
    #[tokio::test]
    async fn test_dispatch_get_artifact_rejects_unreleased_mock() {
        let error = Dispatch::dispatch(
            GetArtifact {
                artifact: ArtifactId::MockFt,
            },
            (),
        )
        .await
        .expect_err("mocks are never released");

        assert!(
            matches!(error, GatewayError::RequestPreconditionFailed(ref message)
                if message.contains("never been released")),
            "{error}"
        );
    }

    /// The NEAR vault has never been deployed, so it has no canonical bytes —
    /// same precondition failure as a mock, for a different reason.
    #[tokio::test]
    async fn test_dispatch_get_artifact_rejects_never_released_contract() {
        let error = Dispatch::dispatch(
            GetArtifact {
                artifact: ArtifactId::Vault,
            },
            (),
        )
        .await
        .expect_err("no NEAR vault has shipped");

        assert!(
            matches!(error, GatewayError::RequestPreconditionFailed(_)),
            "{error}"
        );
    }
}
