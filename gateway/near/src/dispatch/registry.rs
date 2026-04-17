use std::sync::Arc;

use blockchain_gateway_core::registry;
use futures::future::BoxFuture;

use crate::{
    actor::{operation_outcome_from_transaction_result, DispatchRead, DispatchWrite, RpcMessage},
    client::ContractWriteOptions,
    ops, GatewayResult, NearClient,
};

impl DispatchRead for registry::ListDeployments {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.params;
            client
                .registry(params.registry_id)
                .list_deployments(params.args)
                .await
                .map(|account_ids| registry::ListDeploymentsResult { account_ids })
        })
    }
}

impl DispatchRead for registry::GetDeployment {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.params;
            client
                .registry(params.registry_id)
                .get_deployment(crate::client::registry::GetDeploymentArgs {
                    account_id: params.account_id,
                })
                .await
                .map(|deployment| registry::GetDeploymentResult { deployment })
        })
    }
}

impl DispatchRead for registry::ListVersions {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.params;
            client
                .registry(params.registry_id)
                .list_versions(params.args)
                .await
                .map(|values| registry::ListVersionsResult { values })
        })
    }
}

impl DispatchWrite for registry::AddVersion {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let deposit = body.deposit;
            let registry_version = ops::contract::version::<blockchain_gateway_core::Registry>(
                &client,
                body.registry_id.0.clone(),
            )
            .await?;
            let tx_result = client
                .registry(body.registry_id.clone())
                .add_version(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(300))
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
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let deposit = body.deposit;
            let registry_version = ops::contract::version::<blockchain_gateway_core::Registry>(
                &client,
                body.registry_id.0.clone(),
            )
            .await?;
            let tx_result = client
                .registry(body.registry_id.clone())
                .deploy(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(300))
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
                signer_account_id,
                tx_result,
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for registry::RemoveVersion {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let tx_result = client
                .registry(body.registry_id.clone())
                .remove_version(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(300))
                        .deposit(blockchain_gateway_core::NearToken::from_yoctonear(1)),
                    crate::client::registry::RemoveVersionArgs {
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
