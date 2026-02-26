use near_sdk::{near, BorshStorageKey};

use super::{price_transformer::ProxyPriceTransformer, OraclePriceId};

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum Proxy {
    Transformer(ProxyPriceTransformer),
    List(Vec<OraclePriceId>),
}

impl Proxy {
    pub fn list(list: impl IntoIterator<Item = OraclePriceId>) -> Self {
        Self::List(list.into_iter().collect())
    }
}

#[derive(Debug, Clone, BorshStorageKey)]
#[near(serializers = [json, borsh])]
pub enum Role {
    ModifyRoles,
    SetOracleId,
    AddProxy,
    Upgrade,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub enum Oracle {
    Pyth,
    RedStone,
}
