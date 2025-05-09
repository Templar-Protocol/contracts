use near_sdk::{near, AccountId, Gas, Promise};

use crate::{
    oracle::pyth::{ext_pyth, OracleResponse, PriceIdentifier},
    price::PricePair,
};

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub struct BalanceOracleConfiguration {
    pub account_id: AccountId,
    pub collateral_asset_price_id: PriceIdentifier,
    pub collateral_asset_decimals: i32,
    pub borrow_asset_price_id: PriceIdentifier,
    pub borrow_asset_decimals: i32,
    pub price_maximum_age_s: u32,
}

impl BalanceOracleConfiguration {
    // Usually seems to take 1.64 TGas.
    pub const GAS_RETRIEVE_PRICE_PAIR: Gas = Gas::from_tgas(3);

    pub fn retrieve_price_pair(&self) -> Promise {
        ext_pyth::ext(self.account_id.clone())
            .with_static_gas(Self::GAS_RETRIEVE_PRICE_PAIR)
            .list_ema_prices_no_older_than(
                vec![self.borrow_asset_price_id, self.collateral_asset_price_id],
                u64::from(self.price_maximum_age_s),
            )
    }

    /// # Errors
    ///
    /// If the response from the oracle does not contain valid prices for the
    /// configured asset pair.
    pub fn create_price_pair(
        &self,
        oracle_response: &OracleResponse,
    ) -> Result<PricePair, error::RetrievalError> {
        Ok(PricePair::new(
            oracle_response
                .get(&self.collateral_asset_price_id)
                .and_then(|o| o.as_ref())
                .ok_or(error::RetrievalError::MissingPrice)?,
            self.collateral_asset_decimals,
            oracle_response
                .get(&self.borrow_asset_price_id)
                .and_then(|o| o.as_ref())
                .ok_or(error::RetrievalError::MissingPrice)?,
            self.borrow_asset_decimals,
        )?)
    }
}

pub mod error {
    use thiserror::Error;

    #[derive(Clone, Debug, Error)]
    #[error("Error retrieving price: {0}")]
    pub enum RetrievalError {
        #[error("Missing price")]
        MissingPrice,
        #[error(transparent)]
        PriceData(#[from] crate::price::error::PriceDataError),
    }
}
