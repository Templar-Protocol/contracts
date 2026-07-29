//! Checkpointed deployment-manifest state transitions.

use std::collections::BTreeMap;

use anyhow::Context;
use tracing::info;

use crate::{
    manifest::{ContractRecord, Manifest},
    stellar::CommandExecutor,
    types::AddressStr,
};

use super::super::{context::CommandContext, inventory::map_args};

pub(in crate::commands) struct DeploymentContext<'ctx, 'deps, 'manifest, E: CommandExecutor> {
    command: &'ctx CommandContext<'deps, E>,
    manifest: &'manifest mut Manifest,
}

impl<'ctx, 'deps, 'manifest, E: CommandExecutor> DeploymentContext<'ctx, 'deps, 'manifest, E> {
    pub(in crate::commands) fn new(
        command: &'ctx CommandContext<'deps, E>,
        manifest: &'manifest mut Manifest,
    ) -> Self {
        Self { command, manifest }
    }

    pub(in crate::commands) fn parts(&mut self) -> (&CommandContext<'deps, E>, &mut Manifest) {
        (self.command, self.manifest)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::commands) enum InitializationState {
    Pending,
    Complete,
}

impl InitializationState {
    const fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }
}

pub(in crate::commands) struct ContractDeployment<'a> {
    pub(in crate::commands) key: &'a str,
    pub(in crate::commands) wasm_hash: &'a str,
    pub(in crate::commands) constructor_args: Vec<String>,
    pub(in crate::commands) constructor_summary: BTreeMap<String, String>,
    pub(in crate::commands) force_new: bool,
    pub(in crate::commands) initialization: InitializationState,
}

pub(in crate::commands) fn deploy_contract_if_needed<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &mut Manifest,
    deployment: ContractDeployment<'_>,
) -> anyhow::Result<String> {
    let ContractDeployment {
        key,
        wasm_hash,
        constructor_args,
        constructor_summary,
        force_new,
        initialization,
    } = deployment;
    let stellar = context.stellar();
    if !force_new {
        if let Some(record) = manifest.contracts.get(key) {
            info!(
                contract_key = key,
                contract_id = %record.contract_id,
                "reusing contract recorded in manifest"
            );
            return Ok(record.contract_id.clone());
        }
    }
    info!(
        contract_key = key,
        wasm_hash, force_new, "deploying contract"
    );
    let contract_id = stellar.deploy(wasm_hash, constructor_args)?;
    manifest.contracts.insert(
        key.to_string(),
        ContractRecord {
            contract_id: contract_id.clone(),
            wasm_hash: wasm_hash.to_string(),
            salt: None,
            constructor_args: constructor_summary,
            deploy_tx: None,
            initialized: initialization.is_complete(),
        },
    );
    context.checkpoint(manifest)?;
    info!(
        contract_key = key,
        contract_id = %contract_id,
        "recorded deployed contract"
    );
    Ok(contract_id)
}

pub(in crate::commands) fn record_imported_contract_if_provided<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &mut Manifest,
    key: &str,
    contract_id: Option<&AddressStr>,
) -> anyhow::Result<()> {
    let Some(contract_id) = contract_id else {
        return Ok(());
    };
    if let Some(record) = manifest.contracts.get(key) {
        anyhow::ensure!(
            record.contract_id == contract_id.as_str(),
            "{key} already recorded as {}; refusing to overwrite with {}",
            record.contract_id,
            contract_id
        );
        context.checkpoint(manifest)?;
        info!(
            contract_key = key,
            contract_id = %record.contract_id,
            "confirmed imported contract already recorded"
        );
        return Ok(());
    }
    manifest.contracts.insert(
        key.to_string(),
        ContractRecord {
            contract_id: contract_id.to_string(),
            wasm_hash: "predeployed".to_string(),
            salt: None,
            constructor_args: BTreeMap::new(),
            deploy_tx: None,
            initialized: true,
        },
    );
    context.checkpoint(manifest)?;
    info!(
        contract_key = key,
        contract_id = %contract_id,
        "recorded imported contract"
    );
    Ok(())
}

pub(in crate::commands) fn record_asset_token<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &mut Manifest,
    asset_token: &str,
    predeployed: bool,
) -> anyhow::Result<()> {
    if let Some(record) = manifest.contracts.get("asset_token") {
        anyhow::ensure!(
            record.contract_id == asset_token,
            "asset_token already recorded as {}; refusing to overwrite with {}",
            record.contract_id,
            asset_token
        );
        context.checkpoint(manifest)?;
        info!(
            contract_key = "asset_token",
            contract_id = %record.contract_id,
            "confirmed asset token already recorded"
        );
        return Ok(());
    }
    let asset_source = if predeployed { "predeployed" } else { "native" };
    manifest.contracts.insert(
        "asset_token".to_string(),
        ContractRecord {
            contract_id: asset_token.to_string(),
            wasm_hash: "stellar-asset-contract".to_string(),
            salt: None,
            constructor_args: map_args([("asset", asset_source)]),
            deploy_tx: None,
            initialized: true,
        },
    );
    context.checkpoint(manifest)?;
    info!(
        contract_key = "asset_token",
        contract_id = %asset_token,
        predeployed,
        "recorded asset token"
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(in crate::commands) fn initialize_vault_if_needed<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &mut Manifest,
    vault: &str,
    admin: &str,
    governance: &str,
    asset_token: &str,
    share_token: &str,
    virtual_shares: i128,
    virtual_assets: i128,
) -> anyhow::Result<()> {
    let stellar = context.stellar();
    if manifest
        .contracts
        .get("vault")
        .context("vault deployment was not recorded in manifest")?
        .initialized
    {
        return Ok(());
    }
    stellar.invoke(
        vault,
        "initialize",
        vec![
            "--curator".to_string(),
            admin.to_string(),
            "--governance".to_string(),
            governance.to_string(),
            "--asset_token".to_string(),
            asset_token.to_string(),
            "--share_token".to_string(),
            share_token.to_string(),
            "--virtual_shares".to_string(),
            virtual_shares.to_string(),
            "--virtual_assets".to_string(),
            virtual_assets.to_string(),
        ],
    )?;
    let record = manifest
        .contracts
        .get_mut("vault")
        .context("vault deployment was not recorded in manifest")?;
    record.initialized = true;
    record.constructor_args.extend(map_args([
        ("curator", admin),
        ("governance", governance),
        ("asset_token", asset_token),
        ("share_token", share_token),
    ]));
    record
        .constructor_args
        .insert("virtual_shares".to_string(), virtual_shares.to_string());
    record
        .constructor_args
        .insert("virtual_assets".to_string(), virtual_assets.to_string());
    context.checkpoint(manifest)?;
    Ok(())
}

pub(in crate::commands) fn initialize_proxy_if_needed<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &mut Manifest,
    key: &str,
    contract_id: &str,
    args: Vec<String>,
) -> anyhow::Result<()> {
    let stellar = context.stellar();
    if manifest
        .contracts
        .get(key)
        .with_context(|| format!("{key} deployment was not recorded in manifest"))?
        .initialized
    {
        return Ok(());
    }
    stellar.invoke(contract_id, "initialize", args)?;
    manifest
        .contracts
        .get_mut(key)
        .with_context(|| format!("{key} deployment was not recorded in manifest"))?
        .initialized = true;
    context.checkpoint(manifest)?;
    Ok(())
}
