use std::{collections::BTreeSet, num::NonZeroU8};

use crate::{
    asset::{BorrowAsset, FungibleAsset},
    supply::SupplyPosition,
    vault::wad::Wad,
};
pub use external::*;
use near_sdk::{
    env,
    json_types::{U128, U64},
    near, require, AccountId, AccountIdRef, Gas, Promise, PromiseOrValue,
};

pub use event::{
    AllocationPositionIssueKind, Event, PositionReportOutcome, QueueAction, QueueStatus, Reason,
    UnbrickPhase, WithdrawProgressPhase, WithdrawalAccountingKind,
};
pub use params::*;

pub mod event;
pub mod external;
pub mod params;
pub mod wad;

pub type TimestampNs = u64;

pub type ExpectedIdx = u32;
pub type ActualIdx = u32;
pub type AllocationWeights = Vec<(MarketId, U128)>;
pub type AllocationPlan = Vec<(MarketId, u128)>;

#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
pub struct RealAssetsReport {
    pub total_assets: U128,
    pub per_market: Vec<(MarketId, U128)>,
    pub refreshed_at: U64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub enum IdleResyncOutcome {
    Ok,
    BalanceReadFailed,
    UnexpectedState,
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct ResyncIdleReport {
    pub outcome: IdleResyncOutcome,
    pub before_idle: U128,
    pub actual_idle: U128,
    pub after_idle: U128,
    pub increased_by: U128,
    pub decreased_by: U128,
    pub fee_anchor_bump: U128,
    pub resynced_at_ns: U64,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [borsh, json])]
pub struct CapGroupId(pub String);

impl From<String> for CapGroupId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for CapGroupId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl core::fmt::Display for CapGroupId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub struct CapGroupRecord {
    /// Absolute cap in underlying units.
    pub cap: U128,
    /// Relative cap as a fraction of total vault assets (WAD, 1e24 = 100%).
    pub relative_cap: Wad,
    /// Sum of principals for all markets assigned to this cap group.
    pub principal: u128,
}

impl Default for CapGroupRecord {
    fn default() -> Self {
        Self {
            cap: U128(0),
            relative_cap: Wad::one(),
            principal: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json])]
pub enum CapGroupUpdate {
    /// Update the absolute cap (in underlying units).
    SetCap {
        cap_group: CapGroupId,
        new_cap: U128,
    },
    /// Update the relative cap (WAD, 1e24 = 100% of total assets).
    SetRelativeCap {
        cap_group: CapGroupId,
        new_relative_cap: U128,
    },
    /// Assign (or remove) a market to/from a cap group.
    SetMarketCapGroup {
        market: MarketId,
        cap_group: Option<CapGroupId>,
    },
}

/// Identifies a pending cap-group timelock action.
#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json])]
pub enum CapGroupUpdateKey {
    SetCap { cap_group: CapGroupId },
    SetRelativeCap { cap_group: CapGroupId },
    SetMarketCapGroup { market: MarketId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[near(serializers = [borsh, json])]
pub struct MarketId(pub u32);

impl From<u32> for MarketId {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl From<MarketId> for u32 {
    fn from(value: MarketId) -> Self {
        value.0
    }
}

impl core::fmt::Display for MarketId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

/// Parsed from the string parameter `msg` passed by `*_transfer_call` to
/// `*_on_transfer` calls.
#[near(serializers = [json])]
pub enum DepositMsg {
    /// Add the attached tokens to the sender's vault position.
    Supply,
}

/// Concrete configuration for a market.
#[derive(Clone, Default, Debug)]
#[near]
pub struct MarketConfiguration {
    /// Supply cap for this market (in underlying asset units)
    pub cap: U128,
    /// Whether market is enabled for deposits/withdrawals
    pub enabled: bool,
    /// Timestamp after which market can be removed (if pending removal)
    pub removable_at: TimestampNs,
    /// Cap group identifier used to throttle correlated exposure
    pub cap_group_id: Option<CapGroupId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct Fee<T> {
    pub fee: T,
    pub recipient: AccountId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct Fees<T> {
    pub performance: Fee<T>,
    pub management: Fee<T>,
    /// Optional cap on how fast `total_assets` is allowed to grow for fee accrual.
    ///
    /// Interpreted as an annualized WAD rate (1e24 = 100% per year). When set,
    /// fee accrual uses `min(cur_total_assets, last_total_assets * (1 + max_rate * dt / YEAR))`
    /// as the effective `cur_total_assets`.
    pub max_total_assets_growth_rate: Option<T>,
}

/// Configuration for the setup of a metavault.
#[derive(Clone)]
#[near(serializers = [json, borsh])]
pub struct VaultConfiguration {
    /// The account that owns this vault.
    pub owner: AccountId,
    /// The account that can submit allocation plans. See [AllocationMode].
    pub curator: AccountId,
    /// The safety role that can revoke pending governance actions.
    pub guardian: AccountId,
    /// The emergency role that can cancel withdrawals and trigger deallocations.
    pub sentinel: AccountId,
    /// The underlying asset for this vault.
    pub underlying_token: FungibleAsset<BorrowAsset>,
    /// The initial timelock for this vault used for modifying the configuration.
    pub initial_timelock_ns: U64,
    /// Fee configuration for performance and management fees as well as their recipients.
    pub fees: Fees<Wad>,
    /// The skim account that can unorphan any assets erroneously sent to this vault.
    pub skim_recipient: AccountId,
    /// The name of the share token.
    pub name: String,
    /// The symbol of the share token.
    pub symbol: String,
    /// The number of decimals for the share token, usually would be the same as the underlying asset.
    pub decimals: NonZeroU8,
    /// Restrictions for this market.
    pub restrictions: Option<Restrictions>,
    /// Optional cooldown (ns) between refresh_markets calls; defaults to contract constant if None.
    pub refresh_cooldown_ns: Option<U64>,
    /// Optional cooldown (ns) between idle_resync calls; defaults to contract constant if None.
    pub idle_resync_cooldown_ns: Option<U64>,
}

/// Restrictions that can be applied to the vault.
///
/// It should cover both Whitelist style functionality and Blacklist style functionality.
/// It should also enable Pausing
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restrictions {
    Paused,
    BlackList(BTreeSet<AccountId>),
    WhiteList(BTreeSet<AccountId>),
}

impl Restrictions {
    /// Check if the account is restricted, and if so, what is the reason
    pub fn is_restricted(&self, account_id: &AccountIdRef) -> Option<Restrictions> {
        match self {
            Restrictions::Paused => Some(Restrictions::Paused),
            Restrictions::BlackList(blacklist) => {
                if blacklist.contains(account_id) {
                    Some(Restrictions::BlackList(blacklist.clone()))
                } else {
                    None
                }
            }
            Restrictions::WhiteList(whitelist) => {
                if whitelist.contains(account_id) || account_id == env::current_account_id() {
                    None
                } else {
                    Some(Restrictions::WhiteList(whitelist.clone()))
                }
            }
        }
    }
}

// Add a 20% buffer to a gas estimate
#[must_use]
pub const fn buffer(size: u64) -> Gas {
    Gas::from_tgas((size * 6).div_ceil(5))
}

pub fn require_at_least(needed: Gas) {
    let gas = env::prepaid_gas();
    require!(
        gas >= needed,
        format!("Insufficient gas: {}, needed: {needed}", gas)
    );
}

#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub struct PendingValue<T: core::fmt::Debug> {
    pub value: T,
    // Timestamp when this pending value can be finalized
    pub valid_at_ns: TimestampNs,
}

impl<T: core::fmt::Debug> PendingValue<T> {
    pub fn verify(&self) {
        require!(
            near_sdk::env::block_timestamp() >= self.valid_at_ns,
            "Timelock not elapsed yet"
        );
    }
}

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

#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
pub struct Delta {
    pub market: MarketId,
    pub amount: U128,
}

impl Delta {
    pub fn new<T: Into<U128>>(market: MarketId, amount: T) -> Self {
        Delta {
            market,
            amount: amount.into(),
        }
    }
    pub fn validate(&self) {
        require!(self.amount.0 > 0, "Delta amount must be greater than zero");
    }
}

// + Supply: forward-supply idle assets to a market
// - Withdraw: ONLY creates a supply-withdrawal request in the market; does not execute it.
#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
pub enum AllocationDelta {
    Supply(Delta),
    Withdraw(Delta),
}

impl AsRef<Delta> for AllocationDelta {
    fn as_ref(&self) -> &Delta {
        match self {
            AllocationDelta::Supply(d) | AllocationDelta::Withdraw(d) => d,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EscrowSettlement {
    pub to_burn: u128,
    pub refund: u128,
}

impl EscrowSettlement {
    pub fn new(escrow_shares: u128, burn_shares: u128) -> Self {
        let to_burn = burn_shares.min(escrow_shares);
        let refund = escrow_shares.saturating_sub(to_burn);

        Self { to_burn, refund }
    }
}

impl From<EscrowSettlement> for (u128, u128) {
    fn from(tuple: EscrowSettlement) -> Self {
        (tuple.to_burn, tuple.refund)
    }
}

#[derive(Debug)]
#[near(serializers = [json])]
pub enum Error {
    // Invariant: Index drift or stale op_id results in a graceful stop
    IndexDrifted(ExpectedIdx, ActualIdx),
    // Invariant: Callback resolved a different market than expected.
    MarketDrifted {
        expected: MarketId,
        actual: MarketId,
    },
    // Invariant: Attempting to work on an unknown market.
    MissingMarket(MarketId),
    NotWithdrawing,
    NotAllocating,
    NotRefreshing,
    NotPayout,
    MarketTransferFailed,
    MissingSupplyPosition,
    PositionReadFailed,
    BalanceReadFailed,
    // Insufficient liquidity across all markets to satisfy withdrawal
    InsufficientLiquidity,
    ZeroAmount,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [borsh])]
pub struct PendingWithdrawal {
    pub owner: AccountId,
    pub receiver: AccountId,
    pub escrow_shares: u128,
    pub expected_assets: u128,
    pub requested_at: u64,
}

impl PendingWithdrawal {
    #[must_use]
    pub fn encoded_size() -> u64 {
        storage_bytes_for_account_id()
            + storage_bytes_for_account_id()
            + 16  // escrow_shares: u128
            + 16  // expected_assets: u128
            + 8 // requested_at: u64
    }
}

// Worst case size encoded for AccountId
#[must_use]
pub const fn storage_bytes_for_account_id() -> u64 {
    // 4 bytes for length prefix + worst case size encoded for AccountId
    4 + AccountId::MAX_LEN as u64
}

#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub enum IdleBalanceDelta {
    Increase(U128),
    Decrease(U128),
}

impl IdleBalanceDelta {
    pub fn apply(&self, balance: u128) -> u128 {
        let new = match self {
            IdleBalanceDelta::Increase(amount) => balance.saturating_add(amount.0),
            IdleBalanceDelta::Decrease(amount) => balance.saturating_sub(amount.0),
        };
        Event::IdleBalanceUpdated {
            prev: U128::from(balance),
            delta: self.clone(),
        }
        .emit();
        new
    }
}

#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, Default)]
pub struct FeeAccrualAnchor {
    pub total_assets: U128,
    pub timestamp_ns: U64,
}

#[derive(Default)]
#[near(serializers = [borsh, serde])]
pub struct Locker {
    to_lock: Vec<MarketId>,
}

impl Locker {
    pub fn lock(&mut self, market: MarketId) {
        if self.is_locked(market) {
            crate::panic_with_message("Market is locked");
        }
        Event::LockChange {
            is_locked: true,
            market,
        }
        .emit();
        self.to_lock.push(market);
    }

    pub fn unlock(&mut self, market: MarketId) {
        Event::LockChange {
            is_locked: false,
            market,
        }
        .emit();
        self.to_lock.retain(|&x| x != market);
    }

    /// Clears the lock status for all markets.
    /// This method should be used with caution as it will unlock all markets
    pub fn clear(&mut self) {
        self.to_lock.clear();
    }

    pub fn is_locked(&self, market: MarketId) -> bool {
        self.to_lock.contains(&market)
    }

    pub fn is_locked_all(&self) -> bool {
        !self.to_lock.is_empty()
    }
}
