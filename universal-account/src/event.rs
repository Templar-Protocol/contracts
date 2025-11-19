use near_sdk::{json_types::U64, near};

use crate::KeyId;

#[near(event_json(standard = "tmplr-ua"))]
pub enum Event {
    #[event_version("1.0.0")]
    KeyAdded { key: KeyId },
    #[event_version("1.0.0")]
    KeyRemoved { key: KeyId },
    #[event_version("1.0.0")]
    NonceExecution { key: KeyId, nonce: U64 },
}
