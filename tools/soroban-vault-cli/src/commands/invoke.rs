//! Soroban argument encoding and command-output conversion helpers.

use crate::{
    stellar::CommandOutput,
    types::{AddressStr, SupplyQueueEntryArg},
};

use super::output::Response;

pub(super) fn supply_queue_entries_json(entries: &[SupplyQueueEntryArg]) -> anyhow::Result<String> {
    Ok(serde_json::to_string(entries)?)
}

pub(super) fn address_vec_json(addresses: &[AddressStr]) -> anyhow::Result<String> {
    Ok(serde_json::to_string(addresses)?)
}

pub(super) fn option_i128_arg(value: Option<i128>) -> String {
    value.map_or_else(|| "null".to_string(), |value| value.to_string())
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "keeps match arms uniform with fallible handlers"
)]
pub(super) fn invoke_response(output: CommandOutput) -> anyhow::Result<Response> {
    Ok(Response::Command {
        stdout: output.stdout,
        stderr: output.stderr,
    })
}
