use std::sync::Arc;

use blockchain_gateway_core::account;
use futures::future::BoxFuture;
use near_api::Account;

use crate::{
    actor::{operation_outcome_from_transaction_result, DispatchRead, DispatchWrite, RpcMessage},
    GatewayResult, NearClient,
};

impl DispatchRead for account::Get {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let account = client.account().get(params.0.params.account_id).await?;

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

impl DispatchWrite for account::Delete {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = Account(request.signer_account_id.0.clone())
                .delete_account_with_beneficiary(request.body.beneficiary_id)
                .with_signer(signer)
                .wait_until(request.wait_until.into())
                .send_to(client.network())
                .await
                .map_err(|error| crate::GatewayError::NearTransaction(error.to_string()))?;

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
