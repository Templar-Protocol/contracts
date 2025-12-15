use super::*;

#[near_sdk::ext_contract(ext_vault)]
pub trait VaultExt {
    // Role and admin
    fn set_curator(account: AccountId);
    fn set_is_allocator(account: AccountId, allowed: bool);
    fn submit_guardian(new_g: AccountId);
    fn accept_guardian();
    fn revoke_pending_guardian();
    fn submit_sentinel(new_s: AccountId);
    fn accept_sentinel();
    fn revoke_pending_sentinel();
    fn set_skim_recipient(account: AccountId);
    fn set_fees(fees: Fees<U128>);
    fn accept_fees();
    fn revoke_pending_fees();
    fn set_restrictions(restrictions: Option<Restrictions>);
    fn accept_restrictions();
    fn revoke_pending_restrictions();
    fn submit_timelock(new_timelock_ns: U64);
    fn accept_timelock();
    fn revoke_pending_timelock();

    // Market config and queues
    fn submit_cap(market: AccountId, new_cap: U128);
    fn accept_cap(market: AccountId);
    fn revoke_pending_cap(market: AccountId);
    fn submit_cap_group_update(update: CapGroupUpdate);
    fn accept_cap_group_update(update: CapGroupUpdateKey);
    fn revoke_pending_cap_group_update(update: CapGroupUpdateKey);
    fn submit_market_removal(market: AccountId);
    fn revoke_pending_market_removal(market: AccountId);
    fn set_supply_queue(markets: Vec<AccountId>);
    fn set_withdraw_queue(queue: Vec<AccountId>);

    // User flows
    fn withdraw(amount: U128, receiver: AccountId) -> PromiseOrValue<()>;
    fn redeem(shares: U128, receiver: AccountId) -> PromiseOrValue<()>;
    fn execute_next_withdrawal_request() -> PromiseOrValue<()>;
    fn skim(token: AccountId) -> Promise;
    fn allocate(weights: AllocationWeights, amount: Option<U128>) -> PromiseOrValue<()>;

    // Views
    fn get_configuration() -> VaultConfiguration;
    fn get_total_assets() -> U128;
    fn get_last_total_assets() -> U128;
    fn get_total_supply() -> U128;
    fn get_max_deposit() -> U128;
    fn convert_to_shares(assets: U128) -> U128;
    fn convert_to_assets(shares: U128) -> U128;
    fn preview_deposit(assets: U128) -> U128;
    fn preview_mint(shares: U128) -> U128;
    fn preview_withdraw(assets: U128) -> U128;
    fn preview_redeem(shares: U128) -> U128;
    fn get_cap_groups() -> Vec<(CapGroupId, CapGroupRecord)>;
    fn get_fee_anchor_timestamp() -> U64;
    fn get_fees() -> Fees<U128>;
    fn get_restrictions() -> Option<Restrictions>;
}
