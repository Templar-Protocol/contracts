use std::sync::RwLock;

use anyhow::Result;
use near_account_id::AccountId as NearAccountId;
use near_jsonrpc_client::{auth::ApiKey, JsonRpcClient};
use near_sdk::json_types::{U128, U64};
use serde::{de::DeserializeOwned, Serialize};
use tracing::instrument;

use crate::{
    parse_account_id, view_core, AccountId, ErrorWrapper, FeeAccrualAnchor, Fees, ForeignU128,
    KeyPoolConfig, RealAssetsReport, Restrictions, VaultConfiguration, ViewCache,
};

/// Read-only vault client that only exposes view/cache APIs and never signs transactions.
#[derive(uniffi::Object)]
pub struct VaultViewClient {
    vault: NearAccountId,
    inner: JsonRpcClient,
    config: KeyPoolConfig,
    view_cache: RwLock<Option<ViewCache>>,
}

#[uniffi::export]
impl VaultViewClient {
    #[uniffi::constructor]
    #[instrument(skip(rpc_url))]
    pub fn new_default(rpc_url: String, vault: &AccountId) -> Result<Self, ErrorWrapper> {
        Self::new(rpc_url, vault, KeyPoolConfig::default())
    }

    #[uniffi::constructor]
    #[instrument(skip(rpc_url, config))]
    pub fn new(
        rpc_url: String,
        vault: &AccountId,
        config: KeyPoolConfig,
    ) -> Result<Self, ErrorWrapper> {
        let inner = {
            let client = JsonRpcClient::connect(rpc_url);
            if let Some(api_key) = &config.rpc_api_key {
                let api_key =
                    ApiKey::new(api_key).map_err(|e| ErrorWrapper::Wrapped(e.to_string()))?;
                client.header(api_key)
            } else {
                client
            }
        };
        let vault: NearAccountId = parse_account_id(vault)?;

        let view_cache = view_core::build_view_cache(&config);

        Ok(Self {
            vault,
            inner,
            config,
            view_cache: RwLock::new(view_cache),
        })
    }

    pub fn vault_account(&self) -> AccountId {
        AccountId::from(self.vault.to_string())
    }
}

impl VaultViewClient {
    #[inline]
    #[allow(clippy::unused_self)]
    fn near_id(&self, id: &AccountId) -> Result<NearAccountId, ErrorWrapper> {
        parse_account_id(id)
    }

    async fn vault_view_u128(
        &self,
        method: &str,
        args: impl Serialize,
    ) -> Result<ForeignU128, ErrorWrapper> {
        let u = self
            .view::<U128>(&self.vault, method, args)
            .await
            .map_err(ErrorWrapper::from)?;
        Ok(u.0.to_string())
    }

    #[instrument(skip(self, args), fields(account_id = %account_id, method = function_name))]
    async fn view<T: DeserializeOwned>(
        &self,
        account_id: &NearAccountId,
        function_name: &str,
        args: impl Serialize,
    ) -> Result<T> {
        view_core::view_with_cache(
            &self.inner,
            &self.config,
            &self.view_cache,
            account_id,
            function_name,
            args,
        )
        .await
    }
}

crate::impl_view_cache_methods!(VaultViewClient);
crate::impl_vault_view_methods!(VaultViewClient);
crate::impl_vault_read_methods!(VaultViewClient);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_default_does_not_require_secret_key() {
        let vault = AccountId::from("vault.testnet".to_string());
        let client =
            VaultViewClient::new_default("https://rpc.testnet.near.org".to_string(), &vault);
        assert!(client.is_ok());
    }
}
