use std::collections::BTreeMap;

use anyhow::Context;
use serde::Serialize;
use templar_curator_proxy_soroban::AllocationDelta;
use templar_soroban_shared_types::VaultCommand as WireVaultCommand;

use crate::{
    artifacts::{ensure_uploaded, ArtifactSpec},
    cli::{
        AdapterArgs, AdapterCommand, Cli, Commands, CuratorCommand, DeployCommand, ExtendTtlArgs,
        GovernanceCommand, ShareTokenCommand, UserCommand,
    },
    manifest::{ContractRecord, Manifest},
    stellar::{CommandExecutor, CommandOutput, Stellar},
    types::{AddressStr, FeeParamsArg, SupplyQueueEntryArg},
};

pub fn run<E: CommandExecutor>(cli: &Cli, executor: &E) -> anyhow::Result<()> {
    guard_write(cli)?;
    let mut manifest = Manifest::load_or_new(
        &cli.state,
        &cli.network,
        cli.rpc_url.clone(),
        cli.source_account.clone(),
    )?;
    let stellar = Stellar::new(cli, executor);
    let result = match &cli.command {
        Commands::Deploy(args) => match &args.command {
            DeployCommand::Stack(stack) => deploy_stack(cli, &stellar, &mut manifest, stack),
            DeployCommand::Adapters(adapters) => {
                deploy_adapters(cli, &stellar, &mut manifest, adapters)
            }
            DeployCommand::Wasm(wasm) => {
                let spec = ArtifactSpec::from_name(wasm.artifact);
                let hash = ensure_uploaded(
                    &stellar,
                    &mut manifest,
                    &cli.workspace_path,
                    spec,
                    wasm.build,
                )?;
                Ok(Response::message(format!("{} wasm hash: {hash}", spec.key)))
            }
        },
        Commands::User(args) => run_user(&stellar, &manifest, &args.command),
        Commands::Curator(args) => run_curator(&stellar, &manifest, &args.command),
        Commands::Governance(args) => run_governance(&stellar, &manifest, &args.command),
        Commands::ShareToken(args) => run_share_token(&stellar, &manifest, &args.command),
        Commands::Adapter(args) => run_adapter(&stellar, &manifest, args),
        Commands::ExtendTtl(args) => run_extend_ttl(&stellar, &manifest, args),
        Commands::Status => Ok(Response::Status(status_response(&manifest))),
        Commands::ExportEnv => Ok(Response::Env(export_env(&manifest))),
    }?;

    if cli.command.is_write() && !cli.dry_run {
        manifest.save(&cli.state)?;
    }
    print_response(&result, cli.json)
}

fn guard_write(cli: &Cli) -> anyhow::Result<()> {
    if cli.command.is_write() && cli.network == "mainnet" && !cli.allow_mainnet_write {
        anyhow::bail!("mainnet write blocked; pass --allow-mainnet-write to continue");
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "deployment orchestration is clearer in sequence"
)]
fn deploy_stack<E: CommandExecutor>(
    cli: &Cli,
    stellar: &Stellar<'_, E>,
    manifest: &mut Manifest,
    args: &crate::cli::DeployStackArgs,
) -> anyhow::Result<Response> {
    if args.governance_timelock_ns == Some(0) && !cli.allow_zero_timelock {
        anyhow::bail!("zero governance timelock requires --allow-zero-timelock");
    }

    let admin = match &args.admin {
        Some(admin) => admin.to_string(),
        None => stellar.keys_address(&cli.source_account)?,
    };

    let include_blend = !args.blend_pools.is_empty();
    let mut wasm_hashes = BTreeMap::new();
    for spec in ArtifactSpec::stack_artifacts(include_blend) {
        let hash = ensure_uploaded(stellar, manifest, &cli.workspace_path, spec, args.build)?;
        wasm_hashes.insert(spec.key.to_string(), hash);
    }

    let asset_token = if let Some(asset) = &args.asset_token {
        asset.to_string()
    } else if let Some(asset) = contract_id(manifest, "asset_token") {
        asset.to_string()
    } else {
        let _ = stellar.deploy_native_asset();
        stellar.native_asset_id()?
    };

    let vault = deploy_contract_if_needed(
        stellar,
        manifest,
        "vault",
        &wasm_hashes["vault"],
        Vec::new(),
        BTreeMap::new(),
        args.force_new,
    )?;
    let share_token = deploy_contract_if_needed(
        stellar,
        manifest,
        "share_token",
        &wasm_hashes["share_token"],
        vec![
            "--admin".to_string(),
            vault.clone(),
            "--vault".to_string(),
            vault.clone(),
            "--name".to_string(),
            args.share_name.clone(),
            "--symbol".to_string(),
            args.share_symbol.clone(),
            "--decimals".to_string(),
            args.share_decimals.to_string(),
        ],
        map_args([
            ("admin", vault.as_str()),
            ("vault", vault.as_str()),
            ("name", args.share_name.as_str()),
            ("symbol", args.share_symbol.as_str()),
        ]),
        args.force_new,
    )?;
    let timelock_ns = args
        .governance_timelock_ns
        .or_else(|| {
            manifest
                .contracts
                .get("governance")
                .and_then(|record| record.constructor_args.get("timelock_ns"))
                .and_then(|value| value.parse::<u64>().ok())
        })
        .context("new governance deployment requires --governance-timelock-ns")?;
    let governance = deploy_contract_if_needed(
        stellar,
        manifest,
        "governance",
        &wasm_hashes["governance"],
        vec![
            "--admin".to_string(),
            admin.clone(),
            "--vault".to_string(),
            vault.clone(),
            "--timelock_ns".to_string(),
            timelock_ns.to_string(),
        ],
        map_args([
            ("admin", admin.as_str()),
            ("vault", vault.as_str()),
            ("timelock_ns", &timelock_ns.to_string()),
        ]),
        args.force_new,
    )?;

    initialize_vault_if_needed(
        stellar,
        manifest,
        &vault,
        &admin,
        &governance,
        &asset_token,
        &share_token,
        args.virtual_shares,
        args.virtual_assets,
    )?;

    let proxy_4626 = deploy_contract_if_needed(
        stellar,
        manifest,
        "proxy_4626",
        &wasm_hashes["proxy_4626"],
        Vec::new(),
        BTreeMap::new(),
        args.force_new,
    )?;
    initialize_proxy_if_needed(
        stellar,
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

    let curator_proxy = deploy_contract_if_needed(
        stellar,
        manifest,
        "curator_proxy",
        &wasm_hashes["curator_proxy"],
        Vec::new(),
        BTreeMap::new(),
        args.force_new,
    )?;
    initialize_proxy_if_needed(
        stellar,
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

    let blend_adapters = append_blend_adapters(
        stellar,
        manifest,
        &wasm_hashes["blend_adapter"],
        &governance,
        &vault,
        &args.blend_pools,
        args.force_new,
    )?;

    record_asset_token(manifest, &asset_token, args.asset_token.is_some())?;

    Ok(Response::Status(StatusResponse {
        network: manifest.network.clone(),
        vault: Some(vault),
        share_token: Some(share_token),
        governance: Some(governance),
        asset_token: Some(asset_token),
        proxy_4626: Some(proxy_4626),
        curator_proxy: Some(curator_proxy),
        blend_adapters,
    }))
}

fn deploy_adapters<E: CommandExecutor>(
    cli: &Cli,
    stellar: &Stellar<'_, E>,
    manifest: &mut Manifest,
    args: &crate::cli::DeployAdaptersArgs,
) -> anyhow::Result<Response> {
    anyhow::ensure!(
        !args.blend_pools.is_empty(),
        "deploy adapters requires at least one --blend-pool"
    );

    record_imported_contract_if_provided(manifest, "vault", args.vault.as_ref())?;
    record_imported_contract_if_provided(manifest, "governance", args.governance.as_ref())?;
    if let Some(asset_token) = &args.asset_token {
        record_asset_token(manifest, asset_token.as_str(), true)?;
    }

    let vault = required_contract(manifest, "vault")?.to_string();
    let governance = required_contract(manifest, "governance")?.to_string();
    let wasm_hash = ensure_uploaded(
        stellar,
        manifest,
        &cli.workspace_path,
        ArtifactSpec::from_name(crate::cli::ArtifactName::BlendAdapter),
        args.build,
    )?;
    let blend_adapters = append_blend_adapters(
        stellar,
        manifest,
        &wasm_hash,
        &governance,
        &vault,
        &args.blend_pools,
        args.force_new,
    )?;

    Ok(Response::Status(StatusResponse {
        network: manifest.network.clone(),
        vault: Some(vault),
        share_token: contract_id(manifest, "share_token").map(ToString::to_string),
        governance: Some(governance),
        asset_token: contract_id(manifest, "asset_token").map(ToString::to_string),
        proxy_4626: contract_id(manifest, "proxy_4626").map(ToString::to_string),
        curator_proxy: contract_id(manifest, "curator_proxy").map(ToString::to_string),
        blend_adapters,
    }))
}

fn deploy_contract_if_needed<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &mut Manifest,
    key: &str,
    wasm_hash: &str,
    constructor_args: Vec<String>,
    constructor_summary: BTreeMap<String, String>,
    force_new: bool,
) -> anyhow::Result<String> {
    if !force_new {
        if let Some(record) = manifest.contracts.get(key) {
            return Ok(record.contract_id.clone());
        }
    }
    let contract_id = stellar.deploy(wasm_hash, constructor_args)?;
    manifest.contracts.insert(
        key.to_string(),
        ContractRecord {
            contract_id: contract_id.clone(),
            wasm_hash: wasm_hash.to_string(),
            salt: None,
            constructor_args: constructor_summary,
            deploy_tx: None,
            initialized: false,
        },
    );
    Ok(contract_id)
}

fn append_blend_adapters<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &mut Manifest,
    wasm_hash: &str,
    governance: &str,
    vault: &str,
    pools: &[AddressStr],
    force_new: bool,
) -> anyhow::Result<Vec<BlendAdapterStatus>> {
    for pool in pools {
        if !force_new && blend_adapter_by_pool(manifest, pool).is_some() {
            continue;
        }
        let key = next_blend_adapter_key(manifest);
        let adapter = deploy_contract_if_needed(
            stellar,
            manifest,
            &key,
            wasm_hash,
            vec![
                "--admin".to_string(),
                governance.to_string(),
                "--vault".to_string(),
                vault.to_string(),
                "--pool".to_string(),
                pool.to_string(),
            ],
            map_args([
                ("admin", governance),
                ("vault", vault),
                ("pool", pool.as_str()),
            ]),
            force_new,
        )?;
        if let Some(record) = manifest.contracts.get_mut(&key) {
            record.contract_id = adapter;
        }
    }
    Ok(blend_adapter_statuses(manifest))
}

fn record_imported_contract_if_provided(
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
    Ok(())
}

fn record_asset_token(
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
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn initialize_vault_if_needed<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &mut Manifest,
    vault: &str,
    admin: &str,
    governance: &str,
    asset_token: &str,
    share_token: &str,
    virtual_shares: i128,
    virtual_assets: i128,
) -> anyhow::Result<()> {
    if manifest
        .contracts
        .get("vault")
        .is_some_and(|record| record.initialized)
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
    if let Some(record) = manifest.contracts.get_mut("vault") {
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
    }
    Ok(())
}

fn initialize_proxy_if_needed<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &mut Manifest,
    key: &str,
    contract_id: &str,
    args: Vec<String>,
) -> anyhow::Result<()> {
    if manifest
        .contracts
        .get(key)
        .is_some_and(|record| record.initialized)
    {
        return Ok(());
    }
    stellar.invoke(contract_id, "initialize", args)?;
    if let Some(record) = manifest.contracts.get_mut(key) {
        record.initialized = true;
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "keeps user command routing local and explicit"
)]
fn run_user<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    command: &UserCommand,
) -> anyhow::Result<Response> {
    match command {
        UserCommand::Deposit {
            operator,
            receiver,
            assets,
            min_shares_out,
        } => {
            let receiver = receiver.as_ref().unwrap_or(operator);
            if let Some(proxy) = contract_id(manifest, "proxy_4626") {
                invoke_response(stellar.invoke(
                    proxy,
                    "deposit_with_min",
                    args([
                        ("--operator", operator.as_str()),
                        ("--assets", &assets.to_string()),
                        ("--receiver", receiver.as_str()),
                        ("--min_shares_out", &min_shares_out.to_string()),
                    ]),
                )?)
            } else {
                execute_vault(
                    stellar,
                    manifest,
                    WireVaultCommand::DepositWithMin {
                        owner: operator.to_string(),
                        receiver: receiver.to_string(),
                        assets: *assets,
                        min_shares_out: *min_shares_out,
                    },
                )
            }
        }
        UserCommand::Mint {
            operator,
            receiver,
            shares,
        } => {
            let receiver = receiver.as_ref().unwrap_or(operator);
            let proxy = required_contract(manifest, "proxy_4626")?;
            invoke_response(stellar.invoke(
                proxy,
                "mint",
                args([
                    ("--operator", operator.as_str()),
                    ("--shares", &shares.to_string()),
                    ("--receiver", receiver.as_str()),
                ]),
            )?)
        }
        UserCommand::Withdraw {
            operator,
            receiver,
            owner,
            assets,
            max_shares_burned,
        } => {
            let owner = owner.as_ref().unwrap_or(operator);
            let receiver = receiver.as_ref().unwrap_or(operator);
            let proxy = required_contract(manifest, "proxy_4626")?;
            invoke_response(stellar.invoke(
                proxy,
                "withdraw",
                args([
                    ("--operator", operator.as_str()),
                    ("--assets", &assets.to_string()),
                    ("--receiver", receiver.as_str()),
                    ("--owner", owner.as_str()),
                    (
                        "--max_shares_burned",
                        &max_shares_burned.unwrap_or(*assets).to_string(),
                    ),
                ]),
            )?)
        }
        UserCommand::Redeem {
            operator,
            receiver,
            owner,
            shares,
            min_assets_out,
        } => {
            let owner = owner.as_ref().unwrap_or(operator);
            let receiver = receiver.as_ref().unwrap_or(operator);
            let proxy = required_contract(manifest, "proxy_4626")?;
            invoke_response(stellar.invoke(
                proxy,
                "redeem",
                args([
                    ("--operator", operator.as_str()),
                    ("--shares", &shares.to_string()),
                    ("--receiver", receiver.as_str()),
                    ("--owner", owner.as_str()),
                    ("--min_assets_out", &min_assets_out.to_string()),
                ]),
            )?)
        }
        UserCommand::RequestWithdraw {
            owner,
            receiver,
            shares,
            min_assets_out,
        } => {
            let receiver = receiver.as_ref().unwrap_or(owner);
            execute_vault(
                stellar,
                manifest,
                WireVaultCommand::RequestWithdraw {
                    owner: owner.to_string(),
                    receiver: receiver.to_string(),
                    shares: *shares,
                    min_assets_out: *min_assets_out,
                },
            )
        }
        UserCommand::ExecuteWithdraw { operator } => {
            if let Some(proxy) = contract_id(manifest, "proxy_4626") {
                invoke_response(stellar.invoke(
                    proxy,
                    "execute_withdraw",
                    args([("--operator", operator.as_str())]),
                )?)
            } else {
                execute_vault(
                    stellar,
                    manifest,
                    WireVaultCommand::ExecuteWithdraw {
                        caller: operator.to_string(),
                    },
                )
            }
        }
        UserCommand::Balance { owner } => {
            let share = required_contract(manifest, "share_token")?;
            invoke_response(stellar.invoke(
                share,
                "balance",
                args([("--account", owner.as_str())]),
            )?)
        }
        UserCommand::Preview {
            owner,
            assets,
            shares,
        }
        | UserCommand::View {
            owner,
            assets,
            shares,
        } => {
            let target = contract_id(manifest, "proxy_4626")
                .or_else(|| contract_id(manifest, "vault"))
                .context("missing proxy_4626 or vault contract id in manifest")?;
            let function = if contract_id(manifest, "proxy_4626").is_some() {
                "preview"
            } else {
                "proxy_view"
            };
            invoke_response(stellar.invoke(
                target,
                function,
                args([
                    ("--owner", owner.as_str()),
                    ("--assets", &assets.to_string()),
                    ("--shares", &shares.to_string()),
                ]),
            )?)
        }
    }
}

fn run_curator<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    command: &CuratorCommand,
) -> anyhow::Result<Response> {
    match command {
        CuratorCommand::AllocateSupply {
            caller,
            market,
            amount,
        } => execute_allocation(
            stellar,
            manifest,
            caller,
            &AllocationDelta::Supply(*market, *amount),
        ),
        CuratorCommand::AllocateWithdraw {
            caller,
            market,
            amount,
        } => execute_allocation(
            stellar,
            manifest,
            caller,
            &AllocationDelta::Withdraw(*market, *amount),
        ),
        CuratorCommand::RefreshMarkets { caller, markets } => execute_vault(
            stellar,
            manifest,
            WireVaultCommand::RefreshMarkets {
                caller: caller.to_string(),
                markets: markets.clone(),
            },
        ),
        CuratorCommand::RefreshFees => {
            execute_vault(stellar, manifest, WireVaultCommand::RefreshFees)
        }
        CuratorCommand::ResyncIdle => {
            execute_vault(stellar, manifest, WireVaultCommand::ResyncIdleBalance)
        }
        CuratorCommand::SetAllowedAdapters {
            admin,
            adapters,
            auto_accept,
        } => submit_and_maybe_accept(
            stellar,
            manifest,
            admin.as_str(),
            "submit_set_allowed_adapters",
            vec![
                "--caller".to_string(),
                admin.to_string(),
                "--adapters".to_string(),
                serde_json::to_string(adapters)?,
            ],
            *auto_accept,
        ),
        CuratorCommand::SetSupplyQueue {
            admin,
            entries,
            auto_accept,
        } => submit_and_maybe_accept(
            stellar,
            manifest,
            admin.as_str(),
            "submit_set_supply_queue",
            vec![
                "--caller".to_string(),
                admin.to_string(),
                "--entries".to_string(),
                supply_queue_entries_json(entries)?,
            ],
            *auto_accept,
        ),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "keeps governance method names and typed argument routing visibly aligned with the contract ABI"
)]
fn run_governance<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    command: &GovernanceCommand,
) -> anyhow::Result<Response> {
    let governance = required_contract(manifest, "governance")?;
    match command {
        GovernanceCommand::Accept { admin, proposal_id } => invoke_response(stellar.invoke(
            governance,
            "accept",
            args([
                ("--caller", admin.as_str()),
                ("--proposal_id", &proposal_id.to_string()),
            ]),
        )?),
        GovernanceCommand::Revoke { admin, proposal_id } => invoke_response(stellar.invoke(
            governance,
            "revoke",
            args([
                ("--caller", admin.as_str()),
                ("--proposal_id", &proposal_id.to_string()),
            ]),
        )?),
        GovernanceCommand::Pending { proposal_id } => {
            if let Some(proposal_id) = proposal_id {
                invoke_response(stellar.invoke(
                    governance,
                    "pending",
                    args([("--proposal_id", &proposal_id.to_string())]),
                )?)
            } else {
                invoke_response(stellar.invoke(governance, "pending_ids", Vec::new())?)
            }
        }
        GovernanceCommand::Timelocks => {
            invoke_response(stellar.invoke(governance, "timelocks", Vec::new())?)
        }
        GovernanceCommand::SubmitSetAdmin { admin, new_admin } => invoke_response(stellar.invoke(
            governance,
            "submit_set_admin",
            args([
                ("--caller", admin.as_str()),
                ("--new_admin", new_admin.as_str()),
            ]),
        )?),
        GovernanceCommand::SubmitSetCurator { admin, new_curator } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_curator",
                args([
                    ("--caller", admin.as_str()),
                    ("--new_curator", new_curator.as_str()),
                ]),
            )?)
        }
        GovernanceCommand::SubmitSetGovernance {
            admin,
            new_governance,
        } => invoke_response(stellar.invoke(
            governance,
            "submit_set_governance",
            args([
                ("--caller", admin.as_str()),
                ("--governance", new_governance.as_str()),
            ]),
        )?),
        GovernanceCommand::SubmitSetPaused { admin, paused } => invoke_response(stellar.invoke(
            governance,
            "submit_set_paused",
            args([
                ("--caller", admin.as_str()),
                ("--paused", &paused.to_string()),
            ]),
        )?),
        GovernanceCommand::SubmitSetSupplyQueue { admin, entries } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_supply_queue",
                vec![
                    "--caller".to_string(),
                    admin.to_string(),
                    "--entries".to_string(),
                    supply_queue_entries_json(entries)?,
                ],
            )?)
        }
        GovernanceCommand::SubmitSetFees {
            admin,
            performance_fee_wad,
            performance_recipient,
            management_fee_wad,
            management_recipient,
            max_growth_rate_wad,
        } => {
            let fees = FeeParamsArg {
                performance_fee_wad: *performance_fee_wad,
                performance_recipient: performance_recipient.clone(),
                management_fee_wad: *management_fee_wad,
                management_recipient: management_recipient.clone(),
                max_growth_rate_wad: *max_growth_rate_wad,
            };
            invoke_response(stellar.invoke(
                governance,
                "submit_set_fees",
                args([
                    (
                        "--performance_fee_wad",
                        &fees.performance_fee_wad.to_string(),
                    ),
                    (
                        "--performance_recipient",
                        fees.performance_recipient.as_str(),
                    ),
                    ("--management_fee_wad", &fees.management_fee_wad.to_string()),
                    ("--management_recipient", fees.management_recipient.as_str()),
                    (
                        "--max_growth_rate_wad",
                        &option_i128_arg(fees.max_growth_rate_wad),
                    ),
                    ("--caller", admin.as_str()),
                ]),
            )?)
        }
        GovernanceCommand::SubmitSetRestrictions {
            admin,
            mode,
            accounts,
        } => invoke_response(stellar.invoke(
            governance,
            "submit_set_restrictions",
            vec![
                "--caller".to_string(),
                admin.to_string(),
                "--mode".to_string(),
                mode.as_u32().to_string(),
                "--accounts".to_string(),
                address_vec_json(accounts)?,
            ],
        )?),
        GovernanceCommand::SubmitSetSentinel { admin, sentinel } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_sentinel",
                args([
                    ("--caller", admin.as_str()),
                    ("--sentinel", sentinel.as_str()),
                ]),
            )?)
        }
        GovernanceCommand::SubmitSetAllocators { admin, allocators } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_allocators",
                vec![
                    "--caller".to_string(),
                    admin.to_string(),
                    "--allocators".to_string(),
                    address_vec_json(allocators)?,
                ],
            )?)
        }
        GovernanceCommand::SubmitSetAllowedAdapters { admin, adapters } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_allowed_adapters",
                vec![
                    "--caller".to_string(),
                    admin.to_string(),
                    "--adapters".to_string(),
                    address_vec_json(adapters)?,
                ],
            )?)
        }
        GovernanceCommand::SubmitSetTimelock {
            admin,
            kind,
            timelock_ns,
        } => invoke_response(stellar.invoke(
            governance,
            "submit_set_timelock",
            args([
                ("--caller", admin.as_str()),
                ("--kind", &kind.to_string()),
                ("--new_timelock_ns", &timelock_ns.to_string()),
            ]),
        )?),
        GovernanceCommand::SubmitSetCap {
            admin,
            market_id,
            cap,
        } => invoke_response(stellar.invoke(
            governance,
            "submit_set_cap",
            args([
                ("--caller", admin.as_str()),
                ("--market_id", &market_id.to_string()),
                ("--new_cap", &cap.to_string()),
            ]),
        )?),
        GovernanceCommand::SubmitRemoveMarket { admin, market_id } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_remove_market",
                args([
                    ("--caller", admin.as_str()),
                    ("--market_id", &market_id.to_string()),
                ]),
            )?)
        }
        GovernanceCommand::SubmitSetGroupCap { admin, group, cap } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_group_cap",
                args([
                    ("--caller", admin.as_str()),
                    ("--cap_group_id", group),
                    ("--new_cap", &cap.to_string()),
                ]),
            )?)
        }
        GovernanceCommand::SubmitSetGroupRelCap {
            admin,
            group,
            relative_cap,
        } => invoke_response(stellar.invoke(
            governance,
            "submit_set_group_rel_cap",
            args([
                ("--caller", admin.as_str()),
                ("--cap_group_id", group),
                ("--new_relative_cap_wad", &relative_cap.to_string()),
            ]),
        )?),
        GovernanceCommand::SubmitSetGroupMember {
            admin,
            market_id,
            group,
        } => invoke_response(stellar.invoke(
            governance,
            "submit_set_group_member",
            args([
                ("--caller", admin.as_str()),
                ("--market_id", &market_id.to_string()),
                ("--cap_group_id", group),
            ]),
        )?),
        GovernanceCommand::SubmitSetSkimRecipient { admin, recipient } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_skim_recipient",
                args([
                    ("--caller", admin.as_str()),
                    ("--recipient", recipient.as_str()),
                ]),
            )?)
        }
        GovernanceCommand::SubmitSkim { admin, token } => invoke_response(stellar.invoke(
            governance,
            "submit_skim",
            args([("--caller", admin.as_str()), ("--token", token.as_str())]),
        )?),
        GovernanceCommand::SubmitSetWithdrawalCooldown { admin, cooldown_ns } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_withdrawal_cooldown",
                args([
                    ("--caller", admin.as_str()),
                    ("--withdrawal_cooldown_ns", &cooldown_ns.to_string()),
                ]),
            )?)
        }
        GovernanceCommand::SubmitSetIdleResyncCooldown { admin, cooldown_ns } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_idle_resync_cooldown",
                args([
                    ("--caller", admin.as_str()),
                    ("--idle_resync_cooldown_ns", &cooldown_ns.to_string()),
                ]),
            )?)
        }
        GovernanceCommand::SubmitUpgrade { admin, wasm_hash } => invoke_response(stellar.invoke(
            governance,
            "submit_upgrade",
            args([
                ("--caller", admin.as_str()),
                ("--new_wasm_hash", wasm_hash.as_str()),
            ]),
        )?),
        GovernanceCommand::SubmitMigrate { admin } => invoke_response(stellar.invoke(
            governance,
            "submit_migrate",
            args([("--caller", admin.as_str())]),
        )?),
        GovernanceCommand::SubmitCancelMigration { admin } => invoke_response(stellar.invoke(
            governance,
            "submit_cancel_migration",
            args([("--caller", admin.as_str())]),
        )?),
        GovernanceCommand::Abdicate { admin, kind } => invoke_response(stellar.invoke(
            governance,
            "abdicate",
            args([("--caller", admin.as_str()), ("--kind", &kind.to_string())]),
        )?),
    }
}

fn run_share_token<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    command: &ShareTokenCommand,
) -> anyhow::Result<Response> {
    let share = required_contract(manifest, "share_token")?;
    match command {
        ShareTokenCommand::Balance { account } => invoke_response(stellar.invoke(
            share,
            "balance",
            args([("--account", account.as_str())]),
        )?),
        ShareTokenCommand::TotalSupply => {
            invoke_response(stellar.invoke(share, "total_supply", Vec::new())?)
        }
        ShareTokenCommand::Admin => invoke_response(stellar.invoke(share, "admin", Vec::new())?),
        ShareTokenCommand::Vault => invoke_response(stellar.invoke(share, "vault", Vec::new())?),
        ShareTokenCommand::ExtendTtl { caller } => invoke_response(stellar.invoke(
            share,
            "extend_ttl",
            args([("--caller", caller.as_str())]),
        )?),
    }
}

fn run_adapter<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    adapter_args: &AdapterArgs,
) -> anyhow::Result<Response> {
    let adapter = selected_blend_adapter(manifest, adapter_args)?;
    match &adapter_args.command {
        AdapterCommand::TotalAssets { asset } => invoke_response(stellar.invoke(
            adapter,
            "total_assets",
            args([("--asset", asset.as_str())]),
        )?),
        AdapterCommand::Admin => invoke_response(stellar.invoke(adapter, "admin", Vec::new())?),
        AdapterCommand::Vault => invoke_response(stellar.invoke(adapter, "vault", Vec::new())?),
        AdapterCommand::Pool => invoke_response(stellar.invoke(adapter, "pool", Vec::new())?),
        AdapterCommand::SetAdmin { caller, admin } => invoke_response(stellar.invoke(
            adapter,
            "set_admin",
            args([("--caller", caller.as_str()), ("--admin", admin.as_str())]),
        )?),
        AdapterCommand::AcceptAdmin { caller } => invoke_response(stellar.invoke(
            adapter,
            "accept_admin",
            args([("--caller", caller.as_str())]),
        )?),
        AdapterCommand::ExtendTtl { caller } => invoke_response(stellar.invoke(
            adapter,
            "extend_ttl",
            args([("--caller", caller.as_str())]),
        )?),
    }
}

fn run_extend_ttl<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    ttl_args: &ExtendTtlArgs,
) -> anyhow::Result<Response> {
    let mut extended = Vec::new();
    let mut skipped = Vec::new();

    if let Some(vault) = contract_id(manifest, "vault") {
        let payload = hex::encode(WireVaultCommand::ExtendTtl.encode());
        stellar.invoke(vault, "execute", args([("--payload", &payload)]))?;
        extended.push("vault".to_string());
    } else {
        skipped.push("vault".to_string());
    }

    if let Some(governance) = contract_id(manifest, "governance") {
        stellar.invoke(governance, "extend_ttl", Vec::new())?;
        extended.push("governance".to_string());
    } else {
        skipped.push("governance".to_string());
    }

    if let Some(proxy) = contract_id(manifest, "proxy_4626") {
        stellar.invoke(proxy, "extend_ttl", Vec::new())?;
        extended.push("proxy_4626".to_string());
    } else {
        skipped.push("proxy_4626".to_string());
    }

    let caller = if contract_id(manifest, "share_token").is_some()
        || !blend_adapter_statuses(manifest).is_empty()
    {
        Some(resolve_extend_ttl_caller(stellar, ttl_args)?)
    } else {
        None
    };

    if let Some(share) = contract_id(manifest, "share_token") {
        let caller = caller.as_ref().context("missing TTL caller")?;
        stellar.invoke(share, "extend_ttl", args([("--caller", caller.as_str())]))?;
        extended.push("share_token".to_string());
    } else {
        skipped.push("share_token".to_string());
    }

    let adapters = blend_adapter_statuses(manifest);
    if adapters.is_empty() {
        skipped.push("blend_adapters".to_string());
    } else {
        let caller = caller.as_ref().context("missing TTL caller")?;
        for adapter in adapters {
            stellar.invoke(
                &adapter.contract_id,
                "extend_ttl",
                args([("--caller", caller.as_str())]),
            )?;
            extended.push(adapter.key);
        }
    }

    for key in ["asset_token", "curator_proxy"] {
        if contract_id(manifest, key).is_some() {
            skipped.push(format!("{key}: no deployment-wide TTL entrypoint"));
        }
    }

    Ok(Response::ExtendTtl(ExtendTtlResponse { extended, skipped }))
}

fn resolve_extend_ttl_caller<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    args: &ExtendTtlArgs,
) -> anyhow::Result<String> {
    if let Some(caller) = &args.caller {
        return Ok(caller.to_string());
    }
    stellar.keys_address_source_account()
}

fn execute_allocation<E: CommandExecutor>(
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
fn execute_vault<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    command: WireVaultCommand,
) -> anyhow::Result<Response> {
    let vault = required_contract(manifest, "vault")?;
    let payload = hex::encode(command.encode());
    invoke_response(stellar.invoke(vault, "execute", args([("--payload", &payload)]))?)
}

fn submit_and_maybe_accept<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    admin: &str,
    submit_method: &str,
    submit_args: Vec<String>,
    auto_accept: bool,
) -> anyhow::Result<Response> {
    let governance = required_contract(manifest, "governance")?;
    let out = stellar.invoke(governance, submit_method, submit_args)?;
    if auto_accept {
        let proposal_id = parse_last_u64(&out.stdout)?;
        stellar.invoke(
            governance,
            "accept",
            args([
                ("--caller", admin),
                ("--proposal_id", &proposal_id.to_string()),
            ]),
        )?;
        Ok(Response::message(format!(
            "submitted and accepted proposal {proposal_id}"
        )))
    } else {
        invoke_response(out)
    }
}

fn supply_queue_entries_json(entries: &[SupplyQueueEntryArg]) -> anyhow::Result<String> {
    Ok(serde_json::to_string(entries)?)
}

fn address_vec_json(addresses: &[AddressStr]) -> anyhow::Result<String> {
    Ok(serde_json::to_string(addresses)?)
}

fn option_i128_arg(value: Option<i128>) -> String {
    value.map_or_else(|| "null".to_string(), |value| value.to_string())
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "keeps match arms uniform with fallible handlers"
)]
fn invoke_response(output: CommandOutput) -> anyhow::Result<Response> {
    Ok(Response::Command {
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

fn contract_id<'a>(manifest: &'a Manifest, key: &str) -> Option<&'a str> {
    manifest
        .contracts
        .get(key)
        .map(|record| record.contract_id.as_str())
}

fn required_contract<'a>(manifest: &'a Manifest, key: &str) -> anyhow::Result<&'a str> {
    contract_id(manifest, key).with_context(|| format!("missing {key} contract id in manifest"))
}

fn blend_adapter_key(index: usize) -> String {
    format!("blend_adapter_{index}")
}

fn next_blend_adapter_key(manifest: &Manifest) -> String {
    blend_adapter_key(next_blend_adapter_index(manifest))
}

fn next_blend_adapter_index(manifest: &Manifest) -> usize {
    let highest_index = manifest
        .contracts
        .keys()
        .filter_map(|key| {
            if key == "blend_adapter" {
                Some(0)
            } else {
                blend_adapter_index(key)
            }
        })
        .max();
    highest_index.map_or(0, |index| index + 1)
}

fn blend_adapter_by_pool<'a>(manifest: &'a Manifest, pool: &AddressStr) -> Option<&'a str> {
    manifest
        .contracts
        .iter()
        .find(|(key, record)| {
            is_blend_adapter_key(key)
                && record
                    .constructor_args
                    .get("pool")
                    .is_some_and(|value| value == pool.as_str())
        })
        .map(|(_, record)| record.contract_id.as_str())
}

fn selected_blend_adapter<'a>(
    manifest: &'a Manifest,
    args: &AdapterArgs,
) -> anyhow::Result<&'a str> {
    if let Some(key) = &args.adapter_key {
        return required_contract(manifest, key);
    }
    if let Some(pool) = &args.adapter_pool {
        return blend_adapter_by_pool(manifest, pool)
            .with_context(|| format!("missing Blend adapter for pool {pool}"));
    }

    let key = blend_adapter_key(args.adapter_index);
    contract_id(manifest, &key)
        .or_else(|| {
            if args.adapter_index == 0 {
                contract_id(manifest, "blend_adapter")
            } else {
                None
            }
        })
        .with_context(|| format!("missing {key} contract id in manifest"))
}

fn is_blend_adapter_key(key: &str) -> bool {
    key == "blend_adapter" || blend_adapter_index(key).is_some()
}

fn blend_adapter_index(key: &str) -> Option<usize> {
    key.strip_prefix("blend_adapter_")?.parse().ok()
}

fn args<const N: usize>(items: [(&str, &str); N]) -> Vec<String> {
    items
        .into_iter()
        .flat_map(|(key, value)| [key.to_string(), value.to_string()])
        .collect()
}

fn map_args<const N: usize>(items: [(&str, &str); N]) -> BTreeMap<String, String> {
    items
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn parse_last_u64(stdout: &str) -> anyhow::Result<u64> {
    stdout
        .split(|c: char| !c.is_ascii_digit())
        .rev()
        .find(|part| !part.is_empty())
        .context("no proposal id found in governance output")?
        .parse()
        .context("parse proposal id")
}

fn status_response(manifest: &Manifest) -> StatusResponse {
    StatusResponse {
        network: manifest.network.clone(),
        vault: contract_id(manifest, "vault").map(ToString::to_string),
        share_token: contract_id(manifest, "share_token").map(ToString::to_string),
        governance: contract_id(manifest, "governance").map(ToString::to_string),
        asset_token: contract_id(manifest, "asset_token").map(ToString::to_string),
        proxy_4626: contract_id(manifest, "proxy_4626").map(ToString::to_string),
        curator_proxy: contract_id(manifest, "curator_proxy").map(ToString::to_string),
        blend_adapters: blend_adapter_statuses(manifest),
    }
}

fn blend_adapter_statuses(manifest: &Manifest) -> Vec<BlendAdapterStatus> {
    let mut adapters = manifest
        .contracts
        .iter()
        .filter_map(|(key, record)| {
            let index = blend_adapter_index(key)?;
            Some((
                index,
                BlendAdapterStatus {
                    key: key.clone(),
                    contract_id: record.contract_id.clone(),
                    pool: record.constructor_args.get("pool").cloned(),
                },
            ))
        })
        .collect::<Vec<_>>();
    adapters.sort_by_key(|(index, _)| *index);
    let mut adapters = adapters
        .into_iter()
        .map(|(_, status)| status)
        .collect::<Vec<_>>();
    if adapters.is_empty() {
        if let Some(record) = manifest.contracts.get("blend_adapter") {
            adapters.push(BlendAdapterStatus {
                key: "blend_adapter".to_string(),
                contract_id: record.contract_id.clone(),
                pool: record.constructor_args.get("pool").cloned(),
            });
        }
    }
    adapters
}

fn export_env(manifest: &Manifest) -> Vec<(String, String)> {
    let mut values = vec![("SOROBAN_NETWORK".to_string(), manifest.network.clone())];
    for (env, key) in [
        ("SOROBAN_CONTRACT_ID", "vault"),
        ("SOROBAN_SHARE_TOKEN", "share_token"),
        ("SOROBAN_GOVERNANCE", "governance"),
        ("SOROBAN_ASSET_TOKEN", "asset_token"),
        ("SOROBAN_4626_PROXY", "proxy_4626"),
        ("SOROBAN_CURATOR_PROXY", "curator_proxy"),
    ] {
        if let Some(value) = contract_id(manifest, key) {
            values.push((env.to_string(), value.to_string()));
        }
    }
    for (index, adapter) in blend_adapter_statuses(manifest).into_iter().enumerate() {
        if index == 0 {
            values.push(("BLEND_ADAPTER_ID".to_string(), adapter.contract_id.clone()));
        }
        values.push((
            format!("BLEND_ADAPTER_{index}_ID"),
            adapter.contract_id.clone(),
        ));
        if let Some(pool) = adapter.pool {
            values.push((format!("BLEND_POOL_{index}_ID"), pool));
        }
    }
    values
}

fn print_response(response: &Response, json: bool) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string(response)?);
        return Ok(());
    }
    match response {
        Response::Message { message } => println!("{message}"),
        Response::Command { stdout, stderr } => {
            if !stdout.is_empty() {
                println!("{stdout}");
            }
            if !stderr.is_empty() {
                eprintln!("{stderr}");
            }
        }
        Response::Status(status) => {
            println!("Network: {}", status.network);
            print_optional("Vault", status.vault.as_ref());
            print_optional("Share Token", status.share_token.as_ref());
            print_optional("Governance", status.governance.as_ref());
            print_optional("Asset Token", status.asset_token.as_ref());
            print_optional("ERC-4626 Proxy", status.proxy_4626.as_ref());
            print_optional("Curator Proxy", status.curator_proxy.as_ref());
            if status.blend_adapters.is_empty() {
                println!("Blend Adapters: not deployed");
            } else {
                for adapter in &status.blend_adapters {
                    println!(
                        "Blend Adapter {}: {}{}",
                        adapter.key,
                        adapter.contract_id,
                        adapter
                            .pool
                            .as_ref()
                            .map_or_else(String::new, |pool| format!(" (pool {pool})"))
                    );
                }
            }
        }
        Response::Env(values) => {
            for (key, value) in values {
                println!("{key}={value}");
            }
        }
        Response::ExtendTtl(result) => {
            if result.extended.is_empty() {
                println!("Extended TTL: none");
            } else {
                println!("Extended TTL: {}", result.extended.join(", "));
            }
            if !result.skipped.is_empty() {
                println!("Skipped: {}", result.skipped.join(", "));
            }
        }
    }
    Ok(())
}

fn print_optional(label: &str, value: Option<&String>) {
    println!(
        "{}: {}",
        label,
        value.map_or("not deployed", String::as_str)
    );
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Response {
    Message { message: String },
    Command { stdout: String, stderr: String },
    Status(StatusResponse),
    Env(Vec<(String, String)>),
    ExtendTtl(ExtendTtlResponse),
}

impl Response {
    fn message(message: String) -> Self {
        Self::Message { message }
    }
}

#[derive(Debug, Serialize)]
struct StatusResponse {
    network: String,
    vault: Option<String>,
    share_token: Option<String>,
    governance: Option<String>,
    asset_token: Option<String>,
    proxy_4626: Option<String>,
    curator_proxy: Option<String>,
    blend_adapters: Vec<BlendAdapterStatus>,
}

#[derive(Debug, Serialize)]
struct BlendAdapterStatus {
    key: String,
    contract_id: String,
    pool: Option<String>,
}

#[derive(Debug, Serialize)]
struct ExtendTtlResponse {
    extended: Vec<String>,
    skipped: Vec<String>,
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Mutex};

    use crate::{
        artifacts::ArtifactSpec,
        cli::{
            ArtifactName, Cli, Commands, DeployArgs, DeployCommand, DeployStackArgs, ExtendTtlArgs,
            GovernanceArgs, UserArgs,
        },
        stellar::{CommandExecutor, CommandOutput},
    };

    use super::*;

    const ACCOUNT: &str = "GBRFSXJNPLMYJV7EBFTBZT2PU6KN5WWPX3UKHDAAQQT7BNS7QTFCS3AY";
    const CONTRACT: &str = "CDY3B7IXFN5L4OY4UFFS2FA4MAQWJZLJD76LW37S7HFVWRS3RPQ2SIXX";

    struct RecordingExecutor {
        calls: Mutex<Vec<(String, Vec<String>)>>,
    }

    impl RecordingExecutor {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<(String, Vec<String>)> {
            self.calls.lock().expect("lock calls").clone()
        }
    }

    impl CommandExecutor for RecordingExecutor {
        fn run(&self, program: &str, args: &[String]) -> anyhow::Result<CommandOutput> {
            self.calls
                .lock()
                .expect("lock calls")
                .push((program.to_string(), args.to_vec()));
            Ok(CommandOutput {
                stdout: CONTRACT.to_string(),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn parses_supply_queue_entries_to_governance_json() {
        let entries = [
            format!("0:{CONTRACT}")
                .parse::<SupplyQueueEntryArg>()
                .expect("first entry"),
            format!("7:{CONTRACT}")
                .parse::<SupplyQueueEntryArg>()
                .expect("second entry"),
        ];
        let encoded = supply_queue_entries_json(&entries).expect("parse entries");
        let value: serde_json::Value = serde_json::from_str(&encoded).expect("json");
        assert_eq!(value[0]["target_id"], 0);
        assert_eq!(value[1]["adapter"], CONTRACT);
    }

    #[test]
    fn export_env_uses_manifest_contracts() {
        let mut manifest = Manifest::new("testnet", None, "alice".to_string());
        manifest.contracts.insert(
            "vault".to_string(),
            ContractRecord {
                contract_id: "CV".to_string(),
                wasm_hash: "h".to_string(),
                salt: None,
                constructor_args: BTreeMap::new(),
                deploy_tx: None,
                initialized: true,
            },
        );
        assert!(
            export_env(&manifest).contains(&("SOROBAN_CONTRACT_ID".to_string(), "CV".to_string()))
        );
    }

    #[test]
    fn mainnet_write_requires_explicit_allow_flag() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cli = base_cli(
            dir.path().join("manifest.json"),
            Commands::User(UserArgs {
                command: UserCommand::Deposit {
                    operator: ACCOUNT.parse().expect("operator"),
                    receiver: None,
                    assets: 1,
                    min_shares_out: 0,
                },
            }),
        );
        let cli = Cli {
            network: "mainnet".to_string(),
            ..cli
        };

        let err = run(&cli, &RecordingExecutor::new()).expect_err("mainnet write blocked");
        assert!(err.to_string().contains("mainnet write blocked"));
    }

    #[test]
    fn user_deposit_prefers_erc4626_proxy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join("manifest.json");
        let mut manifest = Manifest::new("testnet", None, "alice".to_string());
        manifest.contracts.insert(
            "proxy_4626".to_string(),
            ContractRecord {
                contract_id: "CPROXY".to_string(),
                wasm_hash: "hash".to_string(),
                salt: None,
                constructor_args: BTreeMap::new(),
                deploy_tx: None,
                initialized: true,
            },
        );
        manifest.save(&state).expect("save manifest");
        let cli = base_cli(
            state,
            Commands::User(UserArgs {
                command: UserCommand::Deposit {
                    operator: ACCOUNT.parse().expect("operator"),
                    receiver: Some(ACCOUNT.parse().expect("receiver")),
                    assets: 11,
                    min_shares_out: 7,
                },
            }),
        );
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("run deposit");

        let calls = executor.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "stellar");
        assert!(calls[0].1.windows(2).any(|pair| pair == ["--id", "CPROXY"]));
        assert!(calls[0].1.iter().any(|arg| arg == "deposit_with_min"));
        assert!(calls[0].1.windows(2).any(|pair| pair == ["--assets", "11"]));
    }

    #[test]
    fn deploy_adapters_appends_new_pool_to_existing_manifest() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fake_blend_wasm(dir.path());
        let state = dir.path().join("manifest.json");
        let mut manifest = Manifest::new("testnet", None, "alice".to_string());
        manifest
            .contracts
            .insert("vault".to_string(), imported_record(CONTRACT));
        manifest
            .contracts
            .insert("governance".to_string(), imported_record(CONTRACT));
        manifest
            .contracts
            .insert("asset_token".to_string(), imported_record(CONTRACT));
        manifest.contracts.insert(
            "blend_adapter_0".to_string(),
            ContractRecord {
                contract_id: CONTRACT.to_string(),
                wasm_hash: "hash".to_string(),
                salt: None,
                constructor_args: map_args([("pool", CONTRACT)]),
                deploy_tx: None,
                initialized: true,
            },
        );
        manifest.save(&state).expect("save manifest");
        let cli = Cli {
            workspace_path: dir.path().into(),
            command: Commands::Deploy(DeployArgs {
                command: DeployCommand::Adapters(crate::cli::DeployAdaptersArgs {
                    vault: None,
                    governance: None,
                    asset_token: Some(CONTRACT.parse().expect("asset token")),
                    blend_pools: vec![
                        CONTRACT.parse().expect("existing pool"),
                        ACCOUNT.parse().expect("new pool"),
                    ],
                    build: false,
                    force_new: false,
                }),
            }),
            ..base_cli(state.clone(), Commands::Status)
        };
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("deploy adapters");

        let loaded = Manifest::load_or_new(&state, "testnet", None, "alice".to_string())
            .expect("load manifest");
        assert!(loaded.contracts.contains_key("blend_adapter_0"));
        assert_eq!(
            loaded
                .contracts
                .get("blend_adapter_1")
                .expect("appended adapter")
                .constructor_args
                .get("pool")
                .map(String::as_str),
            Some(ACCOUNT)
        );
        let adapter_deploys = executor
            .calls()
            .iter()
            .filter(|(_, args)| args.iter().any(|arg| arg == "--pool"))
            .count();
        assert_eq!(adapter_deploys, 1);
    }

    #[test]
    fn dry_run_deploy_adapters_does_not_execute_or_write_manifest() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fake_blend_wasm(dir.path());
        let state = dir.path().join("manifest.json");
        manifest_with_governance_and_vault(&state);
        let before = fs::read_to_string(&state).expect("read manifest");
        let cli = Cli {
            workspace_path: dir.path().into(),
            dry_run: true,
            command: Commands::Deploy(DeployArgs {
                command: DeployCommand::Adapters(crate::cli::DeployAdaptersArgs {
                    vault: None,
                    governance: None,
                    asset_token: None,
                    blend_pools: vec![CONTRACT.parse().expect("pool")],
                    build: false,
                    force_new: false,
                }),
            }),
            ..base_cli(state.clone(), Commands::Status)
        };
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("dry-run deploy adapters");

        assert!(executor.calls().is_empty());
        let after = fs::read_to_string(&state).expect("read manifest");
        assert_eq!(before, after);
    }

    #[test]
    fn extend_ttl_runs_for_entire_ttl_capable_deployment_set() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join("manifest.json");
        let mut manifest = Manifest::new("testnet", None, "alice".to_string());
        for (key, contract_id) in [
            ("vault", "CVAULT"),
            ("governance", "CGOVERNANCE"),
            ("proxy_4626", "CPROXY4626"),
            ("share_token", "CSHARE"),
            ("curator_proxy", "CCURATORPROXY"),
            ("asset_token", "CASSET"),
        ] {
            manifest
                .contracts
                .insert(key.to_string(), imported_record(contract_id));
        }
        manifest
            .contracts
            .insert("blend_adapter_0".to_string(), imported_record("CADAPTER0"));
        manifest.save(&state).expect("save manifest");
        let cli = base_cli(
            state,
            Commands::ExtendTtl(ExtendTtlArgs {
                caller: Some(ACCOUNT.parse().expect("caller")),
            }),
        );
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("extend ttl");

        let calls = executor.calls();
        assert_eq!(calls.len(), 5);
        assert!(calls.iter().any(|(_, args)| args
            .windows(2)
            .any(|pair| pair == ["--id", "CVAULT"])
            && args.iter().any(|arg| arg == "execute")));
        for contract_id in ["CGOVERNANCE", "CPROXY4626", "CSHARE", "CADAPTER0"] {
            assert!(calls.iter().any(|(_, args)| args
                .windows(2)
                .any(|pair| pair == ["--id", contract_id])
                && args.iter().any(|arg| arg == "extend_ttl")));
        }
        assert!(!calls.iter().any(|(_, args)| args
            .windows(2)
            .any(|pair| pair == ["--id", "CCURATORPROXY"] || pair == ["--id", "CASSET"])));
    }

    #[test]
    fn governance_timelock_uses_typed_kind_and_direct_contract_method() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join("manifest.json");
        manifest_with_governance(&state);
        let cli = base_cli(
            state,
            Commands::Governance(GovernanceArgs {
                command: GovernanceCommand::SubmitSetTimelock {
                    admin: ACCOUNT.parse().expect("admin"),
                    kind: "supply-queue".parse().expect("kind"),
                    timelock_ns: 42,
                },
            }),
        );
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("run governance timelock");

        let calls = executor.calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].1.iter().any(|arg| arg == "submit_set_timelock"));
        assert!(calls[0]
            .1
            .windows(2)
            .any(|pair| pair == ["--kind", "SupplyQueue"]));
        assert!(calls[0]
            .1
            .windows(2)
            .any(|pair| pair == ["--new_timelock_ns", "42"]));
    }

    #[test]
    fn governance_restrictions_use_typed_mode_and_address_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join("manifest.json");
        manifest_with_governance(&state);
        let cli = base_cli(
            state,
            Commands::Governance(GovernanceArgs {
                command: GovernanceCommand::SubmitSetRestrictions {
                    admin: ACCOUNT.parse().expect("admin"),
                    mode: "blacklist".parse().expect("mode"),
                    accounts: vec![ACCOUNT.parse().expect("account")],
                },
            }),
        );
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("run governance restrictions");

        let calls = executor.calls();
        assert!(calls[0]
            .1
            .iter()
            .any(|arg| arg == "submit_set_restrictions"));
        assert!(calls[0].1.windows(2).any(|pair| pair == ["--mode", "1"]));
        assert!(calls[0]
            .1
            .windows(2)
            .any(|pair| pair[0] == "--accounts" && pair[1].contains(ACCOUNT)));
    }

    #[test]
    fn deploy_stack_deploys_one_blend_adapter_per_pool() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fake_stack_wasms(dir.path());
        let state = dir.path().join("manifest.json");
        let cli = Cli {
            workspace_path: dir.path().into(),
            command: Commands::Deploy(DeployArgs {
                command: DeployCommand::Stack(Box::new(DeployStackArgs {
                    admin: Some(ACCOUNT.parse().expect("admin")),
                    asset_token: Some(CONTRACT.parse().expect("asset token")),
                    governance_timelock_ns: Some(1_000),
                    virtual_shares: 0,
                    virtual_assets: 0,
                    share_name: "Templar Vault Share".to_string(),
                    share_symbol: "tvSHARE".to_string(),
                    share_decimals: 7,
                    blend_pools: vec![
                        CONTRACT.parse().expect("first pool"),
                        ACCOUNT.parse().expect("second pool"),
                    ],
                    build: false,
                    force_new: false,
                })),
            }),
            ..base_cli(state, Commands::Status)
        };
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("deploy stack");

        let calls = executor.calls();
        let adapter_deploys = calls
            .iter()
            .filter(|(_, args)| args.iter().any(|arg| arg == "--pool"))
            .count();
        assert_eq!(adapter_deploys, 2);
    }

    fn base_cli(state: std::path::PathBuf, command: Commands) -> Cli {
        Cli {
            network: "testnet".to_string(),
            rpc_url: None,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            source_account: "alice".to_string(),
            config_dir: None,
            state,
            workspace_path: ".".into(),
            json: true,
            dry_run: false,
            yes: false,
            allow_mainnet_write: false,
            allow_zero_timelock: false,
            command,
        }
    }

    fn manifest_with_governance(path: &std::path::Path) {
        let mut manifest = Manifest::new("testnet", None, "alice".to_string());
        manifest
            .contracts
            .insert("governance".to_string(), imported_record(CONTRACT));
        manifest.save(path).expect("save manifest");
    }

    fn manifest_with_governance_and_vault(path: &std::path::Path) {
        let mut manifest = Manifest::new("testnet", None, "alice".to_string());
        manifest
            .contracts
            .insert("governance".to_string(), imported_record(CONTRACT));
        manifest
            .contracts
            .insert("vault".to_string(), imported_record(CONTRACT));
        manifest.save(path).expect("save manifest");
    }

    fn imported_record(contract_id: &str) -> ContractRecord {
        ContractRecord {
            contract_id: contract_id.to_string(),
            wasm_hash: "predeployed".to_string(),
            salt: None,
            constructor_args: BTreeMap::new(),
            deploy_tx: None,
            initialized: true,
        }
    }

    fn write_fake_stack_wasms(root: &std::path::Path) {
        for artifact in [
            ArtifactName::Vault,
            ArtifactName::Governance,
            ArtifactName::ShareToken,
            ArtifactName::BlendAdapter,
            ArtifactName::Proxy4626,
            ArtifactName::CuratorProxy,
        ] {
            let path = ArtifactSpec::from_name(artifact).wasm_path(root);
            fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
            fs::write(path, format!("{artifact:?}")).expect("write wasm");
        }
    }

    fn write_fake_blend_wasm(root: &std::path::Path) {
        let path = ArtifactSpec::from_name(ArtifactName::BlendAdapter).wasm_path(root);
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(path, "blend").expect("write wasm");
    }
}
