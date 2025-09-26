pub mod passkey;

pub trait SignedMessage {
    type Key;
    type Output;
    type Error: ToString;

    fn nonce(&self) -> u64;
    fn execute(&self, key: &Self::Key) -> Result<Self::Output, Self::Error>;
}
