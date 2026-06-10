use near_sdk::{
    json_types::{Base58CryptoHash, U64},
    near,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "rpc", derive(clap::ValueEnum))]
#[near(serializers = [json, borsh])]
pub enum DeployMode {
    Normal,
    GlobalHash,
}

impl std::fmt::Display for DeployMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeployMode::Normal => write!(f, "Normal"),
            DeployMode::GlobalHash => write!(f, "GlobalHash"),
        }
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub struct Deployment {
    pub version_key: String,
    pub code_hash: Base58CryptoHash,
    pub block_height: U64,
}
