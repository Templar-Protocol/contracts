use std::collections::HashSet;

use axum::{extract::State, Json};
use near_sdk::{
    serde::{Deserialize, Serialize},
    AccountId,
};

use crate::{app::App, route::SimpleResponse};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct UpdatePricesRequest {
    pub market_ids: Vec<AccountId>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct UpdatePricesResponse {
    pub market_ids: Vec<AccountId>,
}

#[tracing::instrument(name = "update_prices", skip(app), fields(market_count = tracing::field::Empty))]
pub async fn update_prices(
    State(app): State<App>,
    Json(UpdatePricesRequest { market_ids }): Json<UpdatePricesRequest>,
) -> SimpleResponse<UpdatePricesResponse> {
    let market_ids: HashSet<_> = market_ids.into_iter().collect();
    tracing::Span::current().record("market_count", market_ids.len());
    tracing::info!(market_ids = ?market_ids, "Processing price update request");

    if market_ids.is_empty() {
        tracing::info!("Rejecting empty price update request");
        return SimpleResponse::Rejected {
            reason: "market_ids must not be empty".to_string(),
        };
    }

    let accounts = app.accounts.read().await.clone();
    if let Some(market_id) = market_ids
        .iter()
        .find(|market_id| !accounts.market_ids.contains(*market_id))
    {
        tracing::info!(%market_id, "Rejecting unknown market in price update request");
        return SimpleResponse::Rejected {
            reason: format!("Unknown market: {market_id}"),
        };
    }

    if let Err(error) = app.update_market_prices(&market_ids).await {
        tracing::error!(%error, market_ids = ?market_ids, "Price update request failed");
        return SimpleResponse::Failure {
            error: error.to_string(),
        };
    }

    let mut market_ids = market_ids.into_iter().collect::<Vec<_>>();
    market_ids.sort_unstable();
    tracing::info!(market_ids = ?market_ids, "Price update request completed");

    UpdatePricesResponse { market_ids }.into()
}
