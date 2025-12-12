use near_sdk::{env, near};

use crate::{
    authentication::{HashForSigning, Key},
    encoding,
};

pub type Message<T> = super::Message<VerifyKey, T>;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct VerifyKey(pub encoding::stellar::PublicKey);
impl super::Ed25519Variant for VerifyKey {
    const PREFIX: &'static [u8] = b"Stellar Signed Message:\n";
}

impl std::fmt::Display for VerifyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<T> Key<Message<T>> for VerifyKey {
    fn check_signature(
        &self,
        mws: &super::MessageWithSignature<Message<T>>,
    ) -> Result<(), super::CheckSignatureError> {
        let preimage = mws.message.preimage_for_signing();
        env::ed25519_verify(&mws.signature, &preimage, &self.0)
            .then_some(())
            .ok_or(super::CheckSignatureError::InvalidSignature)
    }
}
