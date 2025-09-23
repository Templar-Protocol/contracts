use near_sdk::{env, json_types::U64, near};

/// Configure a method of determining the current time chunk.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct TimeChunkConfiguration {
    pub duration_ms: U64,
}

impl TimeChunkConfiguration {
    pub fn new(duration_ms: u64) -> Self {
        Self {
            duration_ms: U64(duration_ms),
        }
    }

    pub fn now(&self) -> TimeChunk {
        let block_timestamp_ms = env::block_timestamp_ms();
        TimeChunk(U64(block_timestamp_ms
            .checked_div(self.duration_ms.0)
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
