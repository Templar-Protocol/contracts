use authentication::passkey::Passkey;
use near_sdk::{json_types::U64, near, serde::Serialize};

pub mod authentication;
pub mod encoding;
mod event;
pub use event::Event;
pub mod transaction;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct InitArgs {
    pub key: KeyId,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub enum KeyId {
    Passkey(Passkey),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
#[serde(deny_unknown_fields)]
pub struct ExecutionParameters {
    /// Static. If a universal account is deleted and recreated with the same
    /// keys, this ensures that old signatures are not replayable.
    pub block_height: U64,
    /// Static. If a key is deleted and re-added to the same account, this
    /// ensures that that old signatures are not replayable.
    pub index: U64,
    /// Increments for each message executed by this key.
    pub nonce: U64,
}

impl ExecutionParameters {
    #[must_use]
    pub fn next(&self) -> Self {
        Self {
            nonce: U64(self.nonce.0 + 1),
            ..self.clone()
        }
    }
}

#[derive(Debug, Clone)]
#[near(serializers = [json])]
pub enum ExecuteArgs {
    Passkey {
        key: Passkey,
        message: authentication::passkey::Message<Box<[transaction::Transaction]>>,
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
