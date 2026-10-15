//! Deploy a fresh governance contract for an existing vault, recorded as
//! *pending* so the active governance id in the manifest is only replaced
//! after the on-chain cutover is confirmed.

use anyhow::Context;

use crate::{
    artifacts::{ensure_uploaded, ArtifactSpec},
    cli::{ArtifactName, DeployGovernanceArgs},
    stellar::CommandExecutor,
};

use super::{
    super::{
        cutover::{extract_address, GOVERNANCE_PENDING_KEY, GOVERNANCE_PREVIOUS_KEY},
        inventory::{map_args, required_contract},
        output::Response,
    },
    session::{
        deploy_contract_if_needed, record_imported_contract_if_provided, ContractDeployment,
        DeploymentContext, InitializationState,
    },
};

pub(in crate::commands) fn deploy_governance<E: CommandExecutor>(
    deployment: &mut DeploymentContext<'_, '_, '_, E>,
    args: &DeployGovernanceArgs,
) -> anyhow::Result<Response> {
    let (context, manifest) = deployment.parts();
    let cli = context.cli();
    let stellar = context.stellar();
    if args.timelock_ns == 0 && !cli.allow_zero_timelock {
        anyhow::bail!("zero governance timelock requires --allow-zero-timelock");
    }
    record_imported_contract_if_provided(context, manifest, "vault", args.vault.as_ref())?;
    let vault = required_contract(manifest, "vault")?.to_string();
    if !args.force_new {
        if let Some(record) = manifest.contracts.get(GOVERNANCE_PENDING_KEY) {
            anyhow::ensure!(
                manifest
                    .contracts
                    .get("governance")
                    .is_none_or(|governance| governance.contract_id != record.contract_id),
                "governance_pending {} is already active; nothing to do",
                record.contract_id
            );
            return Ok(follow_up(&vault, &record.contract_id, &record.wasm_hash));
        }
    }
    let admin = if let Some(admin) = &args.admin {
        admin.to_string()
    } else {
        // No fallback to the signing key: silently deploying with the
        // deployer as admin can construct a different authority than the
        // vault's current governance carries.
        let current = required_contract(manifest, "governance")?;
        let observed = stellar
            .invoke_view(current, "admin", Vec::new())
            .with_context(|| {
                format!(
                    "cannot read admin() of current governance {current}; pass --admin explicitly"
                )
            })?;
        match extract_address(&observed.stdout) {
            Some(address) => address,
            None => anyhow::bail!(
                "current governance {current} admin() returned no address; pass --admin explicitly"
            ),
        }
    };
    anyhow::ensure!(
        admin != vault,
        "governance admin must not be the vault itself"
    );
    let wasm_hash = if let Some(hash) = &args.wasm_hash {
        hash.to_string()
    } else {
        let spec = ArtifactSpec::from_name(ArtifactName::Governance);
        let hash = ensure_uploaded(stellar, manifest, &cli.workspace_path, spec, args.build)?;
        context.checkpoint(manifest)?;
        hash
    };
    let pending = deploy_contract_if_needed(
        context,
        manifest,
        ContractDeployment {
            key: GOVERNANCE_PENDING_KEY,
            wasm_hash: &wasm_hash,
            constructor_args: vec![
                "--admin".to_string(),
                admin.clone(),
                "--vault".to_string(),
                vault.clone(),
                "--timelock_ns".to_string(),
                args.timelock_ns.to_string(),
            ],
            constructor_summary: map_args([
                ("admin", admin.as_str()),
                ("vault", vault.as_str()),
                ("timelock_ns", &args.timelock_ns.to_string()),
            ]),
            force_new: args.force_new,
            initialization: InitializationState::Complete,
        },
    )?;
    anyhow::ensure!(
        manifest
            .contracts
            .get("governance")
            .is_none_or(|record| record.contract_id != pending),
        "deployed governance {pending} is already the active governance"
    );
    if !cli.dry_run && !cli.json {
        eprintln!("deployed governance {pending} (pending) for vault {vault}");
    }
    Ok(follow_up(&vault, &pending, &wasm_hash))
}

fn follow_up(vault: &str, pending: &str, wasm_hash: &str) -> Response {
    let steps = [
        format!(
            "tmplr-soroban-vault governance submit-set-governance --admin <admin> \
             --new-governance {pending} --expected-wasm-hash {wasm_hash}"
        ),
        "governance accept the proposal once the timelock has elapsed, then".to_string(),
        "tmplr-soroban-vault upgrade confirm-governance-cutover".to_string(),
        format!("tmplr-soroban-vault deploy curator-proxy --governance {pending} --force-new"),
    ];
    Response::message(format!(
        "governance {pending} recorded as pending for vault {vault}; \
         the active governance id is unchanged until the cutover is confirmed.\n\
         manifest record: {GOVERNANCE_PENDING_KEY} (previous kept under {GOVERNANCE_PREVIOUS_KEY} on promotion)\n\
         next steps:\n{}",
        steps
            .iter()
            .map(|step| format!("  - {step}"))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}
