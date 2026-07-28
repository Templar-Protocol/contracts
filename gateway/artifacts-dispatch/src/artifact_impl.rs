use async_trait::async_trait;
use templar_contract_artifacts::{fetch, version_key_from_digest, ArtifactId};
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

struct LoadedArtifact {
    metadata: &'static templar_contract_artifacts::ArtifactMetadata,
    code: Vec<u8>,
    sha256: String,
    version_key: String,
}

/// Resolve an artifact's canonical bytes: the newest released version,
/// downloaded from its GitHub Release and verified against the catalog's
/// SHA-256 pin.
async fn load_artifact(artifact: ArtifactId) -> GatewayResult<LoadedArtifact> {
    let metadata = artifact.metadata();
    let Some(release) = metadata.current() else {
        return Err(GatewayError::RequestPreconditionFailed(format!(
            "{artifact} has never been released, so it has no canonical bytes. \
             Mock contracts are test scaffolding and are never released."
        )));
    };

    let code = fetch::released_bytes(artifact, release.version)
        .await
        .map_err(|error| match error {
            // "Not a release" is a property of the request, not a failure of
            // ours; a download failure is the opposite.
            unreleased @ fetch::FetchError::UnknownRelease { .. } => {
                GatewayError::RequestPreconditionFailed(unreleased.to_string())
            }
            other => GatewayError::ExternalService(other.to_string()),
        })?;

    // `released_bytes` guarantees the digest equals the pin, so re-hashing
    // 400 KB per request would recompute a value we already have.
    let sha256 = release.sha256.to_owned();
    let version_key = version_key_from_digest(metadata.package_name, release.version, &sha256);

    Ok(LoadedArtifact {
        metadata,
        code,
        sha256,
        version_key,
    })
}

#[async_trait]
impl<C> DispatchRead<GetArtifact, C> for Dispatch
where
    C: Send + 'static,
{
    async fn dispatch(request: GetArtifact, _ctx: C) -> GatewayResult<GetArtifactResult> {
        let artifact = load_artifact(request.artifact).await?;

        Ok(GetArtifactResult {
            metadata: ArtifactMetadata::from(artifact.metadata),
            code: templar_gateway_types::Base64Bytes(artifact.code),
            sha256: artifact.sha256,
            version_key: artifact.version_key,
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
            artifacts: ArtifactId::ALL
                .iter()
                .map(|id| ArtifactMetadata::from(id.metadata()))
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
        let artifact = load_artifact(body.artifact).await?;

        let registry_version = ctx
            .near_client()
            .contract(body.registry_id.clone())
            .cached_version()
            .await?;

        ctx.near_client()
            .registry(body.registry_id)
            .add_version(
                ContractWriteOptions::new(request.signer_account_id)
                    .tgas(300)
                    .deposit(body.deposit),
                registry_version,
                AddVersionArgs {
                    version_key: artifact.version_key,
                    mode: body.deploy_mode,
                    code: artifact.code,
                },
            )
            .map(OperationPlan::from)
    }
}
