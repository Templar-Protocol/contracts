use std::{collections::HashSet, hash::BuildHasher};

use axum::{http::StatusCode, response::IntoResponse, Json};
use near_sdk::{
    serde::{Deserialize, Serialize},
    AccountId,
};

pub mod get_allowance;
pub mod get_market_prices;
pub mod relay;
pub mod universal_account;
pub mod update_prices;

/// Why a set of requested market IDs is not serviceable by the relayer.
#[derive(Debug, PartialEq, Eq)]
pub enum MarketIdRejection {
    Empty,
    Unknown(AccountId),
}

impl MarketIdRejection {
    pub fn reason(&self) -> String {
        match self {
            Self::Empty => "market_ids must not be empty".to_string(),
            Self::Unknown(market_id) => format!("Unknown market: {market_id}"),
        }
    }
}

/// Validate requested market IDs against the relayer's known/allowlisted set —
/// the shared policy boundary for `/update_prices` and `/get_market_prices`.
/// An empty request or any unknown market is rejected.
pub fn validate_market_ids<'a, S: BuildHasher>(
    requested: impl IntoIterator<Item = &'a AccountId>,
    known: &HashSet<AccountId, S>,
) -> Result<(), MarketIdRejection> {
    let mut empty = true;
    for market_id in requested {
        empty = false;
        if !known.contains(market_id) {
            return Err(MarketIdRejection::Unknown(market_id.clone()));
        }
    }
    if empty {
        return Err(MarketIdRejection::Empty);
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub enum SimpleResponse<T> {
    Success(T),
    Failure { error: String },
    Rejected { reason: String },
}

impl<T> From<T> for SimpleResponse<T> {
    fn from(value: T) -> Self {
        SimpleResponse::Success(value)
    }
}

impl<T> SimpleResponse<T> {
    pub fn success(value: T) -> Self {
        Self::Success(value)
    }
}

impl<T: Serialize> IntoResponse for SimpleResponse<T> {
    fn into_response(self) -> axum::response::Response {
        let status_code = match self {
            Self::Success { .. } => StatusCode::OK,
            Self::Failure { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Rejected { .. } => StatusCode::BAD_REQUEST,
        };
        (status_code, Json(self)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&str]) -> Vec<AccountId> {
        list.iter().map(|id| id.parse().unwrap()).collect()
    }

    fn known(list: &[&str]) -> HashSet<AccountId> {
        list.iter().map(|id| id.parse().unwrap()).collect()
    }

    #[test]
    fn empty_request_is_rejected() {
        let error =
            validate_market_ids(std::iter::empty::<&AccountId>(), &known(&["a.near"])).unwrap_err();
        assert_eq!(error, MarketIdRejection::Empty);
        assert_eq!(error.reason(), "market_ids must not be empty");
    }

    #[test]
    fn unknown_market_is_rejected() {
        let requested = ids(&["unknown-market.test.near"]);
        let error = validate_market_ids(requested.iter(), &known(&["a.near"])).unwrap_err();
        assert_eq!(
            error,
            MarketIdRejection::Unknown("unknown-market.test.near".parse().unwrap()),
        );
        assert_eq!(error.reason(), "Unknown market: unknown-market.test.near");
    }

    #[test]
    fn all_known_markets_pass() {
        let requested = ids(&["a.near", "b.near"]);
        validate_market_ids(requested.iter(), &known(&["a.near", "b.near", "c.near"])).unwrap();
    }
}
