//! Event topics and payloads emitted by the custodial adapter.
//!
//! Events are the append-only audit record for accepted custodial
//! valuation reports. They are published only after every authorization,
//! domain-binding, sequence, and payload check has succeeded; rejected
//! submissions produce no adapter event for the report itself.
//!
//! The accepted-report event topic is `("val_rep", asset)` and its
//! payload is `(sequence, as_of, submitted_at, assets_value,
//! report_hash)`, where `report_hash` is the optional fixed-size
//! integrity digest supplied by the reporter, carried for audit only.

use soroban_sdk::{symbol_short, Address, BytesN, Env};

/// Publish the accepted-report event for a successfully persisted
/// custodial valuation report.
pub fn report_accepted(
    env: &Env,
    asset: &Address,
    sequence: u64,
    as_of: u64,
    submitted_at: u64,
    assets_value: i128,
    report_hash: Option<BytesN<32>>,
) {
    env.events().publish(
        (symbol_short!("val_rep"), asset.clone()),
        (sequence, as_of, submitted_at, assets_value, report_hash),
    );
}
