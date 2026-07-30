//! Curator-proxy deployment, initialization provenance, and version verification.

use std::collections::BTreeMap;

use anyhow::Context;

use crate::{
    artifacts::{ensure_uploaded, ArtifactSpec},
    cli::{Cli, DeployCuratorProxyArgs},
    manifest::Manifest,
    stellar::{CommandExecutor, Stellar},
};

use super::{
    super::{
        inventory::{map_args, required_contract},
        output::Response,
        CURATOR_PROXY_GOVERNANCE_ARG, CURATOR_PROXY_INITIALIZATION_AUTHORITY_ARG,
        CURATOR_PROXY_INITIALIZER_ARG, CURATOR_PROXY_LEGACY_V1_HASH_ARG, CURATOR_PROXY_VAULT_ARG,
        CURATOR_PROXY_VERSION_DISCOVERY_ARG,
    },
    session::{
        deploy_contract_if_needed, record_imported_contract_if_provided, ContractDeployment,
        DeploymentContext, InitializationState,
    },
};

pub(in crate::commands) fn deploy_curator_proxy<E: CommandExecutor>(
    deployment: &mut DeploymentContext<'_, '_, '_, E>,
    args: &DeployCuratorProxyArgs,
) -> anyhow::Result<Response> {
    let (context, manifest) = deployment.parts();
    let cli = context.cli();
    let stellar = context.stellar();
    record_imported_contract_if_provided(context, manifest, "vault", args.vault.as_ref())?;
    context.checkpoint(manifest)?;
    record_imported_contract_if_provided(
        context,
        manifest,
        "governance",
        args.governance.as_ref(),
    )?;
    context.checkpoint(manifest)?;

    let vault = required_contract(manifest, "vault")?.to_string();
    let governance = required_contract(manifest, "governance")?.to_string();

    let wasm_hash = ensure_uploaded(
        stellar,
        manifest,
        &cli.workspace_path,
        ArtifactSpec::from_name(crate::cli::ArtifactName::CuratorProxy),
        args.build,
    )?;
    context.checkpoint(manifest)?;

    let (initializer, initializer_args, legacy_hash) =
        curator_proxy_initializer(args, &vault, &governance);
    let (reuse_checkpoint, constructor_args, constructor_summary) =
        curator_proxy_deployment_args(args, stellar, manifest, &wasm_hash)?;
    let already_initialized = reuse_checkpoint
        && manifest
            .contracts
            .get("curator_proxy")
            .is_some_and(|record| record.initialized);
    if !already_initialized {
        if let Some(expected_hash) = args.legacy_v1_wasm_hash.as_ref() {
            verify_current_contract_wasm_hash(cli, stellar, &vault, expected_hash.as_str())?;
        }
    }
    let curator_proxy = deploy_contract_if_needed(
        context,
        manifest,
        ContractDeployment {
            key: "curator_proxy",
            wasm_hash: &wasm_hash,
            constructor_args,
            constructor_summary,
            force_new: !reuse_checkpoint,
            initialization: InitializationState::Pending,
        },
    )?;

    if already_initialized {
        let has_provenance = manifest
            .contracts
            .get("curator_proxy")
            .is_some_and(|record| {
                record
                    .constructor_args
                    .contains_key(CURATOR_PROXY_INITIALIZER_ARG)
            });
        if has_provenance {
            ensure_curator_proxy_initialization_matches(
                manifest,
                &vault,
                &governance,
                initializer,
                legacy_hash,
            )?;
        } else {
            if !cli.dry_run {
                verify_curator_proxy_targets(stellar, &curator_proxy, &vault, &governance)?;
            }
            record_verified_curator_proxy_targets(manifest, &vault, &governance)?;
            context.checkpoint(manifest)?;
        }
    } else {
        stellar.invoke(&curator_proxy, initializer, initializer_args)?;
        if let Some(record) = manifest.contracts.get_mut("curator_proxy") {
            record.initialized = true;
        }
        record_curator_proxy_initialization(
            manifest,
            &vault,
            &governance,
            initializer,
            legacy_hash,
        )?;
        context.checkpoint(manifest)?;
    }
    let version = verify_curator_proxy_version(cli, stellar, &curator_proxy)?;
    mark_curator_proxy_version_discovery(manifest)?;
    context.checkpoint(manifest)?;
    Ok(Response::message(format!(
        "curator proxy {curator_proxy} vault version: {version}"
    )))
}

pub(in crate::commands) fn curator_proxy_initializer<'a>(
    args: &'a DeployCuratorProxyArgs,
    vault: &str,
    governance: &str,
) -> (&'static str, Vec<String>, Option<&'a str>) {
    if let Some(legacy_hash) = args.legacy_v1_wasm_hash.as_ref() {
        (
            "initialize_legacy_v1",
            vec![
                "--vault_address".to_string(),
                vault.to_string(),
                "--governance_address".to_string(),
                governance.to_string(),
                "--legacy_v1_wasm_hash".to_string(),
                legacy_hash.to_string(),
            ],
            Some(legacy_hash.as_str()),
        )
    } else {
        (
            "initialize",
            vec![
                "--vault_address".to_string(),
                vault.to_string(),
                "--governance_address".to_string(),
                governance.to_string(),
            ],
            None,
        )
    }
}

pub(in crate::commands) fn curator_proxy_deployment_args<E: CommandExecutor>(
    args: &DeployCuratorProxyArgs,
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    wasm_hash: &str,
) -> anyhow::Result<(bool, Vec<String>, BTreeMap<String, String>)> {
    let reuse_checkpoint = !args.force_new
        && manifest
            .contracts
            .get("curator_proxy")
            .is_some_and(|record| record.wasm_hash == wasm_hash);
    if reuse_checkpoint {
        return Ok((true, Vec::new(), BTreeMap::new()));
    }

    let initialization_authority = stellar.source_public_address()?;
    Ok((
        false,
        vec![
            "--initialization_authority".to_string(),
            initialization_authority.clone(),
        ],
        map_args([(
            CURATOR_PROXY_INITIALIZATION_AUTHORITY_ARG,
            initialization_authority.as_str(),
        )]),
    ))
}

pub(in crate::commands) fn verify_current_contract_wasm_hash<E: CommandExecutor>(
    cli: &Cli,
    stellar: &Stellar<'_, E>,
    contract_id: &str,
    expected_hash: &str,
) -> anyhow::Result<()> {
    if cli.dry_run {
        return Ok(());
    }
    let actual_hash = stellar.fetch_contract_wasm_hash(contract_id)?;
    anyhow::ensure!(
        actual_hash == expected_hash,
        "legacy v1 WASM hash mismatch for vault {contract_id}: expected {expected_hash}, found {actual_hash}"
    );
    Ok(())
}

pub(in crate::commands) fn record_curator_proxy_initialization(
    manifest: &mut Manifest,
    vault: &str,
    governance: &str,
    initializer: &str,
    legacy_v1_wasm_hash: Option<&str>,
) -> anyhow::Result<()> {
    let record = manifest
        .contracts
        .get_mut("curator_proxy")
        .context("curator proxy deployment was not recorded in manifest")?;
    record.constructor_args.insert(
        CURATOR_PROXY_INITIALIZER_ARG.to_string(),
        initializer.to_string(),
    );
    record
        .constructor_args
        .remove(CURATOR_PROXY_VERSION_DISCOVERY_ARG);
    record
        .constructor_args
        .insert(CURATOR_PROXY_VAULT_ARG.to_string(), vault.to_string());
    record.constructor_args.insert(
        CURATOR_PROXY_GOVERNANCE_ARG.to_string(),
        governance.to_string(),
    );
    match legacy_v1_wasm_hash {
        Some(hash) => {
            record.constructor_args.insert(
                CURATOR_PROXY_LEGACY_V1_HASH_ARG.to_string(),
                hash.to_string(),
            );
        }
        None => {
            record
                .constructor_args
                .remove(CURATOR_PROXY_LEGACY_V1_HASH_ARG);
        }
    }
    Ok(())
}

pub(in crate::commands) fn record_verified_curator_proxy_targets(
    manifest: &mut Manifest,
    vault: &str,
    governance: &str,
) -> anyhow::Result<()> {
    let record = manifest
        .contracts
        .get_mut("curator_proxy")
        .context("curator proxy deployment was not recorded in manifest")?;
    record
        .constructor_args
        .remove(CURATOR_PROXY_INITIALIZER_ARG);
    record
        .constructor_args
        .remove(CURATOR_PROXY_LEGACY_V1_HASH_ARG);
    record
        .constructor_args
        .insert(CURATOR_PROXY_VAULT_ARG.to_string(), vault.to_string());
    record.constructor_args.insert(
        CURATOR_PROXY_GOVERNANCE_ARG.to_string(),
        governance.to_string(),
    );
    Ok(())
}

pub(in crate::commands) fn ensure_curator_proxy_initialization_matches(
    manifest: &Manifest,
    vault: &str,
    governance: &str,
    initializer: &str,
    legacy_v1_wasm_hash: Option<&str>,
) -> anyhow::Result<()> {
    let record = manifest
        .contracts
        .get("curator_proxy")
        .context("curator proxy deployment was not recorded in manifest")?;
    anyhow::ensure!(
        record
            .constructor_args
            .get(CURATOR_PROXY_INITIALIZER_ARG)
            .map(String::as_str)
            == Some(initializer),
        "checkpointed curator proxy used a different initializer; pass --force-new to replace it"
    );
    anyhow::ensure!(
        record
            .constructor_args
            .get(CURATOR_PROXY_VAULT_ARG)
            .map(String::as_str)
            == Some(vault),
        "checkpointed curator proxy targets a different vault; pass --force-new to replace it"
    );
    anyhow::ensure!(
        record
            .constructor_args
            .get(CURATOR_PROXY_GOVERNANCE_ARG)
            .map(String::as_str)
            == Some(governance),
        "checkpointed curator proxy targets different governance; pass --force-new to replace it"
    );
    anyhow::ensure!(
        record
            .constructor_args
            .get(CURATOR_PROXY_LEGACY_V1_HASH_ARG)
            .map(String::as_str)
            == legacy_v1_wasm_hash,
        "checkpointed curator proxy used a different legacy-v1 hash; pass --force-new to replace it"
    );
    Ok(())
}

pub(in crate::commands) fn verify_curator_proxy_targets<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    curator_proxy: &str,
    vault: &str,
    governance: &str,
) -> anyhow::Result<()> {
    for (function, expected) in [("vault", vault), ("governance", governance)] {
        let output = stellar.invoke_view(curator_proxy, function, Vec::new())?;
        anyhow::ensure!(
            output.stdout.contains(expected),
            "checkpointed curator proxy {function} mismatch: expected {expected}, observed {}",
            output.stdout
        );
    }
    Ok(())
}

pub(in crate::commands) fn record_standard_curator_proxy_initialization_if_missing(
    manifest: &mut Manifest,
    vault: &str,
    governance: &str,
) -> anyhow::Result<()> {
    let has_initializer = manifest
        .contracts
        .get("curator_proxy")
        .is_some_and(|record| {
            record
                .constructor_args
                .contains_key(CURATOR_PROXY_INITIALIZER_ARG)
        });
    if has_initializer {
        return Ok(());
    }
    record_curator_proxy_initialization(manifest, vault, governance, "initialize", None)
}

pub(in crate::commands) fn mark_curator_proxy_version_discovery(
    manifest: &mut Manifest,
) -> anyhow::Result<()> {
    let record = manifest
        .contracts
        .get_mut("curator_proxy")
        .context("curator proxy deployment was not recorded in manifest")?;
    record.constructor_args.insert(
        CURATOR_PROXY_VERSION_DISCOVERY_ARG.to_string(),
        "true".to_string(),
    );
    Ok(())
}

pub(in crate::commands) fn verify_curator_proxy_version<E: CommandExecutor>(
    cli: &Cli,
    stellar: &Stellar<'_, E>,
    curator_proxy: &str,
) -> anyhow::Result<String> {
    let output = stellar.invoke_view(curator_proxy, "vault_version", Vec::new())?;
    if cli.dry_run {
        return Ok("<dry-run>".to_string());
    }
    anyhow::ensure!(
        !output.stdout.trim().is_empty(),
        "curator proxy vault_version returned an empty response"
    );
    Ok(output.stdout)
}
