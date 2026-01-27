use near_sdk::AccountId;
use near_sdk_contract_tools::Nep297;
use redstone_common::PriceData;

#[derive(Clone, Debug, Nep297)]
#[nep297(standard = "redstone-adapter", version = "1.0.0")]
#[near_sdk::near(serializers = [json])]
pub struct WritePrices {
    pub updater: AccountId,
    pub updated_feeds: Vec<PriceData>,
}
