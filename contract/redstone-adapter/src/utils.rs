use redstone::{FeedId, TimestampMillis};

const MS_IN_SEC: u64 = 1_000;

pub fn feed_to_string(feed: FeedId) -> String {
    let feed_bytes = feed.to_array();

    let end = feed_bytes
        .iter()
        .rposition(|&b| b != 0)
        .map_or(0, |i| i + 1);
    let start = feed_bytes[..end].iter().position(|&b| b != 0).unwrap_or(0);
    let trimmed = &feed_bytes[start..end];

    String::from_utf8_lossy(trimmed).to_string()
}

#[test]
fn test_feed_to_string_simple() {
    let btc_feed_id_array: [u8; 32] = [
        66, 84, 67, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0,
    ];

    let btc_feed_id = FeedId::from(btc_feed_id_array);

    let convert_btc_feed_id_to_string = feed_to_string(btc_feed_id);

    let non_btc_feed_id_array: [u8; 32] = [
        66, 84, 67, 0, 67, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0,
    ];

    let non_btc_feed_id = FeedId::from(non_btc_feed_id_array);

    let convert_non_btc_feed_id_to_string = feed_to_string(non_btc_feed_id);

    assert_ne!(
        convert_btc_feed_id_to_string,
        convert_non_btc_feed_id_to_string
    );
}
