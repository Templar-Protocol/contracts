use async_trait::async_trait;
use templar_gateway_types::{registry::DeployBody, universal_account, MethodSpec};
use templar_universal_account::InitArgs;

use super::Dispatch;
use crate::{
    client::{
        universal_account::{UaExecuteArgs, UaGetKeyArgs},
        ContractWriteOptions,
    },
    dispatch::registry::plan_deploy_from_registry,
    operation::OperationPlan,
    DispatchRead, GatewayResult, HasNearClient, PlanWrite,
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
        verifying_contract: parameters.verifying_contract,
        salt: parameters
            .salt
            .and_then(|value| serde_json::to_value(value).ok())
            .and_then(|value| value.as_str().map(str::to_owned)),
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<universal_account::GetKey, C> for Dispatch {
    async fn dispatch(
        params: <universal_account::GetKey as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<universal_account::GetKeyResult> {
        ctx.near_client()
            .universal_account(params.params.account_id.clone())
            .get_key(UaGetKeyArgs {
                key: params.params.key,
            })
            .await
            .map(|parameters| universal_account::GetKeyResult {
                parameters: parameters.map(into_parameters_view),
            })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<universal_account::Execute, C> for Dispatch {
    async fn plan(
        request: <universal_account::Execute as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        ctx.near_client()
            .universal_account(request.body.account_id)
            .execute(
                ContractWriteOptions::new(request.signer_account_id).tgas(300),
                UaExecuteArgs {
                    args: request.body.args,
                },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<universal_account::Create, C> for Dispatch {
    async fn plan(
        request: <universal_account::Create as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        plan_deploy_from_registry(
            &ctx,
            request.signer_account_id,
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
    }
}
