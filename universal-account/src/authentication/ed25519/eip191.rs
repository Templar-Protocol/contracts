use near_sdk::{
    near,
    serde::{self, de::DeserializeOwned, Serialize},
};

use crate::{
    authentication::{
        verify_key, with_raw_string::WithRawString, CheckSignatureError, ExecutionContextProvider,
        Key, MessageWithValidSignature, Payload, SignableMessage,
    },
    encoding,
};

verify_key!(encoding::ethereum::Address);

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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use alloy::signers::local::PrivateKeySigner;
    use near_sdk::{serde_json, AccountId};

    use crate::{
        authentication::payload::Payload,
        transaction::{Action, Transaction},
        PayloadExecutionParameters, NEAR_TESTNET_CHAIN_ID,
    };

    use super::*;

    #[test]
    fn serialization() {
        let m = Message::from_parsed(Payload::new(
            PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
                .zero()
                .verifying_contract(AccountId::from_str("account_id").unwrap())
                .build_salt(),
            "hello, world".to_string(),
        ));

        let json = serde_json::to_string(&m).unwrap();

        eprintln!("{json:?}");

        let parsed: Message<String> = serde_json::from_str(&json).unwrap();

        assert_eq!(m, parsed);
    }

    fn signer() -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&[0x55_u8; 32].into()).unwrap()
    }

    fn signer2() -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&[0x66_u8; 32].into()).unwrap()
    }

    fn message() -> Message<Box<[Transaction]>> {
        Message::from_parsed(Payload::new(
            PayloadExecutionParameters::new_empty("account_id".parse().unwrap()),
            vec![Transaction {
                receiver_id: "receiver".parse().unwrap(),
                actions: vec![Action::CreateAccount].into_boxed_slice(),
            }]
            .into_boxed_slice(),
        ))
    }

    #[test]
    fn sign_message() {
        let signer = signer();
        let message = message();

        let mws = message.sign(&signer).unwrap();

        let verify_key = VerifyKey(signer.address().into());

        verify_key.verify_signature(mws).unwrap();
    }

    #[test]
    #[should_panic = "InvalidSignature"]
    fn sign_message_fail_signer() {
        let signer = signer();
        let message = message();

        let mws = message.sign(&signer).unwrap();

        let verify_key = VerifyKey(signer2().address().into());

        verify_key.verify_signature(mws).unwrap();
    }

    #[test]
    #[should_panic = "InvalidSignature"]
    fn sign_message_fail_message() {
        let signer = signer();
        let message = message();

        let mut mws = message.sign(&signer).unwrap();

        let verify_key = VerifyKey(signer.address().into());

        let mut payload_parsed = mws.message.0.parsed;
        payload_parsed.payload_mut()[0].receiver_id = "different".parse().unwrap();
        mws.message.0 = WithRawString::from_parsed(payload_parsed);

        verify_key.verify_signature(mws).unwrap();
    }

    #[test]
    #[should_panic = "InvalidSignature"]
    fn sign_message_fail_domain() {
        let signer = signer();
        let message = message();

        let mut mws = message.sign(&signer).unwrap();

        let verify_key = VerifyKey(signer.address().into());

        let mut parameters = mws.message.0.parsed.parameters();
        parameters.name = Some("different".to_string());
        mws.message.0 =
            WithRawString::from_parsed(Payload::new(parameters, mws.message.0.parsed.payload()));

        verify_key.verify_signature(mws).unwrap();
    }
}
