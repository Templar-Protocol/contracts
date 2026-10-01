use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::lazer::FeedDataResponse;
use templar_gateway_macros::MethodSpec;

/// Read stored feed data from a Pyth Lazer adapter.
///
/// Bulk, because that is the call the proxy oracle makes. The adapter stores and
/// serves; a consumer projects the raw data to a price itself.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "lazer.getFeedsData", output = FeedDataResponse)]
pub struct GetFeedsData {
    pub oracle_id: AccountId,
    pub feed_ids: Vec<u32>,
}
