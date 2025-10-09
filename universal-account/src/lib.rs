use authentication::passkey::Passkey;
use near_sdk::{
    json_types::U64,
    near,
    serde::{Deserialize, Serialize},
};

pub mod authentication;
pub mod encoding;
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

pub trait Execute {
    type Output<'a>
    where
        Self: 'a;

    fn execute(&self) -> Self::Output<'_>;
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub enum ExecuteArgs {
    Passkey {
        key: Passkey,
        message: authentication::passkey::Message<Vec<transaction::Transaction>>,
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
