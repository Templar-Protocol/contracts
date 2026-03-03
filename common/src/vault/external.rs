use super::{
    AccountId, AllocationDelta, CapGroupId, CapGroupRecord, CapGroupUpdate, CapGroupUpdateKey,
    Fees, MarketId, Promise, PromiseOrValue, RealAssetsReport, Restrictions, TimelockKind,
    VaultConfiguration, U128, U64,
};

#[near_sdk::ext_contract(ext_vault)]
pub trait VaultExternalInterface {
    fn set_curator(&mut self, account: AccountId);
    fn set_is_allocator(&mut self, account: AccountId, allowed: bool);
    fn submit_guardian(&mut self, new_g: AccountId);
    fn accept_guardian(&mut self);
    fn revoke_pending_guardian(&mut self);
    fn submit_sentinel(&mut self, new_s: AccountId);
    fn accept_sentinel(&mut self);
    fn revoke_pending_sentinel(&mut self);
    fn set_skim_recipient(&mut self, account: AccountId);
    fn set_fees(&mut self, fees: Fees<U128>);
    fn accept_fees(&mut self);
    fn revoke_pending_fees(&mut self);
    fn set_restrictions(&mut self, restrictions: Option<Restrictions>);
    fn accept_restrictions(&mut self);
    fn revoke_pending_restrictions(&mut self);
    fn submit_timelock(&mut self, new_timelock_ns: U64, kind: Option<TimelockKind>);
    fn accept_timelock(&mut self);
    fn revoke_pending_timelock(&mut self);

    fn submit_cap(&mut self, market: AccountId, new_cap: U128);
    fn accept_cap(&mut self, market: AccountId);
    fn revoke_pending_cap(&mut self, market: AccountId);
    fn submit_cap_group_update(&mut self, update: CapGroupUpdate);
    fn accept_cap_group_update(&mut self, update: CapGroupUpdateKey);
    fn revoke_pending_cap_group_update(&mut self, update: CapGroupUpdateKey);
    fn submit_market_removal(&mut self, market: AccountId);
    fn revoke_pending_market_removal(&mut self, market: AccountId);
    fn set_supply_queue(&mut self, markets: Vec<MarketId>);

    fn withdraw(&mut self, amount: U128, receiver: AccountId) -> PromiseOrValue<()>;
    fn redeem(&mut self, shares: U128, receiver: AccountId) -> PromiseOrValue<()>;
    fn reallocate(&mut self, delta: AllocationDelta) -> PromiseOrValue<()>;
    fn execute_rebalance_withdrawal(
        &mut self,
        market_id: MarketId,
        batch_limit: Option<u32>,
    ) -> PromiseOrValue<()>;
    fn execute_withdrawal(&mut self, route: Vec<MarketId>) -> PromiseOrValue<()>;
    fn execute_market_withdrawal(
        &mut self,
        op_id: U64,
        market: MarketId,
        batch_limit: Option<u32>,
    ) -> PromiseOrValue<()>;
    fn unbrick(&mut self) -> PromiseOrValue<()>;
    fn skim(&mut self, token: AccountId) -> Promise;
    fn refresh_markets(&mut self, markets: Vec<MarketId>) -> PromiseOrValue<RealAssetsReport>;

    fn get_configuration(&self) -> VaultConfiguration;
    fn get_total_assets(&self) -> U128;
    fn get_last_total_assets(&self) -> U128;
    fn get_total_supply(&self) -> U128;
    fn get_max_deposit(&self) -> U128;
    fn convert_to_shares(&self, assets: U128) -> U128;
    fn convert_to_assets(&self, shares: U128) -> U128;
    fn preview_deposit(&self, assets: U128) -> U128;
    fn preview_mint(&self, shares: U128) -> U128;
    fn preview_withdraw(&self, assets: U128) -> U128;
    fn preview_redeem(&self, shares: U128) -> U128;
    fn get_cap_groups(&self) -> Vec<(CapGroupId, CapGroupRecord)>;
    fn get_fee_anchor_timestamp(&self) -> U64;
    fn get_fees(&self) -> Fees<U128>;
    fn get_restrictions(&self) -> Option<Restrictions>;
}

pub trait VaultExt: VaultExternalInterface {}
impl<T: VaultExternalInterface + ?Sized> VaultExt for T {}
