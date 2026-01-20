use super::*;

#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
pub enum Reason {
    NoRoom,
    ZeroTarget,
    RouteExhaustedNoFunds,
    Other(String),
}

#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
pub enum QueueAction {
    Dequeued,
    Parked,
}

#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
pub enum QueueStatus {
    NextFound,
    Empty,
}

#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
pub enum WithdrawProgressPhase {
    ExecutionStarted,
    SkippedDust,
    CoveredByIdle,
    ExecutionRequired,
}

#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
pub enum AllocationPositionIssueKind {
    Missing,
    ReadFailed,
}

#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
pub enum WithdrawalAccountingKind {
    InflowMismatch,
    OverpayCredited,
}

#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
pub enum PositionReportOutcome {
    Ok,
    Missing,
    ReadFailed,
}

#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
pub enum UnbrickPhase {
    Withdrawing,
    Payout,
}

#[near(event_json(standard = "templar-vault"))]
pub enum Event {
    #[event_version("1.0.0")]
    IdleBalanceUpdated { prev: U128, delta: IdleBalanceDelta },
    #[event_version("1.0.0")]
    PerformanceFeeAccrued { recipient: AccountId, shares: U128 },
    #[event_version("1.0.0")]
    PerformanceFeeMintFailed { error: String },
    #[event_version("1.0.0")]
    ManagementFeeAccrued { recipient: AccountId, shares: U128 },
    #[event_version("1.0.0")]
    ManagementFeeMintFailed { error: String },
    #[event_version("1.0.0")]
    ManagementFeeSet { fee: U128 },
    #[event_version("1.0.0")]
    ManagementFeeRecipientSet { account: AccountId },
    #[event_version("1.0.0")]
    LockChange { is_locked: bool, market: MarketId },

    #[event_version("1.0.0")]
    AllocationPlanSet {
        op_id: U64,
        total: U128,
        plan: Vec<(MarketId, U128)>,
    },
    #[event_version("1.0.0")]
    AllocationStarted { op_id: U64, remaining: U128 },
    #[event_version("1.0.0")]
    AllocationStepPlan {
        op_id: U64,
        index: u32,
        market: MarketId,
        target: U128,
        room: U128,
        to_supply: U128,
        remaining_before: U128,
        planned: bool,
        reason: Option<Reason>,
    },
    #[event_version("1.0.0")]
    AllocationTransferFailed {
        op_id: U64,
        index: u32,
        market: MarketId,
        attempted: U128,
    },
    #[event_version("1.0.0")]
    AllocationStepSettled {
        op_id: U64,
        index: u32,
        market: MarketId,
        before: U128,
        new_principal: U128,
        accepted: U128,
        attempted: U128,
        refunded: U128,
        remaining_after: U128,
    },
    #[event_version("1.0.0")]
    AllocationCompleted { op_id: u64 },
    #[event_version("1.0.0")]
    AllocationStopped {
        op_id: U64,
        index: u32,
        remaining: U128,
        reason: Option<Reason>,
    },

    #[event_version("1.0.0")]
    RefreshStarted {
        op_id: U64,
        markets: Vec<MarketId>,
        caller: AccountId,
    },
    #[event_version("1.0.0")]
    RefreshCompleted {
        op_id: U64,
        markets: Vec<MarketId>,
        total_assets: U128,
        refreshed_at: U64,
    },

    #[event_version("1.0.0")]
    CuratorSet { account: AccountId },
    #[event_version("1.0.0")]
    GuardianSet { account: AccountId },
    #[event_version("1.0.0")]
    SentinelSet { account: AccountId },
    #[event_version("1.0.0")]
    AllocatorRoleSet { account: AccountId, allowed: bool },
    #[event_version("1.0.0")]
    SkimRecipientSet { account: AccountId },
    #[event_version("1.0.0")]
    FeeRecipientSet { account: AccountId },
    #[event_version("1.0.0")]
    PerformanceFeeSet { fee: U128 },
    #[event_version("1.0.0")]
    MaxTotalAssetsGrowthRateSet { max_rate: Option<U128> },
    #[event_version("1.0.0")]
    RestrictionsSet { restrictions: Option<Restrictions> },
    #[event_version("1.0.0")]
    TimelockSet { seconds: U64 },
    #[event_version("1.0.0")]
    TimelockChangeSubmitted { valid_at_ns: U64 },
    #[event_version("1.0.0")]
    FeesChangeSubmitted { fees: Fees<U128>, valid_at_ns: u64 },
    #[event_version("1.0.0")]
    FeesChangeRevoked,
    #[event_version("1.0.0")]
    RestrictionsChangeSubmitted {
        restrictions: Option<Restrictions>,
        valid_at_ns: u64,
    },
    #[event_version("1.0.0")]
    RestrictionsChangeRevoked,
    #[event_version("1.0.0")]
    PendingTimelockRevoked,

    #[event_version("1.0.0")]
    Abdicated { method_name: String },

    #[event_version("1.0.0")]
    MarketCreated { market: MarketId },
    #[event_version("1.0.0")]
    MarketEnabled { market: MarketId },
    #[event_version("1.0.0")]
    MarketRemovalSubmitted { market: MarketId, removable_at: U64 },
    #[event_version("1.0.0")]
    MarketRemovalRevoked { market: MarketId },
    #[event_version("1.0.0")]
    SupplyCapRaiseSubmitted {
        market: MarketId,
        new_cap: U128,
        valid_at_ns: u64,
    },
    #[event_version("1.0.0")]
    SupplyCapRaiseRevoked { market: MarketId },
    #[event_version("1.0.0")]
    SupplyCapSet { market: MarketId, new_cap: U128 },
    #[event_version("1.0.0")]
    CapGroupRaiseSubmitted {
        cap_group: CapGroupId,
        new_cap: U128,
        valid_at_ns: u64,
    },
    #[event_version("1.0.0")]
    CapGroupRaiseRevoked { cap_group: CapGroupId },
    #[event_version("1.0.0")]
    CapGroupSet {
        cap_group: CapGroupId,
        new_cap: U128,
    },
    #[event_version("1.0.0")]
    CapGroupRelativeCapRaiseSubmitted {
        cap_group: CapGroupId,
        new_relative_cap: U128,
        valid_at_ns: u64,
    },
    #[event_version("1.0.0")]
    CapGroupRelativeCapRaiseRevoked { cap_group: CapGroupId },
    #[event_version("1.0.0")]
    CapGroupRelativeCapSet {
        cap_group: CapGroupId,
        new_relative_cap: U128,
    },
    #[event_version("1.0.0")]
    CapGroupPrincipalUpdated {
        cap_group: CapGroupId,
        principal: U128,
    },
    #[event_version("1.0.0")]
    CapGroupMembershipSet {
        market: MarketId,
        cap_group: Option<CapGroupId>,
    },
    #[event_version("1.0.0")]
    CapGroupMembershipRevoked { market: MarketId },

    #[event_version("1.0.0")]
    WithdrawQueueUpdate { action: QueueAction, id: U64 },
    #[event_version("1.0.0")]
    WithdrawParkedDetail {
        id: U64,
        failed_route: Vec<MarketId>,
        reason: Reason,
    },
    #[event_version("1.0.0")]
    WithdrawQueueStatus {
        status: QueueStatus,
        id: Option<U64>,
    },

    #[event_version("1.0.0")]
    RebalanceWithdrawCompleted { op_id: U64, market: MarketId },
    #[event_version("1.0.0")]
    RebalanceWithdrawStopped {
        op_id: U64,
        market: MarketId,
        reason: Option<Reason>,
    },

    #[event_version("1.0.0")]
    RedeemRequested {
        shares: U128,
        estimated_assets: U128,
    },
    #[event_version("1.0.0")]
    WithdrawalQueued {
        id: U64,
        owner: AccountId,
        receiver: AccountId,
        escrow_shares: U128,
        expected_assets: U128,
        requested_at: U64,
    },
    #[event_version("1.0.0")]
    WithdrawPreview { shares: U128, receiver: AccountId },
    #[event_version("1.0.0")]
    WithdrawProgress {
        phase: WithdrawProgressPhase,
        op_id: Option<U64>,
        id: Option<U64>,
        market: Option<MarketId>,
        owner: Option<AccountId>,
        receiver: Option<AccountId>,
        escrow_shares: Option<U128>,
        expected_assets: Option<U128>,
        requested_at: Option<U64>,
    },
    #[event_version("1.0.0")]
    SupplyWithdrawRequestCreated { market: MarketId, amount: U128 },
    #[event_version("1.0.0")]
    WithdrawRequestCreated { market: MarketId, amount: U128 },
    #[event_version("1.0.0")]
    #[event_version("1.0.0")]
    AllocationPositionIssue {
        op_id: U64,
        index: u32,
        market: MarketId,
        attempted: U128,
        accepted: U128,
        kind: AllocationPositionIssueKind,
    },

    #[event_version("1.0.0")]
    CreateWithdrawalFailed {
        op_id: U64,
        market: MarketId,
        need: U128,
    },

    #[event_version("1.0.0")]
    WithdrawalAccounting {
        kind: WithdrawalAccountingKind,
        op_id: U64,
        market: MarketId,
        delta: Option<U128>,
        inflow: Option<U128>,
        extra: Option<U128>,
    },

    #[event_version("1.0.0")]
    PayoutUnexpectedState {
        op_id: U64,
        receiver: AccountId,
        amount: U128,
    },
    #[event_version("1.0.0")]
    WithdrawalStopped {
        op_id: U64,
        index: u32,
        remaining: U128,
        collected: U128,
        reason: Option<Reason>,
    },
    #[event_version("1.0.0")]
    PayoutStopped {
        op_id: U64,
        receiver: AccountId,
        amount: U128,
        reason: Option<Reason>,
    },
    #[event_version("1.0.0")]
    OperationStoppedWhileIdle { reason: Option<Reason> },
    #[event_version("1.0.0")]
    UnbrickInvoked {
        phase: UnbrickPhase,
        op_id: Option<U64>,
        id: Option<U64>,
    },

    #[event_version("1.0.0")]
    WithdrawPositionReport {
        outcome: PositionReportOutcome,
        op_id: U64,
        market: MarketId,
        position: Option<SupplyPosition>,
        before: Option<U128>,
    },

    #[event_version("1.0.0")]
    VaultBalance { amount: U128 },

    #[event_version("1.0.0")]
    IdleResyncStarted {
        op_id: U64,
        caller: AccountId,
        before_idle: U128,
        started_at_ns: U64,
    },
    #[event_version("1.0.0")]
    IdleResyncCompleted {
        op_id: U64,
        caller: AccountId,
        before_idle: U128,
        actual_idle: U128,
        after_idle: U128,
        increased_by: U128,
        decreased_by: U128,
        fee_anchor_bump: U128,
        finished_at_ns: U64,
    },
    #[event_version("1.0.0")]
    IdleResyncStopped {
        op_id: U64,
        caller: AccountId,
        before_idle: U128,
        reason: Option<Reason>,
        finished_at_ns: U64,
    },
    #[event_version("1.0.0")]
    IdleResyncCallbackIgnored { op_id: U64, reason: Reason },
}
