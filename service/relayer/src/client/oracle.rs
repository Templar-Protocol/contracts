use templar_gateway_core::GatewayError;
use templar_gateway_types::OperationStatus;

/// Failure modes when refreshing a market's oracle prices through the gateway's
/// `oracle.updatePrices` op and validating the result before relaying against it.
#[derive(thiserror::Error, Debug)]
pub enum UpdateError {
    #[error("gateway execution failed: {0}")]
    Gateway(#[from] GatewayError),
    #[error("oracle update operation {operation_id} ended with status {status:?}")]
    NotSucceeded {
        operation_id: String,
        status: OperationStatus,
    },
    #[error("failed to decode proxy update result: {0}")]
    ProxyResultDecode(near_sdk::serde_json::Error),
    #[error("proxy prices unavailable after update: {price_ids:?}")]
    ProxyPricesUnavailable {
        price_ids: Vec<templar_common::oracle::pyth::PriceIdentifier>,
    },
}
