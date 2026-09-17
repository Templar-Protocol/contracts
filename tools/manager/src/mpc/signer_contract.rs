//! Wire types of the NEAR MPC signer contract (`v1.signer`): the `sign` and
//! `derived_public_key` arguments, and the `SignatureResponse` a `sign` receipt
//! returns.

use anyhow::Context as _;
use clap::ValueEnum;
use near_account_id::AccountId;
use near_api::types::{crypto::KeyType as NearKeyType, PublicKey, Signature};
use serde::{Deserialize, Serialize};

/// The curve of the derived key, which the contract calls a signing domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyType {
    Ed25519,
    Secp256k1,
}

impl KeyType {
    pub const fn domain_id(self) -> u64 {
        match self {
            Self::Secp256k1 => 0,
            Self::Ed25519 => 1,
        }
    }

    pub fn from_domain_id(domain_id: u64) -> anyhow::Result<Self> {
        match domain_id {
            0 => Ok(Self::Secp256k1),
            1 => Ok(Self::Ed25519),
            other => anyhow::bail!("unsupported MPC signing domain {other}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedPublicKeyArgs {
    pub path: String,
    pub predecessor: AccountId,
    pub domain_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignArgs {
    pub request: SignRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignRequest {
    #[serde(rename = "payload_v2")]
    pub payload: Payload,
    pub path: String,
    pub domain_id: u64,
}

impl SignRequest {
    /// The signing domain, which the payload variant must agree with: the
    /// contract rejects a mismatch, so a request that passes review here must
    /// not fail there.
    pub fn key_type(&self) -> anyhow::Result<KeyType> {
        let key_type = KeyType::from_domain_id(self.domain_id)?;
        let expected = match self.payload {
            Payload::Ecdsa(_) => KeyType::Secp256k1,
            Payload::Eddsa(_) => KeyType::Ed25519,
        };
        anyhow::ensure!(
            key_type == expected,
            "the sign request names domain {} ({key_type:?}) but carries a {expected:?} payload",
            self.domain_id
        );
        Ok(key_type)
    }
}

/// The message the MPC signs. Both variants carry a NEAR 32-byte hash here;
/// `Eddsa` is a variable-length message on the wire, so the hash is checked on
/// the way out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Payload {
    Ecdsa(#[serde(with = "hex::serde")] [u8; 32]),
    Eddsa(#[serde(with = "hex::serde")] Vec<u8>),
}

impl Payload {
    pub fn for_hash(key_type: KeyType, hash: [u8; 32]) -> Self {
        match key_type {
            KeyType::Secp256k1 => Self::Ecdsa(hash),
            KeyType::Ed25519 => Self::Eddsa(hash.to_vec()),
        }
    }

    pub fn hash(&self) -> anyhow::Result<[u8; 32]> {
        match self {
            Self::Ecdsa(hash) => Ok(*hash),
            Self::Eddsa(message) => <[u8; 32]>::try_from(message.as_slice()).map_err(|_| {
                anyhow::anyhow!(
                    "the proposal's Eddsa payload is {} bytes, not a 32-byte NEAR hash",
                    message.len()
                )
            }),
        }
    }
}

/// The value a successful `sign` receipt returns.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "scheme")]
pub enum SignatureResponse {
    Ed25519 {
        signature: Vec<u8>,
    },
    Secp256k1 {
        big_r: AffinePoint,
        s: Scalar,
        recovery_id: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AffinePoint {
    /// SEC1 compressed point: one prefix byte then the 32-byte x coordinate.
    #[serde(with = "hex::serde")]
    pub affine_point: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Scalar {
    #[serde(with = "hex::serde")]
    pub scalar: [u8; 32],
}

impl SignatureResponse {
    /// The NEAR signature: 64 bytes for ed25519; `r ‖ s ‖ recovery_id` for secp256k1.
    pub fn into_signature(self) -> anyhow::Result<Signature> {
        match self {
            Self::Ed25519 { signature } => Signature::from_parts(NearKeyType::ED25519, &signature)
                .context("MPC returned a malformed ed25519 signature"),
            Self::Secp256k1 {
                big_r,
                s,
                recovery_id,
            } => {
                let r = big_r
                    .affine_point
                    .get(1..33)
                    .context("MPC returned a malformed secp256k1 big_r")?;
                let mut bytes = Vec::with_capacity(65);
                bytes.extend_from_slice(r);
                bytes.extend_from_slice(&s.scalar);
                bytes.push(recovery_id);
                Signature::from_parts(NearKeyType::SECP256K1, &bytes)
                    .context("MPC returned a malformed secp256k1 signature")
            }
        }
    }
}

/// Parse the `derived_public_key` view's return value.
pub fn parse_public_key(value: &serde_json::Value) -> anyhow::Result<PublicKey> {
    let text = value
        .as_str()
        .context("derived_public_key did not return a string")?;
    text.parse()
        .map_err(|error| anyhow::anyhow!("derived_public_key returned `{text}`: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_api::types::CryptoHash;
    use rstest::rstest;

    /// The real `v1.signer` response inside mainnet tx
    /// `FvP7is2jBG96ni1RueGSY1sB5MHyRc4Sp7arm9WTfWS9` (shape only; bytes are illustrative).
    #[test]
    fn ed25519_response_parses_from_a_json_array() {
        let bytes: Vec<u8> = (0..64).collect();
        let json = serde_json::json!({ "scheme": "Ed25519", "signature": bytes });
        let response: SignatureResponse = serde_json::from_value(json).expect("parses");
        let signature = response.into_signature().expect("64 bytes");
        assert!(matches!(signature.key_type(), NearKeyType::ED25519));
    }

    #[test]
    fn secp256k1_response_strips_the_point_prefix() {
        let json = serde_json::json!({
            "scheme": "Secp256k1",
            "big_r": { "affine_point": format!("02{}", "11".repeat(32)) },
            "s": { "scalar": "22".repeat(32) },
            "recovery_id": 1,
        });
        let response: SignatureResponse = serde_json::from_value(json).expect("parses");
        let signature = response.into_signature().expect("65 bytes");
        let encoded = borsh::to_vec(&signature).expect("borsh");
        // Tag byte, then r, s, recovery id.
        assert_eq!(encoded[0], 1);
        assert_eq!(&encoded[1..33], &[0x11; 32]);
        assert_eq!(&encoded[33..65], &[0x22; 32]);
        assert_eq!(encoded[65], 1);
    }

    #[test]
    fn a_wrong_length_ed25519_signature_is_rejected() {
        let response = SignatureResponse::Ed25519 {
            signature: vec![0; 63],
        };
        assert!(response.into_signature().is_err());
    }

    #[rstest]
    #[case(KeyType::Ed25519, 1)]
    #[case(KeyType::Secp256k1, 0)]
    fn domain_ids_round_trip(#[case] key_type: KeyType, #[case] domain_id: u64) {
        assert_eq!(key_type.domain_id(), domain_id);
        assert_eq!(KeyType::from_domain_id(domain_id).expect("known"), key_type);
    }

    #[rstest]
    #[case::ed25519(KeyType::Ed25519, 1, true)]
    #[case::secp256k1(KeyType::Secp256k1, 0, true)]
    #[case::eddsa_in_the_ecdsa_domain(KeyType::Ed25519, 0, false)]
    #[case::ecdsa_in_the_eddsa_domain(KeyType::Secp256k1, 1, false)]
    fn a_request_must_pair_its_payload_with_its_domain(
        #[case] payload_for: KeyType,
        #[case] domain_id: u64,
        #[case] accepted: bool,
    ) {
        let request = SignRequest {
            payload: Payload::for_hash(payload_for, [1; 32]),
            path: "p".to_owned(),
            domain_id,
        };
        assert_eq!(request.key_type().is_ok(), accepted);
    }

    #[test]
    fn sign_args_match_the_contract_wire_shape() {
        let hash = [0xab; 32];
        let args = SignArgs {
            request: SignRequest {
                payload: Payload::for_hash(KeyType::Ed25519, hash),
                path: "dao.near-target.near".to_owned(),
                domain_id: 1,
            },
        };
        assert_eq!(
            serde_json::to_value(&args).expect("serializes"),
            serde_json::json!({
                "request": {
                    "payload_v2": { "Eddsa": "ab".repeat(32) },
                    "path": "dao.near-target.near",
                    "domain_id": 1,
                }
            })
        );
        assert_eq!(args.request.payload.hash().expect("32 bytes"), hash);
    }

    #[test]
    fn a_locally_signed_hash_verifies_like_an_mpc_signature() {
        let secret = near_api::signer::generate_secret_key().expect("key");
        let hash = CryptoHash([7; 32]);
        let response = SignatureResponse::Ed25519 {
            signature: borsh::to_vec(&secret.sign(hash)).expect("borsh")[1..].to_vec(),
        };
        let signature = response.into_signature().expect("signature");
        assert!(signature.verify(hash, secret.public_key()));
    }
}
