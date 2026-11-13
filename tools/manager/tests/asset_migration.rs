//! Actual shell/Python + compiled manager + real neard acceptance.
//! Generator byte/layout/atomic rollback boundaries are covered by `config_patch`
//! and signed-action/signature/nonce codec boundaries by `dao_mpc_patch` and the
//! `requires_sandbox_mpc` lifecycle target. These tests exercise their consumers.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::OpenOptions,
    io::Write as _,
    path::{Path, PathBuf},
    process::Output,
};

use anyhow::{bail, ensure, Context as _, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use near_api::Contract;
use serde::Deserialize as _;
use serde_json::{json, Value};
use sha2::Digest as _;
use templar_common::oracle::pyth::{Price, PriceIdentifier};
use templar_gateway_methods_spec::{
    account, chain, market, owner, proxy_oracle_governance as gov, tx,
};
use templar_gateway_types::{
    operation::ReceiptStatus, ManagedAccountId, NearToken, ProposalEncoding,
};

#[allow(
    dead_code,
    reason = "shared real fixtures expose helpers consumed by several integration targets"
)]
#[path = "common/migration.rs"]
mod fixture;
use fixture::{ChangedRoles, MigrationFixture, MigrationOptions, RestoreMode, TokenMode};

#[path = "common/state.rs"]
mod state_fixture;
use state_fixture::raw_storage;

const BIN: &str = env!("CARGO_BIN_EXE_tmplrmgr");
const FEEDS: [PriceIdentifier; 2] = [PriceIdentifier([0xbb; 32]), PriceIdentifier([0xcc; 32])];
const STAGES: [&str; 8] = [
    "00-prepare",
    "01-register",
    "02-pause",
    "03-patch",
    "04-swap",
    "05-reconcile",
    "06-resume",
    "07-finalize",
];

fn scripts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../script/asset-migration")
}

#[test]
fn structured_mt_components_are_not_reinterpreted_or_coerced() -> Result<()> {
    let output = std::process::Command::new("python3")
        .args([
            "-c",
            r#"
import sys
sys.path.insert(0, sys.argv[1])
from common import Stop, config_asset

expected = {"Nep245": {"contract_id": "tokens.near", "token_id": "prefix:suffix"}}
for asset in [
    "nep245:tokens.near:prefix:suffix",
    expected,
    {"standard": "mt", **expected["Nep245"]},
]:
    assert config_asset(asset) == expected, asset

for fields in [
    {"contract_id": "tokens.near:prefix", "token_id": "suffix"},
    {"contract_id": 123, "token_id": "suffix"},
    {"contract_id": "tokens.near", "token_id": 456},
    {"contract_id": "", "token_id": "suffix"},
    {"contract_id": "tokens.near", "token_id": ""},
]:
    for asset in [{"Nep245": fields}, {"standard": "mt", **fields}]:
        try:
            config_asset(asset)
        except Stop as error:
            assert error.code == 20, error
        else:
            raise AssertionError(f"accepted malformed structured MT asset: {asset!r}")
"#,
        ])
        .arg(scripts())
        .output()?;
    ensure!(
        output.status.success(),
        "asset normalization regression: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn load(path: impl AsRef<Path>) -> Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

/// Intentionally retained on both success and failure: failed receipt/artifact
/// evidence must survive a test panic, and subprocess logs name this directory.
struct Driver {
    root: PathBuf,
    run: PathBuf,
    campaign: PathBuf,
    state: PathBuf,
    refs: Option<PathBuf>,
    sequence: usize,
    binary: PathBuf,
}

impl Driver {
    fn new(fixture: &MigrationFixture, transport: &str) -> Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "asset-migration-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        std::fs::create_dir(&root)?;
        let campaign = root.join("campaign.toml");
        fixture.write_campaign(&campaign, transport)?;
        Ok(Self {
            run: root.join("run"),
            state: root.join("state"),
            root,
            campaign,
            refs: None,
            sequence: 0,
            binary: PathBuf::from(BIN),
        })
    }

    fn command(&self, stage: Option<&str>, submit: bool) -> Result<tokio::process::Command> {
        let mut command = if let Some(stage) = stage {
            let mut command = tokio::process::Command::new("python3");
            command.arg(scripts().join(format!("{stage}.py")));
            command
        } else {
            let mut command = tokio::process::Command::new("bash");
            command.arg(scripts().join("run.sh"));
            command
        };
        command.arg("--run-dir").arg(&self.run);
        if stage.is_none() || stage == Some("00-prepare") {
            command.arg("--campaign").arg(&self.campaign);
        }
        if submit {
            command.arg("--submit");
        }
        if let Some(refs) = &self.refs {
            command.arg("--execution-refs").arg(refs);
        }
        let key = templar_gateway_testing::test_secret_key()?.to_string();
        command
            .env("ASSET_MIGRATION_TMPLRMGR", &self.binary)
            .env("ASSET_MIGRATION_SUBMITTER_SECRET_KEY", &key)
            .env("ASSET_MIGRATION_RELAYER_SECRET_KEY", &key)
            .env("XDG_STATE_HOME", &self.state)
            .env_remove("SECRET_KEY");
        Ok(command)
    }

    async fn invoke(&mut self, stage: Option<&str>, submit: bool) -> Result<Output> {
        let output = self.command(stage, submit)?.output().await?;
        self.sequence += 1;
        let stem = format!("{:04}-{}", self.sequence, stage.unwrap_or("run"));
        write_new(&self.root.join(format!("{stem}.stdout")), &output.stdout)?;
        write_new(&self.root.join(format!("{stem}.stderr")), &output.stderr)?;
        Ok(output)
    }

    async fn expect(&mut self, stage: Option<&str>, submit: bool, code: i32) -> Result<Value> {
        let output = self.invoke(stage, submit).await?;
        ensure!(
            output.status.code() == Some(code),
            "{stage:?}: expected {code}, got {}; retained {}\nstdout={}\nstderr={}",
            output.status,
            self.root.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        // run.sh emits one structured result per stage, not one JSON document.
        let line = output
            .stdout
            .split(|byte| *byte == b'\n')
            .rev()
            .find(|line| !line.iter().all(u8::is_ascii_whitespace))
            .context("structured script stdout")?;
        Ok(serde_json::from_slice(line)?)
    }

    fn events(&self) -> Result<Vec<Value>> {
        let path = self.run.join("journal.jsonl");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let text = std::fs::read_to_string(path)?;
        ensure!(text.ends_with('\n'), "journal must retain complete lines");
        text.lines()
            .map(|line| Ok(serde_json::from_str(line)?))
            .collect()
    }

    fn confirmed(&self, stage: &str, key: &str) -> Result<Option<Value>> {
        Ok(self
            .events()?
            .into_iter()
            .rev()
            .find(|event| {
                event["stage"] == stage && event["key"] == key && event["operation"].is_null()
            })
            .filter(|event| event["event"] == "confirmed")
            .map(|event| event["evidence"].clone()))
    }

    fn financial_starts(&self) -> Result<Vec<Value>> {
        Ok(self
            .events()?
            .into_iter()
            .filter(|event| event["event"] == "attempt_started")
            .collect())
    }

    fn submitted_proposal_ids(&self) -> Result<Vec<u64>> {
        Ok(self
            .events()?
            .iter()
            .filter(|event| event["event"] == "submitted")
            .filter_map(|event| event["evidence"]["proposal_id"].as_u64())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }

    fn intent(&self) -> Result<Value> {
        load(self.run.join("intent.json"))
    }

    fn handoff(&mut self, fixture: &MigrationFixture) -> Result<()> {
        let path = self
            .root
            .join(format!("execution-refs-{:04}.json", self.sequence));
        fixture.write_execution_refs(&self.run, &path)?;
        self.refs = Some(path);
        Ok(())
    }

    /// Fixed finite stage driver, not an alternate workflow implementation.
    /// Every external wait is restarted once *without* voting first, proving
    /// the journal does not manufacture another financial operation.
    async fn drive_stage(&mut self, fixture: &mut MigrationFixture, stage: &str) -> Result<()> {
        for _ in 0..80 {
            let output = self.invoke(Some(stage), true).await?;
            if output.status.success() {
                return Ok(());
            }
            ensure!(
                output.status.code() == Some(10),
                "{stage}: {} at {}\n{}\n{}",
                output.status,
                self.root.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let starts = self.financial_starts()?;
            self.expect(Some(stage), true, 10).await?;
            ensure!(
                self.financial_starts()? == starts,
                "restart at wait submitted another financial operation"
            );
            fixture.approve_pending(&self.run).await?;
            self.handoff(fixture)?;
        }
        bail!(
            "external wait bound exhausted for {stage}; evidence {}",
            self.root.display()
        )
    }

    async fn through(&mut self, fixture: &mut MigrationFixture, last: &str) -> Result<()> {
        for stage in STAGES {
            self.drive_stage(fixture, stage).await?;
            if stage == last {
                return Ok(());
            }
        }
        bail!("unknown stage")
    }

    async fn assert_blocked(
        &mut self,
        fixture: &MigrationFixture,
        stage: &str,
        code: i32,
    ) -> Result<()> {
        let starts = self.financial_starts()?;
        let count = proposal_count(fixture).await?;
        self.expect(Some(stage), true, code).await?;
        self.expect(Some(stage), true, code).await?;
        ensure!(
            self.financial_starts()? == starts,
            "blocked restart attempted a write"
        );
        ensure!(
            proposal_count(fixture).await? == count,
            "blocked restart created a DAO proposal"
        );
        ensure!(
            self.confirmed(stage, "stage")?.is_none(),
            "failed stage claimed completion"
        );
        ensure!(
            self.confirmed("06-resume", "stage")?.is_none(),
            "adverse case resumed feeds"
        );
        Ok(())
    }
}

async fn cli(
    fixture: &MigrationFixture,
    args: &[&str],
    signer: Option<&ManagedAccountId>,
) -> Result<Output> {
    let mut command = tokio::process::Command::new(BIN);
    command
        .args([
            "--network",
            "testnet",
            "--rpc-url",
            fixture.seeded.harness.network.rpc_endpoints[0].url.as_str(),
            "-q",
        ])
        .args(args)
        .env_remove("SECRET_KEY");
    if let Some(signer) = signer {
        command.args([
            "--signer-id",
            signer.as_str(),
            "--secret-key",
            &templar_gateway_testing::test_secret_key()?.to_string(),
        ]);
    }
    Ok(command.output().await?)
}

async fn read(fixture: &MigrationFixture, method: &str, params: Value) -> Result<Value> {
    let output = cli(
        fixture,
        &["read", method, "--json", &params.to_string()],
        None,
    )
    .await?;
    ensure!(
        output.status.success(),
        "read {method}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

async fn proposal_count(fixture: &MigrationFixture) -> Result<u64> {
    fixture
        .seeded
        .harness
        .view_json(&fixture.dao, "get_last_proposal_id", json!({}))
        .await
}

async fn balance(fixture: &MigrationFixture, reference: &Value, account: &str) -> Result<u128> {
    Ok(read(
        fixture,
        "token.getBalanceOf",
        json!({"token":reference,"account_id":account}),
    )
    .await?
    .as_str()
    .context("canonical balance string")?
    .parse()?)
}

async fn identity(fixture: &MigrationFixture, account: &str) -> Result<Value> {
    let value = read(fixture, "account.get", json!({"account_id":account})).await?;
    Ok(
        json!({"code_hash":value["code_hash"],"global_contract_hash":value["global_contract_hash"],
        "global_contract_account_id":value["global_contract_account_id"]}),
    )
}

fn assert_receipts(receipts: &Value) -> Result<()> {
    ensure!(
        tx::Status::deserialize(&receipts["status"])? == tx::Status::Succeeded,
        "nonterminal/failed receipt result"
    );
    ensure!(
        receipts["failed_receipts"] == json!([]),
        "failed child receipts"
    );
    let graph = receipts["receipts"].as_array().context("receipt graph")?;
    for receipt in graph {
        ensure!(
            ReceiptStatus::deserialize(&receipt["status"])? == ReceiptStatus::Succeeded,
            "unsuccessful child {receipt}"
        );
    }
    Ok(())
}

async fn assert_manual(fixture: &MigrationFixture, expected: bool) -> Result<()> {
    for feed in FEEDS {
        let value = read(
            fixture,
            "proxyOracle.getProxyCircuitBreakerSet",
            json!({"oracle_id":fixture.proxy,"id":feed}),
        )
        .await?;
        ensure!(
            value["is_manually_tripped"] == expected,
            "manual feed state differs: {value}"
        );
    }
    Ok(())
}

async fn assert_final_inventory(
    fixture: &MigrationFixture,
    driver: &Driver,
    market: &Value,
    account: &str,
) -> Result<()> {
    for role in market["roles"].as_array().context("roles")? {
        let amount: u128 = role["initial_old_balance"]
            .as_str()
            .context("old balance")?
            .parse()?;
        let before: u128 = role["initial_new_balance"]
            .as_str()
            .context("new balance")?
            .parse()?;
        ensure!(
            balance(fixture, &role["old_reference"], account).await? == 0,
            "residual old inventory"
        );
        ensure!(
            balance(fixture, &role["new_reference"], account).await? == before + amount,
            "raw-unit conservation failed"
        );
        let key = format!(
            "{account}:{}",
            role["inventory_role"].as_str().context("inventory role")?
        );
        let swap = driver
            .confirmed("04-swap", &key)?
            .context("swap confirmation")?;
        ensure!(
            swap["zero_inventory"] == (amount == 0),
            "zero inventory evidence differs"
        );
        if amount > 0 {
            assert_receipts(&swap["receipts"])?;
        }
    }
    Ok(())
}

async fn assert_final_prices(
    fixture: &MigrationFixture,
    intent: &Value,
    paused: bool,
) -> Result<()> {
    for feed in FEEDS {
        let feed_id = feed.to_string();
        let age = intent["markets"]
            .as_array()
            .context("markets")?
            .iter()
            .map(|market| &market["expected_configuration"]["price_oracle_configuration"])
            .chain(
                intent["dependency_consumers"]
                    .as_array()
                    .context("dependency consumers")?
                    .iter()
                    .map(|consumer| &consumer["configuration"]["price_oracle_configuration"]),
            )
            .filter(|configuration| {
                configuration["account_id"].as_str() == Some(fixture.proxy.as_str())
                    && (configuration["borrow_asset_price_id"].as_str() == Some(feed_id.as_str())
                        || configuration["collateral_asset_price_id"].as_str()
                            == Some(feed_id.as_str()))
            })
            .map(|configuration| {
                configuration["price_maximum_age_s"]
                    .as_u64()
                    .context("market freshness bound")
            })
            .try_fold(u64::MAX, |age, value| value.map(|value| age.min(value)))?;
        ensure!(age != u64::MAX, "feed has no scoped market freshness bound");
        let cached = read(
            fixture,
            "proxyOracle.getCachedPrice",
            json!({"oracle_id":fixture.proxy,"id":feed,"age":age}),
        )
        .await?;
        if paused {
            ensure!(cached.is_null(), "initially paused feed falsely healthy");
        } else {
            let price: Price = serde_json::from_value(cached)
                .context("resumed feed lacks an actual cached price")?;
            ensure!(price.price.0 > 0, "resumed cached price is not positive");
            let published = price
                .publish_time
                .try_into_time()
                .context("invalid cached publication timestamp")?
                .as_ns();
            let now = fixture
                .seeded
                .harness
                .client()?
                .read(chain::GetBlock::default())
                .await?
                .timestamp_ns;
            let elapsed = now
                .checked_sub(published)
                .context("cached price is from the future")?;
            ensure!(
                elapsed
                    <= age
                        .checked_mul(1_000_000_000)
                        .context("freshness bound overflow")?,
                "resumed cached price exceeds the market freshness bound"
            );
        }
    }
    Ok(())
}

async fn final_assertions(fixture: &MigrationFixture, driver: &Driver, paused: bool) -> Result<()> {
    let intent = driver.intent()?;
    for market in intent["markets"].as_array().context("markets")? {
        let account = market["account"].as_str().context("market account")?;
        ensure!(
            read(
                fixture,
                "market.getConfiguration",
                json!({"market_id":account})
            )
            .await?
                == market["expected_configuration"],
            "final typed configuration differs"
        );
        ensure!(
            identity(fixture, account).await? == market["account_identity"],
            "restore linkage differs"
        );
        let patch = driver
            .confirmed("03-patch", account)?
            .context("patch domain confirmation")?;
        assert_receipts(&patch["receipts"])?;
        // Independent raw-state assertion, not the script's own boolean claim.
        let actual = raw_storage(&fixture.seeded.harness, &account.parse()?).await?;
        let baseline = exported_storage(
            &driver
                .run
                .join(patch["baseline"].as_str().context("baseline")?),
        )?;
        ensure!(
            actual == baseline,
            "conversion changed market storage/deposit accounting"
        );
        let initial = exported_storage(
            &driver.run.join(
                market["snapshot"]
                    .as_str()
                    .context("initial raw snapshot")?,
            ),
        )?;
        ensure!(
            initial
                .iter()
                .filter(|(key, _)| key.as_slice() != b"STATE")
                .eq(actual.iter().filter(|(key, _)| key.as_slice() != b"STATE")),
            "patch changed a nonconfiguration storage key"
        );
        let plan = driver
            .run
            .join(patch["plan"].as_str().context("stamped plan")?);
        let blobs = plan
            .parent()
            .context("patch directory")?
            .join("config-patch.blobs");
        ensure!(
            actual.get(b"STATE".as_slice()).context("new STATE")?
                == &std::fs::read(blobs.join("new-state.bin"))?,
            "actual STATE differs from frozen generator output"
        );
        let report = load(blobs.join("report.json"))?;
        let offsets = &report["configuration_offsets"];
        let start = usize::try_from(offsets["start"].as_u64().context("configuration start")?)?;
        let old_end = usize::try_from(
            offsets["old_end"]
                .as_u64()
                .context("old configuration end")?,
        )?;
        let new_end = usize::try_from(
            offsets["new_end"]
                .as_u64()
                .context("new configuration end")?,
        )?;
        let old_state = initial.get(b"STATE".as_slice()).context("old STATE")?;
        let new_state = actual.get(b"STATE".as_slice()).context("new STATE")?;
        ensure!(
            old_state.get(..start).context("old prefix")?
                == new_state.get(..start).context("new prefix")?
                && old_state.get(old_end..).context("old opaque suffix")?
                    == new_state.get(new_end..).context("new opaque suffix")?,
            "configuration patch failed prefix/suffix preservation"
        );
        assert_final_inventory(fixture, driver, market, account).await?;
    }
    ensure!(
        driver.confirmed("07-finalize", "stage")?.is_some(),
        "finalization missing"
    );
    assert_manual(fixture, paused).await?;
    assert_final_prices(fixture, &intent, paused).await
}

fn exported_storage(path: &Path) -> Result<BTreeMap<Vec<u8>, Vec<u8>>> {
    let document: toml::Value = toml::from_str(&std::fs::read_to_string(path)?)?;
    let mut result = BTreeMap::new();
    for entry in document["check"].as_array().context("export checks")? {
        let key = decode_bytes(&entry["key"], path.parent().context("export parent")?)?;
        let value = decode_bytes(&entry["expect"], path.parent().context("export parent")?)?;
        ensure!(
            result.insert(key, value).is_none(),
            "duplicate exported key"
        );
    }
    Ok(result)
}

fn decode_bytes(value: &toml::Value, base: &Path) -> Result<Vec<u8>> {
    if let Some(value) = value.get("base64").and_then(toml::Value::as_str) {
        return Ok(STANDARD.decode(value)?);
    }
    if let Some(value) = value.get("hex").and_then(toml::Value::as_str) {
        return Ok(hex::decode(value)?);
    }
    if let Some(value) = value.get("utf8").and_then(toml::Value::as_str) {
        return Ok(value.as_bytes().to_vec());
    }
    if let Some(value) = value.get("file").and_then(toml::Value::as_str) {
        let path = base.join(value).canonicalize()?;
        ensure!(
            path.starts_with(base.canonicalize()?),
            "export blob escapes directory"
        );
        return Ok(std::fs::read(path)?);
    }
    bail!("unsupported export byte expression {value}")
}

#[rstest::rstest]
#[case::borrow_ft(
    ChangedRoles::Borrow,
    TokenMode::Ft,
    TokenMode::Ft,
    RestoreMode::Local,
    "delegate-action",
    false,
    false
)]
#[case::collateral_mt(
    ChangedRoles::Collateral,
    TokenMode::Ft,
    TokenMode::Mt,
    RestoreMode::GlobalHash,
    "transaction",
    false,
    false
)]
#[case::both_mixed(
    ChangedRoles::Both,
    TokenMode::Mt,
    TokenMode::Ft,
    RestoreMode::GlobalAccount,
    "transaction",
    false,
    false
)]
#[case::cohort(
    ChangedRoles::Both,
    TokenMode::Ft,
    TokenMode::Mt,
    RestoreMode::Local,
    "delegate-action",
    false,
    true
)]
#[case::original_pause(
    ChangedRoles::Both,
    TokenMode::Mt,
    TokenMode::Mt,
    RestoreMode::GlobalAccount,
    "delegate-action",
    true,
    false
)]
#[tokio::test]
async fn requires_sandbox_scripts_complete_and_restart_without_duplicate_financial_writes(
    #[case] changes: ChangedRoles,
    #[case] borrow_mode: TokenMode,
    #[case] collateral_mode: TokenMode,
    #[case] restore: RestoreMode,
    #[case] transport: &str,
    #[case] initially_paused: bool,
    #[case] cohort: bool,
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions {
        changes,
        borrow_mode,
        collateral_mode,
        restore,
        initially_paused,
    })
    .await?;
    if cohort {
        fixture.add_shared_market(RestoreMode::GlobalHash).await?;
    }
    let mut driver = Driver::new(&fixture, transport)?;
    driver.expect(None, false, 10).await?;
    ensure!(
        driver.financial_starts()?.is_empty() && proposal_count(&fixture).await? == 0,
        "read-only shell invocation submitted a financial write"
    );
    // Exercise the actual shell entrypoint and every external wait, not helpers
    // imported into an in-process Python test.
    for _ in 0..100 {
        let output = driver.invoke(None, true).await?;
        if output.status.success() {
            break;
        }
        ensure!(
            output.status.code() == Some(10),
            "shell workflow failed {} at {}\n{}\n{}",
            output.status,
            driver.root.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let starts = driver.financial_starts()?;
        driver.expect(None, true, 10).await?;
        ensure!(
            driver.financial_starts()? == starts,
            "wait restart duplicated financial submission"
        );
        fixture.approve_pending(&driver.run).await?;
        driver.handoff(&fixture)?;
    }
    final_assertions(&fixture, &driver, initially_paused).await?;
    if cohort {
        ensure!(
            driver.intent()?["feeds"]
                .as_array()
                .context("cohort feeds")?
                .len()
                == 2,
            "shared feeds were duplicated"
        );
    }
    let governance_proposals = if initially_paused { 0 } else { 4 };
    for stage in ["02-pause", "06-resume"] {
        ensure!(
            driver
                .financial_starts()?
                .iter()
                .filter(|event| event["stage"] == stage)
                .count()
                == governance_proposals,
            "each unique feed must have exactly one governance create and one execute approval"
        );
    }
    let starts = driver.financial_starts()?;
    let count = proposal_count(&fixture).await?;
    driver.expect(None, true, 0).await?;
    driver.expect(None, false, 0).await?;
    ensure!(
        driver.financial_starts()? == starts && proposal_count(&fixture).await? == count,
        "completed restart duplicated writes"
    );
    final_assertions(&fixture, &driver, initially_paused).await
}

async fn governance_operation(fixture: &MigrationFixture, operation: Value) -> Result<Value> {
    let harness = &fixture.seeded.harness;
    let id = harness
        .client()?
        .read(gov::NextProposalId {
            governance_id: fixture.governance.clone(),
        })
        .await?;
    let admin = ManagedAccountId(fixture.dao.clone());
    let created = harness
        .execute(
            &admin,
            gov::CreateProposal {
                governance_id: fixture.governance.clone(),
                id,
                operation: serde_json::from_value(operation)?,
                requested_ttl: templar_common::Nanoseconds::zero(),
                encoding: ProposalEncoding::Json,
                gas: None,
            },
        )
        .await?;
    fixture::finalize(harness, &admin.0, &created).await?;
    harness.fast_forward(5).await?;
    let executed = harness
        .execute(
            &admin,
            gov::ExecuteProposal {
                governance_id: fixture.governance.clone(),
                id,
                gas: None,
            },
        )
        .await?;
    fixture::finalize(harness, &admin.0, &executed).await?;
    Ok(serde_json::to_value(executed)?)
}

async fn target_call(fixture: &MigrationFixture, method: &str, args: Value) -> Result<Value> {
    governance_operation(
        fixture,
        json!({"TargetFunctionCall":{
            "method_name":method,"args":STANDARD.encode(serde_json::to_vec(&args)?),
            "attached_deposit":"0","gas":"100000000000000"
        }}),
    )
    .await
}

async fn transfer_proxy_ownership(
    fixture: &MigrationFixture,
    next: &ManagedAccountId,
) -> Result<()> {
    governance_operation(
        fixture,
        json!({"TargetFunctionCall":{
            "method_name":"own_propose_owner",
            "args":STANDARD.encode(serde_json::to_vec(&json!({"account_id":next.0}))?),
            "attached_deposit":"1","gas":"100000000000000"
        }}),
    )
    .await?;
    let harness = &fixture.seeded.harness;
    let accepted = harness
        .execute(
            next,
            owner::AcceptOwner {
                contract_id: fixture.proxy.clone(),
            },
        )
        .await?;
    fixture::finalize(harness, &next.0, &accepted).await?;
    ensure!(
        harness
            .client()?
            .read(owner::GetOwner {
                contract_id: fixture.proxy.clone(),
            })
            .await?
            .as_ref()
            == Some(&next.0),
        "adverse ownership setup did not install the intended proxy owner"
    );
    Ok(())
}

async fn set_manual(fixture: &MigrationFixture, value: bool) -> Result<()> {
    for feed in FEEDS {
        target_call(
            fixture,
            "admin_set_manual_trip",
            json!({"id":feed,"is_manually_tripped":value,"metadata":null}),
        )
        .await?;
    }
    assert_manual(fixture, value).await
}

async fn set_converter(fixture: &MigrationFixture, mode: Value) -> Result<()> {
    fixture
        .seeded
        .harness
        .call_function(
            &fixture.proposer,
            &fixture.converter,
            "set_failure_mode",
            json!({"mode":mode}),
        )
        .await?;
    Ok(())
}

#[rstest::rstest]
#[case::wrong_chain("chain")]
#[case::wrong_genesis("genesis")]
#[case::bad_path("path")]
#[case::wrong_dao("dao")]
#[case::ineligible_submitter("submitter")]
#[case::unknown_route("route")]
#[case::wrong_converter_hash("converter")]
#[case::unreviewed_consumer("consumer")]
#[case::insufficient_bond("bond")]
#[case::insufficient_spendable_storage_funding("storage_funding")]
#[case::missing_key("key")]
#[case::wrong_governance_owner("owner")]
#[case::wrong_governance_target("target")]
#[case::missing_governance_role("role")]
#[tokio::test]
async fn requires_sandbox_scripts_reject_bad_admission_before_campaign_write(
    #[case] fault: &str,
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions::default()).await?;
    let harness = &fixture.seeded.harness;
    match fault {
        "chain" => fixture.campaign["expected_chain_id"] = json!("not-this-chain"),
        "genesis" => {
            fixture.campaign["expected_genesis_hash"] = json!("11111111111111111111111111111111");
        }
        "path" => fixture.campaign["markets"][0]["mpc_path"] = json!("wrong-installed-path"),
        "dao" => fixture.campaign["dao"] = json!(fixture.outsider.0),
        "submitter" => fixture.campaign["submitter"] = json!(fixture.outsider.0),
        "route" => {
            fixture.campaign["conversions"][0]["new_asset"] = json!(fixture.assets[1].new_cli);
        }
        "converter" => fixture.campaign["converter_code_sha256"] = json!("00".repeat(32)),
        "consumer" => fixture.campaign["external_consumers"] = json!([fixture.outsider.0]),
        "storage_funding" => {
            let account = harness
                .client()?
                .read(account::Get {
                    account_id: fixture.dao.clone(),
                })
                .await?;
            // The default sandbox protocol's storage backing is 10^19 yocto
            // per byte. Keep that backing and a small transfer-gas buffer, but
            // less spendable cash than the live 300Tgas registration budget.
            let backing = u128::from(account.storage_usage) * 10_000_000_000_000_000_000;
            let reserve = backing + NearToken::from_millinear(20).as_yoctonear();
            harness
                .execute(
                    &ManagedAccountId(fixture.dao.clone()),
                    templar_gateway_methods_spec::tx::Transfer {
                        receiver_id: fixture.outsider.0.clone(),
                        amount: NearToken::from_yoctonear(
                            account
                                .amount
                                .as_yoctonear()
                                .checked_sub(reserve)
                                .context("funded DAO")?,
                        ),
                    },
                )
                .await?;
        }
        "bond" => {
            let policy = json!({"roles":[
                {"name":"proposers","kind":{"Group":[fixture.proposer.0]},"permissions":["call:AddProposal"],"vote_policy":{}},
                {"name":"council","kind":{"Group":[fixture.council[0].0,fixture.council[1].0]},
                    "permissions":["*:VoteApprove"],"vote_policy":{}}
            ],"default_vote_policy":{"weight_kind":"RoleWeight","quorum":"2","threshold":"2"},
                "proposal_bond":NearToken::from_near(1_000_000),"proposal_period":"604800000000000",
                "bounty_bond":"0","bounty_forgiveness_period":"86400000000000"});
            // Reinitialization is forbidden. A second real DAO with the
            // unaffordable policy is installed as the fixture's controller.
            let dao = harness
                .deploy_sputnik_dao("unfundable-dao", serde_json::from_value(policy)?)
                .await?;
            governance_operation(
                &fixture,
                json!({"Reflexive":{"SetRole":{
                    "account_id":dao,"role":"Admin","set":true
                }}}),
            )
            .await?;
            fixture.campaign["dao"] = json!(dao);
            fixture.dao = dao;
            let controlled = ManagedAccountId(fixture.seeded.market.market_id.clone());
            let output = cli(
                &fixture,
                &[
                    "dao",
                    "mpc",
                    "install-key",
                    "--dao",
                    fixture.dao.as_str(),
                    "--mpc-contract",
                    fixture.mpc_contract.as_str(),
                ],
                Some(&controlled),
            )
            .await?;
            ensure!(output.status.success(), "setup unfundable DAO key");
        }
        "key" => {
            let output = cli(
                &fixture,
                &[
                    "dao",
                    "mpc",
                    "derive-key",
                    "--dao",
                    fixture.dao.as_str(),
                    "--mpc-contract",
                    fixture.mpc_contract.as_str(),
                    "--account-id",
                    fixture.seeded.market.market_id.as_str(),
                ],
                None,
            )
            .await?;
            ensure!(output.status.success(), "derive fixture key");
            let derived: Value = serde_json::from_slice(&output.stdout)?;
            harness
                .execute(
                    &ManagedAccountId(fixture.seeded.market.market_id.clone()),
                    account::DeleteKey {
                        public_key: serde_json::from_value(derived["public_key"].clone())?,
                    },
                )
                .await?;
        }
        "owner" => {
            transfer_proxy_ownership(&fixture, &fixture.outsider).await?;
        }
        "target" => {
            let (other_proxy, proxy_signer) = harness
                .create_account_via_tx("wrong-target-proxy", NearToken::from_near(100))
                .await?;
            Contract::deploy(other_proxy.clone())
                .use_code(templar_gateway_testing::wasm::proxy_oracle().await.to_vec())
                .with_init_call("new", json!({}))?
                .with_signer(proxy_signer)
                .wait_until(templar_gateway_core::FinalityPolicy::Final.transaction_status())
                .send_to(&harness.network)
                .await?
                .assert_success();
            let (wrong, signer) = harness
                .create_account_via_tx("wrong-target-governance", NearToken::from_near(100))
                .await?;
            Contract::deploy(wrong.clone())
                .use_code(templar_gateway_testing::wasm::proxy_governance().await.to_vec())
                .with_init_call("new",json!({
                    "proxy_oracle_id":other_proxy,"admin_id":fixture.dao,
                    "policy":{"reflexive_ttls":{"set_policy":"1000000000","set_role":"1000000000","self_upgrade":"1000000000"},
                        "default_target":{"ttl":"1000000000","role":"Admin"},"method_policies":{}}
                }))?
                .with_signer(signer)
                .wait_until(templar_gateway_core::FinalityPolicy::Final.transaction_status())
                .send_to(&harness.network)
                .await?
                .assert_success();
            // Fixture account keys make this adversarial ownership setup
            // possible without falsifying the governance contract's view.
            transfer_proxy_ownership(&fixture, &ManagedAccountId(wrong)).await?;
        }
        "role" => {
            governance_operation(
                &fixture,
                json!({"Reflexive":{"SetRole":{
                    "account_id":fixture.outsider.0,"role":"Admin","set":true
                }}}),
            )
            .await?;
            governance_operation(
                &fixture,
                json!({"Reflexive":{"SetRole":{
                    "account_id":fixture.dao,"role":"Admin","set":false
                }}}),
            )
            .await?;
        }
        _ => unreachable!(),
    }
    let original = raw_storage(harness, &fixture.seeded.market.market_id).await?;
    let original_identity = identity(&fixture, fixture.seeded.market.market_id.as_str()).await?;
    let count = proposal_count(&fixture).await?;
    let mut driver = Driver::new(&fixture, "delegate-action")?;
    driver.expect(Some("00-prepare"), false, 20).await?;
    ensure!(
        driver.financial_starts()?.is_empty(),
        "admission performed a financial submission"
    );
    ensure!(
        proposal_count(&fixture).await? == count,
        "admission created a proposal"
    );
    ensure!(
        raw_storage(harness, &fixture.seeded.market.market_id).await? == original,
        "admission mutated market storage"
    );
    ensure!(
        identity(&fixture, fixture.seeded.market.market_id.as_str()).await? == original_identity,
        "admission changed code"
    );
    Ok(())
}

#[tokio::test]
async fn requires_sandbox_scripts_existing_registrations_and_zero_inventory_submit_nothing_redundant(
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions {
        changes: ChangedRoles::Borrow,
        ..MigrationOptions::default()
    })
    .await?;
    fixture.register_destinations().await?;
    let market_id = fixture.seeded.market.market_id.clone();
    let inventory = read(&fixture,"token.getBalanceOf",json!({
        "token":{"standard":"ft","contract_id":fixture.seeded.market.borrow_ft_id},"account_id":market_id
    })).await?;
    let output = cli(
        &fixture,
        &[
            "write",
            "token.transfer",
            "--json",
            &json!({
                "token":{"standard":"ft","contract_id":fixture.seeded.market.borrow_ft_id},
                "receiver_id":fixture.converter,"amount":inventory
            })
            .to_string(),
        ],
        Some(&ManagedAccountId(market_id.clone())),
    )
    .await?;
    ensure!(output.status.success(), "setup zero inventory transfer");
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "07-finalize").await?;
    ensure!(
        !driver
            .financial_starts()?
            .iter()
            .any(|event| event["stage"] == "01-register" || event["stage"] == "04-swap"),
        "already registered/zero inventory created redundant financial operations"
    );
    final_assertions(&fixture, &driver, false).await
}

/// Read the actual broadcast retained by the scripts, not the DAO signing vote.
/// The old token's final used amount proves resolver completion independently
/// of the converter's explicitly nonauthoritative requested-refund event.
#[allow(
    clippy::too_many_lines,
    reason = "fault proof binds one retained broadcast to its native receipt graph and settlement"
)]
async fn bad_conversion_receipt_proof(
    fixture: &MigrationFixture,
    driver: &Driver,
    role: &Value,
    amount: u128,
    mode: &Value,
) -> Result<templar_gateway_methods_spec::tx::GetResult> {
    use templar_gateway_methods_spec::tx;
    use templar_gateway_types::{common::TxExecutionStatus, operation::ReceiptStatus};

    let key = format!(
        "{}:{}",
        fixture.seeded.market.market_id,
        role["role"].as_str().context("conversion role")?
    );
    let executions = driver
        .events()?
        .into_iter()
        .filter(|event| {
            event["stage"] == "04-swap"
                && event["key"] == key
                && event["operation"] == "mpc"
                && event["event"] == "submitted"
                && event["evidence"]["phase"] == "execute"
        })
        .collect::<Vec<_>>();
    ensure!(executions.len() == 1, "fault must broadcast exactly once");
    let evidence = &executions[0]["evidence"];
    let retained = load(
        driver.run.join(
            evidence["result"]
                .as_str()
                .context("retained execution result")?,
        ),
    )?;
    let reference = &evidence["transaction"];
    ensure!(
        retained["output"]["tx_hash"] == reference["tx_hash"]
            && retained["output"]["sender_account_id"] == reference["sender_account_id"],
        "execution reference must come from the actual native broadcast output"
    );
    let result = fixture
        .seeded
        .harness
        .client()?
        .read(tx::Get {
            tx_hash: serde_json::from_value(reference["tx_hash"].clone())?,
            sender_account_id: serde_json::from_value(reference["sender_account_id"].clone())?,
            wait_until: Some(TxExecutionStatus::Final),
            encoding: tx::ValueEncoding::Json,
        })
        .await?;
    write_new(
        &driver.root.join(format!(
            "bad-conversion-{}.json",
            role["role"].as_str().context("role")?
        )),
        &serde_json::to_vec_pretty(&result)?,
    )?;
    ensure!(
        result.status == tx::Status::Succeeded,
        "old token resolver did not finish: {result:?}"
    );
    let transaction = result
        .transaction
        .as_ref()
        .context("actual broadcast transaction")?;
    ensure!(
        serde_json::to_value(transaction.hash)? == reference["tx_hash"]
            && transaction.signer_id == fixture.seeded.market.market_id
            && transaction.receiver_id.as_str()
                == role["old_reference"]["contract_id"]
                    .as_str()
                    .context("old token")?
            && transaction.action_count == 1
            && transaction.function_calls.len() == 1,
        "fault did not execute the market's exact token transaction"
    );
    let call = &transaction.function_calls[0];
    let args: Value = serde_json::from_slice(&call.args.0)?;
    let asset = fixture
        .assets
        .iter()
        .find(|asset| role["role"] == asset.role)
        .context("conversion route")?;
    let message: Value = serde_json::from_str(args["msg"].as_str().context("conversion message")?)?;
    ensure!(
        message == json!({"route":asset.route_id}),
        "wrong converter route"
    );
    ensure!(
        call.index == 0
            && call.method_name.0
                == if role["old_reference"]["standard"] == "mt" {
                    "mt_transfer_call"
                } else {
                    "ft_transfer_call"
                }
            && args["receiver_id"] == json!(fixture.converter)
            && args["amount"] == json!(amount.to_string()),
        "fault broadcast differs from the converter input"
    );
    if role["old_reference"]["standard"] == "mt" {
        ensure!(
            args["token_id"] == role["old_reference"]["token_id"],
            "wrong input token ID"
        );
    }
    let used = match mode.as_str() {
        Some("Panic" | "RefundAll") => 0,
        Some("RefundPartial") => amount / 2,
        _ => amount,
    };
    let used_value = if role["old_reference"]["standard"] == "mt" {
        json!([used.to_string()])
    } else {
        json!(used.to_string())
    };
    ensure!(
        result.return_value == Some(tx::ReturnValue::Json(used_value.clone()))
            && result.receipts.iter().any(|receipt| {
                receipt.executor_id == transaction.receiver_id
                    && receipt.status == ReceiptStatus::Succeeded
                    && receipt.return_value == Some(tx::ReturnValue::Json(used_value.clone()))
            }),
        "authoritative old-token resolver used amount differs: {result:?}"
    );
    let panic = mode == &json!("Panic");
    ensure!(
        result.failed_receipts
            == if panic {
                vec![fixture.converter.clone()]
            } else {
                Vec::new()
            }
            && result.receipts.iter().all(|receipt| {
                receipt.status
                    == if panic && receipt.executor_id == fixture.converter {
                        ReceiptStatus::Failed
                    } else {
                        ReceiptStatus::Succeeded
                    }
            })
            && result
                .receipts
                .iter()
                .any(|receipt| receipt.executor_id == fixture.converter),
        "converter panic and successful refund/output paths must be distinguishable: {result:?}"
    );
    let mut settlements = Vec::new();
    for receipt in result
        .receipts
        .iter()
        .filter(|receipt| receipt.executor_id == fixture.converter)
    {
        for log in &receipt.logs {
            if let Some(event) = log.strip_prefix("EVENT_JSON:") {
                let event: Value = serde_json::from_str(event)?;
                if event["standard"] == "templar_test_asset_converter" {
                    ensure!(
                        event["version"] == "1.0.0"
                            && event["event"] == "settlement"
                            && event["data"].as_array().context("settlement data")?.len() == 1,
                        "unexpected converter event"
                    );
                    settlements.push(event["data"][0].clone());
                }
            }
        }
    }
    if panic {
        ensure!(
            settlements.is_empty(),
            "panicked converter emitted a completed settlement"
        );
    } else {
        ensure!(
            settlements.len() == 1,
            "converter must emit exactly one actual settlement"
        );
        let settlement = &settlements[0];
        let recipient = mode
            .get("WrongRecipient")
            .cloned()
            .unwrap_or_else(|| json!(transaction.signer_id));
        let output_asset = mode.get("WrongOutputToken").unwrap_or(&asset.new_asset);
        let output = if mode == &json!("MissingOutput") {
            0
        } else {
            used
        };
        ensure!(
            settlement["mode"] == *mode
                && settlement["route"] == json!(asset.route_id)
                && settlement["sender"] == json!(transaction.signer_id)
                && settlement["recipient"] == recipient
                && settlement["old_asset"] == asset.old_asset
                && settlement["new_asset"] == *output_asset
                && settlement["input"] == json!(amount.to_string())
                && settlement["consumed"] == json!(used.to_string())
                && settlement["refunded"] == json!((amount - used).to_string())
                && settlement["output"] == json!(output.to_string())
                && settlement["refund_is_requested"] == true
                && settlement["output_succeeded"] == (output > 0),
            "actual converter settlement differs from injected fault: {settlement}"
        );
    }
    Ok(result)
}

fn bad_conversion_ft_output(
    receipts: &templar_gateway_methods_spec::tx::GetResult,
    token: &Value,
    sender: &str,
    recipient: &str,
    amount: u128,
) -> Result<()> {
    ensure!(
        token["standard"] == "ft",
        "this fault fixture requires FT output"
    );
    for receipt in &receipts.receipts {
        if receipt.executor_id.as_str() != token["contract_id"].as_str().context("output token")? {
            continue;
        }
        for log in &receipt.logs {
            if let Some(event) = log.strip_prefix("EVENT_JSON:") {
                let event: Value = serde_json::from_str(event)?;
                if event["standard"] == "nep141"
                    && event["event"] == "ft_transfer"
                    && event["data"]
                        .as_array()
                        .context("FT event data")?
                        .iter()
                        .any(|transfer| {
                            transfer["old_owner_id"] == sender
                                && transfer["new_owner_id"] == recipient
                                && transfer["amount"] == json!(amount.to_string())
                        })
                {
                    return Ok(());
                }
            }
        }
    }
    bail!("missing actual output-token transfer to the fault recipient")
}

#[rstest::rstest]
#[case::panic("Panic")]
#[case::full_refund("RefundAll")]
#[case::partial_refund("RefundPartial")]
#[case::missing_output("MissingOutput")]
#[case::wrong_recipient("wrong_recipient")]
#[case::wrong_token("wrong_token")]
#[case::deposit_back("DepositBack")]
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "each actual conversion fault retains its settlement, balance, storage and restart evidence"
)]
async fn requires_sandbox_scripts_bad_conversion_never_reconciles_or_unpauses(
    #[case] fault: &str,
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions {
        changes: ChangedRoles::Borrow,
        ..MigrationOptions::default()
    })
    .await?;
    if fault == "DepositBack" {
        fixture
            .seeded
            .harness
            .storage_deposit_min(
                &ManagedAccountId(fixture.converter.clone()),
                &fixture.seeded.market.market_id,
            )
            .await?;
    }
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "03-patch").await?;
    let intent = driver.intent()?;
    let role = &intent["markets"][0]["roles"][0];
    let account = intent["markets"][0]["account"]
        .as_str()
        .context("market account")?;
    let old = balance(&fixture, &role["old_reference"], account).await?;
    let new = balance(&fixture, &role["new_reference"], account).await?;
    let baseline = raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await?;
    let mode = match fault {
        "wrong_recipient" => json!({"WrongRecipient":fixture.outsider.0}),
        "wrong_token" => json!({"WrongOutputToken":fixture.assets[0].old_asset}),
        other => json!(other),
    };
    if fault == "wrong_recipient" {
        fixture
            .seeded
            .harness
            .storage_deposit(
                &fixture.outsider,
                role["new_reference"]["contract_id"]
                    .as_str()
                    .context("new contract")?
                    .parse::<near_account_id::AccountId>()?,
                NearToken::from_millinear(10),
            )
            .await?;
    }
    let outsider_before = balance(
        &fixture,
        &role["new_reference"],
        fixture.outsider.0.as_str(),
    )
    .await?;
    set_converter(&fixture, mode.clone()).await?;
    // The bad settlement is submitted exactly once, after a real signature.
    driver.expect(Some("04-swap"), true, 10).await?;
    fixture.approve_pending(&driver.run).await?;
    driver.handoff(&fixture)?;
    driver.expect(Some("04-swap"), true, 20).await?;
    let receipts = bad_conversion_receipt_proof(&fixture, &driver, role, old, &mode).await?;
    driver.assert_blocked(&fixture, "04-swap", 20).await?;
    driver.expect(Some("05-reconcile"), true, 20).await?;
    driver.expect(Some("06-resume"), true, 20).await?;
    ensure!(
        driver
            .confirmed("04-swap", &format!("{account}:borrow"))?
            .is_none()
            && driver.confirmed("05-reconcile", "stage")?.is_none(),
        "bad conversion claimed settlement or reconciliation"
    );
    assert_manual(&fixture, true).await?;
    let old_after = balance(&fixture, &role["old_reference"], account).await?;
    let new_after = balance(&fixture, &role["new_reference"], account).await?;
    match fault {
        "Panic" | "RefundAll" => ensure!(
            old_after == old && new_after == new,
            "full refund did not restore actual balances"
        ),
        "RefundPartial" => ensure!(
            old_after == old - old / 2 && new_after == new + old / 2,
            "partial settlement did not match exact resolver units"
        ),
        "MissingOutput" => ensure!(
            old_after == 0 && new_after == new,
            "missing-output scenario was not exercised"
        ),
        "wrong_recipient" => {
            ensure!(
                old_after == 0
                    && new_after == new
                    && balance(
                        &fixture,
                        &role["new_reference"],
                        fixture.outsider.0.as_str()
                    )
                    .await?
                        == outsider_before + old,
                "wrong-recipient output did not reach the actual outsider"
            );
            bad_conversion_ft_output(
                &receipts,
                &role["new_reference"],
                fixture.converter.as_str(),
                fixture.outsider.0.as_str(),
                old,
            )?;
        }
        "DepositBack" => ensure!(
            raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await?
                != baseline,
            "deposit-back must exercise real receiver/storage mutation"
        ),
        "wrong_token" => {
            ensure!(
                old_after == old && new_after == new,
                "wrong-token route did not return the actual old token"
            );
            bad_conversion_ft_output(
                &receipts,
                &role["old_reference"],
                fixture.converter.as_str(),
                account,
                old,
            )?;
        }
        _ => unreachable!(),
    }
    if fault != "DepositBack" {
        ensure!(
            raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await?
                == baseline,
            "conversion changed raw market storage"
        );
    }
    Ok(())
}

#[tokio::test]
async fn requires_sandbox_scripts_second_role_failure_retains_first_terminal_conversion(
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions::default()).await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "03-patch").await?;
    driver.expect(Some("04-swap"), true, 10).await?;
    fixture.approve_pending(&driver.run).await?;
    driver.handoff(&fixture)?;
    // First swap executes, then the second independent signature waits.
    driver.expect(Some("04-swap"), true, 10).await?;
    let account = fixture.seeded.market.market_id.to_string();
    let first = driver
        .confirmed("04-swap", &format!("{account}:borrow"))?
        .context("first conversion must be confirmed")?;
    assert_receipts(&first["receipts"])?;
    let roles = driver.intent()?["markets"][0]["roles"]
        .as_array()
        .context("roles")?
        .clone();
    let borrow_role = roles
        .iter()
        .find(|role| role["role"] == "borrow")
        .context("borrow role")?;
    ensure!(
        balance(&fixture, &borrow_role["old_reference"], &account).await? == 0,
        "first swap incomplete"
    );
    let first_balance = balance(&fixture, &borrow_role["new_reference"], &account).await?;
    let collateral_role = roles
        .iter()
        .find(|role| role["role"] == "collateral")
        .context("collateral role")?;
    let collateral_old = balance(&fixture, &collateral_role["old_reference"], &account).await?;
    let collateral_new = balance(&fixture, &collateral_role["new_reference"], &account).await?;
    let baseline = raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await?;
    set_converter(&fixture, json!("Panic")).await?;
    fixture.approve_pending(&driver.run).await?;
    driver.handoff(&fixture)?;
    driver.expect(Some("04-swap"), true, 20).await?;
    bad_conversion_receipt_proof(
        &fixture,
        &driver,
        collateral_role,
        collateral_old,
        &json!("Panic"),
    )
    .await?;
    driver.assert_blocked(&fixture, "04-swap", 20).await?;
    driver.expect(Some("05-reconcile"), true, 20).await?;
    driver.expect(Some("06-resume"), true, 20).await?;
    ensure!(
        driver
            .confirmed("04-swap", &format!("{account}:collateral"))?
            .is_none()
            && driver.confirmed("05-reconcile", "stage")?.is_none(),
        "failed second role claimed settlement or reconciliation"
    );
    ensure!(
        balance(&fixture, &collateral_role["old_reference"], &account).await? == collateral_old
            && balance(&fixture, &collateral_role["new_reference"], &account).await?
                == collateral_new
            && raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await?
                == baseline,
        "second-role panic failed to preserve refunded inventory and raw market state"
    );
    ensure!(
        driver.confirmed("04-swap", &format!("{account}:borrow"))? == Some(first),
        "successful first conversion rewritten"
    );
    ensure!(
        balance(&fixture, &borrow_role["new_reference"], &account).await? == first_balance,
        "first swap rolled back/repeated"
    );
    assert_manual(&fixture, true).await
}

#[rstest::rstest]
#[case::user_deposit("deposit")]
#[case::accrual("accrual")]
#[case::static_yield("yield")]
#[case::debt_free_collateral("collateral")]
#[tokio::test]
async fn requires_sandbox_scripts_detect_actual_unpriced_mutations(
    #[case] mutation: &str,
) -> Result<()> {
    let mut fixture = MigrationFixture::new_with_terms(
        MigrationOptions::default(),
        604_800_000_000_000,
        (mutation == "accrual")
            .then(|| serde_json::from_value(json!({"Linear":{"base":"1000","top":"1000"}})))
            .transpose()?,
        None,
    )
    .await?;
    let harness = &fixture.seeded.harness;
    let debt_free = if mutation == "collateral" {
        let account = harness.create_user("unpriced-debt-free").await?;
        harness.fund_user(&account, &fixture.seeded.market).await?;
        harness
            .storage_deposit_min(&account, &fixture.seeded.market.market_id)
            .await?;
        harness
            .collateralize(&account, &fixture.seeded.market, 1_000)
            .await?;
        Some(account)
    } else {
        None
    };
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "02-pause").await?;
    let before = raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await?;
    let harness = &fixture.seeded.harness;
    let supplier = ManagedAccountId(
        fixture.seeded.observations["supplier"]["account_id"]
            .as_str()
            .context("supplier")?
            .parse()?,
    );
    let protocol = ManagedAccountId(
        fixture.seeded.observations["static_yield"]["account_id"]
            .as_str()
            .context("protocol")?
            .parse()?,
    );
    match mutation {
        "deposit" => {
            harness
                .supply(&supplier, &fixture.seeded.market, 100)
                .await?;
        }
        "accrual" => {
            let borrower = fixture.seeded.observations["borrower"]["account_id"]
                .as_str()
                .context("borrower")?
                .parse()?;
            let before_interest = harness
                .get_borrow_position(&fixture.seeded.market, &borrower)
                .await?
                .context("borrow position")?
                .interest
                .get_total();
            harness.fast_forward(20).await?;
            fixture::finalize(
                harness,
                &fixture.proposer.0,
                &harness
                    .execute(
                        &fixture.proposer,
                        market::ApplyInterest {
                            market_id: fixture.seeded.market.market_id.clone(),
                            account_id: Some(borrower.clone()),
                            snapshot_limit: None,
                        },
                    )
                    .await?,
            )
            .await?;
            ensure!(
                harness
                    .get_borrow_position(&fixture.seeded.market, &borrower)
                    .await?
                    .context("borrow position")?
                    .interest
                    .get_total()
                    > before_interest,
                "explicit unpriced accrual must increase actual borrower interest"
            );
        }
        "yield" => {
            harness
                .withdraw_static_yield(&protocol, &fixture.seeded.market, Some(1))
                .await?;
        }
        "collateral" => {
            harness
                .withdraw_collateral(
                    debt_free
                        .as_ref()
                        .context("missing debt-free collateral fixture")?,
                    &fixture.seeded.market,
                    100,
                )
                .await?;
        }
        _ => unreachable!(),
    }
    ensure!(
        raw_storage(harness, &fixture.seeded.market.market_id).await? != before,
        "adverse operation did not mutate actual market state"
    );
    driver.assert_blocked(&fixture, "03-patch", 20).await?;
    assert_manual(&fixture, true).await
}

#[rstest::rstest]
#[case::storage("storage")]
#[case::code("code")]
#[case::key("key")]
#[case::external_nonce("nonce")]
#[case::immutable_artifact("artifact")]
#[case::referenced_blob("blob")]
#[case::global_restore_identity("restore")]
#[case::inventory("inventory")]
#[tokio::test]
async fn requires_sandbox_scripts_drift_while_signature_waits_blocks_approved_patch(
    #[case] drift: &str,
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions {
        restore: if drift == "restore" {
            RestoreMode::GlobalAccount
        } else {
            RestoreMode::Local
        },
        ..MigrationOptions::default()
    })
    .await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "02-pause").await?;
    driver.expect(Some("03-patch"), true, 10).await?;
    let intent = driver.intent()?;
    let market = &intent["markets"][0];
    let controlled = ManagedAccountId(fixture.seeded.market.market_id.clone());
    let configuration = read(
        &fixture,
        "market.getConfiguration",
        json!({"market_id":controlled.0}),
    )
    .await?;
    match drift {
        "storage" => {
            let supplier = ManagedAccountId(
                fixture.seeded.observations["supplier"]["account_id"]
                    .as_str()
                    .context("supplier")?
                    .parse()?,
            );
            fixture
                .seeded
                .harness
                .create_supply_withdrawal_request(&supplier, &fixture.seeded.market, 2_000)
                .await?;
        }
        "code" => {
            Contract::deploy(controlled.0.clone())
                .use_code(templar_gateway_testing::wasm::mock_signer().await.to_vec())
                .without_init_call()
                .with_signer(templar_gateway_testing::test_signer())
                .send_to(&fixture.seeded.harness.network)
                .await?
                .assert_success();
        }
        "key" => {
            fixture
                .seeded
                .harness
                .execute(
                    &controlled,
                    account::DeleteKey {
                        public_key: serde_json::from_value(market["public_key"].clone())?,
                    },
                )
                .await?;
        }
        "nonce" => {
            // A real transaction using the other installed FAK consumes its
            // nonce while leaving the market's storage untouched.
            let before = raw_storage(&fixture.seeded.harness, &controlled.0).await?;
            fixture
                .seeded
                .harness
                .call_function(&controlled, &controlled.0, "get_configuration", json!({}))
                .await?;
            ensure!(
                raw_storage(&fixture.seeded.harness, &controlled.0).await? == before,
                "nonce injection changed storage"
            );
        }
        "restore" => {
            let publisher = market["account_identity"]["global_contract_account_id"]
                .as_str()
                .context("global-account restore publisher")?
                .parse()?;
            Contract::deploy_global_contract_code(
                templar_gateway_testing::wasm::mock_signer().await.to_vec(),
            )
            .as_account_id(publisher)
            .with_signer(templar_gateway_testing::test_signer())
            .send_to(&fixture.seeded.harness.network)
            .await?
            .assert_success();
        }
        "blob" => {
            let report = driver.run.join(
                market["layout_report"]
                    .as_str()
                    .context("generator report")?,
            );
            let path = report
                .parent()
                .context("blobs directory")?
                .join("new-state.bin");
            let mut file = OpenOptions::new().append(true).open(path)?;
            file.write_all(b"\0")?;
            file.sync_all()?;
        }
        "artifact" => {
            let path = driver
                .run
                .join(market["changes"].as_str().context("changes artifact")?);
            let mut file = OpenOptions::new().append(true).open(path)?;
            file.write_all(b"\n ")?;
            file.sync_all()?;
        }
        "inventory" => {
            fixture
                .seeded
                .harness
                .mint(&controlled, &fixture.seeded.market.borrow_ft_id, 1)
                .await?;
        }
        _ => unreachable!(),
    }
    fixture.approve_pending(&driver.run).await?;
    driver.handoff(&fixture)?;
    driver.assert_blocked(&fixture, "03-patch", 20).await?;
    if !matches!(drift, "code" | "restore") {
        ensure!(
            read(
                &fixture,
                "market.getConfiguration",
                json!({"market_id":controlled.0})
            )
            .await?
                == configuration,
            "approved patch executed despite drift"
        );
    }
    assert_manual(&fixture, true).await
}

#[rstest::rstest]
#[case::refusal(false)]
#[case::expired(true)]
#[tokio::test]
async fn requires_sandbox_scripts_signer_refusal_and_expiry_do_not_patch(
    #[case] expired: bool,
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions::default()).await?;
    if expired {
        fixture.campaign["mpc_valid_for_blocks"] = json!(500);
    }
    let mut driver = Driver::new(&fixture, "delegate-action")?;
    driver.through(&mut fixture, "02-pause").await?;
    driver.expect(Some("03-patch"), true, 10).await?;
    let baseline = raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await?;
    let original_identity = identity(&fixture, fixture.seeded.market.market_id.as_str()).await?;
    if expired {
        fixture.approve_pending(&driver.run).await?;
        fixture.seeded.harness.fast_forward(1_000).await?;
    } else {
        fixture
            .seeded
            .harness
            .call_function(
                &fixture.outsider,
                &fixture.mpc_contract,
                "set_refuse",
                json!({"refuse":true}),
            )
            .await?;
        fixture.approve_pending(&driver.run).await?;
    }
    driver.handoff(&fixture)?;
    let starts = driver.financial_starts()?;
    driver.expect(Some("03-patch"), true, 20).await?;
    ensure!(
        driver.financial_starts()? == starts,
        "refused/expired signature broadcast a patch"
    );
    ensure!(
        raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await? == baseline,
        "failed signature changed state"
    );
    ensure!(
        identity(&fixture, fixture.seeded.market.market_id.as_str()).await? == original_identity,
        "failed signature stranded patch code"
    );
    ensure!(
        driver.confirmed("03-patch", "stage")?.is_none(),
        "failed signature claimed patch completion"
    );
    assert_manual(&fixture, true).await
}

#[rstest::rstest]
#[case::rejected("rejected")]
#[case::collision("collision")]
#[case::target_child_failure("child")]
#[tokio::test]
async fn requires_sandbox_scripts_governance_failure_requires_real_target_success(
    #[case] fault: &str,
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions::default()).await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "01-register").await?;
    driver.expect(Some("02-pause"), true, 10).await?;
    let ids = driver.submitted_proposal_ids()?;
    let id = *ids.last().context("pause DAO proposal")?;
    let stored = fixture.proposal(id).await?;
    let before = raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await?;
    match fault {
        "rejected" => {
            let unauthorized = fixture
                .seeded
                .harness
                .try_call_function(
                    &fixture.outsider,
                    &fixture.dao,
                    "act_proposal",
                    json!({"id":id,"action":"VoteApprove","proposal":stored["kind"],"memo":null}),
                )
                .await?;
            ensure!(
                templar_gateway_testing::failed_receipts(&unauthorized)
                    .next()
                    .is_some(),
                "unauthorized actor passed Sputnik policy"
            );
            ensure!(
                fixture.proposal(id).await?["votes"] == stored["votes"],
                "unauthorized vote changed tally"
            );
            for voter in &fixture.council {
                fixture.seeded.harness.call_function(voter,&fixture.dao,"act_proposal",
                    json!({"id":id,"action":"VoteReject","proposal":stored["kind"],"memo":null})).await?;
            }
            ensure!(
                fixture.proposal(id).await?["status"] == "Rejected",
                "two-member rejection not exercised"
            );
        }
        "collision" => {
            let calls = stored["kind"]["FunctionCall"]["actions"]
                .as_array()
                .context("governance calls")?;
            let args: Value = serde_json::from_slice(
                &STANDARD.decode(calls[0]["args"].as_str().context("call args")?)?,
            )?;
            fixture.seeded.harness.execute(&ManagedAccountId(fixture.dao.clone()),gov::CreateProposal {
                governance_id:fixture.governance.clone(),id:args["id"].as_u64().context("governance ID")?.try_into()?,
                operation:serde_json::from_value(json!({"TargetFunctionCall":{
                    "method_name":"admin_set_manual_trip","args":STANDARD.encode(serde_json::to_vec(&json!({
                        "id":FEEDS[1],"is_manually_tripped":false,"metadata":null
                    }))?),"attached_deposit":"0","gas":"50000000000000"
                }}))?,requested_ttl:templar_common::Nanoseconds::zero(),encoding:ProposalEncoding::Json,
                gas: None,
            }).await?;
        }
        "child" => {
            fixture.approve_pending(&driver.run).await?;
            driver.handoff(&fixture)?;
            driver.expect(Some("02-pause"), true, 10).await?;
            // Governance execution is already approved as a *plan*, but its
            // detached target will lose authorization before the final vote.
            transfer_proxy_ownership(&fixture, &fixture.outsider).await?;
            fixture.approve_pending(&driver.run).await?;
            driver.handoff(&fixture)?;
            let id = *driver
                .submitted_proposal_ids()?
                .last()
                .context("execute proposal")?;
            ensure!(
                fixture.proposal(id).await?["status"] == "Approved",
                "target-failure case did not approve outer DAO"
            );
            let proof = fixture.execution_votes.get(&id).context("final vote")?;
            let receipts = read(&fixture,"tx.get",json!({
                "tx_hash":proof["tx_hash"],"sender_account_id":proof["sender_account_id"],"encoding":"json"
            })).await?;
            ensure!(!receipts["failed_receipts"].as_array().context("failed children")?.is_empty(),
                "target-child failure must be real receipt evidence, not merely an ownership precheck");
        }
        _ => unreachable!(),
    }
    driver.assert_blocked(&fixture, "02-pause", 20).await?;
    ensure!(
        driver.confirmed("03-patch", "stage")?.is_none(),
        "governance failure patched market"
    );
    ensure!(
        raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await? == before,
        "governance failure changed market"
    );
    assert_manual(&fixture, false).await
}

#[rstest::rstest]
#[case::missing_source("missing")]
#[case::stale_source("stale")]
#[case::manual_pause_aba("aba")]
#[case::automatic_breaker("automatic")]
#[tokio::test]
async fn requires_sandbox_scripts_readiness_or_incident_change_never_unpauses(
    #[case] fault: &str,
) -> Result<()> {
    let mut fixture = MigrationFixture::new_with_terms(
        MigrationOptions::default(),
        604_800_000_000_000,
        None,
        (fault == "stale").then_some(600),
    )
    .await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "05-reconcile").await?;
    match fault {
        "missing" => {
            fixture
                .seeded
                .harness
                .set_mock_oracle_pyth_price(fixture.source_oracle.clone(), FEEDS[0], None)
                .await?;
        }
        "stale" => {
            // Only this fixture uses a 600s bound. Sandbox fast-forward advances
            // time by 310ms per block, so 2,000 blocks cross it explicitly.
            fixture.seeded.harness.fast_forward(2_000).await?;
            ensure!(
                read(
                    &fixture,
                    "proxyOracle.getFreshSourcePrice",
                    json!({"oracle_id":fixture.proxy,"id":FEEDS[0],"age":600}),
                )
                .await?
                .is_null(),
                "adverse source price did not become unavailable under its exact freshness bound"
            );
        }
        "aba" => {
            set_manual(&fixture, false).await?;
            set_manual(&fixture, true).await?;
        }
        "automatic" => {
            target_call(
                &fixture,
                "admin_configure_circuit_breakers",
                json!({
                    "id":FEEDS[0],"config":{"sample_interval_ns":"0","history_len":4}
                }),
            )
            .await?;
            target_call(
                &fixture,
                "admin_add_circuit_breaker",
                json!({
                    "id":FEEDS[0],"breaker_id":0,
                    "breaker":{"StepwiseChange":{"max_relative_change":"0.01"}}
                }),
            )
            .await?;
        }
        _ => unreachable!(),
    }
    let breakers = read(
        &fixture,
        "proxyOracle.getProxyCircuitBreakerSet",
        json!({"oracle_id":fixture.proxy,"id":FEEDS[0]}),
    )
    .await?;
    driver.assert_blocked(&fixture, "06-resume", 20).await?;
    ensure!(
        read(
            &fixture,
            "proxyOracle.getProxyCircuitBreakerSet",
            json!({"oracle_id":fixture.proxy,"id":FEEDS[0]})
        )
        .await?
            == breakers,
        "resume modified manual/automatic breaker despite changed readiness/incident"
    );
    assert_manual(&fixture, true).await
}

#[tokio::test]
async fn requires_sandbox_scripts_new_manual_pause_after_partial_resume_blocks_cohort() -> Result<()>
{
    let mut fixture = MigrationFixture::new(MigrationOptions::default()).await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "05-reconcile").await?;
    let intent = driver.intent()?;
    let feeds = intent["feeds"].as_array().context("cohort feeds")?;
    let first = &feeds[0];
    let first_feed = first["feed"].as_str().context("first feed")?;
    let first_key = format!(
        "{}:{first_feed}",
        first["oracle"].as_str().context("first oracle")?
    );
    let second_key = format!(
        "{}:{}",
        feeds[1]["oracle"].as_str().context("second oracle")?,
        feeds[1]["feed"].as_str().context("second feed")?
    );
    let mut partially_resumed = false;
    for _ in 0..8 {
        driver.expect(Some("06-resume"), true, 10).await?;
        if driver.confirmed("06-resume", &first_key)?.is_some() {
            ensure!(driver.confirmed("06-resume", &second_key)?.is_none());
            partially_resumed = true;
            break;
        }
        fixture.approve_pending(&driver.run).await?;
        driver.handoff(&fixture)?;
    }
    ensure!(partially_resumed, "fixture never reached a partial resume");
    let before = raw_storage(&fixture.seeded.harness, &fixture.proxy).await?;
    // The actual owner key changes the now-unpaused feed without consuming the
    // pending campaign governance ID, so an ID collision cannot mask the guard.
    let owner = ManagedAccountId(
        first["governance"]
            .as_str()
            .context("oracle owner")?
            .parse()?,
    );
    let changed = fixture
        .seeded
        .harness
        .call_function(
            &owner,
            &fixture.proxy,
            "admin_set_manual_trip",
            json!({
                "id":first_feed,"is_manually_tripped":true,
                "metadata":STANDARD.encode(b"new external incident after own resume")
            }),
        )
        .await?;
    fixture::finalize(&fixture.seeded.harness, &owner.0, &changed).await?;
    let incident = raw_storage(&fixture.seeded.harness, &fixture.proxy).await?;
    ensure!(
        incident != before,
        "new manual incident did not change real oracle state"
    );
    driver.assert_blocked(&fixture, "06-resume", 20).await?;
    ensure!(
        raw_storage(&fixture.seeded.harness, &fixture.proxy).await? == incident,
        "blocked resume changed the new external incident"
    );
    assert_manual(&fixture, true).await
}

#[tokio::test]
async fn requires_sandbox_scripts_failed_final_refresh_reports_actual_open_feed_without_repause(
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions::default()).await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "06-resume").await?;
    assert_manual(&fixture, false).await?;
    driver.expect(Some("07-finalize"), true, 10).await?;
    fixture
        .seeded
        .harness
        .set_mock_oracle_pyth_price(fixture.source_oracle.clone(), FEEDS[0], None)
        .await?;
    fixture.approve_pending(&driver.run).await?;
    driver.handoff(&fixture)?;
    driver.expect(Some("07-finalize"), true, 20).await?;
    ensure!(
        driver.confirmed("07-finalize", "stage")?.is_none(),
        "failed refresh falsely finalized healthy"
    );
    assert_manual(&fixture, false).await?;
    let starts = driver.financial_starts()?;
    driver.expect(Some("07-finalize"), true, 20).await?;
    ensure!(
        driver.financial_starts()? == starts,
        "failed refresh automatically retried/repaused"
    );
    Ok(())
}

/// A real process boundary fault, not a fake manager/RPC response. The wrapper
/// forwards every command to the compiled binary and lets one actual proposal
/// or MPC broadcast finish on chain. It then withholds the response (or kills
/// the waiting Python process). Retained native output is external evidence.
fn install_response_fault(
    driver: &mut Driver,
    crash: bool,
    mpc_execution: bool,
) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;
    let wrapper = driver.root.join("lose-native-response.py");
    let proof = driver.root.join("lost-native-response.json");
    let source = format!(
        r#"#!/usr/bin/env python3
import json, os, signal, subprocess, sys
from pathlib import Path
args = sys.argv[1:]
binary = {binary}
proof = Path({proof})
result = subprocess.run([binary, *args], capture_output=True, check=False)
financial = ("broadcast" in args if {mpc_execution} else "propose" in args) and "--print" not in args
if financial and not proof.exists():
    with proof.open("xb") as stream:
        stream.write(result.stdout)
        stream.flush()
        os.fsync(stream.fileno())
    if {crash}:
        os.kill(os.getppid(), signal.SIGKILL)
    os._exit(75)
sys.stdout.buffer.write(result.stdout)
sys.stderr.buffer.write(result.stderr)
sys.exit(result.returncode)
"#,
        binary = serde_json::to_string(BIN)?,
        proof = serde_json::to_string(&proof)?,
        crash = if crash { "True" } else { "False" },
        mpc_execution = if mpc_execution { "True" } else { "False" },
    );
    write_new(&wrapper, source.as_bytes())?;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700))?;
    driver.binary = wrapper;
    Ok(proof)
}

#[rstest::rstest]
#[case::lost_response(false)]
#[case::crash_after_submission(true)]
#[tokio::test]
async fn requires_sandbox_scripts_recover_actual_lost_response_without_repeating_write(
    #[case] crash: bool,
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions {
        changes: ChangedRoles::Borrow,
        ..MigrationOptions::default()
    })
    .await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "00-prepare").await?;
    let proof = install_response_fault(&mut driver, crash, false)?;
    let count = proposal_count(&fixture).await?;
    let output = driver.invoke(Some("01-register"), true).await?;
    ensure!(
        !output.status.success(),
        "fault did not interrupt real script"
    );
    let submitted = load(proof)?;
    let id = submitted["proposal_id"]
        .as_u64()
        .context("retained real native proposal ID")?;
    ensure!(
        proposal_count(&fixture).await? == count + 1,
        "fault did not follow exactly one real chain submission"
    );
    ensure!(
        fixture.proposal(id).await?["status"] == "InProgress",
        "lost response fabricated an approved operation"
    );
    let started = driver.financial_starts()?;
    ensure!(
        started.len() == 1,
        "first submission must retain exactly one durable boundary"
    );
    driver.binary = PathBuf::from(BIN);
    driver.expect(Some("01-register"), true, 30).await?;
    driver.expect(Some("01-register"), true, 30).await?;
    ensure!(
        proposal_count(&fixture).await? == count + 1 && driver.financial_starts()? == started,
        "unknown submission was retried without chain evidence"
    );
    fixture.approve_id(id).await?;
    fixture.seeded.harness.fast_forward(5).await?;
    let vote = fixture
        .execution_votes
        .get(&id)
        .context("threshold vote proof")?
        .clone();
    let refs = driver.root.join("recovered-execution-ref.json");
    write_new(
        &refs,
        &serde_json::to_vec_pretty(&json!({
            "schema":1,"intent_digest":driver.intent()?["intent_digest"],"operations":[{
                "stage":started[0]["stage"],"key":started[0]["key"],"attempt":started[0]["attempt"],
                "dao_proposal_id":id,"tx_hash":vote["tx_hash"],"sender_account_id":vote["sender_account_id"]
            }]
        }))?,
    )?;
    driver.refs = Some(refs);
    driver.drive_stage(&mut fixture, "01-register").await?;
    ensure!(
        proposal_count(&fixture).await? == count + 1,
        "recovered registration submitted a duplicate"
    );
    ensure!(
        driver.financial_starts()?.len() == 1,
        "recovered registration crossed a second write boundary"
    );
    let receipts = read(&fixture,"tx.get",json!({
        "tx_hash":vote["tx_hash"],"sender_account_id":vote["sender_account_id"],"encoding":"json"
    })).await?;
    assert_receipts(&receipts)?;
    Ok(())
}

async fn hold_lock(path: &Path) -> Result<tokio::process::Child> {
    use tokio::io::{AsyncBufReadExt as _, BufReader};
    let mut child = tokio::process::Command::new("python3").args(["-c",
        "import fcntl,sys; f=open(sys.argv[1],'a+b'); fcntl.flock(f,fcntl.LOCK_EX); print('locked',flush=True); sys.stdin.readline()"])
        .arg(path).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
        .kill_on_drop(true).spawn()?;
    let mut ready = String::new();
    BufReader::new(child.stdout.take().context("lock pipe")?)
        .read_line(&mut ready)
        .await?;
    ensure!(
        ready == "locked\n",
        "external lock process did not acquire lock"
    );
    Ok(child)
}

#[rstest::rstest]
#[case::same_run(false)]
#[case::same_derived_key(true)]
#[tokio::test]
async fn requires_sandbox_scripts_real_concurrent_locks_block_before_submission(
    #[case] key_lock: bool,
) -> Result<()> {
    let fixture = MigrationFixture::new(MigrationOptions::default()).await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.expect(Some("00-prepare"), false, 0).await?;
    let path = if key_lock {
        std::fs::read_dir(driver.state.join("templar-asset-migration"))?
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter()
            .map(|entry| entry.path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "lock")
            })
            .context("derived-key lock file")?
    } else {
        driver.run.join(".run.lock")
    };
    let mut holder = hold_lock(&path).await?;
    let count = proposal_count(&fixture).await?;
    driver.expect(Some("01-register"), true, 20).await?;
    ensure!(
        proposal_count(&fixture).await? == count && driver.financial_starts()?.is_empty(),
        "concurrent runner submitted despite exclusive lock"
    );
    holder.start_kill()?;
    holder.wait().await?;
    Ok(())
}

#[tokio::test]
async fn requires_sandbox_scripts_durable_key_lease_blocks_a_different_run_directory() -> Result<()>
{
    let mut fixture = MigrationFixture::new(MigrationOptions::default()).await?;
    let mut first = Driver::new(&fixture, "transaction")?;
    first.through(&mut fixture, "02-pause").await?;
    first.expect(Some("03-patch"), true, 10).await?;
    let mut second = Driver::new(&fixture, "transaction")?;
    second.state = first.state.clone();
    second.expect(Some("00-prepare"), false, 0).await?;
    let count = proposal_count(&fixture).await?;
    second.expect(Some("01-register"), true, 20).await?;
    ensure!(
        second.financial_starts()?.is_empty() && proposal_count(&fixture).await? == count,
        "a second directory bypassed unresolved market-key lease"
    );
    assert_manual(&fixture, true).await
}

#[tokio::test]
async fn requires_sandbox_scripts_production_submitting_entrypoints_all_fail_closed() -> Result<()>
{
    let mut fixture = MigrationFixture::new(MigrationOptions::default()).await?;
    fixture.campaign["environment"] = json!("production");
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.expect(Some("00-prepare"), false, 0).await?;
    let count = proposal_count(&fixture).await?;
    let before = raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await?;
    for stage in &STAGES[1..] {
        driver.expect(Some(stage), true, 20).await?;
        ensure!(
            driver.confirmed(stage, "stage")?.is_none(),
            "production entrypoint claimed completion"
        );
    }
    ensure!(
        driver.financial_starts()?.is_empty() && proposal_count(&fixture).await? == count,
        "production fixture submitted a prohibited operation"
    );
    ensure!(
        raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await? == before,
        "production attempt mutated storage"
    );
    assert_manual(&fixture, false).await
}

#[tokio::test]
async fn requires_sandbox_scripts_nonlocal_fixture_endpoint_is_not_admitted() -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions::default()).await?;
    // 0.0.0.0 reaches this actual Linux sandbox but is not a loopback identity.
    // No production host or fixture credentials are contacted.
    let mut url = fixture.seeded.harness.network.rpc_endpoints[0].url.clone();
    url.set_host(Some("0.0.0.0"))?;
    fixture.campaign["rpc_url"] = json!(url.as_str());
    let mut driver = Driver::new(&fixture, "transaction")?;
    let output = driver.invoke(Some("00-prepare"), false).await?;
    if output.status.success() {
        driver.expect(Some("01-register"), true, 20).await?;
    } else {
        ensure!(
            output.status.code() == Some(20),
            "nonlocal admission returned an unexpected process failure"
        );
    }
    ensure!(
        driver.financial_starts()?.is_empty() && proposal_count(&fixture).await? == 0,
        "nonlocal endpoint accepted local converter submission"
    );
    Ok(())
}

#[tokio::test]
async fn requires_sandbox_scripts_changed_authored_campaign_and_truncated_journal_are_not_resumed(
) -> Result<()> {
    let fixture = MigrationFixture::new(MigrationOptions::default()).await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.expect(Some("00-prepare"), false, 0).await?;
    let original = std::fs::read(&driver.campaign)?;
    let mut changed = fixture.campaign.clone();
    changed["submitter"] = json!(fixture.outsider.0);
    std::fs::write(&driver.campaign, toml::to_string(&changed)?)?;
    driver.expect(None, true, 20).await?;
    ensure!(
        driver.financial_starts()?.is_empty(),
        "changed campaign submitted a write"
    );
    std::fs::write(&driver.campaign, original)?;
    let journal = driver.run.join("journal.jsonl");
    let mut file = OpenOptions::new().append(true).open(journal)?;
    file.write_all(b"{\"interrupted\":")?;
    file.sync_all()?;
    let output = driver.invoke(None, true).await?;
    ensure!(
        output.status.code() == Some(20) || output.status.code() == Some(30),
        "truncated journal was silently reset"
    );
    ensure!(
        proposal_count(&fixture).await? == 0,
        "truncated journal submitted a proposal"
    );
    Ok(())
}

async fn trip_automatic_breaker(fixture: &MigrationFixture) -> Result<Value> {
    target_call(
        fixture,
        "admin_configure_circuit_breakers",
        json!({
            "id":FEEDS[0],"config":{"sample_interval_ns":"0","history_len":4}
        }),
    )
    .await?;
    target_call(
        fixture,
        "admin_add_circuit_breaker",
        json!({
            "id":FEEDS[0],"breaker_id":0,"breaker":{"StepwiseChange":{"max_relative_change":"0.01"}}
        }),
    )
    .await?;
    fixture
        .seeded
        .harness
        .update_proxy_prices(fixture.proxy.clone(), vec![FEEDS[0]])
        .await?;
    let published = fixture
        .seeded
        .harness
        .client()?
        .read(templar_gateway_methods_spec::chain::GetBlock::default())
        .await?
        .timestamp_ns
        / 1_000_000_000;
    fixture
        .seeded
        .harness
        .set_mock_oracle_pyth_price(
            fixture.source_oracle.clone(),
            FEEDS[0],
            Some(templar_common::oracle::pyth::Price {
                price: near_sdk::json_types::I64(20_000),
                conf: near_sdk::json_types::U64(0),
                expo: -4,
                publish_time: templar_common::oracle::pyth::PythTimestamp::from_secs(
                    published.try_into()?,
                ),
            }),
        )
        .await?;
    fixture.seeded.harness.fast_forward(5).await?;
    fixture
        .seeded
        .harness
        .update_proxy_prices(fixture.proxy.clone(), vec![FEEDS[0]])
        .await?;
    let breaker = read(
        fixture,
        "proxyOracle.getProxyCircuitBreakerSet",
        json!({"oracle_id":fixture.proxy,"id":FEEDS[0]}),
    )
    .await?;
    ensure!(
        breaker["breakers"]["0"]["is_enforced"] == true
            && breaker["breakers"]["0"]["status"].get("Tripped").is_some(),
        "automatic fault must really trip on chain: {breaker}"
    );
    Ok(breaker)
}

#[tokio::test]
async fn requires_sandbox_scripts_preserve_original_manual_and_active_automatic_breaker(
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions::default()).await?;
    trip_automatic_breaker(&fixture).await?;
    set_manual(&fixture, true).await?;
    let original = read(
        &fixture,
        "proxyOracle.getProxyCircuitBreakerSet",
        json!({"oracle_id":fixture.proxy,"id":FEEDS[0]}),
    )
    .await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "07-finalize").await?;
    ensure!(
        read(
            &fixture,
            "proxyOracle.getProxyCircuitBreakerSet",
            json!({"oracle_id":fixture.proxy,"id":FEEDS[0]})
        )
        .await?
            == original,
        "campaign cleared or changed an original automatic breaker"
    );
    ensure!(
        !driver
            .financial_starts()?
            .iter()
            .any(|event| event["stage"] == "02-pause"
                || event["stage"] == "06-resume"
                || event["stage"] == "07-finalize"),
        "originally paused feeds scheduled governance trip/clear/refresh"
    );
    final_assertions(&fixture, &driver, true).await
}

#[tokio::test]
async fn requires_sandbox_scripts_expired_sputnik_approval_is_terminal_without_replacement(
) -> Result<()> {
    let mut fixture =
        MigrationFixture::new_with_terms(MigrationOptions::default(), 60_000_000_000, None, None)
            .await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "01-register").await?;
    driver.expect(Some("02-pause"), true, 10).await?;
    let id = *driver
        .submitted_proposal_ids()?
        .last()
        .context("proposal")?;
    fixture.seeded.harness.fast_forward(1_000).await?;
    let stored = fixture.proposal(id).await?;
    fixture
        .seeded
        .harness
        .call_function(
            &fixture.council[0],
            &fixture.dao,
            "act_proposal",
            json!({"id":id,"action":"Finalize","proposal":stored["kind"],"memo":null}),
        )
        .await?;
    ensure!(
        fixture.proposal(id).await?["status"] == "Expired",
        "actual Sputnik expiration not exercised"
    );
    driver.assert_blocked(&fixture, "02-pause", 20).await?;
    assert_manual(&fixture, false).await
}

#[tokio::test]
async fn requires_sandbox_scripts_actual_spent_conversion_signature_cannot_be_replayed(
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions {
        changes: ChangedRoles::Borrow,
        ..MigrationOptions::default()
    })
    .await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "04-swap").await?;
    let event = driver
        .events()?
        .into_iter()
        .find(|event| {
            event["stage"] == "04-swap"
                && event["operation"] == "mpc"
                && event["event"] == "confirmed"
        })
        .context("retained signed conversion")?;
    let evidence = &event["evidence"];
    let account = fixture.seeded.market.market_id.clone();
    let baseline = raw_storage(&fixture.seeded.harness, &account).await?;
    let intent = driver.intent()?;
    let role = &intent["markets"][0]["roles"][0];
    let old = balance(&fixture, &role["old_reference"], account.as_str()).await?;
    let new = balance(&fixture, &role["new_reference"], account.as_str()).await?;
    let key = read(
        &fixture,
        "account.getAccessKey",
        json!({
            "account_id":account,"public_key":intent["markets"][0]["public_key"]
        }),
    )
    .await?;
    ensure!(
        key["nonce"] == evidence["nonce"],
        "conversion did not consume the approved nonce"
    );
    let payload = driver
        .run
        .join(evidence["payload"].as_str().context("envelope")?);
    let plan = driver
        .run
        .join(evidence["source_plan"].as_str().context("source plan")?);
    let output = cli(
        &fixture,
        &[
            "dao",
            "mpc",
            "broadcast",
            "--dao",
            fixture.dao.as_str(),
            "--proposal-id",
            &evidence["proposal_id"].to_string(),
            "--mpc-contract",
            fixture.mpc_contract.as_str(),
            "--payload-file",
            payload.to_str().context("payload path")?,
            "--plan",
            plan.to_str().context("plan path")?,
            "--tx-hash",
            evidence["execution_ref"]["tx_hash"]
                .as_str()
                .context("vote hash")?,
            "--tx-signer",
            evidence["execution_ref"]["sender_account_id"]
                .as_str()
                .context("vote sender")?,
        ],
        None,
    )
    .await?;
    ensure!(
        !output.status.success(),
        "spent conversion signature replay was accepted"
    );
    write_new(&driver.root.join("replay.stdout"), &output.stdout)?;
    write_new(&driver.root.join("replay.stderr"), &output.stderr)?;
    ensure!(
        raw_storage(&fixture.seeded.harness, &account).await? == baseline,
        "replay mutated market storage"
    );
    ensure!(
        balance(&fixture, &role["old_reference"], account.as_str()).await? == old
            && balance(&fixture, &role["new_reference"], account.as_str()).await? == new,
        "replay moved inventory twice"
    );
    Ok(())
}

#[tokio::test]
async fn requires_sandbox_scripts_one_diverged_cohort_member_blocks_shared_feed_resume(
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions::default()).await?;
    fixture
        .add_shared_market(RestoreMode::GlobalAccount)
        .await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "03-patch").await?;
    let peer = &fixture.additional_markets[0];
    let before = raw_storage(&fixture.seeded.harness, &peer.market_id).await?;
    let registered = fixture
        .seeded
        .harness
        .storage_deposit_min(&fixture.outsider, &peer.market_id)
        .await?;
    fixture::finalize(&fixture.seeded.harness, &fixture.outsider.0, &registered).await?;
    let diverged = raw_storage(&fixture.seeded.harness, &peer.market_id).await?;
    ensure!(
        diverged != before,
        "cohort adverse registration did not mutate real peer storage"
    );
    // The first member may finish its conversion before the second member is
    // inspected. Neither that success nor a shared feed authorizes early resume.
    let mut stopped = false;
    for _ in 0..12 {
        let output = driver.invoke(Some("04-swap"), true).await?;
        if output.status.code() == Some(20) {
            stopped = true;
            break;
        }
        ensure!(
            output.status.code() == Some(10),
            "unexpected cohort outcome {}",
            output.status
        );
        fixture.approve_pending(&driver.run).await?;
        driver.handoff(&fixture)?;
    }
    ensure!(stopped, "diverged member did not stop conversion");
    driver.expect(Some("05-reconcile"), true, 20).await?;
    driver.expect(Some("06-resume"), true, 20).await?;
    ensure!(
        !driver
            .financial_starts()?
            .iter()
            .any(|event| event["stage"] == "06-resume"),
        "one reconciled market resumed a shared feed early"
    );
    ensure!(
        raw_storage(
            &fixture.seeded.harness,
            &fixture.additional_markets[0].market_id
        )
        .await?
            == diverged,
        "blocked cohort workflow altered the diverged peer's exact storage"
    );
    assert_manual(&fixture, true).await
}

#[tokio::test]
async fn requires_sandbox_scripts_completed_run_is_not_a_perpetual_fresh_price_certificate(
) -> Result<()> {
    let mut fixture = MigrationFixture::new_with_terms(
        MigrationOptions {
            changes: ChangedRoles::Borrow,
            ..MigrationOptions::default()
        },
        604_800_000_000_000,
        None,
        Some(600),
    )
    .await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "07-finalize").await?;
    final_assertions(&fixture, &driver, false).await?;
    let count = proposal_count(&fixture).await?;
    let starts = driver.financial_starts()?;
    // This fixture's 600s bound expires after 2,000 simulated 310ms blocks.
    fixture.seeded.harness.fast_forward(2_000).await?;
    ensure!(
        read(
            &fixture,
            "proxyOracle.getCachedPrice",
            json!({"oracle_id":fixture.proxy,"id":FEEDS[0],"age":600}),
        )
        .await?
        .is_null(),
        "completed run retained a usable cached price beyond its exact freshness bound"
    );
    driver.expect(None, true, 20).await?;
    ensure!(
        driver.financial_starts()? == starts && proposal_count(&fixture).await? == count,
        "stale completed run automatically refreshed or submitted another operation"
    );
    ensure!(
        driver
            .events()?
            .iter()
            .any(|event| event["stage"] == "07-finalize" && event["event"] == "failed"),
        "stale completion did not retain failed freshness evidence"
    );
    ensure!(
        driver.confirmed("07-finalize", "stage")?.is_none(),
        "stale completion remained current after its failed recheck"
    );
    assert_manual(&fixture, false).await
}

#[rstest::rstest]
#[case::registration("01-register")]
#[case::governance_creation("02-pause")]
#[tokio::test]
async fn requires_sandbox_scripts_require_the_executing_threshold_vote_receipts(
    #[case] stage: &str,
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions {
        changes: ChangedRoles::Borrow,
        borrow_mode: TokenMode::Ft,
        collateral_mode: TokenMode::Ft,
        restore: RestoreMode::Local,
        initially_paused: false,
    })
    .await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "00-prepare").await?;
    if stage == "02-pause" {
        driver.drive_stage(&mut fixture, "01-register").await?;
    }
    let baseline = raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await?;
    let count = proposal_count(&fixture).await?;
    driver.expect(Some(stage), true, 10).await?;
    let submitted = driver
        .events()?
        .into_iter()
        .rev()
        .find(|event| event["stage"] == stage && event["event"] == "submitted")
        .context("ordinary proposal submission")?;
    let id = submitted["evidence"]["proposal_id"]
        .as_u64()
        .context("ordinary proposal ID")?;
    let mut votes = Vec::new();
    for (index, voter) in fixture.council.iter().enumerate() {
        let stored = fixture.proposal(id).await?;
        let vote = fixture
            .seeded
            .harness
            .try_call_function(
                voter,
                &fixture.dao,
                "act_proposal",
                json!({"id":id,"action":"VoteApprove","proposal":stored["kind"],"memo":null}),
            )
            .await?;
        let reference = json!({
            "tx_hash":vote.operation.latest_tx_hash().context("vote hash")?.to_string(),
            "sender_account_id":voter.0
        });
        let receipts = read(
            &fixture,
            "tx.get",
            json!({"tx_hash":reference["tx_hash"],
                "sender_account_id":reference["sender_account_id"],"encoding":"json"}),
        )
        .await?;
        assert_receipts(&receipts)?;
        let after = fixture.proposal(id).await?;
        ensure!(
            after["status"] == if index == 0 { "InProgress" } else { "Approved" },
            "fixture did not exercise the two-vote execution threshold"
        );
        votes.push((reference, receipts));
    }
    fixture.seeded.harness.fast_forward(5).await?;
    let proposal = fixture.proposal(id).await?;
    let receiver = proposal["kind"]["FunctionCall"]["receiver_id"]
        .as_str()
        .context("ordinary target")?;
    for (index, (_, receipts)) in votes.iter().enumerate() {
        let executed_target = receipts["receipts"]
            .as_array()
            .context("actual vote receipt graph")?
            .iter()
            .any(|receipt| receipt["executor_id"] == receiver);
        ensure!(
            executed_target == (index == 1),
            "only the final threshold vote may execute the target"
        );
    }
    // The target is already live before testing the wrong reference. Neither
    // its present state nor DAO Approved status proves which vote executed it.
    let intent = driver.intent()?;
    if stage == "01-register" {
        let registration = intent["registrations"]
            .as_array()
            .context("registrations")?
            .iter()
            .find(|registration| registration["contract_id"] == receiver)
            .context("submitted registration")?;
        let live = read(
            &fixture,
            "storage.getBalanceOf",
            json!({"contract_id":receiver,"account_id":registration["account_id"]}),
        )
        .await?;
        ensure!(
            live["total"]
                .as_str()
                .context("registered total")?
                .parse::<u128>()?
                >= registration["minimum"]
                    .as_str()
                    .context("minimum")?
                    .parse::<u128>()?,
            "actual threshold vote did not register the recipient"
        );
    } else {
        let args: Value = serde_json::from_slice(
            &STANDARD.decode(
                proposal["kind"]["FunctionCall"]["actions"][0]["args"]
                    .as_str()
                    .context("governance creation arguments")?,
            )?,
        )?;
        let live = read(
            &fixture,
            "proxyOracleGovernance.getProposal",
            json!({"governance_id":receiver,"id":args["id"]}),
        )
        .await?;
        ensure!(
            live["operation"] == args["operation"] && live["created_by"] == fixture.dao.as_str(),
            "actual threshold vote did not create the exact governance operation"
        );
    }
    let starts = driver.financial_starts()?;
    for (index, (reference, _)) in votes.iter().enumerate() {
        let path = driver
            .root
            .join(format!("threshold-reference-{index}.json"));
        write_new(
            &path,
            &serde_json::to_vec_pretty(&json!({
                "schema":1,"intent_digest":driver.intent()?["intent_digest"],
                "operations":[{
                    "stage":submitted["stage"],"key":submitted["key"],"attempt":submitted["attempt"],
                    "dao_proposal_id":id,"tx_hash":reference["tx_hash"],
                    "sender_account_id":reference["sender_account_id"]
                }]
            }))?,
        )?;
        driver.refs = Some(path);
        if index == 0 {
            driver.assert_blocked(&fixture, stage, 20).await?;
            ensure!(
                !driver.events()?.iter().any(|event| {
                    event["stage"] == submitted["stage"]
                        && event["key"] == submitted["key"]
                        && event["operation"] == "dao"
                        && event["event"] == "confirmed"
                }),
                "nonexecuting vote financially confirmed an already-live target"
            );
        } else {
            // Read-only recovery can prepare the next operation, but cannot
            // submit it. The same proposal must become receipt-proven.
            let output = driver.invoke(Some(stage), false).await?;
            ensure!(
                matches!(output.status.code(), Some(0 | 10)),
                "corrected final-vote handoff did not recover: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let confirmed = driver
                .events()?
                .into_iter()
                .find(|event| {
                    event["stage"] == submitted["stage"]
                        && event["key"] == submitted["key"]
                        && event["operation"] == "dao"
                        && event["event"] == "confirmed"
                })
                .context("receipt-proven financial confirmation")?;
            ensure!(
                confirmed["evidence"]["execution_ref"]["tx_hash"] == reference["tx_hash"]
                    && confirmed["evidence"]["receipts"] == votes[1].1,
                "recovery did not retain the exact executing receipt graph"
            );
        }
        ensure!(
            driver.financial_starts()? == starts && proposal_count(&fixture).await? == count + 1,
            "correcting execution evidence created a new financial operation"
        );
    }
    ensure!(
        raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await? == baseline,
        "ordinary receipt inspection changed market storage"
    );
    assert_manual(&fixture, false).await
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one approval-wait timeline keeps exact inventory, identity and storage checkpoints together"
)]
async fn requires_sandbox_scripts_freeze_unchanged_collateral_inventory_while_patch_waits(
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions {
        changes: ChangedRoles::Borrow,
        borrow_mode: TokenMode::Ft,
        collateral_mode: TokenMode::Ft,
        restore: RestoreMode::Local,
        initially_paused: false,
    })
    .await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "02-pause").await?;
    driver.expect(Some("03-patch"), true, 10).await?;
    let intent = driver.intent()?;
    let market = &intent["markets"][0];
    let unchanged = market["unchanged_assets"]
        .as_array()
        .context("frozen unchanged assets")?;
    ensure!(unchanged.len() == 1 && unchanged[0]["role"] == "collateral");
    let collateral = &unchanged[0];
    let account = market["account"].as_str().context("market account")?;
    let before_balance = balance(&fixture, &collateral["reference"], account).await?;
    ensure!(
        before_balance.to_string() == collateral["balance"].as_str().context("frozen balance")?
    );
    let raw = raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await?;
    let keys = read(
        &fixture,
        "account.listAccessKeys",
        json!({"account_id":account}),
    )
    .await?;
    let code = identity(&fixture, account).await?;
    let contract = collateral["reference"]["contract_id"]
        .as_str()
        .context("unchanged FT contract")?
        .parse::<near_account_id::AccountId>()?;
    let harness = &fixture.seeded.harness;
    fixture::finalize(
        harness,
        &fixture.outsider.0,
        &harness
            .storage_deposit_min(&fixture.outsider, &contract)
            .await?,
    )
    .await?;
    fixture::finalize(
        harness,
        &fixture.outsider.0,
        &harness.mint(&fixture.outsider, contract, 7).await?,
    )
    .await?;
    fixture::finalize(
        harness,
        &fixture.outsider.0,
        &harness
            .execute(
                &fixture.outsider,
                templar_gateway_methods_spec::token::Transfer {
                    token: templar_gateway_methods_spec::token::TokenReference::deserialize(
                        &collateral["reference"],
                    )?,
                    receiver_id: account.parse()?,
                    amount: 7.into(),
                    memo: None,
                },
            )
            .await?,
    )
    .await?;
    ensure!(balance(&fixture, &collateral["reference"], account).await? == before_balance + 7);
    ensure!(
        raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await? == raw
            && read(
                &fixture,
                "account.listAccessKeys",
                json!({"account_id":account})
            )
            .await?
                == keys
            && identity(&fixture, account).await? == code,
        "external inventory injection must not change market state, code or key nonces"
    );
    fixture.approve_pending(&driver.run).await?;
    driver.handoff(&fixture)?;
    for stage in ["03-patch", "04-swap", "05-reconcile"] {
        driver.assert_blocked(&fixture, stage, 20).await?;
    }
    ensure!(
        raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await? == raw
            && identity(&fixture, account).await? == code
            && balance(&fixture, &collateral["reference"], account).await? == before_balance + 7,
        "rejected drift changed market or external collateral inventory"
    );
    for role in market["roles"].as_array().context("changed roles")? {
        ensure!(
            balance(&fixture, &role["old_reference"], account)
                .await?
                .to_string()
                == role["initial_old_balance"]
                    .as_str()
                    .context("old inventory")?
                && balance(&fixture, &role["new_reference"], account)
                    .await?
                    .to_string()
                    == role["initial_new_balance"]
                        .as_str()
                        .context("new inventory")?,
            "inventory drift permitted a changed-role transfer"
        );
    }
    assert_manual(&fixture, true).await
}

#[allow(
    clippy::too_many_lines,
    reason = "one ordered two-feed ABA timeline retains the exact checkpoint and pending proposal"
)]
#[rstest::rstest]
#[case::before_creation_approval(false)]
#[case::pending_execution_approval(true)]
#[tokio::test]
async fn requires_sandbox_scripts_partial_pause_preserves_checkpoint_across_external_aba(
    #[case] late: bool,
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions {
        changes: ChangedRoles::Both,
        borrow_mode: TokenMode::Ft,
        collateral_mode: TokenMode::Ft,
        restore: RestoreMode::Local,
        initially_paused: false,
    })
    .await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "01-register").await?;
    let intent = driver.intent()?;
    let feeds = intent["feeds"].as_array().context("cohort feeds")?;
    ensure!(feeds.len() == 2, "fixture must expose two distinct feeds");
    let first = &feeds[0];
    let second = &feeds[1];
    let first_key = format!(
        "{}:{}",
        first["oracle"].as_str().context("oracle")?,
        first["feed"].as_str().context("feed")?
    );
    let second_key = format!(
        "{}:{}",
        second["oracle"].as_str().context("oracle")?,
        second["feed"].as_str().context("feed")?
    );
    let raw = raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await?;
    // A creation, then A execution, then B creation awaiting its votes.
    driver.expect(Some("02-pause"), true, 10).await?;
    for _ in 0..2 {
        fixture.approve_pending(&driver.run).await?;
        driver.handoff(&fixture)?;
        driver.expect(Some("02-pause"), true, 10).await?;
    }
    let checkpoint = driver
        .confirmed("02-pause", &first_key)?
        .context("confirmed A pause")?;
    ensure!(checkpoint["ownership"] == "campaign");
    ensure!(driver.confirmed("02-pause", &second_key)?.is_none());
    let pending = driver
        .submitted_proposal_ids()?
        .into_iter()
        .next_back()
        .context("B creation proposal")?;
    ensure!(fixture.proposal(pending).await?["status"] == "InProgress");
    if late {
        // B creation is approved and its separate execution proposal already
        // waits for the final council vote before the foreign A incident.
        fixture.approve_pending(&driver.run).await?;
        driver.handoff(&fixture)?;
        driver.expect(Some("02-pause"), true, 10).await?;
        let execution = driver
            .submitted_proposal_ids()?
            .into_iter()
            .next_back()
            .context("B execution proposal")?;
        ensure!(execution != pending);
        let proposal = fixture.proposal(execution).await?;
        ensure!(proposal["status"] == "InProgress");
        let call = &proposal["kind"]["FunctionCall"]["actions"][0];
        ensure!(call["method_name"] == "execute_proposal");
        let voter = &fixture.council[0];
        let vote = fixture
            .seeded
            .harness
            .try_call_function(
                voter,
                &fixture.dao,
                "act_proposal",
                json!({"id":execution,"action":"VoteApprove","proposal":proposal["kind"],"memo":null}),
            )
            .await?;
        let receipts = read(
            &fixture,
            "tx.get",
            json!({"tx_hash":vote.operation.latest_tx_hash().context("first vote")?,
                "sender_account_id":voter.0,"encoding":"json"}),
        )
        .await?;
        assert_receipts(&receipts)?;
        ensure!(fixture.proposal(execution).await?["status"] == "InProgress");
    }
    let paused_raw = raw_storage(&fixture.seeded.harness, &fixture.proxy).await?;
    for tripped in [false, true] {
        target_call(
            &fixture,
            "admin_set_manual_trip",
            json!({"id":first["feed"],"is_manually_tripped":tripped,"metadata":null}),
        )
        .await?;
    }
    ensure!(
        raw_storage(&fixture.seeded.harness, &fixture.proxy).await? != paused_raw,
        "real ABA must change the raw oracle incident checkpoint"
    );
    let external_raw = raw_storage(&fixture.seeded.harness, &fixture.proxy).await?;
    ensure!(
        read(
            &fixture,
            "proxyOracle.getProxyCircuitBreakerSet",
            json!({"oracle_id":first["oracle"],"id":first["feed"]}),
        )
        .await?
            == checkpoint["context"]["breaker"],
        "ABA must restore the visible breaker while changing the raw checkpoint"
    );
    fixture.approve_pending(&driver.run).await?;
    driver.handoff(&fixture)?;
    if late {
        let id = driver
            .submitted_proposal_ids()?
            .into_iter()
            .next_back()
            .context("B execution")?;
        let proof = fixture.execution_votes.get(&id).context("B final vote")?;
        let receipts = read(&fixture, "tx.get", json!({
            "tx_hash":proof["tx_hash"],"sender_account_id":proof["sender_account_id"],"encoding":"json"
        })).await?;
        assert_receipts(&receipts)?;
        ensure!(fixture.proposal(id).await?["status"] == "Approved");
        ensure!(
            receipts["receipts"]
                .as_array()
                .context("B target receipts")?
                .iter()
                .any(|receipt| receipt["executor_id"] == fixture.proxy.as_str()),
            "late window did not actually execute B's oracle operation"
        );
    }
    let approved_raw = raw_storage(&fixture.seeded.harness, &fixture.proxy).await?;
    ensure!(
        late || approved_raw == external_raw,
        "B creation unexpectedly changed oracle storage"
    );
    // B's approved creation/execution cannot certify a recaptured foreign A
    // incident, even when its execution was already waiting for the final vote.
    driver.assert_blocked(&fixture, "02-pause", 20).await?;
    driver.assert_blocked(&fixture, "06-resume", 20).await?;
    let second_directory = driver
        .run
        .join("02-pause")
        .join(format!("{:x}", sha2::Sha256::digest(second_key.as_bytes())));
    ensure!(
        !second_directory.join("execution.json").exists(),
        "foreign A incident was persisted as certified B execution evidence"
    );
    ensure!(
        driver.confirmed("02-pause", &first_key)? == Some(checkpoint)
            && driver.confirmed("02-pause", &second_key)?.is_none(),
        "partial restart replaced ownership evidence after the foreign ABA"
    );
    ensure!(
        raw_storage(&fixture.seeded.harness, &fixture.proxy).await? == approved_raw,
        "partial restart changed oracle incident storage"
    );
    ensure!(
        raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await? == raw,
        "partial restart changed market storage"
    );
    for market in intent["markets"].as_array().context("markets")? {
        let account = market["account"].as_str().context("market")?;
        for role in market["roles"].as_array().context("roles")? {
            ensure!(
                balance(&fixture, &role["old_reference"], account)
                    .await?
                    .to_string()
                    == role["initial_old_balance"]
                        .as_str()
                        .context("old inventory")?
                    && balance(&fixture, &role["new_reference"], account)
                        .await?
                        .to_string()
                        == role["initial_new_balance"]
                            .as_str()
                            .context("new inventory")?,
                "partial pause submitted an asset transfer"
            );
        }
    }
    for (feed, expected) in [(first, true), (second, late)] {
        let breaker = read(
            &fixture,
            "proxyOracle.getProxyCircuitBreakerSet",
            json!({"oracle_id":feed["oracle"],"id":feed["feed"]}),
        )
        .await?;
        ensure!(breaker["is_manually_tripped"] == expected);
    }
    Ok(())
}

#[rstest::rstest]
#[case::market_provider(false)]
#[case::oracle_provider(true)]
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one provider-update timeline binds unchanged behavior and state to effective-code drift"
)]
async fn requires_sandbox_scripts_effective_provider_code_after_patch(
    #[case] oracle: bool,
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions {
        restore: RestoreMode::GlobalAccount,
        ..MigrationOptions::default()
    })
    .await?;
    let (target, publisher, mut code) = if oracle {
        let code = read(
            &fixture,
            "account.getCode",
            json!({"account_id":fixture.proxy}),
        )
        .await?;
        let code = STANDARD.decode(code.as_str().context("oracle WASM")?)?;
        let (publisher, signer) = fixture
            .seeded
            .harness
            .create_account_via_tx("oracle-global-provider", NearToken::from_near(100))
            .await?;
        Contract::deploy_global_contract_code(code.clone())
            .as_account_id(publisher.clone())
            .with_signer(signer)
            .wait_until(templar_gateway_core::FinalityPolicy::Final.transaction_status())
            .send_to(&fixture.seeded.harness.network)
            .await?
            .assert_success();
        Contract::deploy(fixture.proxy.clone())
            .use_global_account_id(publisher.clone())
            .without_init_call()
            .with_signer(templar_gateway_testing::test_signer())
            .wait_until(templar_gateway_core::FinalityPolicy::Final.transaction_status())
            .send_to(&fixture.seeded.harness.network)
            .await?
            .assert_success();
        // Back the real global-linked state, not the much larger local WASM.
        // A predictor that localizes code must not reject this valid oracle.
        let actor = ManagedAccountId(fixture.proxy.clone());
        let spendable = fixture
            .seeded
            .harness
            .client()?
            .read(account::GetSpendableBalance {
                account_id: fixture.proxy.clone(),
            })
            .await?;
        fixture::finalize(
            &fixture.seeded.harness,
            &actor.0,
            &fixture
                .seeded
                .harness
                .execute(
                    &actor,
                    tx::Transfer {
                        receiver_id: fixture.outsider.0.clone(),
                        amount: spendable
                            .checked_sub(NearToken::from_millinear(500))
                            .context("global oracle spendable reserve")?,
                    },
                )
                .await?,
        )
        .await?;
        (fixture.proxy.clone(), publisher, code)
    } else {
        let target = fixture.seeded.market.market_id.clone();
        let publisher = identity(&fixture, target.as_str()).await?["global_contract_account_id"]
            .as_str()
            .context("market global provider")?
            .parse()?;
        let code = templar_gateway_testing::wasm::released(
            templar_gateway_testing::ArtifactId::Market,
            "1.6.1",
        )
        .await;
        (target, publisher, code)
    };
    // Publishing global code burns its storage fee on every publication.
    fixture::finalize(
        &fixture.seeded.harness,
        &fixture.outsider.0,
        &fixture
            .seeded
            .harness
            .execute(
                &fixture.outsider,
                tx::Transfer {
                    receiver_id: publisher.clone(),
                    amount: NearToken::from_near(50),
                },
            )
            .await?,
    )
    .await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "03-patch").await?;
    let intent = driver.intent()?;
    let market = &intent["markets"][0];
    let account = market["account"].as_str().context("market account")?;
    let before_identity = identity(&fixture, target.as_str()).await?;
    let before_storage = raw_storage(&fixture.seeded.harness, &target).await?;
    let market_storage = raw_storage(&fixture.seeded.harness, &account.parse()?).await?;
    let configuration = read(
        &fixture,
        "market.getConfiguration",
        json!({"market_id":account}),
    )
    .await?;
    let keys = read(
        &fixture,
        "account.listAccessKeys",
        json!({"account_id":account}),
    )
    .await?;
    let mut inventories = Vec::new();
    for role in market["roles"].as_array().context("roles")? {
        for side in ["old_reference", "new_reference"] {
            let reference = role[side].clone();
            inventories.push((
                reference.clone(),
                balance(&fixture, &reference, account).await?,
            ));
        }
    }
    let mut hashes = Vec::new();
    for changed in [false, true] {
        if changed {
            // A valid custom section changes bytes, not executable behavior.
            code.extend_from_slice(&[0, 2, 1, b'x']);
            Contract::deploy_global_contract_code(code.clone())
                .as_account_id(publisher.clone())
                .with_signer(templar_gateway_testing::test_signer())
                .wait_until(templar_gateway_core::FinalityPolicy::Final.transaction_status())
                .send_to(&fixture.seeded.harness.network)
                .await?
                .into_result()
                .context("redeploy behavior-preserving global provider code")?;
        }
        let expected_hash = format!("{:x}", sha2::Sha256::digest(&code));
        // Global code distribution can follow the publisher's finalized receipt.
        // Observe the actual target code before testing admission against drift.
        let report = 'visible: {
            for observation in 0..20 {
                let out = driver
                    .root
                    .join(format!("provider-{changed}-{observation}.toml"));
                let exported = cli(
                    &fixture,
                    &[
                        "patch",
                        "export",
                        target.as_str(),
                        "--out",
                        out.to_str().context("export")?,
                    ],
                    None,
                )
                .await?;
                ensure!(
                    exported.status.success(),
                    "provider export: {}",
                    String::from_utf8_lossy(&exported.stderr)
                );
                let report: Value = serde_json::from_slice(&exported.stdout)?;
                if report["effective_code_sha256"] == expected_hash {
                    break 'visible report;
                }
                ensure!(
                    changed,
                    "initial effective provider code differs: observed={}, expected={expected_hash}",
                    report["effective_code_sha256"]
                );
                fixture.seeded.harness.fast_forward(1).await?;
            }
            bail!("global provider publication never became visible: expected {expected_hash}");
        };
        hashes.push(
            report["effective_code_sha256"]
                .as_str()
                .context("effective code hash")?
                .to_owned(),
        );
    }
    ensure!(
        hashes[0] != hashes[1],
        "provider update did not change effective WASM"
    );
    ensure!(
        identity(&fixture, target.as_str()).await? == before_identity,
        "provider linkage changed"
    );
    ensure!(
        raw_storage(&fixture.seeded.harness, &target).await? == before_storage,
        "provider update changed storage"
    );
    driver.assert_blocked(&fixture, "04-swap", 20).await?;
    ensure!(
        raw_storage(&fixture.seeded.harness, &account.parse()?).await? == market_storage
            && read(
                &fixture,
                "market.getConfiguration",
                json!({"market_id":account})
            )
            .await?
                == configuration
            && read(
                &fixture,
                "account.listAccessKeys",
                json!({"account_id":account})
            )
            .await?
                == keys,
        "effective-code guard changed market state, configuration or keys"
    );
    for (reference, expected) in inventories {
        ensure!(
            balance(&fixture, &reference, account).await? == expected,
            "effective-code drift allowed inventory movement"
        );
    }
    assert_manual(&fixture, true).await
}

/// Mutate the real FT fixture's native metadata entry, not any manager output.
async fn change_ft_metadata(
    fixture: &MigrationFixture,
    reference: &Value,
    decimals: bool,
) -> Result<()> {
    // NEP-148 FungibleTokenMetadata's canonical Borsh field order.
    type FtMetadata = (
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<Vec<u8>>,
        u8,
    );
    ensure!(reference["standard"] == "ft");
    let account = reference["contract_id"]
        .as_str()
        .context("FT account")?
        .parse()?;
    let entries = raw_storage(&fixture.seeded.harness, &account).await?;
    let mut metadata: FtMetadata =
        near_sdk::borsh::from_slice(entries.get(b"m".as_slice()).context("FT metadata entry")?)?;
    let before = read(fixture, "token.getMetadata", json!({"token":reference})).await?;
    if decimals {
        metadata.6 = metadata.6.checked_add(1).context("metadata decimals")?;
    } else {
        metadata.1.push_str(" revised");
    }
    fixture
        .seeded
        .harness
        .patch_state(
            &account,
            [(b"m".to_vec(), near_sdk::borsh::to_vec(&metadata)?)],
        )
        .await?;
    let after = read(fixture, "token.getMetadata", json!({"token":reference})).await?;
    ensure!(
        after != before,
        "metadata mutation was not visible to consumers"
    );
    ensure!(
        (after["decimals"] != before["decimals"]) == decimals,
        "metadata-only row unexpectedly changed decimals"
    );
    Ok(())
}

#[rstest::rstest]
#[case::dependency_decimals_before_swap("03-patch", "04-swap", true, true)]
#[case::dependency_name_before_swap("03-patch", "04-swap", true, false)]
#[case::dependency_before_reconcile("04-swap", "05-reconcile", true, false)]
#[case::dependency_before_resume("05-reconcile", "06-resume", true, false)]
#[case::destination_before_swap("03-patch", "04-swap", false, false)]
#[case::completed_destination_decimals("07-finalize", "07-finalize", false, true)]
#[case::completed_destination_name("07-finalize", "07-finalize", false, false)]
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one metadata timeline compares unchanged dependency and campaign state across stage boundaries"
)]
async fn requires_sandbox_scripts_frozen_full_token_metadata(
    #[case] last: &str,
    #[case] blocked: &str,
    #[case] dependency: bool,
    #[case] decimals: bool,
) -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions {
        changes: ChangedRoles::Borrow,
        ..MigrationOptions::default()
    })
    .await?;
    let dependency_account = if dependency {
        let token = fixture
            .seeded
            .harness
            .deploy_ft("dependency-only", "Dependency", "DEP")
            .await?;
        let mut configuration = read(
            &fixture,
            "market.getConfiguration",
            json!({"market_id":fixture.seeded.market.market_id}),
        )
        .await?;
        configuration["collateral_asset"] = json!({"Nep141":token});
        let (account, signer) = fixture
            .seeded
            .harness
            .create_account_via_tx("unchanged-dependency", NearToken::from_near(100))
            .await?;
        Contract::deploy(account.clone())
            .use_code(
                templar_gateway_testing::wasm::released(
                    templar_gateway_testing::ArtifactId::Market,
                    "1.6.1",
                )
                .await,
            )
            .with_init_call("new", json!({"configuration":configuration}))?
            .with_signer(signer)
            .send_to(&fixture.seeded.harness.network)
            .await?
            .assert_success();
        fixture.campaign["dependency_scope_accounts"]
            .as_array_mut()
            .context("scope")?
            .push(json!(account));
        Some(account)
    } else {
        None
    };
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, last).await?;
    let intent = driver.intent()?;
    let market = &intent["markets"][0];
    let account = market["account"].as_str().context("market")?;
    let reference = if dependency {
        let consumer = intent["dependency_consumers"]
            .as_array()
            .context("dependencies")?
            .iter()
            .find(|consumer| consumer["account"] == json!(dependency_account))
            .context("distinct unchanged consumer")?;
        let reference = consumer["assets"][1]["reference"].clone();
        ensure!(
            !market["roles"]
                .as_array()
                .context("roles")?
                .iter()
                .any(
                    |role| role["old_reference"] == reference || role["new_reference"] == reference
                ),
            "dependency metadata row reused a changed-role token"
        );
        reference
    } else {
        market["roles"][0]["new_reference"].clone()
    };
    let raw = raw_storage(&fixture.seeded.harness, &account.parse()?).await?;
    let code = identity(&fixture, account).await?;
    let config = read(
        &fixture,
        "market.getConfiguration",
        json!({"market_id":account}),
    )
    .await?;
    let dependency_raw = if let Some(account) = &dependency_account {
        Some(raw_storage(&fixture.seeded.harness, account).await?)
    } else {
        None
    };
    let mut balances = Vec::new();
    for role in market["roles"].as_array().context("roles")? {
        for side in ["old_reference", "new_reference"] {
            balances.push((
                role[side].clone(),
                balance(&fixture, &role[side], account).await?,
            ));
        }
    }
    let dependency_balance = if let Some(account) = &dependency_account {
        Some(balance(&fixture, &reference, account.as_str()).await?)
    } else {
        None
    };
    change_ft_metadata(&fixture, &reference, decimals).await?;
    let starts = driver.financial_starts()?;
    let proposals = proposal_count(&fixture).await?;
    // Completed evidence is not a permanent metadata certificate. Exercise
    // the read-only entrypoint too, without demanding erasure of old evidence.
    driver.expect(Some(blocked), false, 20).await?;
    driver.expect(Some(blocked), true, 20).await?;
    ensure!(driver.financial_starts()? == starts && proposal_count(&fixture).await? == proposals);
    ensure!(
        raw_storage(&fixture.seeded.harness, &account.parse()?).await? == raw
            && identity(&fixture, account).await? == code
            && read(
                &fixture,
                "market.getConfiguration",
                json!({"market_id":account})
            )
            .await?
                == config,
        "metadata guard changed market configuration, code or storage"
    );
    for (reference, expected) in balances {
        ensure!(
            balance(&fixture, &reference, account).await? == expected,
            "metadata drift moved inventory"
        );
    }
    if let Some(account) = &dependency_account {
        ensure!(Some(raw_storage(&fixture.seeded.harness, account).await?) == dependency_raw);
        ensure!(Some(balance(&fixture, &reference, account.as_str()).await?) == dependency_balance);
    }
    assert_manual(&fixture, last != "07-finalize").await
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one interrupted real conversion retains both threshold votes and exact execution recovery evidence"
)]
async fn requires_sandbox_scripts_recovered_mpc_requires_signature_bearing_vote() -> Result<()> {
    let mut fixture = MigrationFixture::new(MigrationOptions {
        changes: ChangedRoles::Borrow,
        ..MigrationOptions::default()
    })
    .await?;
    let mut driver = Driver::new(&fixture, "transaction")?;
    driver.through(&mut fixture, "03-patch").await?;
    driver.expect(Some("04-swap"), true, 10).await?;
    let submitted = driver
        .events()?
        .into_iter()
        .rev()
        .find(|event| {
            event["stage"] == "04-swap"
                && event["event"] == "submitted"
                && event["evidence"]["phase"] == "proposal"
        })
        .context("conversion signing proposal")?;
    let id = submitted["evidence"]["proposal_id"]
        .as_u64()
        .context("MPC proposal ID")?;
    let proposal = fixture.proposal(id).await?;
    let voter = &fixture.council[0];
    let vote = fixture
        .seeded
        .harness
        .try_call_function(
            voter,
            &fixture.dao,
            "act_proposal",
            json!({"id":id,"action":"VoteApprove","proposal":proposal["kind"],"memo":null}),
        )
        .await?;
    let first = json!({
        "tx_hash":vote.operation.latest_tx_hash().context("nonexecuting vote")?.to_string(),
        "sender_account_id":voter.0
    });
    ensure!(fixture.proposal(id).await?["status"] == "InProgress");
    let first_receipts = read(&fixture, "tx.get", json!({
        "tx_hash":first["tx_hash"],"sender_account_id":first["sender_account_id"],"encoding":"json","wait_until":"FINAL"
    })).await?;
    assert_receipts(&first_receipts)?;
    ensure!(
        !first_receipts["receipts"]
            .as_array()
            .context("first vote receipts")?
            .iter()
            .any(|receipt| receipt["executor_id"] == fixture.mpc_contract.as_str()),
        "first vote unexpectedly executed MPC"
    );
    fixture.approve_pending(&driver.run).await?;
    let final_vote = fixture
        .execution_votes
        .get(&id)
        .context("signature-bearing vote")?
        .clone();
    driver.handoff(&fixture)?;
    let before_storage =
        raw_storage(&fixture.seeded.harness, &fixture.seeded.market.market_id).await?;
    let before_identity = identity(&fixture, fixture.seeded.market.market_id.as_str()).await?;
    let before_config = read(
        &fixture,
        "market.getConfiguration",
        json!({"market_id":fixture.seeded.market.market_id}),
    )
    .await?;
    // Losing stdout alone can reconcile immediately from the frozen raw hash.
    // Crash before journaling completion to exercise external receipt recovery.
    let proof = install_response_fault(&mut driver, true, true)?;
    let output = driver.invoke(Some("04-swap"), true).await?;
    ensure!(
        !output.status.success(),
        "fault did not interrupt actual broadcast response"
    );
    let execution = load(proof)?;
    ensure!(execution["sender_account_id"] == fixture.seeded.market.market_id.as_str());
    let execution_receipts = read(&fixture, "tx.get", json!({
        "tx_hash":execution["tx_hash"],"sender_account_id":execution["sender_account_id"],"encoding":"json","wait_until":"FINAL"
    })).await?;
    assert_receipts(&execution_receipts)?;
    driver.binary = PathBuf::from(BIN);
    let intent = driver.intent()?;
    let market = &intent["markets"][0];
    let role = &market["roles"][0];
    let account = market["account"].as_str().context("market")?;
    let old = balance(&fixture, &role["old_reference"], account).await?;
    let new = balance(&fixture, &role["new_reference"], account).await?;
    ensure!(
        old == 0
            && new
                == role["initial_new_balance"]
                    .as_str()
                    .context("initial output")?
                    .parse::<u128>()?
                    + role["initial_old_balance"]
                        .as_str()
                        .context("input")?
                        .parse::<u128>()?,
        "fault did not follow terminal value-conserving conversion"
    );
    let storage = raw_storage(&fixture.seeded.harness, &account.parse()?).await?;
    ensure!(
        storage == before_storage
            && identity(&fixture, account).await? == before_identity
            && read(
                &fixture,
                "market.getConfiguration",
                json!({"market_id":account})
            )
            .await?
                == before_config,
        "interrupted conversion changed market storage, configuration or restore identity"
    );
    let starts = driver.financial_starts()?;
    let proposals = proposal_count(&fixture).await?;
    let key = submitted["key"].as_str().context("MPC journal key")?;
    let directory_key = format!("{key}:mpc");
    let attempt_directory = driver
        .run
        .join("04-swap")
        .join(format!(
            "{:x}",
            sha2::Sha256::digest(directory_key.as_bytes())
        ))
        .join(format!(
            "attempt-{}",
            submitted["attempt"].as_u64().context("attempt")?
        ));
    let submission_bytes = std::fs::read(attempt_directory.join("submission.json"))?;
    ensure!(
        !attempt_directory.join("transaction.json").exists(),
        "interrupted broadcast unexpectedly retained a recovered transaction"
    );
    for (index, reference) in [&first, &final_vote].into_iter().enumerate() {
        let path = driver.root.join(format!("recovered-mpc-vote-{index}.json"));
        write_new(
            &path,
            &serde_json::to_vec_pretty(&json!({
                "schema":1,"intent_digest":intent["intent_digest"],"operations":[{
                    "stage":"04-swap","key":key,"attempt":submitted["attempt"],
                    "dao_proposal_id":id,"tx_hash":reference["tx_hash"],
                    "sender_account_id":reference["sender_account_id"],
                    "execution_tx_hash":execution["tx_hash"],
                    "execution_sender_account_id":execution["sender_account_id"]
                }]
            }))?,
        )?;
        driver.refs = Some(path);
        if index == 0 {
            driver.assert_blocked(&fixture, "04-swap", 20).await?;
            ensure!(
                !driver
                    .events()?
                    .iter()
                    .any(|event| event["stage"] == "04-swap"
                        && event["key"] == key
                        && event["event"] == "confirmed"),
                "nonexecuting vote confirmed a recovered financial execution"
            );
            ensure!(
                !attempt_directory.join("transaction.json").exists()
                    && std::fs::read(attempt_directory.join("submission.json"))?
                        == submission_bytes,
                "nonexecuting vote persisted execution evidence or rewrote immutable submission"
            );
        } else {
            driver.expect(Some("04-swap"), false, 0).await?;
            let confirmed = driver
                .events()?
                .into_iter()
                .rev()
                .find(|event| {
                    event["stage"] == "04-swap"
                        && event["key"] == key
                        && event["operation"] == "mpc"
                        && event["event"] == "confirmed"
                })
                .context("recovered MPC confirmation")?;
            ensure!(
                confirmed["evidence"]["execution_ref"]["tx_hash"] == final_vote["tx_hash"]
                    && confirmed["evidence"]["transaction_ref"]["tx_hash"] == execution["tx_hash"]
                    && confirmed["evidence"]["signature_review"]["signature_execution"]["tx_hash"]
                        == final_vote["tx_hash"],
                "recovery substituted an execution or signature reference"
            );
            let recovered_receipts = &confirmed["evidence"]["receipts"];
            assert_receipts(recovered_receipts)?;
            ensure!(
                recovered_receipts["transaction"]["hash"] == execution["tx_hash"]
                    && recovered_receipts["transaction"]["signer_id"]
                        == execution["sender_account_id"],
                "recovered receipt proof names another signed transaction"
            );
            let signature_receipts = read(&fixture, "tx.get", json!({
                "tx_hash":final_vote["tx_hash"],"sender_account_id":final_vote["sender_account_id"],"encoding":"json","wait_until":"FINAL"
            })).await?;
            assert_receipts(&signature_receipts)?;
            let signature = &confirmed["evidence"]["signature_review"]["signature_execution"];
            let index = usize::try_from(
                signature["receipt_index"]
                    .as_u64()
                    .context("signature receipt index")?,
            )?;
            ensure!(
                signature["receipt"] == signature_receipts["receipts"][index]
                    && signature["receipt"]["executor_id"] == fixture.mpc_contract.as_str()
                    && confirmed["evidence"]["signature_review"]["nonce_spent"] == true,
                "recovered signature proof is not the exact historical MPC receipt with spent nonce"
            );
        }
        ensure!(
            driver.financial_starts()? == starts && proposal_count(&fixture).await? == proposals,
            "recovery created another proposal or financial execution"
        );
        ensure!(raw_storage(&fixture.seeded.harness, &account.parse()?).await? == storage);
        ensure!(
            balance(&fixture, &role["old_reference"], account).await? == old
                && balance(&fixture, &role["new_reference"], account).await? == new,
            "recovery repeated inventory conversion"
        );
    }
    assert_manual(&fixture, true).await
}
