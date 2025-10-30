use near_sdk::{env, json_types::U64, near};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub enum V0 {
    BlockHeight { divisor: U64 },
    EpochHeight { divisor: U64 },
    BlockTimestampMs { divisor: U64 },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct V1 {
    pub duration_ms: U64,
}

/// Configure a method of determining the current time chunk.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
#[serde(tag = "version")]
pub enum TimeChunkConfiguration {
    #[serde(untagged)]
    V0(V0),
    #[serde(untagged)]
    V1(V1),
}

impl TimeChunkConfiguration {
    pub fn new(duration_ms: u64) -> Self {
        Self::V1(V1 {
            duration_ms: U64(duration_ms),
        })
    }

    pub fn duration_ms(&self) -> u64 {
        match self {
            TimeChunkConfiguration::V0(V0::BlockTimestampMs { divisor }) => divisor.0,
            TimeChunkConfiguration::V0(_) => env::panic_str("Unsupported time chunk configuration"),
            TimeChunkConfiguration::V1(v1) => v1.duration_ms.0,
        }
    }

    pub fn now(&self) -> TimeChunk {
        let block_timestamp_ms = env::block_timestamp_ms();
        TimeChunk(U64(block_timestamp_ms
            .checked_div(self.duration_ms())
            .unwrap_or(block_timestamp_ms)))
    }

    pub fn previous(&self) -> TimeChunk {
        let TimeChunk(U64(time)) = self.now();
        #[allow(clippy::unwrap_used, reason = "Assume now > 0")]
        TimeChunk(U64(time.checked_sub(1).unwrap()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct TimeChunk(pub U64);

#[cfg(test)]
mod tests {
    use near_sdk::serde_json;

    use super::*;

    #[test]
    fn v0_deserialization() {
        let s = r#"{
          "BlockTimestampMs": {
            "divisor": "600000"
          }
        }"#;

        let d: TimeChunkConfiguration = serde_json::from_str(s).unwrap();

        assert_eq!(
            d,
            TimeChunkConfiguration::V0(V0::BlockTimestampMs {
                divisor: U64(600_000),
            }),
        );
        assert_eq!(d.duration_ms(), 600_000);
    }

    #[test]
    fn v0_serialization() {
        let v0 = TimeChunkConfiguration::V0(V0::BlockTimestampMs {
            divisor: U64(600_000),
        });

        let s = serde_json::to_string(&v0).unwrap();

        assert_eq!(s, r#"{"BlockTimestampMs":{"divisor":"600000"}}"#);
    }

    #[test]
    fn v1_deserialization() {
        let s = r#"{
          "duration_ms": "600000"
        }"#;

        let d: TimeChunkConfiguration = serde_json::from_str(s).unwrap();

        assert_eq!(
            d,
            TimeChunkConfiguration::V1(V1 {
                duration_ms: U64(600_000),
            }),
        );
        assert_eq!(d.duration_ms(), 600_000);
    }

    #[test]
    fn v1_serialization() {
        let v0 = TimeChunkConfiguration::V1(V1 {
            duration_ms: U64(600_000),
        });

        let s = serde_json::to_string(&v0).unwrap();

        assert_eq!(s, r#"{"duration_ms":"600000"}"#);
    }
}
