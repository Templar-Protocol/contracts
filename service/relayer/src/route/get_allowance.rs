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
#[tracing::instrument(
    name = "get_allowance",
    skip(app),
    fields(account_id = %account_id)
)]
pub async fn get_allowance(
    State(app): State<App>,
    Query(GetAllowanceRequest { account_id }): Query<GetAllowanceRequest>,
) -> Response {
    tracing::debug!("Fetching allowance for account");

    let Ok(allowance) = app.database.get_available_allowance(&account_id).await else {
        tracing::error!("Database error fetching allowance");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };

    let result = allowance.unwrap_or(app.args.relay.starting_allowance_yocto);
    tracing::debug!(allowance = %result, "Allowance retrieved");

    Json(result).into_response()
}
