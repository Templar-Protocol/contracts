use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::Write as _,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{ensure, Context as _, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use clap::Parser as _;
use near_account_id::AccountId;
use near_api::Contract;
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use templar_common::{
    fee::Fee, interest_rate_strategy::InterestRateStrategy, market::MarketConfiguration,
};
use templar_gateway_methods_spec::{chain, tx};
use templar_gateway_testing::SandboxHarness;
use templar_gateway_types::{
    common::TxExecutionStatus, ActionInput, Base64Bytes, GlobalContractIdentifierInput,
    ManagedAccountId,
};
use templar_primitives::dec;

use crate::{
    cli::Cli,
    dispatch::{
        market_config_patch::{apply_changes, ConfigChange},
        market_state,
        patch_state::{fetch_complete_state, StateSnapshot},
    },
};

use super::market_fixture::{finalize, SeededMarket, RELEASE, WASM_LENGTH, WASM_SHA256};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/market-layouts/market-1.6.1.json")
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn configuration_schema_sha256() -> Result<String> {
    let schema = borsh::schema_container_of::<MarketConfiguration>();
    Ok(sha256(&borsh::to_vec(&schema)?))
}

#[track_caller]
fn assert_account_unchanged(before: &StateSnapshot, after: &StateSnapshot) {
    assert_eq!(
        before.amount, after.amount,
        "native balance changed between blocks {} and {}",
        before.block_hash, after.block_hash
    );
    assert_eq!(before.locked, after.locked, "locked balance changed");
    assert_eq!(
        before.storage_usage, after.storage_usage,
        "storage usage changed"
    );
    assert_eq!(
        before.contract, after.contract,
        "code/global identity changed"
    );
    assert!(
        before.code == after.code,
        "effective code changed: before={} after={}",
        sha256(&before.code),
        sha256(&after.code),
    );
    assert_eq!(
        before.access_keys, after.access_keys,
        "access keys or nonces changed"
    );
    assert_eq!(before.entries, after.entries, "raw market storage changed");
}

struct OutputDir(PathBuf);

impl OutputDir {
    fn new() -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "templar-config-patch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for OutputDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn dispatch(harness: &SandboxHarness, args: &[String]) -> Result<()> {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr".to_owned(),
            "--network".to_owned(),
            "testnet".to_owned(),
            "--rpc-url".to_owned(),
            harness.network.rpc_endpoints[0].url.to_string(),
        ]
        .into_iter()
        .chain(args.iter().cloned()),
    )?;
    let context = crate::context::build_context(&cli)?;
    crate::dispatch::dispatch(context, cli.command).await
}

async fn snapshot(harness: &SandboxHarness, market: &AccountId) -> Result<StateSnapshot> {
    let client = templar_gateway_client::Client::builder(harness.network.clone())
        .finality_policy(templar_gateway_core::FinalityPolicy::Final)
        .build()?;
    let block = client.read(chain::GetBlock::default()).await?;
    let limits = client.read(chain::GetProtocolLimits).await?;
    fetch_complete_state(&harness.network, market, block.hash.into(), &limits).await
}

fn entries(snapshot: &StateSnapshot) -> BTreeMap<Vec<u8>, Vec<u8>> {
    snapshot
        .entries
        .iter()
        .map(|entry| (entry.key.clone(), entry.value.clone()))
        .collect()
}

fn state(snapshot: &StateSnapshot) -> Result<&[u8]> {
    snapshot
        .entries
        .iter()
        .find(|entry| entry.key == b"STATE")
        .map(|entry| entry.value.as_slice())
        .context("released market must have STATE")
}

#[tokio::test]
#[ignore = "explicitly capture real released Wasm layout evidence before running the config-patch suite"]
async fn requires_sandbox_config_patch_capture_market_1_6_1() -> Result<()> {
    let seeded = SeededMarket::new().await?;
    let snapshot = snapshot(&seeded.harness, &seeded.market.market_id).await?;
    ensure!(
        sha256(&snapshot.code) == WASM_SHA256,
        "snapshot must contain exact released code"
    );
    let raw_state = state(&snapshot)?;
    let envelope = market_state::decode(raw_state)?;
    let reader = templar_gateway_core::NearClient::new(seeded.harness.network.clone());
    let block_hash = templar_gateway_types::CryptoHash::from(snapshot.block_hash);
    let configuration = reader
        .market(seeded.market.market_id.clone())
        .get_configuration_at(block_hash)
        .await?;
    let block = reader.chain().block(Some(block_hash)).await?;
    ensure!(
        envelope.configuration == configuration,
        "released state/pinned view configuration mismatch"
    );
    let fixture = json!({
        "schema":1,"release":RELEASE,"wasm_sha256":WASM_SHA256,"wasm_length":WASM_LENGTH,
        "account_id":seeded.market.market_id,"block_hash":snapshot.block_hash.to_string(),"block_height":block.height,
        "state_base64":STANDARD.encode(raw_state),"state_sha256":sha256(raw_state),
        "configuration":configuration,
        "configuration_borsh_schema_sha256":configuration_schema_sha256()?,
        "configuration_offsets":{"start":envelope.configuration_start,"end":envelope.configuration_end},
        "opaque_suffix_sha256":sha256(&raw_state[envelope.configuration_end..]),
        "observations":seeded.observations,
        "entries":snapshot.entries.iter().map(|entry| json!({"key_base64":STANDARD.encode(&entry.key),"value_base64":STANDARD.encode(&entry.value)})).collect::<Vec<_>>()
    });
    let path = fixture_path();
    std::fs::create_dir_all(path.parent().context("fixture parent")?)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("capture refuses to overwrite {}", path.display()))?;
    file.write_all(&serde_json::to_vec_pretty(&fixture)?)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn captured_fixture() -> Result<Value> {
    let path = fixture_path();
    serde_json::from_slice(&std::fs::read(&path).with_context(|| format!("missing real layout evidence {}; run the ignored requires_sandbox_config_patch_capture_market_1_6_1 test", path.display()))?).map_err(Into::into)
}

#[test]
fn config_patch_pins_code_and_replacement_file_bytes() -> Result<()> {
    let dir = OutputDir::new()?;
    let source_path = dir.0.join("patch.toml");
    let data_path = dir.0.join("new-state.bin");
    let replacement = b"replacement STATE bytes";
    std::fs::write(&data_path, replacement)?;
    let code_hash = templar_gateway_types::CryptoHash::from(near_api::types::CryptoHash([7; 32]));
    let spec: crate::spec::patch::PatchSpec = serde_json::from_value(json!({
        "schema":3,"account_id":"market.testnet",
        "op":[{
            "op":"set","key":{"utf8":"STATE"},
            "value":{"pinned_file":{"path":"new-state.bin","sha256":sha256(replacement)}},
            "expect":{"hash":sha256(b"old STATE bytes")}
        }],
        "check":[{"code_hash":code_hash}]
    }))?;
    spec.verify_code_hash(&code_hash)?;
    let resolved = spec.resolve(&source_path)?;
    let [crate::spec::patch::ResolvedOperation::Set { value, .. }] = resolved.operations.as_slice()
    else {
        anyhow::bail!("expected one resolved STATE replacement");
    };
    assert_eq!(value.0, replacement);
    let wrong_code = templar_gateway_types::CryptoHash::from(near_api::types::CryptoHash([8; 32]));
    assert!(spec.verify_code_hash(&wrong_code).is_err());
    std::fs::write(&data_path, b"tampered STATE bytes")?;
    assert!(spec.resolve(&source_path).is_err());
    Ok(())
}

#[test]
fn config_patch_released_layout_fixture_preserves_opaque_bytes() -> Result<()> {
    let fixture = captured_fixture()?;
    assert_eq!(fixture["wasm_sha256"], WASM_SHA256);
    assert_eq!(fixture["wasm_length"], WASM_LENGTH);
    assert_eq!(fixture["release"], RELEASE);
    assert_eq!(
        configuration_schema_sha256()?,
        fixture["configuration_borsh_schema_sha256"]
    );
    let raw = STANDARD.decode(fixture["state_base64"].as_str().context("fixture STATE")?)?;
    assert_eq!(sha256(&raw), fixture["state_sha256"]);
    let envelope = market_state::decode(&raw)?;
    let configuration: MarketConfiguration =
        serde_json::from_value(fixture["configuration"].clone())?;
    assert_eq!(envelope.configuration, configuration);
    assert_eq!(
        borsh::to_vec(&configuration)?,
        raw[envelope.configuration_start..envelope.configuration_end]
    );
    assert_eq!(envelope.replace(&raw, &configuration)?, raw);
    assert!(!raw[envelope.configuration_end..].is_empty());
    assert_eq!(
        sha256(&raw[envelope.configuration_end..]),
        fixture["opaque_suffix_sha256"]
    );
    assert!(
        fixture["observations"]["static_yield"]["amount"]
            .as_str()
            .context("observed static yield")?
            .parse::<u128>()?
            > 0
    );
    assert!(
        fixture["observations"]["queue"]["status"]["length"]
            .as_u64()
            .context("observed queue length")?
            > 0
    );
    let mut changed = configuration;
    changed.protocol_account_id = "another-protocol.test.near".parse()?;
    let replaced = envelope.replace(&raw, &changed)?;
    let new = market_state::decode(&replaced)?;
    assert_eq!(
        &raw[..envelope.configuration_start],
        &replaced[..new.configuration_start]
    );
    assert_eq!(
        &raw[envelope.configuration_end..],
        &replaced[new.configuration_end..]
    );
    for end in [
        0,
        1,
        3,
        envelope.configuration_start,
        envelope.configuration_end - 1,
    ] {
        assert!(
            market_state::decode(&raw[..end]).is_err(),
            "truncated configuration at {end} decoded"
        );
    }
    let mut bad_prefix = raw;
    bad_prefix[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(market_state::decode(&bad_prefix).is_err());
    Ok(())
}

fn decimal_configuration() -> MarketConfiguration {
    let mut configuration = crate::spec::plan::testing::alpha_market()
        .into_market_configuration(6, 7)
        .unwrap();
    configuration.borrow_mcr_maintenance = dec!("1.4");
    configuration.borrow_mcr_liquidation = dec!("1.3");
    configuration.liquidation_maximum_spread = dec!("0.1");
    configuration.validate().unwrap();
    configuration
}

#[rstest::rstest]
#[case::short_replacement(json!([
    {"op":"replace","path":"/borrow_mcr_maintenance","value":"1.8"}
]))]
#[case::semantic_tests(json!([
    {"op":"test","path":"/borrow_mcr_maintenance","value":"1.4"},
    {"op":"replace","path":"/borrow_mcr_maintenance","value":"1.8"},
    {"op":"test","path":"/borrow_mcr_maintenance","value":"1.8"}
]))]
fn config_patch_decimal_tests_and_replacements_preserve_opaque_state(
    #[case] changes: Value,
) -> Result<()> {
    let fixture = captured_fixture()?;
    let state = STANDARD.decode(fixture["state_base64"].as_str().context("fixture STATE")?)?;
    let before: MarketConfiguration = serde_json::from_value(fixture["configuration"].clone())?;
    let changes: Vec<ConfigChange> = serde_json::from_value(changes)?;
    let after = apply_changes(&before, &changes)?;
    let mut expected = before.clone();
    expected.borrow_mcr_maintenance = dec!("1.8");
    assert_eq!(after, expected);
    assert_eq!(before.borrow_mcr_maintenance, dec!("1.4"));

    let envelope = market_state::decode(&state)?;
    let replacement = envelope.replace(&state, &after)?;
    let decoded = market_state::decode(&replacement)?;
    assert_eq!(decoded.configuration, expected);
    assert_eq!(
        &replacement[..decoded.configuration_start],
        &state[..envelope.configuration_start]
    );
    assert_eq!(
        &replacement[decoded.configuration_end..],
        &state[envelope.configuration_end..]
    );
    Ok(())
}

#[test]
fn config_patch_decimal_normalization_handles_fee_objects_and_nested_leaves() -> Result<()> {
    let mut before = decimal_configuration();
    before.borrow_origination_fee = Fee::Proportional(dec!("0.1"));
    before.supply_withdrawal_fee.fee = Fee::Flat(0.into());
    let mut fee = serde_json::to_value(&before.supply_withdrawal_fee)?;
    fee["fee"] = json!({"Proportional":"0.1"});
    let changes: Vec<ConfigChange> = serde_json::from_value(json!([
        {"op":"test","path":"/borrow_origination_fee","value":{"Proportional":"0.1"}},
        {"op":"replace","path":"/borrow_origination_fee/Proportional","value":"0.2"},
        {"op":"test","path":"/borrow_origination_fee/Proportional","value":"0.2"},
        {"op":"replace","path":"/supply_withdrawal_fee","value":fee},
        {"op":"test","path":"/supply_withdrawal_fee","value":fee},
        {"op":"replace","path":"/supply_withdrawal_fee/fee/Proportional","value":"0.3"},
        {"op":"test","path":"/supply_withdrawal_fee/fee/Proportional","value":"0.3"}
    ]))?;
    let after = apply_changes(&before, &changes)?;
    let mut expected = before;
    expected.borrow_origination_fee = Fee::Proportional(dec!("0.2"));
    expected.supply_withdrawal_fee.fee = Fee::Proportional(dec!("0.3"));
    assert_eq!(after, expected);
    Ok(())
}

#[test]
fn config_patch_decimal_normalization_handles_root_tests_and_strategy_objects() -> Result<()> {
    for strategy in [
        json!({"Linear":{"base":"0.1","top":"0.2"}}),
        json!({"Piecewise":{"base":"0.1","optimal":"0.8","rate_1":"0.2","rate_2":"0.4"}}),
        json!({"Exponential2":{"base":"0.1","top":"0.2","eccentricity":"1.8"}}),
    ] {
        let mut before = decimal_configuration();
        before.borrow_interest_rate_strategy =
            serde_json::from_value::<InterestRateStrategy>(strategy.clone())?;
        let mut whole = serde_json::to_value(&before)?;
        whole["borrow_mcr_maintenance"] = json!("1.4");
        whole["borrow_mcr_liquidation"] = json!("1.3");
        whole["liquidation_maximum_spread"] = json!("0.1");
        whole["borrow_interest_rate_strategy"] = strategy.clone();
        let variant = strategy
            .as_object()
            .and_then(|object| object.keys().next())
            .context("strategy variant")?;
        let base_path = format!("/borrow_interest_rate_strategy/{variant}/base");
        let changes: Vec<ConfigChange> = serde_json::from_value(json!([
            {"op":"test","path":"","value":whole},
            {"op":"test","path":"/borrow_interest_rate_strategy","value":strategy},
            {"op":"replace","path":"/borrow_interest_rate_strategy","value":strategy},
            {"op":"test","path":base_path,"value":"0.1"},
            {"op":"replace","path":base_path,"value":"0.10"},
            {"op":"replace","path":"/borrow_mcr_maintenance","value":"1.8"}
        ]))?;
        let after = apply_changes(&before, &changes)?;
        let mut expected = before;
        expected.borrow_mcr_maintenance = dec!("1.8");
        assert_eq!(after, expected);
    }
    Ok(())
}

#[test]
fn config_patch_decimal_normalization_keeps_tests_strict_and_blocks_noops() -> Result<()> {
    let mut before = decimal_configuration();
    before.borrow_interest_rate_strategy = InterestRateStrategy::zero();
    for changes in [
        json!([
            {"op":"test","path":"/borrow_mcr_maintenance","value":"1.5"},
            {"op":"replace","path":"/borrow_mcr_maintenance","value":"1.8"}
        ]),
        json!([
            {"op":"replace","path":"/borrow_mcr_maintenance","value":"1.8"},
            {"op":"test","path":"/borrow_mcr_maintenance","value":"1.4"}
        ]),
        json!([{"op":"replace","path":"/borrow_mcr_maintenance","value":"1.40"}]),
        json!([{"op":"test","path":"/borrow_mcr_maintenance","value":"1.4"}]),
        json!([{"op":"replace","path":"/borrow_mcr_maintenance","value":1.8}]),
        json!([{"op":"replace","path":"/borrow_mcr_maintenance","value":"01.8"}]),
        json!([
            {"op":"replace","path":"/borrow_interest_rate_strategy","value":{"Linear":{"base":"0.1","top":"0.2"}}},
            {"op":"replace","path":"/borrow_mcr_maintenance","value":"1.8"}
        ]),
    ] {
        let changes: Vec<ConfigChange> = serde_json::from_value(changes)?;
        assert!(apply_changes(&before, &changes).is_err());
    }
    Ok(())
}

#[test]
fn config_patch_decimal_normalization_rejects_unrelated_noncanonical_values() -> Result<()> {
    let before = decimal_configuration();
    let mut unknown_fee = serde_json::to_value(&before.supply_withdrawal_fee)?;
    unknown_fee["fee"] = json!({"Proportional":"0.1"});
    unknown_fee["unknown"] = json!(true);
    for change in [
        json!({"op":"replace","path":"/borrow_maximum_duration_ms","value":"001"}),
        json!({"op":"replace","path":"/borrow_range/minimum","value":"001"}),
        json!({"op":"replace","path":"/borrow_origination_fee","value":{"Flat":"001"}}),
        json!({"op":"replace","path":"/supply_withdrawal_fee/duration","value":"001"}),
        json!({"op":"replace","path":"/supply_withdrawal_fee","value":unknown_fee}),
        json!({"op":"replace","path":"/protocol_account_id","value":"UPPER.near"}),
        json!({"op":"replace","path":"/borrow_mcr_maintenance_extra","value":"1.8"}),
        json!({"op":"replace","path":"","value":serde_json::to_value(&before)?}),
        json!({"op":"replace","path":"/borrow_mcr_maintenance~2","value":"1.8"}),
    ] {
        let changes: Vec<ConfigChange> = serde_json::from_value(json!([
            {"op":"replace","path":"/borrow_mcr_maintenance","value":"1.8"},
            change
        ]))?;
        assert!(apply_changes(&before, &changes).is_err());
    }

    let mut before = before;
    before.collateral_asset = serde_json::from_value(json!({
        "Nep245":{"contract_id":"multi.near","token_id":"1.8"}
    }))?;
    let changes: Vec<ConfigChange> = serde_json::from_value(json!([
        {"op":"test","path":"/collateral_asset/Nep245/token_id","value":"1.800"},
        {"op":"replace","path":"/borrow_mcr_maintenance","value":"1.8"}
    ]))?;
    assert!(apply_changes(&before, &changes).is_err());
    let changes: Vec<ConfigChange> = serde_json::from_value(json!([
        {"op":"replace","path":"/collateral_asset/Nep245/token_id","value":"001.8"},
        {"op":"replace","path":"/borrow_mcr_maintenance","value":"1.8"}
    ]))?;
    let after = apply_changes(&before, &changes)?;
    let mut expected = before;
    expected.collateral_asset = serde_json::from_value(json!({
        "Nep245":{"contract_id":"multi.near","token_id":"001.8"}
    }))?;
    expected.borrow_mcr_maintenance = dec!("1.8");
    assert_eq!(after, expected);
    Ok(())
}

async fn generate(seeded: &SeededMarket, changes: &Value, output: &Path) -> Result<()> {
    let changes_path = output.with_extension("changes.json");
    std::fs::write(&changes_path, serde_json::to_vec(changes)?)?;
    dispatch(
        &seeded.harness,
        &[
            "market".into(),
            "generate-patch".into(),
            "config".into(),
            seeded.market.market_id.to_string(),
            "--changes".into(),
            changes_path.display().to_string(),
            "--out".into(),
            output.display().to_string(),
        ],
    )
    .await
}

enum RestoreFixture {
    Local,
    GlobalHash,
    GlobalAccount,
}

enum PatchScenario {
    Assets,
    Generic,
    GuardMismatch,
}

async fn assert_receipt_rollback(
    seeded: &SeededMarket,
    before: &StateSnapshot,
    mut batch: tx::Batch,
) -> Result<()> {
    let ActionInput::FunctionCall {
        args: templar_gateway_types::common::ContractArgs::Raw(args),
        ..
    } = &mut batch.actions[1]
    else {
        anyhow::bail!("generated batch must contain the raw PatchState call");
    };
    let mut patch = templar_patch_state_types::Patch::from_borsh_slice(&args.0)
        .map_err(|error| anyhow::anyhow!("decode generated PatchState arguments: {error:?}"))?;
    // The generated expectation passes and its Set changes STATE first. This
    // final expectation then fails inside the same receipt, after a real write.
    patch.ops.push(templar_patch_state_types::Op::Expect {
        key: b"STATE".to_vec(),
        value: Some(state(before)?.to_vec()),
    });
    args.0 = borsh::to_vec(&patch)?;

    // Submit through the native gateway planner, deliberately not patch apply:
    // its live preflight must continue rejecting tampered plans. The unmodified
    // deploy/restore actions still surround the deliberately failing patch.
    let result = seeded
        .harness
        .try_execute(&ManagedAccountId(seeded.market.market_id.clone()), batch)
        .await?;
    let outcome = result
        .operation
        .steps
        .first()
        .and_then(|step| step.status.outcome())
        .context("expected an executed on-chain batch, not preflight rejection")?;
    assert!(
        outcome
            .failure
            .as_deref()
            .is_some_and(|failure| failure.contains("storage expectation failed")),
        "expected the PatchState guard failure, got {:?}",
        outcome.failure
    );
    let failed = templar_gateway_testing::failed_receipts(&result).collect::<Vec<_>>();
    assert_eq!(failed.len(), 1, "expected exactly the atomic batch to fail");
    assert_eq!(failed[0].contract_id, seeded.market.market_id);
    let finality = seeded
        .harness
        .client()?
        .read(tx::Get {
            tx_hash: result
                .operation
                .latest_tx_hash()
                .context("failed batch transaction hash")?,
            sender_account_id: seeded.market.market_id.clone(),
            wait_until: Some(TxExecutionStatus::Final),
            encoding: tx::ValueEncoding::Json,
        })
        .await?;
    assert_eq!(finality.status, tx::Status::Failed);
    assert_eq!(
        finality.failed_receipts,
        vec![seeded.market.market_id.clone()]
    );
    seeded.harness.fast_forward(10).await?;
    let after = snapshot(&seeded.harness, &seeded.market.market_id).await?;
    assert_eq!(entries(&after), entries(before), "partial storage mutation");
    assert_eq!(after.storage_usage, before.storage_usage);
    assert_eq!(
        after.contract, before.contract,
        "failed batch must preserve the exact local/global linkage"
    );
    assert_eq!(
        after.code, before.code,
        "failed batch must not strand the account on PatchState"
    );
    assert_eq!(
        seeded
            .harness
            .get_configuration(&seeded.market.market_id)
            .await?,
        seeded.market.configuration
    );
    // Native gas balance and signer nonce intentionally change on a failed tx;
    // raw storage, code/linkage and the live configuration must not.
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "one release lifecycle compares raw checkpoints, sandbox replay and exact code restoration"
)]
async fn exercise_patch(restore: RestoreFixture, scenario: PatchScenario) -> Result<()> {
    captured_fixture()?;
    let seeded = SeededMarket::new().await?;
    let harness = &seeded.harness;
    let global = match restore {
        RestoreFixture::Local => None,
        RestoreFixture::GlobalHash => {
            let code = templar_gateway_testing::wasm::released(
                templar_gateway_testing::ArtifactId::Market,
                RELEASE,
            )
            .await;
            let hash = harness.deploy_global_contract(code).await?;
            Some(GlobalContractIdentifierInput::CodeHash(hash.into()))
        }
        RestoreFixture::GlobalAccount => {
            let publisher = harness.create_user("layout-publisher").await?;
            let code = templar_gateway_testing::wasm::released(
                templar_gateway_testing::ArtifactId::Market,
                RELEASE,
            )
            .await;
            Contract::deploy_global_contract_code(code)
                .as_account_id(publisher.0.clone())
                .with_signer(templar_gateway_testing::test_signer())
                .send_to(&harness.network)
                .await?
                .assert_success();
            Some(GlobalContractIdentifierInput::AccountId(publisher.0))
        }
    };
    if let Some(contract_identifier) = global {
        let use_global = harness
            .execute(
                &ManagedAccountId(seeded.market.market_id.clone()),
                tx::Batch {
                    receiver_id: seeded.market.market_id.clone(),
                    actions: vec![ActionInput::UseGlobalContract {
                        contract_identifier,
                    }],
                },
            )
            .await?;
        let finality = harness
            .client()?
            .read(tx::Get {
                tx_hash: use_global
                    .operation
                    .latest_tx_hash()
                    .context("UseGlobal transaction hash")?,
                sender_account_id: seeded.market.market_id.clone(),
                wait_until: Some(TxExecutionStatus::Final),
                encoding: tx::ValueEncoding::Json,
            })
            .await?;
        ensure!(
            finality.status == tx::Status::Succeeded && finality.failed_receipts.is_empty(),
            "UseGlobal must finalize including refunds before account checkpoints"
        );
    }
    harness.fast_forward(10).await?;
    let before = snapshot(harness, &seeded.market.market_id).await?;
    let dir = OutputDir::new()?;
    let spec = dir.0.join("patch.toml");
    let changes = if matches!(
        scenario,
        PatchScenario::Assets | PatchScenario::GuardMismatch
    ) {
        json!([
            {"op":"test","path":"/borrow_asset","value":seeded.market.configuration.borrow_asset},
            {"op":"replace","path":"/borrow_asset","value":{"Nep141":"new-borrow.test.near"}},
            {"op":"replace","path":"/collateral_asset","value":{"Nep245":{"contract_id":"new-collateral.test.near","token_id":"chain:asset:with:colons"}}}
        ])
    } else {
        json!([
            {"op":"test","path":"/borrow_mcr_maintenance","value":"1.4"},
            {"op":"replace","path":"/borrow_mcr_maintenance","value":"1.8"}
        ])
    };
    generate(&seeded, &changes, &spec).await?;
    let authored = snapshot(harness, &seeded.market.market_id).await?;
    assert_account_unchanged(&before, &authored);
    let blobs = dir.0.join("patch.blobs");
    assert_eq!(std::fs::read(blobs.join("old-state.bin"))?, state(&before)?);
    let expected_state = std::fs::read(blobs.join("new-state.bin"))?;
    let expected_configuration: MarketConfiguration =
        serde_json::from_slice(&std::fs::read(blobs.join("after.json"))?)?;
    let generated = crate::spec::patch::PatchSpec::load(&spec)?;
    let resolved = generated.resolve(&spec)?;
    assert_eq!(
        resolved.operations,
        vec![crate::spec::patch::ResolvedOperation::Set {
            key: Base64Bytes(b"STATE".to_vec()),
            value: Base64Bytes(expected_state.clone()),
            expected: Some(crate::spec::patch::ResolvedExpectation::Hash(
                crate::spec::patch::Sha256Digest(Sha256::digest(state(&before)?).into())
            )),
        }]
    );
    let checks = generated.view_checks().collect::<Vec<_>>();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].method_name.0, "get_configuration");
    assert_eq!(
        checks[0].expected()?,
        Some(&serde_json::to_value(&expected_configuration)?)
    );
    let envelope = market_state::decode(state(&before)?)?;
    assert_eq!(
        expected_state,
        envelope.replace(state(&before)?, &expected_configuration)?
    );
    let report: Value = serde_json::from_slice(&std::fs::read(blobs.join("report.json"))?)?;
    assert_eq!(report["effective_code_sha256"], WASM_SHA256);
    assert_eq!(report["old_state_sha256"], sha256(state(&before)?));
    assert_eq!(report["new_state_sha256"], sha256(&expected_state));
    assert_eq!(
        report["opaque_suffix_sha256"]["before"],
        report["opaque_suffix_sha256"]["after"]
    );
    let plan = dir.0.join("plan.json");
    let key = templar_gateway_testing::test_secret_key()?;
    dispatch(
        harness,
        &[
            "patch".into(),
            "plan".into(),
            spec.display().to_string(),
            "--out".into(),
            plan.display().to_string(),
            "--signer-id".into(),
            seeded.market.market_id.to_string(),
            "--public-key".into(),
            key.public_key().to_string(),
        ],
    )
    .await?;
    if matches!(scenario, PatchScenario::GuardMismatch) {
        let planned: crate::spec::patch_plan::PatchPlan =
            serde_json::from_slice(&std::fs::read(&plan)?)?;
        return assert_receipt_rollback(&seeded, &before, planned.batch).await;
    }
    dispatch(
        harness,
        &[
            "patch".into(),
            "dry-run".into(),
            "--plan".into(),
            plan.display().to_string(),
        ],
    )
    .await?;
    let stamped: crate::spec::patch_plan::PatchPlan =
        serde_json::from_slice(&std::fs::read(&plan)?)?;
    ensure!(
        stamped.dry_run.is_some(),
        "sandbox replay must stamp the plan"
    );
    dispatch(
        harness,
        &[
            "patch".into(),
            "apply".into(),
            "--plan".into(),
            plan.display().to_string(),
            "--signer-id".into(),
            seeded.market.market_id.to_string(),
            "--secret-key".into(),
            key.to_string(),
        ],
    )
    .await?;
    harness.fast_forward(10).await?;
    let after = snapshot(harness, &seeded.market.market_id).await?;
    let mut expected_entries = entries(&before);
    expected_entries.insert(b"STATE".to_vec(), expected_state);
    assert_eq!(
        entries(&after),
        expected_entries,
        "only the exact configuration span of STATE may change"
    );
    assert_eq!(
        after.contract, before.contract,
        "restore must retain exact local/global linkage"
    );
    assert_eq!(
        after.code, before.code,
        "restore must retain exact released code"
    );
    assert_eq!(
        harness.get_configuration(&seeded.market.market_id).await?,
        expected_configuration
    );
    assert_eq!(
        before
            .access_keys
            .iter()
            .map(|(key, _)| key)
            .collect::<Vec<_>>(),
        after
            .access_keys
            .iter()
            .map(|(key, _)| key)
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[tokio::test]
async fn requires_sandbox_config_patch_assets_local_restore() -> Result<()> {
    exercise_patch(RestoreFixture::Local, PatchScenario::Assets).await
}

#[tokio::test]
async fn requires_sandbox_config_patch_generic_global_restore() -> Result<()> {
    exercise_patch(RestoreFixture::GlobalHash, PatchScenario::Generic).await
}

#[tokio::test]
async fn requires_sandbox_config_patch_assets_global_account_restore() -> Result<()> {
    exercise_patch(RestoreFixture::GlobalAccount, PatchScenario::Assets).await
}

#[tokio::test]
async fn requires_sandbox_config_patch_receipt_guard_rolls_back_storage_and_code() -> Result<()> {
    for restore in [
        RestoreFixture::Local,
        RestoreFixture::GlobalHash,
        RestoreFixture::GlobalAccount,
    ] {
        exercise_patch(restore, PatchScenario::GuardMismatch).await?;
    }
    Ok(())
}

#[tokio::test]
async fn requires_sandbox_config_patch_invalid_documents_publish_nothing() -> Result<()> {
    captured_fixture()?;
    let seeded = SeededMarket::new().await?;
    let before = snapshot(&seeded.harness, &seeded.market.market_id).await?;
    let dir = OutputDir::new()?;
    let invalid = [
        json!([{"op":"test","path":"/protocol_account_id","value":"wrong.test.near"}]),
        json!([{"op":"replace","path":"/missing","value":1}]),
        json!([{"op":"replace","path":"/protocol_account_id/child","value":"bad.test.near"}]),
        json!([{"op":"replace","path":"","value":{}}]),
        json!([{"op":"replace","path":"/borrow_mcr_maintenance","value":"0"}]),
        json!([{"op":"replace","path":"/time_chunk_configuration","value":{"duration_ms":"2000"}}]),
        json!([{"op":"replace","path":"/borrow_interest_rate_strategy","value":{"Linear":{"base":"0.1","top":"0.2"}}}]),
        json!([{"op":"remove","path":"/protocol_account_id"}]),
        json!([{"op":"replace","path":"/protocol_account_id","value":seeded.market.configuration.protocol_account_id}]),
        json!([{"op":"replace","path":"/borrow_range","value":{"minimum":"1","maximum":null,"unknown":true}}]),
    ];
    for (index, changes) in invalid.iter().enumerate() {
        let output = dir.0.join(format!("invalid-{index}.toml"));
        assert!(
            generate(&seeded, changes, &output).await.is_err(),
            "invalid document {index} admitted"
        );
        assert!(!output.exists(), "invalid generation published a spec");
        assert!(
            !dir.0.join(format!("invalid-{index}.blobs")).exists(),
            "invalid generation left sidecars"
        );
    }
    // Publication must be no-clobber even when the conflicting file is not a spec.
    let output = dir.0.join("existing.toml");
    std::fs::write(&output, b"operator-owned")?;
    assert!(generate(
        &seeded,
        &json!([{"op":"replace","path":"/protocol_account_id","value":"new.test.near"}]),
        &output
    )
    .await
    .is_err());
    assert_eq!(std::fs::read(&output)?, b"operator-owned");
    let sidecar = dir.0.join("reserved.blobs");
    std::fs::create_dir(&sidecar)?;
    std::fs::write(sidecar.join("keep.bin"), b"operator-sidecar")?;
    let output = dir.0.join("reserved.toml");
    assert!(generate(
        &seeded,
        &json!([{"op":"replace","path":"/protocol_account_id","value":"new.test.near"}]),
        &output
    )
    .await
    .is_err());
    assert!(!output.exists());
    assert_eq!(
        std::fs::read(sidecar.join("keep.bin"))?,
        b"operator-sidecar"
    );
    let after = snapshot(&seeded.harness, &seeded.market.market_id).await?;
    assert_account_unchanged(&before, &after);
    let redeploy = seeded
        .harness
        .execute(
            &ManagedAccountId(seeded.market.market_id.clone()),
            tx::DeployContract {
                account_id: seeded.market.market_id.clone(),
                code: Base64Bytes(templar_gateway_testing::wasm::ft().await.to_vec()),
            },
        )
        .await?;
    finalize(&seeded.harness, &seeded.market.market_id, &redeploy).await?;
    let unsupported = snapshot(&seeded.harness, &seeded.market.market_id).await?;
    let output = dir.0.join("unsupported.toml");
    assert!(generate(
        &seeded,
        &json!([{"op":"replace","path":"/protocol_account_id","value":"new.test.near"}]),
        &output,
    )
    .await
    .is_err());
    assert!(!output.exists());
    assert!(!dir.0.join("unsupported.blobs").exists());
    assert_account_unchanged(
        &unsupported,
        &snapshot(&seeded.harness, &seeded.market.market_id).await?,
    );
    Ok(())
}

#[tokio::test]
async fn requires_sandbox_config_patch_rejects_code_drift_without_storage_drift() -> Result<()> {
    let seeded = SeededMarket::new().await?;
    let harness = &seeded.harness;
    let before = snapshot(harness, &seeded.market.market_id).await?;
    let dir = OutputDir::new()?;
    let spec = dir.0.join("patch.toml");
    generate(
        &seeded,
        &json!([{
            "op":"replace","path":"/protocol_account_id","value":"new-protocol.test.near"
        }]),
        &spec,
    )
    .await?;
    // A valid custom section changes code identity without changing behavior or STATE.
    let mut changed_code = before.code.clone();
    changed_code.extend_from_slice(b"\0\x02\x01x");
    let deployed = harness
        .execute(
            &ManagedAccountId(seeded.market.market_id.clone()),
            tx::Batch {
                receiver_id: seeded.market.market_id.clone(),
                actions: vec![ActionInput::DeployContract {
                    code: Base64Bytes(changed_code),
                }],
            },
        )
        .await?;
    finalize(harness, &seeded.market.market_id, &deployed).await?;
    let changed = snapshot(harness, &seeded.market.market_id).await?;
    assert_eq!(entries(&changed), entries(&before));
    assert_ne!(changed.code, before.code);
    let plan = dir.0.join("plan.json");
    let key = templar_gateway_testing::test_secret_key()?;
    let error = dispatch(
        harness,
        &[
            "patch".into(),
            "plan".into(),
            spec.display().to_string(),
            "--out".into(),
            plan.display().to_string(),
            "--signer-id".into(),
            seeded.market.market_id.to_string(),
            "--public-key".into(),
            key.public_key().to_string(),
        ],
    )
    .await
    .expect_err("code drift must refuse planning despite identical storage");
    let diagnostic = format!("{error:#}");
    for code in [&before.code, &changed.code] {
        let hash = templar_gateway_types::CryptoHash::from(near_api::types::CryptoHash::hash(code));
        assert!(diagnostic.contains(&hash.to_string()), "{diagnostic}");
    }
    assert!(!plan.exists());
    assert_eq!(
        entries(&snapshot(harness, &seeded.market.market_id).await?),
        entries(&changed)
    );
    Ok(())
}
