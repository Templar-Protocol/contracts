use axum::{extract::State, Json};
use near_primitives::hash::CryptoHash;
use near_sdk::serde::{Deserialize, Serialize};

use crate::{app::App, route::SimpleResponse};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub enum RelayRequest {
    Passkey,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct RelayResponse {
    pub transaction_hash: CryptoHash,
}

pub async fn relay(
    State(app): State<App>,
    Json(request): Json<RelayRequest>,
) -> SimpleResponse<RelayResponse> {
    todo!()
}
