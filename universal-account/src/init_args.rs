use near_sdk::near;

use crate::KeyId;

#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
pub struct InitArgs {
    pub key: KeyId,
    pub chain_id: near_sdk::json_types::U128,
    pub execute: Option<Vec<crate::transaction::Transaction>>,
}
