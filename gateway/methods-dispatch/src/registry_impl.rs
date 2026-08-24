use async_trait::async_trait;
use templar_gateway_core::{
    client::registry::{
        AddVersionArgs, DeployArgs, GetDeploymentArgs, GetRegistryEntryArgs, GetVersionArgs,
        RemoveVersionArgs,
    },
    query_contract_kind, ContractWriteOptions, DispatchRead, GatewayError, GatewayResult,
    HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::registry;
use templar_gateway_types::RegistryVersion;

use crate::Dispatch;

#[async_trait]
impl<C> DispatchRead<registry::ListDeployments, C> for Dispatch
where
    C: HasNearClient,
{
    async fn dispatch(
        request: registry::ListDeployments,
        ctx: C,
    ) -> GatewayResult<registry::ListDeploymentsResult> {
        ctx.near_client()
            .registry(request.registry_id)
            .list_deployments(request.args)
            .await
            .map(|account_ids| registry::ListDeploymentsResult { account_ids })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<registry::GetDeployment, C> for Dispatch {
    async fn dispatch(
        request: registry::GetDeployment,
        ctx: C,
    ) -> GatewayResult<registry::GetDeploymentResult> {
        ctx.near_client()
            .registry(request.registry_id)
            .get_deployment(GetDeploymentArgs {
                account_id: request.account_id,
            })
            .await
            .map(|deployment| registry::GetDeploymentResult { deployment })
    }
}

/// Refuse a registry too old to serve `get_registry_entry` / `get_version`, rather than answering
/// from the views it does have.
///
/// Both exist to report a state their predecessors collapse — a reserved name, a version whose
/// code was removed. Synthesising either from `get_deployment` or `list_versions` would return the
/// very answer they were added to correct, and return it indistinguishably from a real one. A
/// caller that can degrade should ask the version first, as `tmplrmgr`'s preflight does.
async fn require_entry_and_version_views<C: HasNearClient>(
    ctx: &C,
    registry_id: near_account_id::AccountId,
) -> GatewayResult<()> {
    let version: RegistryVersion = ctx
        .near_client()
        .contract(registry_id.clone())
        .cached_version()
        .await?;

    if !version.supports_entry_and_version_views() {
        return Err(GatewayError::UnsupportedFeature(format!(
            "registry {registry_id} is version {version}; \
             getRegistryEntry and getVersion require 2.0.0"
        )));
    }

    Ok(())
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<registry::GetRegistryEntry, C> for Dispatch {
    async fn dispatch(
        request: registry::GetRegistryEntry,
        ctx: C,
    ) -> GatewayResult<registry::GetRegistryEntryResult> {
        require_entry_and_version_views(&ctx, request.registry_id.clone()).await?;
        ctx.near_client()
            .registry(request.registry_id)
            .get_registry_entry(GetRegistryEntryArgs {
                account_id: request.account_id,
            })
            .await
            .map(|entry| registry::GetRegistryEntryResult { entry })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<registry::GetVersion, C> for Dispatch {
    async fn dispatch(
        request: registry::GetVersion,
        ctx: C,
    ) -> GatewayResult<registry::GetVersionResult> {
        require_entry_and_version_views(&ctx, request.registry_id.clone()).await?;
        ctx.near_client()
            .registry(request.registry_id)
            .get_version(GetVersionArgs {
                version_key: request.version_key,
            })
            .await
            .map(|version| registry::GetVersionResult { version })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<registry::ListVersions, C> for Dispatch {
    async fn dispatch(
        request: registry::ListVersions,
        ctx: C,
    ) -> GatewayResult<registry::ListVersionsResult> {
        ctx.near_client()
            .registry(request.registry_id)
            .list_versions(request.args)
            .await
            .map(|values| registry::ListVersionsResult { values })
    }
}

#[async_trait]
impl<C> DispatchRead<registry::ListDeploymentsByKind, C> for Dispatch
where
    C: HasNearClient,
{
    async fn dispatch(
        request: registry::ListDeploymentsByKind,
        ctx: C,
    ) -> GatewayResult<registry::ListDeploymentsResult> {
        let account_ids = ctx
            .near_client()
            .registry(request.registry_id)
            .list_deployments(templar_gateway_types::common::Pagination::default())
            .await?;

        let mut filtered = Vec::new();
        for account_id in account_ids {
            if query_contract_kind(&ctx, account_id.clone()).await? == request.kind {
                filtered.push(account_id);
            }
        }

        let offset = request.args.offset.unwrap_or_default() as usize;
        let limit = request.args.limit.map(|value| value as usize);
        let account_ids = if let Some(limit) = limit {
            filtered.into_iter().skip(offset).take(limit).collect()
        } else {
            filtered.into_iter().skip(offset).collect()
        };

        Ok(registry::ListDeploymentsResult { account_ids })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<registry::AddVersion, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<registry::AddVersion>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
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
                    version_key: body.version_key,
                    source: body.source,
                },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C> PlanWrite<registry::Deploy, C> for Dispatch
where
    C: HasNearClient,
{
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<registry::Deploy>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let registry::Deploy { target, init_args } = request.body;
        plan_create_from_registry(&ctx, request.signer_account_id, target, init_args.0).await
    }
}

/// Plan the deploy of a registered version to a new sub-account under the registry.
///
/// Encode `init_args` with `serde_json` unless the payload holds a `HashMap`, whose
/// order varies per process: JSON Canonicalization sorts keys but rounds every
/// integer through `f64`, silently altering any `u64` past 2^53.
pub(crate) async fn plan_create_from_registry<C: HasNearClient>(
    ctx: &C,
    signer_account_id: templar_gateway_types::ManagedAccountId,
    target: registry::DeployTarget,
    init_args: Vec<u8>,
) -> GatewayResult<OperationPlan> {
    let registry_version = ctx
        .near_client()
        .contract(target.registry_id.clone())
        .cached_version()
        .await?;
    Ok(OperationPlan::single(
        ctx.near_client().registry(target.registry_id).deploy(
            ContractWriteOptions::new(signer_account_id)
                .tgas(300)
                .deposit(target.deposit),
            registry_version,
            DeployArgs {
                name: target.name,
                version_key: target.version_key,
                init_args: init_args.into(),
                full_access_keys: target
                    .full_access_keys
                    .map(|keys| keys.into_iter().map(Into::into).collect()),
            },
        )?,
    ))
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<registry::RemoveVersion, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<registry::RemoveVersion>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .registry(body.registry_id)
            .remove_version(
                ContractWriteOptions::new(request.signer_account_id)
                    .tgas(300)
                    .one_yocto(),
                RemoveVersionArgs {
                    version_key: body.version_key,
                },
            )
            .map(OperationPlan::from)
    }
}
