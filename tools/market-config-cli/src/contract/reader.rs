use crate::{rpc::view, CliError, CliResult};
use near_jsonrpc_client::{methods::query::RpcQueryRequest, JsonRpcClient};
use near_primitives::types::{AccountId, Finality};
use near_sdk::serde_json::json;
use templar_common::{market::MarketConfiguration, utils::Network};

pub struct ContractReader {
    client: JsonRpcClient,
}

impl ContractReader {
    /// # Errors
    pub fn new(network: Network) -> CliResult<Self> {
        let rpc_url = network.rpc_url();
        Ok(Self {
            client: JsonRpcClient::connect(rpc_url),
        })
    }

    /// # Errors
    pub fn from_rpc_url(rpc_url: &str) -> CliResult<Self> {
        Ok(Self {
            client: JsonRpcClient::connect(rpc_url),
        })
    }

    /// Read the market configuration from a deployed contract
    /// # Errors
    pub async fn read_config(&self, contract_id: AccountId) -> CliResult<MarketConfiguration> {
        if !self.contract_exists(contract_id.clone()).await? {
            return Err(CliError::Contract(format!(
                "Contract {contract_id} does not exist or is not accessible",
            )));
        }

        let configuration: MarketConfiguration =
            view(&self.client, contract_id, "get_configuration", json!({})).await?;
        Ok(configuration)
    }

    /// Check if a contract exists and is accessible
    /// # Errors
    pub async fn contract_exists(&self, contract_id: AccountId) -> CliResult<bool> {
        let request = RpcQueryRequest {
            block_reference: Finality::Final.into(),
            request: near_primitives::views::QueryRequest::ViewAccount {
                account_id: contract_id,
            },
        };

        match self.client.call(request).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_contract_exists() {
        let reader = ContractReader::new(Network::Testnet).expect("reader should construct");
        // Known testnet account
        let account: AccountId = "templar-in-training.testnet".parse().unwrap();
        let exists = reader.contract_exists(account.clone()).await.unwrap();
        assert!(exists);

        // Likely non-existent account
        let fake_account: AccountId = "nonexistent-account-xyz.testnet".parse().unwrap();
        let not_exists = reader.contract_exists(fake_account).await.unwrap();
        assert!(!not_exists);
    }
}
