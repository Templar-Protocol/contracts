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

pub type ExpectedIdx = u32;
pub type ActualIdx = u32;
pub type AllocationWeights = Vec<(AccountId, u128)>;
pub type AllocationPlan = Vec<(AccountId, u128)>;

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub enum AllocationMode {
    //     When eager makes sense
    //
    //  • Retail/auto-pilot vaults: users expect deposits to “start earning” immediately without an active allocator.
    //  • Small/simple vaults: stable caps/ordering, few markets; operational simplicity > fine-grained control.
    //  • Integrations that assume quick deployment of idle assets.
    //
    // Risks/trade-offs of eager
    //
    //  • Gas burden on depositors: ft_transfer_call into your vault must carry enough gas for multi-hop allocation.
    //    Under-provisioned gas leads to partial allocations and extra callbacks.
    //  • Timing control: depositors implicitly decide when allocation runs, which can fight the allocator’s planned rebalancing
    //    cadence.
    //  • Thrashing: many small deposits can trigger many allocation passes.
    //  • Current code is “eager-ish but incomplete”: it only auto-starts when Idle, and does not auto-restart after the op. Deposits
    //    that arrive during an allocation stay idle until someone triggers another pass.
    //
    // Behaviour
    // • On deposit: if Idle and idle_balance ≥ min_batch, start_allocation(idle_balance).
    // • Eager allocation can still honor a per-op plan if one is set (plan wins); otherwise fall back to supply_queue order.
    Eager { min_batch: u128 },
    Lazy,
}
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

#[derive(Clone)]
#[near(serializers = [json, borsh])]
pub struct VaultConfiguration {
    pub mode: AllocationMode,
    pub owner: AccountId,
    pub curator: AccountId,
    pub guardian: AccountId,
    pub underlying_token: FungibleAsset<BorrowAsset>,
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

#[derive(Debug, Clone)]
#[near(serializers = [json, borsh])]
/// Operation state machine for asynchronous allocation, withdrawal, and payout flows.
pub enum OpState {
    Idle,
    Allocating {
        op_id: u64,
        index: u32,
        remaining: u128,
    },
    Withdrawing {
        op_id: u64,
        index: u32,
        remaining: u128,
        collected: u128,
        receiver: AccountId,
        owner: AccountId,
        escrow_shares: u128,
    },
    Payout {
        op_id: u64,
        receiver: AccountId,
        amount: u128,
        owner: AccountId,
        escrow_shares: u128,
    },
}

#[derive(Debug)]
#[near(serializers = [json])]
pub enum Error {
    // Invariant: Index drift or stale op_id results in a graceful stop
    IndexDrifted(ExpectedIdx, ActualIdx),
    // Invariant: Attempting to work on a market that is missing from the withdraw queue
    MissingMarket(u32),
    NotWithdrawing(OpState),
    NotAllocating(OpState),
    MarketTransferFailed,
    MissingSupplyPosition,
    PositionReadFailed,
    // Invariant: Insufficient liquidity across all markets to satisfy withdrawal
    InsufficientLiquidity,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
