use std::str::FromStr;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing, Json, Router,
};
use near_sdk::{
    json_types::U64,
    serde::{Deserialize, Serialize},
    AccountId,
};
use sha2::{Digest, Sha256};
use templar_universal_account::{
    authentication::{ed25519_raw::Ed25519RawKey, passkey::Passkey},
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
}

pub async fn index(State(app): State<App>) -> impl IntoResponse {
    Json(Index {
        executor_id: app.args.ua.account_id,
        registry_id: app.args.ua.registry_id,
        pow_difficulty: app.args.ua.pow_difficulty,
        blockref_max_age_secs: app.args.ua.blockref_max_age.as_secs().into(),
    })
}

// We cannot use the KeyId type directly because this deserializes from the
// query string so it needs to use #[serde(tag = "...")]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "near_sdk::serde", tag = "type")]
pub enum KeyQuery {
    Passkey { key: Passkey },
    Ed25519Raw { key: Ed25519RawKey },
}

impl From<KeyQuery> for KeyId {
    fn from(value: KeyQuery) -> Self {
        match value {
            KeyQuery::Passkey { key } => key.into(),
            KeyQuery::Ed25519Raw { key } => key.into(),
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

    use super::*;

    #[rstest]
    #[case(
        Passkey("p256:NKzTCoSPccskQudsdyjoKyMLXTC6GQ9WYwsV9SJebAdb1gzbZEQcfwo4nikCWMHGBAXCFGCD5EZcPnqDFxdjzdDJ".parse().unwrap()).into(),
        "549bca2d5a64",
    )]
    #[case(
        Ed25519RawKey("ed25519:DWYRtzDDtbX63hcXziJEXgXZamSPQT61YPFGM1oFTqVp".parse().unwrap()).into(),
        "e2cac2be8cef",
    )]
    #[test]
    fn account_slug_regression(#[case] key: KeyId, #[case] expected_slug: &str) {
        assert_eq!(public_key_to_account_id_slug(&key), expected_slug);
    }
}
