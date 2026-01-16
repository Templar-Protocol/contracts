use std::{sync::RwLock, time::Duration};

use anyhow::{bail, Result};
use near_account_id::AccountId as NearAccountId;
use near_jsonrpc_client::{methods::query::RpcQueryRequest, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::{
    types::{BlockReference, Gas},
    views::QueryRequest,
};
use near_sdk::json_types::{U128, U64};
use serde::{de::DeserializeOwned, Serialize};
use tracing::instrument;

use crate::{
    lock_ext::RwLockExt, parse_account_id, retry, AccountId, AllocationDelta, CapGroup,
    CapGroupUpdate, CapGroupUpdateKey, ErrorWrapper, FeeAccrualAnchor, Fees, ForeignU128,
    KeyPoolConfig, MarketId, MarketWithId, PendingGovernanceAction, PendingValueSerde,
    RealAssetsReport, Restrictions, TimelockKind, VaultConfiguration, VaultSnapshot, ViewCache,
    ViewCacheKey,
};

#[derive(uniffi::Object)]
pub struct VaultViewClient {
    vault: NearAccountId,
    inner: JsonRpcClient,
    config: KeyPoolConfig,
    view_cache: RwLock<Option<ViewCache>>,
}

#[uniffi::export(async_runtime = "tokio")]
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
        let inner = JsonRpcClient::connect(rpc_url);
        let vault: NearAccountId = parse_account_id(vault)?;

        let view_cache = if config.view_cache_capacity > 0 {
            Some(
                ViewCache::builder()
                    .max_capacity(config.view_cache_capacity as u64)
                    .time_to_live(Duration::from_secs(config.view_cache_ttl_seconds))
                    .build(),
            )
        } else {
            None
        };

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

    pub fn enable_view_cache(&self, capacity: u32, ttl_seconds: u64) {
        if capacity == 0 {
            *self.view_cache.write_recover() = None;
            return;
        }

        let cache = ViewCache::builder()
            .max_capacity(capacity as u64)
            .time_to_live(Duration::from_secs(ttl_seconds))
            .build();

        *self.view_cache.write_recover() = Some(cache);
    }

    pub fn disable_view_cache(&self) {
        *self.view_cache.write_recover() = None;
    }

    pub async fn clear_view_cache(&self) -> Result<(), ErrorWrapper> {
        let cache = { self.view_cache.read_recover().clone() };
        if let Some(cache) = cache {
            cache.invalidate_all();
        }
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn get_cap_groups(&self) -> Result<Vec<CapGroup>, ErrorWrapper> {
        let groups = self
            .view::<Vec<(
                templar_common::vault::CapGroupId,
                templar_common::vault::CapGroupRecord,
            )>>(&self.vault, "get_cap_groups", ())
            .await
            .map_err(ErrorWrapper::from)?;

        Ok(groups
            .into_iter()
            .map(|(id, rec)| CapGroup {
                id: id.into(),
                cap: rec.cap.0.to_string(),
                relative_cap: u128::from(rec.relative_cap).to_string(),
                principal: rec.principal.to_string(),
            })
            .collect())
    }

    #[instrument(skip(self))]
    pub async fn get_pending_governance_actions(
        &self,
    ) -> Result<Vec<PendingGovernanceAction>, ErrorWrapper> {
        let pending = self
            .view::<Vec<PendingValueSerde>>(&self.vault, "get_pending_governance_actions", ())
            .await
            .map_err(ErrorWrapper::from)?;

        Ok(pending
            .into_iter()
            .map(|p| PendingGovernanceAction {
                action: p.value.into(),
                valid_at_ns: p.valid_at_ns,
            })
            .collect())
    }

    #[instrument(skip(self, market))]
    pub async fn get_market_id_of_account(
        &self,
        market: &AccountId,
    ) -> Result<Option<MarketId>, ErrorWrapper> {
        let res = self
            .view::<Option<U64>>(
                &self.vault,
                "get_market_id_of_account",
                (self.near_id(market)?,),
            )
            .await
            .map_err(ErrorWrapper::from)?;

        let Some(u) = res else {
            return Ok(None);
        };

        let id_u32: u32 =
            u.0.try_into()
                .map_err(|_| ErrorWrapper::Wrapped("market id out of u32 range".to_string()))?;

        Ok(Some(MarketId(id_u32)))
    }

    #[instrument(skip(self, market_id))]
    pub async fn get_market_account_by_id(
        &self,
        market_id: MarketId,
    ) -> Result<Option<AccountId>, ErrorWrapper> {
        let res = self
            .view::<Option<NearAccountId>>(
                &self.vault,
                "get_market_account_by_id",
                (U64::from(market_id.0 as u64),),
            )
            .await
            .map_err(ErrorWrapper::from)?;

        Ok(res.map(|a| AccountId::from(a.to_string())))
    }

    #[instrument(skip(self))]
    pub async fn list_markets_with_ids(&self) -> Result<Vec<MarketWithId>, ErrorWrapper> {
        let res = self
            .view::<Vec<(U64, NearAccountId)>>(&self.vault, "list_markets_with_ids", ())
            .await
            .map_err(ErrorWrapper::from)?;

        let mapped =
            res.into_iter()
                .map(|(id, account)| {
                    let id_u32: u32 = id.0.try_into().map_err(|_| {
                        ErrorWrapper::Wrapped("market id out of u32 range".to_string())
                    })?;
                    Ok(MarketWithId {
                        market_id: MarketId(id_u32),
                        account: AccountId::from(account.to_string()),
                    })
                })
                .collect::<Result<Vec<_>, ErrorWrapper>>()?;

        Ok(mapped)
    }

    #[instrument(skip(self))]
    pub async fn get_vault_snapshot(&self) -> Result<VaultSnapshot, ErrorWrapper> {
        Ok(VaultSnapshot {
            configuration: self.get_configuration().await?,
            total_assets: self.get_total_assets().await?,
            last_total_assets: self.get_last_total_assets().await?,
            idle_balance: self.get_idle_balance().await?,
            total_supply: self.get_total_supply().await?,
            max_deposit: self.get_max_deposit().await?,
            max_single_market_deposit: self.get_max_single_market_deposit().await?,
            fee_anchor: self.get_fee_anchor().await?,
            fees: self.get_fees().await?,
            restrictions: self.get_restrictions().await?,
            cap_groups: self.get_cap_groups().await?,
            pending_governance_actions: self.get_pending_governance_actions().await?,
            withdrawing_op_id: self.get_withdrawing_op_id().await?,
            has_pending_market_withdrawal: self.has_pending_market_withdrawal().await?,
            current_withdraw_request_id: self.get_current_withdraw_request_id().await?,
            queue_tail: self.queue_tail().await?,
            next_pending_withdrawal_id: self.peek_next_pending_withdrawal_id().await?,
            markets_with_ids: self.list_markets_with_ids().await?,
        })
    }

    #[instrument(skip(self, markets))]
    pub async fn resolve_market_ids(
        &self,
        markets: &[AccountId],
    ) -> Result<Vec<Option<MarketId>>, ErrorWrapper> {
        let mut out = Vec::with_capacity(markets.len());
        for market in markets {
            out.push(self.get_market_id_of_account(market).await?);
        }
        Ok(out)
    }

    #[instrument(skip(self, market_ids))]
    pub async fn resolve_market_accounts(
        &self,
        market_ids: &[MarketId],
    ) -> Result<Vec<Option<AccountId>>, ErrorWrapper> {
        let mut out = Vec::with_capacity(market_ids.len());
        for id in market_ids {
            out.push(self.get_market_account_by_id(*id).await?);
        }
        Ok(out)
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
        let args_bytes = serde_json::to_vec(&args)?;
        let key = ViewCacheKey {
            account_id: account_id.to_string(),
            method: function_name.to_string(),
            args: args_bytes.clone(),
        };

        let cache = { self.view_cache.read_recover().clone() };
        if let Some(cache) = &cache {
            if let Some(bytes) = cache.get(&key) {
                let value = serde_json::from_slice(&bytes)?;
                return Ok(value);
            }
        }

        let timeout = Duration::from_secs(self.config.timeout_seconds);
        let mut retry_state = retry::RetryState::new(self.config.retry);

        loop {
            retry_state.begin_attempt();

            let response = tokio::time::timeout(
                timeout,
                self.inner.call(RpcQueryRequest {
                    block_reference: BlockReference::latest(),
                    request: QueryRequest::CallFunction {
                        account_id: account_id.clone(),
                        method_name: function_name.to_owned(),
                        args: args_bytes.clone().into(),
                    },
                }),
            )
            .await;

            let response = match response {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    let err: anyhow::Error = e.into();
                    if !retry_state.should_retry_err(&err).await {
                        return Err(err);
                    }
                    continue;
                }
                Err(e) => {
                    let err: anyhow::Error = e.into();
                    if !retry_state.should_retry_err(&err).await {
                        return Err(err);
                    }
                    continue;
                }
            };

            let QueryResponseKind::CallResult(result) = response.kind else {
                bail!("Expected CallResult got {:?}", response.kind);
            };

            if let Some(cache) = &cache {
                cache.insert(key.clone(), result.result.clone());
            }

            let value = serde_json::from_slice(&result.result)?;
            return Ok(value);
        }
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
