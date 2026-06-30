use crate::{client::NearClient, GatewayResult, ReadNear};

#[derive(Clone, Copy)]
pub struct AccountClient<'a> {
    pub(crate) inner: &'a NearClient,
}

impl AccountClient<'_> {
    pub async fn get(
        &self,
        account_id: near_account_id::AccountId,
    ) -> GatewayResult<near_api::types::account::Account> {
        <NearClient as ReadNear>::view_account(self.inner, account_id).await
    }

    pub async fn access_key(
        &self,
        account_id: near_account_id::AccountId,
        public_key: near_api::types::PublicKey,
    ) -> GatewayResult<near_api::types::transaction::actions::AccessKey> {
        <NearClient as ReadNear>::view_access_key(self.inner, account_id, public_key).await
    }
}
