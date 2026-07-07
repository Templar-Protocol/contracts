use templar_gateway_artifacts_spec::artifact;
use templar_gateway_types::common::WriteRequest;

use super::*;

#[tokio::test]
async fn artifact_list_endpoint_returns_catalog_metadata_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;

    let result = stack
        .controller
        .request::<artifact::ListArtifacts>(&artifact::ListArtifacts {})
        .await?;

    assert_eq!(
        result.artifacts.len(),
        templar_contract_artifacts::ArtifactId::ALL.len()
    );
    assert!(result.artifacts.iter().any(|metadata| metadata.artifact
        == templar_contract_artifacts::ArtifactId::Market
        && metadata.package_name == "templar-market-contract"));

    let json = serde_json::to_value(&result)?;
    let first_artifact = json["artifacts"].as_array().unwrap().first().unwrap();
    assert!(first_artifact.get("code").is_none());

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn artifact_get_endpoint_works_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;

    let result = stack
        .controller
        .request::<artifact::GetArtifact>(&artifact::GetArtifact {
            artifact: templar_contract_artifacts::ArtifactId::Market,
        })
        .await?;

    // Check metadata is consistent.
    assert_eq!(
        result.artifact,
        templar_contract_artifacts::ArtifactId::Market
    );
    assert_eq!(result.package_name, "templar-market-contract");
    assert!(!result.cargo_target_name.is_empty());
    assert!(!result.source_path.is_empty());
    assert!(!result.version.is_empty());

    // Check WASM bytes are valid.
    assert!(!result.code.0.is_empty());
    assert_eq!(&result.code.0[0..4], b"\0asm");

    // Check SHA-256 and version key are present.
    assert!(!result.sha256.is_empty());
    assert_eq!(result.sha256.len(), 64);
    assert!(result
        .version_key
        .starts_with(&format!("{}@{}#", result.package_name, result.version)));
    assert_eq!(
        result.version_key.len(),
        format!("{}@{}#", result.package_name, result.version).len() + 64
    );

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn artifact_add_endpoint_works_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let registry_id = stack.harness.deploy_registry().await?;
    let mock_ft = templar_contract_artifacts::ArtifactId::MockFt.metadata();
    let expected_version_prefix = format!("{}@{}#", mock_ft.package_name, mock_ft.version);

    let write_result = stack
        .controller
        .request::<artifact::AddArtifactVersion>(&WriteRequest {
            signer_account_id: stack.harness.registry_signer_account_id.clone(),
            idempotency_key: None,
            body: artifact::AddArtifactVersion {
                registry_id: registry_id.clone(),
                artifact: templar_contract_artifacts::ArtifactId::MockFt,
                deploy_mode: templar_common::registry::DeployMode::Normal,
                deposit: NearToken::from_yoctonear(1),
            },
        })
        .await?;

    assert_eq!(
        write_result.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );

    // List versions to confirm it was added.
    let versions = stack
        .controller
        .request::<registry::ListVersions>(&registry::ListVersions {
            registry_id: registry_id.clone(),
            args: templar_gateway_types::common::Pagination::default(),
        })
        .await?;
    assert!(versions
        .values
        .iter()
        .any(|version_key| version_key.starts_with(&expected_version_prefix)));

    stack.shutdown().await;
    Ok(())
}
