use blockchain_gateway_core::account;
use futures::future::BoxFuture;
use near_api::types::transaction::actions::{Action, DeleteAccountAction};

use crate::{
    actor::{DispatchRead, PlanWrite},
    operation::{OperationPlan, PlannedTransaction},
    GatewayContext, GatewayResult,
};

impl DispatchRead for account::Get {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let account = ctx.account().get(request.params.account_id).await?;

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
        })
    }
}

impl PlanWrite for account::Delete {
    fn plan(
        request: Self::Input,
        _context: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            Ok(OperationPlan {
                steps: vec![PlannedTransaction {
                    signer_account_id: request.signer_account_id.clone(),
                    wait_until:
                        blockchain_gateway_core::common::TxExecutionStatus::ExecutedOptimistic,
                    receiver_id: request.signer_account_id.0,
                    actions: vec![Action::DeleteAccount(DeleteAccountAction {
                        beneficiary_id: request.body.beneficiary_id,
                    })],
                }],
            })
        })
    }
}
