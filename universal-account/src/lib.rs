use authentication::passkey::Passkey;
use near_sdk::{json_types::U64, near};

pub mod authentication;
pub mod key;
pub mod transaction;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub enum KeyId {
    Passkey(Passkey),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct ExecutionParameters {
    pub index: U64,
    pub nonce: U64,
}
