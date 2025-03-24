use std::{fmt::Display, marker::PhantomData};

use near_contract_standards::fungible_token::core::ext_ft_core;
use near_sdk::{env, json_types::U128, near, AccountId, NearToken, Promise};

use crate::number::Decimal;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [json, borsh])]
pub struct FungibleAsset<T: AssetClass> {
    #[serde(skip)]
    #[borsh(skip)]
    discriminant: PhantomData<T>,
    #[serde(flatten)]
    kind: FungibleAssetKind,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [json, borsh])]
enum FungibleAssetKind {
    Nep141(AccountId),
}

impl<T: AssetClass> FungibleAsset<T> {
    pub fn transfer(&self, receiver_id: AccountId, amount: FungibleAssetAmount<T>) -> Promise {
        match self.kind {
            FungibleAssetKind::Nep141(ref contract_id) => ext_ft_core::ext(contract_id.clone())
                .with_attached_deposit(NearToken::from_yoctonear(1))
                .ft_transfer(receiver_id, u128::from(amount).into(), None),
        }
    }

    pub fn nep141(contract_id: AccountId) -> Self {
        Self {
            discriminant: PhantomData,
            kind: FungibleAssetKind::Nep141(contract_id),
        }
    }

    pub fn is_nep141(&self, account_id: &AccountId) -> bool {
        let FungibleAssetKind::Nep141(ref contract_id) = self.kind;
        contract_id == account_id
    }

    pub fn into_nep141(self) -> Option<AccountId> {
        let FungibleAssetKind::Nep141(contract_id) = self.kind;
        Some(contract_id)
    }

    pub fn current_account_balance(&self) -> Promise {
        let current_account_id = env::current_account_id();
        let FungibleAssetKind::Nep141(ref account_id) = self.kind;
        ext_ft_core::ext(account_id.clone()).ft_balance_of(current_account_id.clone())
    }
}

impl<T: AssetClass> Display for FungibleAsset<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self.kind {
                FungibleAssetKind::Nep141(ref contract_id) => contract_id.as_str(),
            }
        )
    }
}

mod sealed {
    pub trait Sealed {}
}
pub trait AssetClass: sealed::Sealed + Copy + Clone {}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct CollateralAsset;
impl sealed::Sealed for CollateralAsset {}
impl AssetClass for CollateralAsset {}
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct BorrowAsset;
impl sealed::Sealed for BorrowAsset {}
impl AssetClass for BorrowAsset {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
#[serde(from = "U128", into = "U128")]
pub struct FungibleAssetAmount<T: AssetClass> {
    amount: U128,
    #[borsh(skip)]
    discriminant: PhantomData<T>,
}

impl<T: AssetClass> Default for FungibleAssetAmount<T> {
    fn default() -> Self {
        Self::zero()
    }
}

impl<T: AssetClass> From<U128> for FungibleAssetAmount<T> {
    fn from(amount: U128) -> Self {
        Self {
            amount,
            discriminant: PhantomData,
        }
    }
}

impl<T: AssetClass> From<FungibleAssetAmount<T>> for U128 {
    fn from(value: FungibleAssetAmount<T>) -> Self {
        value.amount
    }
}

impl<T: AssetClass> From<u128> for FungibleAssetAmount<T> {
    fn from(value: u128) -> Self {
        Self::new(value)
    }
}

impl<T: AssetClass> FungibleAssetAmount<T> {
    pub fn new(amount: u128) -> Self {
        Self {
            amount: amount.into(),
            discriminant: PhantomData,
        }
    }

    pub fn zero() -> Self {
        Self {
            amount: 0.into(),
            discriminant: PhantomData,
        }
    }

    pub fn is_zero(&self) -> bool {
        self.amount.0 == 0
    }

    pub fn split(&mut self, amount: impl Into<Self>) -> Option<Self> {
        let a = amount.into();
        self.amount.0 = self.amount.0.checked_sub(a.amount.0)?;
        Some(a)
    }

    pub fn join(&mut self, other: Self) -> Option<()> {
        self.amount.0 = self.amount.0.checked_add(other.amount.0)?;
        Some(())
    }
}

impl<T: AssetClass> From<FungibleAssetAmount<T>> for Decimal {
    fn from(value: FungibleAssetAmount<T>) -> Self {
        value.amount.0.into()
    }
}

impl<T: AssetClass> From<FungibleAssetAmount<T>> for u128 {
    fn from(value: FungibleAssetAmount<T>) -> Self {
        value.amount.0
    }
}

impl<T: AssetClass> std::fmt::Display for FungibleAssetAmount<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.amount.0)
    }
}

pub type BorrowAssetAmount = FungibleAssetAmount<BorrowAsset>;
pub type CollateralAssetAmount = FungibleAssetAmount<CollateralAsset>;

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::serde_json;

    #[test]
    fn serialization() {
        let amount = BorrowAssetAmount::new(100);
        let serialized = serde_json::to_string(&amount).unwrap();
        assert_eq!(serialized, "\"100\"");
        let deserialized: BorrowAssetAmount = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, amount);
    }
}
