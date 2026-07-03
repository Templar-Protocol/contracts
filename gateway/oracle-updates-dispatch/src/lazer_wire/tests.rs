use std::time::Duration;

use pyth_lazer_protocol::{api::Channel, time::FixedRate};
use templar_gateway_core::RedactedString;

use super::*;
use crate::LazerSubscriptionConfig;

#[test]
fn subscription_frame_uses_protocol_request_types() {
    use PriceFeedProperty::{
        Confidence, EmaConfidence, EmaPrice, Exponent, FeedUpdateTimestamp, Price,
    };

    let config = LazerSourceConfig::new(
        "wss://example.com/v1/stream".parse().expect("valid URL"),
        RedactedString::from("secret-token"),
        LazerSubscriptionConfig {
            channel: None,
            max_payload_age: Duration::from_secs(5),
        },
    )
    .expect("valid Lazer config");

    let frame =
        subscription_frame_for_feeds(&config, SubscriptionId(1), vec![8, 7].into_iter().collect())
            .expect("frame should serialize");
    println!("subscription_frame={frame}");
    let request = serde_json::from_str::<WsRequest>(&frame).expect("protocol request");

    match request {
        WsRequest::Subscribe(request) => {
            assert_eq!(request.subscription_id, SubscriptionId(1));
            assert_eq!(
                request.params.price_feed_ids,
                Some(vec![PriceFeedId(7), PriceFeedId(8)])
            );
            assert_eq!(request.params.symbols, None);
            assert_eq!(
                request.params.properties,
                vec![
                    Price,
                    Confidence,
                    Exponent,
                    EmaPrice,
                    EmaConfidence,
                    FeedUpdateTimestamp
                ]
            );
            assert_eq!(request.params.formats, vec![Format::Solana]);
            assert_eq!(request.params.delivery_format, DeliveryFormat::Json);
            assert_eq!(
                request.params.json_binary_encoding,
                JsonBinaryEncoding::Base64
            );
            assert_eq!(
                request.params.channel,
                Channel::FixedRate(FixedRate::RATE_200_MS)
            );
            assert!(request.params.ignore_invalid_feeds);
        }
        WsRequest::Unsubscribe(_) => panic!("expected subscribe request"),
    }
}
