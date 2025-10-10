use authentication::passkey::Passkey;
use near_sdk::{json_types::U64, near, serde::Serialize};

pub mod authentication;
pub mod encoding;
pub mod transaction;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub enum KeyId {
    Passkey(Passkey),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct ExecutionParameters {
    pub index: U64,
    pub nonce: U64,
}

impl ExecutionParameters {
    #[must_use]
    pub fn next(&self) -> Self {
        Self {
            index: self.index,
            nonce: U64(self.nonce.0 + 1),
        }
    }
}

#[derive(Debug, Clone)]
#[near(serializers = [json])]
pub enum ExecuteArgs {
    Passkey {
        key: Passkey,
        message: authentication::passkey::Message<transaction::TransactionList>,
    },
}

impl ExecuteArgs {
    pub fn key(&self) -> KeyId {
        let Self::Passkey { ref key, .. } = self;
        KeyId::Passkey(key.clone())
    }

    pub fn message(&self) -> impl Serialize + '_ {
        let Self::Passkey { ref message, .. } = self;
        message
    }
}
