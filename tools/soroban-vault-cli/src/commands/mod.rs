//! Application-layer command dispatch for the Soroban vault CLI.

mod adapter;
mod audit;
mod context;
mod curator;
mod deploy;
mod doctor;
mod governance;
mod inventory;
mod invoke;
mod output;
mod safety;
mod share_token;
mod ttl;
mod user;
mod vault_ops;

use std::io;

use clap::CommandFactory;
use tracing::{debug, info};

use crate::{
    cli::{Cli, Commands, ProfileCommand},
    profile,
    stellar::CommandExecutor,
};

use adapter::run_adapter;
use audit::{command_name, transaction_record};
use context::CommandContext;
use curator::run_curator;
use deploy::run_reconcile;
use doctor::run_doctor;
use governance::run_governance;
use inventory::{export_env, status_response};
pub use output::{print_error, print_parse_error};
use output::{print_response, Response};
use safety::{confirm_dangerous_governance_change, guard_fresh_state_usage, guard_write};
use share_token::run_share_token;
use ttl::run_extend_ttl;
use user::run_user;

const CONTRACT_TTL_EXTEND_LEDGERS: u32 = 3_110_400;
const CURATOR_PROXY_INITIALIZATION_AUTHORITY_ARG: &str = "initialization_authority";
const CURATOR_PROXY_INITIALIZER_ARG: &str = "initializer";
const CURATOR_PROXY_VERSION_DISCOVERY_ARG: &str = "version_discovery";
const CURATOR_PROXY_VAULT_ARG: &str = "vault_address";
const CURATOR_PROXY_GOVERNANCE_ARG: &str = "governance_address";
const CURATOR_PROXY_LEGACY_V1_HASH_ARG: &str = "legacy_v1_wasm_hash";

pub fn run<E: CommandExecutor>(cli: &Cli, executor: &E) -> anyhow::Result<()> {
    guard_write(cli)?;
    guard_fresh_state_usage(cli)?;
    let context = CommandContext::new(cli, executor);
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
        let result = run_doctor(&context);
        return print_response(&result, cli);
    }
    let mut manifest = context.load_manifest()?;
    confirm_dangerous_governance_change(&context, &manifest)?;
    let result = match &cli.command {
        Commands::Doctor => unreachable!("doctor returns before manifest load"),
        Commands::Profile(_) | Commands::Completions { .. } | Commands::Man => {
            unreachable!("handled before manifest load")
        }
        Commands::Deploy(args) => deploy::run(&context, &mut manifest, args),
        Commands::Reconcile(args) => Ok(run_reconcile(&context, &manifest, args)),
        Commands::User(args) => run_user(&context, &manifest, &args.command),
        Commands::Curator(args) => run_curator(&context, &manifest, &args.command),
        Commands::Governance(args) => run_governance(&context, &manifest, &args.command),
        Commands::ShareToken(args) => run_share_token(&context, &manifest, &args.command),
        Commands::Adapter(args) => run_adapter(&context, &manifest, args),
        Commands::ExtendTtl(args) => run_extend_ttl(&context, &manifest, args),
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

#[cfg(test)]
#[path = "test_support.rs"]
mod tests;
