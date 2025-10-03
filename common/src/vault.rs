use near_sdk::{json_types::U128, near, AccountId, Gas};

use crate::{
    asset::{BorrowAsset, FungibleAsset},
    supply::SupplyPosition,
};

pub type TimestampNs = u64;

// FIXME:
pub const GAS_XFER: Gas = Gas::from_tgas(4);
pub const GAS_CB: Gas = Gas::from_tgas(30);
pub const ONE_YOCTO: u128 = 1;

pub const MIN_TIMELOCK_NS: u64 = 86_400_000_000_000; // 1 day
pub const MAX_TIMELOCK_NS: u64 = 30 * 86_400_000_000_000; // 30 days
pub const MAX_QUEUE_LEN: usize = 64;

/// Parsed from the string parameter `msg` passed by `*_transfer_call` to
/// `*_on_transfer` calls.
#[near(serializers = [json])]
pub enum DepositMsg {
    /// Add the attached tokens to the sender's vault position.
    Supply,
}

#[derive(Clone, Default)]
#[near]
pub struct MarketConfiguration {
    // Supply cap for this market (in underlying asset units)
    pub cap: u128,
    // Whether market is enabled for deposits/withdrawals
    pub enabled: bool,
    // Timestamp (ns) after which market can be removed (if pending removal)
    pub removable_at: TimestampNs,
}

#[near(serializers = [json, borsh])]
pub struct VaultConfiguration {
    pub owner_id: AccountId,
    pub curator_id: AccountId,
    pub guardian_id: AccountId,
    pub underlying_token_id: FungibleAsset<BorrowAsset>,
    pub initial_timelock_sec: u32,
    pub fee_recipient: AccountId,
    pub skim_recipient: AccountId,
    pub name: String,
    pub symbol: String,
    // TODO: decide if should assert decimals as underlying
    pub decimals: u8,
}

#[near_sdk::ext_contract(ext_self)]
pub trait Callbacks {
    fn after_supply_1_check(&mut self, op_id: u64, market_index: u32, attempted: U128) -> bool;
    fn after_supply_2_read(
        &mut self,
        op_id: u64,
        market_index: u32,
        before: U128,
        attempted: U128,
        refunded: U128,
    ) -> bool;

    fn after_create_withdraw_req(&mut self, op_id: u64, market_index: u32, need: U128) -> bool;
    fn after_exec_withdraw_req(&mut self, op_id: u64, market_index: u32, need: U128) -> bool;

    fn after_send_to_user(&mut self, op_id: u64, receiver: AccountId, amount: U128) -> bool;

    fn after_skim_balance(&mut self, token: AccountId, recipient: AccountId) -> bool;
}

#[derive(Clone)]
#[near]
pub struct PendingValue<T> {
    pub value: T,
    // Timestamp when this pending value can be finalized
    pub valid_at: TimestampNs,
}
