use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use near_sdk::{
    serde::{Deserialize, Serialize},
    AccountId,
};

use crate::app::App;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct GetAllowanceRequest {
    account_id: AccountId,
}

/// Handler for the `GET /get_allowance` endpoint.
pub async fn get_allowance(
    State(app): State<App>,
    Query(GetAllowanceRequest { account_id }): Query<GetAllowanceRequest>,
) -> Response {
    let Ok(allowance) = app.database.get_available_allowance(&account_id).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };

    Json(allowance.unwrap_or(app.args.relay.starting_allowance_yocto)).into_response()
}
