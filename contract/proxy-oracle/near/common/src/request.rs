use near_sdk::{near, AccountId};
use templar_common::oracle::{pyth::PriceIdentifier, redstone};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [json, borsh])]
pub enum OracleRequest {
    Pyth(PythRequest),
    RedStone(RedStoneRequest),
    Lazer(LazerRequest),
}

impl OracleRequest {
    pub fn oracle_id(&self) -> &near_sdk::AccountId {
        match self {
            OracleRequest::Pyth(id) => &id.oracle_id,
            OracleRequest::RedStone(id) => &id.oracle_id,
            OracleRequest::Lazer(id) => &id.oracle_id,
        }
    }

    pub fn pyth(oracle_id: AccountId, price_id: PriceIdentifier) -> Self {
        Self::Pyth(PythRequest {
            oracle_id,
            price_id,
        })
    }

    pub fn redstone(oracle_id: AccountId, price_id: impl Into<redstone::FeedId>) -> Self {
        Self::RedStone(RedStoneRequest {
            oracle_id,
            price_id: price_id.into(),
        })
    }

    pub fn lazer(oracle_id: AccountId, feed_id: u32) -> Self {
        Self::Lazer(LazerRequest { oracle_id, feed_id })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [json, borsh])]
pub struct PythRequest {
    pub oracle_id: near_sdk::AccountId,
    pub price_id: PriceIdentifier,
}

impl From<PythRequest> for OracleRequest {
    fn from(id: PythRequest) -> Self {
        Self::Pyth(id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [json, borsh])]
pub struct RedStoneRequest {
    pub oracle_id: near_sdk::AccountId,
    pub price_id: redstone::FeedId,
}

impl From<RedStoneRequest> for OracleRequest {
    fn from(id: RedStoneRequest) -> Self {
        Self::RedStone(id)
    }
}

/// A Lazer-backed source: the Pyth Pro adapter account plus the native Lazer `u32` feed id. Unlike
/// [`PythRequest`], this carries no `PriceIdentifier` — the adapter is keyed by feed id, so the
/// feed id is the single source of truth for both the on-chain read and the gateway write routing,
/// with no `[u8; 32] <-> u32` mapping to drift.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [json, borsh])]
pub struct LazerRequest {
    pub oracle_id: near_sdk::AccountId,
    pub feed_id: u32,
}

impl From<LazerRequest> for OracleRequest {
    fn from(id: LazerRequest) -> Self {
        Self::Lazer(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pyth_request() -> OracleRequest {
        OracleRequest::pyth("pyth.near".parse().unwrap(), PriceIdentifier([0xaa; 32]))
    }

    fn redstone_request() -> OracleRequest {
        OracleRequest::redstone("redstone.near".parse().unwrap(), "BTC")
    }

    #[test]
    fn pyth_variant_json_shape_is_unchanged() {
        let json = near_sdk::serde_json::to_string(&pyth_request()).unwrap();
        assert_eq!(
            json,
            concat!(
                r#"{"Pyth":{"oracle_id":"pyth.near","#,
                r#""price_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#
            )
        );
    }

    #[test]
    fn redstone_variant_json_shape_is_unchanged() {
        let json = near_sdk::serde_json::to_string(&redstone_request()).unwrap();
        assert_eq!(
            json,
            r#"{"RedStone":{"oracle_id":"redstone.near","price_id":"BTC"}}"#
        );
    }

    #[test]
    fn pyth_variant_round_trips_through_json() {
        let original = pyth_request();
        let json = near_sdk::serde_json::to_string(&original).unwrap();
        let back: OracleRequest = near_sdk::serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);
    }

    #[test]
    fn redstone_variant_round_trips_through_json() {
        let original = redstone_request();
        let json = near_sdk::serde_json::to_string(&original).unwrap();
        let back: OracleRequest = near_sdk::serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);
    }

    #[test]
    fn pyth_variant_borsh_tag_is_zero() {
        // Borsh enum tags are positional: Pyth == 0u8.
        let bytes = near_sdk::borsh::to_vec(&pyth_request()).unwrap();
        assert_eq!(bytes[0], 0u8);
    }

    #[test]
    fn redstone_variant_borsh_tag_is_one() {
        // Borsh enum tags are positional: RedStone == 1u8.
        let bytes = near_sdk::borsh::to_vec(&redstone_request()).unwrap();
        assert_eq!(bytes[0], 1u8);
    }

    #[test]
    fn oracle_id_accessor_returns_pyth_oracle_id() {
        let req = pyth_request();
        assert_eq!(req.oracle_id(), &"pyth.near".parse::<AccountId>().unwrap());
    }

    #[test]
    fn oracle_id_accessor_returns_redstone_oracle_id() {
        let req = redstone_request();
        assert_eq!(
            req.oracle_id(),
            &"redstone.near".parse::<AccountId>().unwrap()
        );
    }

    fn lazer_request(feed_id: u32) -> LazerRequest {
        LazerRequest {
            oracle_id: "lazer-pyth-pro.near".parse().unwrap(),
            feed_id,
        }
    }

    #[test]
    fn lazer_variant_json_shape_is_externally_tagged() {
        let req = OracleRequest::Lazer(lazer_request(7));
        let json = near_sdk::serde_json::to_string(&req).unwrap();
        assert_eq!(
            json,
            r#"{"Lazer":{"oracle_id":"lazer-pyth-pro.near","feed_id":7}}"#
        );
    }

    #[test]
    fn lazer_variant_round_trips_through_json() {
        let original = OracleRequest::Lazer(lazer_request(42));
        let json = near_sdk::serde_json::to_string(&original).unwrap();
        let back: OracleRequest = near_sdk::serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);
    }

    #[test]
    fn lazer_variant_round_trips_through_borsh() {
        let original = OracleRequest::Lazer(lazer_request(99));
        let bytes = near_sdk::borsh::to_vec(&original).unwrap();
        let back: OracleRequest = near_sdk::borsh::from_slice(&bytes).unwrap();
        assert_eq!(original, back);
    }

    #[test]
    fn lazer_variant_borsh_tag_is_two() {
        // Borsh enum tags are positional; Lazer is APPENDED last so existing
        // Pyth(0) / RedStone(1) tags are preserved.
        let bytes = near_sdk::borsh::to_vec(&OracleRequest::Lazer(lazer_request(1))).unwrap();
        assert_eq!(bytes[0], 2u8);
    }

    #[test]
    fn lazer_variant_handles_edge_feed_ids() {
        for feed_id in [u32::MIN, u32::MAX, 1, 0xDEAD_BEEF] {
            let original = OracleRequest::Lazer(lazer_request(feed_id));
            let json = near_sdk::serde_json::to_string(&original).unwrap();
            let from_json: OracleRequest = near_sdk::serde_json::from_str(&json).unwrap();
            assert_eq!(original, from_json, "feed_id={feed_id}");

            let bytes = near_sdk::borsh::to_vec(&original).unwrap();
            let from_borsh: OracleRequest = near_sdk::borsh::from_slice(&bytes).unwrap();
            assert_eq!(original, from_borsh, "feed_id={feed_id}");
        }
    }

    #[test]
    fn lazer_constructor_builds_lazer_variant() {
        let oracle_id: AccountId = "lazer-pyth-pro.near".parse().unwrap();
        let req = OracleRequest::lazer(oracle_id.clone(), 5);
        match req {
            OracleRequest::Lazer(inner) => {
                assert_eq!(inner.oracle_id, oracle_id);
                assert_eq!(inner.feed_id, 5);
            }
            other => panic!("expected Lazer, got {other:?}"),
        }
    }

    #[test]
    fn lazer_request_converts_into_oracle_request() {
        let inner = lazer_request(13);
        let req: OracleRequest = inner.clone().into();
        assert_eq!(req, OracleRequest::Lazer(inner));
    }

    #[test]
    fn oracle_id_accessor_returns_lazer_oracle_id() {
        let req = OracleRequest::Lazer(lazer_request(7));
        assert_eq!(
            req.oracle_id(),
            &"lazer-pyth-pro.near".parse::<AccountId>().unwrap()
        );
    }
}
