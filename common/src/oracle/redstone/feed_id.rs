use std::{fmt::Display, ops::Deref, sync::Arc};

use near_sdk::near;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [json, borsh])]
pub struct FeedId(Arc<str>);

impl Deref for FeedId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Clone for FeedId {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl AsRef<str> for FeedId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<&str> for FeedId {
    fn from(value: &str) -> Self {
        Self(Arc::from(value))
    }
}

impl From<String> for FeedId {
    fn from(value: String) -> Self {
        Self(Arc::from(value))
    }
}

impl Display for FeedId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<redstone::FeedId> for FeedId {
    fn from(id: redstone::FeedId) -> Self {
        let bytes = id.to_array();

        let mut end = bytes.len();
        while end > 0 && bytes[end - 1] == 0 {
            end -= 1;
        }

        Self(Arc::from(String::from_utf8_lossy(&bytes[..end])))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_feed_to_string_simple() {
        let btc_feed_id_array: [u8; 32] = [
            66, 84, 67, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ];

        let btc_feed_id = redstone::FeedId::from(btc_feed_id_array);

        let convert_btc_feed_id_to_string = super::FeedId::from(btc_feed_id);

        assert_eq!(convert_btc_feed_id_to_string, "BTC".into());

        let non_btc_feed_id_array: [u8; 32] = [
            66, 84, 67, 0, 67, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ];

        let non_btc_feed_id = redstone::FeedId::from(non_btc_feed_id_array);

        let convert_non_btc_feed_id_to_string = super::FeedId::from(non_btc_feed_id);

        assert_eq!(convert_non_btc_feed_id_to_string, "BTC\0C".into());
    }
}
