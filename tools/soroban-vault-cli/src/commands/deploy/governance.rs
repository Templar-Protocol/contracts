//! Deploy a fresh governance contract for an existing vault, recorded as
//! *pending* so the active governance id in the manifest is only replaced
//! after the on-chain cutover is confirmed.

use anyhow::Context;

use crate::{
    artifacts::{ensure_uploaded, ArtifactSpec},
    cli::{ArtifactName, DeployGovernanceArgs},
    manifest::Manifest,
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

/// Cheap input checks both the real deployment and the dry-run plan
/// share, so a dry-run never promises a plan the real path would reject.
pub(in crate::commands) fn validate_governance_dry_run(
    cli: &crate::cli::Cli,
    manifest: &Manifest,
    args: &DeployGovernanceArgs,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        args.timelock_ns != 0 || cli.allow_zero_timelock,
        "zero governance timelock requires --allow-zero-timelock"
    );
    let vault = args
        .vault
        .as_ref()
        .map(ToString::to_string)
        .or_else(|| {
            manifest
                .contracts
                .get("vault")
                .map(|record| record.contract_id.clone())
        })
        .context("deploy governance requires an existing vault (manifest record or --vault)")?;
    if let Some(admin) = &args.admin {
        anyhow::ensure!(
            admin.as_str() != vault,
            "governance admin must not be the vault itself"
        );
    }
    Ok(())
}

pub(in crate::commands) fn deploy_governance<E: CommandExecutor>(
    deployment: &mut DeploymentContext<'_, '_, '_, E>,
    args: &DeployGovernanceArgs,
) -> anyhow::Result<Response> {
    let (context, manifest) = deployment.parts();
    let cli = context.cli();
    let stellar = context.stellar();
    validate_governance_dry_run(cli, manifest, args)?;
    record_imported_contract_if_provided(context, manifest, "vault", args.vault.as_ref())?;
    let vault = required_contract(manifest, "vault")?.to_string();
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
            anyhow::ensure!(
                pending_matches_request(record, &vault, &admin, &args.timelock_ns.to_string()),
                "governance_pending {} was deployed with different vault/admin/timelock than \
                 requested (vault {vault}, admin {admin}, timelock_ns {}); pass --force-new to \
                 deploy a fresh governance",
                record.contract_id,
                args.timelock_ns
            );
            return Ok(follow_up(&vault, &record.contract_id, &record.wasm_hash));
        }
    }
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

/// An existing pending record is only reusable when its constructor args are
/// exactly what this request would deploy.
fn pending_matches_request(
    record: &crate::manifest::ContractRecord,
    vault: &str,
    admin: &str,
    timelock_ns: &str,
) -> bool {
    let recorded = |key: &str| record.constructor_args.get(key).map(String::as_str);
    recorded("vault") == Some(vault)
        && recorded("admin") == Some(admin)
        && recorded("timelock_ns") == Some(timelock_ns)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ContractRecord;

    #[test]
    fn pending_record_is_reused_only_on_exact_match() {
        let record = ContractRecord {
            contract_id: "CPENDING".to_string(),
            wasm_hash: "ab".repeat(32),
            salt: None,
            deploy_tx: None,
            constructor_args: map_args([("admin", "GA"), ("vault", "CV"), ("timelock_ns", "5")]),
            initialized: true,
        };
        assert!(pending_matches_request(&record, "CV", "GA", "5"));
        assert!(!pending_matches_request(&record, "CV2", "GA", "5"));
        assert!(!pending_matches_request(&record, "CV", "GB", "5"));
        assert!(!pending_matches_request(&record, "CV", "GA", "6"));
    }
}
