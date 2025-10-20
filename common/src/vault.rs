use std::num::NonZeroU8;

use near_sdk::{
    env,
    json_types::{U128, U64},
    near, require, AccountId, Gas, Promise, PromiseOrValue,
};

use crate::asset::{BorrowAsset, FungibleAsset};

pub type TimestampNs = u64;

pub const MIN_TIMELOCK_NS: u64 = 0;
pub const MAX_TIMELOCK_NS: u64 = 30 * 86_400_000_000_000; // 30 days
pub const MAX_QUEUE_LEN: usize = 64;

pub type ExpectedIdx = u32;
pub type ActualIdx = u32;
pub type AllocationWeights = Vec<(AccountId, U128)>;
pub type AllocationPlan = Vec<(AccountId, u128)>;

#[derive(Clone, Debug, Default)]
#[near(serializers = [json, borsh])]
pub enum AllocationMode {
    /// When eager makes sense
    ///
    ///  • Retail/auto-pilot vaults: users expect deposits to “start earning” immediately without an active allocator.
    ///  • Small/simple vaults: stable caps/ordering, few markets; operational simplicity > fine-grained control.
    ///  • Integrations that assume quick deployment of idle assets.
    ///
    /// Risks/trade-offs of eager
    ///
    ///  • Gas burden on depositors: ft_transfer_call into your vault must carry enough gas for multi-hop allocation.
    ///    Under-provisioned gas leads to partial allocations and extra callbacks.
    ///  • Timing control: depositors implicitly decide when allocation runs, which can fight the allocator’s planned rebalancing
    ///    cadence.
    ///  • Thrashing: many small deposits can trigger many allocation passes.
    ///  • Current code is “eager-ish but incomplete”: it only auto-starts when Idle, and does not auto-restart after the op. Deposits
    ///    that arrive during an allocation stay idle until someone triggers another pass.
    ///
    /// Behaviour
    /// • On deposit: if Idle and idle_balance ≥ min_batch, start_allocation(idle_balance).
    /// • Eager allocation can still honor a per-op plan if one is set (plan wins); otherwise fall back to supply_queue order.
    Eager { min_batch: U128 },
    #[default]
    Lazy,
}

/// Parsed from the string parameter `msg` passed by `*_transfer_call` to
/// `*_on_transfer` calls.
#[near(serializers = [json])]
pub enum DepositMsg {
    /// Add the attached tokens to the sender's vault position.
    Supply,
}

/// Confrete configuration for a market.
#[derive(Clone, Default)]
#[near]
pub struct MarketConfiguration {
    /// Supply cap for this market (in underlying asset units)
    pub cap: U128,
    /// Whether market is enabled for deposits/withdrawals
    pub enabled: bool,
    /// Timestamp (ns) after which market can be removed (if pending removal)
    pub removable_at: TimestampNs,
}

impl MarketConfiguration {
    /// Size of the market configuration in borsh encoded bytes.
    pub const fn encoded_size() -> usize {
        16 + 1 + 8
    }
}

/// Configuration for the setup of a metavault.
#[derive(Clone)]
#[near(serializers = [json, borsh])]
pub struct VaultConfiguration {
    /// The allocation mode for this vault.
    pub mode: AllocationMode,
    /// The account that owns this vault.
    pub owner: AccountId,
    /// The account that can submit allocation plans. See [AllocationMode].
    pub curator: AccountId,
    /// The account that can set guardianship. See [AllocationMode].
    pub guardian: AccountId,
    /// The underlying asset for this vault.
    pub underlying_token: FungibleAsset<BorrowAsset>,
    /// The initial timelock for this vault used for modifying the configuration.
    pub initial_timelock_sec: u32,
    /// The account that receives fees for this vault.
    pub fee_recipient: AccountId,
    /// The skim account that can unorphan any assets erroneously sent to this vault.
    pub skim_recipient: AccountId,
    /// The name of the share token.
    pub name: String,
    /// The symbol of the share token.
    pub symbol: String,
    /// The number of decimals for the share token, usually would be the same as the underlying asset.
    pub decimals: NonZeroU8,
}

#[near_sdk::ext_contract(ext_vault)]
pub trait VaultExt {
    // Role and admin
    fn set_curator(account: AccountId);
    fn set_is_allocator(account: AccountId, allowed: bool);
    fn submit_guardian(new_g: AccountId);
    fn accept_guardian();
    fn revoke_pending_guardian();
    fn set_skim_recipient(account: AccountId);
    fn set_fee_recipient(account: AccountId);
    fn set_performance_fee(fee: U128);
    fn submit_timelock(new_timelock_secs: u32);
    fn accept_timelock();
    fn revoke_pending_timelock();

    // Market config and queues
    fn submit_cap(market: AccountId, new_cap: U128);
    fn accept_cap(market: AccountId);
    fn revoke_pending_cap(market: AccountId);
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
    fn get_total_supply() -> U128;
    fn get_max_deposit() -> U128;
    fn convert_to_shares(assets: U128) -> U128;
    fn convert_to_assets(shares: U128) -> U128;
    fn preview_deposit(assets: U128) -> U128;
    fn preview_mint(shares: U128) -> U128;
    fn preview_withdraw(assets: U128) -> U128;
    fn preview_redeem(shares: U128) -> U128;
}

// FIXME: move to market
pub const GET_SUPPLY_POSITION_GAS: Gas = Gas::from_tgas(4);
// FIXME: move to market
pub const CREATE_WITHDRAW_REQ_GAS: Gas = Gas::from_tgas(10);
// FIXME: move to market
pub const EXECUTE_WITHDRAW_REQ_GAS: Gas = Gas::from_tgas(10);

pub const AFTER_SUPPLY_ENSURE_GAS: Gas = Gas::from_tgas(30);
pub const AFTER_SUPPLY_POSITION_CHECK_GAS: Gas = Gas::from_tgas(10);
pub const AFTER_CREATE_WITHDRAW_REQ_GAS: Gas = Gas::from_tgas(20);
pub const AFTER_EXEC_WITHDRAW_READ_GAS: Gas = Gas::from_tgas(10);
pub const AFTER_SEND_TO_USER_GAS: Gas = Gas::from_tgas(5);

// Add a 20% buffer to a gas estimate
pub const fn buffer(size: usize) -> Gas {
    // 20% buffer => multiply by 6/5 (≈1.2x)
    Gas::from_tgas(size as u64)
        .saturating_mul(6)
        .saturating_div(5)
}

// NOTE: these are taken after running the contract with the gas report
pub const SUPPLY_GAS: Gas = buffer(8);
pub const ALLOCATE_GAS: Gas = buffer(21);
pub const WITHDRAW_GAS: Gas = buffer(4);
pub const EXECUTE_WITHDRAW_GAS: Gas = buffer(9);
pub const SUBMIT_CAP_GAS: Gas = buffer(4);

pub fn require_at_least(needed: Gas) {
    let gas = env::prepaid_gas();
    require!(
        gas >= needed,
        format!("Insufficient gas: {}, needed: {needed}", gas)
    );
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
        accepted: U128,
    ) -> bool;
    fn after_create_withdraw_req(&mut self, op_id: u64, market_index: u32, need: U128) -> bool;
    fn after_exec_withdraw_req(&mut self, op_id: u64, market_index: u32, need: U128) -> bool;
    fn after_exec_withdraw_read(&mut self, op_id: u64, market_index: u32, before: U128, need: U128);
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

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [borsh])]
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
        burn_shares: u128,
    },
}

#[derive(Debug)]
#[near(serializers = [json])]
pub enum Error {
    // Invariant: Index drift or stale op_id results in a graceful stop
    IndexDrifted(ExpectedIdx, ActualIdx),
    // Invariant: Attempting to work on a market that is missing from the withdraw queue
    MissingMarket(u32),
    NotWithdrawing,
    NotAllocating,
    MarketTransferFailed,
    MissingSupplyPosition,
    PositionReadFailed,
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
    pub const fn encoded_size() -> usize {
        storage_bytes_for_account_id() as usize
            + storage_bytes_for_account_id() as usize
            + 16  // escrow_shares: u128
            + 16  // expected_assets: u128
            + 8 // requested_at: u64
    }
}

// Worst case size encoded for AccountId
pub const fn storage_bytes_for_account_id() -> u64 {
    // 4 bytes for length prefix + worst case size encoded for AccountId
    4 + AccountId::MAX_LEN as u64
}

#[near(event_json(standard = "templar-vault"))]
pub enum Event {
    #[event_version("1.0.0")]
    MintedShares { amount: U128, receiver: AccountId },
    #[event_version("1.0.0")]
    AllocationStarted { op_id: U64, remaining: U128 },

    // Allocation lifecycle (plan/request)
    #[event_version("1.0.0")]
    AllocationRequestedQueue { op_id: U64, total: U128 },
    #[event_version("1.0.0")]
    AllocationPlanSet {
        op_id: U64,
        total: U128,
        plan: Vec<(AccountId, U128)>,
    },

    // Per-step planning and outcomes
    #[event_version("1.0.0")]
    AllocationStepPlanned {
        op_id: U64,
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
        op_id: U64,
        index: u32,
        market: AccountId,
        reason: String,
        remaining: U128,
    },
    #[event_version("1.0.0")]
    AllocationTransferFailed {
        op_id: U64,
        index: u32,
        market: AccountId,
        attempted: U128,
    },
    #[event_version("1.0.0")]
    AllocationStepSettled {
        op_id: U64,
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
        op_id: U64,
        index: u32,
        remaining: U128,
        reason: Option<String>,
    },

    // Eager
    #[event_version("1.0.0")]
    AllocationEagerTriggered {
        op_id: U64,
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
    TimelockChangeSubmitted { new_seconds: u32, valid_at: U64 },
    #[event_version("1.0.0")]
    PendingTimelockRevoked,

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
        removable_at: U64,
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
    #[event_version("1.0.0")]
    WithdrawalQueued {
        id: U64,
        owner: AccountId,
        receiver: AccountId,
        escrow_shares: U128,
        expected_assets: U128,
        requested_at: U64,
    },

    // Allocation read/settlement diagnostics
    #[event_version("1.0.0")]
    AllocationPositionMissing {
        op_id: U64,
        index: u32,
        market: AccountId,
        attempted: U128,
        accepted: U128,
    },
    #[event_version("1.0.0")]
    AllocationPositionReadFailed {
        op_id: U64,
        index: u32,
        market: AccountId,
        attempted: U128,
        accepted: U128,
    },

    // Withdrawal read diagnostics
    #[event_version("1.0.0")]
    WithdrawalPositionMissing {
        op_id: U64,
        market: AccountId,
        index: u32,
        before: U128,
        need: U128,
    },
    #[event_version("1.0.0")]
    WithdrawalPositionReadFailed {
        op_id: U64,
        market: AccountId,
        index: u32,
        before: U128,
        need: U128,
    },

    // Payout and stop diagnostics
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
        reason: Option<String>,
    },
    #[event_version("1.0.0")]
    PayoutStopped {
        op_id: U64,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    const _: [(); MarketConfiguration::encoded_size()] = [(); 25];
    const _EXPECTED_FROM_TYPES: usize =
        core::mem::size_of::<u128>() + core::mem::size_of::<bool>() + core::mem::size_of::<u64>();
    const _: [(); MarketConfiguration::encoded_size()] = [(); _EXPECTED_FROM_TYPES];

    #[test]
    fn encoded_size_is_25() {
        assert_eq!(MarketConfiguration::encoded_size(), 25);
    }

    #[test]
    fn encoded_size_market_matches_field_sizes() {
        assert_eq!(
            MarketConfiguration::encoded_size(),
            borsh::to_vec(&MarketConfiguration::default())
                .unwrap()
                .len(),
        );
    }

    #[test]
    fn encoded_size_pending_withdrawal_matches_field_sizes() {
        // let 64 byte account id
        let s = "abc1abc2abc3abc4abc5abc6abc7abc8abc9abc0abc1abc2abc3abc4abc5abc6";
        assert_eq!(s.len(), 64);
        let account = AccountId::from_str(s).unwrap();
        assert_eq!(account.len(), 64);
        assert_eq!(
            borsh::to_vec(&PendingWithdrawal {
                owner: account.clone(),
                receiver: account.clone(),
                escrow_shares: 3,
                expected_assets: 4,
                requested_at: 5
            })
            .unwrap()
            .len(),
            PendingWithdrawal::encoded_size()
        );
    }
}
