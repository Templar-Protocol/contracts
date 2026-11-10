use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use templar_common::market::MarketConfiguration;
use templar_gateway_methods_spec::chain;
use templar_gateway_types::CryptoHash;

use crate::{
    commands::{load_json_file, market::GenerateConfigPatch},
    context::{print_json, CliContext},
    spec::{
        patch::{
            ByteExpr, Expectation, PatchCheck, PatchOperation, PatchSpec, PatchViewCheck,
            PATCH_SCHEMA_VERSION,
        },
        patch_plan::RestoreCode,
    },
};

use super::{market_state, patch::restore_from_state, patch_state::snapshot_final};

const ECONOMIC_CAVEAT: &str = "Configuration-span consistency is not economic safety. Asset changes do not convert inventory or prove raw-unit equivalence or solvency. MCR, duration, ranges, fees, oracle parameters, yield weights and protocol recipients can affect existing positions and history. No balances, accumulators, positions or cached interest are migrated. Oracle pause does not isolate the market or prevent concurrent state changes; guarded planning/replay/approval remain required.";

/// A strict subset of RFC 6902, not the permissive general patch input type.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum ConfigChange {
    Test { path: String, value: Value },
    Replace { path: String, value: Value },
}

pub(crate) fn apply_changes(
    before: &MarketConfiguration,
    changes: &[ConfigChange],
) -> Result<MarketConfiguration> {
    let mut document = serde_json::to_value(before)?;
    apply_document(&mut document, changes)?;
    let after: MarketConfiguration = MarketConfiguration::deserialize(&document)
        .context("deserialize patched market configuration")?;
    ensure!(
        serde_json::to_value(&after)? == document,
        "patched configuration contains unknown fields or noncanonical values"
    );
    after
        .validate()
        .context("validate patched market configuration")?;
    ensure!(
        before.time_chunk_configuration == after.time_chunk_configuration,
        "time_chunk_configuration edits require migration of persisted derived interest state"
    );
    ensure!(
        before.borrow_interest_rate_strategy == after.borrow_interest_rate_strategy,
        "borrow_interest_rate_strategy edits require migration of persisted derived interest state"
    );
    ensure!(before != &after, "configuration changes are a no-op");
    Ok(after)
}

fn apply_document(document: &mut Value, changes: &[ConfigChange]) -> Result<()> {
    for change in changes {
        if let ConfigChange::Replace { path, .. } = change {
            ensure!(
                !path.is_empty(),
                "root configuration replacement is not supported"
            );
        }
        // json-patch uses its RFC 6901 parser, including escaped object keys,
        // canonical array indices, existing-path checks and ordered tests.
        let operation: json_patch::PatchOperation =
            serde_json::from_value(serde_json::to_value(change)?)
                .context("invalid configuration JSON Pointer")?;
        json_patch::patch(document, &[operation]).context("apply configuration JSON Patch")?;
    }
    Ok(())
}

#[derive(Serialize)]
struct ConfigurationOffsets {
    start: usize,
    old_end: usize,
    new_end: usize,
}

#[derive(Serialize)]
struct SuffixHashes {
    before: String,
    after: String,
}

#[derive(Serialize)]
struct GeneratedPaths {
    spec: PathBuf,
    blobs: PathBuf,
    old_state: PathBuf,
    new_state: PathBuf,
    before: PathBuf,
    after: PathBuf,
    report: PathBuf,
}

impl GeneratedPaths {
    fn new(spec: PathBuf) -> Result<Self> {
        let stem = spec
            .file_stem()
            .and_then(|name| name.to_str())
            .context("patch output must have a UTF-8 file stem")?;
        let blobs = spec.with_file_name(format!("{stem}.blobs"));
        Ok(Self {
            spec,
            old_state: blobs.join("old-state.bin"),
            new_state: blobs.join("new-state.bin"),
            before: blobs.join("before.json"),
            after: blobs.join("after.json"),
            report: blobs.join("report.json"),
            blobs,
        })
    }
}

#[derive(Serialize)]
struct GenerationReport {
    account_id: near_account_id::AccountId,
    snapshot_block_hash: CryptoHash,
    snapshot_block_height: u64,
    layout: &'static str,
    release: &'static templar_contract_artifacts::ArtifactRelease,
    effective_code_sha256: String,
    effective_code_hash: CryptoHash,
    restore: RestoreCode,
    changes: Vec<ConfigChange>,
    old_configuration: MarketConfiguration,
    new_configuration: MarketConfiguration,
    old_state_sha256: String,
    new_state_sha256: String,
    configuration_offsets: ConfigurationOffsets,
    opaque_suffix_sha256: SuffixHashes,
    economic_caveat: &'static str,
    paths: GeneratedPaths,
}

pub(super) async fn generate(ctx: CliContext, args: GenerateConfigPatch) -> Result<()> {
    let changes: Vec<ConfigChange> = load_json_file(&args.changes)?;
    let paths = GeneratedPaths::new(args.out)?;
    ensure!(
        !paths.spec.try_exists()?,
        "refusing to overwrite {}",
        paths.spec.display()
    );
    ensure!(
        !paths.blobs.try_exists()?,
        "refusing to overwrite {}",
        paths.blobs.display()
    );
    let mut reporter = ctx.reporter(&[]);
    let limits = ctx.client.read(chain::GetProtocolLimits).await?;
    let snapshot = match snapshot_final(
        &ctx,
        &args.market_id,
        &limits,
        "market.config_patch.state_complete",
        &mut reporter,
    )
    .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            reporter.digest();
            return Err(error);
        }
    };
    let release = market_state::supported_release(&snapshot.code)?;
    let restore = restore_from_state(&snapshot)?;
    let mut entries = snapshot
        .entries
        .iter()
        .filter(|entry| entry.key == b"STATE");
    let state = &entries
        .next()
        .context("market snapshot has no STATE entry")?
        .value;
    ensure!(
        entries.next().is_none(),
        "market snapshot has ambiguous STATE entries"
    );
    let envelope = market_state::decode(state)?;
    let block_hash = CryptoHash::from(snapshot.block_hash);
    let reader = templar_gateway_core::NearClient::new(ctx.network_config().clone());
    let market_reader = reader.market(args.market_id.clone());
    let chain_reader = reader.chain();
    let (view, block) = tokio::try_join!(
        market_reader.get_configuration_at(block_hash),
        chain_reader.block(Some(block_hash)),
    )?;
    ensure!(
        block.hash == block_hash,
        "snapshot block header hash mismatch"
    );
    ensure!(
        view == envelope.configuration,
        "STATE configuration differs from get_configuration at the snapshot block"
    );
    let after = apply_changes(&envelope.configuration, &changes)?;
    let replacement = envelope.replace(state, &after)?;
    let new_end = replacement.len() - (state.len() - envelope.configuration_end);
    let before_suffix = &state[envelope.configuration_end..];
    let after_suffix = &replacement[new_end..];
    ensure!(before_suffix == after_suffix, "opaque STATE suffix changed");
    let report = GenerationReport {
        account_id: args.market_id,
        snapshot_block_hash: block_hash,
        snapshot_block_height: block.height,
        layout: market_state::LAYOUT,
        release,
        effective_code_sha256: sha256(&snapshot.code),
        effective_code_hash: CryptoHash::from(near_api::types::CryptoHash::hash(&snapshot.code)),
        restore,
        changes,
        old_configuration: envelope.configuration,
        new_configuration: after,
        old_state_sha256: sha256(state),
        new_state_sha256: sha256(&replacement),
        configuration_offsets: ConfigurationOffsets {
            start: envelope.configuration_start,
            old_end: envelope.configuration_end,
            new_end,
        },
        opaque_suffix_sha256: SuffixHashes {
            before: sha256(before_suffix),
            after: sha256(after_suffix),
        },
        economic_caveat: ECONOMIC_CAVEAT,
        paths,
    };
    publish(&report, state, &replacement)?;
    reporter.digest();
    print_json(&report)
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn fresh_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("create {} without overwriting", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync {}", path.display()))
}

fn pretty_json(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Reserve the sidecar with create_dir, then publish the spec last using a
/// no-clobber hard link. A concurrent author can never replace either output.
/// On failure only our reserved directory and temporary file are removed.
fn publish(report: &GenerationReport, old_state: &[u8], new_state: &[u8]) -> Result<()> {
    let paths = &report.paths;
    let blob_name = paths
        .blobs
        .file_name()
        .context("missing sidecar directory name")?;
    let blob_relative = Path::new(blob_name);
    let spec = PatchSpec {
        schema: PATCH_SCHEMA_VERSION,
        extends: Vec::new(),
        account_id: report.account_id.clone(),
        ops: vec![PatchOperation::Set {
            key: ByteExpr::Utf8("STATE".into()),
            value: ByteExpr::File(blob_relative.join("new-state.bin")),
            expect: Some(Expectation::Bytes(ByteExpr::File(
                blob_relative.join("old-state.bin"),
            ))),
        }],
        checks: vec![PatchCheck::View(Box::new(PatchViewCheck {
            method_name: "get_configuration".to_owned().into(),
            args: serde_json::json!({}),
            expect: None,
            expect_json: Some(crate::spec::patch::JsonText::from_value(
                serde_json::to_value(&report.new_configuration)?,
            )?),
        }))],
    };
    let rendered = toml::to_string_pretty(&spec).context("render generated patch spec")?;
    let temporary = paths.blobs.join(".patch-spec.tmp");
    // create_dir is the reservation: failure leaves someone else's output alone.
    fs::create_dir(&paths.blobs)
        .with_context(|| format!("reserve {} without overwriting", paths.blobs.display()))?;
    let result = (|| {
        fresh_file(&paths.old_state, old_state)?;
        fresh_file(&paths.new_state, new_state)?;
        fresh_file(&paths.before, &pretty_json(&report.old_configuration)?)?;
        fresh_file(&paths.after, &pretty_json(&report.new_configuration)?)?;
        fresh_file(&paths.report, &pretty_json(report)?)?;
        fs::File::open(&paths.blobs)?.sync_all()?;
        fresh_file(&temporary, rendered.as_bytes())?;
        fs::hard_link(&temporary, &paths.spec)
            .with_context(|| format!("publish {} without overwriting", paths.spec.display()))?;
        Ok(())
    })();
    let _ = fs::remove_file(&temporary);
    if result.is_err() {
        let _ = fs::remove_dir_all(&paths.blobs);
    } else if let Some(parent) = paths
        .spec
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        // Match existing atomic writers: directory fsync is best effort after
        // the complete spec has already become visible.
        let _ = fs::File::open(parent).and_then(|directory| directory.sync_all());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn configuration() -> MarketConfiguration {
        crate::spec::plan::testing::alpha_market()
            .into_market_configuration(6, 7)
            .unwrap()
    }

    fn changes(value: Value) -> Vec<ConfigChange> {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn applies_ordered_tests_escaped_keys_and_array_indices() {
        let mut document = json!({"a/b": {"~key": [1, 2]}});
        apply_document(
            &mut document,
            &changes(json!([
                {"op":"test","path":"","value":{"a/b":{"~key":[1,2]}}}
            ])),
        )
        .unwrap();
        apply_document(
            &mut document,
            &changes(json!([
                {"op":"test","path":"/a~1b/~0key/1","value":2},
                {"op":"replace","path":"/a~1b/~0key/1","value":3},
                {"op":"test","path":"/a~1b/~0key/1","value":3}
            ])),
        )
        .unwrap();
        assert_eq!(document, json!({"a/b": {"~key": [1, 3]}}));
        for path in [
            "",
            "a/b",
            "/a~2b",
            "/a~1b/~0key/01",
            "/a~1b/~0key/-",
            "/a~1b/~0key/2",
            "/a~1b/missing",
        ] {
            assert!(
                apply_document(
                    &mut document.clone(),
                    &changes(json!([
                        {"op":"replace","path":path,"value":0}
                    ]))
                )
                .is_err(),
                "accepted invalid path {path}"
            );
        }
        assert!(apply_document(
            &mut document,
            &changes(json!([
                {"op":"test","path":"/a~1b/~0key/1","value":2}
            ]))
        )
        .is_err());
    }

    #[test]
    fn rejects_other_operations_and_unknown_operation_fields() {
        for value in [
            json!([{"op":"add","path":"/protocol_account_id","value":"other.near"}]),
            json!([{"op":"remove","path":"/protocol_account_id"}]),
            json!([{"op":"copy","from":"/borrow_asset","path":"/collateral_asset"}]),
            json!([{"op":"move","from":"/borrow_asset","path":"/collateral_asset"}]),
            json!([{"op":"replace","path":"/protocol_account_id","value":"other.near","extra":1}]),
            json!([{"op":"test","path":"/borrow_asset"}]),
        ] {
            assert!(serde_json::from_value::<Vec<ConfigChange>>(value).is_err());
        }
    }

    #[test]
    fn allows_both_asset_standards_and_generic_nonasset_edits() {
        let before = configuration();
        let patch = changes(json!([
            {"op":"test","path":"/borrow_asset","value":serde_json::to_value(&before.borrow_asset).unwrap()},
            {"op":"replace","path":"/borrow_asset","value":{"Nep141":"new-borrow.near"}},
            {"op":"replace","path":"/collateral_asset","value":{"Nep245":{"contract_id":"new-mt.near","token_id":"nested:token:id"}}},
            {"op":"replace","path":"/protocol_account_id","value":"new-recipient.near"}
        ]));
        let after = apply_changes(&before, &patch).unwrap();
        let canonical = serde_json::to_value(after).unwrap();
        assert_eq!(
            canonical["borrow_asset"],
            json!({"Nep141":"new-borrow.near"})
        );
        assert_eq!(
            canonical["collateral_asset"],
            json!({"Nep245":{"contract_id":"new-mt.near","token_id":"nested:token:id"}})
        );
        assert_eq!(canonical["protocol_account_id"], "new-recipient.near");
    }

    #[test]
    fn rejects_noops_unknown_fields_invalid_configs_and_coupled_edits() {
        let before = configuration();
        let strategy = templar_common::interest_rate_strategy::InterestRateStrategy::linear(
            "0.02".parse().unwrap(),
            "0.2".parse().unwrap(),
        )
        .unwrap();
        let mut unknown_oracle = serde_json::to_value(&before.price_oracle_configuration).unwrap();
        unknown_oracle["unknown_field"] = json!(true);
        for patch in [
            json!([]),
            json!([{"op":"test","path":"/protocol_account_id","value":before.protocol_account_id}]),
            json!([{"op":"replace","path":"/protocol_account_id","value":before.protocol_account_id}]),
            json!([{"op":"replace","path":"/missing","value":1}]),
            json!([{"op":"replace","path":"/borrow_asset","value":{"Nep141":"new.near","ignored":true}}]),
            json!([{"op":"replace","path":"/price_oracle_configuration","value":unknown_oracle}]),
            json!([{"op":"replace","path":"/borrow_mcr_maintenance","value":"0"}]),
            json!([{"op":"replace","path":"/time_chunk_configuration","value":{"duration_ms":"1200000"}}]),
            json!([{"op":"replace","path":"/borrow_interest_rate_strategy","value":strategy}]),
        ] {
            assert!(apply_changes(&before, &changes(patch)).is_err());
        }
    }

    #[test]
    fn legacy_time_chunk_forms_survive_unrelated_edits_and_borsh_replacement() {
        for time_chunk in [
            json!({"BlockTimestampMs":{"divisor":"600000"}}),
            json!({"duration_ms":"600000"}),
        ] {
            let mut before = configuration();
            before.time_chunk_configuration = serde_json::from_value(time_chunk.clone()).unwrap();
            let nested_path = if time_chunk.get("duration_ms").is_some() {
                "/time_chunk_configuration/duration_ms"
            } else {
                "/time_chunk_configuration/BlockTimestampMs/divisor"
            };
            assert!(apply_changes(
                &before,
                &changes(json!([
                    {"op":"replace","path":nested_path,"value":"1200000"}
                ]))
            )
            .is_err());
            let patch = changes(json!([
                {"op":"test","path":"/time_chunk_configuration","value":time_chunk},
                {"op":"replace","path":"/time_chunk_configuration","value":time_chunk},
                {"op":"replace","path":"/protocol_account_id","value":"new-recipient.near"}
            ]));
            let after = apply_changes(&before, &patch).unwrap();
            assert_eq!(
                serde_json::to_value(&after).unwrap()["time_chunk_configuration"],
                time_chunk
            );
            let mut state = borsh::to_vec(&vec![0u8, 255, 42]).unwrap();
            state.extend(borsh::to_vec(&before).unwrap());
            let config_end = state.len();
            assert!(market_state::decode(&state).is_err());
            assert!(market_state::decode(&state[..config_end - 1]).is_err());
            let suffix = [255, 0, 42, 255, 128, 0, 0, 0];
            state.extend(suffix);
            let envelope = market_state::decode(&state).unwrap();
            let replacement = envelope.replace(&state, &after).unwrap();
            let decoded = market_state::decode(&replacement).unwrap();
            assert_eq!(decoded.configuration, after);
            assert_eq!(
                &replacement[..envelope.configuration_start],
                &state[..envelope.configuration_start]
            );
            assert_eq!(&replacement[decoded.configuration_end..], &suffix);
        }
    }
}
