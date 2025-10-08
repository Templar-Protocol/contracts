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
    ZeroAmount,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[near(event_json(standard = "templar-vault"))]
pub enum Event {
    #[event_version("1.0.0")]
    MintedShares { amount: U128, receiver: AccountId },
    #[event_version("1.0.0")]
    AllocationStarted { op_id: u64, remaining: U128 },

    // Allocation lifecycle (plan/request)
    #[event_version("1.0.0")]
    AllocationRequestedQueue { op_id: u64, total: U128 },
    #[event_version("1.0.0")]
    AllocationRequestedWeighted {
        op_id: u64,
        total: U128,
        weights: Vec<(AccountId, U128)>,
    },
    #[event_version("1.0.0")]
    AllocationPlanSet {
        op_id: u64,
        plan: Vec<(AccountId, U128)>,
    },

    // Per-step planning and outcomes
    #[event_version("1.0.0")]
    AllocationStepPlanned {
        op_id: u64,
        index: u32,
        market: AccountId,
        target: U128,
        room: U128,
        to_supply: U128,
        remaining_before: U128,
        planned: bool,
    },
    #[event_version("1.0.0")]
    AllocationStepSkipped {
        op_id: u64,
        index: u32,
        market: AccountId,
        reason: String,
        remaining: U128,
    },
    #[event_version("1.0.0")]
    AllocationTransferFailed {
        op_id: u64,
        index: u32,
        market: AccountId,
        attempted: U128,
    },
    #[event_version("1.0.0")]
    AllocationStepSettled {
        op_id: u64,
        index: u32,
        market: AccountId,
        before: U128,
        new_principal: U128,
        accepted: U128,
        attempted: U128,
        refunded: U128,
        remaining_after: U128,
    },

    // Completion and stop
    #[event_version("1.0.0")]
    AllocationCompleted { op_id: u64 },
    #[event_version("1.0.0")]
    AllocationStopped {
        op_id: u64,
        index: u32,
        remaining: U128,
        reason: Option<String>,
    },

    // Eager
    #[event_version("1.0.0")]
    AllocationEagerTriggered {
        op_id: u64,
        idle_balance: U128,
        min_batch: U128,
        deposit_accepted: U128,
    },

    // Admin and configuration events
    #[event_version("1.0.0")]
    CuratorSet { account: AccountId },
    #[event_version("1.0.0")]
    AllocatorRoleSet { account: AccountId, allowed: bool },
    #[event_version("1.0.0")]
    SkimRecipientSet { account: AccountId },
    #[event_version("1.0.0")]
    FeeRecipientSet { account: AccountId },
    #[event_version("1.0.0")]
    PerformanceFeeSet { fee: U128 },

    #[event_version("1.0.0")]
    TimelockSet { seconds: u32 },
    #[event_version("1.0.0")]
    TimelockChangeSubmitted { new_seconds: u32, valid_at: u64 },
    #[event_version("1.0.0")]
    PendingTimelockRevoked {},

    // Market and queue management
    #[event_version("1.0.0")]
    MarketCreated { market: AccountId },
    #[event_version("1.0.0")]
    SupplyCapRaiseSubmitted {
        market: AccountId,
        new_cap: U128,
        valid_at: u64,
    },
    #[event_version("1.0.0")]
    SupplyCapSet { market: AccountId, new_cap: U128 },
    #[event_version("1.0.0")]
    MarketEnabled { market: AccountId },
    #[event_version("1.0.0")]
    MarketAlreadyInWithdrawQueue { market: AccountId },
    #[event_version("1.0.0")]
    WithdrawQueueMarketAdded { market: AccountId },
    #[event_version("1.0.0")]
    MarketRemovalSubmitted {
        market: AccountId,
        removable_at: u64,
    },
    #[event_version("1.0.0")]
    MarketRemovalRevoked { market: AccountId },
    #[event_version("1.0.0")]
    WithdrawQueueUpdated { markets: Vec<AccountId> },

    // User flows
    #[event_version("1.0.0")]
    RedeemRequested {
        shares: U128,
        estimated_assets: U128,
    },

    // Allocation read/settlement diagnostics
    #[event_version("1.0.0")]
    AllocationPositionMissing {
        op_id: u64,
        index: u32,
        market: AccountId,
        attempted: U128,
        refunded: U128,
    },
    #[event_version("1.0.0")]
    AllocationPositionReadFailed {
        op_id: u64,
        index: u32,
        market: AccountId,
        attempted: U128,
        refunded: U128,
    },

    // Withdrawal read diagnostics
    #[event_version("1.0.0")]
    WithdrawalPositionMissing {
        op_id: u64,
        market: AccountId,
        index: u32,
        before: U128,
        need: U128,
    },
    #[event_version("1.0.0")]
    WithdrawalPositionReadFailed {
        op_id: u64,
        market: AccountId,
        index: u32,
        before: U128,
        need: U128,
    },

    // Payout and stop diagnostics
    #[event_version("1.0.0")]
    PayoutUnexpectedState {
        op_id: u64,
        receiver: AccountId,
        amount: U128,
    },
    #[event_version("1.0.0")]
    WithdrawalStopped {
        op_id: u64,
        index: u32,
        remaining: U128,
        collected: U128,
        reason: Option<String>,
    },
    #[event_version("1.0.0")]
    PayoutStopped {
        op_id: u64,
        receiver: AccountId,
        amount: U128,
        reason: Option<String>,
    },
    #[event_version("1.0.0")]
    OperationStoppedWhileIdle { reason: Option<String> },

    // Skim and deposits
    #[event_version("1.0.0")]
    SkimNoop {
        token: AccountId,
        recipient: AccountId,
    },
    #[event_version("1.0.0")]
    DepositRejectedWrongAsset { token: AccountId },
    #[event_version("1.0.0")]
    DepositRejectedZeroAmount { sender: AccountId },
}
