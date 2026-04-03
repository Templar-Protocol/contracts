use axum::{http::StatusCode, response::IntoResponse, Json};
use near_sdk::serde::{Deserialize, Serialize};

pub mod get_allowance;
pub mod get_market_prices;
pub mod relay;
pub mod universal_account;
pub mod update_prices;

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
