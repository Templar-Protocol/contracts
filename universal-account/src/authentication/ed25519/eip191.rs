use near_sdk::{
    near,
    serde::{self, de::DeserializeOwned, Serialize},
};

use crate::{
    authentication::{
        with_raw_string::WithRawString, CheckSignatureError, ExecutionContextProvider, Key,
        MessageWithValidSignature, Payload, SignableMessage,
    },
    encoding, verify_key,
};

verify_key!(VerifyKey(encoding::ethereum::Address));

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json])]
#[serde(bound = "T: DeserializeOwned")]
pub struct Message<T>(pub WithRawString<Payload<T>>);

impl<T> Message<T> {
    pub fn from_parsed(payload: Payload<T>) -> Self
    where
        T: Serialize,
    {
        Self(WithRawString::from_parsed(payload))
    }
}

impl<T: serde::Serialize> SignableMessage for Message<T> {
    type Key = VerifyKey;
    type Signature = encoding::ethereum::Signature;
    type Auxiliary = ();
}

impl<T: serde::Serialize> Key<Message<T>> for VerifyKey {
    fn check_signature(
        &self,
        mws: &super::MessageWithSignature<Message<T>>,
    ) -> Result<(), CheckSignatureError> {
        let recovered_address = mws
            .signature
            .0
            .recover_address_from_prehash(&mws.message.eip191_hash())
            .map_err(|_| CheckSignatureError::InvalidSignature)?;

        (recovered_address == self.0 .0)
            .then_some(())
            .ok_or(CheckSignatureError::InvalidSignature)
    }
}

impl<T: serde::Serialize> Message<T> {
    pub fn eip191_hash(&self) -> alloy::primitives::FixedBytes<32> {
        alloy::primitives::eip191_hash_message(&self.0.raw)
    }

    /// # Errors
    ///
    /// - Signing errors
    #[cfg(any(test, feature = "signing"))]
    pub fn sign(
        self,
        key: &alloy::signers::local::PrivateKeySigner,
    ) -> Result<super::MessageWithSignature<Self>, alloy::signers::Error> {
        use alloy::signers::SignerSync;

        let signature = key.sign_hash_sync(&self.eip191_hash())?;
        Ok(super::MessageWithSignature {
            message: self,
            signature: signature.into(),
            auxiliary: (),
        })
    }
}

impl<T: serde::Serialize> ExecutionContextProvider for MessageWithValidSignature<Message<T>> {
    type Payload = T;

    fn payload(self) -> Payload<Self::Payload> {
        self.0.message.0.parsed
    }

    fn origin(&self) -> Option<&str> {
        None
    }
}
