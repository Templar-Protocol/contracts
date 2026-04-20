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
    GatewayResult,
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
