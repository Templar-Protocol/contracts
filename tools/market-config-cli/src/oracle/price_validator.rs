use crate::{rpc::price_feed_exists, CliError, CliResult};
use near_jsonrpc_client::JsonRpcClient;
use near_sdk::AccountId;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_common::utils::Network;

pub struct PriceValidator {
    client: JsonRpcClient,
}

impl PriceValidator {
    pub fn new(network: Network) -> Self {
        let rpc_url = network.rpc_url();
        Self {
            client: JsonRpcClient::connect(rpc_url),
        }
    }

    /// Validate that a price feed exists on the oracle contract
    /// # Errors
    pub async fn validate_price_feed(
        &self,
        oracle_contract_id: AccountId,
        price_id: &PriceIdentifier,
    ) -> CliResult {
        let exists = price_feed_exists(&self.client, oracle_contract_id.clone(), price_id).await?;

        let str = serde_json::to_string(&price_id).unwrap_or_default();
        let price_hash = format!("0x{}", &str[1..str.len() - 1]);

        if !exists {
            return Err(CliError::Oracle(format!(
                "Price feed {price_hash} does not exist on oracle contract {oracle_contract_id}"
            )));
        }

        Ok(())
    }
}
