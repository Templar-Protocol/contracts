use async_trait::async_trait;
use templar_contract_artifacts::{
    artifact_catalog, find_by_id, format_version_key, read_embedded_by_id, sha256_hex,
};
use templar_gateway_artifacts_spec::artifact::{
    AddArtifactVersion, ArtifactMetadata, GetArtifact, GetArtifactResult, ListArtifacts,
    ListArtifactsResult,
};
use templar_gateway_core::{
    client::registry::AddVersionArgs, ContractWriteOptions, DispatchRead, GatewayError,
    GatewayResult, HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_types::common::WriteRequest;

use crate::Dispatch;

#[async_trait]
impl<C> DispatchRead<GetArtifact, C> for Dispatch
where
    C: Send + 'static,
{
    async fn dispatch(request: GetArtifact, _ctx: C) -> GatewayResult<GetArtifactResult> {
        let metadata = find_by_id(request.artifact)
            .map_err(|e| GatewayError::NearQuery(format!("artifact lookup failed: {e}")))?;

        let code = read_embedded_by_id(request.artifact)
            .map(|bytes| bytes.to_vec())
            .map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("embedded WASM not found for {:?}: {e}", request.artifact),
                )
            })?;

        let sha256 = sha256_hex(&code);
        let version_key = format_version_key(metadata.package_name, metadata.version, &code);

        Ok(GetArtifactResult {
            artifact: request.artifact,
            package_name: metadata.package_name.to_string(),
            cargo_target_name: metadata.cargo_target_name.to_string(),
            source_path: metadata.source_path.to_string(),
            version: metadata.version.to_string(),
            code: templar_gateway_types::Base64Bytes(code),
            sha256,
            version_key,
        })
    }
}

#[async_trait]
impl<C> DispatchRead<ListArtifacts, C> for Dispatch
where
    C: Send + 'static,
{
    async fn dispatch(_request: ListArtifacts, _ctx: C) -> GatewayResult<ListArtifactsResult> {
        Ok(ListArtifactsResult {
            artifacts: artifact_catalog()
                .iter()
                .map(ArtifactMetadata::from)
                .collect(),
        })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<AddArtifactVersion, C> for Dispatch {
    async fn plan(
        request: WriteRequest<AddArtifactVersion>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;

        let metadata = find_by_id(body.artifact)
            .map_err(|e| GatewayError::NearQuery(format!("artifact lookup failed: {e}")))?;

        let code = read_embedded_by_id(body.artifact)
            .map(|bytes| bytes.to_vec())
            .map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("embedded WASM not found for {:?}: {e}", body.artifact),
                )
            })?;

        let version_key = format_version_key(metadata.package_name, metadata.version, &code);

        let registry_version = ctx
            .near_client()
            .contract(body.registry_id.clone())
            .version()
            .await?;

        ctx.near_client()
            .registry(body.registry_id)
            .add_version(
                ContractWriteOptions::new(request.signer_account_id)
                    .tgas(300)
                    .deposit(body.deposit),
                registry_version,
                AddVersionArgs {
                    version_key,
                    mode: body.deploy_mode,
                    code,
                },
            )
            .map(OperationPlan::from)
    }
}
