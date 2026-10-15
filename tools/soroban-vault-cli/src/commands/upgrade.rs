//! Read-only pre-upgrade safety checks for an existing vault.
//!
//! The runtime contract only exposes `proxy_view`, `version`, and
//! `execute` as public entrypoints on every supported version (v1.1.1,
//! v1.2.0, and dev), so any check that needs more detail reads the
//! instance storage entries the contract itself keeps (verified stable
//! across those versions) via `stellar contract read` + `stellar xdr
//! decode`, and reports `unverifiable` with the manual fallback when even
//! that is not possible.

use serde_json::Value;

use anyhow::Context;

use crate::{
    cli::UpgradePreflightArgs,
    manifest::Manifest,
    stellar::{CommandExecutor, Stellar},
};

use super::{
    context::CommandContext,
    inventory::args,
    output::{
        PreflightFinding, PreflightReport, PreflightStatus, Response, UpgradePreflightResponse,
    },
};

pub(super) const KERNEL_ADDRESS_DOMAIN: &[u8] = b"templar:soroban:address";
pub(super) const KERNEL_ADDRESS_VERSION: u8 = 1;
/// Governance's `PROPOSAL_EXECUTION_WINDOW_NS`: a proposal older than this
/// past its ready time is ProposalExpired on governance that enforces it.
pub(super) const PROPOSAL_EXECUTION_WINDOW_NS: u128 = 7 * 24 * 60 * 60 * 1_000_000_000;
pub(super) const INSTANCE_KEY_XDR: &str = "\"ledger_key_contract_instance\"";

/// The on-chain facts the preflight decision needs. `None` always means
/// "could not verify", never "checked and clean".
#[derive(Debug, Default)]
pub(super) struct VaultFacts {
    pub(super) vault_id: String,
    pub(super) asset_token_id: Option<String>,
    pub(super) share_token_id: Option<String>,
    pub(super) runtime_version: Option<String>,
    pub(super) governance_view: Option<String>,
    pub(super) governance_manifest: Option<String>,
    pub(super) paused: Option<bool>,
    pub(super) virtual_assets: Option<i128>,
    pub(super) external_assets_present: Option<bool>,
    pub(super) max_deposit_allowed: Option<i128>,
    pub(super) migration_flag: Option<bool>,
    pub(super) fee_recipients: Option<[(u128, String); 2]>,
    pub(super) adapter_bindings: Option<Vec<(u32, String)>>,
    pub(super) pending_proposals: Option<Vec<u64>>,
    pub(super) stale_proposals: Option<u64>,
    pub(super) attest_empty_withdrawal_queue: bool,
}

/// Pure preflight decision logic: everything needed is already in
/// `facts`, so the classification can be unit-tested without a chain.
pub(super) fn evaluate_vault_preflight(facts: &VaultFacts) -> PreflightReport {
    let mut findings = Vec::new();
    findings.push(finding(
        "runtime_version",
        match &facts.runtime_version {
            Some(version) => (PreflightStatus::Pass, format!("runtime reports {version}")),
            None => (
                PreflightStatus::Unverifiable,
                "version view unavailable".to_string(),
            ),
        },
    ));
    findings.push(match facts.migration_flag {
        Some(true) => blocking(
            "migration_flag",
            "a migration is already in progress on this vault; finish or roll it back before upgrading",
        ),
        Some(false) => pass(
            "migration_flag",
            "no migration is flagged on this vault",
        ),
        None => unverifiable(
            "migration_flag",
            "no view exposes the migration flag; manual check: `stellar contract read --id <vault> --key-xdr \"\\\"ledger_key_contract_instance\\\"\"` and inspect the `migrate` instance entry",
        ),
    });
    findings.push(fee_recipient_finding(facts));
    findings.push(withdrawal_queue_finding(facts));
    findings.push(op_state_finding(facts));
    findings.push(adapter_binding_finding(facts));
    findings.push(external_assets_finding(facts));
    findings.push(governance_wiring_finding(facts));
    findings.push(governance_queue_finding(facts));
    let blocking = findings
        .iter()
        .any(|finding| finding.status == PreflightStatus::Blocking);
    let unverifiable = findings
        .iter()
        .any(|finding| finding.status == PreflightStatus::Unverifiable);
    PreflightReport {
        runtime_version: facts
            .runtime_version
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        findings,
        ok: !blocking && !unverifiable,
    }
}

fn fee_recipient_finding(facts: &VaultFacts) -> PreflightFinding {
    match &facts.fee_recipients {
        None => unverifiable(
            "fee_recipients",
            "fee recipient storage unreadable; manual check: `stellar contract read` the `fees` instance entry and compare recipient addresses against the vault, asset token, and share token",
        ),

        Some(recipients) => {
            let mut problems = Vec::new();
            for (slot, (fee, recipient)) in recipients.iter().enumerate() {
                let label = if slot == 0 { "performance" } else { "management" };
                if fee == &0 {
                    continue;
                }
                for banned in [
                    "vault",
                    "asset_token",
                    "share_token",
                ]
                .into_iter()
                .zip(banned_recipients(facts))
                {
                    if banned.1 == *recipient {
                        problems.push(format!(
                            "{label} fee recipient {recipient} is the {} (FIND-009 would reject the upgrade)",
                            banned.0
                        ));
                    }
                }
            }
            if problems.is_empty() {
                pass(
                    "fee_recipients",
                    "neither fee recipient is the vault, asset token, or share token",
                )
            } else {
                blocking("fee_recipients", problems.join("; "))
            }
        }
    }
}

fn banned_recipients(facts: &VaultFacts) -> Vec<String> {
    std::iter::once(kernel_address(&facts.vault_id))
        .chain(
            facts
                .asset_token_id
                .as_deref()
                .into_iter()
                .chain(facts.share_token_id.as_deref())
                .map(kernel_address),
        )
        .collect()
}

fn withdrawal_queue_finding(facts: &VaultFacts) -> PreflightFinding {
    match facts.virtual_assets {
        None => unverifiable(
            "withdrawal_queue",
            "virtual assets unknown; manual check: read the `vassets` instance entry and the `withdraw_queue` storage, then `normalize_migrated_state` would fail on a non-empty queue",
        ),
        Some(0) => pass(
            "withdrawal_queue",
            "virtual assets are zero, so normalize_migrated_state cannot be blocked by pending withdrawals",
        ),
        Some(_) if facts.attest_empty_withdrawal_queue => unverifiable(
            "withdrawal_queue",
            "operator attested the withdrawal queue is empty but the check cannot confirm it from the exposed views",
        ),
        Some(_) => unverifiable(
            "withdrawal_queue",
            "virtual assets are non-zero and no view exposes the withdrawal queue; manual check: `stellar contract read` the vault instance and persistent entries for `withdraw_queue`",
        ),
    }
}

fn op_state_finding(facts: &VaultFacts) -> PreflightFinding {
    match (facts.paused, facts.max_deposit_allowed) {
        (Some(true), _) => unverifiable(
            "op_state_idle",
            "the vault is paused, so deposit limits cannot prove the operator state; manual check: wait for the unpause or read the operator state blob",
        ),
        (Some(false), Some(0)) => blocking(
            "op_state_idle",
            "deposits are capped to zero while unpaused, which means the operator state is not idle",
        ),
        (Some(false), Some(_)) => pass(
            "op_state_idle",
            "deposits are allowed, so the operator state is idle",
        ),
        (Some(false), None) => unverifiable(
            "op_state_idle",
            "deposit limit view unavailable; manual check: proxy_view max_deposit must be non-zero",
        ),
        (None, _) => unverifiable(
            "op_state_idle",
            "proxy_view unavailable; manual check: proxy_view(max_deposit) and paused flags",
        ),
    }
}

fn adapter_binding_finding(facts: &VaultFacts) -> PreflightFinding {
    match &facts.adapter_bindings {
        None => unverifiable(
            "adapter_bindings",
            "adapter map unreadable; manual check: read the `adapmap` instance entry and confirm no adapter address appears under two market ids",
        ),
        Some(bindings) => {
            let duplicates = duplicate_adapters(bindings);
            if duplicates.is_empty() {
                pass(
                    "adapter_bindings",
                    format!("{} adapter bindings, no adapter bound to two markets", bindings.len()),
                )
            } else {
                blocking(
                    "adapter_bindings",
                    format!(
                        "adapter(s) {} bound to multiple markets (FIND-019); the upgrade must de-duplicate first",
                        duplicates.join(", ")
                    ),
                )
            }
        }
    }
}

pub(super) fn duplicate_adapters(bindings: &[(u32, String)]) -> Vec<String> {
    let mut seen: std::collections::BTreeMap<&String, u32> = std::collections::BTreeMap::new();
    let mut duplicates = Vec::new();
    for (market, adapter) in bindings {
        if let Some(previous) = seen.insert(adapter, *market) {
            let _ = previous;
            if !duplicates.contains(adapter) {
                duplicates.push(adapter.clone());
            }
        }
    }
    duplicates
}

fn external_assets_finding(facts: &VaultFacts) -> PreflightFinding {
    match facts.external_assets_present {
        Some(false) => pass(
            "external_assets",
            "no external assets are marked present",
        ),
        Some(true) => PreflightFinding {
            check: "external_assets".to_string(),
            status: PreflightStatus::Warn,
            detail: "external assets are present: deposits stay blocked until a full RefreshMarkets \
                     covers every market after the upgrade"
                .to_string(),
        },
        None => unverifiable(
            "external_assets",
            "proxy_view external-asset slot unavailable; manual check: proxy_view position 3 element 5",
        ),
    }
}

fn governance_wiring_finding(facts: &VaultFacts) -> PreflightFinding {
    match (&facts.governance_view, &facts.governance_manifest) {
        (Some(view), Some(manifest)) if view == manifest => pass(
            "governance_wiring",
            format!("manifest governance {manifest} matches the on-chain governance binding"),
        ),
        (Some(view), Some(manifest)) => blocking(
            "governance_wiring",
            format!("manifest governance {manifest} does not match on-chain governance {view}"),
        ),
        _ => unverifiable(
            "governance_wiring",
            "the vault's governance binding is unknown; manual check: proxy_view position 0 element 1",
        ),
    }
}

fn governance_queue_finding(facts: &VaultFacts) -> PreflightFinding {
    match (&facts.pending_proposals, facts.stale_proposals) {
        (Some(pending), _) if pending.is_empty() => pass(
            "governance_queue",
            "no proposals are pending on the active governance contract",
        ),
        (Some(pending), Some(0)) => pass(
            "governance_queue",
            format!(
                "{} proposal(s) pending, none past the 7-day execution window",
                pending.len()
            ),
        ),
        (Some(pending), Some(stale)) => PreflightFinding {
            check: "governance_queue".to_string(),
            status: PreflightStatus::Warn,
            detail: format!(
                "{stale} of {} pending proposal(s) are older than the 7-day execution window and \
                 will be ProposalExpired on governance that enforces it; accept, revoke, or \
                 re-submit them before the cutover",
                pending.len()
            ),
        },
        (Some(pending), None) => unverifiable(
            "governance_queue",
            format!(
                "{} proposal(s) pending but their ready times are unreadable",
                pending.len()
            ),
        ),
        (None, _) => unverifiable(
            "governance_queue",
            "the governance queue is unreadable; manual check: governance pending_ids",
        ),
    }
}

fn finding(check: &str, (status, detail): (PreflightStatus, String)) -> PreflightFinding {
    PreflightFinding {
        check: check.to_string(),
        status,
        detail,
    }
}

fn pass(check: &str, detail: impl Into<String>) -> PreflightFinding {
    finding(check, (PreflightStatus::Pass, detail.into()))
}

fn blocking(check: &str, detail: impl Into<String>) -> PreflightFinding {
    finding(check, (PreflightStatus::Blocking, detail.into()))
}

fn unverifiable(check: &str, detail: impl Into<String>) -> PreflightFinding {
    finding(check, (PreflightStatus::Unverifiable, detail.into()))
}

/// Reads every fact the preflight decision needs. Individual reads that
/// fail stay `None`; only a vault that cannot be reached at all aborts.
pub(super) fn collect_vault_facts<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    vault: &str,
    attest_empty_withdrawal_queue: bool,
) -> VaultFacts {
    let mut facts = VaultFacts {
        vault_id: vault.to_string(),
        attest_empty_withdrawal_queue,
        ..VaultFacts::default()
    };
    facts.governance_manifest = manifest
        .contracts
        .get("governance")
        .map(|record| record.contract_id.clone());
    facts.asset_token_id = manifest
        .contracts
        .get("asset_token")
        .map(|record| record.contract_id.clone());
    facts.share_token_id = manifest
        .contracts
        .get("share_token")
        .map(|record| record.contract_id.clone());

    let version = stellar
        .invoke_view(vault, "version", Vec::new())
        .map(|out| out.stdout.trim().trim_matches('"').to_string())
        .ok();
    facts.runtime_version = version.filter(|v| !v.is_empty());

    if let Ok(view) = stellar.invoke_view(
        vault,
        "proxy_view",
        args([("--owner", vault), ("--assets", "0"), ("--shares", "0")]),
    ) {
        parse_proxy_view(&view.stdout, &mut facts);
    }

    if let Some(entries) = read_instance_storage(stellar, vault) {
        facts.migration_flag = entries.get("migrate").and_then(|v| v.as_bool());
        facts.fee_recipients = entries
            .get("fees")
            .and_then(|v| v.as_str().and_then(decode_fee_recipients));
        facts.adapter_bindings = entries.get("adapmap").and_then(parse_adapter_map);
    }

    if let Some(governance) = facts.governance_view.clone() {
        if let Ok(pending) = stellar.invoke_view(&governance, "pending_ids", Vec::new()) {
            let ids = parse_pending_ids(&pending.stdout);
            facts.stale_proposals =
                ids.iter()
                    .map(|id| {
                        stellar
                            .invoke_view(
                                &governance,
                                "pending",
                                args([("--proposal_id", &id.to_string())]),
                            )
                            .ok()
                            .and_then(|out| {
                                serde_json::from_str::<Value>(&out.stdout).ok().and_then(
                                    |proposal| proposal.get("valid_after_ns").and_then(json_u128),
                                )
                            })
                    })
                    .collect::<Option<Vec<u128>>>()
                    .map(|valid_after| {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos();
                        u64::try_from(
                            valid_after
                                .into_iter()
                                .filter(|ready| now > *ready + PROPOSAL_EXECUTION_WINDOW_NS)
                                .count(),
                        )
                        .unwrap_or(u64::MAX)
                    });
            facts.pending_proposals = Some(ids);
        }
    }
    facts
}

pub(super) fn read_instance_storage<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    vault: &str,
) -> Option<serde_json::Map<String, Value>> {
    let out = stellar
        .run(
            [
                vec!["contract".to_string(), "read".to_string()],
                vec!["--contract".to_string(), vault.to_string()],
                vec!["--key-xdr".to_string(), INSTANCE_KEY_XDR.to_string()],
                stellar.network_args(),
            ]
            .concat(),
            &[],
            stellar.source_env(),
        )
        .ok()?;
    let encoded = out.stdout.trim();
    // `contract read` emits CSV; the value column holds base64 XDR.
    let encoded = encoded.split(',').next_back()?.trim_matches('"');
    let decoded = stellar
        .run(
            vec![
                "xdr".to_string(),
                "decode".to_string(),
                "--type".to_string(),
                "ScVal".to_string(),
                "--input".to_string(),
                "single-base64".to_string(),
                "--output".to_string(),
                "json".to_string(),
                encoded.to_string(),
            ],
            &[],
            Vec::new(),
        )
        .ok()?;
    parse_instance_storage(&decoded.stdout)
}

pub(super) fn parse_instance_storage(stdout: &str) -> Option<serde_json::Map<String, Value>> {
    let value: Value = serde_json::from_str(stdout).ok()?;
    let entries = value.get("contract_instance")?.get("storage")?.as_array()?;
    let mut map = serde_json::Map::new();
    for entry in entries {
        let key = entry.get("key")?.get("symbol")?.as_str()?;
        map.insert(key.to_string(), entry.get("val")?.clone());
    }
    // The `init` symbol is written by `initialize` on every supported
    // version; its absence means we are not looking at this vault's
    // instance storage, so report the read as unavailable.
    map.contains_key("init").then_some(map)
}

pub(super) fn decode_fee_recipients(encoded: &str) -> Option<[(u128, String); 2]> {
    // `stellar xdr decode --output json` renders ScVal bytes as base64.
    use base64::Engine as _;
    let bytes = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::general_purpose::PAD,
    )
    .decode(encoded.trim())
    .ok()?;
    if bytes.len() < 96 {
        return None;
    }
    let fee_at = |offset: usize| -> Option<u128> {
        let chunk: [u8; 16] = bytes[offset..offset + 16].try_into().ok()?;
        Some(u128::from_be_bytes(chunk))
    };
    let recipient_at = |offset: usize| hex::encode(&bytes[offset..offset + 32]);
    Some([
        (fee_at(0)?, recipient_at(16)),
        (fee_at(48)?, recipient_at(64)),
    ])
}

/// Address kernel key: sha256(domain || version || strkey), the exact bytes
/// the runtime stores for fee recipients and banned addresses.
pub(super) fn kernel_address(strkey: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(KERNEL_ADDRESS_DOMAIN);
    hasher.update([KERNEL_ADDRESS_VERSION]);
    hasher.update(strkey.as_bytes());
    hex::encode(hasher.finalize())
}

pub(super) fn parse_proxy_view(stdout: &str, facts: &mut VaultFacts) {
    let Ok(value) = serde_json::from_str::<Value>(stdout) else {
        return;
    };
    facts.governance_view = value
        .get(0)
        .and_then(|tuple| tuple.get(1))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    if let Some(position1) = value.get(1) {
        facts.paused = position1.get(0).and_then(Value::as_bool);
        facts.virtual_assets = position1.get(2).and_then(json_i128);
        facts.external_assets_present = position1.get(5).and_then(Value::as_bool);
    }
    if let Some(position2) = value.get(2) {
        facts.max_deposit_allowed = position2.get(5).and_then(json_i128);
    }
}

pub(super) fn parse_adapter_map(value: &Value) -> Option<Vec<(u32, String)>> {
    let entries = value.as_object()?;
    let mut bindings = Vec::new();
    for (market, adapter) in entries {
        bindings.push((market.parse().ok()?, adapter.as_str()?.to_string()));
    }
    Some(bindings)
}

pub(super) fn parse_pending_ids(stdout: &str) -> Vec<u64> {
    match serde_json::from_str::<Value>(stdout) {
        Ok(value) if value.is_array() => value
            .as_array()
            .map(|values| values.iter().filter_map(json_u64).collect())
            .unwrap_or_default(),
        _ => stdout
            .split_whitespace()
            .filter_map(|token| token.parse().ok())
            .collect(),
    }
}

fn json_i128(value: &Value) -> Option<i128> {
    match value {
        Value::String(text) => text.replace('_', "").parse().ok(),
        Value::Number(number) => number.to_string().parse().ok(),
        _ => None,
    }
}

fn json_u128(value: &Value) -> Option<u128> {
    match value {
        Value::String(text) => text.replace('_', "").parse().ok(),
        Value::Number(number) => number.to_string().parse().ok(),
        _ => None,
    }
}

fn json_u64(value: &Value) -> Option<u64> {
    json_u128(value).and_then(|value| u64::try_from(value).ok())
}

pub(super) fn run_preflight<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &Manifest,
    args: &UpgradePreflightArgs,
) -> anyhow::Result<Response> {
    let cli = context.cli();
    let vault = match &args.vault {
        Some(vault) => vault.to_string(),
        None => manifest
            .contracts
            .get("vault")
            .map(|record| record.contract_id.clone())
            .context("upgrade preflight requires --vault or a vault recorded in the manifest")?,
    };
    if cli.dry_run {
        return Ok(Response::message(
            "upgrade preflight is read-only; dry-run skips the chain reads".to_string(),
        ));
    }
    let facts = collect_vault_facts(
        context.stellar(),
        manifest,
        &vault,
        args.attest_empty_withdrawal_queue,
    );
    let report = evaluate_vault_preflight(&facts);
    let response = UpgradePreflightResponse {
        vault,
        report,
        allow_unverifiable: args.allow_unverifiable,
    };
    if response.report.ok || args.allow_unverifiable {
        return Ok(Response::UpgradePreflight(response));
    }
    anyhow::bail!(
        "upgrade preflight blocked: {}",
        response
            .report
            .findings
            .iter()
            .filter(|f| f.status == PreflightStatus::Blocking
                || f.status == PreflightStatus::Unverifiable)
            .map(|f| format!("{}: {}", f.check, f.detail))
            .collect::<Vec<_>>()
            .join("; ")
    );
}
