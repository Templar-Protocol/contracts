//! Fee structures for vault operations.
//!
//! Portable across NEAR and Soroban.

use alloc::string::String;

#[cfg(feature = "near")]
use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "near")]
use serde::{Deserialize, Serialize};

/// A fee configuration with a rate and recipient.
#[cfg_attr(feature = "near", derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fee<T> {
    /// The fee rate (interpretation depends on T).
    pub fee: T,
    /// The recipient identifier (account/address as string).
    pub recipient: String,
}

/// Collection of fees for a vault.
#[cfg_attr(feature = "near", derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fees<T> {
    /// Performance fee (charged on profits).
    pub performance: Fee<T>,
    /// Management fee (charged over time).
    pub management: Fee<T>,
    /// Optional cap on how fast `total_assets` is allowed to grow for fee accrual.
    ///
    /// Interpreted as an annualized WAD rate (1e24 = 100% per year). When set,
    /// fee accrual uses `min(cur_total_assets, last_total_assets * (1 + max_rate * dt / YEAR))`
    /// as the effective `cur_total_assets`.
    pub max_total_assets_growth_rate: Option<T>,
}
