use async_trait::async_trait;
use templar_gateway_core::{
    client::registry::{AddVersionArgs, DeployArgs, GetDeploymentArgs, RemoveVersionArgs},
    query_contract_kind, ContractWriteOptions, DispatchRead, GatewayResult, HasNearClient,
    OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::registry;

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
        let params = request;
        let account_ids = ctx
            .near_client()
            .registry(params.registry_id)
            .list_deployments(templar_gateway_types::common::Pagination::default())
            .await?;

        let mut filtered = Vec::new();
        for account_id in account_ids {
            if query_contract_kind(&ctx, account_id.clone()).await? == params.kind {
                filtered.push(account_id);
            }
        }

        let offset = params.args.offset.unwrap_or_default() as usize;
        let limit = params.args.limit.map(|value| value as usize);
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
                    version_key: body.version_key,
                    mode: body.deploy_mode,
                    code: body.code.0,
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
        plan_deploy_from_registry(&ctx, request.signer_account_id, request.body).await
    }
}

pub async fn plan_deploy_from_registry<C: HasNearClient>(
    ctx: &C,
    signer_account_id: templar_gateway_types::ManagedAccountId,
    body: registry::Deploy,
) -> GatewayResult<OperationPlan> {
    let deposit = body.deposit;
    let registry_version = ctx
        .near_client()
        .contract(body.registry_id.clone())
        .version()
        .await?;
    Ok(OperationPlan::single(
        ctx.near_client().registry(body.registry_id).deploy(
            ContractWriteOptions::new(signer_account_id)
                .tgas(300)
                .deposit(deposit),
            registry_version,
            DeployArgs {
                name: body.name,
                version_key: body.version_key,
                init_args: body.init_args,
                full_access_keys: body
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
