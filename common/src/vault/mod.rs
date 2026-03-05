use std::num::NonZeroU8;

use derive_more::{Display, From, Into};

use crate::{
    asset::{BorrowAsset, FungibleAsset},
    supply::SupplyPosition,
};
pub use external::*;
use near_sdk::{
    env,
    json_types::{U128, U64},
    near, require, AccountId, Gas, Promise, PromiseOrValue,
};
pub use templar_vault_kernel::types::{ActualIdx, ExpectedIdx, TimestampNs};
use templar_vault_kernel::{TimeGate, Wad};

pub use event::{
    AllocationPositionIssueKind, Event, PositionReportOutcome, QueueAction, QueueStatus, Reason,
    UnbrickPhase, WithdrawProgressPhase, WithdrawalAccountingKind,
};
pub use params::*;

pub mod errors;
pub mod event;
pub mod external;
pub mod gas;
pub mod lock;
pub mod params;
pub mod restrictions;
pub mod state;

pub use errors::Error;
pub use gas::*;
pub use lock::Locker;
pub use restrictions::*;
pub use state::*;

/// Broad import surface for vault consumers.
///
/// Prefer `use templar_common::vault::prelude::*;` at call sites that need
/// most vault types, constants, and wad/math helpers.
pub mod prelude {
    pub use super::event::{
        AllocationPositionIssueKind, Event, PositionReportOutcome, QueueAction, QueueStatus,
        Reason, UnbrickPhase, WithdrawProgressPhase, WithdrawalAccountingKind,
    };
    pub use super::external::*;
    pub use super::gas::*;
    pub use super::params::*;
    pub use super::restrictions::*;
    pub use super::state::*;
    pub use super::{
        require_at_least, storage_bytes_for_account_id, ActualIdx, AllocationDelta, AllocationPlan,
        AllocationWeights, CapGroupId, CapGroupRecord, CapGroupUpdate, CapGroupUpdateKey, Delta,
        DepositMsg, Error, EscrowSettlement, ExpectedIdx, Fee, FeeAccrualAnchor, Fees,
        IdleBalanceDelta, IdleResyncOutcome, Locker, MarketConfiguration, MarketId, PendingValue,
        PendingWithdrawal, RealAssetsReport, ResyncIdleReport, TimestampNs, VaultConfiguration,
    };
    pub use templar_vault_kernel::math::number::{Number, WIDE};
    pub use templar_vault_kernel::{
        compute_fee_shares, compute_fee_shares_from_assets, mul_div_ceil, mul_div_floor,
        mul_wad_floor, Wad, MAX_FEE_WAD, MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD,
    };
}

pub type AllocationWeights = Vec<(MarketId, U128)>;
pub type AllocationPlan = Vec<(MarketId, u128)>;

/// Report of real (live) total assets broken down by market, used for AUM refresh.
#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
pub struct RealAssetsReport {
    pub total_assets: U128,
    pub per_market: Vec<(MarketId, U128)>,
    /// Block timestamp in nanoseconds when this report was generated.
    pub refreshed_at: U64,
}

/// Outcome of an idle balance resynchronization attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub enum IdleResyncOutcome {
    /// Resync succeeded.
    Ok,
    /// ft_balance_of call failed.
    BalanceReadFailed,
    /// Vault not in expected state.
    UnexpectedState,
    /// Resync was a no-op (e.g. cooldown not elapsed).
    Ignored,
}

/// Detailed report from an idle balance resync operation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct ResyncIdleReport {
    pub outcome: IdleResyncOutcome,
    /// Balance snapshot before resync.
    pub before_idle: U128,
    /// Actual idle balance read from contract.
    pub actual_idle: U128,
    /// Balance snapshot after resync.
    pub after_idle: U128,
    /// Magnitude of increase adjustment.
    pub increased_by: U128,
    /// Magnitude of decrease adjustment.
    pub decreased_by: U128,
    /// Amount added to fee anchor to prevent donation fees.
    pub fee_anchor_bump: U128,
    /// Completion timestamp in nanoseconds.
    pub resynced_at_ns: U64,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, From, Into, Display)]
#[near(serializers = [borsh, json])]
#[display("{_0}")]
pub struct CapGroupId(pub String);

impl From<&str> for CapGroupId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// Configuration and accounting state for a cap group. Cap groups throttle correlated market exposure by enforcing both absolute and relative caps across member markets.
#[derive(Clone)]
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

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, From, Into, Display,
)]
#[near(serializers = [borsh, json])]
#[display("{_0}")]
pub struct MarketId(pub u32);

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

/// A fee configuration pairing a fee rate with its recipient.
#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct Fee<T> {
    pub fee: T,
    pub recipient: AccountId,
}

/// Complete fee configuration for the vault, including performance and management fees.
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
    /// Restrictions for this vault.
    pub restrictions: Option<Restrictions>,
    /// Optional cooldown (ns) between refresh_markets calls; defaults to contract constant if None.
    pub refresh_cooldown_ns: Option<U64>,
    /// Optional cooldown (ns) between idle_resync calls; defaults to contract constant if None.
    pub idle_resync_cooldown_ns: Option<U64>,
    /// Optional cooldown (ns) before a withdrawal can be executed; defaults to contract constant if None.
    pub withdrawal_cooldown_ns: Option<U64>,
}

/// A governance value pending timelock expiry. Stores the proposed value and the nanosecond timestamp after which it can be finalized.
#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub struct PendingValue<T: core::fmt::Debug> {
    pub value: T,
    /// Timestamp when this pending value can be finalized.
    pub valid_at_ns: TimestampNs,
}

impl<T: core::fmt::Debug> PendingValue<T> {
    pub fn verify(&self) {
        require!(
            TimeGate::from_ready_at(self.valid_at_ns).is_ready(env::block_timestamp()),
            "Timelock not elapsed yet"
        );
    }
}

/// A single market allocation delta specifying a market and an amount in underlying asset units.
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

/// Allocation instruction for a single market. `Supply` forwards idle assets to the market. `Withdraw` creates a supply-withdrawal request in the market (does not execute it).
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

/// Settlement breakdown for escrowed withdrawal shares. Invariant: `to_burn + refund == original escrow_shares`.
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

/// A queued withdrawal request with shares held in escrow. Fields use underlying asset units for `expected_assets` and nanosecond timestamps for `requested_at`.
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

/// Direction and magnitude of an idle balance change. Emits an `IdleBalanceUpdated` event when applied.
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

/// Anchor point for fee accrual: stores the total assets and nanosecond timestamp at which fees were last accrued.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, Default)]
pub struct FeeAccrualAnchor {
    pub total_assets: U128,
    pub timestamp_ns: U64,
}

#[cfg(test)]
mod tests;
