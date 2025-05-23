use std::collections::HashMap;
use std::num::NonZeroU16;

use near_sdk::{env, near, AccountId};

use crate::{asset::BorrowAssetAmount, number::Decimal};

mod balance_oracle_configuration;
pub use balance_oracle_configuration::*;
mod configuration;
pub use configuration::*;
mod external;
pub use external::*;
mod r#impl;
pub use r#impl::*;

#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub struct BorrowAssetMetrics {
    pub available: BorrowAssetAmount,
    pub deposited_active: BorrowAssetAmount,
    pub deposited_inactive: BorrowAssetAmount,
    pub borrowed: BorrowAssetAmount,
}

#[derive(Clone, Debug)]
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
            .unwrap_or_else(|| env::panic_str("Total weight overflow"))
    }

    pub fn static_share(&self, account_id: &AccountId) -> Decimal {
        self.r#static
            .get(account_id)
            .map_or(Decimal::ZERO, |weight| {
                Decimal::from(*weight) / u16::from(self.total_weight())
            })
    }
}

#[near(serializers = [json])]
pub enum Nep141MarketDepositMessage {
    Supply,
    Collateralize,
    Repay,
    Liquidate(LiquidateMsg),
}

#[near(serializers = [json])]
pub struct LiquidateMsg {
    pub account_id: AccountId,
}

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub struct WithdrawalResolution {
    pub account_id: AccountId,
    pub amount_to_account: BorrowAssetAmount,
    pub amount_to_fees: BorrowAssetAmount,
}
