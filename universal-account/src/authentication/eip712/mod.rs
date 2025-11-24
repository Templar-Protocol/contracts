use alloy::{dyn_abi::Eip712Domain, signers::SignerSync, sol_types::SolStruct};
use near_sdk::{near, serde, serde_json};

use crate::{authentication::SolPayload, encoding};

use super::{Key, MessageWithSignature, Payload, SignableMessage};

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
pub struct Message<T>(pub Payload<T>);

impl<T: serde::Serialize> SignableMessage for Message<T> {
    type Key = VerifyKey;
    type Signature = SignatureAndDomain;
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json])]
pub struct SignatureAndDomain {
    pub signature: encoding::ethereum::Signature,
    pub domain: Eip712Domain,
}

impl<T: serde::Serialize> Key<Message<T>> for VerifyKey {
    fn has_valid_signature(&self, mws: &super::MessageWithSignature<Message<T>>) -> bool {
        let Ok(prehash) = mws.message.eip712_prehash(&mws.signature.domain) else {
            return false;
        };
        let Ok(recovered_address) = mws
            .signature
            .signature
            .0
            .recover_address_from_prehash(&prehash)
        else {
            return false;
        };

        recovered_address == self.0 .0
    }
}

impl<T: serde::Serialize> Message<T> {
    pub fn eip712_prehash(
        &self,
        domain: &Eip712Domain,
    ) -> Result<alloy::primitives::FixedBytes<32>, serde_json::Error> {
        let sol_payload: SolPayload = (&self.0).try_into()?;
        Ok(sol_payload.eip712_signing_hash(domain))
    }

    pub fn sign(
        self,
        key: &alloy::signers::local::PrivateKeySigner,
        domain: Eip712Domain,
    ) -> Result<MessageWithSignature<Self>, serde_json::Error> {
        let signature = key.sign_hash_sync(&self.eip712_prehash(&domain)?).unwrap();
        Ok(MessageWithSignature {
            message: self,
            signature: SignatureAndDomain {
                signature: signature.into(),
                domain,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use alloy::{
        primitives::U256,
        signers::{local::PrivateKeySigner, Signer, SignerSync},
        sol_types::SolStruct,
    };

    use crate::{
        authentication::payload::{Payload, SolPayload},
        transaction::{Action, Transaction},
        ExecutionParameters,
    };

    use super::*;

    #[test]
    fn serialization() {
        let m = Message(Payload {
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
        Message(Payload {
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

        let mws = message.sign(&signer, domain.clone()).unwrap();

        let verify_key = VerifyKey(signer.address().into());

        mws.verify_signature(&verify_key).unwrap();
    }

    #[test]
    #[should_panic = "InvalidSignatureError"]
    fn sign_message_fail_signer() {
        let signer = signer();
        let message = message();
        let domain = domain();

        let mws = message.sign(&signer, domain.clone()).unwrap();

        let verify_key = VerifyKey(signer2().address().into());

        mws.verify_signature(&verify_key).unwrap();
    }

    #[test]
    #[should_panic = "InvalidSignatureError"]
    fn sign_message_fail_message() {
        let signer = signer();
        let message = message();
        let domain = domain();

        let mut mws = message.sign(&signer, domain.clone()).unwrap();

        let verify_key = VerifyKey(signer.address().into());

        mws.message.0.payload[0].receiver_id = "different".parse().unwrap();

        mws.verify_signature(&verify_key).unwrap();
    }

    #[test]
    #[should_panic = "InvalidSignatureError"]
    fn sign_message_fail_domain() {
        let signer = signer();
        let message = message();
        let domain = domain();

        let mut mws = message.sign(&signer, domain.clone()).unwrap();

        let verify_key = VerifyKey(signer.address().into());

        mws.signature.domain.name = Some("different".into());

        mws.verify_signature(&verify_key).unwrap();
    }
}
