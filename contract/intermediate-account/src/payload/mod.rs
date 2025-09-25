use near_sdk::{serde::Deserialize, Promise};

pub mod passkey;

pub trait NonceExtractor {
    fn nonce(&self) -> u64;
}

pub trait PayloadExecutor {
    type Input: NonceExtractor + for<'a> Deserialize<'a>;
    type Error: ToString;

    fn execute(&self, input: &Self::Input) -> Result<Promise, Self::Error>;
}
