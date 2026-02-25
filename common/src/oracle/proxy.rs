use near_sdk::near;

use super::{price_transformer::ProxyPriceTransformer, OraclePriceId};

#[derive(Debug, Clone)]
#[near(serializers = [json, borsh])]
pub enum Proxy {
    Transformer(ProxyPriceTransformer),
    List(Vec<OraclePriceId>),
}
