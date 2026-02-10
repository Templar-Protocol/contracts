use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [borsh])]
/// No operation in-flight. The vault is ready to start a new allocation or withdrawal.
pub struct IdleState;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [borsh])]
/// Supplying idle underlying to markets according to a plan or queue.
///
/// Transitions:
/// - On completion of allocation: Withdrawing (to satisfy pending user requests) or Idle (if stopped).
/// - On stop/failure: Idle.
pub struct AllocatingState {
    /// Unique operation id used to correlate async callbacks and detect drift.
    pub op_id: u64,
    /// Zero-based position within the allocation plan/queue currently being processed.
    pub index: u32,
    /// Amount of underlying (in asset units) still to allocate during this operation.
    pub remaining: u128,
    /// Plan for allocation.
    pub plan: Vec<(MarketId, u128)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [borsh])]
/// Collecting liquidity from markets to satisfy a user withdrawal/redeem request.
///
/// Transitions:
/// - Advance within queue: Withdrawing (index increments) while collecting funds.
/// - When enough is collected to satisfy the request: Payout.
/// - If the op is stopped or cannot proceed and needs to refund: Idle (escrow_shares refunded).
pub struct WithdrawingState {
    /// Unique operation id used to correlate async callbacks and detect drift.
    pub op_id: u64,
    /// Zero-based position within the withdraw queue currently being processed.
    pub index: u32,
    /// Remaining assets that must still be collected to satisfy the request.
    pub remaining: u128,
    /// Assets already collected and held as idle_balance pending payout.
    pub collected: u128,
    /// Account that should receive the assets during payout.
    pub receiver: AccountId,
    /// The owner whose shares are being redeemed.
    pub owner: AccountId,
    /// Shares locked in escrow for this request.
    /// - Refunded on stop/failure.
    /// - On payout success, a portion is burned (see burn_shares) and any remainder is refunded.
    pub escrow_shares: u128,
}

/// Read-only refresh of market principals to update stored AUM.
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct RefreshingState {
    /// Unique operation id used to correlate async callbacks and detect drift.
    pub op_id: u64,
    /// Zero-based position within the refresh plan currently being processed.
    pub index: u32,
    /// Markets to refresh.
    pub plan: Vec<MarketId>,
}

/// Final step that transfers assets to the receiver and settles the share escrow.
///
/// Transitions:
/// - On success or failure: Idle.
///
/// Invariant hooks:
/// - idle_balance decreases only on payout success by `amount`.
/// - On success, `burn_shares` are burned from `escrow_shares`; any remainder is refunded.
/// - On failure, all `escrow_shares` are refunded.
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [borsh])]
pub struct PayoutState {
    /// Unique operation id used to correlate async callbacks and detect drift.
    pub op_id: u64,
    /// Receiver of the asset payout.
    pub receiver: AccountId,
    /// Amount of assets to transfer out from idle_balance.
    pub amount: u128,
    /// The owner whose shares were escrowed for this payout.
    pub owner: AccountId,
    /// Total shares currently held in escrow for this operation.
    pub escrow_shares: u128,
    /// Portion of `escrow_shares` that will be burned on successful payout.
    pub burn_shares: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [borsh])]
/// Operation state machine for asynchronous allocation, withdrawal, and payout flows.
///
/// State machine:
/// - Allocating -> Withdrawing (or Idle via stop)
/// - Withdrawing -> Withdrawing (advance) | Payout | Idle (refund)
/// - Payout -> Idle (success or failure)
///
/// Invariants:
/// - idle_balance increases only when funds are received and decreases only on payout success.
/// - escrow_shares are refunded on stop/failure or partially burned/refunded on payout success.
pub enum OpState {
    /// No operation in-flight. The vault is ready to start a new allocation or withdrawal.
    Idle,

    /// Supplying idle underlying to markets according to a plan or queue.
    ///
    /// Transitions:
    /// - On completion of allocation: Withdrawing (to satisfy pending user requests) or Idle (if stopped).
    /// - On stop/failure: Idle.
    Allocating(AllocatingState),

    /// Collecting liquidity from markets to satisfy a user withdrawal/redeem request.
    ///
    /// Transitions:
    /// - Advance within queue: Withdrawing (index increments) while collecting funds.
    /// - When enough is collected to satisfy the request: Payout.
    /// - If the op is stopped or cannot proceed and needs to refund: Idle (escrow_shares refunded).
    Withdrawing(WithdrawingState),

    /// Read-only refresh of market principals to update stored AUM.
    Refreshing(RefreshingState),

    /// Final step that transfers assets to the receiver and settles the share escrow.
    ///
    /// Transitions:
    /// - On success or failure: Idle.
    ///
    /// Invariant hooks:
    /// - idle_balance decreases only on payout success by `amount`.
    /// - On success, `burn_shares` are burned from `escrow_shares`; any remainder is refunded.
    /// - On failure, all `escrow_shares` are refunded.
    Payout(PayoutState),
}

impl From<IdleState> for OpState {
    fn from(_: IdleState) -> Self {
        OpState::Idle
    }
}

impl From<AllocatingState> for OpState {
    fn from(s: AllocatingState) -> Self {
        OpState::Allocating(s)
    }
}

impl From<WithdrawingState> for OpState {
    fn from(s: WithdrawingState) -> Self {
        OpState::Withdrawing(s)
    }
}

impl From<RefreshingState> for OpState {
    fn from(s: RefreshingState) -> Self {
        OpState::Refreshing(s)
    }
}

impl From<PayoutState> for OpState {
    fn from(s: PayoutState) -> Self {
        OpState::Payout(s)
    }
}

impl OpState {
    #[inline]
    #[must_use]
    pub const fn as_idle(&self) -> Option<&IdleState> {
        match self {
            OpState::Idle => Some(&IdleState),
            _ => None,
        }
    }

    #[inline]
    #[must_use]
    pub const fn as_allocating(&self) -> Option<&AllocatingState> {
        match self {
            OpState::Allocating(s) => Some(s),
            _ => None,
        }
    }

    #[inline]
    #[must_use]
    pub const fn as_withdrawing(&self) -> Option<&WithdrawingState> {
        match self {
            OpState::Withdrawing(s) => Some(s),
            _ => None,
        }
    }

    #[inline]
    #[must_use]
    pub const fn as_refreshing(&self) -> Option<&RefreshingState> {
        match self {
            OpState::Refreshing(s) => Some(s),
            _ => None,
        }
    }

    #[inline]
    #[must_use]
    pub const fn as_payout(&self) -> Option<&PayoutState> {
        match self {
            OpState::Payout(s) => Some(s),
            _ => None,
        }
    }
}
