use blockchain_gateway_core::account;
use near_api::Account;

use crate::{
    client::NearClient,
    error::{GatewayError, GatewayResult},
};

#[derive(Clone, Copy)]
pub struct AccountClient<'a> {
    pub(crate) inner: &'a NearClient,
}

impl AccountClient<'_> {
    pub async fn get(&self, params: account::GetParams) -> GatewayResult<account::GetResult> {
        let account = Account(params.account_id)
            .view()
            .fetch_from(self.inner.network())
            .await
            .map_err(|error| GatewayError::NearQuery(error.to_string()))?;
        let (code_hash, global_contract_hash, global_contract_account_id) =
            match account.data.contract_state {
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
            amount: account.data.amount,
            locked: account.data.locked,
            code_hash,
            storage_usage: account.data.storage_usage,
            global_contract_hash,
            global_contract_account_id,
        })
    }
}
