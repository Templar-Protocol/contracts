use async_trait::async_trait;
use near_api::types::transaction::actions::{Action, DeleteAccountAction};
use templar_gateway_core::{DispatchRead, GatewayResult, HasNearClient, OperationPlan, PlanWrite};
use templar_gateway_methods_spec::account;

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<account::Get, C> for Dispatch {
    async fn dispatch(request: account::Get, ctx: C) -> GatewayResult<account::GetResult> {
        let account = ctx.near_client().account().get(request.account_id).await?;

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
impl<C: HasNearClient> DispatchRead<account::GetAccessKey, C> for Dispatch {
    async fn dispatch(
        request: account::GetAccessKey,
        ctx: C,
    ) -> GatewayResult<account::GetAccessKeyResult> {
        use near_api::types::transaction::actions::AccessKeyPermission as NearPermission;

        let key = ctx
            .near_client()
            .account()
            .access_key(request.account_id, request.public_key.into())
            .await?;

        let permission = match key.permission {
            NearPermission::FullAccess => account::AccessKeyPermission::FullAccess,
            NearPermission::FunctionCall(function_call) => {
                account::AccessKeyPermission::FunctionCall {
                    allowance: function_call.allowance,
                    receiver_id: function_call.receiver_id,
                    method_names: function_call.method_names,
                }
            }
        };

        Ok(account::GetAccessKeyResult {
            nonce: key.nonce.0,
            permission,
        })
    }
}

#[async_trait]
impl<C: Send + 'static> PlanWrite<account::Delete, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<account::Delete>,
        _context: C,
    ) -> GatewayResult<OperationPlan> {
        Ok(OperationPlan::execute(
            request.signer_account_id.clone(),
            request.signer_account_id.0,
            vec![Action::DeleteAccount(DeleteAccountAction {
                beneficiary_id: request.body.beneficiary_id,
            })],
        ))
    }
}
