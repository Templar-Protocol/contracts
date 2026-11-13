use clap::Args;
use near_account_id::AccountId;
use templar_common::oracle::pyth::PriceIdentifier;

use super::parse_price_identifier;
use crate::resolve::OracleTarget;

/// Predict the complete raw-storage effect of one manual-trip operation locally.
/// This command never signs, submits, or changes the oracle.
#[derive(Args, Debug)]
pub struct PredictManualTrip {
    #[command(flatten)]
    pub(crate) target: OracleTarget,
    #[arg(long, value_name = "HEX", value_parser = parse_price_identifier)]
    pub(crate) price_id: PriceIdentifier,
    /// Governance account that will call the oracle.
    #[arg(long)]
    pub(crate) actor: AccountId,
    /// Set the manual trip; omit to clear it.
    #[arg(long)]
    pub(crate) tripped: bool,
    /// Exact manual-trip event metadata, base64 encoded.
    #[arg(long)]
    pub(crate) metadata_base64: String,
}
