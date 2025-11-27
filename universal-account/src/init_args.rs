use near_sdk::near;

use crate::KeyId;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct InitArgs {
    pub key: KeyId,
    pub chain_id: near_sdk::json_types::U128,
}
