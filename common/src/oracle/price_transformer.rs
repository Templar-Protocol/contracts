use near_sdk::{
    json_types::{Base64VecU8, U64},
    near, AccountId, Gas,
};

use crate::number::Decimal;

use super::{
    pyth::{self, PriceIdentifier},
    OracleRequest,
};

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum Action {
    NormalizeNativeLstPrice { decimals: u32 },
}

impl Action {
    pub fn apply(&self, mut price: pyth::Price, input: Decimal) -> Option<pyth::Price> {
        match self {
            Self::NormalizeNativeLstPrice { decimals } => {
                let scale_factor = input / 10u128.pow(*decimals);

                let price_is_negative = if price.price.0.is_negative() { -1 } else { 1 };
                let abs_price_u128 = i128::from(price.price.0).unsigned_abs();
                price.price.0 = price_is_negative
                    * i64::try_from((abs_price_u128 * scale_factor).to_u128_floor()?).ok()?;
                price.conf.0 = u64::try_from((price.conf.0 * scale_factor).to_u128_ceil()?).ok()?;
                Some(price)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Call {
    pub account_id: AccountId,
    pub method_name: String,
    pub args: Base64VecU8,
    pub gas: U64,
}

impl Call {
    #[cfg(all(not(target_arch = "wasm32"), feature = "rpc"))]
    #[allow(clippy::unwrap_used)]
    pub fn new(
        account_id: &near_sdk::AccountIdRef,
        method_name: impl Into<String>,
        args: impl near_sdk::serde::Serialize,
        gas: Gas,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            method_name: method_name.into(),
            args: near_sdk::serde_json::to_vec(&args).unwrap().into(),
            gas: gas.as_gas().into(),
        }
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "rpc"))]
    pub fn new_simple(account_id: &near_sdk::AccountIdRef, method_name: impl Into<String>) -> Self {
        Self::new(
            account_id,
            method_name,
            near_sdk::serde_json::Value::Null,
            Gas::from_tgas(3),
        )
    }

    pub fn promise(&self) -> near_sdk::Promise {
        near_sdk::Promise::new(self.account_id.clone()).function_call(
            self.method_name.clone(),
            self.args.0.clone(),
            near_sdk::NearToken::from_near(0),
            Gas::from_gas(self.gas.0),
        )
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "rpc"))]
    #[allow(clippy::expect_used, reason = "AccountId round-trip parse cannot fail")]
    pub fn rpc_call(&self) -> near_primitives::views::QueryRequest {
        near_primitives::views::QueryRequest::CallFunction {
            account_id: self.account_id.as_str().parse().expect("valid account_id"),
            method_name: self.method_name.clone(),
            args: self.args.0.clone().into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct PriceTransformer {
    pub price_id: PriceIdentifier,
    pub call: Call,
    pub action: Action,
}

impl PriceTransformer {
    pub fn lst(price_id: PriceIdentifier, decimals: u32, call: Call) -> Self {
        Self {
            price_id,
            call,
            action: Action::NormalizeNativeLstPrice { decimals },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct ProxyPriceTransformer {
    pub request: OracleRequest,
    pub call: Call,
    pub action: Action,
}

impl ProxyPriceTransformer {
    pub fn lst(price_id: OracleRequest, decimals: u32, call: Call) -> Self {
        Self {
            request: price_id,
            call,
            action: Action::NormalizeNativeLstPrice { decimals },
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::dec;

    use super::*;

    #[test]
    fn price_transformation() {
        let transformation = Action::NormalizeNativeLstPrice { decimals: 24 };
        let price_before = pyth::Price {
            price: 1234.into(),
            conf: 4.into(),
            expo: 5,
            publish_time: 0.into(),
        };

        let price_after = transformation
            .apply(price_before, dec!("1.2").mul_pow10(24).unwrap())
            .unwrap();

        assert_eq!(
            price_after,
            pyth::Price {
                price: 1480.into(),
                conf: 5.into(),
                expo: 5,
                publish_time: 0.into(),
            },
        );
    }
}
