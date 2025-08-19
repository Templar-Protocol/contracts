use crate::number::Decimal;
use near_contract_standards::fungible_token::core::ext_ft_core;
use near_primitives::action::{Action, FunctionCallAction};
use near_sdk::base64::Engine;
use near_sdk::serde_json::Value;
use near_sdk::{
    base64, env,
    json_types::U128,
    near,
    serde_json::{self, json},
    AccountId, Gas, NearToken, Promise,
};
use std::str::FromStr;
use std::{fmt::Display, marker::PhantomData};

pub type AssetId<'a> = (&'a near_sdk::AccountIdRef, Option<&'a str>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TransferCallParams {
    pub account_id: AccountId,
    pub method_name: String,
    pub args: Value,
}

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
    Nep245 {
        contract_id: AccountId,
        token_id: String,
    },
}

impl<T: AssetClass> FungibleAsset<T> {
    /// Really depends on the implementation, but this should suffice, since
    /// normal implementations use < 3TGas.
    pub const GAS_FT_TRANSFER: Gas = Gas::from_tgas(6);
    /// NEAR Intents implementation uses < 4TGas.
    pub const GAS_MT_TRANSFER: Gas = Gas::from_tgas(7);

    #[allow(clippy::missing_panics_doc, clippy::unwrap_used)]
    pub fn transfer(&self, receiver_id: AccountId, amount: FungibleAssetAmount<T>) -> Promise {
        match self.kind {
            FungibleAssetKind::Nep141(ref contract_id) => ext_ft_core::ext(contract_id.clone())
                .with_static_gas(Self::GAS_FT_TRANSFER)
                .with_attached_deposit(NearToken::from_yoctonear(1))
                .ft_transfer(receiver_id, u128::from(amount).into(), None),
            FungibleAssetKind::Nep245 {
                ref contract_id,
                ref token_id,
            } => Promise::new(contract_id.clone()).function_call(
                "mt_transfer".into(),
                serde_json::to_vec(&json!({
                   "receiver_id": receiver_id,
                   "token_id": token_id,
                   "amount": amount,
                }))
                .unwrap(),
                NearToken::from_yoctonear(1),
                Self::GAS_MT_TRANSFER,
            ),
        }
    }

    #[allow(clippy::expect_used, reason = "Args serialization shouldn't fail")]
    pub fn create_function_call_action(
        &self,
        receiver_id: &AccountId,
        amount: U128,
        msg: &str,
    ) -> Action {
        let (method_name, args_json) = match &self.kind {
            FungibleAssetKind::Nep141(..) => (
                "ft_transfer_call".to_string(),
                json!({
                    "receiver_id": receiver_id,
                    "amount": amount,
                    "msg": msg,
                }),
            ),
            FungibleAssetKind::Nep245 { token_id, .. } => (
                "mt_transfer_call".to_string(),
                json!({
                    "receiver_id": receiver_id,
                    "token_id": format!("nep141:{}", token_id),
                    "amount": amount,
                    "msg": msg,
                }),
            ),
        };

        let args = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_string(&args_json).expect("Failed to serialize data"))
            .into_bytes();

        Action::FunctionCall(Box::new(FunctionCallAction {
            method_name,
            args,
            gas: Self::GAS_MT_TRANSFER.as_gas(),
            deposit: NearToken::from_yoctonear(1).as_yoctonear(),
        }))
    }

    pub fn nep141(contract_id: AccountId) -> Self {
        Self {
            discriminant: PhantomData,
            kind: FungibleAssetKind::Nep141(contract_id),
        }
    }

    pub fn nep245(contract_id: AccountId, token_id: String) -> Self {
        Self {
            discriminant: PhantomData,
            kind: FungibleAssetKind::Nep245 {
                contract_id,
                token_id,
            },
        }
    }

    pub fn is_nep141(&self, account_id: &AccountId) -> bool {
        matches!(self.kind, FungibleAssetKind::Nep141(ref contract_id) if contract_id == account_id)
    }

    pub fn into_nep141(self) -> Option<AccountId> {
        match self.kind {
            FungibleAssetKind::Nep141(contract_id) => Some(contract_id),
            FungibleAssetKind::Nep245 { .. } => None,
        }
    }

    pub fn is_nep245(&self, account_id: &AccountId, token_id: &str) -> bool {
        let t = token_id;
        matches!(self.kind, FungibleAssetKind::Nep245 { ref contract_id, ref token_id } if contract_id == account_id && token_id == t)
    }

    pub fn into_nep245(self) -> Option<(AccountId, String)> {
        match self.kind {
            FungibleAssetKind::Nep245 {
                contract_id,
                token_id,
            } => Some((contract_id, token_id)),
            FungibleAssetKind::Nep141(_) => None,
        }
    }

    pub fn contract_id(&self) -> AccountId {
        match self.kind {
            FungibleAssetKind::Nep245 {
                ref contract_id, ..
            }
            | FungibleAssetKind::Nep141(ref contract_id) => contract_id.clone(),
        }
    }

    pub fn as_asset_id(&self) -> AssetId {
        match &self.kind {
            FungibleAssetKind::Nep141(account_id) => (account_id, None),
            FungibleAssetKind::Nep245 {
                contract_id,
                token_id,
            } => (contract_id, Some(token_id)),
        }
    }

    #[allow(clippy::missing_panics_doc, clippy::unwrap_used)]
    pub fn current_account_balance(&self) -> Promise {
        let current_account_id = env::current_account_id();
        match self.kind {
            FungibleAssetKind::Nep141(ref account_id) => {
                ext_ft_core::ext(account_id.clone()).ft_balance_of(current_account_id.clone())
            }
            FungibleAssetKind::Nep245 {
                ref contract_id,
                ref token_id,
            } => Promise::new(contract_id.clone()).function_call(
                "mt_balance_of".into(),
                serde_json::to_vec(&json!({
                    "account_id": current_account_id,
                    "token_id": token_id,
                }))
                .unwrap(),
                NearToken::from_millinear(0),
                Gas::from_tgas(4),
            ),
        }
    }

    pub fn coerce<U: AssetClass>(self) -> FungibleAsset<U> {
        FungibleAsset {
            discriminant: PhantomData,
            kind: self.kind,
        }
    }
}

impl<T: AssetClass> Display for FungibleAsset<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            FungibleAssetKind::Nep141(ref contract_id) => {
                write!(f, "nep141:{contract_id}")
            }
            FungibleAssetKind::Nep245 {
                ref contract_id,
                ref token_id,
            } => write!(f, "nep245:{contract_id}:{token_id}"),
        }
    }
}

impl<T: AssetClass> FromStr for FungibleAsset<T> {
    type Err = <AccountId as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((contract_id, token_id)) = s.split_once(':') {
            if let Some(token_id) = token_id.strip_prefix("nep245:") {
                return Ok(FungibleAsset::nep245(
                    AccountId::try_from(contract_id.to_string())?,
                    token_id.to_string(),
                ));
            }
        }
        Ok(FungibleAsset::nep141(AccountId::try_from(s.to_string())?))
    }
}

mod sealed {
    pub trait Sealed {}
}
pub trait AssetClass: sealed::Sealed + Copy + Clone + Send {}

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

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FromAsset;
impl sealed::Sealed for FromAsset {}
impl AssetClass for FromAsset {}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ToAsset;
impl sealed::Sealed for ToAsset {}
impl AssetClass for ToAsset {}

macro_rules! impl_cross_eq {
    ($a:ident, $b:ident) => {
        impl PartialEq<FungibleAsset<$b>> for FungibleAsset<$a> {
            fn eq(&self, other: &FungibleAsset<$b>) -> bool {
                self.kind == other.kind // Compare the actual FungibleAssetKind
            }
        }

        impl PartialEq<FungibleAsset<$a>> for FungibleAsset<$b> {
            fn eq(&self, other: &FungibleAsset<$a>) -> bool {
                self.kind == other.kind // Compare the actual FungibleAssetKind
            }
        }
    };
}

impl_cross_eq!(CollateralAsset, BorrowAsset);
impl_cross_eq!(CollateralAsset, FromAsset);
impl_cross_eq!(CollateralAsset, ToAsset);
impl_cross_eq!(BorrowAsset, FromAsset);
impl_cross_eq!(BorrowAsset, ToAsset);
impl_cross_eq!(FromAsset, ToAsset);

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
            amount: U128(amount),
            discriminant: PhantomData,
        }
    }

    pub const fn zero() -> Self {
        Self {
            amount: U128(0),
            discriminant: PhantomData,
        }
    }

    pub fn is_zero(&self) -> bool {
        self.amount.0 == 0
    }

    #[must_use]
    pub fn split(&mut self, amount: impl Into<Self>) -> Option<Self> {
        let a = amount.into();
        self.amount.0 = self.amount.0.checked_sub(a.amount.0)?;
        Some(a)
    }

    #[must_use]
    pub fn join(&mut self, amount: impl Into<Self>) -> Option<()> {
        let a = amount.into();
        self.amount.0 = self.amount.0.checked_add(a.amount.0)?;
        Some(())
    }
}

#[macro_export]
macro_rules! asset_op {
    (@msg($($msg:literal)?) $a_head:ident $(. $a_tail:ident)* += $b:expr $(;)*) => {
        $crate::asset::FungibleAssetAmount::join(&mut $a_head $(.$a_tail)*, $b).unwrap_or_else(|| {
            ::near_sdk::env::panic_str(concat!($($msg, ": ",)? stringify!($a_head $(.$a_tail)*), " + ", stringify!($b), " overflow"));
        });
    };
    ($a_head:ident $(. $a_tail:ident)* += $b:expr $(;)*) => {
        $crate::asset_op!(@msg() $a_head $(.$a_tail)* += $b);
    };
    (@msg($($msg:literal)?) $a_head:ident $(. $a_tail:ident)* += $b:expr ; $($tail:tt)*) => {
        $crate::asset_op!(@msg($($msg)?) $a_head $(.$a_tail)* += $b);
        $crate::asset_op!($($tail)*);
    };
    ($a_head:ident $(. $a_tail:ident)* += $b:expr ; $($tail:tt)*) => {
        $crate::asset_op!($a_head $(.$a_tail)* += $b);
        $crate::asset_op!($($tail)*);
    };

    (@msg($($msg:literal)?) $a_head:ident $(. $a_tail:ident)* -= $b:expr $(;)*) => {
        $crate::asset::FungibleAssetAmount::split(&mut $a_head $(.$a_tail)*, $b).unwrap_or_else(|| {
            ::near_sdk::env::panic_str(concat!($($msg, ": ",)? stringify!($a_head $(.$a_tail)*), " - ", stringify!($b), " underflow"));
        });
    };
    ($a_head:ident $(. $a_tail:ident)* -= $b:expr $(;)*) => {
        $crate::asset_op!(@msg() $a_head $(.$a_tail)* -= $b);
    };
    (@msg($($msg:literal)?) $a_head:ident $(. $a_tail:ident)* -= $b:expr ; $($tail:tt)*) => {
        $crate::asset_op!(@msg($($msg)?) $a_head $(.$a_tail)* -= $b);
        $crate::asset_op!($($tail)*);
    };
    ($a_head:ident $(. $a_tail:ident)* -= $b:expr ; $($tail:tt)*) => {
        $crate::asset_op!($a_head $(.$a_tail)* -= $b);
        $crate::asset_op!($($tail)*);
    };

    ($s:stmt $(;)*) => {
        $s;
    };
    ($s:stmt ; $($tail:tt)*) => {
        $s;
        $crate::asset_op!($($tail)*);
    };

    () => {};
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

    #[test]
    #[should_panic = "a + u128::MAX overflow"]
    fn asset_op_macro_overflow() {
        let mut a = BorrowAssetAmount::new(100);

        asset_op! {
            a += u128::MAX;
        };
    }

    #[test]
    #[should_panic = "a - 101u128 underflow"]
    fn asset_op_macro_underflow() {
        let mut a = BorrowAssetAmount::new(100);

        asset_op! {
            a -= 101u128;
        };
    }

    #[test]
    fn test_cross_type_with_same_kind() {
        let collateral = FungibleAsset::<CollateralAsset> {
            discriminant: PhantomData,
            kind: FungibleAssetKind::Nep141("usdc.near".parse().unwrap()),
        };

        let borrow = FungibleAsset::<BorrowAsset> {
            discriminant: PhantomData,
            kind: FungibleAssetKind::Nep141("usdc.near".parse().unwrap()),
        };

        // Different types but same kind = equal!
        assert_eq!(collateral, borrow);
        assert_eq!(borrow, collateral);
    }

    #[test]
    fn test_cross_type_with_different_kind() {
        let collateral = FungibleAsset::<CollateralAsset> {
            discriminant: PhantomData,
            kind: FungibleAssetKind::Nep141("usdc.near".parse().unwrap()),
        };

        let borrow = FungibleAsset::<BorrowAsset> {
            discriminant: PhantomData,
            kind: FungibleAssetKind::Nep141("usdt.near".parse().unwrap()),
        };

        // Different types and different kind = not equal
        assert_ne!(collateral, borrow);
        assert_ne!(borrow, collateral);
    }

    #[test]
    fn test_same_type_equality() {
        let asset1 = FungibleAsset::<CollateralAsset> {
            discriminant: PhantomData,
            kind: FungibleAssetKind::Nep141("usdc.near".parse().unwrap()),
        };

        let asset2 = FungibleAsset::<CollateralAsset> {
            discriminant: PhantomData,
            kind: FungibleAssetKind::Nep141("usdc.near".parse().unwrap()),
        };

        // Same type, same kind = equal (uses derived PartialEq)
        assert_eq!(asset1, asset2);
    }
}
