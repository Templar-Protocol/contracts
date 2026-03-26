use axum::{extract::State, Json};
use near_sdk::{
    serde::{Deserialize, Serialize},
    AccountId,
};

use crate::{app::App, route::SimpleResponse, ViewMarketPrices};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct GetMarketPricesRequest {
    pub market_id: AccountId,
}

#[tracing::instrument(name = "get_market_prices", skip(app))]
pub async fn get_market_prices(
    State(app): State<App>,
    Json(GetMarketPricesRequest { market_id }): Json<GetMarketPricesRequest>,
) -> SimpleResponse<ViewMarketPrices> {
    let market = {
        let accounts = app.accounts.read().await;
        let Some(market) = accounts.market_data.get(&market_id).cloned() else {
            return SimpleResponse::Rejected {
                reason: format!("Unknown market: {market_id}"),
            };
        };
        market
    };

    let market_prices = match app.relay_near.load_market_prices(&market).await {
        Ok(p) => p,
        Err(error) => {
            tracing::error!(%error, "Failed to load market prices");
            return SimpleResponse::Failure {
                error: "Failed to load market prices".to_string(),
            };
        }
    };

    SimpleResponse::success(market_prices)
}
