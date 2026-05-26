use near_sdk::near;
use templar_common::Nanoseconds;
use templar_proxy_oracle_kernel::{proxy::circuit_breaker::PriceBlockedReason, Price};

pub const MAX_CACHED_RESOLVE_ERROR_LEN: usize = 256;

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedProxyPrice {
    pub updated_at_ns: Nanoseconds,
    pub status: CachedProxyPriceStatus,
}

impl CachedProxyPrice {
    pub fn accepted_price_no_older_than(
        &self,
        now: Nanoseconds,
        max_age: Nanoseconds,
    ) -> Option<&Price> {
        let CachedProxyPriceStatus::Accepted { price } = &self.status else {
            return None;
        };
        if price.publish_time_ns > now {
            return None;
        }
        if now.saturating_sub(price.publish_time_ns) > max_age {
            return None;
        }

        Some(price)
    }
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CachedProxyPriceStatus {
    Accepted { price: Price },
    Blocked { reason: PriceBlockedReason },
    ResolveFailed { message: String },
}

pub fn bounded_resolve_error_message(message: impl Into<String>) -> String {
    let mut message = message.into();
    if message.len() <= MAX_CACHED_RESOLVE_ERROR_LEN {
        return message;
    }

    let mut boundary = MAX_CACHED_RESOLVE_ERROR_LEN;
    while !message.is_char_boundary(boundary) {
        boundary -= 1;
    }
    message.truncate(boundary);
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_message_preserves_utf8_boundary() {
        let message = format!("{}{}", "a".repeat(MAX_CACHED_RESOLVE_ERROR_LEN - 1), "é");

        let bounded = bounded_resolve_error_message(message);

        assert_eq!(bounded.len(), MAX_CACHED_RESOLVE_ERROR_LEN - 1);
        assert!(bounded.is_char_boundary(bounded.len()));
    }

    #[test]
    fn accepted_price_no_older_than_returns_only_fresh_accepted_prices() {
        let price = Price {
            price: 100,
            conf: 0,
            expo: 0,
            publish_time_ns: Nanoseconds::from_secs(10),
        };
        let cached = CachedProxyPrice {
            updated_at_ns: Nanoseconds::from_secs(20),
            status: CachedProxyPriceStatus::Accepted {
                price: price.clone(),
            },
        };

        assert_eq!(
            cached.accepted_price_no_older_than(
                Nanoseconds::from_secs(15),
                Nanoseconds::from_secs(5)
            ),
            Some(&price)
        );
        assert_eq!(
            cached.accepted_price_no_older_than(
                Nanoseconds::from_secs(16),
                Nanoseconds::from_secs(5)
            ),
            None
        );
        assert_eq!(
            cached
                .accepted_price_no_older_than(Nanoseconds::from_secs(9), Nanoseconds::from_secs(5)),
            None
        );

        let blocked = CachedProxyPrice {
            updated_at_ns: Nanoseconds::from_secs(20),
            status: CachedProxyPriceStatus::ResolveFailed {
                message: "failed".to_string(),
            },
        };
        assert_eq!(
            blocked.accepted_price_no_older_than(
                Nanoseconds::from_secs(20),
                Nanoseconds::from_secs(5)
            ),
            None
        );
    }
}
