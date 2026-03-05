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
pub use templar_curator_primitives::{
    CapGroupId, CapGroupRecord, CapGroupUpdate, CapGroupUpdateKey,
};
pub use templar_vault_kernel::state::op_state::{
    AllocatingState, IdleState, OpState, PayoutState, RefreshingState, TargetId, WithdrawingState,
};
pub use templar_vault_kernel::types::{ActualIdx, ExpectedIdx, TimestampNs};
use templar_vault_kernel::Wad;

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

pub use errors::Error;
pub use gas::*;
pub use lock::Locker;
pub use restrictions::*;

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
    pub use super::{
        require_at_least, storage_bytes_for_account_id, ActualIdx, AllocationDelta, AllocationPlan,
        AllocationWeights, CapGroupId, CapGroupRecord, CapGroupUpdate, CapGroupUpdateKey, Delta,
        DepositMsg, Error, EscrowSettlement, ExpectedIdx, Fee, FeeAccrualAnchor, Fees,
        IdleBalanceDelta, IdleResyncOutcome, Locker, MarketConfiguration, MarketId,
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

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, From, Into, Display,
)]
#[near(serializers = [borsh, json])]
#[display("{_0}")]
pub struct MarketId(pub u32);

impl MarketId {
    #[must_use]
    pub fn as_u64(self) -> u64 {
        u64::from(self.0)
    }

    #[must_use]
    pub fn try_from_u64(value: u64) -> Option<Self> {
        u32::try_from(value).ok().map(Self)
    }
}

#[cfg(test)]
mod market_id_tests {
    use super::MarketId;

    #[test]
    fn try_from_u64_accepts_u32_range() {
        assert_eq!(
            MarketId::try_from_u64(u32::MAX as u64),
            Some(MarketId(u32::MAX))
        );
    }

    #[test]
    fn try_from_u64_rejects_out_of_range() {
        assert_eq!(MarketId::try_from_u64(u64::from(u32::MAX) + 1), None);
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
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Default)]
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
    /// Interpreted as an annualized WAD rate (1e18 = 100% per year). When set,
    /// fee accrual uses `min(cur_total_assets, last_total_assets * (1 + max_rate * dt / YEAR))`
    /// as the effective `cur_total_assets`.
    pub max_total_assets_growth_rate: Option<T>,
}

impl From<Fee<Wad>> for Fee<U128> {
    fn from(value: Fee<Wad>) -> Self {
        Self {
            fee: U128(u128::from(value.fee)),
            recipient: value.recipient,
        }
    }
}

impl From<Fees<Wad>> for Fees<U128> {
    fn from(value: Fees<Wad>) -> Self {
        Self {
            performance: value.performance.into(),
            management: value.management.into(),
            max_total_assets_growth_rate: value
                .max_total_assets_growth_rate
                .map(|rate| U128(u128::from(rate))),
        }
    }
}

impl From<Fee<U128>> for Fee<Wad> {
    fn from(value: Fee<U128>) -> Self {
        Self {
            fee: Wad::from(value.fee.0),
            recipient: value.recipient,
        }
    }
}

impl From<Fees<U128>> for Fees<Wad> {
    fn from(value: Fees<U128>) -> Self {
        Self {
            performance: value.performance.into(),
            management: value.management.into(),
            max_total_assets_growth_rate: value
                .max_total_assets_growth_rate
                .map(|rate| rate.0.into()),
        }
    }
}

/// Configuration for the setup of a metavault.
#[derive(Clone)]
#[near(serializers = [json, borsh])]
pub struct VaultConfiguration {
    /// The account that owns this vault.
    pub owner: AccountId,
    /// The account that can submit allocation plans. See [AllocationMode].
    pub curator: AccountId,
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
        let settlement = templar_vault_kernel::types::EscrowSettlement::from_escrow_and_burn(
            escrow_shares,
            burn_shares,
        );
        Self {
            to_burn: settlement.to_burn,
            refund: settlement.refund,
        }
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
mod tests {
    use super::{Fee, Fees};
    use near_sdk::json_types::U128;
    use near_sdk::AccountId;
    use templar_vault_kernel::Wad;

    #[test]
    fn fees_roundtrip_between_u128_and_wad_preserves_values() {
        let fees_u128 = Fees {
            performance: Fee {
                fee: U128(10),
                recipient: "perf.testnet"
                    .parse::<AccountId>()
                    .expect("valid account id"),
            },
            management: Fee {
                fee: U128(20),
                recipient: "mgmt.testnet"
                    .parse::<AccountId>()
                    .expect("valid account id"),
            },
            max_total_assets_growth_rate: Some(U128(30)),
        };

        let fees_wad: Fees<Wad> = fees_u128.clone().into();
        assert_eq!(u128::from(fees_wad.performance.fee), 10);
        assert_eq!(u128::from(fees_wad.management.fee), 20);
        assert_eq!(
            fees_wad
                .max_total_assets_growth_rate
                .map(u128::from)
                .expect("max rate must be present"),
            30
        );

        let roundtrip: Fees<U128> = fees_wad.into();
        assert_eq!(roundtrip.performance.fee.0, 10);
        assert_eq!(roundtrip.management.fee.0, 20);
        assert_eq!(
            roundtrip.max_total_assets_growth_rate.map(|v| v.0),
            Some(30)
        );
        assert_eq!(roundtrip.performance.recipient.as_str(), "perf.testnet");
        assert_eq!(roundtrip.management.recipient.as_str(), "mgmt.testnet");
    }
}
