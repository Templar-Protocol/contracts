use near_sdk::{
    serde::{Deserialize, Serialize},
    NearToken,
};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct Configuration {
    pub allowed_methods: Vec<String>,
    pub starting_allowance_yocto: NearToken,
    pub cache: CacheConfiguration,
    pub broom: BroomConfiguration,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct CacheConfiguration {
    pub gas_price_secs: u64,
    pub nonce_secs: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct BroomConfiguration {
    pub batch_size: u32,
    pub interval_secs: u64,
}
