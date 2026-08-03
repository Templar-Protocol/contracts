//! Aggregate contract instance and code TTL maintenance.

use std::collections::BTreeSet;

use anyhow::Context;
use templar_soroban_shared_types::VaultCommand as WireVaultCommand;

use crate::{
    cli::ExtendTtlArgs,
    manifest::{ContractRecord, Manifest},
    stellar::{CommandExecutor, Stellar},
};

use super::{
    context::CommandContext,
    inventory::{args, blend_adapter_statuses, contract_id, custodial_adapter_statuses},
    output::{ExtendTtlResponse, Response},
    CONTRACT_TTL_EXTEND_LEDGERS,
};

pub(super) fn run_extend_ttl<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &Manifest,
    _ttl_args: &ExtendTtlArgs,
) -> anyhow::Result<Response> {
    let stellar = context.stellar();
    let mut extended = Vec::new();
    let mut skipped = Vec::new();
    let mut protocol_wasm_hashes = BTreeSet::new();

    if let Some(vault) = manifest.contracts.get("vault") {
        let payload = hex::encode(WireVaultCommand::ExtendTtl.encode());
        stellar.invoke(
            &vault.contract_id,
            "execute",
            args([("--payload", &payload)]),
        )?;
        protocol_wasm_hashes.insert(wasm_hash_for_ttl(stellar, vault)?);
        extended.push("vault".to_string());
    } else {
        skipped.push("vault".to_string());
    }

    if let Some(governance) = manifest.contracts.get("governance") {
        stellar.invoke(&governance.contract_id, "extend_ttl", Vec::new())?;
        protocol_wasm_hashes.insert(wasm_hash_for_ttl(stellar, governance)?);
        extended.push("governance".to_string());
    } else {
        skipped.push("governance".to_string());
    }

    if let Some(proxy) = manifest.contracts.get("proxy_4626") {
        stellar.invoke(&proxy.contract_id, "extend_ttl", Vec::new())?;
        protocol_wasm_hashes.insert(wasm_hash_for_ttl(stellar, proxy)?);
        extended.push("proxy_4626".to_string());
    } else {
        skipped.push("proxy_4626".to_string());
    }

    if let Some(proxy) = manifest.contracts.get("curator_proxy") {
        stellar.invoke(&proxy.contract_id, "extend_ttl", Vec::new())?;
        protocol_wasm_hashes.insert(wasm_hash_for_ttl(stellar, proxy)?);
        extended.push("curator_proxy".to_string());
    } else {
        skipped.push("curator_proxy".to_string());
    }

    if let Some(share) = manifest.contracts.get("share_token") {
        stellar.extend_contract_instance_ttl(&share.contract_id, CONTRACT_TTL_EXTEND_LEDGERS)?;
        protocol_wasm_hashes.insert(wasm_hash_for_ttl(stellar, share)?);
        extended.push("share_token".to_string());
    } else {
        skipped.push("share_token".to_string());
    }

    let adapters = blend_adapter_statuses(manifest);
    if adapters.is_empty() {
        skipped.push("blend_adapters".to_string());
    } else {
        for adapter in adapters {
            stellar
                .extend_contract_instance_ttl(&adapter.contract_id, CONTRACT_TTL_EXTEND_LEDGERS)?;
            let record = manifest
                .contracts
                .get(&adapter.key)
                .with_context(|| format!("missing {} contract record", adapter.key))?;
            protocol_wasm_hashes.insert(wasm_hash_for_ttl(stellar, record)?);
            extended.push(adapter.key);
        }
    }

    for wasm_hash in &protocol_wasm_hashes {
        stellar.extend_contract_code_ttl(wasm_hash, CONTRACT_TTL_EXTEND_LEDGERS)?;
    }

    let adapters = custodial_adapter_statuses(manifest);
    if adapters.is_empty() {
        skipped.push("custodial_adapters".to_string());
    } else {
        for adapter in adapters {
            stellar.invoke(&adapter.contract_id, "extend_ttl", Vec::new())?;
            let record = manifest
                .contracts
                .get(&adapter.key)
                .with_context(|| format!("missing {} contract record", adapter.key))?;
            let wasm_hash = wasm_hash_for_ttl(stellar, record)?;
            if protocol_wasm_hashes.insert(wasm_hash.clone()) {
                stellar.extend_contract_code_ttl(&wasm_hash, CONTRACT_TTL_EXTEND_LEDGERS)?;
            }
            extended.push(adapter.key);
        }
    }

    for key in ["asset_token"] {
        if contract_id(manifest, key).is_some() {
            skipped.push(format!("{key}: no deployment-wide TTL entrypoint"));
        }
    }

    Ok(Response::ExtendTtl(ExtendTtlResponse { extended, skipped }))
}

pub(super) fn wasm_hash_for_ttl<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    record: &ContractRecord,
) -> anyhow::Result<String> {
    stellar
        .fetch_contract_wasm_hash(&record.contract_id)
        .with_context(|| format!("resolve WASM hash for contract {}", record.contract_id))
}
