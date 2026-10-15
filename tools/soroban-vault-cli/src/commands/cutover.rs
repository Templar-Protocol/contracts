//! Governance cutover guard: verify a replacement governance contract on-chain
//! before any command hands it authority over the vault.

use anyhow::Context;

use crate::{
    cli::UpgradeCommand,
    manifest::Manifest,
    stellar::{CommandExecutor, Stellar},
    types::WasmHash,
};

use super::{context::CommandContext, inventory::required_contract, output::Response};

pub(super) const GOVERNANCE_PENDING_KEY: &str = "governance_pending";
pub(super) const GOVERNANCE_PREVIOUS_KEY: &str = "governance_previous";

/// Everything the cutover guard needs to make its decision. `None` means the
/// fact could not be read from the chain; the guard must treat that as a
/// failure, never as a pass.
#[derive(Debug, PartialEq, Eq)]
#[allow(
    clippy::struct_field_names,
    reason = "fields describe the same on-chain target"
)]
pub(super) struct CutoverFacts {
    pub(super) target_vault: Option<String>,
    pub(super) target_admin: Option<String>,
    pub(super) target_wasm_hash: Option<String>,
    pub(super) current_vault: Option<String>,
}

#[allow(
    clippy::struct_field_names,
    reason = "guard inputs are fixed by the check list"
)]
#[derive(Debug)]
pub(super) struct CutoverParams {
    pub(super) vault: String,
    pub(super) current_governance: String,
    pub(super) new_governance: String,
    /// The admin the target governance must report. The CLI callers resolve
    /// it from the pending record's constructor admin (confirm) or the
    /// submitting/expected admin (submit-set-governance) before constructing
    /// the params, so the guard always compares against a concrete value.
    pub(super) submitting_admin: String,
    pub(super) expected_wasm_hash: String,
}

/// Pure decision logic for the cutover guard. Returns the problems that block
/// submission; an empty list means the target governance is verified.
pub(super) fn cutover_problems(params: &CutoverParams, facts: &CutoverFacts) -> Vec<String> {
    let mut problems = Vec::new();
    if params.new_governance == params.current_governance {
        problems.push(format!(
            "target governance {} is already the vault's active governance",
            params.new_governance
        ));
    }
    if params.new_governance == params.vault {
        problems.push(format!(
            "target governance {} is the vault contract itself",
            params.new_governance
        ));
    }
    check_fact(
        &mut problems,
        "vault",
        facts.target_vault.as_deref(),
        &params.vault,
        || "run `governance pending` against the target contract first".to_string(),
    );

    // On-chain proof that the manifest's governance really controls the
    // manifest vault: the current governance's `vault()` binding must
    // match, or the manifest describes another vault than this governance
    // can act on.
    check_fact(
        &mut problems,
        "current-governance vault binding",
        facts.current_vault.as_deref(),
        &params.vault,
        || {
            format!(
                "read `vault()` on the current governance {} first",
                params.current_governance
            )
        },
    );

    check_fact(
        &mut problems,
        "admin",
        facts.target_admin.as_deref(),
        &params.submitting_admin,
        || {
            format!(
                "deploy governance with --admin <expected admin> for {}",
                params.new_governance
            )
        },
    );
    check_fact(
        &mut problems,
        "wasm hash",
        facts.target_wasm_hash.as_deref(),
        &params.expected_wasm_hash,
        || "re-deploy the governance contract from the pinned release WASM".to_string(),
    );
    problems
}

fn check_fact(
    problems: &mut Vec<String>,
    label: &str,
    observed: Option<&str>,
    expected: &str,
    manual_check: impl FnOnce() -> String,
) {
    let Some(observed) = observed else {
        problems.push(format!(
            "could not read the target governance {label} on-chain; manual check: {}",
            manual_check()
        ));
        return;
    };
    if observed != expected {
        problems.push(format!(
            "target governance {label} mismatch: observed {observed}, expected {expected}"
        ));
    }
}

/// Reads the target governance contract's `vault()` and `admin()` views and
/// its deployed WASM hash. Unreadable facts come back as `None`.
pub(super) fn target_governance_facts<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    target: &str,
) -> CutoverFacts {
    CutoverFacts {
        target_vault: view_address(stellar, target, "vault"),
        target_admin: view_address(stellar, target, "admin"),
        target_wasm_hash: stellar
            .fetch_contract_wasm_hash(target)
            .ok()
            .filter(|hash| !hash.is_empty()),
        current_vault: None,
    }
}

/// The cutover facts plus the vault the CURRENT governance is bound to on
/// chain; the guard fails closed when that binding is unreadable or names
/// another vault than the manifest records.
pub(super) fn cutover_facts<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    current_governance: &str,
    target: &str,
) -> CutoverFacts {
    CutoverFacts {
        current_vault: view_address(stellar, current_governance, "vault"),
        ..target_governance_facts(stellar, target)
    }
}

fn view_address<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    contract: &str,
    function: &str,
) -> Option<String> {
    let out = stellar.invoke_view(contract, function, Vec::new()).ok()?;
    extract_address(&out.stdout)
}

/// Pulls the first Soroban address (C…/G…) out of a CLI view response.
pub(super) fn extract_address(stdout: &str) -> Option<String> {
    let trimmed = stdout.trim().trim_matches('"').to_string();
    if trimmed.len() == 56 && (trimmed.starts_with('C') || trimmed.starts_with('G')) {
        return Some(trimmed);
    }
    None
}

/// Resolves the WASM hash the new governance is expected to carry: an
/// explicit flag wins; otherwise the pinned release governance SHA-256.
/// The hash recorded on the pending record is trusted only when it equals
/// the pin, so `deploy governance --wasm-hash` cannot silently arm the
/// guard with an unpinned build.
pub(super) fn resolve_expected_wasm_hash(
    manifest: &Manifest,
    expected: Option<&WasmHash>,
    new_governance: &str,
) -> String {
    let pin = crate::artifacts::ArtifactName::Governance.pin().sha256;
    if let Some(hash) = expected {
        if hash.as_str() != pin {
            eprintln!(
                "warning: governance {new_governance} carries non-pinned WASM hash {}; \
                 the cutover guard will expect this exact hash",
                hash.as_str()
            );
        }
        return hash.as_str().to_string();
    }
    if let Some(pending) = manifest
        .contracts
        .get(GOVERNANCE_PENDING_KEY)
        .filter(|pending| pending.contract_id == new_governance)
        .map(|pending| pending.wasm_hash.clone())
        .filter(|hash| hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
    {
        if pending != pin {
            eprintln!(
                "warning: pending governance {new_governance} was deployed with non-pinned \
                 WASM hash {pending}; the guard expects the pinned release hash. \
                 pass --expected-wasm-hash {pending} explicitly to override."
            );
        }
    }
    pin.to_string()
}

/// Guard shared by `governance submit-set-governance`. Returns `Ok(())` only
/// when the target governance is verified to belong to this vault.
pub(super) fn guard_set_governance<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    params: &CutoverParams,
    allow_unverified: bool,
) -> anyhow::Result<()> {
    if allow_unverified {
        eprintln!(
            "warning: submitting governance cutover without on-chain verification of {}",
            params.new_governance
        );
        return Ok(());
    }
    let facts = cutover_facts(stellar, &params.current_governance, &params.new_governance);
    let problems = cutover_problems(params, &facts);
    if problems.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "governance cutover guard refused to submit:\n{}\n\
         resolve the problems above or re-run with --allow-unverified-governance",
        problems
            .iter()
            .map(|problem| format!("  - {problem}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Applies the manifest half of the cutover after the on-chain proposal was
/// accepted: the pending governance becomes active, the previous one is
/// preserved for audit. Returns the promoted contract id.
pub(super) fn promote_pending_governance(manifest: &mut Manifest) -> anyhow::Result<String> {
    let pending = manifest
        .contracts
        .get(GOVERNANCE_PENDING_KEY)
        .with_context(|| format!("manifest has no {GOVERNANCE_PENDING_KEY} record"))?
        .clone();
    if let Some(current) = manifest.contracts.get("governance") {
        manifest
            .contracts
            .insert(GOVERNANCE_PREVIOUS_KEY.to_string(), current.clone());
    }
    manifest.contracts.remove(GOVERNANCE_PENDING_KEY);
    manifest
        .contracts
        .insert("governance".to_string(), pending.clone());
    Ok(pending.contract_id)
}

pub(super) fn run_upgrade<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &mut Manifest,
    command: &UpgradeCommand,
) -> anyhow::Result<Response> {
    match command {
        UpgradeCommand::Preflight(args) => {
            crate::commands::upgrade::run_preflight(context, manifest, args)
        }
        UpgradeCommand::ConfirmGovernanceCutover => confirm_governance_cutover(context, manifest),
    }
}

fn confirm_governance_cutover<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &mut Manifest,
) -> anyhow::Result<Response> {
    let pending = manifest
        .contracts
        .get(GOVERNANCE_PENDING_KEY)
        .cloned()
        .context("no pending governance recorded; run `deploy governance` first")?;
    let pending_id = pending.contract_id.clone();
    let vault = required_contract(manifest, "vault")?.to_string();
    if context.cli().dry_run {
        return Ok(Response::message(format!(
            "dry-run: would promote governance {pending_id} for vault {vault}"
        )));
    }
    let current_governance = manifest
        .contracts
        .get("governance")
        .map(|record| record.contract_id.clone())
        .unwrap_or_default();
    // Expected values come from the pending record `deploy governance`
    // wrote (admin it constructed, WASM hash it deployed), never from the
    // current governance: an admin rotated during the cutover or a
    // deliberately different `--admin`/`--wasm-hash` must still confirm.
    // The guard then proves the on-chain contract matches that record.
    let expected_admin = pending
        .constructor_args
        .get("admin")
        .cloned()
        .filter(|admin| !admin.is_empty())
        .with_context(|| {
            format!(
                "governance cutover cannot be confirmed: {GOVERNANCE_PENDING_KEY} records no valid constructor admin"
            )
        })?;
    anyhow::ensure!(
        pending.wasm_hash.len() == 64 && pending.wasm_hash.chars().all(|c| c.is_ascii_hexdigit()),
        "{GOVERNANCE_PENDING_KEY} records no valid wasm hash; cannot verify the deployed governance"
    );
    let expected_wasm_hash = pending.wasm_hash.to_ascii_lowercase();
    if expected_wasm_hash != crate::artifacts::ArtifactName::Governance.pin().sha256 {
        eprintln!(
            "warning: pending governance {pending_id} carries non-pinned WASM hash \
             {expected_wasm_hash}; confirming against the recorded hash"
        );
    }
    let params = CutoverParams {
        vault: vault.clone(),
        current_governance,
        new_governance: pending_id.clone(),
        submitting_admin: expected_admin,
        expected_wasm_hash,
    };
    let facts = cutover_facts(context.stellar(), &params.current_governance, &pending_id);
    let problems = cutover_problems(&params, &facts);
    anyhow::ensure!(
        problems.is_empty(),
        "governance cutover cannot be confirmed:\n{}",
        problems
            .iter()
            .map(|problem| format!("  - {problem}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    // The manifest may only be promoted once the on-chain SetGovernance has
    // executed: the vault itself must report the pending governance.
    match crate::commands::upgrade::read_vault_view(context.stellar(), &vault)
        .and_then(|facts| facts.governance_view)
    {
        Some(bound) if bound == pending_id => {}
        Some(bound) => anyhow::bail!(
            "vault {vault} is still bound to governance {bound}, not {pending_id}; accept the \
             set_governance proposal on the current governance before confirming the cutover"
        ),
        None => anyhow::bail!(
            "cannot read the governance binding of vault {vault} (proxy_view); refusing to \
             confirm the cutover (fail closed)"
        ),
    }
    anyhow::ensure!(
        manifest
            .contracts
            .get("governance")
            .is_none_or(|record| record.contract_id != pending_id),
        "governance {pending_id} is already the active governance"
    );
    let promoted = promote_pending_governance(manifest)?;
    context.checkpoint(manifest)?;
    Ok(Response::message(format!(
        "governance cutover confirmed: {promoted} is now active for vault {vault}\n\
         next: tmplr-soroban-vault deploy curator-proxy --governance {promoted} --force-new"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> CutoverParams {
        CutoverParams {
            vault: "CVAULT".to_string(),
            current_governance: "COLD".to_string(),
            new_governance: "CNEW".to_string(),
            submitting_admin: "GADMIN".to_string(),
            expected_wasm_hash: "ab".repeat(32),
        }
    }

    fn facts() -> CutoverFacts {
        CutoverFacts {
            target_vault: Some("CVAULT".to_string()),
            target_admin: Some("GADMIN".to_string()),
            target_wasm_hash: Some("ab".repeat(32)),
            current_vault: Some("CVAULT".to_string()),
        }
    }

    #[test]
    fn verified_target_passes_the_guard() {
        assert!(cutover_problems(&params(), &facts()).is_empty());
    }

    #[test]
    fn unreadable_facts_fail_closed() {
        let problems = cutover_problems(
            &params(),
            &CutoverFacts {
                target_vault: None,
                target_admin: None,
                target_wasm_hash: None,
                current_vault: None,
            },
        );
        assert_eq!(problems.len(), 4);
        assert!(problems[0].contains("manual check"));
    }

    #[test]
    fn mismatched_facts_are_rejected() {
        let problems = cutover_problems(
            &params(),
            &CutoverFacts {
                target_vault: Some("COTHER".to_string()),
                target_admin: Some("GOTHER".to_string()),
                target_wasm_hash: Some("cd".repeat(32)),
                current_vault: Some("CVAULT".to_string()),
            },
        );
        assert_eq!(problems.len(), 3);
        assert!(problems.iter().all(|p| p.contains("mismatch")));
    }

    #[test]
    fn targeting_the_current_governance_or_the_vault_is_rejected() {
        let guard = CutoverParams {
            new_governance: "COLD".to_string(),
            ..params()
        };
        assert!(cutover_problems(&guard, &facts())
            .iter()
            .any(|p| p.contains("already the vault's active governance")));
        let guard = CutoverParams {
            new_governance: "CVAULT".to_string(),
            ..params()
        };
        assert!(cutover_problems(&guard, &facts())
            .iter()
            .any(|p| p.contains("vault contract itself")));
    }

    #[test]
    fn pending_record_with_unpinned_hash_falls_back_to_the_release_pin() {
        let mut manifest = Manifest::new("testnet", None);
        manifest.contracts.insert(
            GOVERNANCE_PENDING_KEY.to_string(),
            crate::manifest::ContractRecord {
                contract_id: "CNEW".to_string(),
                wasm_hash: "ab".repeat(32),
                salt: None,
                constructor_args: std::collections::BTreeMap::new(),
                deploy_tx: None,
                initialized: true,
            },
        );
        let pin = crate::artifacts::ArtifactName::Governance
            .pin()
            .sha256
            .to_string();
        assert_ne!(
            pin,
            "ab".repeat(32),
            "fixture hash must differ from the pin"
        );
        // A pending record with a non-pinned hash must not arm the guard.
        assert_eq!(resolve_expected_wasm_hash(&manifest, None, "CNEW"), pin);
        // An explicit flag always wins.
        assert_eq!(
            resolve_expected_wasm_hash(
                &manifest,
                Some(&"ab".repeat(32).parse().expect("hash")),
                "CNEW"
            ),
            "ab".repeat(32)
        );
    }

    #[test]
    fn promotion_preserves_previous_and_clears_pending() {
        let mut manifest = Manifest::new("testnet", None);
        manifest.contracts.insert(
            "governance".to_string(),
            crate::manifest::ContractRecord {
                contract_id: "COLD".to_string(),
                wasm_hash: "cd".repeat(32),
                salt: None,
                constructor_args: std::collections::BTreeMap::new(),
                deploy_tx: None,
                initialized: true,
            },
        );
        manifest.contracts.insert(
            GOVERNANCE_PENDING_KEY.to_string(),
            crate::manifest::ContractRecord {
                contract_id: "CNEW".to_string(),
                wasm_hash: "ab".repeat(32),
                salt: None,
                constructor_args: std::collections::BTreeMap::new(),
                deploy_tx: None,
                initialized: true,
            },
        );
        let promoted = promote_pending_governance(&mut manifest).expect("promote");
        assert_eq!(promoted, "CNEW");
        assert_eq!(
            manifest.contracts["governance"].contract_id, "CNEW",
            "active governance must be the promoted one"
        );
        assert_eq!(
            manifest.contracts[GOVERNANCE_PREVIOUS_KEY].contract_id, "COLD",
            "previous governance must be preserved for audit"
        );
        assert!(
            !manifest.contracts.contains_key(GOVERNANCE_PENDING_KEY),
            "pending record must be consumed"
        );
    }

    #[test]
    fn extract_address_reads_plain_and_quoted_output() {
        assert_eq!(
            extract_address("CABCDEF\n").as_deref(),
            None,
            "short strings are not addresses"
        );
        let addr = format!("C{}", "A".repeat(55));
        assert_eq!(extract_address(&addr).as_deref(), Some(addr.as_str()));
        assert_eq!(
            extract_address(&format!("\"{addr}\"")).as_deref(),
            Some(addr.as_str())
        );
        let g = format!("G{}", "B".repeat(55));
        assert_eq!(extract_address(&g).as_deref(), Some(g.as_str()));
    }
}
