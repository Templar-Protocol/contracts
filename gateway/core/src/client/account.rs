use near_api::Account;

use crate::{client::NearClient, GatewayError, GatewayResult};

#[derive(Clone, Copy)]
pub struct AccountClient<'a> {
    pub(crate) inner: &'a NearClient,
}

impl AccountClient<'_> {
    pub async fn get(
        &self,
        account_id: near_account_id::AccountId,
    ) -> GatewayResult<near_api::types::account::Account> {
        let account = Account(account_id)
            .view()
            .fetch_from(self.inner.network())
            .await
            .map_err(|error| GatewayError::NearQuery(error.to_string()))?;
        Ok(account.data)
    }
}
