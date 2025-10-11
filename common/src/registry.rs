use near_sdk::{
    json_types::{Base58CryptoHash, U64},
    near,
};

#[derive(Debug, Clone)]
#[near(serializers = [json, borsh])]
pub enum DeployMode {
    Normal,
    GlobalHash,
}

#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub struct Deployment {
    pub version_key: String,
    pub code_hash: Base58CryptoHash,
    pub block_height: U64,
}
