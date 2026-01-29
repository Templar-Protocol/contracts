use crate::{CliError, CliResult};
use near_sdk::AccountId;
use std::str::FromStr;
use templar_common::{
    asset::{AssetClass, FungibleAsset},
    oracle::pyth::PriceIdentifier,
};

/// # Errors
pub fn parse_asset_input<T: AssetClass>(value: &str, field: &str) -> CliResult<FungibleAsset<T>> {
    match value.parse::<FungibleAsset<T>>() {
        Ok(asset) => Ok(asset),
        Err(_) => AccountId::from_str(value)
            .map(FungibleAsset::nep141)
            .map_err(|e| CliError::InvalidInput(format!("Invalid {field}: {e}"))),
    }
}

/// Parse a price ID from a hex string
/// # Errors
pub fn parse_price_id(hex_string: &str) -> CliResult<PriceIdentifier> {
    let hex_string = hex_string.trim_start_matches("0x");

    if hex_string.len() != 64 {
        return Err(CliError::InvalidInput(
            "Price ID must be 64 hex characters (32 bytes)".into(),
        ));
    }

    let bytes = hex::decode(hex_string)
        .map_err(|e| CliError::InvalidInput(format!("Invalid hex string: {e}")))?;

    let mut array = [0u8; 32];
    array.copy_from_slice(&bytes);

    Ok(PriceIdentifier(array))
}

/// # Errors
pub fn price_id_from_input(value: &str) -> CliResult<PriceIdentifier> {
    parse_price_id(value.trim())
        .map_err(|e| CliError::InvalidInput(format!("Invalid price ID '{value}': {e}",)))
}
