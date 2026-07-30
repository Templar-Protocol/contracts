//! Adapter authority validation, capability checks, and additive deployment.

use std::collections::BTreeSet;

use anyhow::Context;
use templar_soroban_shared_types::RUNTIME_FEATURE_COMPANION_UPGRADE;

use crate::{
    artifacts::{ensure_uploaded, ArtifactSpec},
    cli::Cli,
    manifest::Manifest,
    stellar::{CommandExecutor, Stellar},
    types::{AdapterAdminArg, AddressStr},
};

use super::{
    super::{
        context::CommandContext,
        inventory::{
            blend_adapter_by_pool, blend_adapter_statuses, contract_id,
            custodial_adapter_by_custodian, custodial_adapter_statuses, map_args,
            next_blend_adapter_key, next_custodial_adapter_key, required_contract,
        },
        output::{BlendAdapterStatus, CustodialAdapterStatus, Response, StatusResponse},
    },
    session::{
        deploy_contract_if_needed, record_asset_token, record_imported_contract_if_provided,
        ContractDeployment, DeploymentContext, InitializationState,
    },
};

pub(in crate::commands) fn require_adapter_admin(
    admin: Option<&AdapterAdminArg>,
) -> anyhow::Result<&AdapterAdminArg> {
    admin.context(
        "adapter deployment requires an explicit --adapter-admin <address|vault>; governance is not an implicit adapter admin",
    )
}

pub(in crate::commands) fn validated_stack_adapter_admin<'a>(
    manifest: &Manifest,
    args: &'a crate::cli::DeployStackArgs,
) -> anyhow::Result<Option<&'a AdapterAdminArg>> {
    let include_blend = !args.blend_pools.is_empty();
    let include_custodial = !args.custodians.is_empty();
    if !include_blend && !include_custodial {
        return Ok(None);
    }
    let admin = require_adapter_admin(args.adapter_admin.as_ref())?;
    let vault = (!args.force_new)
        .then(|| contract_id(manifest, "vault"))
        .flatten();
    let governance = contract_id(manifest, "governance");
    let custodial_asset = include_custodial.then(|| {
        args.asset_token
            .as_ref()
            .map(AddressStr::as_str)
            .or_else(|| contract_id(manifest, "asset_token"))
    });
    validate_adapter_admin(admin, vault, governance, custodial_asset.flatten())?;
    Ok(Some(admin))
}

pub(in crate::commands) fn validate_adapter_deployment_request(
    manifest: &Manifest,
    args: &crate::cli::DeployAdaptersArgs,
) -> anyhow::Result<()> {
    let vault =
        contract_id(manifest, "vault").or_else(|| args.vault.as_ref().map(AddressStr::as_str));
    let governance = contract_id(manifest, "governance")
        .or_else(|| args.governance.as_ref().map(AddressStr::as_str));
    let custodial_asset = (!args.custodians.is_empty()).then(|| {
        contract_id(manifest, "asset_token")
            .or_else(|| args.asset_token.as_ref().map(AddressStr::as_str))
    });
    validate_adapter_admin(
        &args.adapter_admin,
        vault,
        governance,
        custodial_asset.flatten(),
    )
}

pub(in crate::commands) fn validate_adapter_admin(
    admin: &AdapterAdminArg,
    vault: Option<&str>,
    governance: Option<&str>,
    custodial_asset: Option<&str>,
) -> anyhow::Result<()> {
    let resolved = match admin {
        AdapterAdminArg::Vault => vault,
        AdapterAdminArg::Address(address) => Some(address.as_str()),
    };
    if let Some(governance) = governance {
        anyhow::ensure!(
            resolved != Some(governance),
            "adapter admin must differ from the governance contract"
        );
    }
    if let Some(asset) = custodial_asset {
        anyhow::ensure!(
            resolved != Some(asset),
            "custodial adapter admin must differ from the asset token"
        );
    }
    Ok(())
}

pub(in crate::commands) fn verify_vault_companion_upgrade<E: CommandExecutor>(
    cli: &Cli,
    stellar: &Stellar<'_, E>,
    vault: &str,
) -> anyhow::Result<()> {
    let output = stellar
        .invoke_view(vault, "version", Vec::new())
        .with_context(|| {
            format!(
                "cannot use vault {vault} as adapter admin: runtime capability detection via version() failed"
            )
        })?;
    if cli.dry_run {
        eprintln!(
            "Warning: dry-run cannot verify that vault {vault} advertises companion-upgrade capability {RUNTIME_FEATURE_COMPANION_UPGRADE:#x}"
        );
        return Ok(());
    }
    let (version, feature_flags) = parse_runtime_version(&output.stdout).with_context(|| {
        format!(
            "cannot use vault {vault} as adapter admin: decode runtime version() capability response"
        )
    })?;
    anyhow::ensure!(
        feature_flags & RUNTIME_FEATURE_COMPANION_UPGRADE != 0,
        "cannot use vault {vault} version {version} as adapter admin: companion-upgrade capability {RUNTIME_FEATURE_COMPANION_UPGRADE:#x} is not advertised (feature mask {feature_flags:#x})"
    );
    Ok(())
}

pub(in crate::commands) fn parse_runtime_version(raw: &str) -> anyhow::Result<(String, u64)> {
    serde_json::from_str(raw.trim())
        .context("expected runtime version response as [version, feature_flags]")
}

pub(in crate::commands) fn deploy_adapters<E: CommandExecutor>(
    deployment: &mut DeploymentContext<'_, '_, '_, E>,
    args: &crate::cli::DeployAdaptersArgs,
) -> anyhow::Result<Response> {
    let (context, manifest) = deployment.parts();
    let cli = context.cli();
    let stellar = context.stellar();
    anyhow::ensure!(
        !args.blend_pools.is_empty() || !args.custodians.is_empty(),
        "deploy adapters requires at least one --blend-pool or --custodian"
    );
    let requested_adapter_admin = &args.adapter_admin;
    validate_adapter_deployment_request(manifest, args)?;

    record_imported_contract_if_provided(context, manifest, "vault", args.vault.as_ref())?;
    context.checkpoint(manifest)?;
    record_imported_contract_if_provided(
        context,
        manifest,
        "governance",
        args.governance.as_ref(),
    )?;
    context.checkpoint(manifest)?;
    if let Some(asset_token) = &args.asset_token {
        record_asset_token(context, manifest, asset_token.as_str(), true)?;
        context.checkpoint(manifest)?;
    }

    let vault = required_contract(manifest, "vault")?.to_string();
    let governance = required_contract(manifest, "governance")?.to_string();
    let asset_token = if args.custodians.is_empty() {
        contract_id(manifest, "asset_token").map(ToString::to_string)
    } else {
        Some(required_contract(manifest, "asset_token")?.to_string())
    };
    let custodial_asset = if args.custodians.is_empty() {
        None
    } else {
        asset_token.as_deref()
    };
    validate_adapter_admin(
        requested_adapter_admin,
        Some(&vault),
        Some(&governance),
        custodial_asset,
    )?;
    if requested_adapter_admin.targets_vault(&vault) {
        verify_vault_companion_upgrade(cli, stellar, &vault)?;
    }
    let blend_adapters = if args.blend_pools.is_empty() {
        blend_adapter_statuses(manifest)
    } else {
        let wasm_hash = ensure_uploaded(
            stellar,
            manifest,
            &cli.workspace_path,
            ArtifactSpec::from_name(crate::cli::ArtifactName::BlendAdapter),
            args.build,
        )?;
        context.checkpoint(manifest)?;
        append_blend_adapters(
            context,
            manifest,
            &wasm_hash,
            requested_adapter_admin,
            &vault,
            &args.blend_pools,
            args.force_new,
        )?
    };
    let custodial_adapters = if args.custodians.is_empty() {
        custodial_adapter_statuses(manifest)
    } else {
        let wasm_hash = ensure_uploaded(
            stellar,
            manifest,
            &cli.workspace_path,
            ArtifactSpec::from_name(crate::cli::ArtifactName::CustodialAdapter),
            args.build,
        )?;
        context.checkpoint(manifest)?;
        append_custodial_adapters(
            context,
            manifest,
            &wasm_hash,
            requested_adapter_admin,
            &vault,
            asset_token
                .as_deref()
                .context("custodial adapters require asset_token in manifest or --asset-token")?,
            &args.custodians,
            args.force_new,
        )?
    };

    Ok(Response::Status(StatusResponse {
        network: manifest.network.clone(),
        vault: Some(vault),
        share_token: contract_id(manifest, "share_token").map(ToString::to_string),
        governance: Some(governance),
        asset_token,
        proxy_4626: contract_id(manifest, "proxy_4626").map(ToString::to_string),
        curator_proxy: contract_id(manifest, "curator_proxy").map(ToString::to_string),
        blend_adapters,
        custodial_adapters,
    }))
}

#[allow(
    clippy::too_many_arguments,
    reason = "adapter deployment needs both manifest checkpoint context and constructor inputs"
)]
pub(in crate::commands) fn append_blend_adapters<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &mut Manifest,
    wasm_hash: &str,
    adapter_admin: &AdapterAdminArg,
    vault: &str,
    pools: &[AddressStr],
    force_new: bool,
) -> anyhow::Result<Vec<BlendAdapterStatus>> {
    for pool in pools {
        if !force_new && blend_adapter_by_pool(manifest, pool).is_some() {
            continue;
        }
        let adapter_admin = adapter_admin.resolve(vault);
        let key = next_blend_adapter_key(manifest);
        let adapter = deploy_contract_if_needed(
            context,
            manifest,
            ContractDeployment {
                key: &key,
                wasm_hash,
                constructor_args: vec![
                    "--admin".to_string(),
                    adapter_admin.to_string(),
                    "--vault".to_string(),
                    vault.to_string(),
                    "--pool".to_string(),
                    pool.to_string(),
                ],
                constructor_summary: map_args([
                    ("admin", adapter_admin),
                    ("vault", vault),
                    ("pool", pool.as_str()),
                ]),
                force_new,
                initialization: InitializationState::Complete,
            },
        )?;
        if let Some(record) = manifest.contracts.get_mut(&key) {
            record.contract_id = adapter;
        }
        context.checkpoint(manifest)?;
    }
    Ok(blend_adapter_statuses(manifest))
}

#[allow(
    clippy::too_many_arguments,
    reason = "adapter deployment needs both manifest checkpoint context and constructor inputs"
)]
pub(in crate::commands) fn append_custodial_adapters<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &mut Manifest,
    wasm_hash: &str,
    adapter_admin: &AdapterAdminArg,
    vault: &str,
    asset_token: &str,
    custodians: &[AddressStr],
    force_new: bool,
) -> anyhow::Result<Vec<CustodialAdapterStatus>> {
    let mut planned_custodians = BTreeSet::new();
    for custodian in custodians {
        if !planned_custodians.insert(custodian.as_str().to_string()) {
            continue;
        }
        if !force_new && custodial_adapter_by_custodian(manifest, custodian).is_some() {
            continue;
        }
        let adapter_admin = adapter_admin.resolve(vault);
        let key = next_custodial_adapter_key(manifest);
        let adapter = deploy_contract_if_needed(
            context,
            manifest,
            ContractDeployment {
                key: &key,
                wasm_hash,
                constructor_args: vec![
                    "--admin".to_string(),
                    adapter_admin.to_string(),
                    "--vault".to_string(),
                    vault.to_string(),
                    "--custodian".to_string(),
                    custodian.to_string(),
                    "--asset".to_string(),
                    asset_token.to_string(),
                ],
                constructor_summary: map_args([
                    ("admin", adapter_admin),
                    ("vault", vault),
                    ("custodian", custodian.as_str()),
                    ("asset", asset_token),
                ]),
                force_new,
                initialization: InitializationState::Complete,
            },
        )?;
        if let Some(record) = manifest.contracts.get_mut(&key) {
            record.contract_id = adapter;
        }
        context.checkpoint(manifest)?;
    }
    Ok(custodial_adapter_statuses(manifest))
}
