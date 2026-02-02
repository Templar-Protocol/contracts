use near_sdk::env;

use crate::{
    authentication::{verify_key, HashForSigning, Key},
    encoding,
};

pub type Message<T> = super::Message<VerifyKey, T>;

verify_key!(encoding::stellar::PublicKey);

impl super::Ed25519Variant for VerifyKey {
    const PREFIX: &'static [u8] = b"Stellar Signed Message:\n";
}

impl<T> Key<Message<T>> for VerifyKey {
    fn check_signature(
        &self,
        mws: &super::MessageWithSignature<Message<T>>,
    ) -> Result<(), super::CheckSignatureError> {
        let hash = mws.message.hash_for_signing();
        env::ed25519_verify(&mws.signature, &hash, &self.0)
            .then_some(())
            .ok_or(super::CheckSignatureError::InvalidSignature)
    }
}
