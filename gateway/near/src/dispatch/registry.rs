use blockchain_gateway_core::registry;
use futures::future::BoxFuture;

use crate::{
    actor::{DispatchRead, PlanWrite},
    client::{
        registry::{AddVersionArgs, GetDeploymentArgs, RemoveVersionArgs},
        ContractWriteOptions,
    },
    dispatch::contract::query_contract_kind,
    dispatch::single_transaction_plan,
    operation::OperationPlan,
    GatewayContext, GatewayResult,
};

impl DispatchRead for registry::ListDeployments {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.registry(request.params.registry_id)
                .list_deployments(request.params.args)
                .await
                .map(|account_ids| registry::ListDeploymentsResult { account_ids })
        })
    }
}

impl DispatchRead for registry::GetDeployment {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.registry(request.params.registry_id)
                .get_deployment(GetDeploymentArgs {
                    account_id: request.params.account_id,
                })
                .await
                .map(|deployment| registry::GetDeploymentResult { deployment })
        })
    }
}

impl DispatchRead for registry::ListVersions {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.registry(request.params.registry_id)
                .list_versions(request.params.args)
                .await
                .map(|values| registry::ListVersionsResult { values })
        })
    }
}

impl DispatchRead for registry::ListDeploymentsByKind {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let account_ids = ctx
                .registry(params.registry_id)
                .list_deployments(blockchain_gateway_core::common::Pagination::default())
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
        })
    }
}

impl PlanWrite for registry::AddVersion {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            let body = request.body;
            let registry_version = ctx.contract(body.registry_id.0.clone()).version().await?;
            Ok(single_transaction_plan(
                ctx.registry(body.registry_id).add_version(
                    ContractWriteOptions::new(request.signer_account_id)
                        .tgas(300)
                        .deposit(body.deposit),
                    registry_version,
                    AddVersionArgs {
                        version_key: body.version_key,
                        mode: body.deploy_mode,
                        code: body.code.0,
                    },
                )?,
            ))
        })
    }
}

impl PlanWrite for registry::Deploy {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            plan_deploy_from_registry(&ctx, request.signer_account_id, request.body).await
        })
    }
}

pub(crate) async fn plan_deploy_from_registry(
    ctx: &GatewayContext,
    signer_account_id: blockchain_gateway_core::ManagedAccountId,
    body: registry::DeployBody,
) -> GatewayResult<OperationPlan> {
    let deposit = body.deposit;
    let registry_version = ctx.contract(body.registry_id.0.clone()).version().await?;
    Ok(single_transaction_plan(
        ctx.registry(body.registry_id).deploy(
            ContractWriteOptions::new(signer_account_id)
                .tgas(300)
                .deposit(deposit),
            registry_version,
            crate::client::registry::DeployArgs {
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

impl PlanWrite for registry::RemoveVersion {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            let body = request.body;
            Ok(single_transaction_plan(
                ctx.registry(body.registry_id).remove_version(
                    ContractWriteOptions::new(request.signer_account_id)
                        .tgas(300)
                        .one_yocto(),
                    RemoveVersionArgs {
                        version_key: body.version_key,
                    },
                )?,
            ))
        })
    }
}
