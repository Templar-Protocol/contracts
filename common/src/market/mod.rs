use std::collections::HashMap;
use std::num::NonZeroU16;

use near_sdk::{near, AccountId};

use crate::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    incoming_deposit::IncomingDeposit,
    number::Decimal,
};
mod configuration;
pub use configuration::{MarketConfiguration, ValidAmountRange, APY_LIMIT};
mod external;
pub use external::*;
mod r#impl;
pub use r#impl::*;
mod price_oracle_configuration;
pub use price_oracle_configuration::PriceOracleConfiguration;

pub mod error {
    pub use super::configuration::error::*;
    pub use super::price_oracle_configuration::error::*;
}

#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub struct BorrowAssetMetrics {
    pub available: BorrowAssetAmount,
    pub deposited_active_real: BorrowAssetAmount,
    pub deposited_active_virtual: BorrowAssetAmount,
    pub deposited_incoming: Vec<IncomingDeposit>,
    pub borrowed: BorrowAssetAmount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct YieldWeights {
    pub supply: NonZeroU16,
    pub r#static: HashMap<AccountId, u16>,
}

impl YieldWeights {
    /// # Panics
    /// - If `supply` is zero.
    #[allow(clippy::unwrap_used, reason = "Only used during initial construction")]
    pub fn new_with_supply_weight(supply: u16) -> Self {
        Self {
            supply: supply.try_into().unwrap(),
            r#static: HashMap::new(),
        }
    }

    #[must_use]
    pub fn with_static(mut self, account_id: AccountId, weight: u16) -> Self {
        self.r#static.insert(account_id, weight);
        self
    }

    pub fn total_weight(&self) -> NonZeroU16 {
        self.r#static
            .values()
            .try_fold(self.supply, |a, b| a.checked_add(*b))
            .unwrap_or_else(|| crate::panic_with_message("Total weight overflow"))
    }

    pub fn static_share(&self, account_id: &AccountId) -> Decimal {
        self.r#static
            .get(account_id)
            .map_or(Decimal::ZERO, |weight| {
                Decimal::from(*weight) / u16::from(self.total_weight())
            })
    }
}

/// Parsed from the string parameter `msg` passed by `*_transfer_call` to
/// `*_on_transfer` calls.
#[near(serializers = [json])]
pub enum DepositMsg {
    /// Add the attached tokens to the sender's supply position's deposit.
    Supply,
    /// Add the attached tokens to the sender's borrow position's collateral
    /// deposit.
    Collateralize,
    /// Use the attached tokens to pay down the sender's borrow position's
    /// liability (sans fees).
    Repay,
    /// Use the attached tokens to pay down a specified borrow position's
    /// liability (sans fees).
    RepayAccount(RepayAccountMsg),
    /// Liquidate an account that is below the configured liquidation
    /// collateralization ratio threshold.
    Liquidate(LiquidateMsg),
}

impl DepositMsg {
    pub fn expects_borrow_asset(&self) -> bool {
        match self {
            Self::Supply | Self::Repay | Self::RepayAccount(..) | Self::Liquidate(..) => true,
            Self::Collateralize => false,
        }
    }
}

/// Indicate an account to repay.
#[near(serializers = [json])]
pub struct RepayAccountMsg {
    pub account_id: AccountId,
}

/// Indicate an account to liquidate.
#[near(serializers = [json])]
pub struct LiquidateMsg {
    pub account_id: AccountId,
    /// How much collateral to liquidate?
    /// Attempts to liquidate the whole position if `None`.
    pub amount: Option<CollateralAssetAmount>,
}

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub struct Withdrawal {
    pub account_id: AccountId,
    pub amount_to_account: BorrowAssetAmount,
    pub amount_to_fees: BorrowAssetAmount,
}
