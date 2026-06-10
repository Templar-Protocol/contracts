use p256::{ecdsa::signature::Signer, elliptic_curve::rand_core::OsRng};
use templar_universal_account::{
    authentication::{
        ed25519::{eip191, raw, sep53},
        eip712,
        passkey::{
            self,
            data::{AuthenticatorData, ClientDataJson},
        },
        with_raw_string::WithRawString,
        HashForSigning, MessageWithSignature, Payload,
    },
    transaction::Transaction,
    ExecuteArgs, ExecuteArgsMessage, KeyId,
};

pub enum TestSigner {
    Passkey(p256::SecretKey),
    Ed25519Raw(ed25519_dalek::SigningKey),
    Eip712(alloy::signers::local::PrivateKeySigner),
    Sep53(ed25519_dalek::SigningKey),
    Eip191(alloy::signers::local::PrivateKeySigner),
}

impl TestSigner {
    pub fn random_passkey() -> Self {
        Self::Passkey(p256::SecretKey::random(&mut OsRng))
    }

    pub fn random_ed25519_raw() -> Self {
        Self::Ed25519Raw(ed25519_dalek::SigningKey::generate(&mut OsRng))
    }

    pub fn random_eip712() -> Self {
        Self::Eip712(alloy::signers::local::PrivateKeySigner::random())
    }

    pub fn random_sep53() -> Self {
        Self::Sep53(ed25519_dalek::SigningKey::generate(&mut OsRng))
    }

    pub fn random_eip191() -> Self {
        Self::Eip191(alloy::signers::local::PrivateKeySigner::random())
    }

    pub fn fixed_passkey(bytes: [u8; 32]) -> Self {
        Self::Passkey(p256::SecretKey::from_bytes(&bytes.into()).unwrap())
    }

    pub fn fixed_ed25519_raw(bytes: [u8; 32]) -> Self {
        Self::Ed25519Raw(ed25519_dalek::SigningKey::from_bytes(&bytes))
    }

    pub fn fixed_sep53(bytes: [u8; 32]) -> Self {
        Self::Sep53(ed25519_dalek::SigningKey::from_bytes(&bytes))
    }

    pub fn fixed_eip191(bytes: [u8; 32]) -> Self {
        Self::Eip191(alloy::signers::local::PrivateKeySigner::from_bytes(&bytes.into()).unwrap())
    }

    pub fn id(&self) -> KeyId {
        match self {
            Self::Passkey(key) => passkey::VerifyKey(key.public_key().into()).into(),
            Self::Ed25519Raw(key) => raw::VerifyKey(key.verifying_key().to_bytes().into()).into(),
            Self::Eip712(key) => eip712::VerifyKey(key.address().into()).into(),
            Self::Sep53(key) => sep53::VerifyKey(key.verifying_key().to_bytes().into()).into(),
            Self::Eip191(key) => eip191::VerifyKey(key.address().into()).into(),
        }
    }

    pub fn execute_args(
        &self,
        payload: WithRawString<Payload<Box<[Transaction]>>>,
    ) -> ExecuteArgs<Box<[Transaction]>> {
        match self {
            Self::Passkey(secret_key) => {
                let payload = passkey::Message(payload);
                let challenge = payload.hash_for_signing();

                let message: MessageWithSignature<_> = payload.sign(
                    secret_key,
                    AuthenticatorData(Box::new([0xff_u8; 32])),
                    ClientDataJson {
                        r#type: "type".to_string(),
                        challenge: challenge.into(),
                        origin: "origin".to_string(),
                        cross_origin: None,
                        top_origin: None,
                    },
                );

                ExecuteArgsMessage {
                    key: passkey::VerifyKey(secret_key.public_key().into()),
                    mws: Box::new(message),
                }
                .into()
            }
            Self::Ed25519Raw(key) => {
                let message = raw::Message::new(payload);
                let signature = key.sign(&message.preimage_for_signing()).to_bytes().into();
                let message = message.with_signature(signature);

                ExecuteArgsMessage {
                    key: raw::VerifyKey(key.verifying_key().to_bytes().into()),
                    mws: Box::new(message),
                }
                .into()
            }
            Self::Eip712(key) => {
                let message = eip712::Message(payload);
                let mws = message.sign(key).unwrap();

                ExecuteArgsMessage {
                    key: eip712::VerifyKey(key.address().into()),
                    mws: Box::new(mws),
                }
                .into()
            }
            Self::Sep53(key) => {
                let message = sep53::Message::new(payload);
                let signature = key.sign(&message.hash_for_signing()).to_bytes().into();
                let message = message.with_signature(signature);

                ExecuteArgsMessage {
                    key: sep53::VerifyKey(key.verifying_key().to_bytes().into()),
                    mws: Box::new(message),
                }
                .into()
            }
            Self::Eip191(key) => {
                let message = eip191::Message(payload);
                let mws = message.sign(key).unwrap();

                ExecuteArgsMessage {
                    key: eip191::VerifyKey(key.address().into()),
                    mws: Box::new(mws),
                }
                .into()
            }
        }
    }
}
