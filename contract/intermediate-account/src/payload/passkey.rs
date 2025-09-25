use std::ops::Deref;

use near_sdk::{
    base64::prelude::*,
    env, near,
    serde::{self, de, Deserialize, Serialize},
    serde_json, Promise,
};
use p256::ecdsa;
use p256::ecdsa::signature::Verifier;

use crate::transaction::Transaction;

use super::{NonceExtractor, PayloadExecutor};

#[derive(Clone, Debug)]
#[near(serializers = [])]
pub struct AuthenticatorData(pub Vec<u8>);

impl Deref for AuthenticatorData {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'de> Deserialize<'de> for AuthenticatorData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes_hex: String = Deserialize::deserialize(deserializer)?;
        let bytes = hex::decode(bytes_hex).map_err(de::Error::custom)?;
        Ok(Self(bytes))
    }
}

impl Serialize for AuthenticatorData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Serialize::serialize(&hex::encode(&self.0), serializer)
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [])]
pub struct WithRawString<T> {
    raw: String,
    parsed: T,
}

impl<'de, T: for<'a> Deserialize<'a>> Deserialize<'de> for WithRawString<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw: String = Deserialize::deserialize(deserializer)?;
        let mut d = serde_json::Deserializer::from_str(&r#raw);
        let parsed: T = Deserialize::deserialize(&mut d).map_err(de::Error::custom)?;

        Ok(Self { raw, parsed })
    }
}

impl<T> Serialize for WithRawString<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Serialize::serialize(&self.raw, serializer)
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
#[serde(rename_all = "camelCase")]
pub struct ClientDataJson {
    r#type: String,
    challenge: String,
    origin: String,
    cross_origin: Option<bool>,
    top_origin: Option<String>,
}

#[derive(Clone, Debug)]
#[near(serializers = [])]
pub struct Signature(pub ecdsa::DerSignature);

impl Deref for Signature {
    type Target = ecdsa::DerSignature;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let signature_b64url: String = serde::Deserialize::deserialize(deserializer)?;
        let signature_bytes = BASE64_URL_SAFE_NO_PAD
            .decode(signature_b64url)
            .map_err(de::Error::custom)?;
        let signature =
            ecdsa::DerSignature::try_from(signature_bytes.as_slice()).map_err(de::Error::custom)?;
        Ok(Self(signature))
    }
}

impl Serialize for Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let signature_b64url = BASE64_URL_SAFE_NO_PAD.encode(self.0.as_bytes());
        Serialize::serialize(&signature_b64url, serializer)
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [json])]
pub struct Input {
    authenticator_data: AuthenticatorData,
    payload: WithRawString<Transaction>,
    client_data_json: WithRawString<ClientDataJson>,
    signature: Signature,
}

impl NonceExtractor for Input {
    fn nonce(&self) -> u64 {
        self.payload.parsed.nonce.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct Passkey {
    public_key: crate::key::p256::PublicKey,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid challenge")]
    InvalidChallenge,
    #[error("Payload hash mismatch")]
    PayloadHashMismatch,
    #[error("Invalid signature")]
    InvalidSignature,
}

impl PayloadExecutor for Passkey {
    type Input = Input;
    type Error = Error;

    fn execute(&self, input: &Input) -> Result<Promise, Self::Error> {
        // Check signature
        let msg = [
            &*input.authenticator_data,
            &env::sha256(input.client_data_json.raw.as_bytes()),
        ]
        .concat();

        self.public_key
            .verify(&msg, &*input.signature)
            .map_err(|_| Error::InvalidSignature)?;

        // Check that the un-hashed payload we received hashes to the value that was signed.
        let payload_hash = BASE64_STANDARD_NO_PAD
            .decode(&input.client_data_json.parsed.challenge)
            .map_err(|_| Error::InvalidChallenge)?;

        if env::sha256(input.payload.raw.as_bytes()) != payload_hash {
            return Err(Error::PayloadHashMismatch);
        }

        Ok(input.payload.parsed.construct_promise())
    }
}

// #[test]
// fn verify() {
//     let b64 = BASE64_URL_SAFE_NO_PAD;

//     let public_key_b64url = b"MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEPt5mQwA2VRJeZatJ7TGHat3rNYasQQNXgkcX8DCJ07aLPsFEIFViBtVjNadmENSxL13l6Jdc2kFO_AVzHfeZEw";
//     let authenticator_data_hex =
//         "49960de5880e8c687434170f6476605b8fe4aeb9a28632c7995cf3ba831d97631d00000000";
//     let authenticator_data_bytes = hex::decode(authenticator_data_hex).unwrap();
//     let payload = "payload";
//     let payload_hash_b64 = "I59Z7VXnN8dxR89VrQwbAwttfudIp0JpUvm4UtWpNeU";
//     let payload_hash = BASE64_STANDARD_NO_PAD.decode(payload_hash_b64).unwrap();
//     assert_eq!(sha256(payload.as_bytes()), payload_hash);
//     let signature_b64url = "MEQCIDmjkX_oRv66u3Zxfc7NsbpXTXHqv3wWw2bn_KQrG9QyAiAv5I6pWb5ljTtSX-2eXJSXIMgtMEqrG8jD34FnAKcEhg";

//     let public_key_bytes = b64.decode(public_key_b64url).unwrap();
//     let public_key_spki = spki::SubjectPublicKeyInfoRef::from_der(&public_key_bytes).unwrap();
//     println!(
//         "{:#?}",
//         spki::SubjectPublicKeyInfo::<ObjectIdentifier, BitString>::from_der(&public_key_bytes)
//             .unwrap(),
//     );
//     let public_key = VerifyingKey::try_from(public_key_spki).unwrap();

//     println!("{public_key:#?}");

//     let signature_bytes = b64.decode(signature_b64url).unwrap();

//     let signature = ecdsa::DerSignature::try_from(signature_bytes.as_slice()).unwrap();

//     println!("{signature:#?}");

//     let client_data_json = br#"{"type":"webauthn.get","challenge":"I59Z7VXnN8dxR89VrQwbAwttfudIp0JpUvm4UtWpNeU","origin":"http://localhost:3000","crossOrigin":false}"#;

//     let payload = [authenticator_data_bytes, sha256(client_data_json)].concat();

//     public_key.verify(&payload, &signature).unwrap();
// }

// SubjectPublicKeyInfo {
//     algorithm: AlgorithmIdentifier {
//         oid: ObjectIdentifier(1.2.840.10045.2.1),
//         parameters: Some(
//             Any {
//                 tag: Tag(0x06: OBJECT IDENTIFIER),
//                 value: BytesOwned {
//                     length: Length(
//                         8,
//                     ),
//                     inner: [
//                         42,
//                         134,
//                         72,
//                         206,
//                         61,
//                         3,
//                         1,
//                         7,
//                     ],
//                 },
//             },
//         ),
//     },
//     subject_public_key: BitString {
//         unused_bits: 0,
//         bit_length: 520,
//         inner: [
//             4,
//             62,
//             222,
//             102,
//             67,
//             0,
//             54,
//             85,
//             18,
//             94,
//             101,
//             171,
//             73,
//             237,
//             49,
//             135,
//             106,
//             221,
//             235,
//             53,
//             134,
//             172,
//             65,
//             3,
//             87,
//             130,
//             71,
//             23,
//             240,
//             48,
//             137,
//             211,
//             182,
//             139,
//             62,
//             193,
//             68,
//             32,
//             85,
//             98,
//             6,
//             213,
//             99,
//             53,
//             167,
//             102,
//             16,
//             212,
//             177,
//             47,
//             93,
//             229,
//             232,
//             151,
//             92,
//             218,
//             65,
//             78,
//             252,
//             5,
//             115,
//             29,
//             247,
//             153,
//             19,
//         ],
//     },
// }
