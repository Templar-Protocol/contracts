use std::sync::Arc;

use blockchain_gateway_core::{registry::DeployBody, universal_account};
use futures::future::BoxFuture;
use templar_universal_account::InitArgs;

use crate::{
    actor::{operation_outcome_from_transaction_result, DispatchRead, DispatchWrite},
    client::{
        universal_account::{UaExecuteArgs, UaGetKeyArgs},
        ContractWriteOptions,
    },
    dispatch::registry::deploy_from_registry,
    GatewayContext, GatewayResult,
};

fn into_parameters_view(
    parameters: templar_universal_account::PayloadExecutionParameters,
) -> universal_account::PayloadExecutionParametersView {
    universal_account::PayloadExecutionParametersView {
        block_height: parameters.block_height.0,
        index: parameters.index.0,
        nonce: parameters.nonce.0,
        name: parameters.name,
        version: parameters.version,
        chain_id: parameters.chain_id.map(|value| value.0),
        verifying_contract: parameters
            .verifying_contract
            .to_string()
            .parse()
            .expect("templar universal account should emit valid account ids"),
        salt: parameters
            .salt
            .and_then(|value| serde_json::to_value(value).ok())
            .and_then(|value| value.as_str().map(str::to_owned)),
    }
}

impl DispatchRead for universal_account::GetKey {
    fn dispatch(
        params: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.universal_account(params.params.account_id.clone())
                .get_key(UaGetKeyArgs {
                    key: params.params.key,
                })
                .await
                .map(|parameters| universal_account::GetKeyResult {
                    parameters: parameters.map(into_parameters_view),
                })
        })
    }
}

impl DispatchWrite for universal_account::Execute {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = ctx
                .universal_account(request.body.account_id)
                .execute(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(300)),
                    UaExecuteArgs {
                        args: request.body.args,
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

impl DispatchWrite for universal_account::Create {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let body = request.body;
            deploy_from_registry(
                ctx,
                signer,
                request.signer_account_id,
                request.wait_until,
                DeployBody {
                    registry_id: body.registry_id,
                    name: body.account_name,
                    version_key: body.version_key,
                    init_args: serde_json::to_vec(&InitArgs {
                        key: body.key,
                        chain_id: body.chain_id.0.into(),
                        execute: body.execute.map(|transactions| transactions.into_vec()),
                    })?
                    .into(),
                    full_access_keys: body.full_access_keys,
                    deposit: body.deposit,
                },
            )
            .await
        })
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}
