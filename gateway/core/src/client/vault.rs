use moka::sync::Cache;
use near_account_id::AccountId;
use templar_common::vault::{
    AllocationDelta, CapGroupId, CapGroupRecord, CapGroupUpdate, CapGroupUpdateKey,
    FeeAccrualAnchor, Fees, MarketId, RealAssetsReport, Restrictions, TimelockKind,
    VaultConfiguration,
};
use templar_common::SU64;
use templar_primitives::SU128;

use crate::client::{
    cache::{immutable_cache, load_cached},
    macros::{contract_views, contract_writes},
    NearClient,
};

use super::BoundContractClient;

const VAULT_CONFIGURATION_CACHE_CAPACITY: u64 = 128;

#[derive(Clone)]
pub(crate) struct VaultClientCaches {
    pub configuration: Cache<AccountId, std::sync::Arc<VaultConfiguration>>,
}

impl VaultClientCaches {
    pub fn new() -> Self {
        Self {
            configuration: immutable_cache(VAULT_CONFIGURATION_CACHE_CAPACITY),
        }
    }
}

#[derive(serde::Serialize)]
pub struct U128AssetsArg {
    pub assets: SU128,
}

#[derive(serde::Serialize)]
pub struct U128SharesArg {
    pub shares: SU128,
}

#[derive(serde::Serialize)]
pub struct MarketAccountArg {
    pub market: AccountId,
}

#[derive(serde::Serialize)]
pub struct MarketIdArg {
    pub market_id: SU64,
}

#[derive(serde::Serialize)]
pub struct DeltaArg {
    pub delta: AllocationDelta,
}

#[derive(serde::Serialize)]
pub struct WithdrawArgs {
    pub amount: SU128,
    pub receiver: AccountId,
}

#[derive(serde::Serialize)]
pub struct RedeemArgs {
    pub shares: SU128,
    pub receiver: AccountId,
}

#[derive(serde::Serialize)]
pub struct ExecuteWithdrawalArgs {
    pub route: Vec<MarketId>,
}

#[derive(serde::Serialize)]
pub struct ExecuteMarketWithdrawalArgs {
    pub op_id: SU64,
    pub market: MarketId,
    pub batch_limit: Option<u32>,
}

#[derive(serde::Serialize)]
pub struct ExecuteRebalanceWithdrawalArgs {
    pub market_id: MarketId,
    pub batch_limit: Option<u32>,
}

#[derive(serde::Serialize)]
pub struct MarketsArg {
    pub markets: Vec<MarketId>,
}

#[derive(serde::Serialize)]
pub struct TokenArg {
    pub token: AccountId,
}

#[derive(serde::Serialize)]
pub struct CapArgs {
    pub market: AccountId,
    pub new_cap: SU128,
}

#[derive(serde::Serialize)]
pub struct CapMarketArg {
    pub market: AccountId,
}

#[derive(serde::Serialize)]
pub struct CapGroupUpdateArg {
    pub update: CapGroupUpdate,
}

#[derive(serde::Serialize)]
pub struct CapGroupUpdateKeyArg {
    pub update: CapGroupUpdateKey,
}

#[derive(serde::Serialize)]
pub struct AccountArg {
    pub account: AccountId,
}

#[derive(serde::Serialize)]
pub struct AllocatorArg {
    pub account: AccountId,
    pub allowed: bool,
}

#[derive(serde::Serialize)]
pub struct FeesArg {
    pub fees: Fees<SU128>,
}

#[derive(serde::Serialize)]
pub struct RestrictionsArg {
    pub restrictions: Option<Restrictions>,
}

#[derive(serde::Serialize)]
pub struct SubmitTimelockArgs {
    pub new_timelock_ns: SU64,
    pub kind: Option<TimelockKind>,
}

#[derive(Clone)]
pub struct VaultClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: AccountId,
}

impl BoundContractClient for VaultClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }

    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

impl VaultClient<'_> {
    pub async fn cached_get_configuration(&self) -> crate::GatewayResult<VaultConfiguration> {
        load_cached(
            &self.inner.cache().vault.configuration,
            self.contract_id.clone(),
            {
                let near = self.inner.clone();
                let contract_id = self.contract_id.clone();
                move || async move { near.vault(contract_id).get_configuration(()).await }
            },
        )
        .await
    }

    contract_views! {
        pub fn get_configuration(()) -> VaultConfiguration;
        pub fn get_total_assets(()) -> SU128;
        pub fn get_last_total_assets(()) -> SU128;
        pub fn get_idle_balance(()) -> SU128;
        pub fn get_total_supply(()) -> SU128;
        pub fn get_max_deposit(()) -> SU128;
        pub fn get_max_single_market_deposit(()) -> SU128;
        pub fn convert_to_shares(U128AssetsArg) -> SU128;
        pub fn convert_to_assets(U128SharesArg) -> SU128;
        pub fn preview_deposit(U128AssetsArg) -> SU128;
        pub fn preview_mint(U128SharesArg) -> SU128;
        pub fn preview_withdraw(U128AssetsArg) -> SU128;
        pub fn preview_redeem(U128SharesArg) -> SU128;
        pub fn get_cap_groups(()) -> Vec<(CapGroupId, CapGroupRecord)>;
        pub fn get_fee_anchor(()) -> FeeAccrualAnchor;
        pub fn get_fees(()) -> Fees<SU128>;
        pub fn get_restrictions(()) -> Option<Restrictions>;
        pub fn get_withdrawing_op_id(()) -> Option<SU64>;
        pub fn get_current_withdraw_request_id(()) -> Option<SU64>;
        pub fn has_pending_market_withdrawal(()) -> bool;
        pub fn queue_tail(()) -> u64;
        pub fn peek_next_pending_withdrawal_id(()) -> Option<u64>;
        pub fn build_real_assets_report(()) -> RealAssetsReport;
        pub fn get_market_id_of_account(MarketAccountArg) -> Option<MarketId>;
        pub fn get_market_account_by_id(MarketIdArg) -> Option<AccountId>;
        pub fn list_markets_with_ids(()) -> Vec<(SU64, AccountId)>;
    }

    contract_writes! {
        pub fn allocate(DeltaArg);
        pub fn withdraw(WithdrawArgs);
        pub fn redeem(RedeemArgs);
        pub fn execute_withdrawal(ExecuteWithdrawalArgs);
        pub fn execute_market_withdrawal(ExecuteMarketWithdrawalArgs);
        pub fn execute_rebalance_withdrawal(ExecuteRebalanceWithdrawalArgs);
        pub fn resync_idle_balance(());
        pub fn refresh_markets(MarketsArg);
        pub fn unbrick(());
        pub fn skim(TokenArg);
        pub fn set_supply_queue(MarketsArg);
        pub fn submit_cap(CapArgs);
        pub fn accept_cap(CapMarketArg);
        pub fn revoke_pending_cap(CapMarketArg);
        pub fn submit_cap_group_update(CapGroupUpdateArg);
        pub fn accept_cap_group_update(CapGroupUpdateKeyArg);
        pub fn revoke_pending_cap_group_update(CapGroupUpdateKeyArg);
        pub fn submit_market_removal(CapMarketArg);
        pub fn accept_market_removal(CapMarketArg);
        pub fn revoke_pending_market_removal(CapMarketArg);
        pub fn set_curator(AccountArg);
        pub fn set_is_allocator(AllocatorArg);
        pub fn submit_sentinel(AccountArg);
        pub fn accept_sentinel(());
        pub fn revoke_pending_sentinel(());
        pub fn set_skim_recipient(AccountArg);
        pub fn set_fees(FeesArg);
        pub fn accept_fees(());
        pub fn revoke_pending_fees(());
        pub fn set_restrictions(RestrictionsArg);
        pub fn accept_restrictions(());
        pub fn revoke_pending_restrictions(());
        pub fn submit_timelock(SubmitTimelockArgs);
        pub fn accept_timelock(());
        pub fn revoke_pending_timelock(());
    }
}
