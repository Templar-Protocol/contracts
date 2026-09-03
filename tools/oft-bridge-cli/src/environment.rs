use crate::{
    domain::{ChainIdentityV1, Environment},
    error::{Error, Result},
};

pub const STELLAR_TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";
pub const STELLAR_PUBLIC_PASSPHRASE: &str = "Public Global Stellar Network ; September 2015";
pub const STELLAR_TESTNET_EID: u32 = 40_600;
pub const STELLAR_MAINNET_EID: u32 = 30_600;
pub const SEPOLIA_EID: u32 = 40_161;
pub const ETHEREUM_EID: u32 = 30_101;
pub const STELLAR_TESTNET_ENDPOINT: &str =
    "CALTBA5S6GRJEHAXFP45LGGLKWWAF7HTZCPNUBUJF2HWWRRLQNV35AIV";
pub const STELLAR_MAINNET_ENDPOINT: &str =
    "CCQLLRE5JBAWYCW3KTWOIWLMFDUOKROQVZNSALQMGOSXNW3ERUOWTZGK";
pub const SEPOLIA_ENDPOINT: &str = "0x6EDCE65403992e310A62460808c4b910D972f10f";

pub fn classify(identity: &ChainIdentityV1) -> Result<Environment> {
    let environment = if identity.stellar_passphrase == STELLAR_TESTNET_PASSPHRASE
        && identity.stellar_eid == STELLAR_TESTNET_EID
        && identity.stellar_endpoint == STELLAR_TESTNET_ENDPOINT
        && identity.evm_chain_id == 11_155_111
        && identity.evm_eid == SEPOLIA_EID
        && identity.evm_endpoint.eq_ignore_ascii_case(SEPOLIA_ENDPOINT)
    {
        Environment::StellarTestnetSepolia
    } else if identity.stellar_passphrase == STELLAR_PUBLIC_PASSPHRASE
        || identity.stellar_eid == STELLAR_MAINNET_EID
        || identity.evm_chain_id == 1
        || identity.evm_eid == ETHEREUM_EID
    {
        if identity.stellar_passphrase == STELLAR_PUBLIC_PASSPHRASE
            && identity.stellar_eid == STELLAR_MAINNET_EID
            && identity.stellar_endpoint == STELLAR_MAINNET_ENDPOINT
            && identity.evm_chain_id == 1
            && identity.evm_eid == ETHEREUM_EID
        {
            Environment::StellarMainnetEthereum
        } else {
            return Err(Error::Policy("unknown_or_mixed_environment".into()));
        }
    } else {
        return Err(Error::Policy("unknown_environment".into()));
    };

    if identity.environment != environment {
        return Err(Error::Policy("requested_environment_mismatch".into()));
    }
    if identity.stellar_endpoint_code_hash.is_empty() || identity.evm_endpoint_code_hash.is_empty()
    {
        return Err(Error::InvalidInput(
            "endpoint code hashes are required for environment binding".into(),
        ));
    }
    Ok(environment)
}

pub fn require_testnet(identity: &ChainIdentityV1) -> Result<()> {
    if classify(identity)?.is_mainnet() {
        return Err(Error::Policy("production_mutation_unsupported_v1".into()));
    }
    Ok(())
}
