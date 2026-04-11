mod v0_to_v1;

use near_sdk::near;
use templar_common::{
    panic_with_message,
    versioned_state::{Migrator, StateTransformer as _},
};
pub use v0_to_v1::V0ToV1;

#[derive(Clone, Debug)]
#[near(serializers = [json])]
#[serde(tag = "from_version", rename_all = "snake_case")]
pub enum Migration {
    V0(V0ToV1),
}

impl From<V0ToV1> for Migration {
    fn from(value: V0ToV1) -> Self {
        Self::V0(value)
    }
}

impl Migrator for Migration {
    fn run(self) {
        match self {
            Migration::V0(v0) => {
                v0.run()
                    .unwrap_or_else(|e| panic_with_message(&format!("Failed to migrate V0: {e}")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::serde_json;

    use super::*;

    #[test]
    fn serialization() {
        let s = serde_json::to_string(&Migration::V0(V0ToV1)).unwrap();
        assert_eq!(s, r#"{"from_version":"v0"}"#);
    }
}
