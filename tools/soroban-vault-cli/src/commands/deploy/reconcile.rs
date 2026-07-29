//! Chain-to-manifest reconciliation, repair classification, and wiring verification.

use std::collections::BTreeSet;

use anyhow::Context;

use crate::{
    cli::ReconcileArgs,
    manifest::{ContractRecord, Manifest},
    stellar::{CommandExecutor, Stellar},
};

use super::super::{
    context::CommandContext,
    inventory::{args, contract_id, is_custodial_adapter_key},
    output::{
        ReconcileComponent, ReconcileResponse, ReconcileStatus, Response, WiringCheck, WiringStatus,
    },
    CURATOR_PROXY_VERSION_DISCOVERY_ARG,
};

pub(in crate::commands) fn run_reconcile<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &Manifest,
    args: &ReconcileArgs,
) -> Response {
    let stellar = context.stellar();
    Response::Reconcile(reconcile_manifest(
        stellar,
        manifest,
        !args.skip_view_verification,
    ))
}

pub(in crate::commands) fn reconcile_manifest<E: CommandExecutor>(
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
pub(in crate::commands) fn reconcile_component<E: CommandExecutor>(
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

    let is_stellar_asset_contract = record.wasm_hash == "stellar-asset-contract";
    if is_stellar_asset_contract {
        let Some(asset) = record
            .constructor_args
            .get("asset")
            .map(String::as_str)
            .filter(|asset| !asset.is_empty())
        else {
            component.status = ReconcileStatus::Unknown;
            component.warnings.push(
                "stellar asset contract record has no canonical asset descriptor".to_string(),
            );
            component.repair_actions.push(format!(
                "{key}: record verified asset provenance before resuming"
            ));
            return component;
        };
        if asset != "predeployed" {
            let expected_contract_id = match stellar.asset_contract_id(asset) {
                Ok(contract_id) => contract_id,
                Err(error) => {
                    component.status = ReconcileStatus::Unknown;
                    component.warnings.push(format!(
                        "could not derive the Stellar asset contract id for {asset}: {error}"
                    ));
                    component.repair_actions.push(format!(
                        "{key}: verify network/RPC and asset provenance before resuming"
                    ));
                    return component;
                }
            };
            if record.contract_id != expected_contract_id {
                component.status = ReconcileStatus::Mismatched;
                component.warnings.push(format!(
                    "recorded contract {} does not match deterministic Stellar asset contract {expected_contract_id} for asset {asset}",
                    record.contract_id
                ));
                component.repair_actions.push(format!(
                    "{key}: replace the wrong asset contract record before resuming"
                ));
                return component;
            }
        }
    }

    let chain_hash = if is_stellar_asset_contract {
        stellar
            .invoke_view(&record.contract_id, "decimals", Vec::new())
            .map(|_| None)
    } else {
        stellar
            .fetch_contract_wasm_hash(&record.contract_id)
            .map(Some)
    };

    match chain_hash {
        Ok(chain_hash) => {
            if let Some(chain_hash) = chain_hash {
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
                } else if wiring
                    .iter()
                    .any(|check| check.status == WiringStatus::Match)
                {
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

pub(in crate::commands) fn apply_reconcile_safe_manifest_updates<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
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
        context.checkpoint(manifest)?;
    }
    Ok(())
}

pub(in crate::commands) fn should_compare_wasm_hash(wasm_hash: &str) -> bool {
    !matches!(wasm_hash, "predeployed" | "stellar-asset-contract")
}

pub(in crate::commands) fn looks_missing_contract(message: &str) -> bool {
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
pub(in crate::commands) fn verify_component_wiring<E: CommandExecutor>(
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
            if curator_proxy_supports_version_discovery(record) {
                checks.push(curator_proxy_version_check(stellar, &record.contract_id)?);
            }
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
        key if is_custodial_adapter_key(key) => {
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

pub(in crate::commands) fn curator_proxy_supports_version_discovery(
    record: &ContractRecord,
) -> bool {
    record
        .constructor_args
        .get(CURATOR_PROXY_VERSION_DISCOVERY_ARG)
        .is_some_and(|value| value == "true")
}

pub(in crate::commands) fn curator_proxy_needs_version_verification(
    record: &ContractRecord,
    current_wasm_hash: &str,
) -> bool {
    record.wasm_hash == current_wasm_hash && !curator_proxy_supports_version_discovery(record)
}

pub(in crate::commands) fn curator_proxy_version_check<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    contract_id: &str,
) -> anyhow::Result<WiringCheck> {
    let output = stellar.invoke_view(contract_id, "vault_version", Vec::new())?;
    let matched = !output.stdout.trim().is_empty();
    Ok(WiringCheck {
        field: "vault_version".to_string(),
        expected: Some("successful non-empty response".to_string()),
        observed: Some(output.stdout),
        status: if matched {
            WiringStatus::Match
        } else {
            WiringStatus::Mismatch
        },
    })
}

pub(in crate::commands) fn view_equals_check<E: CommandExecutor>(
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

pub(in crate::commands) fn push_contains_check(
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
