use near_sdk::near;

use super::{price_transformer::PriceTransformer, OraclePriceId};

#[derive(Debug, Clone)]
#[near(serializers = [json, borsh])]
pub enum Proxy {
    Transformer(PriceTransformer),
    List(Vec<OraclePriceId>),
}
