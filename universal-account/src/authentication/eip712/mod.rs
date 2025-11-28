use alloy::signers::SignerSync;
use alloy::sol_types::{Eip712Domain, SolStruct};
use near_sdk::{
    near,
    serde::{self, de::DeserializeOwned, Serialize},
    serde_json,
};

use super::SolBytes;
use super::{
    with_raw_string::WithRawString, CheckSignatureError, ExecutionContextProvider, Key,
    MessageWithSignature, MessageWithValidSignature, Payload, SignableMessage,
};
use crate::encoding;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct VerifyKey(pub encoding::ethereum::Address);

impl std::fmt::Display for VerifyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

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
}

impl<T: serde::Serialize> Key<Message<T>> for VerifyKey {
    fn check_signature(
        &self,
        mws: &super::MessageWithSignature<Message<T>>,
    ) -> Result<(), CheckSignatureError> {
        let calculated_domain = Eip712Domain::from(mws.message.0.parsed.parameters());

        let prehash = mws
            .message
            .eip712_prehash(&calculated_domain)
            .map_err(CheckSignatureError::other)?;

        let recovered_address = mws
            .signature
            .0
            .recover_address_from_prehash(&prehash)
            .map_err(CheckSignatureError::other)?;

        (recovered_address == self.0 .0)
            .then_some(())
            .ok_or(CheckSignatureError::InvalidSignature)
    }
}

impl<T: serde::Serialize> Message<T> {
    /// # Errors
    ///
    /// - If serialization of `T` to bytes fails.
    pub fn eip712_prehash(
        &self,
        domain: &Eip712Domain,
    ) -> Result<alloy::primitives::FixedBytes<32>, serde_json::Error> {
        let sol_payload = SolBytes {
            inner: self.0.raw.clone().into_bytes().into(),
        };
        Ok(sol_payload.eip712_signing_hash(domain))
    }

    /// # Panics
    ///
    /// - Serialization errors
    /// - Signing errors
    pub fn sign(self, key: &alloy::signers::local::PrivateKeySigner) -> MessageWithSignature<Self> {
        let domain = Eip712Domain::from(self.0.parsed.parameters());
        #[allow(
            clippy::unwrap_used,
            reason = "This function should not be used in a case where panicking is unsafe"
        )]
        let signature = key
            .sign_hash_sync(&self.eip712_prehash(&domain).unwrap())
            .unwrap();
        MessageWithSignature {
            message: self,
            signature: signature.into(),
        }
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
    use alloy::signers::local::PrivateKeySigner;

    use crate::{
        authentication::payload::Payload,
        transaction::{Action, Transaction},
        KeyParameters, PayloadExecutionParameters, NEAR_TESTNET_CHAIN_ID,
    };

    use super::*;

    #[test]
    fn serialization() {
        let m = Message::from_parsed(Payload::new(
            PayloadExecutionParameters::new_auto(
                "account_id".parse().unwrap(),
                KeyParameters::default(),
                NEAR_TESTNET_CHAIN_ID,
            ),
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

        let mws = message.sign(&signer);

        let verify_key = VerifyKey(signer.address().into());

        verify_key.verify_signature(mws).unwrap();
    }

    #[test]
    #[should_panic = "InvalidSignature"]
    fn sign_message_fail_signer() {
        let signer = signer();
        let message = message();

        let mws = message.sign(&signer);

        let verify_key = VerifyKey(signer2().address().into());

        verify_key.verify_signature(mws).unwrap();
    }

    #[test]
    #[should_panic = "InvalidSignature"]
    fn sign_message_fail_message() {
        let signer = signer();
        let message = message();

        let mut mws = message.sign(&signer);

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

        let mut mws = message.sign(&signer);

        let verify_key = VerifyKey(signer.address().into());

        let mut parameters = mws.message.0.parsed.parameters();
        parameters.name = Some("different".to_string());
        mws.message.0 =
            WithRawString::from_parsed(Payload::new(parameters, mws.message.0.parsed.payload()));

        verify_key.verify_signature(mws).unwrap();
    }
}
