//! Manifest audit records for successful write commands.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    artifacts::ArtifactSpec,
    cli::{Cli, Commands, DeployCommand, UserCommand},
    manifest::{Manifest, TransactionRecord},
    types::SourceAccount,
};

use super::{
    inventory::{contract_id, selected_blend_adapter},
    output::Response,
};

pub(super) fn transaction_record(
    cli: &Cli,
    manifest: &Manifest,
    response: &Response,
) -> TransactionRecord {
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

pub(super) fn command_name(command: &Commands) -> String {
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

pub(super) fn command_target_and_function(
    command: &Commands,
    manifest: &Manifest,
) -> (Option<String>, Option<String>) {
    match command {
        Commands::User(args) => {
            let (target, function) = match &args.command {
                UserCommand::Deposit { .. } => {
                    let proxy = contract_id(manifest, "proxy_4626");
                    (
                        proxy.or_else(|| contract_id(manifest, "vault")),
                        if proxy.is_some() {
                            "deposit_with_min"
                        } else {
                            "execute"
                        },
                    )
                }
                UserCommand::Mint { .. } => (contract_id(manifest, "proxy_4626"), "mint"),
                UserCommand::Withdraw { .. }
                | UserCommand::Redeem { .. }
                | UserCommand::RequestWithdraw { .. } => {
                    (contract_id(manifest, "vault"), "execute")
                }
                UserCommand::ExecuteWithdraw { .. } => (
                    contract_id(manifest, "proxy_4626").or_else(|| contract_id(manifest, "vault")),
                    if contract_id(manifest, "proxy_4626").is_some() {
                        "execute_withdraw"
                    } else {
                        "execute"
                    },
                ),
                UserCommand::Balance { .. }
                | UserCommand::Preview { .. }
                | UserCommand::View { .. } => (None, "view"),
            };
            (target.map(ToString::to_string), Some(function.to_string()))
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

pub(super) fn command_artifact_hash(command: &Commands, manifest: &Manifest) -> Option<String> {
    let Commands::Deploy(args) = command else {
        return None;
    };
    match &args.command {
        DeployCommand::Wasm(wasm) => manifest
            .artifacts
            .get(ArtifactSpec::from_name(wasm.artifact).key)
            .and_then(|record| record.remote_wasm_hash.clone()),
        DeployCommand::CuratorProxy(_) => manifest
            .artifacts
            .get(ArtifactSpec::from_name(crate::cli::ArtifactName::CuratorProxy).key)
            .and_then(|record| record.remote_wasm_hash.clone()),
        DeployCommand::Stack(_)
        | DeployCommand::Resume(_)
        | DeployCommand::Adapters(_)
        | DeployCommand::Plan(_)
        | DeployCommand::Repair(_) => None,
    }
}
