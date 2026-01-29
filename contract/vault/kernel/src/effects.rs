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
    Placeholder,
}
