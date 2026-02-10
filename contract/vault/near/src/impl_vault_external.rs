use crate::Contract;
use near_sdk::{
    json_types::{U128, U64},
    AccountId, Promise, PromiseOrValue,
};
use templar_common::vault::{
    AllocationDelta, CapGroupId, CapGroupRecord, CapGroupUpdate, CapGroupUpdateKey, Fees, MarketId,
    RealAssetsReport, Restrictions, TimelockKind, VaultConfiguration, VaultExternalInterface,
};

impl VaultExternalInterface for Contract {
    fn set_curator(&mut self, account: AccountId) {
        Contract::set_curator(self, account);
    }

    fn set_is_allocator(&mut self, account: AccountId, allowed: bool) {
        Contract::set_is_allocator(self, account, allowed);
    }

    fn submit_guardian(&mut self, account: AccountId) {
        Contract::submit_guardian(self, account);
    }

    fn accept_guardian(&mut self) {
        Contract::accept_guardian(self);
    }

    fn revoke_pending_guardian(&mut self) {
        Contract::revoke_pending_guardian(self);
    }

    fn submit_sentinel(&mut self, account: AccountId) {
        Contract::submit_sentinel(self, account);
    }

    fn accept_sentinel(&mut self) {
        Contract::accept_sentinel(self);
    }

    fn revoke_pending_sentinel(&mut self) {
        Contract::revoke_pending_sentinel(self);
    }

    fn set_skim_recipient(&mut self, account: AccountId) {
        Contract::set_skim_recipient(self, account);
    }

    fn set_fees(&mut self, fees: Fees<U128>) {
        Contract::set_fees(self, fees);
    }

    fn accept_fees(&mut self) {
        Contract::accept_fees(self);
    }

    fn revoke_pending_fees(&mut self) {
        Contract::revoke_pending_fees(self);
    }

    fn set_restrictions(&mut self, restrictions: Option<Restrictions>) {
        Contract::set_restrictions(self, restrictions);
    }

    fn accept_restrictions(&mut self) {
        Contract::accept_restrictions(self);
    }

    fn revoke_pending_restrictions(&mut self) {
        Contract::revoke_pending_restrictions(self);
    }

    fn submit_timelock(&mut self, new_timelock_ns: U64, kind: Option<TimelockKind>) {
        Contract::submit_timelock(self, new_timelock_ns, kind);
    }

    fn accept_timelock(&mut self) {
        Contract::accept_timelock(self);
    }

    fn revoke_pending_timelock(&mut self) {
        Contract::revoke_pending_timelock(self);
    }

    fn submit_cap(&mut self, market: AccountId, new_cap: U128) {
        Contract::submit_cap(self, market, new_cap);
    }

    fn accept_cap(&mut self, market: AccountId) {
        Contract::accept_cap(self, market);
    }

    fn revoke_pending_cap(&mut self, market: AccountId) {
        Contract::revoke_pending_cap(self, market);
    }

    fn submit_cap_group_update(&mut self, update: CapGroupUpdate) {
        Contract::submit_cap_group_update(self, update);
    }

    fn accept_cap_group_update(&mut self, update: CapGroupUpdateKey) {
        Contract::accept_cap_group_update(self, update);
    }

    fn revoke_pending_cap_group_update(&mut self, update: CapGroupUpdateKey) {
        Contract::revoke_pending_cap_group_update(self, update);
    }

    fn submit_market_removal(&mut self, market: AccountId) {
        Contract::submit_market_removal(self, market);
    }

    fn revoke_pending_market_removal(&mut self, market: AccountId) {
        Contract::revoke_pending_market_removal(self, market);
    }

    fn set_supply_queue(&mut self, markets: Vec<MarketId>) {
        Contract::set_supply_queue(self, markets);
    }

    fn withdraw(&mut self, amount: U128, receiver: AccountId) -> PromiseOrValue<()> {
        Contract::withdraw(self, amount, receiver)
    }

    fn redeem(&mut self, shares: U128, receiver: AccountId) -> PromiseOrValue<()> {
        Contract::redeem(self, shares, receiver)
    }

    fn reallocate(&mut self, delta: AllocationDelta) -> PromiseOrValue<()> {
        Contract::reallocate(self, delta)
    }

    fn execute_rebalance_withdrawal(
        &mut self,
        market_id: MarketId,
        batch_limit: Option<u32>,
    ) -> PromiseOrValue<()> {
        Contract::execute_rebalance_withdrawal(self, market_id, batch_limit)
    }

    fn execute_withdrawal(&mut self, route: Vec<MarketId>) -> PromiseOrValue<()> {
        Contract::execute_withdrawal(self, route)
    }

    fn execute_market_withdrawal(
        &mut self,
        op_id: U64,
        market: MarketId,
        batch_limit: Option<u32>,
    ) -> PromiseOrValue<()> {
        Contract::execute_market_withdrawal(self, op_id, market, batch_limit)
    }

    fn unbrick(&mut self) -> PromiseOrValue<()> {
        Contract::unbrick(self)
    }

    fn skim(&mut self, token: AccountId) -> Promise {
        Contract::skim(self, token)
    }

    fn refresh_markets(&mut self, markets: Vec<MarketId>) -> PromiseOrValue<RealAssetsReport> {
        Contract::refresh_markets(self, markets)
    }

    fn get_configuration(&self) -> VaultConfiguration {
        Contract::get_configuration(self)
    }

    fn get_total_assets(&self) -> U128 {
        Contract::get_total_assets(self)
    }

    fn get_last_total_assets(&self) -> U128 {
        Contract::get_last_total_assets(self)
    }

    fn get_total_supply(&self) -> U128 {
        Contract::get_total_supply(self)
    }

    fn get_max_deposit(&self) -> U128 {
        Contract::get_max_deposit(self)
    }

    fn convert_to_shares(&self, assets: U128) -> U128 {
        Contract::convert_to_shares(self, assets)
    }

    fn convert_to_assets(&self, shares: U128) -> U128 {
        Contract::convert_to_assets(self, shares)
    }

    fn preview_deposit(&self, assets: U128) -> U128 {
        Contract::preview_deposit(self, assets)
    }

    fn preview_mint(&self, shares: U128) -> U128 {
        Contract::preview_mint(self, shares)
    }

    fn preview_withdraw(&self, assets: U128) -> U128 {
        Contract::preview_withdraw(self, assets)
    }

    fn preview_redeem(&self, shares: U128) -> U128 {
        Contract::preview_redeem(self, shares)
    }

    fn get_cap_groups(&self) -> Vec<(CapGroupId, CapGroupRecord)> {
        Contract::get_cap_groups(self)
    }

    fn get_fee_anchor_timestamp(&self) -> U64 {
        self.fee_anchor.timestamp_ns
    }

    fn get_fees(&self) -> Fees<U128> {
        Contract::get_fees(self)
    }

    fn get_restrictions(&self) -> Option<Restrictions> {
        Contract::get_restrictions(self)
    }
}
