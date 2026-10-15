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
    governance::{governance_queue, system_now_ns},
    inventory::args,
    output::{
        PreflightFinding, PreflightReport, PreflightStatus, Response, UpgradePreflightResponse,
    },
};

const KERNEL_ADDRESS_DOMAIN: &[u8] = b"templar:soroban:address";
/// Governance's `PROPOSAL_EXECUTION_WINDOW_NS`: a proposal older than this
/// past its ready time is ProposalExpired on governance that enforces it.
const PROPOSAL_EXECUTION_WINDOW_NS: u64 = 7 * 24 * 60 * 60 * 1_000_000_000;
/// Base64 XDR of `ScVal::LedgerKeyContractInstance`.
const INSTANCE_KEY_XDR: &str = "AAAAFA==";

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
            "instance storage unreadable; manual check: `stellar contract read --id <vault> --durability persistent --output json` and inspect the `migrate` instance entry (absent means false)",
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
    let (Some(recipients), Some(asset), Some(share)) = (
        &facts.fee_recipients,
        &facts.asset_token_id,
        &facts.share_token_id,
    ) else {
        return unverifiable(
            "fee_recipients",
            "fee recipient storage or token identities unreadable; manual check: `stellar contract read --id <vault> --durability persistent --output json` and compare the `fees` recipients against sha256(domain||strkey) of the vault, asset token, and share token",
        );
    };
    // `migrate` checks both slots even when the fee is zero.
    let banned = [
        ("vault", kernel_address(&facts.vault_id)),
        ("asset token", kernel_address(asset)),
        ("share token", kernel_address(share)),
    ];
    let mut problems = Vec::new();
    for (slot, (_, recipient)) in recipients.iter().enumerate() {
        let label = if slot == 0 {
            "performance"
        } else {
            "management"
        };
        for (name, address) in &banned {
            if address == recipient {
                problems.push(format!(
                    "{label} fee recipient is the {name} (FIND-009: migrate would reject the vault); set a valid recipient before upgrading"
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
        Some(_) if facts.attest_empty_withdrawal_queue => warn(
            "withdrawal_queue",
            "operator attested the withdrawal queue is empty; the check cannot confirm it from the exposed views, so a non-empty queue would make `migrate` fail",
        ),
        Some(_) => unverifiable(
            "withdrawal_queue",
            "virtual assets are non-zero and no view exposes the withdrawal queue; drain the queue, then re-run with --attest-empty-withdrawal-queue",
        ),
    }
}

/// `migrate` has no idle or paused precondition, so operator state only
/// ever warns (it still matters for fee changes after the upgrade).
fn op_state_finding(facts: &VaultFacts) -> PreflightFinding {
    match (facts.paused, facts.max_deposit_allowed) {
        (Some(true), _) => warn(
            "op_state_idle",
            "the vault is paused; migrate does not require an idle vault, but confirm no operator action is mid-flight",
        ),
        (Some(false), Some(0)) => warn(
            "op_state_idle",
            "deposits are capped to zero while unpaused, so the operator state is probably not idle; migrate does not require idleness",
        ),
        (Some(false), Some(_)) => pass(
            "op_state_idle",
            "deposits are allowed, so the operator state is idle",
        ),
        _ => warn(
            "op_state_idle",
            "proxy_view unavailable; migrate does not require an idle vault, but check proxy_view paused/max_deposit manually",
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
    let mut seen = std::collections::BTreeSet::new();
    let mut duplicates = Vec::new();
    for (_, adapter) in bindings {
        if !seen.insert(adapter) && !duplicates.contains(adapter) {
            duplicates.push(adapter.clone());
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

/// Pure decision for the governance-write guard: writes against the manifest
/// governance are only safe while it matches the vault's on-chain binding.
/// Unreadable bindings fail closed — a stale manifest record must never let
/// governance writes land on a contract the vault no longer consults.
pub(super) fn governance_binding_matches(
    manifest_governance: Option<&str>,
    on_chain: Option<&str>,
) -> anyhow::Result<()> {
    match (manifest_governance, on_chain) {
        (Some(manifest), Some(on_chain)) if manifest == on_chain => Ok(()),
        (Some(manifest), Some(on_chain)) => anyhow::bail!(
            "manifest governance {manifest} does not match the vault's on-chain governance \
             {on_chain}; refusing governance writes — if this is a cutover that already \
             executed, run `upgrade confirm-governance-cutover`; otherwise remove the stale \
             governance record"
        ),
        (Some(manifest), None) => anyhow::bail!(
            "cannot read the vault's governance binding (proxy_view); refusing governance \
             writes for manifest governance {manifest} (fail closed)"
        ),
        (None, _) => anyhow::bail!(
            "manifest records no governance; refusing governance writes (fail closed)"
        ),
    }
}

/// Reads the vault's governance binding via `proxy_view` (`[0][0][1]`).
/// `None` means the binding could not be read.
pub(super) fn read_governance_binding<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    vault: &str,
) -> Option<String> {
    let view = stellar
        .invoke_view(
            vault,
            "proxy_view",
            args([("--owner", vault), ("--assets", "0"), ("--shares", "0")]),
        )
        .ok()?;
    let mut facts = VaultFacts::default();
    parse_proxy_view(&view.stdout, &mut facts);
    facts.governance_view
}

fn governance_queue_finding(facts: &VaultFacts) -> PreflightFinding {
    match (&facts.pending_proposals, facts.stale_proposals) {
        (Some(pending), _) if pending.is_empty() => pass(
            "governance_queue",
            "no proposals are pending on the active governance contract",
        ),
        (Some(pending), _) => blocking(
            "governance_queue",
            format!(
                "{} proposal(s) pending on the active governance; pending proposals do not \
                 move to the new governance — accept or revoke them (`governance queue`, \
                 `governance accept --proposal-id`, `governance revoke --proposal-id`){}",
                pending.len(),
                match facts.stale_proposals {
                    Some(stale) if stale > 0 =>
                        format!(" ({stale} are older than the 7-day execution window: revoke)"),
                    _ => String::new(),
                }
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

fn warn(check: &str, detail: impl Into<String>) -> PreflightFinding {
    finding(check, (PreflightStatus::Warn, detail.into()))
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
        facts.migration_flag = migration_flag_from(&entries);
        facts.fee_recipients = entries.get("fees").and_then(decode_fee_recipients);
        facts.adapter_bindings = adapter_bindings_from(&entries);
    }

    if let Some(governance) = facts.governance_view.clone() {
        if let Ok(queue) = governance_queue(stellar, &governance, None) {
            facts.pending_proposals = Some(queue.proposals.iter().map(|p| p.proposal_id).collect());
            if queue.warnings.is_empty() {
                facts.stale_proposals = queue
                    .proposals
                    .iter()
                    .map(|p| p.valid_after_ns)
                    .collect::<Option<Vec<u64>>>()
                    .map(|readys| {
                        let now = system_now_ns();
                        readys
                            .iter()
                            .filter(|ready| {
                                now.saturating_sub(**ready) > PROPOSAL_EXECUTION_WINDOW_NS
                            })
                            .count() as u64
                    });
            }
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
                vec!["--id".to_string(), vault.to_string()],
                vec!["--key-xdr".to_string(), INSTANCE_KEY_XDR.to_string()],
                vec!["--durability".to_string(), "persistent".to_string()],
                vec!["--output".to_string(), "json".to_string()],
                stellar.network_args(),
            ]
            .concat(),
            &[],
            stellar.source_env(),
        )
        .ok()?;
    parse_contract_read_instance(&out.stdout)
}

/// `stellar contract read --output json` emits CSV rows of
/// `key,value,last_modified_ledger,live_until_ledger`; the value column is
/// the JSON-rendered ledger entry.
pub(super) fn parse_contract_read_instance(stdout: &str) -> Option<serde_json::Map<String, Value>> {
    parse_instance_storage(&csv_field(stdout, 1)?)
}

/// Returns the zero-based `index`th field of the first CSV record, handling
/// quoted fields with doubled quotes and embedded newlines.
fn csv_field(input: &str, index: usize) -> Option<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut quoted = false;
    while let Some(ch) = chars.next() {
        match (ch, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                chars.next();
                current.push('"');
            }
            ('"', true) => quoted = false,
            ('"', false) if current.is_empty() => quoted = true,
            (',', false) => fields.push(std::mem::take(&mut current)),
            ('\n' | '\r', false) => break,
            (c, _) => current.push(c),
        }
    }
    fields.push(current);
    fields.into_iter().nth(index)
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

/// The stored `migrate` flag: absent means false (the runtime removes the
/// key when clearing); present but not a tagged bool is unreadable.
pub(super) fn migration_flag_from(entries: &serde_json::Map<String, Value>) -> Option<bool> {
    match entries.get("migrate") {
        None => Some(false),
        Some(value) => value.get("bool")?.as_bool(),
    }
}

/// Absent `adapmap` means no adapter bindings.
pub(super) fn adapter_bindings_from(
    entries: &serde_json::Map<String, Value>,
) -> Option<Vec<(u32, String)>> {
    match entries.get("adapmap") {
        None => Some(Vec::new()),
        Some(value) => parse_adapter_map(value),
    }
}

/// Decodes the stored `fees` blob, rendered as `{"bytes": "<hex>"}`. Layout
/// (little-endian WADs): fee(16) recipient(32) fee(16) recipient(32) then a
/// growth-cap flag byte (+16 bytes when set): 97 or 113 bytes.
pub(super) fn decode_fee_recipients(value: &Value) -> Option<[(u128, String); 2]> {
    let bytes = hex::decode(value.get("bytes")?.as_str()?.trim()).ok()?;
    if bytes.len() != 97 && bytes.len() != 113 {
        return None;
    }
    let fee_at = |offset: usize| -> Option<u128> {
        let chunk: [u8; 16] = bytes[offset..offset + 16].try_into().ok()?;
        Some(u128::from_le_bytes(chunk))
    };
    let recipient_at = |offset: usize| hex::encode(&bytes[offset..offset + 32]);
    Some([
        (fee_at(0)?, recipient_at(16)),
        (fee_at(48)?, recipient_at(64)),
    ])
}

/// Address kernel key: sha256(domain || strkey), identical to the runtime's
/// `kernel_address_from_sdk` (no version byte).
pub(super) fn kernel_address(strkey: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(KERNEL_ADDRESS_DOMAIN);
    hasher.update(strkey.as_bytes());
    hex::encode(hasher.finalize())
}

/// Parses `proxy_view`'s JSON. Layout (entrypoints.rs `proxy_view`):
/// `[[[_, governance, asset, share], [vshares, vassets, paused],
/// [_, _, external, _], _], _, [_, _, max_deposit, ..]]`.
pub(super) fn parse_proxy_view(stdout: &str, facts: &mut VaultFacts) {
    let trimmed = stdout.trim();
    let value = serde_json::from_str::<Value>(trimmed)
        .ok()
        .filter(Value::is_array)
        .or_else(|| {
            // Read-only invokes print a banner line before the JSON value.
            trimmed
                .find('[')
                .and_then(|index| serde_json::from_str::<Value>(&trimmed[index..]).ok())
                .filter(Value::is_array)
        });
    let Some(value) = value else {
        return;
    };
    let text_at = |path: &[usize]| {
        path.iter()
            .try_fold(&value, |node, index| node.get(*index))
            .and_then(Value::as_str)
            .map(ToString::to_string)
    };
    let node_at = |path: &[usize]| path.iter().try_fold(&value, |node, index| node.get(*index));
    facts.governance_view = text_at(&[0, 0, 1]);
    if let Some(asset) = text_at(&[0, 0, 2]) {
        facts.asset_token_id = Some(asset);
    }
    if let Some(share) = text_at(&[0, 0, 3]) {
        facts.share_token_id = Some(share);
    }
    facts.virtual_assets = node_at(&[0, 1, 1]).and_then(json_num);
    facts.paused = node_at(&[0, 1, 2]).and_then(Value::as_bool);
    facts.external_assets_present = node_at(&[0, 2, 2])
        .and_then(json_num)
        .map(|external: i128| external != 0);
    facts.max_deposit_allowed = node_at(&[2, 2]).and_then(json_num);
}

/// Parses the tagged ScVal map `{"map":[{"key":{"u32":n},"val":{"address":"C…"}}]}`.
pub(super) fn parse_adapter_map(value: &Value) -> Option<Vec<(u32, String)>> {
    let mut bindings = Vec::new();
    for entry in value.get("map")?.as_array()? {
        let market = entry.get("key")?.get("u32")?.as_u64()?;
        let adapter = entry.get("val")?.get("address")?.as_str()?;
        bindings.push((u32::try_from(market).ok()?, adapter.to_string()));
    }
    Some(bindings)
}

fn json_num<T: std::str::FromStr>(value: &Value) -> Option<T> {
    match value {
        Value::String(text) => text.replace('_', "").parse().ok(),
        Value::Number(number) => number.to_string().parse().ok(),
        _ => None,
    }
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
    let response = UpgradePreflightResponse { vault, report };
    if response.report.ok {
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

#[cfg(test)]
mod tests {
    use super::governance_binding_matches;

    #[test]
    fn governance_write_guard_fails_closed_on_mismatch_or_unreadable_binding() {
        assert!(governance_binding_matches(Some("CNEW"), Some("CNEW")).is_ok());
        let mismatch = governance_binding_matches(Some("COLD"), Some("CNEW")).unwrap_err();
        assert!(mismatch
            .to_string()
            .contains("upgrade confirm-governance-cutover"));
        assert!(governance_binding_matches(Some("COLD"), None).is_err());
        assert!(governance_binding_matches(None, Some("CNEW")).is_err());
    }
}
