//! Full-stack deployment orchestration and checkpointed resume.

use std::{
    collections::BTreeMap,
    io::{self, IsTerminal},
};

use anyhow::Context;
use indicatif::{ProgressBar, ProgressStyle};

use crate::{
    artifacts::{ensure_uploaded, ArtifactSpec},
    cli::{Cli, DeployStackArgs},
    manifest::Manifest,
    stellar::CommandExecutor,
};

use super::{
    super::{
        context::CommandContext,
        inventory::{blend_adapter_statuses, contract_id, custodial_adapter_statuses, map_args},
        output::{BlendAdapterStatus, CustodialAdapterStatus, Response, StatusResponse},
        CURATOR_PROXY_INITIALIZATION_AUTHORITY_ARG,
    },
    adapters::{
        append_blend_adapters, append_custodial_adapters, validate_adapter_admin,
        validated_stack_adapter_admin, verify_vault_companion_upgrade,
    },
    curator_proxy::{
        mark_curator_proxy_version_discovery,
        record_standard_curator_proxy_initialization_if_missing, verify_curator_proxy_version,
    },
    reconcile::{
        apply_reconcile_safe_manifest_updates, curator_proxy_needs_version_verification,
        reconcile_manifest,
    },
    session::{
        deploy_contract_if_needed, initialize_proxy_if_needed, initialize_vault_if_needed,
        record_asset_token, ContractDeployment, DeploymentContext, InitializationState,
    },
};

pub(in crate::commands) struct DeploymentProgress {
    bar: Option<ProgressBar>,
}

impl DeploymentProgress {
    fn stack(cli: &Cli, steps: u64) -> Self {
        if cli.json || cli.json_lines || cli.dry_run || !io::stderr().is_terminal() {
            return Self { bar: None };
        }
        let bar = ProgressBar::new(steps);
        let style = ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:30.cyan/blue}] {pos}/{len} {msg}",
        )
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("=>-");
        bar.set_style(style);
        bar.set_message("starting stack deployment");
        Self { bar: Some(bar) }
    }

    fn step<T>(
        &self,
        label: impl Into<String>,
        operation: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let label = label.into();
        if let Some(bar) = &self.bar {
            bar.set_message(label.clone());
        }
        match operation() {
            Ok(value) => {
                if let Some(bar) = &self.bar {
                    bar.inc(1);
                }
                Ok(value)
            }
            Err(error) => {
                if let Some(bar) = &self.bar {
                    bar.abandon_with_message(format!("failed: {label}"));
                }
                Err(error)
            }
        }
    }

    fn finish(&self) {
        if let Some(bar) = &self.bar {
            bar.finish_with_message("stack deployment complete");
        }
    }
}

pub(in crate::commands) fn stack_progress_steps(
    include_blend: bool,
    include_custodial: bool,
    blend_pool_count: usize,
    custodian_count: usize,
) -> u64 {
    let artifact_steps = ArtifactSpec::stack_artifacts(include_blend, include_custodial).len();
    u64::try_from(
        artifact_steps + 9 + usize::from(blend_pool_count > 0) + usize::from(custodian_count > 0),
    )
    .unwrap_or(u64::MAX)
}

struct StackArtifactHashes {
    vault: String,
    governance: String,
    share_token: String,
    proxy_4626: String,
    curator_proxy: String,
    adapters: StackAdapterArtifactHashes,
}

enum StackAdapterArtifactHashes {
    None,
    Blend {
        blend_adapter: String,
    },
    Custodial {
        custodial_adapter: String,
    },
    Both {
        blend_adapter: String,
        custodial_adapter: String,
    },
}

struct StackContracts {
    vault: String,
    governance: String,
    share_token: String,
    asset_token: String,
    proxy_4626: String,
    curator_proxy: String,
}

impl StackContracts {
    fn into_status(
        self,
        network: String,
        blend_adapters: Vec<BlendAdapterStatus>,
        custodial_adapters: Vec<CustodialAdapterStatus>,
    ) -> StatusResponse {
        StatusResponse {
            network,
            vault: Some(self.vault),
            share_token: Some(self.share_token),
            governance: Some(self.governance),
            asset_token: Some(self.asset_token),
            proxy_4626: Some(self.proxy_4626),
            curator_proxy: Some(self.curator_proxy),
            blend_adapters,
            custodial_adapters,
        }
    }
}

impl StackArtifactHashes {
    fn upload<E: CommandExecutor>(
        context: &CommandContext<'_, E>,
        manifest: &mut Manifest,
        progress: &DeploymentProgress,
        include_blend: bool,
        include_custodial: bool,
        build: bool,
    ) -> anyhow::Result<Self> {
        let vault = upload_stack_artifact(
            context,
            manifest,
            progress,
            crate::cli::ArtifactName::Vault,
            build,
        )?;
        let governance = upload_stack_artifact(
            context,
            manifest,
            progress,
            crate::cli::ArtifactName::Governance,
            build,
        )?;
        let share_token = upload_stack_artifact(
            context,
            manifest,
            progress,
            crate::cli::ArtifactName::ShareToken,
            build,
        )?;
        let proxy_4626 = upload_stack_artifact(
            context,
            manifest,
            progress,
            crate::cli::ArtifactName::Proxy4626,
            build,
        )?;
        let curator_proxy = upload_stack_artifact(
            context,
            manifest,
            progress,
            crate::cli::ArtifactName::CuratorProxy,
            build,
        )?;
        let adapters = match (include_blend, include_custodial) {
            (false, false) => StackAdapterArtifactHashes::None,
            (true, false) => StackAdapterArtifactHashes::Blend {
                blend_adapter: upload_stack_artifact(
                    context,
                    manifest,
                    progress,
                    crate::cli::ArtifactName::BlendAdapter,
                    build,
                )?,
            },
            (false, true) => StackAdapterArtifactHashes::Custodial {
                custodial_adapter: upload_stack_artifact(
                    context,
                    manifest,
                    progress,
                    crate::cli::ArtifactName::CustodialAdapter,
                    build,
                )?,
            },
            (true, true) => {
                let blend_adapter = upload_stack_artifact(
                    context,
                    manifest,
                    progress,
                    crate::cli::ArtifactName::BlendAdapter,
                    build,
                )?;
                let custodial_adapter = upload_stack_artifact(
                    context,
                    manifest,
                    progress,
                    crate::cli::ArtifactName::CustodialAdapter,
                    build,
                )?;
                StackAdapterArtifactHashes::Both {
                    blend_adapter,
                    custodial_adapter,
                }
            }
        };
        Ok(Self {
            vault,
            governance,
            share_token,
            proxy_4626,
            curator_proxy,
            adapters,
        })
    }
}

fn upload_stack_artifact<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &mut Manifest,
    progress: &DeploymentProgress,
    name: crate::cli::ArtifactName,
    build: bool,
) -> anyhow::Result<String> {
    let spec = ArtifactSpec::from_name(name);
    progress.step(format!("WASM {} upload/reuse", spec.key), || {
        let hash = ensure_uploaded(
            context.stellar(),
            manifest,
            &context.cli().workspace_path,
            spec,
            build,
        )?;
        context.checkpoint(manifest)?;
        Ok(hash)
    })
}

pub(in crate::commands) fn resolve_governance_timelock_ns(
    manifest: &Manifest,
    args: &DeployStackArgs,
) -> anyhow::Result<u64> {
    args.governance_timelock_ns
        .or_else(|| {
            if args.force_new {
                return None;
            }
            manifest
                .contracts
                .get("governance")
                .and_then(|record| record.constructor_args.get("timelock_ns"))
                .and_then(|value| value.parse::<u64>().ok())
        })
        .context("new governance deployment requires --governance-timelock-ns")
}

#[allow(
    clippy::too_many_lines,
    reason = "deployment orchestration is clearer in sequence"
)]
pub(in crate::commands) fn deploy_stack<E: CommandExecutor>(
    deployment: &mut DeploymentContext<'_, '_, '_, E>,
    args: &DeployStackArgs,
) -> anyhow::Result<Response> {
    let (context, manifest) = deployment.parts();
    let cli = context.cli();
    let stellar = context.stellar();
    if args.governance_timelock_ns == Some(0) && !cli.allow_zero_timelock {
        anyhow::bail!("zero governance timelock requires --allow-zero-timelock");
    }
    let timelock_ns = resolve_governance_timelock_ns(manifest, args)?;

    let include_blend = !args.blend_pools.is_empty();
    let include_custodial = !args.custodians.is_empty();
    let requested_adapter_admin = validated_stack_adapter_admin(manifest, args)?;
    let admin = match &args.admin {
        Some(admin) => admin.to_string(),
        None => stellar.source_public_address()?,
    };
    let deploy_share_token = args.force_new || !manifest.contracts.contains_key("share_token");
    if deploy_share_token && !args.force_new {
        if let Some(vault) = contract_id(manifest, "vault") {
            anyhow::ensure!(
                admin != vault,
                "share-token admin must differ from the vault; choose a separately reachable admin"
            );
        }
    }
    let progress = DeploymentProgress::stack(
        cli,
        stack_progress_steps(
            include_blend,
            include_custodial,
            args.blend_pools.len(),
            args.custodians.len(),
        ),
    );
    let wasm_hashes = StackArtifactHashes::upload(
        context,
        manifest,
        &progress,
        include_blend,
        include_custodial,
        args.build,
    )?;

    let asset_token = progress.step("asset token record", || {
        let asset_token = if let Some(asset) = &args.asset_token {
            asset.to_string()
        } else if let Some(asset) = contract_id(manifest, "asset_token") {
            asset.to_string()
        } else {
            let _ = stellar.deploy_native_asset();
            stellar.native_asset_id()?
        };
        record_asset_token(context, manifest, &asset_token, args.asset_token.is_some())?;
        context.checkpoint(manifest)?;
        Ok(asset_token)
    })?;

    let vault = progress.step("vault deploy/reuse", || {
        let vault = deploy_contract_if_needed(
            context,
            manifest,
            ContractDeployment {
                key: "vault",
                wasm_hash: &wasm_hashes.vault,
                constructor_args: Vec::new(),
                constructor_summary: BTreeMap::new(),
                force_new: args.force_new,
                initialization: InitializationState::Pending,
            },
        )?;
        context.checkpoint(manifest)?;
        Ok(vault)
    })?;
    if requested_adapter_admin.is_some_and(|admin| admin.targets_vault(&vault)) {
        verify_vault_companion_upgrade(cli, stellar, &vault)?;
    }
    let share_token = progress.step("share token deploy/reuse", || {
        if deploy_share_token {
            anyhow::ensure!(
                admin != vault,
                "share-token admin must differ from the vault; choose a separately reachable admin"
            );
        }
        let share_decimals = args.share_decimals.to_string();
        let share_token = deploy_contract_if_needed(
            context,
            manifest,
            ContractDeployment {
                key: "share_token",
                wasm_hash: &wasm_hashes.share_token,
                constructor_args: vec![
                    "--admin".to_string(),
                    admin.clone(),
                    "--vault".to_string(),
                    vault.clone(),
                    "--name".to_string(),
                    args.share_name.clone(),
                    "--symbol".to_string(),
                    args.share_symbol.clone(),
                    "--decimals".to_string(),
                    args.share_decimals.to_string(),
                ],
                constructor_summary: map_args([
                    ("admin", admin.as_str()),
                    ("vault", vault.as_str()),
                    ("name", args.share_name.as_str()),
                    ("symbol", args.share_symbol.as_str()),
                    ("decimals", share_decimals.as_str()),
                ]),
                force_new: args.force_new,
                initialization: InitializationState::Complete,
            },
        )?;
        context.checkpoint(manifest)?;
        Ok(share_token)
    })?;
    let governance = progress.step("governance deploy/reuse", || {
        let governance = deploy_contract_if_needed(
            context,
            manifest,
            ContractDeployment {
                key: "governance",
                wasm_hash: &wasm_hashes.governance,
                constructor_args: vec![
                    "--admin".to_string(),
                    admin.clone(),
                    "--vault".to_string(),
                    vault.clone(),
                    "--timelock_ns".to_string(),
                    timelock_ns.to_string(),
                ],
                constructor_summary: map_args([
                    ("admin", admin.as_str()),
                    ("vault", vault.as_str()),
                    ("timelock_ns", &timelock_ns.to_string()),
                ]),
                force_new: args.force_new,
                initialization: InitializationState::Complete,
            },
        )?;
        context.checkpoint(manifest)?;
        Ok(governance)
    })?;
    let adapter_admin = if include_blend || include_custodial {
        let adapter_admin = requested_adapter_admin
            .context("adapter deployment requires --adapter-admin <address|vault>")?;
        validate_adapter_admin(
            adapter_admin,
            Some(&vault),
            Some(&governance),
            include_custodial.then_some(asset_token.as_str()),
        )?;
        Some(adapter_admin)
    } else {
        None
    };

    progress.step("vault initialize", || {
        initialize_vault_if_needed(
            context,
            manifest,
            &vault,
            &admin,
            &governance,
            &asset_token,
            &share_token,
            args.virtual_shares,
            args.virtual_assets,
        )?;
        context.checkpoint(manifest)
    })?;

    let proxy_4626 = progress.step("ERC-4626 proxy deploy/reuse", || {
        let proxy_4626 = deploy_contract_if_needed(
            context,
            manifest,
            ContractDeployment {
                key: "proxy_4626",
                wasm_hash: &wasm_hashes.proxy_4626,
                constructor_args: Vec::new(),
                constructor_summary: BTreeMap::new(),
                force_new: args.force_new,
                initialization: InitializationState::Pending,
            },
        )?;
        context.checkpoint(manifest)?;
        Ok(proxy_4626)
    })?;
    progress.step("ERC-4626 proxy initialize", || {
        initialize_proxy_if_needed(
            context,
            manifest,
            "proxy_4626",
            &proxy_4626,
            vec![
                "--vault_address".to_string(),
                vault.clone(),
                "--asset_token".to_string(),
                asset_token.clone(),
                "--share_token".to_string(),
                share_token.clone(),
            ],
        )?;
        context.checkpoint(manifest)
    })?;

    let curator_proxy = progress.step("curator proxy deploy/reuse", || {
        let (constructor_args, constructor_summary) =
            if !args.force_new && manifest.contracts.contains_key("curator_proxy") {
                (Vec::new(), BTreeMap::new())
            } else {
                let initialization_authority = stellar.source_public_address()?;
                (
                    vec![
                        "--initialization_authority".to_string(),
                        initialization_authority.clone(),
                    ],
                    map_args([(
                        CURATOR_PROXY_INITIALIZATION_AUTHORITY_ARG,
                        initialization_authority.as_str(),
                    )]),
                )
            };
        let curator_proxy = deploy_contract_if_needed(
            context,
            manifest,
            ContractDeployment {
                key: "curator_proxy",
                wasm_hash: &wasm_hashes.curator_proxy,
                constructor_args,
                constructor_summary,
                force_new: args.force_new,
                initialization: InitializationState::Pending,
            },
        )?;
        context.checkpoint(manifest)?;
        Ok(curator_proxy)
    })?;
    progress.step("curator proxy initialize", || {
        initialize_proxy_if_needed(
            context,
            manifest,
            "curator_proxy",
            &curator_proxy,
            vec![
                "--vault_address".to_string(),
                vault.clone(),
                "--governance_address".to_string(),
                governance.clone(),
            ],
        )?;
        let needs_version_verification =
            manifest
                .contracts
                .get("curator_proxy")
                .is_some_and(|record| {
                    curator_proxy_needs_version_verification(record, &wasm_hashes.curator_proxy)
                });
        if needs_version_verification {
            record_standard_curator_proxy_initialization_if_missing(manifest, &vault, &governance)?;
            context.checkpoint(manifest)?;
            verify_curator_proxy_version(cli, stellar, &curator_proxy)?;
            mark_curator_proxy_version_discovery(manifest)?;
            context.checkpoint(manifest)?;
        }
        context.checkpoint(manifest)
    })?;

    let blend_adapters = match &wasm_hashes.adapters {
        StackAdapterArtifactHashes::None | StackAdapterArtifactHashes::Custodial { .. } => {
            blend_adapter_statuses(manifest)
        }
        StackAdapterArtifactHashes::Blend { blend_adapter }
        | StackAdapterArtifactHashes::Both { blend_adapter, .. } => {
            progress.step("Blend adapters deploy/reuse", || {
                append_blend_adapters(
                    context,
                    manifest,
                    blend_adapter,
                    adapter_admin.context("blend adapter deployment requires --adapter-admin")?,
                    &vault,
                    &args.blend_pools,
                    args.force_new,
                )
            })?
        }
    };
    let custodial_adapters = match &wasm_hashes.adapters {
        StackAdapterArtifactHashes::None | StackAdapterArtifactHashes::Blend { .. } => {
            custodial_adapter_statuses(manifest)
        }
        StackAdapterArtifactHashes::Custodial { custodial_adapter }
        | StackAdapterArtifactHashes::Both {
            custodial_adapter, ..
        } => progress.step("Custodial adapters deploy/reuse", || {
            append_custodial_adapters(
                context,
                manifest,
                custodial_adapter,
                adapter_admin.context("custodial adapter deployment requires --adapter-admin")?,
                &vault,
                &asset_token,
                &args.custodians,
                args.force_new,
            )
        })?,
    };
    progress.finish();

    let contracts = StackContracts {
        vault,
        governance,
        share_token,
        asset_token,
        proxy_4626,
        curator_proxy,
    };
    Ok(Response::Status(contracts.into_status(
        manifest.network.clone(),
        blend_adapters,
        custodial_adapters,
    )))
}

pub(in crate::commands) fn deploy_resume<E: CommandExecutor>(
    deployment: &mut DeploymentContext<'_, '_, '_, E>,
    args: &DeployStackArgs,
) -> anyhow::Result<Response> {
    {
        let (context, manifest) = deployment.parts();
        let reconcile = reconcile_manifest(context.stellar(), manifest, true);
        anyhow::ensure!(
            reconcile.safe_to_resume,
            "manifest is not safe to resume; run `tmplr-soroban-vault reconcile --json` or `tmplr-soroban-vault deploy repair --json` for the repair plan"
        );
        apply_reconcile_safe_manifest_updates(context, manifest, &reconcile)?;
    }
    deploy_stack(deployment, args)
}
