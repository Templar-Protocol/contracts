use blockchain_gateway_core::registry;
use futures::future::BoxFuture;

use crate::{
    client::{
        registry::{AddVersionArgs, DeployArgs, RemoveVersionArgs},
        ContractWriteOptions,
    },
    GatewayResult, NearClient,
};

use super::{operation_outcome_from_transaction_result, DispatchWrite};

impl DispatchWrite for registry::AddVersion {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: std::sync::Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let body = request.body;
            let contract_id = &body.registry_id.0;
            let version = client.contract(contract_id.clone()).version().await?;
            let tx_result = client
                .registry(body.registry_id.clone())
                .add_version(
                    ContractWriteOptions::new(request.signer_account_id.clone(), signer)
                        .wait_until(request.wait_until)
                        .tgas(300)
                        .deposit(body.deposit),
                    version,
                    AddVersionArgs {
                        version_key: body.version_key,
                        mode: body.deploy_mode,
                        code: body.code.0,
                    },
                )
                .await?;

            Ok(operation_outcome_from_transaction_result(
                request.signer_account_id,
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
        signer: std::sync::Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let body = request.body;
            let version = client
                .contract(body.registry_id.0.clone())
                .version()
                .await?;
            let tx_result = client
                .registry(body.registry_id.clone())
                .deploy(
                    ContractWriteOptions::new(request.signer_account_id.clone(), signer)
                        .wait_until(request.wait_until)
                        .tgas(300)
                        .deposit(body.deposit),
                    version,
                    DeployArgs {
                        name: body.name,
                        version_key: body.version_key,
                        init_args: body.init_args,
                        full_access_keys: body
                            .full_access_keys
                            .map(|k| k.into_iter().map(Into::into).collect()),
                    },
                )
                .await?;

            Ok(operation_outcome_from_transaction_result(
                request.signer_account_id.clone(),
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
        signer: std::sync::Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let tx_result = client
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
