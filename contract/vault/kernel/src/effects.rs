extern crate alloc;

use alloc::vec::Vec;

use crate::types::Address;
#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use derive_more::IsVariant;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Side effects produced by kernel state transitions.
///
/// The executor layer interprets these effects by interacting with the
/// underlying blockchain (token operations, external calls, etc.).
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq, IsVariant)]
pub enum KernelEffect {
    /// Mint new share tokens to an owner.
    MintShares { owner: Address, shares: u128 },
    /// Burn share tokens from an owner.
    BurnShares { owner: Address, shares: u128 },
    /// Transfer shares between addresses.
    TransferShares {
        from: Address,
        to: Address,
        shares: u128,
    },
    /// Transfer underlying assets to a recipient.
    TransferAssets { to: Address, amount: u128 },
    /// Transfer underlying assets between two addresses.
    TransferAssetsFrom {
        from: Address,
        to: Address,
        amount: u128,
    },
    /// Make an external cross-contract call (NEAR only).
    #[cfg(feature = "near")]
    ExternalCall {
        target: Address,
        selector: u32,
        args: Vec<u8>,
        attached_value: u128,
        callback: Option<KernelCallback>,
    },
    /// Charge storage costs to a payer (NEAR only).
    #[cfg(feature = "near")]
    ChargeStorage { payer: Address, bytes: u64 },
    /// Emit an event for indexers and clients.
    EmitEvent { event: KernelEvent },
}

/// Callback identifiers for async cross-contract calls.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq, IsVariant)]
pub enum KernelCallback {
    /// Callback for allocation step completion.
    AllocationStep,
    /// Callback for withdrawal step completion.
    WithdrawalStep,
    /// Callback for refresh step completion.
    RefreshStep,
    /// Callback for payout transfer completion.
    PayoutTransfer,
}

/// Events emitted by kernel transitions for indexing and observability.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq, IsVariant)]
pub enum KernelEvent {
    /// Allocation operation started.
    AllocationStarted {
        op_id: u64,
        total: u128,
        plan_len: u32,
    },
    /// Allocation step failed and allocation aborted.
    AllocationStepFailed {
        op_id: u64,
        index: u32,
        remaining: u128,
        /// Amount successfully allocated before failure (original total - remaining).
        /// Caller uses this to restore idle_assets correctly.
        total_allocated: u128,
    },
    /// Allocation completed (either returns to Idle or proceeds to withdrawal).
    AllocationCompleted { op_id: u64, has_withdrawal: bool },
    /// Withdrawal operation started.
    WithdrawalStarted {
        op_id: u64,
        amount: u128,
        escrow_shares: u128,
        owner: Address,
        receiver: Address,
    },
    /// Withdrawal collected enough to proceed to payout.
    WithdrawalCollected {
        op_id: u64,
        burn_shares: u128,
        collected: u128,
    },
    /// Withdrawal stopped and escrow refunded.
    WithdrawalStopped { op_id: u64, escrow_shares: u128 },
    /// Refresh operation started.
    RefreshStarted { op_id: u64, plan_len: u32 },
    /// Refresh operation completed.
    RefreshCompleted { op_id: u64 },
    /// Payout completed (success or failure).
    PayoutCompleted {
        op_id: u64,
        success: bool,
        burn_shares: u128,
        refund_shares: u128,
        amount: u128,
    },
    /// Deposit processed and shares minted.
    DepositProcessed {
        owner: Address,
        receiver: Address,
        assets_in: u128,
        shares_out: u128,
    },
    /// Withdrawal requested and enqueued.
    WithdrawalRequested {
        id: u64,
        owner: Address,
        receiver: Address,
        shares: u128,
        expected_assets: u128,
    },
    /// External assets synchronized for an operation.
    ExternalAssetsSynced {
        op_id: u64,
        new_external_assets: u128,
        total_assets: u128,
    },
    /// Fees refreshed for the vault.
    FeesRefreshed { now_ns: u64, total_assets: u128 },
    /// Pause state updated.
    PauseUpdated { paused: bool },
    /// Emergency reset forced the vault back to Idle.
    EmergencyResetCompleted { op_id: u64, from_state: u32 },
}

impl From<KernelEvent> for KernelEffect {
    fn from(event: KernelEvent) -> Self {
        Self::EmitEvent { event }
    }
}

impl KernelEffect {
    /// Collect all addresses that must be resolved before this effect can be applied.
    pub fn required_addresses(&self) -> Vec<Address> {
        match self {
            KernelEffect::MintShares { owner, .. } => alloc::vec![*owner],
            KernelEffect::BurnShares { owner, .. } => alloc::vec![*owner],
            KernelEffect::TransferShares { from, to, .. } => alloc::vec![*from, *to],
            KernelEffect::TransferAssets { to, .. } => alloc::vec![*to],
            KernelEffect::TransferAssetsFrom { from, to, .. } => alloc::vec![*from, *to],
            #[cfg(feature = "near")]
            KernelEffect::ExternalCall { target, .. } => alloc::vec![*target],
            #[cfg(feature = "near")]
            KernelEffect::ChargeStorage { payer, .. } => alloc::vec![*payer],
            KernelEffect::EmitEvent { event } => event.required_addresses(),
        }
    }
}

impl KernelEvent {
    /// Collect all addresses referenced by this event.
    pub fn required_addresses(&self) -> Vec<Address> {
        match self {
            KernelEvent::WithdrawalStarted {
                owner, receiver, ..
            } => alloc::vec![*owner, *receiver],
            KernelEvent::DepositProcessed {
                owner, receiver, ..
            } => alloc::vec![*owner, *receiver],
            KernelEvent::WithdrawalRequested {
                owner, receiver, ..
            } => alloc::vec![*owner, *receiver],
            _ => alloc::vec![],
        }
    }
}
