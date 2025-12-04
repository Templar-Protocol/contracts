use near_sdk::near;

use crate::{
    authentication::{HashForSigning, Key},
    encoding,
};

pub type Message<T> = super::Message<VerifyKey, T>;

impl<T> HashForSigning for Message<T> {
    const MAGIC_NUMBER: &'static [u8] = b"Stellar Signed Message:\n";

    fn content_bytes(&self) -> Vec<u8> {
        self.0.raw.as_bytes().to_vec()
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct VerifyKey(pub encoding::stellar::PublicKey);
impl super::Ed25519Variant for VerifyKey {}

impl std::fmt::Display for VerifyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<T> Key<Message<T>> for VerifyKey {
    fn check_signature(
        &self,
        message: &super::MessageWithSignature<Message<T>>,
    ) -> Result<(), super::CheckSignatureError> {
        (self.0)
            .verify(&message.message.preimage_for_signing(), &message.signature)
            .then_some(())
            .ok_or(super::CheckSignatureError::InvalidSignature)
    }
}
