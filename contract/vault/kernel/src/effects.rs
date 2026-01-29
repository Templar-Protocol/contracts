#[cfg(feature = "near")]
use alloc::vec::Vec;

use crate::types::Address;
#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelEffect {
    MintShares {
        owner: Address,
        shares: u128,
    },
    BurnShares {
        owner: Address,
        shares: u128,
    },
    TransferShares {
        from: Address,
        to: Address,
        shares: u128,
    },
    TransferAssets {
        to: Address,
        amount: u128,
    },
    #[cfg(feature = "near")]
    ExternalCall {
        target: Address,
        selector: u32,
        args: Vec<u8>,
        attached_value: u128,
        callback: Option<KernelCallback>,
    },
    #[cfg(feature = "near")]
    ChargeStorage {
        payer: Address,
        bytes: u64,
    },
    EmitEvent {
        event: KernelEvent,
    },
}

#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelCallback {
    AllocationStep,
    WithdrawalStep,
    RefreshStep,
    PayoutTransfer,
}

#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelEvent {
    /// Placeholder event for legacy transitions.
    Placeholder,
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
    },
    /// Allocation completed (either returns to Idle or proceeds to withdrawal).
    AllocationCompleted {
        op_id: u64,
        has_withdrawal: bool,
    },
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
    WithdrawalStopped {
        op_id: u64,
        escrow_shares: u128,
    },
    /// Refresh operation started.
    RefreshStarted {
        op_id: u64,
        plan_len: u32,
    },
    /// Refresh operation completed.
    RefreshCompleted {
        op_id: u64,
    },
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
    FeesRefreshed {
        now_ns: u64,
        total_assets: u128,
    },
    /// Pause state updated.
    PauseUpdated {
        paused: bool,
    },
}
