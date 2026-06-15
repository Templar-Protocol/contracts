use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{self, IsTerminal, Write as _},
    path::Path,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use clap::CommandFactory;
use indicatif::{ProgressBar, ProgressStyle};
use serde::Serialize;
use templar_curator_proxy_soroban::AllocationDelta;
use templar_soroban_shared_types::VaultCommand as WireVaultCommand;
use tracing::{debug, info};

use crate::{
    artifacts::{ensure_uploaded, sha256_file, ArtifactSpec},
    cli::{
        AdapterArgs, AdapterCommand, Cli, Commands, CuratorCommand, DeployCommand,
        DeployPlanCommand, ExtendTtlArgs, GovernanceCommand, GovernanceSubmitAndWaitCommand,
        ProfileCommand, ReconcileArgs, ShareTokenCommand, UserCommand,
    },
    manifest::{ContractRecord, Manifest, TransactionRecord},
    profile,
    stellar::{CommandExecutor, CommandOutput, Stellar},
    types::{
        AddressStr, DecimalAmount, FeeParamsArg, ShareDecimalsArg, SourceAccount,
        SupplyQueueEntryArg,
    },
};

pub fn run<E: CommandExecutor>(cli: &Cli, executor: &E) -> anyhow::Result<()> {
    guard_write(cli)?;
    debug!(
        command = command_name(&cli.command),
        network = %cli.network,
        manifest = %cli.state.display(),
        dry_run = cli.dry_run,
        json = cli.json || cli.json_lines,
        "starting CLI command"
    );
    match &cli.command {
        Commands::Profile(args) => return run_profile(cli, &args.command),
        Commands::Completions { shell } => {
            print_completions(*shell);
            return Ok(());
        }
        Commands::Man => return print_manpage(),
        _ => {}
    }
    if matches!(cli.command, Commands::Doctor) {
        let result = run_doctor(cli, executor);
        return print_response(&result, cli);
    }
    let mut manifest = Manifest::load_or_new(&cli.state, &cli.network, cli.rpc_url.clone())?;
    let stellar = Stellar::new(cli, executor);
    confirm_dangerous_governance_change(cli, &stellar, &manifest)?;
    let result = match &cli.command {
        Commands::Doctor => unreachable!("doctor returns before manifest load"),
        Commands::Profile(_) | Commands::Completions { .. } | Commands::Man => {
            unreachable!("handled before manifest load")
        }
        Commands::Deploy(args) => match &args.command {
            DeployCommand::Plan(plan) => run_deploy_plan(cli, &manifest, plan),
            DeployCommand::Stack(stack) => deploy_stack(cli, &stellar, &mut manifest, stack),
            DeployCommand::Resume(stack) => deploy_resume(cli, &stellar, &mut manifest, stack),
            DeployCommand::Repair(repair) => Ok(run_reconcile(&stellar, &manifest, repair)),
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
                checkpoint_manifest(cli, &manifest)?;
                Ok(Response::message(format!("{} wasm hash: {hash}", spec.key)))
            }
        },
        Commands::Reconcile(args) => Ok(run_reconcile(&stellar, &manifest, args)),
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
        manifest
            .transactions
            .push(transaction_record(cli, &manifest, &result));
        debug!(
            manifest = %cli.state.display(),
            command = command_name(&cli.command),
            "recording transaction audit entry"
        );
        manifest.save(&cli.state)?;
    }
    info!(
        command = command_name(&cli.command),
        network = %cli.network,
        "completed CLI command"
    );
    print_response(&result, cli)
}

fn guard_write(cli: &Cli) -> anyhow::Result<()> {
    if cli.command.is_write() && cli.network == "mainnet" && !cli.allow_mainnet_write {
        anyhow::bail!("mainnet write blocked; pass --allow-mainnet-write to continue");
    }
    Ok(())
}

fn checkpoint_manifest(cli: &Cli, manifest: &Manifest) -> anyhow::Result<()> {
    if cli.dry_run {
        debug!(
            manifest = %cli.state.display(),
            "skipping manifest checkpoint during dry run"
        );
        return Ok(());
    }
    debug!(
        manifest = %cli.state.display(),
        contracts = manifest.contracts.len(),
        transactions = manifest.transactions.len(),
        "checkpointing deployment manifest"
    );
    manifest.save(&cli.state)
}

fn run_profile(cli: &Cli, command: &ProfileCommand) -> anyhow::Result<()> {
    let response = match command {
        ProfileCommand::Init { name, force } => {
            let path = profile::init_profile(name, *force)?;
            Response::message(format!("created profile {name} at {}", path.display()))
        }
    };
    print_response(&response, cli)
}

fn print_completions(shell: clap_complete::Shell) {
    let mut command = Cli::command();
    let bin_name = command.get_name().to_string();
    clap_complete::generate(shell, &mut command, bin_name, &mut io::stdout());
}

fn print_manpage() -> anyhow::Result<()> {
    let command = Cli::command();
    clap_mangen::Man::new(command).render(&mut io::stdout().lock())?;
    Ok(())
}

fn confirm_dangerous_governance_change<E: CommandExecutor>(
    cli: &Cli,
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
) -> anyhow::Result<()> {
    if cli.json || cli.json_lines {
        return Ok(());
    }
    let Some(diff) = governance_safety_diff(stellar, manifest, &cli.command)? else {
        return Ok(());
    };
    eprintln!("Dangerous governance change: {}", diff.title);
    for line in diff.lines {
        eprintln!("  {line}");
    }
    if cli.yes || cli.dry_run {
        return Ok(());
    }

    eprint!("Continue? Type 'yes' to submit: ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    anyhow::ensure!(
        matches!(answer.trim(), "yes" | "y"),
        "operation cancelled; pass --yes to confirm after reviewing the semantic diff"
    );
    Ok(())
}

fn governance_safety_diff<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    command: &Commands,
) -> anyhow::Result<Option<SafetyDiff>> {
    let Commands::Governance(governance_args) = command else {
        return Ok(None);
    };
    let governance = required_contract(manifest, "governance")?;
    let diff = match &governance_args.command {
        GovernanceCommand::SubmitSetAdmin { new_admin, .. } => SafetyDiff {
            title: "admin rotation".to_string(),
            lines: vec![format!(
                "admin: {} -> {}",
                view_or_unavailable(stellar, governance, "admin", Vec::new()),
                new_admin
            )],
        },
        GovernanceCommand::SubmitSetTimelock {
            kind, timelock_ns, ..
        } => SafetyDiff {
            title: "timelock update".to_string(),
            lines: vec![format!(
                "{kind} timelock_ns: {} -> {}",
                view_or_unavailable(
                    stellar,
                    governance,
                    "timelock_ns",
                    args([("--kind", &kind.to_string())]),
                ),
                timelock_ns
            )],
        },
        GovernanceCommand::SubmitSetSupplyQueue { admin, entries } => SafetyDiff {
            title: "supply queue replacement".to_string(),
            lines: vec![
                format!(
                    "current supply queue view: {}",
                    current_vault_view(stellar, manifest, admin)
                ),
                format!(
                    "proposed supply queue: {}",
                    supply_queue_entries_json(entries)?
                ),
            ],
        },
        GovernanceCommand::SubmitSetFees {
            admin,
            performance_fee_wad,
            performance_recipient,
            management_fee_wad,
            management_recipient,
            max_growth_rate_wad,
        } => SafetyDiff {
            title: "fee parameter update".to_string(),
            lines: vec![
                format!("current fee view: {}", current_vault_view(stellar, manifest, admin)),
                format!(
                    "proposed fees: performance_fee_wad={} performance_recipient={} management_fee_wad={} management_recipient={} max_growth_rate_wad={}",
                    performance_fee_wad,
                    performance_recipient,
                    management_fee_wad,
                    management_recipient,
                    option_i128_arg(*max_growth_rate_wad),
                ),
            ],
        },
        _ => return Ok(None),
    };
    Ok(Some(diff))
}

fn view_or_unavailable<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    contract_id: &str,
    function: &str,
    args: Vec<String>,
) -> String {
    match stellar.invoke(contract_id, function, args) {
        Ok(output) if !output.stdout.is_empty() => output.stdout,
        Ok(_) => "<empty>".to_string(),
        Err(error) => format!("unavailable ({error})"),
    }
}

fn current_vault_view<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    owner: &AddressStr,
) -> String {
    let Some(vault) = contract_id(manifest, "vault") else {
        return "unavailable (missing vault contract id in manifest)".to_string();
    };
    view_or_unavailable(
        stellar,
        vault,
        "proxy_view",
        args([
            ("--owner", owner.as_str()),
            ("--assets", "0"),
            ("--shares", "0"),
        ]),
    )
}

struct SafetyDiff {
    title: String,
    lines: Vec<String>,
}

fn run_doctor<E: CommandExecutor>(cli: &Cli, executor: &E) -> Response {
    let mut checks = Vec::new();

    let version_args = vec!["--version".to_string()];
    match executor.run("stellar", &version_args, &[], &[]) {
        Ok(output) => checks.push(DoctorCheck::pass(
            "stellar_version",
            first_nonempty_line(&output.stdout, &output.stderr)
                .unwrap_or("stellar CLI responded")
                .to_string(),
        )),
        Err(error) => checks.push(DoctorCheck::fail(
            "stellar_version",
            format!("stellar CLI is not runnable: {error}"),
        )),
    }

    checks.push(DoctorCheck::pass(
        "network",
        format!(
            "network={} passphrase={}",
            cli.network, cli.network_passphrase
        ),
    ));
    if let Some(rpc_url) = &cli.rpc_url {
        checks.push(DoctorCheck::pass("rpc_url", rpc_url.clone()));
    } else {
        checks.push(DoctorCheck::warn(
            "rpc_url",
            "no RPC URL override configured; Stellar CLI network config must provide one"
                .to_string(),
        ));
    }
    if cli.network == "mainnet" && !cli.allow_mainnet_write {
        checks.push(DoctorCheck::warn(
            "mainnet_guard",
            "mainnet is selected; write commands remain blocked until --allow-mainnet-write is passed"
                .to_string(),
        ));
    }

    checks.push(source_account_doctor_check(cli, executor));
    checks.push(manifest_writable_check(&cli.state));
    checks.extend(artifact_doctor_checks(cli));
    checks.extend(docker_mount_checks(cli));

    Response::Doctor(DoctorResponse {
        ok: checks
            .iter()
            .all(|check| check.status != DoctorStatus::Fail),
        checks,
    })
}

fn source_account_doctor_check<E: CommandExecutor>(cli: &Cli, executor: &E) -> DoctorCheck {
    if cli.source_account.is_some() {
        let stellar = Stellar::new(cli, executor);
        return match stellar.keys_address_source_account() {
            Ok(address) => DoctorCheck::pass(
                "source_account",
                format!("source identity/address resolves to {address}"),
            ),
            Err(error) => DoctorCheck::fail(
                "source_account",
                format!("source identity/address did not resolve: {error}"),
            ),
        };
    }
    if std::env::var_os("STELLAR_ACCOUNT").is_some() {
        return DoctorCheck::pass(
            "source_account",
            "STELLAR_ACCOUNT is set for child stellar signing; value is not inspected".to_string(),
        );
    }

    let args = vec!["keys".to_string(), "address".to_string()];
    match executor.run("stellar", &args, &[], &[]) {
        Ok(output) if !output.stdout.trim().is_empty() => DoctorCheck::pass(
            "source_account",
            format!(
                "default Stellar identity resolves to {}",
                output.stdout.trim()
            ),
        ),
        Ok(_) => DoctorCheck::warn(
            "source_account",
            "no --source-account, SOROBAN_IDENTITY, STELLAR_ACCOUNT, or default Stellar identity detected"
                .to_string(),
        ),
        Err(error) => DoctorCheck::warn(
            "source_account",
            format!("could not inspect default Stellar identity: {error}"),
        ),
    }
}

fn manifest_writable_check(path: &Path) -> DoctorCheck {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return DoctorCheck::warn(
            "manifest_writable",
            format!("manifest path {} has no parent directory", path.display()),
        );
    };
    if !parent.exists() {
        return DoctorCheck::warn(
            "manifest_writable",
            format!(
                "manifest directory {} does not exist yet; deploy will try to create it",
                parent.display()
            ),
        );
    }
    if !parent.is_dir() {
        return DoctorCheck::fail(
            "manifest_writable",
            format!("manifest parent {} is not a directory", parent.display()),
        );
    }

    let probe = parent.join(format!(
        ".tmplr-soroban-vault-cli-write-test-{}",
        std::process::id()
    ));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            DoctorCheck::pass(
                "manifest_writable",
                format!("manifest directory {} is writable", parent.display()),
            )
        }
        Err(error) => DoctorCheck::fail(
            "manifest_writable",
            format!(
                "cannot write manifest directory {}: {error}",
                parent.display()
            ),
        ),
    }
}

fn artifact_doctor_checks(cli: &Cli) -> Vec<DoctorCheck> {
    let workspace_manifest = cli.workspace_path.join("Cargo.toml");
    ArtifactSpec::stack_artifacts(true, true)
        .into_iter()
        .map(|spec| {
            let wasm_path = spec.wasm_path(&cli.workspace_path);
            if wasm_path.exists() {
                DoctorCheck::pass(
                    format!("artifact_{}", spec.key),
                    format!("found {}", wasm_path.display()),
                )
            } else if workspace_manifest.exists() {
                DoctorCheck::warn(
                    format!("artifact_{}", spec.key),
                    format!(
                        "{} is missing; deploy --build can build package {}",
                        wasm_path.display(),
                        spec.package
                    ),
                )
            } else {
                DoctorCheck::fail(
                    format!("artifact_{}", spec.key),
                    format!(
                        "{} is missing and {} was not found",
                        wasm_path.display(),
                        workspace_manifest.display()
                    ),
                )
            }
        })
        .collect()
}

fn docker_mount_checks(cli: &Cli) -> Vec<DoctorCheck> {
    if !Path::new("/.dockerenv").exists() {
        return vec![DoctorCheck::warn(
            "docker_mounts",
            "not running inside Docker; mount checks skipped".to_string(),
        )];
    }

    let mut checks = Vec::new();
    if cli.workspace_path.exists() {
        checks.push(DoctorCheck::pass(
            "docker_workspace_mount",
            format!("workspace path {} exists", cli.workspace_path.display()),
        ));
    } else {
        checks.push(DoctorCheck::fail(
            "docker_workspace_mount",
            format!("workspace path {} is missing", cli.workspace_path.display()),
        ));
    }

    let target = cli.workspace_path.join("target");
    if target.exists() {
        checks.push(DoctorCheck::pass(
            "docker_target_mount",
            format!("target path {} exists", target.display()),
        ));
    } else {
        checks.push(DoctorCheck::warn(
            "docker_target_mount",
            format!(
                "target path {} is missing; builds will not reuse host artifacts",
                target.display()
            ),
        ));
    }

    if let Some(config_dir) = &cli.config_dir {
        if config_dir.exists() {
            checks.push(DoctorCheck::pass(
                "docker_stellar_config_mount",
                format!("Stellar config path {} exists", config_dir.display()),
            ));
        } else {
            checks.push(DoctorCheck::warn(
                "docker_stellar_config_mount",
                format!(
                    "Stellar config path {} is missing; identities may not persist",
                    config_dir.display()
                ),
            ));
        }
    }
    checks
}

fn first_nonempty_line<'a>(first: &'a str, second: &'a str) -> Option<&'a str> {
    first
        .lines()
        .chain(second.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
}

struct DeploymentProgress {
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

fn stack_progress_steps(
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
        None => stellar.keys_address_source_account()?,
    };

    let include_blend = !args.blend_pools.is_empty();
    let include_custodial = !args.custodians.is_empty();
    let progress = DeploymentProgress::stack(
        cli,
        stack_progress_steps(
            include_blend,
            include_custodial,
            args.blend_pools.len(),
            args.custodians.len(),
        ),
    );
    let mut wasm_hashes = BTreeMap::new();
    for spec in ArtifactSpec::stack_artifacts(include_blend, include_custodial) {
        let hash = progress.step(format!("WASM {} upload/reuse", spec.key), || {
            let hash = ensure_uploaded(stellar, manifest, &cli.workspace_path, spec, args.build)?;
            checkpoint_manifest(cli, manifest)?;
            Ok(hash)
        })?;
        wasm_hashes.insert(spec.key.to_string(), hash);
    }

    let asset_token = progress.step("asset token record", || {
        let asset_token = if let Some(asset) = &args.asset_token {
            asset.to_string()
        } else if let Some(asset) = contract_id(manifest, "asset_token") {
            asset.to_string()
        } else {
            let _ = stellar.deploy_native_asset();
            stellar.native_asset_id()?
        };
        record_asset_token(cli, manifest, &asset_token, args.asset_token.is_some())?;
        checkpoint_manifest(cli, manifest)?;
        Ok(asset_token)
    })?;

    let vault = progress.step("vault deploy/reuse", || {
        let vault = deploy_contract_if_needed(
            cli,
            stellar,
            manifest,
            "vault",
            &wasm_hashes["vault"],
            Vec::new(),
            BTreeMap::new(),
            args.force_new,
        )?;
        checkpoint_manifest(cli, manifest)?;
        Ok(vault)
    })?;
    let share_token = progress.step("share token deploy/reuse", || {
        let share_token = deploy_contract_if_needed(
            cli,
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
                ("decimals", &args.share_decimals.to_string()),
            ]),
            args.force_new,
        )?;
        checkpoint_manifest(cli, manifest)?;
        Ok(share_token)
    })?;
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
    let governance = progress.step("governance deploy/reuse", || {
        let governance = deploy_contract_if_needed(
            cli,
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
        checkpoint_manifest(cli, manifest)?;
        Ok(governance)
    })?;

    progress.step("vault initialize", || {
        initialize_vault_if_needed(
            cli,
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
        checkpoint_manifest(cli, manifest)
    })?;

    let proxy_4626 = progress.step("ERC-4626 proxy deploy/reuse", || {
        let proxy_4626 = deploy_contract_if_needed(
            cli,
            stellar,
            manifest,
            "proxy_4626",
            &wasm_hashes["proxy_4626"],
            Vec::new(),
            BTreeMap::new(),
            args.force_new,
        )?;
        checkpoint_manifest(cli, manifest)?;
        Ok(proxy_4626)
    })?;
    progress.step("ERC-4626 proxy initialize", || {
        initialize_proxy_if_needed(
            cli,
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
        checkpoint_manifest(cli, manifest)
    })?;

    let curator_proxy = progress.step("curator proxy deploy/reuse", || {
        let curator_proxy = deploy_contract_if_needed(
            cli,
            stellar,
            manifest,
            "curator_proxy",
            &wasm_hashes["curator_proxy"],
            Vec::new(),
            BTreeMap::new(),
            args.force_new,
        )?;
        checkpoint_manifest(cli, manifest)?;
        Ok(curator_proxy)
    })?;
    progress.step("curator proxy initialize", || {
        initialize_proxy_if_needed(
            cli,
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
        checkpoint_manifest(cli, manifest)
    })?;

    let blend_adapters = if args.blend_pools.is_empty() {
        blend_adapter_statuses(manifest)
    } else {
        progress.step("Blend adapters deploy/reuse", || {
            append_blend_adapters(
                cli,
                stellar,
                manifest,
                &wasm_hashes["blend_adapter"],
                &governance,
                &vault,
                &args.blend_pools,
                args.force_new,
            )
        })?
    };
    let custodial_adapters = if args.custodians.is_empty() {
        custodial_adapter_statuses(manifest)
    } else {
        progress.step("Custodial adapters deploy/reuse", || {
            append_custodial_adapters(
                cli,
                stellar,
                manifest,
                &wasm_hashes["custodial_adapter"],
                &governance,
                &vault,
                &asset_token,
                &args.custodians,
                args.force_new,
            )
        })?
    };
    progress.finish();

    Ok(Response::Status(StatusResponse {
        network: manifest.network.clone(),
        vault: Some(vault),
        share_token: Some(share_token),
        governance: Some(governance),
        asset_token: Some(asset_token),
        proxy_4626: Some(proxy_4626),
        curator_proxy: Some(curator_proxy),
        blend_adapters,
        custodial_adapters,
    }))
}

fn deploy_resume<E: CommandExecutor>(
    cli: &Cli,
    stellar: &Stellar<'_, E>,
    manifest: &mut Manifest,
    args: &crate::cli::DeployStackArgs,
) -> anyhow::Result<Response> {
    let reconcile = reconcile_manifest(stellar, manifest, true);
    anyhow::ensure!(
        reconcile.safe_to_resume,
        "manifest is not safe to resume; run `tmplr-soroban-vault reconcile --json` or `tmplr-soroban-vault deploy repair --json` for the repair plan"
    );
    apply_reconcile_safe_manifest_updates(cli, manifest, &reconcile)?;
    deploy_stack(cli, stellar, manifest, args)
}

fn run_reconcile<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    args: &ReconcileArgs,
) -> Response {
    Response::Reconcile(reconcile_manifest(
        stellar,
        manifest,
        !args.skip_view_verification,
    ))
}

fn reconcile_manifest<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    verify_views: bool,
) -> ReconcileResponse {
    let mut keys = BTreeSet::<String>::new();
    for key in [
        "vault",
        "governance",
        "share_token",
        "asset_token",
        "proxy_4626",
        "curator_proxy",
    ] {
        keys.insert(key.to_string());
    }
    for key in manifest.contracts.keys() {
        keys.insert(key.clone());
    }

    let mut components = Vec::new();
    let mut repair_actions = Vec::new();
    for key in &keys {
        let component = reconcile_component(stellar, manifest, key, verify_views);
        repair_actions.extend(component.repair_actions.clone());
        components.push(component);
    }

    let safe_to_resume = components.iter().all(ReconcileComponent::safe_to_resume);
    let drift_detected = components
        .iter()
        .any(|component| component.status.is_drift() || !component.warnings.is_empty());
    let mut safe_next_steps = Vec::new();
    if safe_to_resume {
        safe_next_steps.push("deploy resume can continue missing manifest components and uninitialized recorded contracts".to_string());
    } else {
        safe_next_steps.push(
            "do not resume until mismatched, unknown, or missing recorded contracts are resolved"
                .to_string(),
        );
    }
    ReconcileResponse {
        safe_to_resume,
        drift_detected,
        components,
        repair_actions,
        safe_next_steps,
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "classification keeps all status transitions in one place"
)]
fn reconcile_component<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    key: &str,
    verify_views: bool,
) -> ReconcileComponent {
    let Some(record) = manifest.contracts.get(key) else {
        return ReconcileComponent {
            key: key.to_string(),
            contract_id: None,
            manifest_recorded: false,
            manifest_initialized: false,
            recorded_wasm_hash: None,
            chain_wasm_hash: None,
            status: ReconcileStatus::Missing,
            wiring: Vec::new(),
            warnings: Vec::new(),
            repair_actions: vec![format!(
                "{key}: deploy or import contract, then checkpoint manifest"
            )],
        };
    };

    let mut component = ReconcileComponent {
        key: key.to_string(),
        contract_id: Some(record.contract_id.clone()),
        manifest_recorded: true,
        manifest_initialized: record.initialized,
        recorded_wasm_hash: Some(record.wasm_hash.clone()),
        chain_wasm_hash: None,
        status: ReconcileStatus::Unknown,
        wiring: Vec::new(),
        warnings: Vec::new(),
        repair_actions: Vec::new(),
    };

    match stellar.fetch_contract_wasm_hash(&record.contract_id) {
        Ok(chain_hash) => {
            component.chain_wasm_hash = Some(chain_hash.clone());
            if should_compare_wasm_hash(&record.wasm_hash) && record.wasm_hash != chain_hash {
                component.status = ReconcileStatus::Mismatched;
                component.warnings.push(format!(
                    "manifest wasm hash {} does not match chain wasm hash {chain_hash}",
                    record.wasm_hash
                ));
                component.repair_actions.push(format!(
                    "{key}: inspect wrong-network or wrong-contract drift before editing manifest"
                ));
                return component;
            }
            component.status = if record.initialized {
                ReconcileStatus::Initialized
            } else {
                ReconcileStatus::Deployed
            };
        }
        Err(error) if looks_missing_contract(&error.to_string()) => {
            component.status = ReconcileStatus::Missing;
            component
                .warnings
                .push(format!("recorded contract was not found on chain: {error}"));
            component.repair_actions.push(format!(
                "{key}: verify network/RPC, then remove or replace stale manifest record manually"
            ));
            return component;
        }
        Err(error) => {
            component.status = ReconcileStatus::Unknown;
            component
                .warnings
                .push(format!("could not fetch recorded contract: {error}"));
            component.repair_actions.push(format!(
                "{key}: retry reconciliation with a healthy RPC before resuming"
            ));
            return component;
        }
    }

    if verify_views {
        match verify_component_wiring(stellar, manifest, key, record) {
            Ok(wiring) => {
                if wiring
                    .iter()
                    .any(|check| check.status == WiringStatus::Mismatch)
                {
                    component.status = ReconcileStatus::Mismatched;
                    component
                        .repair_actions
                        .push(format!("{key}: investigate manifest/chain wiring mismatch"));
                } else if !wiring.is_empty() {
                    component.status = ReconcileStatus::Initialized;
                }
                component.wiring = wiring;
            }
            Err(error) => {
                component
                    .warnings
                    .push(format!("view verification unavailable: {error}"));
                if record.initialized {
                    component.status = ReconcileStatus::Unknown;
                    component.repair_actions.push(format!(
                        "{key}: retry view verification before treating as initialized"
                    ));
                }
            }
        }
    }

    if component.status == ReconcileStatus::Deployed {
        component
            .repair_actions
            .push(format!("{key}: run deploy resume to continue initialization if this component has an initializer"));
    }
    if component.status == ReconcileStatus::Initialized && !component.manifest_initialized {
        component.warnings.push(
            "manifest marks this contract uninitialized, but chain views indicate it is initialized"
                .to_string(),
        );
        component.repair_actions.push(format!(
            "{key}: deploy resume can safely checkpoint initialized=true before continuing"
        ));
    }
    component
}

fn apply_reconcile_safe_manifest_updates(
    cli: &Cli,
    manifest: &mut Manifest,
    reconcile: &ReconcileResponse,
) -> anyhow::Result<()> {
    let mut changed = false;
    for component in &reconcile.components {
        if component.status != ReconcileStatus::Initialized || component.manifest_initialized {
            continue;
        }
        if let Some(record) = manifest.contracts.get_mut(&component.key) {
            record.initialized = true;
            changed = true;
        }
    }
    if changed {
        checkpoint_manifest(cli, manifest)?;
    }
    Ok(())
}

fn should_compare_wasm_hash(wasm_hash: &str) -> bool {
    !matches!(wasm_hash, "predeployed" | "stellar-asset-contract")
}

fn looks_missing_contract(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("not found")
        || message.contains("does not exist")
        || message.contains("missing")
        || message.contains("not exist")
}

#[allow(
    clippy::too_many_lines,
    reason = "contract-specific view checks are clearer as one dispatch table"
)]
fn verify_component_wiring<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    key: &str,
    record: &ContractRecord,
) -> anyhow::Result<Vec<WiringCheck>> {
    let mut checks = Vec::new();
    match key {
        "vault" => {
            let owner = contract_id(manifest, "governance")
                .or_else(|| contract_id(manifest, "vault"))
                .context("vault proxy_view needs a recorded owner address")?;
            let out = stellar.invoke_view(
                &record.contract_id,
                "proxy_view",
                args([("--owner", owner), ("--assets", "0"), ("--shares", "0")]),
            )?;
            push_contains_check(
                &mut checks,
                "governance",
                contract_id(manifest, "governance"),
                &out.stdout,
            );
            push_contains_check(
                &mut checks,
                "asset_token",
                contract_id(manifest, "asset_token"),
                &out.stdout,
            );
            push_contains_check(
                &mut checks,
                "share_token",
                contract_id(manifest, "share_token"),
                &out.stdout,
            );
        }
        "governance" => {
            checks.push(view_equals_check(
                stellar,
                &record.contract_id,
                "vault",
                contract_id(manifest, "vault"),
            )?);
            if let Some(admin) = record.constructor_args.get("admin") {
                checks.push(view_equals_check(
                    stellar,
                    &record.contract_id,
                    "admin",
                    Some(admin),
                )?);
            }
        }
        "share_token" => {
            checks.push(view_equals_check(
                stellar,
                &record.contract_id,
                "vault",
                contract_id(manifest, "vault"),
            )?);
            checks.push(view_equals_check(
                stellar,
                &record.contract_id,
                "admin",
                contract_id(manifest, "vault"),
            )?);
        }
        "proxy_4626" => {
            checks.push(view_equals_check(
                stellar,
                &record.contract_id,
                "asset",
                contract_id(manifest, "asset_token"),
            )?);
        }
        "curator_proxy" => {
            checks.push(view_equals_check(
                stellar,
                &record.contract_id,
                "vault",
                contract_id(manifest, "vault"),
            )?);
            checks.push(view_equals_check(
                stellar,
                &record.contract_id,
                "governance",
                contract_id(manifest, "governance"),
            )?);
        }
        key if key.starts_with("blend_adapter") => {
            checks.push(view_equals_check(
                stellar,
                &record.contract_id,
                "vault",
                contract_id(manifest, "vault"),
            )?);
            if let Some(pool) = record.constructor_args.get("pool") {
                checks.push(view_equals_check(
                    stellar,
                    &record.contract_id,
                    "pool",
                    Some(pool),
                )?);
            }
            if let Some(admin) = record.constructor_args.get("admin") {
                checks.push(view_equals_check(
                    stellar,
                    &record.contract_id,
                    "admin",
                    Some(admin),
                )?);
            }
        }
        key if key.starts_with("custodial_adapter") => {
            checks.push(view_equals_check(
                stellar,
                &record.contract_id,
                "vault",
                contract_id(manifest, "vault"),
            )?);
            checks.push(view_equals_check(
                stellar,
                &record.contract_id,
                "asset",
                contract_id(manifest, "asset_token"),
            )?);
            if let Some(custodian) = record.constructor_args.get("custodian") {
                checks.push(view_equals_check(
                    stellar,
                    &record.contract_id,
                    "custodian",
                    Some(custodian),
                )?);
            }
            if let Some(admin) = record.constructor_args.get("admin") {
                checks.push(view_equals_check(
                    stellar,
                    &record.contract_id,
                    "admin",
                    Some(admin),
                )?);
            }
        }
        _ => {}
    }
    Ok(checks)
}

fn view_equals_check<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    contract_id: &str,
    function: &str,
    expected: Option<&str>,
) -> anyhow::Result<WiringCheck> {
    let Some(expected) = expected else {
        return Ok(WiringCheck {
            field: function.to_string(),
            expected: None,
            observed: None,
            status: WiringStatus::Unknown,
        });
    };
    let out = stellar.invoke_view(contract_id, function, Vec::new())?;
    Ok(WiringCheck {
        field: function.to_string(),
        expected: Some(expected.to_string()),
        observed: Some(out.stdout.clone()),
        status: if out.stdout.contains(expected) {
            WiringStatus::Match
        } else {
            WiringStatus::Mismatch
        },
    })
}

fn push_contains_check(
    checks: &mut Vec<WiringCheck>,
    field: &str,
    expected: Option<&str>,
    observed: &str,
) {
    checks.push(WiringCheck {
        field: field.to_string(),
        expected: expected.map(ToString::to_string),
        observed: Some(observed.to_string()),
        status: match expected {
            Some(expected) if observed.contains(expected) => WiringStatus::Match,
            Some(_) => WiringStatus::Mismatch,
            None => WiringStatus::Unknown,
        },
    });
}

fn deploy_adapters<E: CommandExecutor>(
    cli: &Cli,
    stellar: &Stellar<'_, E>,
    manifest: &mut Manifest,
    args: &crate::cli::DeployAdaptersArgs,
) -> anyhow::Result<Response> {
    anyhow::ensure!(
        !args.blend_pools.is_empty() || !args.custodians.is_empty(),
        "deploy adapters requires at least one --blend-pool or --custodian"
    );

    record_imported_contract_if_provided(cli, manifest, "vault", args.vault.as_ref())?;
    checkpoint_manifest(cli, manifest)?;
    record_imported_contract_if_provided(cli, manifest, "governance", args.governance.as_ref())?;
    checkpoint_manifest(cli, manifest)?;
    if let Some(asset_token) = &args.asset_token {
        record_asset_token(cli, manifest, asset_token.as_str(), true)?;
        checkpoint_manifest(cli, manifest)?;
    }

    let vault = required_contract(manifest, "vault")?.to_string();
    let governance = required_contract(manifest, "governance")?.to_string();
    let asset_token = if args.custodians.is_empty() {
        contract_id(manifest, "asset_token").map(ToString::to_string)
    } else {
        Some(required_contract(manifest, "asset_token")?.to_string())
    };
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
        checkpoint_manifest(cli, manifest)?;
        append_blend_adapters(
            cli,
            stellar,
            manifest,
            &wasm_hash,
            &governance,
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
        checkpoint_manifest(cli, manifest)?;
        append_custodial_adapters(
            cli,
            stellar,
            manifest,
            &wasm_hash,
            &governance,
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

fn run_deploy_plan(
    cli: &Cli,
    manifest: &Manifest,
    args: &crate::cli::DeployPlanArgs,
) -> anyhow::Result<Response> {
    let plan = match &args.command {
        DeployPlanCommand::Stack(stack) => deploy_stack_plan(cli, manifest, stack)?,
        DeployPlanCommand::Adapters(adapters) => deploy_adapters_plan(cli, manifest, adapters)?,
    };
    Ok(Response::Plan(plan))
}

fn deploy_stack_plan(
    cli: &Cli,
    manifest: &Manifest,
    args: &crate::cli::DeployStackArgs,
) -> anyhow::Result<PlanResponse> {
    let mut plan = PlanResponse::new("deploy stack", &cli.network);
    if args.governance_timelock_ns == Some(0) && !cli.allow_zero_timelock {
        plan.warnings.push(
            "zero governance timelock would be blocked without --allow-zero-timelock".to_string(),
        );
    }
    plan.required_signers.push(
        args.admin
            .as_ref()
            .map_or_else(default_source_label, |admin| admin.to_string()),
    );

    let include_blend = !args.blend_pools.is_empty();
    let include_custodial = !args.custodians.is_empty();
    for spec in ArtifactSpec::stack_artifacts(include_blend, include_custodial) {
        plan.wasm.push(wasm_plan(cli, manifest, spec, args.build)?);
    }

    for key in [
        "vault",
        "share_token",
        "governance",
        "proxy_4626",
        "curator_proxy",
    ] {
        push_contract_plan(&mut plan, manifest, key, args.force_new);
    }
    if let Some(asset_token) = &args.asset_token {
        if let Some(existing) = contract_id(manifest, "asset_token") {
            plan.contracts_to_reuse.push(PlanContract {
                key: "asset_token".to_string(),
                contract_id: Some(existing.to_string()),
                reason: "already recorded in manifest".to_string(),
            });
        } else {
            plan.manifest_mutations.push(format!(
                "record provided asset_token contract {asset_token}"
            ));
        }
    } else if let Some(asset_token) = contract_id(manifest, "asset_token") {
        plan.contracts_to_reuse.push(PlanContract {
            key: "asset_token".to_string(),
            contract_id: Some(asset_token.to_string()),
            reason: "already recorded in manifest".to_string(),
        });
    } else {
        plan.manifest_mutations
            .push("record native asset token contract id".to_string());
        plan.stellar_commands.push(stellar_command_shape(
            "contract asset deploy --asset native",
            true,
        ));
    }
    for pool in &args.blend_pools {
        if !args.force_new && blend_adapter_by_pool(manifest, pool).is_some() {
            plan.contracts_to_reuse.push(PlanContract {
                key: format!("blend adapter for pool {pool}"),
                contract_id: blend_adapter_by_pool(manifest, pool).map(ToString::to_string),
                reason: "adapter for pool is already recorded in manifest".to_string(),
            });
        } else {
            plan.contracts_to_deploy.push(PlanContract {
                key: next_blend_adapter_key(manifest),
                contract_id: None,
                reason: format!("new adapter for pool {pool}"),
            });
            plan.manifest_mutations
                .push(format!("record new Blend adapter for pool {pool}"));
            plan.stellar_commands.push(stellar_command_shape(
                "contract deploy --wasm-hash <blend_adapter_hash> -- --admin <governance> --vault <vault> --pool <pool>",
                true,
            ));
        }
    }
    for custodian in &args.custodians {
        if !args.force_new && custodial_adapter_by_custodian(manifest, custodian).is_some() {
            plan.contracts_to_reuse.push(PlanContract {
                key: format!("custodial adapter for custodian {custodian}"),
                contract_id: custodial_adapter_by_custodian(manifest, custodian)
                    .map(ToString::to_string),
                reason: "adapter for custodian is already recorded in manifest".to_string(),
            });
        } else {
            plan.contracts_to_deploy.push(PlanContract {
                key: next_custodial_adapter_key(manifest),
                contract_id: None,
                reason: format!("new adapter for custodian {custodian}"),
            });
            plan.manifest_mutations.push(format!(
                "record new custodial adapter for custodian {custodian}"
            ));
            plan.stellar_commands.push(stellar_command_shape(
                "contract deploy --wasm-hash <custodial_adapter_hash> -- --admin <governance> --vault <vault> --custodian <custodian> --asset <asset_token>",
                true,
            ));
        }
    }
    plan.manifest_mutations
        .push("mark initialized contracts after successful initialize calls".to_string());
    Ok(plan)
}

fn deploy_adapters_plan(
    cli: &Cli,
    manifest: &Manifest,
    args: &crate::cli::DeployAdaptersArgs,
) -> anyhow::Result<PlanResponse> {
    let mut plan = PlanResponse::new("deploy adapters", &cli.network);
    plan.required_signers.push(default_source_label());
    if !args.blend_pools.is_empty() {
        plan.wasm.push(wasm_plan(
            cli,
            manifest,
            ArtifactSpec::from_name(crate::cli::ArtifactName::BlendAdapter),
            args.build,
        )?);
    }
    if !args.custodians.is_empty() {
        plan.wasm.push(wasm_plan(
            cli,
            manifest,
            ArtifactSpec::from_name(crate::cli::ArtifactName::CustodialAdapter),
            args.build,
        )?);
    }

    for (key, provided) in [
        ("vault", args.vault.as_ref()),
        ("governance", args.governance.as_ref()),
        ("asset_token", args.asset_token.as_ref()),
    ] {
        if let Some(existing) = contract_id(manifest, key) {
            plan.contracts_to_reuse.push(PlanContract {
                key: key.to_string(),
                contract_id: Some(existing.to_string()),
                reason: "already recorded in manifest".to_string(),
            });
        } else if let Some(provided) = provided {
            plan.manifest_mutations
                .push(format!("record imported {key} contract {provided}"));
        } else if key != "asset_token" {
            plan.warnings.push(format!(
                "{key} is missing from manifest and must be passed for deploy adapters"
            ));
        } else if !args.custodians.is_empty() {
            plan.warnings.push(
                "asset_token is missing from manifest and must be passed for custodial adapters"
                    .to_string(),
            );
        }
    }

    for pool in &args.blend_pools {
        if !args.force_new && blend_adapter_by_pool(manifest, pool).is_some() {
            plan.contracts_to_reuse.push(PlanContract {
                key: format!("blend adapter for pool {pool}"),
                contract_id: blend_adapter_by_pool(manifest, pool).map(ToString::to_string),
                reason: "adapter for pool is already recorded in manifest".to_string(),
            });
        } else {
            plan.contracts_to_deploy.push(PlanContract {
                key: next_blend_adapter_key(manifest),
                contract_id: None,
                reason: format!("new adapter for pool {pool}"),
            });
            plan.manifest_mutations
                .push(format!("record new Blend adapter for pool {pool}"));
            plan.stellar_commands.push(stellar_command_shape(
                "contract deploy --wasm-hash <blend_adapter_hash> -- --admin <governance> --vault <vault> --pool <pool>",
                true,
            ));
        }
    }
    for custodian in &args.custodians {
        if !args.force_new && custodial_adapter_by_custodian(manifest, custodian).is_some() {
            plan.contracts_to_reuse.push(PlanContract {
                key: format!("custodial adapter for custodian {custodian}"),
                contract_id: custodial_adapter_by_custodian(manifest, custodian)
                    .map(ToString::to_string),
                reason: "adapter for custodian is already recorded in manifest".to_string(),
            });
        } else {
            plan.contracts_to_deploy.push(PlanContract {
                key: next_custodial_adapter_key(manifest),
                contract_id: None,
                reason: format!("new adapter for custodian {custodian}"),
            });
            plan.manifest_mutations.push(format!(
                "record new custodial adapter for custodian {custodian}"
            ));
            plan.stellar_commands.push(stellar_command_shape(
                "contract deploy --wasm-hash <custodial_adapter_hash> -- --admin <governance> --vault <vault> --custodian <custodian> --asset <asset_token>",
                true,
            ));
        }
    }
    Ok(plan)
}

fn push_contract_plan(plan: &mut PlanResponse, manifest: &Manifest, key: &str, force_new: bool) {
    if !force_new {
        if let Some(contract_id) = contract_id(manifest, key) {
            plan.contracts_to_reuse.push(PlanContract {
                key: key.to_string(),
                contract_id: Some(contract_id.to_string()),
                reason: "already recorded in manifest".to_string(),
            });
            return;
        }
    }
    plan.contracts_to_deploy.push(PlanContract {
        key: key.to_string(),
        contract_id: None,
        reason: if force_new {
            "--force-new requested".to_string()
        } else {
            "not recorded in manifest".to_string()
        },
    });
    plan.manifest_mutations
        .push(format!("record deployed {key} contract id"));
    plan.stellar_commands.push(stellar_command_shape(
        &format!("contract deploy --wasm-hash <{key}_hash>"),
        true,
    ));
}

fn wasm_plan(
    cli: &Cli,
    manifest: &Manifest,
    spec: ArtifactSpec,
    build: bool,
) -> anyhow::Result<PlanWasm> {
    let wasm_path = spec.wasm_path(&cli.workspace_path);
    let local_hash = if wasm_path.exists() {
        Some(sha256_file(&wasm_path)?)
    } else {
        None
    };
    let recorded_remote_hash = manifest
        .artifacts
        .get(spec.key)
        .and_then(|record| record.remote_wasm_hash.clone());
    let action = match (&local_hash, &recorded_remote_hash) {
        (Some(local), Some(remote)) if local == remote => {
            "reuse recorded remote hash after fetch verification".to_string()
        }
        (Some(_), _) => "fetch local hash, upload if missing remotely".to_string(),
        (None, _) if build => "build artifact, then fetch/upload resulting hash".to_string(),
        (None, _) => "missing local artifact and build disabled".to_string(),
    };
    Ok(PlanWasm {
        key: spec.key.to_string(),
        package: spec.package.to_string(),
        path: wasm_path.display().to_string(),
        local_hash,
        recorded_remote_hash,
        action,
    })
}

fn stellar_command_shape(command: &str, uses_source: bool) -> String {
    if uses_source {
        format!("STELLAR_ACCOUNT=<redacted-if-overridden> stellar {command}")
    } else {
        format!("stellar {command}")
    }
}

fn default_source_label() -> String {
    "Stellar default identity/keystore or STELLAR_ACCOUNT".to_string()
}

#[allow(
    clippy::too_many_arguments,
    reason = "deployment recording needs checkpoint context plus constructor metadata"
)]
fn deploy_contract_if_needed<E: CommandExecutor>(
    cli: &Cli,
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
    let has_constructor_args = !constructor_args.is_empty();
    let contract_id = stellar.deploy(wasm_hash, constructor_args)?;
    manifest.contracts.insert(
        key.to_string(),
        ContractRecord {
            contract_id: contract_id.clone(),
            wasm_hash: wasm_hash.to_string(),
            salt: None,
            constructor_args: constructor_summary,
            deploy_tx: None,
            initialized: has_constructor_args,
        },
    );
    checkpoint_manifest(cli, manifest)?;
    info!(
        contract_key = key,
        contract_id = %contract_id,
        "recorded deployed contract"
    );
    Ok(contract_id)
}

#[allow(
    clippy::too_many_arguments,
    reason = "adapter deployment needs both manifest checkpoint context and constructor inputs"
)]
fn append_blend_adapters<E: CommandExecutor>(
    cli: &Cli,
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
            cli,
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
        checkpoint_manifest(cli, manifest)?;
    }
    Ok(blend_adapter_statuses(manifest))
}

#[allow(
    clippy::too_many_arguments,
    reason = "adapter deployment needs both manifest checkpoint context and constructor inputs"
)]
fn append_custodial_adapters<E: CommandExecutor>(
    cli: &Cli,
    stellar: &Stellar<'_, E>,
    manifest: &mut Manifest,
    wasm_hash: &str,
    governance: &str,
    vault: &str,
    asset_token: &str,
    custodians: &[AddressStr],
    force_new: bool,
) -> anyhow::Result<Vec<CustodialAdapterStatus>> {
    for custodian in custodians {
        if !force_new && custodial_adapter_by_custodian(manifest, custodian).is_some() {
            continue;
        }
        let key = next_custodial_adapter_key(manifest);
        let adapter = deploy_contract_if_needed(
            cli,
            stellar,
            manifest,
            &key,
            wasm_hash,
            vec![
                "--admin".to_string(),
                governance.to_string(),
                "--vault".to_string(),
                vault.to_string(),
                "--custodian".to_string(),
                custodian.to_string(),
                "--asset".to_string(),
                asset_token.to_string(),
            ],
            map_args([
                ("admin", governance),
                ("vault", vault),
                ("custodian", custodian.as_str()),
                ("asset", asset_token),
            ]),
            force_new,
        )?;
        if let Some(record) = manifest.contracts.get_mut(&key) {
            record.contract_id = adapter;
        }
        checkpoint_manifest(cli, manifest)?;
    }
    Ok(custodial_adapter_statuses(manifest))
}

fn record_imported_contract_if_provided(
    cli: &Cli,
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
        checkpoint_manifest(cli, manifest)?;
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
    checkpoint_manifest(cli, manifest)?;
    info!(
        contract_key = key,
        contract_id = %contract_id,
        "recorded imported contract"
    );
    Ok(())
}

fn record_asset_token(
    cli: &Cli,
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
        checkpoint_manifest(cli, manifest)?;
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
    checkpoint_manifest(cli, manifest)?;
    info!(
        contract_key = "asset_token",
        contract_id = %asset_token,
        predeployed,
        "recorded asset token"
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn initialize_vault_if_needed<E: CommandExecutor>(
    cli: &Cli,
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
    checkpoint_manifest(cli, manifest)?;
    Ok(())
}

fn initialize_proxy_if_needed<E: CommandExecutor>(
    cli: &Cli,
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
    checkpoint_manifest(cli, manifest)?;
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
            assets_raw,
            asset_decimals,
            min_shares_out,
            min_shares_out_raw,
            share_decimals,
        } => {
            let assets = required_amount("assets", assets.as_ref(), *assets_raw, *asset_decimals)?;
            let min_shares_out = optional_share_amount(
                manifest,
                "min_shares_out",
                min_shares_out.as_ref(),
                Some(*min_shares_out_raw),
                *share_decimals,
            )?
            .unwrap_or(0);
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
                        assets,
                        min_shares_out,
                    },
                )
            }
        }
        UserCommand::Mint {
            operator,
            receiver,
            shares,
            shares_raw,
            share_decimals,
        } => {
            let shares = required_share_amount(
                manifest,
                "shares",
                shares.as_ref(),
                *shares_raw,
                *share_decimals,
            )?;
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
            assets_raw,
            asset_decimals,
            max_shares_burned,
            max_shares_burned_raw,
            share_decimals,
        } => {
            let assets = required_amount("assets", assets.as_ref(), *assets_raw, *asset_decimals)?;
            let max_shares_burned = optional_share_amount(
                manifest,
                "max_shares_burned",
                max_shares_burned.as_ref(),
                *max_shares_burned_raw,
                *share_decimals,
            )?
            .unwrap_or(assets);
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
                    ("--max_shares_burned", &max_shares_burned.to_string()),
                ]),
            )?)
        }
        UserCommand::Redeem {
            operator,
            receiver,
            owner,
            shares,
            shares_raw,
            share_decimals,
            min_assets_out,
            min_assets_out_raw,
            asset_decimals,
        } => {
            let shares = required_share_amount(
                manifest,
                "shares",
                shares.as_ref(),
                *shares_raw,
                *share_decimals,
            )?;
            let min_assets_out = optional_amount(
                "min_assets_out",
                min_assets_out.as_ref(),
                Some(*min_assets_out_raw),
                *asset_decimals,
            )?;
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
            shares_raw,
            share_decimals,
            min_assets_out,
            min_assets_out_raw,
            asset_decimals,
        } => {
            let shares = required_share_amount(
                manifest,
                "shares",
                shares.as_ref(),
                *shares_raw,
                *share_decimals,
            )?;
            let min_assets_out = optional_amount(
                "min_assets_out",
                min_assets_out.as_ref(),
                Some(*min_assets_out_raw),
                *asset_decimals,
            )?;
            let receiver = receiver.as_ref().unwrap_or(owner);
            execute_vault(
                stellar,
                manifest,
                WireVaultCommand::RequestWithdraw {
                    owner: owner.to_string(),
                    receiver: receiver.to_string(),
                    shares,
                    min_assets_out,
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
            invoke_response(stellar.invoke_view(
                share,
                "balance",
                args([("--account", owner.as_str())]),
            )?)
        }
        UserCommand::Preview {
            owner,
            assets,
            assets_raw,
            asset_decimals,
            shares,
            shares_raw,
            share_decimals,
        }
        | UserCommand::View {
            owner,
            assets,
            assets_raw,
            asset_decimals,
            shares,
            shares_raw,
            share_decimals,
        } => {
            let assets = optional_amount(
                "assets",
                assets.as_ref(),
                Some(*assets_raw),
                *asset_decimals,
            )?;
            let shares = optional_share_amount(
                manifest,
                "shares",
                shares.as_ref(),
                Some(*shares_raw),
                *share_decimals,
            )?
            .unwrap_or(0);
            let target = contract_id(manifest, "proxy_4626")
                .or_else(|| contract_id(manifest, "vault"))
                .context("missing proxy_4626 or vault contract id in manifest")?;
            let function = if contract_id(manifest, "proxy_4626").is_some() {
                "preview"
            } else {
                "proxy_view"
            };
            invoke_response(stellar.invoke_view(
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
            amount_raw,
            asset_decimals,
        } => {
            let amount = required_amount("amount", amount.as_ref(), *amount_raw, *asset_decimals)?;
            execute_allocation(
                stellar,
                manifest,
                caller,
                &AllocationDelta::Supply(*market, amount),
            )
        }
        CuratorCommand::AllocateWithdraw {
            caller,
            market,
            amount,
            amount_raw,
            asset_decimals,
        } => {
            let amount = required_amount("amount", amount.as_ref(), *amount_raw, *asset_decimals)?;
            execute_allocation(
                stellar,
                manifest,
                caller,
                &AllocationDelta::Withdraw(*market, amount),
            )
        }
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

fn required_amount(
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

fn optional_amount(
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

fn required_share_amount(
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

fn optional_share_amount(
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

fn resolve_share_decimals(manifest: &Manifest, decimals: ShareDecimalsArg) -> anyhow::Result<u32> {
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
        GovernanceCommand::PlanAccept { admin, proposal_id } => {
            Ok(Response::Plan(governance_plan(
                "governance accept",
                &manifest.network,
                vec![admin.to_string()],
                vec![stellar_command_shape(
                    &format!(
                        "contract invoke --id {governance} -- accept --caller {admin} --proposal_id {proposal_id}"
                    ),
                    true,
                )],
            )))
        }
        GovernanceCommand::PlanSubmitSetSupplyQueue { admin, entries } => {
            let entries_json = supply_queue_entries_json(entries)?;
            Ok(Response::Plan(governance_plan(
                "governance submit-set-supply-queue",
                &manifest.network,
                vec![admin.to_string()],
                vec![stellar_command_shape(
                    &format!(
                        "contract invoke --id {governance} -- submit_set_supply_queue --caller {admin} --entries '{entries_json}'"
                    ),
                    true,
                )],
            )))
        }
        GovernanceCommand::PlanSubmitSetTimelock {
            admin,
            kind,
            timelock_ns,
        } => Ok(Response::Plan(governance_plan(
            "governance submit-set-timelock",
            &manifest.network,
            vec![admin.to_string()],
            vec![stellar_command_shape(
                &format!(
                    "contract invoke --id {governance} -- submit_set_timelock --caller {admin} --kind {kind} --new_timelock_ns {timelock_ns}"
                ),
                true,
            )],
        ))),
        GovernanceCommand::Queue { kind } => {
            let queue = governance_queue(stellar, governance, kind.as_ref())?;
            Ok(Response::GovernanceQueue(queue))
        }
        GovernanceCommand::Explain { proposal_id } => {
            let proposal = inspect_governance_proposal(stellar, governance, *proposal_id)?;
            Ok(Response::GovernanceExplain(proposal))
        }
        GovernanceCommand::AcceptReady { admin, kind, limit } => {
            run_governance_accept_ready(stellar, governance, admin, kind.as_ref(), *limit)
        }
        GovernanceCommand::SubmitAndWait(args) => {
            run_governance_submit_and_wait(stellar, governance, args)
        }
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
                invoke_response(stellar.invoke_view(
                    governance,
                    "pending",
                    args([("--proposal_id", &proposal_id.to_string())]),
                )?)
            } else {
                invoke_response(stellar.invoke_view(governance, "pending_ids", Vec::new())?)
            }
        }
        GovernanceCommand::Timelocks => {
            invoke_response(stellar.invoke_view(governance, "timelocks", Vec::new())?)
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

fn governance_plan(
    scope: impl Into<String>,
    network: &str,
    required_signers: Vec<String>,
    stellar_commands: Vec<String>,
) -> PlanResponse {
    let mut plan = PlanResponse::new(scope, network);
    plan.required_signers = required_signers;
    plan.stellar_commands = stellar_commands;
    plan.manifest_mutations
        .push("none; governance proposals are stored on-chain".to_string());
    plan
}

fn governance_queue<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    governance: &str,
    kind: Option<&crate::types::GovernanceActionKindArg>,
) -> anyhow::Result<GovernanceQueueResponse> {
    let out = stellar.invoke_view(governance, "pending_ids", Vec::new())?;
    let ids = parse_u64s(&out.stdout);
    let mut proposals = Vec::new();
    let mut warnings = Vec::new();
    for proposal_id in ids {
        match inspect_governance_proposal(stellar, governance, proposal_id) {
            Ok(proposal) if proposal_matches_kind(&proposal, kind) => proposals.push(proposal),
            Ok(_) => {}
            Err(error) => warnings.push(format!("proposal {proposal_id}: {error}")),
        }
    }
    Ok(GovernanceQueueResponse {
        proposals,
        warnings,
    })
}

fn inspect_governance_proposal<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    governance: &str,
    proposal_id: u64,
) -> anyhow::Result<GovernanceProposalView> {
    let out = stellar.invoke_view(
        governance,
        "pending",
        args([("--proposal_id", &proposal_id.to_string())]),
    )?;
    Ok(governance_proposal_view(proposal_id, out.stdout))
}

fn governance_proposal_view(proposal_id: u64, raw: String) -> GovernanceProposalView {
    let valid_after_ns =
        parse_named_u64(&raw, "valid_after_ns").or_else(|| parse_named_u64(&raw, "valid_at_ns"));
    let now_ns = system_now_ns();
    let ready = valid_after_ns.map(|valid_after_ns| now_ns >= valid_after_ns);
    let eta_seconds = valid_after_ns.map(|valid_after_ns| {
        if now_ns >= valid_after_ns {
            0
        } else {
            i64::try_from((valid_after_ns - now_ns) / 1_000_000_000).unwrap_or(i64::MAX)
        }
    });
    GovernanceProposalView {
        proposal_id,
        action: summarize_governance_action(&raw),
        valid_after_ns,
        ready,
        eta_seconds,
        raw,
    }
}

fn run_governance_accept_ready<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    governance: &str,
    admin: &AddressStr,
    kind: Option<&crate::types::GovernanceActionKindArg>,
    limit: Option<usize>,
) -> anyhow::Result<Response> {
    let queue = governance_queue(stellar, governance, kind)?;
    let mut accepted = Vec::new();
    let mut skipped = queue.warnings;
    for proposal in queue.proposals {
        if limit.is_some_and(|limit| accepted.len() >= limit) {
            skipped.push(format!("proposal {}: limit reached", proposal.proposal_id));
            continue;
        }
        match proposal.ready {
            Some(true) => {
                stellar.invoke(
                    governance,
                    "accept",
                    args([
                        ("--caller", admin.as_str()),
                        ("--proposal_id", &proposal.proposal_id.to_string()),
                    ]),
                )?;
                accepted.push(proposal.proposal_id);
            }
            Some(false) => skipped.push(format!(
                "proposal {}: not ready for {} seconds",
                proposal.proposal_id,
                proposal.eta_seconds.unwrap_or_default()
            )),
            None => skipped.push(format!(
                "proposal {}: readiness could not be decoded",
                proposal.proposal_id
            )),
        }
    }
    Ok(Response::GovernanceAcceptReady(
        GovernanceAcceptReadyResponse { accepted, skipped },
    ))
}

fn run_governance_submit_and_wait<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    governance: &str,
    wait_args: &crate::cli::GovernanceSubmitAndWaitArgs,
) -> anyhow::Result<Response> {
    let (admin, proposal_id) = match &wait_args.command {
        GovernanceSubmitAndWaitCommand::Proposal { admin, proposal_id } => (admin, *proposal_id),
        GovernanceSubmitAndWaitCommand::SetSupplyQueue { admin, entries } => {
            let out = stellar.invoke(
                governance,
                "submit_set_supply_queue",
                vec![
                    "--caller".to_string(),
                    admin.to_string(),
                    "--entries".to_string(),
                    supply_queue_entries_json(entries)?,
                ],
            )?;
            (admin, parse_proposal_id(&out.stdout)?)
        }
        GovernanceSubmitAndWaitCommand::SetTimelock {
            admin,
            kind,
            timelock_ns,
        } => {
            let out = stellar.invoke(
                governance,
                "submit_set_timelock",
                args([
                    ("--caller", admin.as_str()),
                    ("--kind", &kind.to_string()),
                    ("--new_timelock_ns", &timelock_ns.to_string()),
                ]),
            )?;
            (admin, parse_proposal_id(&out.stdout)?)
        }
    };
    wait_for_governance_proposal(
        stellar,
        governance,
        admin,
        proposal_id,
        wait_args.poll_seconds,
        wait_args.max_wait_seconds,
    )
}

fn wait_for_governance_proposal<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    governance: &str,
    admin: &AddressStr,
    proposal_id: u64,
    poll_seconds: u64,
    max_wait_seconds: u64,
) -> anyhow::Result<Response> {
    let started = SystemTime::now();
    loop {
        let proposal = inspect_governance_proposal(stellar, governance, proposal_id)?;
        if proposal.ready == Some(true) {
            stellar.invoke(
                governance,
                "accept",
                args([
                    ("--caller", admin.as_str()),
                    ("--proposal_id", &proposal_id.to_string()),
                ]),
            )?;
            return Ok(Response::message(format!(
                "accepted ready proposal {proposal_id}"
            )));
        }
        if max_wait_seconds == 0 {
            return Ok(Response::GovernanceExplain(proposal));
        }
        let elapsed = started.elapsed().unwrap_or_default().as_secs();
        if elapsed >= max_wait_seconds {
            return Ok(Response::GovernanceExplain(proposal));
        }
        let remaining = max_wait_seconds.saturating_sub(elapsed);
        thread::sleep(Duration::from_secs(poll_seconds.min(remaining).max(1)));
    }
}

fn proposal_matches_kind(
    proposal: &GovernanceProposalView,
    kind: Option<&crate::types::GovernanceActionKindArg>,
) -> bool {
    let Some(kind) = kind else {
        return true;
    };
    let needle = kind.to_string().to_ascii_lowercase();
    proposal.action.to_ascii_lowercase().contains(&needle)
        || proposal.raw.to_ascii_lowercase().contains(&needle)
}

fn summarize_governance_action(raw: &str) -> String {
    for action in [
        "SetAdmin",
        "SetCurator",
        "SetGovernance",
        "SetPaused",
        "SetSupplyQueue",
        "SetFees",
        "SetRestrictions",
        "SetSentinel",
        "SetAllocators",
        "SetAllowedAdapters",
        "SetTimelock",
        "SetCap",
        "RemoveMarket",
        "SetGroupCap",
        "SetGroupRelCap",
        "SetGroupMember",
        "SetSkimRecipient",
        "Skim",
        "Upgrade",
        "Migrate",
        "CancelMigration",
        "SetWithdrawalCooldown",
        "SetIdleResyncCooldown",
    ] {
        if raw.contains(action) {
            return action.to_string();
        }
    }
    "unknown".to_string()
}

fn parse_named_u64(raw: &str, name: &str) -> Option<u64> {
    let start = raw.find(name)? + name.len();
    raw[start..]
        .split(|c: char| !c.is_ascii_digit())
        .find(|part| !part.is_empty())?
        .parse()
        .ok()
}

fn parse_u64s(raw: &str) -> Vec<u64> {
    raw.split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn system_now_ns() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    u64::try_from(nanos).unwrap_or(u64::MAX)
}

fn transaction_record(cli: &Cli, manifest: &Manifest, response: &Response) -> TransactionRecord {
    let (contract_id, function) = command_target_and_function(&cli.command, manifest);
    let tx_hashes = response.tx_hashes();
    TransactionRecord {
        timestamp_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        command: Some(command_name(&cli.command)),
        action: response.kind().to_string(),
        target: contract_id.clone(),
        contract_id,
        function,
        tx_hash: tx_hashes.first().cloned(),
        source_public_address: cli
            .source_account
            .as_ref()
            .and_then(SourceAccount::public_address),
        result_status: Some("success".to_string()),
        artifact_hash: command_artifact_hash(&cli.command, manifest),
    }
}

fn command_name(command: &Commands) -> String {
    match command {
        Commands::Doctor => "doctor",
        Commands::Deploy(_) => "deploy",
        Commands::User(_) => "user",
        Commands::Curator(_) => "curator",
        Commands::Governance(_) => "governance",
        Commands::ShareToken(_) => "share-token",
        Commands::Adapter(_) => "adapter",
        Commands::ExtendTtl(_) => "extend-ttl",
        Commands::Reconcile(_) => "reconcile",
        Commands::Status => "status",
        Commands::ExportEnv => "export-env",
        Commands::Profile(_) => "profile",
        Commands::Completions { .. } => "completions",
        Commands::Man => "man",
    }
    .to_string()
}

fn command_target_and_function(
    command: &Commands,
    manifest: &Manifest,
) -> (Option<String>, Option<String>) {
    match command {
        Commands::User(args) => {
            let target = contract_id(manifest, "proxy_4626")
                .or_else(|| contract_id(manifest, "vault"))
                .map(ToString::to_string);
            let function = match &args.command {
                UserCommand::Deposit { .. } => "deposit_with_min",
                UserCommand::Mint { .. } => "mint",
                UserCommand::Withdraw { .. } => "withdraw",
                UserCommand::Redeem { .. } => "redeem",
                UserCommand::RequestWithdraw { .. } => "execute",
                UserCommand::ExecuteWithdraw { .. } => "execute_withdraw",
                UserCommand::Balance { .. }
                | UserCommand::Preview { .. }
                | UserCommand::View { .. } => "view",
            };
            (target, Some(function.to_string()))
        }
        Commands::Curator(_) => (
            contract_id(manifest, "vault").map(ToString::to_string),
            Some("execute".to_string()),
        ),
        Commands::Governance(_) => (
            contract_id(manifest, "governance").map(ToString::to_string),
            Some("governance".to_string()),
        ),
        Commands::ShareToken(_) => (
            contract_id(manifest, "share_token").map(ToString::to_string),
            Some("share_token".to_string()),
        ),
        Commands::Adapter(args) => (
            selected_blend_adapter(manifest, args)
                .ok()
                .map(ToString::to_string),
            Some("adapter".to_string()),
        ),
        Commands::ExtendTtl(_) => (None, Some("extend_ttl".to_string())),
        Commands::Deploy(_) => (None, Some("deploy".to_string())),
        Commands::Reconcile(_) => (None, Some("reconcile".to_string())),
        Commands::Doctor
        | Commands::Status
        | Commands::ExportEnv
        | Commands::Profile(_)
        | Commands::Completions { .. }
        | Commands::Man => (None, None),
    }
}

fn command_artifact_hash(command: &Commands, manifest: &Manifest) -> Option<String> {
    let Commands::Deploy(args) = command else {
        return None;
    };
    match &args.command {
        DeployCommand::Wasm(wasm) => manifest
            .artifacts
            .get(ArtifactSpec::from_name(wasm.artifact).key)
            .and_then(|record| record.remote_wasm_hash.clone()),
        DeployCommand::Stack(_)
        | DeployCommand::Resume(_)
        | DeployCommand::Adapters(_)
        | DeployCommand::Plan(_)
        | DeployCommand::Repair(_) => None,
    }
}

fn run_share_token<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    command: &ShareTokenCommand,
) -> anyhow::Result<Response> {
    let share = required_contract(manifest, "share_token")?;
    match command {
        ShareTokenCommand::Balance { account } => invoke_response(stellar.invoke_view(
            share,
            "balance",
            args([("--account", account.as_str())]),
        )?),
        ShareTokenCommand::TotalSupply => {
            invoke_response(stellar.invoke_view(share, "total_supply", Vec::new())?)
        }
        ShareTokenCommand::Admin => {
            invoke_response(stellar.invoke_view(share, "admin", Vec::new())?)
        }
        ShareTokenCommand::Vault => {
            invoke_response(stellar.invoke_view(share, "vault", Vec::new())?)
        }
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
        AdapterCommand::TotalAssets { asset } => invoke_response(stellar.invoke_view(
            adapter,
            "total_assets",
            args([("--asset", asset.as_str())]),
        )?),
        AdapterCommand::Admin => {
            invoke_response(stellar.invoke_view(adapter, "admin", Vec::new())?)
        }
        AdapterCommand::Vault => {
            invoke_response(stellar.invoke_view(adapter, "vault", Vec::new())?)
        }
        AdapterCommand::Pool => {
            invoke_response(stellar.invoke_view(adapter, "pool", Vec::new())?)
        }
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

    if let Some(proxy) = contract_id(manifest, "curator_proxy") {
        stellar.invoke(proxy, "extend_ttl", Vec::new())?;
        extended.push("curator_proxy".to_string());
    } else {
        skipped.push("curator_proxy".to_string());
    }

    let caller = if contract_id(manifest, "share_token").is_some()
        || !blend_adapter_statuses(manifest).is_empty()
        || !custodial_adapter_statuses(manifest).is_empty()
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

    let adapters = custodial_adapter_statuses(manifest);
    if adapters.is_empty() {
        skipped.push("custodial_adapters".to_string());
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

    for key in ["asset_token"] {
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
        let proposal_id = parse_proposal_id(&out.stdout)?;
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

fn custodial_adapter_key(index: usize) -> String {
    format!("custodial_adapter_{index}")
}

fn next_custodial_adapter_key(manifest: &Manifest) -> String {
    custodial_adapter_key(next_custodial_adapter_index(manifest))
}

fn next_custodial_adapter_index(manifest: &Manifest) -> usize {
    let highest_index = manifest
        .contracts
        .keys()
        .filter_map(|key| {
            if key == "custodial_adapter" {
                Some(0)
            } else {
                custodial_adapter_index(key)
            }
        })
        .max();
    highest_index.map_or(0, |index| index + 1)
}

fn custodial_adapter_by_custodian<'a>(
    manifest: &'a Manifest,
    custodian: &AddressStr,
) -> Option<&'a str> {
    manifest
        .contracts
        .iter()
        .find(|(key, record)| {
            is_custodial_adapter_key(key)
                && record
                    .constructor_args
                    .get("custodian")
                    .is_some_and(|value| value == custodian.as_str())
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

fn is_custodial_adapter_key(key: &str) -> bool {
    key == "custodial_adapter" || custodial_adapter_index(key).is_some()
}

fn custodial_adapter_index(key: &str) -> Option<usize> {
    key.strip_prefix("custodial_adapter_")?.parse().ok()
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

fn parse_proposal_id(stdout: &str) -> anyhow::Result<u64> {
    let proposal_output = stdout
        .lines()
        .take_while(|line| {
            !line
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("tx hash:")
        })
        .collect::<Vec<_>>()
        .join("\n");
    proposal_output
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
        custodial_adapters: custodial_adapter_statuses(manifest),
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

fn custodial_adapter_statuses(manifest: &Manifest) -> Vec<CustodialAdapterStatus> {
    let mut adapters = manifest
        .contracts
        .iter()
        .filter_map(|(key, record)| {
            let index = custodial_adapter_index(key)?;
            Some((
                index,
                CustodialAdapterStatus {
                    key: key.clone(),
                    contract_id: record.contract_id.clone(),
                    custodian: record.constructor_args.get("custodian").cloned(),
                    asset: record.constructor_args.get("asset").cloned(),
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
        if let Some(record) = manifest.contracts.get("custodial_adapter") {
            adapters.push(CustodialAdapterStatus {
                key: "custodial_adapter".to_string(),
                contract_id: record.contract_id.clone(),
                custodian: record.constructor_args.get("custodian").cloned(),
                asset: record.constructor_args.get("asset").cloned(),
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
    for (index, adapter) in custodial_adapter_statuses(manifest).into_iter().enumerate() {
        if index == 0 {
            values.push((
                "CUSTODIAL_ADAPTER_ID".to_string(),
                adapter.contract_id.clone(),
            ));
        }
        values.push((
            format!("CUSTODIAL_ADAPTER_{index}_ID"),
            adapter.contract_id.clone(),
        ));
        if let Some(custodian) = adapter.custodian {
            if index == 0 {
                values.push(("CUSTODIAL_ADDRESS".to_string(), custodian.clone()));
            }
            values.push((format!("CUSTODIAL_{index}_ADDRESS"), custodian));
        }
        if let Some(asset) = adapter.asset {
            values.push((format!("CUSTODIAL_{index}_ASSET"), asset));
        }
    }
    values
}

#[allow(
    clippy::too_many_lines,
    reason = "single response printer keeps CLI human output routing explicit"
)]
fn print_response(response: &Response, cli: &Cli) -> anyhow::Result<()> {
    if cli.json || cli.json_lines {
        println!(
            "{}",
            serde_json::to_string(&OutputEnvelope::success(cli, response))?
        );
        return Ok(());
    }
    match response {
        Response::Message { message } => println!("{message}"),
        Response::Command { stdout, stderr: _ } => {
            if !stdout.is_empty() {
                println!("{stdout}");
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
            if status.custodial_adapters.is_empty() {
                println!("Custodial Adapters: not deployed");
            } else {
                for adapter in &status.custodial_adapters {
                    println!(
                        "Custodial Adapter {}: {}{}{}",
                        adapter.key,
                        adapter.contract_id,
                        adapter
                            .custodian
                            .as_ref()
                            .map_or_else(String::new, |custodian| {
                                format!(" (custodian {custodian})")
                            }),
                        adapter
                            .asset
                            .as_ref()
                            .map_or_else(String::new, |asset| format!(" (asset {asset})"))
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
        Response::Reconcile(result) => {
            println!("Safe to resume: {}", result.safe_to_resume);
            println!("Drift detected: {}", result.drift_detected);
            println!("Components:");
            for component in &result.components {
                println!(
                    "  - {}: {}{}",
                    component.key,
                    component.status.as_label(),
                    component
                        .contract_id
                        .as_ref()
                        .map(|id| format!(" ({id})"))
                        .unwrap_or_default()
                );
                for warning in &component.warnings {
                    println!("    warning: {warning}");
                }
            }
            if !result.repair_actions.is_empty() {
                println!("Repair plan:");
                for action in &result.repair_actions {
                    println!("  - {action}");
                }
            }
            if !result.safe_next_steps.is_empty() {
                println!("Next steps:");
                for step in &result.safe_next_steps {
                    println!("  - {step}");
                }
            }
        }
        Response::Doctor(result) => {
            println!(
                "Doctor: {}",
                if result.ok {
                    "ready"
                } else {
                    "action required"
                }
            );
            for check in &result.checks {
                println!(
                    "[{}] {}: {}",
                    check.status.as_label(),
                    check.name,
                    check.message
                );
            }
        }
        Response::Plan(plan) => {
            println!("Plan: {} ({})", plan.scope, plan.network);
            if !plan.required_signers.is_empty() {
                println!("Required signers: {}", plan.required_signers.join(", "));
            }
            print_plan_contracts("Reuse", &plan.contracts_to_reuse);
            print_plan_contracts("Deploy", &plan.contracts_to_deploy);
            if !plan.wasm.is_empty() {
                println!("WASM:");
                for wasm in &plan.wasm {
                    println!("  - {}: {}", wasm.key, wasm.action);
                    if let Some(hash) = &wasm.local_hash {
                        println!("    local hash: {hash}");
                    }
                    if let Some(hash) = &wasm.recorded_remote_hash {
                        println!("    recorded remote hash: {hash}");
                    }
                }
            }
            if !plan.manifest_mutations.is_empty() {
                println!("Manifest mutations:");
                for mutation in &plan.manifest_mutations {
                    println!("  - {mutation}");
                }
            }
            if !plan.stellar_commands.is_empty() {
                println!("Stellar commands:");
                for command in &plan.stellar_commands {
                    println!("  - {command}");
                }
            }
            if !plan.warnings.is_empty() {
                println!("Warnings:");
                for warning in &plan.warnings {
                    println!("  - {warning}");
                }
            }
        }
        Response::GovernanceQueue(queue) => {
            if queue.proposals.is_empty() {
                println!("Governance queue: no matching pending proposals");
            } else {
                println!("Governance queue:");
                for proposal in &queue.proposals {
                    print_governance_proposal(proposal);
                }
            }
            for warning in &queue.warnings {
                println!("Warning: {warning}");
            }
        }
        Response::GovernanceExplain(proposal) => {
            print_governance_proposal(proposal);
            println!("Raw: {}", proposal.raw);
        }
        Response::GovernanceAcceptReady(result) => {
            if result.accepted.is_empty() {
                println!("Accepted proposals: none");
            } else {
                println!("Accepted proposals: {:?}", result.accepted);
            }
            if !result.skipped.is_empty() {
                println!("Skipped:");
                for skipped in &result.skipped {
                    println!("  - {skipped}");
                }
            }
        }
    }
    Ok(())
}

pub fn print_error(cli: &Cli, error: &anyhow::Error) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string(&OutputEnvelope::error(cli, error))?
    );
    Ok(())
}

pub fn print_parse_error(raw_args: &[String], error: &clap::Error) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string(&ParseErrorEnvelope::new(raw_args, error))?
    );
    Ok(())
}

fn print_governance_proposal(proposal: &GovernanceProposalView) {
    println!(
        "  - #{} {} ready={} eta_seconds={}",
        proposal.proposal_id,
        proposal.action,
        proposal
            .ready
            .map_or_else(|| "unknown".to_string(), |ready| ready.to_string()),
        proposal
            .eta_seconds
            .map_or_else(|| "unknown".to_string(), |eta| eta.to_string())
    );
}

fn print_plan_contracts(label: &str, contracts: &[PlanContract]) {
    if contracts.is_empty() {
        return;
    }
    println!("{label}:");
    for contract in contracts {
        println!(
            "  - {}{}: {}",
            contract.key,
            contract
                .contract_id
                .as_ref()
                .map_or_else(String::new, |id| format!(" ({id})")),
            contract.reason
        );
    }
}

fn print_optional(label: &str, value: Option<&String>) {
    println!(
        "{}: {}",
        label,
        value.map_or("not deployed", String::as_str)
    );
}

#[derive(Debug, Serialize)]
struct OutputEnvelope<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    ok: bool,
    network: &'a str,
    manifest: String,
    commands: Vec<String>,
    tx_hashes: Vec<String>,
    warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<&'a Response>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorEnvelope>,
}

impl<'a> OutputEnvelope<'a> {
    fn success(cli: &'a Cli, response: &'a Response) -> Self {
        Self {
            kind: response.kind(),
            ok: true,
            network: &cli.network,
            manifest: cli.state.display().to_string(),
            commands: response.command_shapes(),
            tx_hashes: response.tx_hashes(),
            warnings: response.warnings(),
            data: Some(response),
            error: None,
        }
    }

    fn error(cli: &'a Cli, error: &anyhow::Error) -> Self {
        Self {
            kind: "error",
            ok: false,
            network: &cli.network,
            manifest: cli.state.display().to_string(),
            commands: Vec::new(),
            tx_hashes: Vec::new(),
            warnings: Vec::new(),
            data: None,
            error: Some(ErrorEnvelope {
                code: classify_error(error),
                message: error.to_string(),
            }),
        }
    }
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    code: &'static str,
    message: String,
}

fn classify_error(error: &anyhow::Error) -> &'static str {
    classify_error_message(&error.to_string())
}

fn classify_error_message(message: &str) -> &'static str {
    if message.contains("missing ") && message.contains(" contract id in manifest") {
        "missing_manifest_contract"
    } else if message.contains("mainnet write blocked") {
        "mainnet_guard"
    } else if message.contains("do not pass secret keys")
        || message.contains("without exposing it to child argv")
    {
        "secret_in_argv"
    } else {
        "command_failed"
    }
}

#[derive(Debug, Serialize)]
struct ParseErrorEnvelope {
    #[serde(rename = "type")]
    kind: &'static str,
    ok: bool,
    network: String,
    manifest: String,
    commands: Vec<String>,
    tx_hashes: Vec<String>,
    warnings: Vec<String>,
    error: ErrorEnvelope,
}

impl ParseErrorEnvelope {
    fn new(raw_args: &[String], error: &clap::Error) -> Self {
        let message = error.to_string();
        Self {
            kind: "error",
            ok: false,
            network: raw_arg_value(raw_args, "--network").unwrap_or_else(|| "testnet".to_string()),
            manifest: raw_arg_value(raw_args, "--state").unwrap_or_else(|| {
                "contract/vault/soroban/.deploy-state/manifest.json".to_string()
            }),
            commands: Vec::new(),
            tx_hashes: Vec::new(),
            warnings: Vec::new(),
            error: ErrorEnvelope {
                code: match classify_error_message(&message) {
                    "command_failed" => "invalid_args",
                    code => code,
                },
                message,
            },
        }
    }
}

fn raw_arg_value(raw_args: &[String], flag: &str) -> Option<String> {
    raw_args.iter().enumerate().find_map(|(index, arg)| {
        if arg == flag {
            return raw_args.get(index + 1).cloned();
        }
        arg.strip_prefix(flag)
            .and_then(|rest| rest.strip_prefix('='))
            .map(ToString::to_string)
    })
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Response {
    Message { message: String },
    Command { stdout: String, stderr: String },
    Status(StatusResponse),
    Env(Vec<(String, String)>),
    ExtendTtl(ExtendTtlResponse),
    Reconcile(ReconcileResponse),
    Doctor(DoctorResponse),
    Plan(PlanResponse),
    GovernanceQueue(GovernanceQueueResponse),
    GovernanceExplain(GovernanceProposalView),
    GovernanceAcceptReady(GovernanceAcceptReadyResponse),
}

impl Response {
    fn message(message: String) -> Self {
        Self::Message { message }
    }

    const fn kind(&self) -> &'static str {
        match self {
            Self::Message { .. } => "message",
            Self::Command { .. } => "command",
            Self::Status(_) => "status",
            Self::Env(_) => "env",
            Self::ExtendTtl(_) => "extend_ttl",
            Self::Reconcile(_) => "reconcile",
            Self::Doctor(_) => "doctor",
            Self::Plan(_) => "plan",
            Self::GovernanceQueue(_) => "governance_queue",
            Self::GovernanceExplain(_) => "governance_explain",
            Self::GovernanceAcceptReady(_) => "governance_accept_ready",
        }
    }

    fn warnings(&self) -> Vec<String> {
        match self {
            Self::Plan(plan) => plan.warnings.clone(),
            Self::GovernanceQueue(queue) => queue.warnings.clone(),
            Self::GovernanceAcceptReady(result) => result.skipped.clone(),
            Self::Reconcile(result) => result
                .components
                .iter()
                .flat_map(|component| component.warnings.clone())
                .collect(),
            Self::Doctor(result) => result
                .checks
                .iter()
                .filter(|check| check.status == DoctorStatus::Warn)
                .map(|check| format!("{}: {}", check.name, check.message))
                .collect(),
            _ => Vec::new(),
        }
    }

    fn command_shapes(&self) -> Vec<String> {
        match self {
            Self::Plan(plan) => plan.stellar_commands.clone(),
            _ => Vec::new(),
        }
    }

    fn tx_hashes(&self) -> Vec<String> {
        match self {
            Self::Command { stdout, stderr } => parse_tx_hashes(stdout)
                .into_iter()
                .chain(parse_tx_hashes(stderr))
                .collect(),
            _ => Vec::new(),
        }
    }
}

fn parse_tx_hashes(value: &str) -> Vec<String> {
    value
        .split(|c: char| !c.is_ascii_hexdigit())
        .filter(|token| token.len() == 64)
        .map(str::to_ascii_lowercase)
        .collect()
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
    custodial_adapters: Vec<CustodialAdapterStatus>,
}

#[derive(Debug, Serialize)]
struct BlendAdapterStatus {
    key: String,
    contract_id: String,
    pool: Option<String>,
}

#[derive(Debug, Serialize)]
struct CustodialAdapterStatus {
    key: String,
    contract_id: String,
    custodian: Option<String>,
    asset: Option<String>,
}

#[derive(Debug, Serialize)]
struct ExtendTtlResponse {
    extended: Vec<String>,
    skipped: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ReconcileResponse {
    safe_to_resume: bool,
    drift_detected: bool,
    components: Vec<ReconcileComponent>,
    repair_actions: Vec<String>,
    safe_next_steps: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ReconcileComponent {
    key: String,
    contract_id: Option<String>,
    manifest_recorded: bool,
    manifest_initialized: bool,
    recorded_wasm_hash: Option<String>,
    chain_wasm_hash: Option<String>,
    status: ReconcileStatus,
    wiring: Vec<WiringCheck>,
    warnings: Vec<String>,
    repair_actions: Vec<String>,
}

impl ReconcileComponent {
    const fn safe_to_resume(&self) -> bool {
        match self.status {
            ReconcileStatus::Initialized | ReconcileStatus::Deployed => true,
            ReconcileStatus::Missing => !self.manifest_recorded,
            ReconcileStatus::Unknown | ReconcileStatus::Mismatched => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReconcileStatus {
    Missing,
    Deployed,
    Initialized,
    Unknown,
    Mismatched,
}

impl ReconcileStatus {
    const fn as_label(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Deployed => "deployed",
            Self::Initialized => "initialized",
            Self::Unknown => "unknown",
            Self::Mismatched => "mismatched",
        }
    }

    const fn is_drift(self) -> bool {
        matches!(self, Self::Unknown | Self::Mismatched)
    }
}

#[derive(Debug, Serialize)]
struct WiringCheck {
    field: String,
    expected: Option<String>,
    observed: Option<String>,
    status: WiringStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum WiringStatus {
    Match,
    Mismatch,
    Unknown,
}

#[derive(Debug, Serialize)]
struct DoctorResponse {
    ok: bool,
    checks: Vec<DoctorCheck>,
}

#[derive(Debug, Serialize)]
struct PlanResponse {
    scope: String,
    network: String,
    required_signers: Vec<String>,
    contracts_to_reuse: Vec<PlanContract>,
    contracts_to_deploy: Vec<PlanContract>,
    wasm: Vec<PlanWasm>,
    manifest_mutations: Vec<String>,
    stellar_commands: Vec<String>,
    warnings: Vec<String>,
}

impl PlanResponse {
    fn new(scope: impl Into<String>, network: &str) -> Self {
        Self {
            scope: scope.into(),
            network: network.to_string(),
            required_signers: Vec::new(),
            contracts_to_reuse: Vec::new(),
            contracts_to_deploy: Vec::new(),
            wasm: Vec::new(),
            manifest_mutations: Vec::new(),
            stellar_commands: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

#[derive(Debug, Serialize)]
struct PlanContract {
    key: String,
    contract_id: Option<String>,
    reason: String,
}

#[derive(Debug, Serialize)]
struct PlanWasm {
    key: String,
    package: String,
    path: String,
    local_hash: Option<String>,
    recorded_remote_hash: Option<String>,
    action: String,
}

#[derive(Debug, Serialize)]
struct GovernanceQueueResponse {
    proposals: Vec<GovernanceProposalView>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct GovernanceProposalView {
    proposal_id: u64,
    action: String,
    valid_after_ns: Option<u64>,
    ready: Option<bool>,
    eta_seconds: Option<i64>,
    raw: String,
}

#[derive(Debug, Serialize)]
struct GovernanceAcceptReadyResponse {
    accepted: Vec<u64>,
    skipped: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DoctorCheck {
    name: String,
    status: DoctorStatus,
    message: String,
}

impl DoctorCheck {
    fn pass(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: DoctorStatus::Pass,
            message: message.into(),
        }
    }

    fn warn(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: DoctorStatus::Warn,
            message: message.into(),
        }
    }

    fn fail(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: DoctorStatus::Fail,
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DoctorStatus {
    Pass,
    Warn,
    Fail,
}

impl DoctorStatus {
    const fn as_label(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Mutex};

    use sha2::{Digest, Sha256};

    use crate::{
        artifacts::ArtifactSpec,
        cli::{
            ArtifactName, Cli, Commands, DeployArgs, DeployCommand, DeployStackArgs, ExtendTtlArgs,
            GovernanceArgs, ShareTokenArgs, UserArgs, DEFAULT_CONTRACT_SOURCE_REPO,
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
        fn run(
            &self,
            program: &str,
            args: &[String],
            _redacted_args: &[usize],
            _env: &[crate::stellar::CommandEnv],
        ) -> anyhow::Result<CommandOutput> {
            self.calls
                .lock()
                .expect("lock calls")
                .push((program.to_string(), args.to_vec()));
            if args.iter().any(|arg| arg == "pending_ids") {
                return Ok(CommandOutput {
                    stdout: "[1, 2]".to_string(),
                    stderr: String::new(),
                });
            }
            if args
                .iter()
                .any(|arg| arg == "submit_set_timelock" || arg == "submit_set_supply_queue")
            {
                return Ok(CommandOutput {
                    stdout: "proposal 1".to_string(),
                    stderr: String::new(),
                });
            }
            if args.iter().any(|arg| arg == "pending") {
                let proposal_id = args
                    .windows(2)
                    .find_map(|pair| (pair[0] == "--proposal_id").then(|| pair[1].as_str()))
                    .unwrap_or("0");
                let valid_after_ns = if proposal_id == "1" { 0 } else { u64::MAX };
                return Ok(CommandOutput {
                    stdout: format!(
                        "{{id: {proposal_id}, action: SetPaused(false), valid_after_ns: {valid_after_ns}}}"
                    ),
                    stderr: String::new(),
                });
            }
            Ok(CommandOutput {
                stdout: CONTRACT.to_string(),
                stderr: String::new(),
            })
        }
    }

    struct FailingInitializeExecutor {
        inner: RecordingExecutor,
    }

    impl FailingInitializeExecutor {
        fn new() -> Self {
            Self {
                inner: RecordingExecutor::new(),
            }
        }
    }

    impl CommandExecutor for FailingInitializeExecutor {
        fn run(
            &self,
            program: &str,
            args: &[String],
            redacted_args: &[usize],
            env: &[crate::stellar::CommandEnv],
        ) -> anyhow::Result<CommandOutput> {
            if args.iter().any(|arg| arg == "initialize") {
                self.inner
                    .calls
                    .lock()
                    .expect("lock calls")
                    .push((program.to_string(), args.to_vec()));
                anyhow::bail!("forced initialize failure");
            }
            self.inner.run(program, args, redacted_args, env)
        }
    }

    struct ChainStateExecutor {
        wasm: &'static [u8],
    }

    impl CommandExecutor for ChainStateExecutor {
        fn run(
            &self,
            _program: &str,
            args: &[String],
            _redacted_args: &[usize],
            _env: &[crate::stellar::CommandEnv],
        ) -> anyhow::Result<CommandOutput> {
            if args
                .windows(2)
                .any(|pair| pair[0] == "--id" && pair[1] == CONTRACT)
            {
                if let Some(path) = args
                    .windows(2)
                    .find_map(|pair| (pair[0] == "--out-file").then(|| &pair[1]))
                {
                    fs::write(path, self.wasm).expect("write fetched wasm");
                }
                return Ok(CommandOutput {
                    stdout: CONTRACT.to_string(),
                    stderr: String::new(),
                });
            }
            anyhow::bail!("contract not found")
        }
    }

    fn submitted_calls(calls: &[(String, Vec<String>)]) -> Vec<(String, Vec<String>)> {
        calls
            .iter()
            .filter(|(_, args)| {
                !args
                    .windows(2)
                    .any(|pair| pair[0] == "--send" && pair[1] == "no")
                    && !args.iter().any(|arg| arg == "--build-only")
                    && !matches!(args.as_slice(), [first, second, ..] if first == "tx" && second == "simulate")
            })
            .cloned()
            .collect()
    }

    fn assert_contract_invokes_are_views(calls: &[(String, Vec<String>)]) {
        for (_, args) in calls
            .iter()
            .filter(|(_, args)| args.windows(2).any(|pair| pair == ["contract", "invoke"]))
        {
            assert!(
                args.windows(2).any(|pair| pair == ["--send", "no"]),
                "contract invoke should use --send no: {args:?}"
            );
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
    fn parse_proposal_id_ignores_confirmed_tx_hash_suffix() {
        let proposal_id =
            parse_proposal_id("proposal 1\ntx hash: abcdef9876543210").expect("proposal id");

        assert_eq!(proposal_id, 1);
    }

    #[test]
    fn export_env_uses_manifest_contracts() {
        let mut manifest = Manifest::new("testnet", None);
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
    fn json_envelope_has_stable_machine_fields() {
        let cli = base_cli("manifest.json".into(), Commands::Status);
        let response = Response::message("ok".to_string());
        let value =
            serde_json::to_value(OutputEnvelope::success(&cli, &response)).expect("json envelope");

        assert_eq!(value["type"], "message");
        assert_eq!(value["ok"], true);
        assert_eq!(value["network"], "testnet");
        assert_eq!(value["manifest"], "manifest.json");
        assert!(value["commands"].is_array());
        assert!(value["tx_hashes"].is_array());
        assert!(value["warnings"].is_array());
        assert_eq!(value["data"]["type"], "message");
    }

    #[test]
    fn reconcile_classifies_matching_recorded_contract_as_initialized() {
        let wasm_hash = format!("{:x}", Sha256::digest(b"vault wasm"));
        let mut manifest = Manifest::new("testnet", None);
        manifest.contracts.insert(
            "vault".to_string(),
            ContractRecord {
                contract_id: CONTRACT.to_string(),
                wasm_hash: wasm_hash.clone(),
                salt: None,
                constructor_args: BTreeMap::new(),
                deploy_tx: None,
                initialized: true,
            },
        );
        let cli = base_cli("manifest.json".into(), Commands::Status);
        let executor = ChainStateExecutor {
            wasm: b"vault wasm",
        };
        let stellar = Stellar::new(&cli, &executor);

        let response = reconcile_manifest(&stellar, &manifest, false);

        let vault = response
            .components
            .iter()
            .find(|component| component.key == "vault")
            .expect("vault component");
        assert_eq!(vault.status, ReconcileStatus::Initialized);
        assert_eq!(vault.chain_wasm_hash.as_deref(), Some(wasm_hash.as_str()));
        assert!(response.safe_to_resume);
    }

    #[test]
    fn reconcile_detects_wasm_hash_mismatch_and_blocks_resume() {
        let mut manifest = Manifest::new("testnet", None);
        manifest.contracts.insert(
            "vault".to_string(),
            ContractRecord {
                contract_id: CONTRACT.to_string(),
                wasm_hash: "different".to_string(),
                salt: None,
                constructor_args: BTreeMap::new(),
                deploy_tx: None,
                initialized: true,
            },
        );
        let cli = base_cli("manifest.json".into(), Commands::Status);
        let executor = ChainStateExecutor {
            wasm: b"vault wasm",
        };
        let stellar = Stellar::new(&cli, &executor);

        let response = reconcile_manifest(&stellar, &manifest, false);

        let vault = response
            .components
            .iter()
            .find(|component| component.key == "vault")
            .expect("vault component");
        assert_eq!(vault.status, ReconcileStatus::Mismatched);
        assert!(!response.safe_to_resume);
        assert!(response.drift_detected);
    }

    #[test]
    fn resume_repair_marks_chain_initialized_manifest_records_initialized() {
        let wasm_hash = format!("{:x}", Sha256::digest(b"share wasm"));
        let mut manifest = Manifest::new("testnet", None);
        manifest.contracts.insert(
            "vault".to_string(),
            ContractRecord {
                contract_id: CONTRACT.to_string(),
                wasm_hash: "predeployed".to_string(),
                salt: None,
                constructor_args: BTreeMap::new(),
                deploy_tx: None,
                initialized: true,
            },
        );
        manifest.contracts.insert(
            "share_token".to_string(),
            ContractRecord {
                contract_id: CONTRACT.to_string(),
                wasm_hash,
                salt: None,
                constructor_args: map_args([("vault", CONTRACT), ("admin", CONTRACT)]),
                deploy_tx: None,
                initialized: false,
            },
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join("manifest.json");
        let cli = base_cli(state.clone(), Commands::Status);
        let executor = ChainStateExecutor {
            wasm: b"share wasm",
        };
        let stellar = Stellar::new(&cli, &executor);

        let response = reconcile_manifest(&stellar, &manifest, true);
        let share = response
            .components
            .iter()
            .find(|component| component.key == "share_token")
            .expect("share component");
        assert_eq!(share.status, ReconcileStatus::Initialized);
        assert!(!share.manifest_initialized);
        assert!(!share.warnings.is_empty());

        apply_reconcile_safe_manifest_updates(&cli, &mut manifest, &response)
            .expect("apply safe repair");

        let share = manifest
            .contracts
            .get("share_token")
            .expect("share token record");
        assert!(share.initialized);
        let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
        assert!(
            loaded
                .contracts
                .get("share_token")
                .expect("saved share token record")
                .initialized
        );
    }

    #[test]
    fn parse_error_envelope_reports_secret_argv_code() {
        let error = clap::Error::raw(
            clap::error::ErrorKind::ValueValidation,
            "do not pass secret keys via --source-account",
        );
        let value = serde_json::to_value(ParseErrorEnvelope::new(
            &[
                "tmplr-soroban-vault".to_string(),
                "--json".to_string(),
                "--network".to_string(),
                "testnet".to_string(),
                "status".to_string(),
            ],
            &error,
        ))
        .expect("json envelope");

        assert_eq!(value["type"], "error");
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "secret_in_argv");
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
                    assets: None,
                    assets_raw: Some(1),
                    asset_decimals: 7,
                    min_shares_out: None,
                    min_shares_out_raw: 0,
                    share_decimals: ShareDecimalsArg::Manifest,
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
    fn doctor_checks_stellar_and_source_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fake_stack_wasms(dir.path());
        fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").expect("write Cargo.toml");
        let cli = Cli {
            workspace_path: dir.path().into(),
            command: Commands::Doctor,
            ..base_cli(dir.path().join("manifest.json"), Commands::Status)
        };
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("run doctor");

        let calls = executor.calls();
        assert!(calls
            .iter()
            .any(|(_, args)| args == &["--version".to_string()]));
        assert!(calls.iter().any(|(_, args)| args
            == &[
                "keys".to_string(),
                "address".to_string(),
                "alice".to_string()
            ]));
    }

    #[test]
    fn user_deposit_prefers_erc4626_proxy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join("manifest.json");
        let mut manifest = Manifest::new("testnet", None);
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
            state.clone(),
            Commands::User(UserArgs {
                command: UserCommand::Deposit {
                    operator: ACCOUNT.parse().expect("operator"),
                    receiver: Some(ACCOUNT.parse().expect("receiver")),
                    assets: None,
                    assets_raw: Some(11),
                    asset_decimals: 7,
                    min_shares_out: None,
                    min_shares_out_raw: 7,
                    share_decimals: ShareDecimalsArg::Manifest,
                },
            }),
        );
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("run deposit");

        let calls = submitted_calls(&executor.calls());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "stellar");
        assert!(calls[0].1.windows(2).any(|pair| pair == ["--id", "CPROXY"]));
        assert!(calls[0].1.iter().any(|arg| arg == "deposit_with_min"));
        assert!(calls[0].1.windows(2).any(|pair| pair == ["--assets", "11"]));
        let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
        let tx = loaded
            .transactions
            .last()
            .expect("transaction record should be written");
        assert_eq!(tx.command.as_deref(), Some("user"));
        assert_eq!(tx.contract_id.as_deref(), Some("CPROXY"));
        assert_eq!(tx.function.as_deref(), Some("deposit_with_min"));
        assert_eq!(tx.result_status.as_deref(), Some("success"));
    }

    #[test]
    fn deploy_adapters_appends_new_pool_to_existing_manifest() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fake_blend_wasm(dir.path());
        let state = dir.path().join("manifest.json");
        let mut manifest = Manifest::new("testnet", None);
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
                    custodians: Vec::new(),
                    build: false,
                    force_new: false,
                }),
            }),
            ..base_cli(state.clone(), Commands::Status)
        };
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("deploy adapters");

        let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
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
        let adapter_deploys = submitted_calls(&executor.calls())
            .iter()
            .filter(|(_, args)| args.iter().any(|arg| arg == "--pool"))
            .count();
        assert_eq!(adapter_deploys, 1);
    }

    #[test]
    fn deploy_adapters_appends_custodial_adapter_to_existing_manifest() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fake_custodial_wasm(dir.path());
        let state = dir.path().join("manifest.json");
        let mut manifest = Manifest::new("testnet", None);
        manifest
            .contracts
            .insert("vault".to_string(), imported_record(CONTRACT));
        manifest
            .contracts
            .insert("governance".to_string(), imported_record(CONTRACT));
        manifest
            .contracts
            .insert("asset_token".to_string(), imported_record(CONTRACT));
        manifest.save(&state).expect("save manifest");
        let cli = Cli {
            workspace_path: dir.path().into(),
            command: Commands::Deploy(DeployArgs {
                command: DeployCommand::Adapters(crate::cli::DeployAdaptersArgs {
                    vault: None,
                    governance: None,
                    asset_token: None,
                    blend_pools: Vec::new(),
                    custodians: vec![ACCOUNT.parse().expect("custodian")],
                    build: false,
                    force_new: false,
                }),
            }),
            ..base_cli(state.clone(), Commands::Status)
        };
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("deploy custodial adapter");

        let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
        let adapter = loaded
            .contracts
            .get("custodial_adapter_0")
            .expect("appended custodial adapter");
        assert_eq!(
            adapter
                .constructor_args
                .get("custodian")
                .map(String::as_str),
            Some(ACCOUNT)
        );
        assert_eq!(
            adapter.constructor_args.get("vault").map(String::as_str),
            Some(CONTRACT)
        );
        assert_eq!(
            adapter.constructor_args.get("admin").map(String::as_str),
            Some(CONTRACT)
        );
        assert_eq!(
            adapter.constructor_args.get("asset").map(String::as_str),
            Some(CONTRACT)
        );
        let calls = submitted_calls(&executor.calls());
        let adapter_deploys = calls
            .iter()
            .filter(|(_, args)| args.iter().any(|arg| arg == "--custodian"))
            .count();
        assert_eq!(adapter_deploys, 1);
        assert!(calls.iter().any(|(_, args)| args
            .windows(2)
            .any(|window| window[0] == "--asset" && window[1] == CONTRACT)));
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
                    custodians: Vec::new(),
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
    fn deploy_plan_does_not_execute_or_write_manifest() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fake_stack_wasms(dir.path());
        let state = dir.path().join("manifest.json");
        manifest_with_governance_and_vault(&state);
        let before = fs::read_to_string(&state).expect("read manifest");
        let cli = Cli {
            workspace_path: dir.path().into(),
            command: Commands::Deploy(DeployArgs {
                command: DeployCommand::Plan(crate::cli::DeployPlanArgs {
                    command: crate::cli::DeployPlanCommand::Stack(Box::new(DeployStackArgs {
                        admin: Some(ACCOUNT.parse().expect("admin")),
                        asset_token: Some(CONTRACT.parse().expect("asset token")),
                        governance_timelock_ns: Some(1_000),
                        virtual_shares: 0,
                        virtual_assets: 0,
                        share_name: "Templar Vault Share".to_string(),
                        share_symbol: "tvSHARE".to_string(),
                        share_decimals: 7,
                        blend_pools: vec![CONTRACT.parse().expect("pool")],
                        custodians: Vec::new(),
                        build: false,
                        force_new: false,
                    })),
                }),
            }),
            ..base_cli(state.clone(), Commands::Status)
        };
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("deploy plan");

        assert!(executor.calls().is_empty());
        let after = fs::read_to_string(&state).expect("read manifest");
        assert_eq!(before, after);
    }

    #[test]
    fn extend_ttl_runs_for_entire_ttl_capable_deployment_set() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join("manifest.json");
        let mut manifest = Manifest::new("testnet", None);
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
        manifest.contracts.insert(
            "custodial_adapter_0".to_string(),
            imported_record("CCUSTODIAL0"),
        );
        manifest.save(&state).expect("save manifest");
        let cli = base_cli(
            state,
            Commands::ExtendTtl(ExtendTtlArgs {
                caller: Some(ACCOUNT.parse().expect("caller")),
            }),
        );
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("extend ttl");

        let calls = submitted_calls(&executor.calls());
        assert_eq!(calls.len(), 7);
        assert!(calls.iter().any(|(_, args)| args
            .windows(2)
            .any(|pair| pair == ["--id", "CVAULT"])
            && args.iter().any(|arg| arg == "execute")));
        for contract_id in [
            "CGOVERNANCE",
            "CPROXY4626",
            "CCURATORPROXY",
            "CSHARE",
            "CADAPTER0",
            "CCUSTODIAL0",
        ] {
            assert!(calls.iter().any(|(_, args)| args
                .windows(2)
                .any(|pair| pair == ["--id", contract_id])
                && args.iter().any(|arg| arg == "extend_ttl")));
        }
        assert!(!calls
            .iter()
            .any(|(_, args)| args.windows(2).any(|pair| pair == ["--id", "CASSET"])));
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

        let calls = submitted_calls(&executor.calls());
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
    fn governance_accept_ready_accepts_only_ready_decoded_proposals() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join("manifest.json");
        manifest_with_governance(&state);
        let cli = base_cli(
            state,
            Commands::Governance(GovernanceArgs {
                command: GovernanceCommand::AcceptReady {
                    admin: ACCOUNT.parse().expect("admin"),
                    kind: None,
                    limit: None,
                },
            }),
        );
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("accept ready proposals");

        let calls = submitted_calls(&executor.calls());
        let accepted = calls
            .iter()
            .filter(|(_, args)| {
                args.iter().any(|arg| arg == "accept")
                    && args.windows(2).any(|pair| pair == ["--proposal_id", "1"])
            })
            .count();
        assert_eq!(accepted, 1);
        assert!(!calls
            .iter()
            .any(|(_, args)| args.iter().any(|arg| arg == "accept")
                && args.windows(2).any(|pair| pair == ["--proposal_id", "2"])));
    }

    #[test]
    fn governance_read_only_commands_use_view_invocation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join("manifest.json");
        manifest_with_governance(&state);

        for command in [
            GovernanceCommand::Pending { proposal_id: None },
            GovernanceCommand::Pending {
                proposal_id: Some(1),
            },
            GovernanceCommand::Timelocks,
            GovernanceCommand::Queue { kind: None },
            GovernanceCommand::Explain { proposal_id: 1 },
        ] {
            let cli = base_cli(
                state.clone(),
                Commands::Governance(GovernanceArgs { command }),
            );
            let executor = RecordingExecutor::new();

            run(&cli, &executor).expect("run governance view");

            let calls = executor.calls();
            assert_contract_invokes_are_views(&calls);
            assert!(submitted_calls(&calls).is_empty());
        }
    }

    #[test]
    fn governance_submit_and_wait_submits_then_accepts_ready_proposal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join("manifest.json");
        manifest_with_governance(&state);
        let cli = base_cli(
            state,
            Commands::Governance(GovernanceArgs {
                command: GovernanceCommand::SubmitAndWait(
                    crate::cli::GovernanceSubmitAndWaitArgs {
                        poll_seconds: 1,
                        max_wait_seconds: 0,
                        command: GovernanceSubmitAndWaitCommand::SetTimelock {
                            admin: ACCOUNT.parse().expect("admin"),
                            kind: "supply-queue".parse().expect("kind"),
                            timelock_ns: 42,
                        },
                    },
                ),
            }),
        );
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("submit and wait");

        let calls = executor.calls();
        assert!(calls
            .iter()
            .any(|(_, args)| args.iter().any(|arg| arg == "submit_set_timelock")));
        assert!(calls
            .iter()
            .any(|(_, args)| args.iter().any(|arg| arg == "accept")
                && args.windows(2).any(|pair| pair == ["--proposal_id", "1"])));
    }

    #[test]
    fn user_share_token_and_adapter_read_only_commands_use_view_invocation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join("manifest.json");
        manifest_with_view_contracts(&state);

        let commands = [
            Commands::User(UserArgs {
                command: UserCommand::Balance {
                    owner: ACCOUNT.parse().expect("owner"),
                },
            }),
            Commands::User(UserArgs {
                command: UserCommand::Preview {
                    owner: ACCOUNT.parse().expect("owner"),
                    assets: None,
                    assets_raw: 0,
                    asset_decimals: 7,
                    shares: None,
                    shares_raw: 0,
                    share_decimals: "manifest".parse().expect("share decimals"),
                },
            }),
            Commands::ShareToken(ShareTokenArgs {
                command: ShareTokenCommand::Balance {
                    account: ACCOUNT.parse().expect("account"),
                },
            }),
            Commands::ShareToken(ShareTokenArgs {
                command: ShareTokenCommand::Admin,
            }),
            Commands::Adapter(AdapterArgs {
                adapter_index: 0,
                adapter_key: None,
                adapter_pool: None,
                command: AdapterCommand::TotalAssets {
                    asset: CONTRACT.parse().expect("asset"),
                },
            }),
            Commands::Adapter(AdapterArgs {
                adapter_index: 0,
                adapter_key: None,
                adapter_pool: None,
                command: AdapterCommand::Pool,
            }),
        ];

        for command in commands {
            let cli = base_cli(state.clone(), command);
            let executor = RecordingExecutor::new();

            run(&cli, &executor).expect("run read-only command");

            let calls = executor.calls();
            assert_contract_invokes_are_views(&calls);
            assert!(submitted_calls(&calls).is_empty());
        }
    }

    #[test]
    fn deploy_stack_deploys_one_blend_adapter_per_pool() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fake_stack_wasms(dir.path());
        let state = dir.path().join("manifest.json");
        let mut manifest = Manifest::new("testnet", None);
        manifest
            .contracts
            .insert("vault".to_string(), imported_record(CONTRACT));
        manifest.save(&state).expect("save manifest");
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
                    custodians: Vec::new(),
                    build: false,
                    force_new: false,
                })),
            }),
            ..base_cli(state, Commands::Status)
        };
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("deploy stack");

        let calls = submitted_calls(&executor.calls());
        let adapter_deploys = calls
            .iter()
            .filter(|(_, args)| args.iter().any(|arg| arg == "--pool"))
            .count();
        assert_eq!(adapter_deploys, 2);
    }

    #[test]
    fn deploy_stack_deploys_one_custodial_adapter_per_custodian() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fake_stack_wasms(dir.path());
        let state = dir.path().join("manifest.json");
        let mut manifest = Manifest::new("testnet", None);
        manifest
            .contracts
            .insert("vault".to_string(), imported_record(CONTRACT));
        manifest.save(&state).expect("save manifest");
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
                    blend_pools: Vec::new(),
                    custodians: vec![ACCOUNT.parse().expect("custodian")],
                    build: false,
                    force_new: false,
                })),
            }),
            ..base_cli(state.clone(), Commands::Status)
        };
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("deploy stack");

        let calls = submitted_calls(&executor.calls());
        let adapter_deploys = calls
            .iter()
            .filter(|(_, args)| args.iter().any(|arg| arg == "--custodian"))
            .count();
        assert_eq!(adapter_deploys, 1);

        let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
        assert_eq!(
            loaded
                .contracts
                .get("custodial_adapter_0")
                .expect("custodial adapter")
                .constructor_args
                .get("custodian")
                .map(String::as_str),
            Some(ACCOUNT)
        );
        assert_eq!(
            loaded
                .contracts
                .get("custodial_adapter_0")
                .expect("custodial adapter")
                .constructor_args
                .get("asset")
                .map(String::as_str),
            Some(CONTRACT)
        );
        assert!(calls.iter().any(|(_, args)| args
            .windows(2)
            .any(|window| window[0] == "--asset" && window[1] == CONTRACT)));
    }

    #[test]
    fn deploy_stack_checkpoints_manifest_before_initialize_failure() {
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
                    blend_pools: Vec::new(),
                    custodians: Vec::new(),
                    build: false,
                    force_new: false,
                })),
            }),
            ..base_cli(state.clone(), Commands::Status)
        };
        let executor = FailingInitializeExecutor::new();

        let err = run(&cli, &executor).expect_err("initialize should fail");
        assert!(
            err.to_string().contains("forced initialize failure")
                || err.to_string().contains("preflight simulation failed")
        );

        let manifest = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
        for key in ["vault", "share_token", "governance", "asset_token"] {
            assert!(
                manifest.contracts.contains_key(key),
                "{key} should be checkpointed"
            );
        }
        assert!(
            !manifest
                .contracts
                .get("vault")
                .expect("vault record")
                .initialized
        );
        assert!(
            manifest
                .contracts
                .get("share_token")
                .expect("share token record")
                .initialized
        );
        assert!(
            manifest
                .contracts
                .get("governance")
                .expect("governance record")
                .initialized
        );
        assert!(!manifest.contracts.contains_key("proxy_4626"));
        assert!(manifest.artifacts.contains_key("vault"));
        assert!(manifest.transactions.is_empty());
    }

    #[test]
    fn deploy_contract_helper_checkpoints_contract_record_immediately() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join("manifest.json");
        let cli = base_cli(state.clone(), Commands::Status);
        let executor = RecordingExecutor::new();
        let stellar = Stellar::new(&cli, &executor);
        let mut manifest = Manifest::new("testnet", None);

        let contract_id = deploy_contract_if_needed(
            &cli,
            &stellar,
            &mut manifest,
            "vault",
            "abc123",
            Vec::new(),
            BTreeMap::new(),
            false,
        )
        .expect("deploy contract");

        let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
        let record = loaded.contracts.get("vault").expect("vault record");
        assert_eq!(contract_id, CONTRACT);
        assert_eq!(record.contract_id, CONTRACT);
        assert!(!record.initialized);
    }

    #[test]
    fn deploy_contract_helper_marks_constructor_deployments_initialized() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join("manifest.json");
        let cli = base_cli(state.clone(), Commands::Status);
        let executor = RecordingExecutor::new();
        let stellar = Stellar::new(&cli, &executor);
        let mut manifest = Manifest::new("testnet", None);

        deploy_contract_if_needed(
            &cli,
            &stellar,
            &mut manifest,
            "governance",
            "abc123",
            args([("--admin", ACCOUNT), ("--vault", CONTRACT)]),
            map_args([("admin", ACCOUNT), ("vault", CONTRACT)]),
            false,
        )
        .expect("deploy contract");

        let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
        let record = loaded
            .contracts
            .get("governance")
            .expect("governance record");
        assert!(record.initialized);
    }

    #[test]
    fn initialize_proxy_helper_checkpoints_initialized_state_immediately() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join("manifest.json");
        let cli = base_cli(state.clone(), Commands::Status);
        let executor = RecordingExecutor::new();
        let stellar = Stellar::new(&cli, &executor);
        let mut manifest = Manifest::new("testnet", None);
        manifest
            .contracts
            .insert("proxy_4626".to_string(), uninitialized_record(CONTRACT));
        manifest.save(&state).expect("save manifest");

        initialize_proxy_if_needed(
            &cli,
            &stellar,
            &mut manifest,
            "proxy_4626",
            CONTRACT,
            Vec::new(),
        )
        .expect("initialize proxy");

        let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
        assert!(
            loaded
                .contracts
                .get("proxy_4626")
                .expect("proxy record")
                .initialized
        );
    }

    #[test]
    fn deploy_stack_without_blend_pools_skips_blend_adapter() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fake_stack_wasms(dir.path());
        let state = dir.path().join("manifest.json");
        let cli = Cli {
            workspace_path: dir.path().into(),
            dry_run: true,
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
                    blend_pools: Vec::new(),
                    custodians: Vec::new(),
                    build: false,
                    force_new: false,
                })),
            }),
            ..base_cli(state, Commands::Status)
        };
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("deploy stack without blend pools");

        assert!(executor.calls().is_empty());
    }

    #[test]
    fn deploy_wasm_build_embeds_source_repo_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        let governance_path =
            ArtifactSpec::from_name(ArtifactName::Governance).wasm_path(dir.path());
        fs::create_dir_all(governance_path.parent().expect("parent")).expect("create parent");
        fs::write(&governance_path, "governance").expect("write wasm");
        fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").expect("write Cargo.toml");
        let cli = Cli {
            workspace_path: dir.path().into(),
            command: Commands::Deploy(DeployArgs {
                command: DeployCommand::Wasm(crate::cli::DeployWasmArgs {
                    artifact: ArtifactName::Governance,
                    build: true,
                }),
            }),
            ..base_cli(dir.path().join("manifest.json"), Commands::Status)
        };
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("deploy wasm");

        let calls = executor.calls();
        let build = calls
            .iter()
            .find(|(_, args)| args.windows(2).any(|pair| pair == ["contract", "build"]))
            .expect("build command should run");
        assert!(build
            .1
            .windows(2)
            .any(|pair| pair == ["--meta", "source_repo=github:Templar-Protocol/contracts"]));
    }

    fn base_cli(state: std::path::PathBuf, command: Commands) -> Cli {
        Cli {
            profile: None,
            network: "testnet".to_string(),
            rpc_url: None,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            source_account: Some("alice".parse().expect("source account")),
            config_dir: None,
            contract_source_repo: Some(DEFAULT_CONTRACT_SOURCE_REPO.to_string()),
            state,
            workspace_path: ".".into(),
            json: true,
            json_lines: false,
            dry_run: false,
            yes: false,
            allow_mainnet_write: false,
            allow_zero_timelock: false,
            command,
        }
    }

    fn manifest_with_governance(path: &std::path::Path) {
        let mut manifest = Manifest::new("testnet", None);
        manifest
            .contracts
            .insert("governance".to_string(), imported_record(CONTRACT));
        manifest.save(path).expect("save manifest");
    }

    fn manifest_with_governance_and_vault(path: &std::path::Path) {
        let mut manifest = Manifest::new("testnet", None);
        manifest
            .contracts
            .insert("governance".to_string(), imported_record(CONTRACT));
        manifest
            .contracts
            .insert("vault".to_string(), imported_record(CONTRACT));
        manifest.save(path).expect("save manifest");
    }

    fn manifest_with_view_contracts(path: &std::path::Path) {
        let mut manifest = Manifest::new("testnet", None);
        for key in ["vault", "proxy_4626", "share_token", "blend_adapter_0"] {
            manifest
                .contracts
                .insert(key.to_string(), imported_record(CONTRACT));
        }
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

    fn uninitialized_record(contract_id: &str) -> ContractRecord {
        ContractRecord {
            initialized: false,
            ..imported_record(contract_id)
        }
    }

    fn write_fake_stack_wasms(root: &std::path::Path) {
        for artifact in [
            ArtifactName::Vault,
            ArtifactName::Governance,
            ArtifactName::ShareToken,
            ArtifactName::BlendAdapter,
            ArtifactName::CustodialAdapter,
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

    fn write_fake_custodial_wasm(root: &std::path::Path) {
        let path = ArtifactSpec::from_name(ArtifactName::CustodialAdapter).wasm_path(root);
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(path, "custodial").expect("write wasm");
    }
}
