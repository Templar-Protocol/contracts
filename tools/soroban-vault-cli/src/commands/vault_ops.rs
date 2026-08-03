//! Shared vault execution and amount conversion helpers.

use anyhow::Context;
use templar_curator_proxy_soroban::AllocationDelta;
use templar_soroban_shared_types::VaultCommand as WireVaultCommand;

use crate::{
    manifest::Manifest,
    stellar::{CommandExecutor, Stellar},
    types::{AddressStr, DecimalAmount, ShareDecimalsArg},
};

use super::{
    inventory::{args, required_contract},
    invoke::invoke_response,
    output::Response,
};

pub(super) fn required_amount(
    name: &str,
    decimal: Option<&DecimalAmount>,
    raw: Option<i128>,
    decimals: u32,
) -> anyhow::Result<i128> {
    if let Some(decimal) = decimal {
        return decimal
            .to_raw(decimals)
            .map_err(|error| anyhow::anyhow!("{name}: {error}"));
    }
    raw.with_context(|| format!("missing amount; pass --{name} or --{name}-raw"))
}

pub(super) fn optional_amount(
    name: &str,
    decimal: Option<&DecimalAmount>,
    raw: Option<i128>,
    decimals: u32,
) -> anyhow::Result<i128> {
    if let Some(decimal) = decimal {
        return decimal
            .to_raw(decimals)
            .map_err(|error| anyhow::anyhow!("{name}: {error}"));
    }
    Ok(raw.unwrap_or(0))
}

pub(super) fn required_share_amount(
    manifest: &Manifest,
    name: &str,
    decimal: Option<&DecimalAmount>,
    raw: Option<i128>,
    decimals: ShareDecimalsArg,
) -> anyhow::Result<i128> {
    if decimal.is_some() {
        let decimals = resolve_share_decimals(manifest, decimals)?;
        return required_amount(name, decimal, raw, decimals);
    }
    raw.with_context(|| format!("missing amount; pass --{name} or --{name}-raw"))
}

pub(super) fn optional_share_amount(
    manifest: &Manifest,
    name: &str,
    decimal: Option<&DecimalAmount>,
    raw: Option<i128>,
    decimals: ShareDecimalsArg,
) -> anyhow::Result<Option<i128>> {
    if let Some(decimal) = decimal {
        let decimals = resolve_share_decimals(manifest, decimals)?;
        return decimal
            .to_raw(decimals)
            .map(Some)
            .map_err(|error| anyhow::anyhow!("{name}: {error}"));
    }
    Ok(raw)
}

pub(super) fn resolve_share_decimals(
    manifest: &Manifest,
    decimals: ShareDecimalsArg,
) -> anyhow::Result<u32> {
    match decimals {
        ShareDecimalsArg::Explicit(decimals) => Ok(decimals),
        ShareDecimalsArg::Manifest => manifest
            .contracts
            .get("share_token")
            .and_then(|record| record.constructor_args.get("decimals"))
            .and_then(|value| value.parse().ok())
            .context(
                "share decimals are not recorded in the manifest; pass --share-decimals <n> or use --shares-raw",
            ),
    }
}

pub(super) fn execute_allocation<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    caller: &AddressStr,
    delta: &AllocationDelta,
) -> anyhow::Result<Response> {
    let (market, amount, supply) = match delta {
        AllocationDelta::Supply(market, amount) => (*market, *amount, true),
        AllocationDelta::Withdraw(market, amount) => (*market, *amount, false),
    };
    execute_vault(
        stellar,
        manifest,
        WireVaultCommand::Allocate {
            caller: caller.to_string(),
            market,
            amount,
            supply,
        },
    )
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "callers hand off a fully built command"
)]
pub(super) fn execute_vault<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    command: WireVaultCommand,
) -> anyhow::Result<Response> {
    let vault = required_contract(manifest, "vault")?;
    let payload = hex::encode(command.encode());
    invoke_response(stellar.invoke(vault, "execute", args([("--payload", &payload)]))?)
}
