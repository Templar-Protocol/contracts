use near_sdk::{serde::Deserialize, Promise};

pub mod passkey;

pub trait Nonce {
    fn nonce(&self) -> u64;
}

pub trait Executor {
    type Input: Nonce + for<'a> Deserialize<'a>;
    type Error: ToString;

    fn execute(&self, input: &Self::Input) -> Result<Promise, Self::Error>;
}
