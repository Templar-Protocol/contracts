use near_sdk::near;

use crate::{
    price_transformer::{Action, Call},
    request::OracleRequest,
};

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
