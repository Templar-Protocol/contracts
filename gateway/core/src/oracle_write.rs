use near_account_id::AccountId;
use near_sdk::json_types::Base64VecU8;
use templar_common::oracle::redstone;
use templar_gateway_types::{ManagedAccountId, NearToken};

use crate::{
    client::{
        pyth_oracle::UpdatePriceFeedsArgs, redstone_oracle::WritePricesArgs, ContractWriteOptions,
        NearClient,
    },
    GatewayResult, PlannedTransaction,
};

const PYTH_UPDATE_DEPOSIT: NearToken = NearToken::from_yoctonear(10_000_000_000_000_000_000_000);

pub fn plan_pyth_update(
    near_client: &NearClient,
    signer_account_id: ManagedAccountId,
    oracle_id: AccountId,
    vaa: Vec<u8>,
) -> GatewayResult<PlannedTransaction> {
    near_client.pyth_oracle(oracle_id).update_price_feeds(
        ContractWriteOptions::new(signer_account_id)
            .tgas(300)
            .deposit(PYTH_UPDATE_DEPOSIT),
        UpdatePriceFeedsArgs {
            data: hex::encode(vaa),
        },
    )
}

pub fn plan_redstone_write_prices(
    near_client: &NearClient,
    signer_account_id: ManagedAccountId,
    oracle_id: AccountId,
    feed_ids: Vec<redstone::FeedId>,
    payload: Vec<u8>,
) -> GatewayResult<PlannedTransaction> {
    near_client.redstone_oracle(oracle_id).write_prices(
        ContractWriteOptions::new(signer_account_id).tgas(300),
        WritePricesArgs {
            feed_ids,
            payload: Base64VecU8(payload),
        },
    )
}
