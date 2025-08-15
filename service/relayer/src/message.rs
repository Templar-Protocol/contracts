use axum::{http::StatusCode, Json};
use near_primitives::{action::delegate::SignedDelegateAction, views::FinalExecutionOutcomeView};
use near_sdk::serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(crate = "near_sdk::serde")]
pub struct RelayRequest {
    #[serde(with = "with_base64")]
    pub signed_delegate_action: SignedDelegateAction,
}

mod with_base64 {
    use near_sdk::base64::engine::{general_purpose::STANDARD as BASE64, Engine};
    use near_sdk::serde::de::DeserializeOwned;
    use near_sdk::serde::{de, ser, Deserialize, Deserializer, Serialize, Serializer};
    use near_sdk::serde_json;

    pub fn deserialize<'a, D, T>(deserializer: D) -> Result<T, D::Error>
    where
        D: Deserializer<'a>,
        T: DeserializeOwned,
    {
        let s = <&str>::deserialize(deserializer)?;
        let bytes = BASE64.decode(s).map_err(de::Error::custom)?;
        serde_json::from_slice::<T>(&bytes).map_err(de::Error::custom)
    }

    pub fn serialize<S, T>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: Serialize,
    {
        let bytes = serde_json::to_vec(value).map_err(ser::Error::custom)?;
        let s = BASE64.encode(bytes);
        serializer.serialize_str(&s)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub enum RelayResponse {
    Success {
        execution: Box<FinalExecutionOutcomeView>,
    },
    Failure {
        error: String,
    },
    Rejected {
        reason: String,
    },
}

#[allow(clippy::needless_pass_by_value)]
impl RelayResponse {
    pub fn failure(e: impl ToString) -> (StatusCode, Json<RelayResponse>) {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(Self::Failure {
                error: e.to_string(),
            }),
        )
    }

    pub fn rejected(r: impl ToString) -> (StatusCode, Json<RelayResponse>) {
        (
            StatusCode::BAD_REQUEST,
            Json(Self::Rejected {
                reason: r.to_string(),
            }),
        )
    }

    pub fn success(execution: FinalExecutionOutcomeView) -> (StatusCode, Json<RelayResponse>) {
        (
            StatusCode::OK,
            Json(Self::Success {
                execution: Box::new(execution),
            }),
        )
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
