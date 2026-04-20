mod account;
mod contract;
mod ft;
mod market;
mod oracle;
mod proxy_oracle;
mod proxy_oracle_governance;
mod proxy_oracle_owner;
mod registry;
mod storage;
mod tx;
mod universal_account;

use blockchain_gateway_core::{
    common::ContractArgs, ContractMethodName, ManagedAccountId, NearGas, NearToken,
};

use crate::{
    operation::{OperationPlan, PlannedTransaction},
    GatewayError, GatewayResult,
};

pub(crate) fn single_transaction_plan(
    wait_until: blockchain_gateway_core::rpc::common::TxExecutionStatus,
    transaction: PlannedTransaction,
) -> OperationPlan {
    OperationPlan {
        wait_until,
        steps: vec![transaction],
    }
}

pub(crate) fn function_call_transaction_json<T: serde::Serialize>(
    signer_account_id: ManagedAccountId,
    receiver_id: near_account_id::AccountId,
    method_name: &str,
    args: T,
    gas: NearGas,
    deposit: NearToken,
) -> GatewayResult<PlannedTransaction> {
    function_call_transaction(
        signer_account_id,
        receiver_id,
        method_name,
        ContractArgs::Json(serde_json::to_value(&args)?),
        gas,
        deposit,
    )
}

pub(crate) fn function_call_transaction(
    signer_account_id: ManagedAccountId,
    receiver_id: near_account_id::AccountId,
    method_name: &str,
    args: ContractArgs,
    gas: NearGas,
    deposit: NearToken,
) -> GatewayResult<PlannedTransaction> {
    Ok(PlannedTransaction {
        signer_account_id,
        receiver_id,
        actions: vec![near_api::types::transaction::actions::Action::FunctionCall(
            Box::new(near_api::types::transaction::actions::FunctionCallAction {
                method_name: ContractMethodName(method_name.to_owned()).0,
                args: args.try_into_bytes()?,
                gas,
                deposit,
            }),
        )],
    })
}

pub(crate) async fn execute_planned_transaction(
    ctx: &crate::GatewayContext,
    signer: std::sync::Arc<near_api::Signer>,
    transaction: PlannedTransaction,
    wait_until: blockchain_gateway_core::rpc::common::TxExecutionStatus,
) -> GatewayResult<near_api::types::transaction::result::TransactionResult> {
    near_api::Transaction::use_transaction(
        near_api::types::transaction::PrepopulateTransaction {
            signer_id: transaction.signer_account_id.0,
            receiver_id: transaction.receiver_id,
            actions: transaction.actions,
        },
        signer,
    )
    .wait_until(wait_until.into())
    .send_to(ctx.network())
    .await
    .map_err(|error| GatewayError::NearTransaction(error.to_string()))
}
