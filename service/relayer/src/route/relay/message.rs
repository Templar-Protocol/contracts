use axum::{http::StatusCode, response::IntoResponse, Json};
use near_primitives::{action::delegate::SignedDelegateAction, hash::CryptoHash};
use near_sdk::serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(crate = "near_sdk::serde")]
pub struct RelayRequest {
    #[serde(with = "with_borsh_base64")]
    pub signed_delegate_action: SignedDelegateAction,
}

mod with_borsh_base64 {
    use near_sdk::base64::prelude::*;
    use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
    use near_sdk::serde::{de, ser, Deserialize, Deserializer, Serializer};

    pub fn deserialize<'a, D, T>(deserializer: D) -> Result<T, D::Error>
    where
        D: Deserializer<'a>,
        T: BorshDeserialize,
    {
        let s = <&str>::deserialize(deserializer)?;
        let bytes = BASE64_STANDARD.decode(s).map_err(de::Error::custom)?;
        borsh::from_slice::<T>(&bytes).map_err(de::Error::custom)
    }

    pub fn serialize<S, T>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: BorshSerialize,
    {
        let bytes = borsh::to_vec(value).map_err(ser::Error::custom)?;
        let s = BASE64_STANDARD.encode(bytes);
        serializer.serialize_str(&s)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub enum RelayResponse {
    Success { transaction_hash: CryptoHash },
    Failure { error: String },
    Rejected { reason: String },
}

impl IntoResponse for RelayResponse {
    fn into_response(self) -> axum::response::Response {
        let status_code = match self {
            RelayResponse::Success { .. } => StatusCode::OK,
            RelayResponse::Failure { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            RelayResponse::Rejected { .. } => StatusCode::BAD_REQUEST,
        };
        (status_code, Json(self)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use near_crypto::{PublicKey, Signature};
    use near_primitives::action::delegate::DelegateAction;
    use near_sdk::serde_json;

    use super::*;

    #[test]
    fn encoding() {
        let r = RelayRequest {
            signed_delegate_action: SignedDelegateAction {
                delegate_action: DelegateAction {
                    sender_id: "sender_id".parse().unwrap(),
                    receiver_id: "receiver_id".parse().unwrap(),
                    actions: vec![],
                    nonce: 88888,
                    max_block_height: 99999,
                    public_key: PublicKey::empty(near_crypto::KeyType::ED25519),
                },
                signature: Signature::empty(near_crypto::KeyType::ED25519),
            },
        };

        let s = serde_json::to_string_pretty(&r).unwrap();

        println!("{s}");

        let r2: RelayRequest = serde_json::from_str(&s).unwrap();

        assert_eq!(r, r2);
    }
}
