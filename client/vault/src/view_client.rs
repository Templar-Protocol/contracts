use std::sync::RwLock;

use anyhow::Result;
use near_account_id::AccountId as NearAccountId;
use near_jsonrpc_client::{auth::ApiKey, JsonRpcClient};
use near_primitives::types::Gas;
use near_sdk::json_types::{U128, U64};
use serde::{de::DeserializeOwned, Serialize};
use tracing::instrument;

use crate::{
    parse_account_id, AccountId, AllocationDelta,
    CapGroupUpdate, CapGroupUpdateKey, ErrorWrapper, FeeAccrualAnchor, Fees, ForeignU128,
    KeyPoolConfig, MarketId, RealAssetsReport,
    Restrictions, TimelockKind, VaultConfiguration, ViewCache, view_core,
};

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
    #[instrument(fields(rpc_url = %rpc_url))]
    pub fn new_default(rpc_url: String, vault: &AccountId) -> Result<Self, ErrorWrapper> {
        Self::new(rpc_url, vault, KeyPoolConfig::default())
    }

    #[uniffi::constructor]
    #[instrument(skip(config), fields(rpc_url = %rpc_url))]
    pub fn new(
        rpc_url: String,
        vault: &AccountId,
        config: KeyPoolConfig,
    ) -> Result<Self, ErrorWrapper> {
        let inner = {
            let client = JsonRpcClient::connect(rpc_url);
            if let Some(api_key) = &config.rpc_api_key {
                let api_key = ApiKey::new(api_key)
                    .map_err(|e| ErrorWrapper::Wrapped(e.to_string()))?;
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
        view_core::view_with_cache(&self.inner, &self.config, &self.view_cache, account_id, function_name, args).await
    }

    async fn vault_call_with(
        &self,
        _method: &str,
        _args: impl Serialize,
        _gas: Option<Gas>,
        _deposit: Option<u128>,
    ) -> Result<(), ErrorWrapper> {
        Err(ErrorWrapper::Wrapped(
            "VaultViewClient is read-only; contract calls are not supported".to_string(),
        ))
    }

    async fn vault_call(&self, method: &str, args: impl Serialize) -> Result<(), ErrorWrapper> {
        self.vault_call_with(method, args, None, None).await
    }

    async fn vault_call_returning<T: DeserializeOwned>(
        &self,
        _method: &str,
        _args: impl Serialize,
        _gas: Option<Gas>,
        _deposit: Option<u128>,
    ) -> Result<T, ErrorWrapper> {
        Err(ErrorWrapper::Wrapped(
            "VaultViewClient is read-only; contract calls are not supported".to_string(),
        ))
    }
}

// Generate view cache management methods via macro
crate::impl_view_cache_methods!(VaultViewClient);

// Generate complex view methods via macro
crate::impl_vault_view_methods!(VaultViewClient);

// Generate common vault methods via macro
crate::impl_vault_methods!(VaultViewClient);

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

    #[tokio::test]
    async fn call_methods_fail_with_read_only_error() {
        let vault = AccountId::from("vault.testnet".to_string());
        let client =
            VaultViewClient::new_default("https://rpc.testnet.near.org".to_string(), &vault)
                .unwrap();

        let err = client.accept_guardian().await.unwrap_err();
        assert!(matches!(
            err,
            ErrorWrapper::Wrapped(msg) if msg.contains("read-only")
        ));
    }
}
