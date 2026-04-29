use async_trait::async_trait;
use near_api::types::transaction::actions::{Action, DeleteAccountAction};
use templar_gateway_types::{account, MethodSpec};

use super::Dispatch;
use crate::{
    operation::{OperationPlan, PlannedTransaction},
    DispatchRead, GatewayResult, HasNearClient, PlanWrite,
};

#[async_trait]
impl<C: HasNearClient> DispatchRead<account::Get, C> for Dispatch {
    async fn dispatch(
        request: <account::Get as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<account::GetResult> {
        let account = ctx
            .near_client()
            .account()
            .get(request.params.account_id)
            .await?;

        let (code_hash, global_contract_hash, global_contract_account_id) =
            match account.contract_state {
                near_api::types::account::ContractState::LocalHash(hash) => {
                    (hash.to_string(), None, None)
                }
                near_api::types::account::ContractState::GlobalHash(hash) => (
                    near_api::types::CryptoHash::default().to_string(),
                    Some(hash.to_string()),
                    None,
                ),
                near_api::types::account::ContractState::GlobalAccountId(account_id) => (
                    near_api::types::CryptoHash::default().to_string(),
                    None,
                    Some(account_id),
                ),
                near_api::types::account::ContractState::None => (
                    near_api::types::CryptoHash::default().to_string(),
                    None,
                    None,
                ),
            };

        Ok(account::GetResult {
            amount: account.amount,
            locked: account.locked,
            code_hash,
            storage_usage: account.storage_usage,
            global_contract_hash,
            global_contract_account_id,
        })
    }
}

#[async_trait]
impl<C: Send + 'static> PlanWrite<account::Delete, C> for Dispatch {
    async fn plan(
        request: <account::Delete as MethodSpec>::Input,
        _context: C,
    ) -> GatewayResult<OperationPlan> {
        Ok(OperationPlan::single(PlannedTransaction {
            signer_account_id: request.signer_account_id.clone(),
            wait_until: templar_gateway_types::common::TxExecutionStatus::ExecutedOptimistic,
            receiver_id: request.signer_account_id.0,
            actions: vec![Action::DeleteAccount(DeleteAccountAction {
                beneficiary_id: request.body.beneficiary_id,
            })],
        }))
    }
}
