use near_sdk::{json_types::U128, near, AccountId, PanicOnDefault};

#[derive(Clone)]
#[near(serializers = [json, borsh])]
pub struct PoolInfo {
    pub token_account_ids: Vec<AccountId>,
    pub shares_total_supply: U128,
}

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    pools: Vec<PoolInfo>,
}

#[near]
impl Contract {
    #[init]
    pub fn new(pools: Vec<PoolInfo>) -> Self {
        Self { pools }
    }

    pub fn get_pools(&self, from_index: Option<u64>, limit: Option<u64>) -> Vec<PoolInfo> {
        let from_index = from_index.unwrap_or(0) as usize;
        let limit = limit.unwrap_or(u64::MAX) as usize;
        self.pools
            .iter()
            .skip(from_index)
            .take(limit)
            .cloned()
            .collect()
    }
}
