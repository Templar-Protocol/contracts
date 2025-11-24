use alloy::signers::SignerSync;
use alloy::sol_types::{Eip712Domain, SolStruct};
use near_sdk::{
    near,
    serde::{self, de::DeserializeOwned, Deserialize, Serialize},
    serde_json,
};
use schemars::JsonSchema;

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
    type Signature = SignatureAndDomain;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "near_sdk::serde")]
pub struct SignatureAndDomain {
    pub signature: encoding::ethereum::Signature,
    pub domain: encoding::ethereum::Domain,
}

impl<T: serde::Serialize> Key<Message<T>> for VerifyKey {
    fn check_signature(
        &self,
        mws: &super::MessageWithSignature<Message<T>>,
    ) -> Result<(), CheckSignatureError> {
        let prehash = mws
            .message
            .eip712_prehash(&mws.signature.domain.0)
            .map_err(CheckSignatureError::other)?;

        let recovered_address = mws
            .signature
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
    pub fn sign(
        self,
        key: &alloy::signers::local::PrivateKeySigner,
        domain: Eip712Domain,
    ) -> MessageWithSignature<Self> {
        #[allow(
            clippy::unwrap_used,
            reason = "This function should not be used in a case where panicking is unsafe"
        )]
        let signature = key
            .sign_hash_sync(&self.eip712_prehash(&domain).unwrap())
            .unwrap();
        MessageWithSignature {
            message: self,
            signature: SignatureAndDomain {
                signature: signature.into(),
                domain: domain.into(),
            },
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
    use alloy::{primitives::U256, signers::local::PrivateKeySigner};

    use crate::{
        authentication::payload::Payload,
        transaction::{Action, Transaction},
        ExecutionParameters,
    };

    use super::*;

    #[test]
    fn serialization() {
        let m = Message::from_parsed(Payload {
            parameters: ExecutionParameters::default(),
            account_id: "account_id".parse().unwrap(),
            payload: "hello, world".to_string(),
        });

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
        Message::from_parsed(Payload {
            parameters: ExecutionParameters::default(),
            account_id: "account_id".parse().unwrap(),
            payload: vec![Transaction {
                receiver_id: "receiver".parse().unwrap(),
                actions: vec![Action::CreateAccount].into_boxed_slice(),
            }]
            .into_boxed_slice(),
        })
    }

    fn domain() -> Eip712Domain {
        Eip712Domain {
            name: Some("Templar Universal Account".into()),
            version: Some("0.0.0".into()),
            chain_id: Some(U256::from(397)),
            verifying_contract: Some(alloy::primitives::Address([0x99_u8; 20].into())),
            salt: None,
        }
    }

    #[test]
    fn sign_message() {
        let signer = signer();
        let message = message();
        let domain = domain();

        let mws = message.sign(&signer, domain.clone());

        let verify_key = VerifyKey(signer.address().into());

        verify_key.verify_signature(mws).unwrap();
    }

    #[test]
    #[should_panic = "InvalidSignatureError"]
    fn sign_message_fail_signer() {
        let signer = signer();
        let message = message();
        let domain = domain();

        let mws = message.sign(&signer, domain.clone());

        let verify_key = VerifyKey(signer2().address().into());

        verify_key.verify_signature(mws).unwrap();
    }

    #[test]
    #[should_panic = "InvalidSignatureError"]
    fn sign_message_fail_message() {
        let signer = signer();
        let message = message();
        let domain = domain();

        let mut mws = message.sign(&signer, domain.clone());

        let verify_key = VerifyKey(signer.address().into());

        mws.message.0.parsed.payload[0].receiver_id = "different".parse().unwrap();

        verify_key.verify_signature(mws).unwrap();
    }

    #[test]
    #[should_panic = "InvalidSignatureError"]
    fn sign_message_fail_domain() {
        let signer = signer();
        let message = message();
        let domain = domain();

        let mut mws = message.sign(&signer, domain.clone());

        let verify_key = VerifyKey(signer.address().into());

        mws.signature.domain.0.name = Some("different".into());

        verify_key.verify_signature(mws).unwrap();
    }
}
