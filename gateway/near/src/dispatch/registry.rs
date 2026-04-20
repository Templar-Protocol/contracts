use std::sync::Arc;

use blockchain_gateway_core::registry;
use futures::future::BoxFuture;

use crate::{
    actor::{operation_outcome_from_transaction_result, DispatchRead, DispatchWrite},
    client::{
        registry::{GetDeploymentArgs, RemoveVersionArgs},
        ContractWriteOptions,
    },
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

impl DispatchWrite for registry::AddVersion {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let deposit = body.deposit;
            let registry_version = ctx.contract(body.registry_id.0.clone()).version().await?;
            let tx_result = ctx
                .registry(body.registry_id.clone())
                .add_version(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .tgas(300)
                        .deposit(deposit),
                    registry_version,
                    crate::client::registry::AddVersionArgs {
                        version_key: body.version_key,
                        mode: body.deploy_mode,
                        code: body.code.0,
                    },
                )
                .await?;

            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for registry::Deploy {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            deploy_from_registry(
                ctx,
                signer,
                request.signer_account_id,
                request.wait_until,
                request.body,
            )
            .await
        })
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

pub(crate) async fn deploy_from_registry(
    ctx: GatewayContext,
    signer: Arc<near_api::Signer>,
    signer_account_id: blockchain_gateway_core::ManagedAccountId,
    wait_until: blockchain_gateway_core::common::TxExecutionStatus,
    body: registry::DeployBody,
) -> GatewayResult<blockchain_gateway_core::common::WriteOperationResult> {
    let signer_account_id_for_result = signer_account_id.clone();
    let deposit = body.deposit;
    let registry_version = ctx.contract(body.registry_id.0.clone()).version().await?;
    let tx_result = ctx
        .registry(body.registry_id.clone())
        .deploy(
            ContractWriteOptions::new(signer_account_id, signer)
                .wait_until(wait_until)
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
        )
        .await?;

    Ok(operation_outcome_from_transaction_result(
        signer_account_id_for_result,
        tx_result,
    ))
}

impl DispatchWrite for registry::RemoveVersion {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let tx_result = ctx
                .registry(body.registry_id.clone())
                .remove_version(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .tgas(300)
                        .one_yocto(),
                    RemoveVersionArgs {
                        version_key: body.version_key,
                    },
                )
                .await?;

            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}
