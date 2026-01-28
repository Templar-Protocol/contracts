#[cfg(feature = "near")]
use alloc::vec::Vec;

#[cfg(feature = "near")]
use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "near")]
use serde::{Deserialize, Serialize};
use crate::types::Address;

#[cfg_attr(feature = "near", derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelEffect {
    MintShares { owner: Address, shares: u128 },
    BurnShares { owner: Address, shares: u128 },
    TransferShares { from: Address, to: Address, shares: u128 },
    TransferAssets { to: Address, amount: u128 },
    #[cfg(feature = "near")]
    ExternalCall {
        target: Address,
        selector: u32,
        args: Vec<u8>,
        attached_value: u128,
        callback: Option<KernelCallback>,
    },
    #[cfg(feature = "near")]
    ChargeStorage { payer: Address, bytes: u64 },
    EmitEvent { event: KernelEvent },
}

#[cfg_attr(feature = "near", derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelCallback {
    AllocationStep,
    WithdrawalStep,
    RefreshStep,
    PayoutTransfer,
}

#[cfg_attr(feature = "near", derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelEvent {
    Placeholder,
}
