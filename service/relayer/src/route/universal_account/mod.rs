use std::str::FromStr;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing, Json, Router,
};
use near_sdk::{
    json_types::{U128, U64},
    serde::{Deserialize, Serialize},
    AccountId,
};
use sha2::{Digest, Sha256};
use templar_universal_account::{
    authentication::{
        ed25519::{eip191, raw, sep53},
        eip712, passkey,
    },
    KeyId,
};

use crate::app::App;

pub mod create;
pub mod pow;
pub mod relay;

pub const ACCOUNT_SLUG_LEN: usize = 12;

pub fn public_key_to_account_id_slug(public_key: &KeyId) -> String {
    hex::encode(&Sha256::digest(public_key.to_string())[0..ACCOUNT_SLUG_LEN / 2])
}

pub fn router() -> Router<App> {
    Router::<App>::new()
        .route("/", routing::get(index))
        .route("/account_id", routing::get(account_id))
        .route("/create", routing::post(create::create))
        .route("/relay", routing::post(relay::relay))
}

// pub fn

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct Index {
    /// The user must sign a payload that authorizes this account to execute creation actions.
    pub executor_id: AccountId,
    /// Universal accounts will be deployed as subaccounts of this account.
    pub registry_id: AccountId,
    /// The proof-of-work difficulty (number of leading binary zeros) for universal account creation requests.
    pub pow_difficulty: usize,
    /// The block hash that is included in the creation request must not be older than this many seconds.
    pub blockref_max_age_secs: U64,
    /// The chain ID that the UA must be deployed on and configured to.
    pub chain_id: U128,
    /// The version key of the UA contract that the relayer will deploy from the registry.
    pub version_key: String,
}

pub async fn index(State(app): State<App>) -> impl IntoResponse {
    Json(Index {
        executor_id: app.args.ua.account_id,
        registry_id: app.args.ua.registry_id,
        pow_difficulty: app.args.ua.pow_difficulty,
        blockref_max_age_secs: app.args.ua.blockref_max_age.as_secs().into(),
        chain_id: app.args.ua.chain_id.into(),
        version_key: app.args.ua.version_key.clone(),
    })
}

// We cannot use the KeyId type directly because this deserializes from the
// query string so it needs to use #[serde(tag = "...")]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "near_sdk::serde", tag = "type")]
pub enum KeyQuery {
    Passkey { key: passkey::VerifyKey },
    Ed25519Raw { key: raw::VerifyKey },
    Eip712 { key: eip712::VerifyKey },
    Sep53 { key: sep53::VerifyKey },
    Eip191 { key: eip191::VerifyKey },
}

impl From<KeyQuery> for KeyId {
    fn from(value: KeyQuery) -> Self {
        match value {
            KeyQuery::Passkey { key } => key.into(),
            KeyQuery::Ed25519Raw { key } => key.into(),
            KeyQuery::Eip712 { key } => key.into(),
            KeyQuery::Sep53 { key } => key.into(),
            KeyQuery::Eip191 { key } => key.into(),
        }
    }
}

// This implementation mainly exists just to ensure that the compiler reminds
// us to update this datatype as we add new key variants.
impl From<KeyId> for KeyQuery {
    fn from(value: KeyId) -> Self {
        match value {
            KeyId::Passkey(key) => Self::Passkey { key },
            KeyId::Ed25519Raw(key) => Self::Ed25519Raw { key },
            KeyId::Eip712(key) => Self::Eip712 { key },
            KeyId::Sep53(key) => Self::Sep53 { key },
            KeyId::Eip191(key) => Self::Eip191 { key },
        }
    }
}

pub async fn account_id(
    State(app): State<App>,
    Query(key_query): Query<KeyQuery>,
) -> impl IntoResponse {
    let account_id_slug = public_key_to_account_id_slug(&key_query.into());
    let registry_id = app.args.ua.registry_id;
    match AccountId::from_str(&format!("{account_id_slug}.{registry_id}")) {
        Ok(account_id) => (StatusCode::OK, account_id.to_string()),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use templar_universal_account::authentication::ed25519::eip191;

    use super::*;

    #[rstest]
    #[case::passkey(
        passkey::VerifyKey("p256:NKzTCoSPccskQudsdyjoKyMLXTC6GQ9WYwsV9SJebAdb1gzbZEQcfwo4nikCWMHGBAXCFGCD5EZcPnqDFxdjzdDJ".parse().unwrap()).into(),
        "549bca2d5a64",
    )]
    #[case::ed25519_raw(
        raw::VerifyKey("ed25519:DWYRtzDDtbX63hcXziJEXgXZamSPQT61YPFGM1oFTqVp".parse().unwrap()).into(),
        "e2cac2be8cef",
    )]
    #[case::sep53(
        sep53::VerifyKey("GBPNJTA5DARWSGLGPAGUHZBE44IOCFSKCL525WK7VZK6BEX4DPLIJXZ7".parse().unwrap()).into(),
        "3ccc6b968690",
    )]
    #[case::eip712(
        eip712::VerifyKey("0xa2E641CcbEB84c6Ed1e1E43e18B720F6D5C5173E".parse().unwrap()).into(),
        "8a98745b4d35",
    )]
    #[case::eip191(
        eip191::VerifyKey("0x03a607faedb00b3f9c747a9cb303255ef86a4da8".parse().unwrap()).into(),
        "6616024d8ced",
    )]
    #[test]
    fn account_slug_regression(#[case] key: KeyId, #[case] expected_slug: &str) {
        assert_eq!(public_key_to_account_id_slug(&key), expected_slug);
    }
}
