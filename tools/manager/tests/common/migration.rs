//! Real-contract setup and external approval driver for script acceptance.
//! Setup's oracle configuration patch is deliberately separate from campaign
//! execution: only the actual scripts submit campaign patches and conversions.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::OpenOptions,
    io::Write as _,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context as _, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use near_account_id::AccountId;
use near_api::Contract;
use serde::Deserialize as _;
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use templar_common::{
    asset::{BorrowAsset, FungibleAsset},
    interest_rate_strategy::InterestRateStrategy,
    oracle::pyth::PriceIdentifier,
};
use templar_gateway_methods_spec::{dao, owner, proxy_oracle_governance as gov, token};
use templar_gateway_testing::{SandboxHarness, TEST_FINALITY_POLICY};
use templar_gateway_types::{ManagedAccountId, NearToken, ProposalEncoding};
use templar_proxy_oracle_kernel::proxy::{FreshnessFilter, Proxy};
use templar_proxy_oracle_near_common::{input::Source, request::OracleRequest};

#[path = "market.rs"]
mod released_market;
pub use released_market::{finalize, wait_for_finality, SeededMarket};
use released_market::{SeedAssetMode, SeededMarketOptions};

#[derive(Clone, Copy, Debug, Default)]
pub enum ChangedRoles {
    Borrow,
    Collateral,
    #[default]
    Both,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum TokenMode {
    #[default]
    Ft,
    Mt,
}

impl From<TokenMode> for SeedAssetMode {
    fn from(mode: TokenMode) -> Self {
        match mode {
            TokenMode::Ft => Self::Ft,
            TokenMode::Mt => Self::Mt,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub enum RestoreMode {
    #[default]
    Local,
    GlobalHash,
    GlobalAccount,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MigrationOptions {
    pub changes: ChangedRoles,
    pub borrow_mode: TokenMode,
    pub collateral_mode: TokenMode,
    pub restore: RestoreMode,
    pub initially_paused: bool,
}

/// One real funded route; identities use canonical MarketConfiguration JSON,
/// while `old_cli`/`new_cli` preserve the complete opaque NEP-245 token ID.
#[derive(Clone, Debug)]
pub struct MigrationAsset {
    pub role: &'static str,
    pub route_id: u32,
    pub old_asset: Value,
    pub new_asset: Value,
    pub old_cli: String,
    pub new_cli: String,
}

pub struct MigrationFixture {
    /// Owns the live neard and released, economically populated market.
    pub seeded: SeededMarket,
    pub dao: AccountId,
    pub governance: AccountId,
    pub proxy: AccountId,
    /// Underlying mock source, distinct from the proxy cached-price interface.
    pub source_oracle: AccountId,
    pub converter: AccountId,
    pub mpc_contract: AccountId,
    pub proposer: ManagedAccountId,
    pub relayer: ManagedAccountId,
    pub council: [ManagedAccountId; 2],
    pub outsider: ManagedAccountId,
    pub assets: Vec<MigrationAsset>,
    pub additional_markets: Vec<templar_gateway_testing::DeployedMarket>,
    pub campaign: Value,
    /// Actual final threshold vote transaction, indexed by real proposal ID.
    pub execution_votes: BTreeMap<u64, Value>,
}

impl MigrationFixture {
    pub async fn new(options: MigrationOptions) -> Result<Self> {
        Self::new_with_terms(options, 604_800_000_000_000, None, None).await
    }

    #[allow(
        clippy::too_many_lines,
        reason = "ordered real fixture setup keeps initialization separate from campaign writes"
    )]
    pub async fn new_with_terms(
        options: MigrationOptions,
        period_ns: u64,
        interest_rate_strategy: Option<InterestRateStrategy>,
        price_maximum_age_s: Option<u32>,
    ) -> Result<Self> {
        let mut seeded = SeededMarket::with_assets(SeededMarketOptions {
            borrow: options.borrow_mode.into(),
            collateral: options.collateral_mode.into(),
            interest_rate_strategy,
            price_maximum_age_s,
        })
        .await?;
        let harness = &seeded.harness;
        let proposer = harness.create_user("migration-proposer").await?;
        let relayer = harness.create_user("migration-relayer").await?;
        let council = [
            harness.create_user("migration-council-a").await?,
            harness.create_user("migration-council-b").await?,
        ];
        let outsider = harness.create_user("migration-outsider").await?;
        let policy = serde_json::from_value(json!({
            "roles": [
                {"name":"proposers", "kind":{"Group":[proposer.0]},
                 "permissions":["call:AddProposal"], "vote_policy":{}},
                {"name":"council", "kind":{"Group":[council[0].0,council[1].0]},
                 "permissions":["*:VoteApprove","*:VoteReject","*:Finalize"], "vote_policy":{}}
            ],
            "default_vote_policy":{"weight_kind":"RoleWeight","quorum":"2","threshold":"2"},
            "proposal_bond":NearToken::from_millinear(100),
            "proposal_period":period_ns.to_string(), "bounty_bond":"0",
            "bounty_forgiveness_period":"86400000000000"
        }))?;
        let dao = harness.deploy_sputnik_dao("migration-dao", policy).await?;
        let mpc_contract = harness.deploy_mock_signer("migration-signer").await?;
        let controlled = ManagedAccountId(seeded.market.market_id.clone());
        cli(
            harness,
            &[
                "dao",
                "mpc",
                "install-key",
                "--dao",
                dao.as_str(),
                "--mpc-contract",
                mpc_contract.as_str(),
            ],
            Some(&controlled),
        )
        .await?;

        let proxy = harness.deploy_proxy_oracle().await?;
        let source = seeded
            .market
            .configuration
            .price_oracle_configuration
            .account_id
            .clone();
        let feeds = [PriceIdentifier([0xbb; 32]), PriceIdentifier([0xcc; 32])];
        let published = i64::try_from(
            harness
                .client()?
                .read(templar_gateway_methods_spec::chain::GetBlock::default())
                .await?
                .timestamp_ns
                / 1_000_000_000,
        )?;
        for feed in feeds {
            harness
                .set_mock_oracle_pyth_price(
                    source.clone(),
                    feed,
                    Some(templar_common::oracle::pyth::Price {
                        price: near_sdk::json_types::I64(10_000),
                        conf: near_sdk::json_types::U64(0),
                        expo: -4,
                        publish_time: templar_common::oracle::pyth::PythTimestamp::from_secs(
                            published,
                        ),
                    }),
                )
                .await?;
            let proxy_definition: Proxy<Source> = Proxy::priority(
                [OracleRequest::pyth(source.clone(), feed).into()],
                FreshnessFilter::empty(),
            );
            harness
                .admin_set_proxy(proxy.clone(), feed, Some(proxy_definition))
                .await?;
        }
        harness
            .update_proxy_prices(proxy.clone(), feeds.to_vec())
            .await?;
        // An existing pause belongs to the fixture's original owner, not to the campaign.
        if options.initially_paused {
            for feed in feeds {
                finalize(
                    harness,
                    &proxy,
                    &harness
                        .call_function(
                            &ManagedAccountId(proxy.clone()),
                            &proxy,
                            "admin_set_manual_trip",
                            json!({"id":feed,"is_manually_tripped":true,"metadata":null}),
                        )
                        .await?,
                )
                .await?;
            }
        }
        let governance = deploy_governance(harness, &proxy, &dao).await?;
        let setup_dir = unique_setup_dir()?;
        configure_proxy(harness, &controlled, &proxy, &setup_dir).await?;
        seeded
            .market
            .configuration
            .price_oracle_configuration
            .account_id = proxy.clone();
        configure_restore(harness, &controlled, options.restore).await?;

        let mut assets = Vec::new();
        for (role, mode, old, id) in [
            (
                "borrow",
                options.borrow_mode,
                serde_json::to_value(&seeded.market.configuration.borrow_asset)?,
                0,
            ),
            (
                "collateral",
                options.collateral_mode,
                serde_json::to_value(&seeded.market.configuration.collateral_asset)?,
                1,
            ),
        ] {
            let changed = match options.changes {
                ChangedRoles::Borrow => role == "borrow",
                ChangedRoles::Collateral => role == "collateral",
                ChangedRoles::Both => true,
            };
            if changed {
                let new_asset = deploy_destination(harness, role, mode).await?;
                let old_cli = asset_cli(&old)?;
                let new_cli = asset_cli(&new_asset)?;
                let old_metadata = metadata(harness, &old).await?;
                let new_metadata = metadata(harness, &new_asset).await?;
                ensure!(
                    old_metadata["decimals"] == new_metadata["decimals"],
                    "fixture raw-unit decimals mismatch"
                );
                assets.push(MigrationAsset {
                    role,
                    route_id: id,
                    old_asset: old,
                    new_asset,
                    old_cli,
                    new_cli,
                });
            }
        }
        let routes = Value::Array(
            assets
                .iter()
                .map(|asset| {
                    json!({
                        "id":asset.route_id,"old_asset":asset.old_asset,"new_asset":asset.new_asset
                    })
                })
                .collect(),
        );
        let (converter, signer) = harness
            .create_account_via_tx("migration-converter", NearToken::from_near(100))
            .await?;
        let converter_wasm = templar_gateway_testing::wasm::mock_asset_converter()
            .await
            .to_vec();
        let converter_hash = hex::encode(Sha256::digest(&converter_wasm));
        Contract::deploy(converter.clone())
            .use_code(converter_wasm)
            .with_init_call("new", json!({"owner_id":proposer.0,"routes":routes}))?
            .with_signer(signer)
            .send_to(&harness.network)
            .await?
            .assert_success();
        for asset in &assets {
            register_and_fund(
                harness,
                &ManagedAccountId(converter.clone()),
                &asset.old_asset,
                0,
            )
            .await?;
            register_and_fund(
                harness,
                &ManagedAccountId(converter.clone()),
                &asset.new_asset,
                100_000_000,
            )
            .await?;
        }
        let identity = cli(
            harness,
            &["read", "chain.getIdentity", "--json", "null"],
            None,
        )
        .await?;
        let mut market = json!({"account":controlled.0});
        for asset in &assets {
            market[format!("{}_asset", asset.role)] = json!(asset.new_cli);
        }
        let campaign = json!({
            "schema":1,"environment":"sandbox","network":"testnet",
            "rpc_url":harness.network.rpc_endpoints[0].url.as_str(),
            "expected_chain_id":identity["chain_id"],"expected_genesis_hash":identity["genesis_hash"],
            "dao":dao,"mpc_contract":mpc_contract,"mpc_key_type":"ed25519",
            "submitter":proposer.0,"relayer":relayer.0,
            "converter_profile":"local-one-to-one","converter_account":converter,
            "converter_code_sha256":converter_hash,
            "approval_wait_seconds":600,"finality_wait_seconds":120,
            "dependency_scope_accounts":[controlled.0],"mpc_transport":"delegate-action",
            "markets":[market],
            "conversions":assets.iter().map(|asset|json!({"old_asset":asset.old_cli,"new_asset":asset.new_cli})).collect::<Vec<_>>()
        });
        Ok(Self {
            seeded,
            dao,
            governance,
            proxy,
            source_oracle: source,
            converter,
            mpc_contract,
            proposer,
            relayer,
            council,
            outsider,
            assets,
            additional_markets: Vec::new(),
            campaign,
            execution_votes: BTreeMap::new(),
        })
    }

    pub fn write_campaign(&self, path: &Path, transport: &str) -> Result<()> {
        ensure!(
            matches!(transport, "delegate-action" | "transaction"),
            "unknown MPC transport"
        );
        let mut campaign = self.campaign.clone();
        campaign["mpc_transport"] = json!(transport);
        write_new(path, toml::to_string_pretty(&campaign)?.as_bytes())
    }

    /// External test-driver action only. Scripts never cast these votes or advance time.
    /// Journal IDs identify proposals; the DAO counter is never used for handoff discovery.
    pub async fn approve_pending(&mut self, run_dir: &Path) -> Result<usize> {
        let operations = journal_proposals(run_dir)?;
        let ids: BTreeSet<_> = operations
            .iter()
            .map(|operation| operation.proposal_id)
            .collect();
        let proof_dir = run_dir.join("fixture-votes");
        std::fs::create_dir_all(&proof_dir)?;
        let mut approved = 0;
        for id in ids {
            let proof_path = proof_dir.join(format!("{id}.json"));
            if proof_path.exists() {
                self.execution_votes
                    .insert(id, serde_json::from_slice(&std::fs::read(&proof_path)?)?);
            }
            let mut stored = self.proposal(id).await?;
            if stored["status"] != "InProgress" {
                continue;
            }
            for voter in &self.council {
                if stored["votes"].get(voter.as_str()).is_some() {
                    continue;
                }
                let vote = self.seeded.harness.try_call_function(
                    voter, &self.dao, "act_proposal",
                    json!({"id":id,"action":"VoteApprove","proposal":stored["kind"],"memo":null}),
                ).await?;
                wait_for_finality(&self.seeded.harness, &voter.0, &vote).await?;
                stored = self.proposal(id).await?;
                let votes = stored["votes"]
                    .as_object()
                    .context("Sputnik vote table")?
                    .len();
                ensure!(votes <= 2, "unexpected extra fixture votes");
                ensure!(
                    votes != 1 || stored["status"] == "InProgress",
                    "one council member bypassed the two-member approval threshold"
                );
                if stored["status"] != "InProgress" {
                    let proof = json!({
                        "tx_hash":vote.operation.latest_tx_hash().context("final vote transaction hash")?.to_string(),
                        "sender_account_id":voter.0,
                        "dao_proposal_id":id,
                        "proposal":stored,
                        "result":vote
                    });
                    write_new(&proof_path, &serde_json::to_vec_pretty(&proof)?)?;
                    self.execution_votes.insert(id, proof);
                    approved += 1;
                    break;
                }
            }
        }
        // Governance TTLs are real and positive. Explicit advancement follows votes,
        // even on resumed invocations where the previous vote already executed.
        self.seeded.harness.fast_forward(5).await?;
        Ok(approved)
    }

    pub async fn proposal(&self, id: u64) -> Result<Value> {
        Ok(serde_json::to_value(
            self.seeded
                .harness
                .client()?
                .read(dao::GetProposal {
                    dao_id: self.dao.clone(),
                    id,
                })
                .await?,
        )?)
    }

    pub fn write_execution_refs(&self, run_dir: &Path, path: &Path) -> Result<()> {
        let intent: Value = serde_json::from_slice(&std::fs::read(run_dir.join("intent.json"))?)?;
        let intent_digest = intent["intent_digest"]
            .as_str()
            .context("frozen intent digest")?;
        let mut operations = Vec::new();
        for operation in journal_proposals(run_dir)? {
            let proof = if let Some(proof) = self.execution_votes.get(&operation.proposal_id) {
                proof.clone()
            } else {
                let proof_path = run_dir
                    .join("fixture-votes")
                    .join(format!("{}.json", operation.proposal_id));
                if !proof_path.exists() {
                    continue;
                }
                serde_json::from_slice(&std::fs::read(proof_path)?)?
            };
            ensure!(
                proof["dao_proposal_id"] == operation.proposal_id,
                "vote proof proposal mismatch"
            );
            operations.push(json!({
                "stage":operation.stage,"key":operation.key,"attempt":operation.attempt,
                "dao_proposal_id":operation.proposal_id,"tx_hash":proof["tx_hash"],
                "sender_account_id":proof["sender_account_id"]
            }));
        }
        write_new(
            path,
            &serde_json::to_vec_pretty(&json!({
                "schema":1,"intent_digest":intent_digest,"operations":operations
            }))?,
        )?;
        Ok(())
    }

    /// Another economically populated market consumes the *same* two proxy
    /// feeds and token inventories. No campaign operation is performed here.
    pub async fn add_shared_market(&mut self, restore: RestoreMode) -> Result<()> {
        let harness = &self.seeded.harness;
        let label = format!("migration-cohort-{}", self.additional_markets.len());
        let market = self.seeded.deploy_peer(&label).await?;
        let controlled = ManagedAccountId(market.market_id.clone());
        cli(
            harness,
            &[
                "dao",
                "mpc",
                "install-key",
                "--dao",
                self.dao.as_str(),
                "--mpc-contract",
                self.mpc_contract.as_str(),
            ],
            Some(&controlled),
        )
        .await?;
        configure_restore(harness, &controlled, restore).await?;
        let mut record = json!({"account":market.market_id});
        for asset in &self.assets {
            record[format!("{}_asset", asset.role)] = json!(asset.new_cli);
        }
        self.campaign["markets"]
            .as_array_mut()
            .context("markets")?
            .push(record);
        self.campaign["dependency_scope_accounts"]
            .as_array_mut()
            .context("scope")?
            .push(json!(market.market_id));
        self.additional_markets.push(market);
        Ok(())
    }

    pub async fn register_destinations(&self) -> Result<()> {
        for market in std::iter::once(&self.seeded.market).chain(&self.additional_markets) {
            for asset in &self.assets {
                register_and_fund(
                    &self.seeded.harness,
                    &ManagedAccountId(market.market_id.clone()),
                    &asset.new_asset,
                    0,
                )
                .await?;
            }
        }
        Ok(())
    }

    pub async fn approve_id(&mut self, id: u64) -> Result<()> {
        let mut stored = self.proposal(id).await?;
        for voter in &self.council {
            let vote = self
                .seeded
                .harness
                .try_call_function(
                    voter,
                    &self.dao,
                    "act_proposal",
                    json!({"id":id,"action":"VoteApprove","proposal":stored["kind"],"memo":null}),
                )
                .await?;
            stored = self.proposal(id).await?;
            if stored["status"] != "InProgress" {
                ensure!(
                    stored["votes"]
                        .as_object()
                        .context("Sputnik vote table")?
                        .len()
                        == 2,
                    "recovery handoff requires the actual two-member threshold vote"
                );
                self.execution_votes.insert(id, json!({
                    "tx_hash":vote.operation.latest_tx_hash().context("vote transaction")?.to_string(),
                    "sender_account_id":voter.0,"dao_proposal_id":id,"proposal":stored,"result":vote
                }));
                return Ok(());
            }
        }
        anyhow::bail!("two-member threshold did not execute proposal {id}")
    }
}

struct JournalProposal {
    stage: String,
    key: String,
    attempt: u64,
    proposal_id: u64,
}

fn journal_proposals(run_dir: &Path) -> Result<Vec<JournalProposal>> {
    let path = run_dir.join("journal.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path)?;
    ensure!(
        text.is_empty() || text.ends_with('\n'),
        "truncated campaign journal"
    );
    let mut seen = BTreeSet::new();
    let mut proposals = Vec::new();
    for line in text.lines() {
        let event: Value = serde_json::from_str(line)?;
        if event["event"] != "submitted" {
            continue;
        }
        let Some(id) = event["evidence"]["proposal_id"].as_u64() else {
            continue;
        };
        let stage = event["stage"].as_str().context("journal stage")?.to_owned();
        let key = event["key"].as_str().context("journal key")?.to_owned();
        let attempt = event["attempt"].as_u64().context("journal attempt")?;
        if seen.insert((stage.clone(), key.clone(), attempt, id)) {
            proposals.push(JournalProposal {
                stage,
                key,
                attempt,
                proposal_id: id,
            });
        }
    }
    Ok(proposals)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn unique_setup_dir() -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "migration-fixture-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::create_dir(&path)?;
    Ok(path)
}

fn text(path: &Path) -> Result<&str> {
    path.to_str().context("fixture path must be UTF-8")
}

async fn cli(
    harness: &SandboxHarness,
    args: &[&str],
    signer: Option<&ManagedAccountId>,
) -> Result<Value> {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_tmplrmgr"));
    command
        .args([
            "--network",
            "testnet",
            "--rpc-url",
            harness.network.rpc_endpoints[0].url.as_str(),
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
    let output = command.output().await?;
    ensure!(
        output.status.success(),
        "setup CLI {args:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&output.stdout).with_context(|| format!("setup CLI JSON for {args:?}"))
}

async fn configure_proxy(
    harness: &SandboxHarness,
    market: &ManagedAccountId,
    proxy: &AccountId,
    dir: &Path,
) -> Result<()> {
    let changes = dir.join("oracle-changes.json");
    let spec = dir.join("oracle-patch.toml");
    let plan = dir.join("oracle-plan.json");
    write_new(
        &changes,
        &serde_json::to_vec(&json!([
            {"op":"replace","path":"/price_oracle_configuration/account_id","value":proxy}
        ]))?,
    )?;
    cli(
        harness,
        &[
            "market",
            "generate-patch",
            "config",
            market.as_str(),
            "--changes",
            text(&changes)?,
            "--out",
            text(&spec)?,
        ],
        None,
    )
    .await?;
    let public_key = templar_gateway_testing::test_secret_key()?
        .public_key()
        .to_string();
    cli(
        harness,
        &[
            "patch",
            "plan",
            text(&spec)?,
            "--out",
            text(&plan)?,
            "--signer-id",
            market.as_str(),
            "--public-key",
            &public_key,
        ],
        None,
    )
    .await?;
    cli(harness, &["patch", "dry-run", "--plan", text(&plan)?], None).await?;
    let applied = cli(
        harness,
        &["patch", "apply", "--plan", text(&plan)?],
        Some(market),
    )
    .await?;
    write_new(
        &dir.join("oracle-setup-application.json"),
        &serde_json::to_vec_pretty(&applied)?,
    )
}

async fn configure_restore(
    harness: &SandboxHarness,
    market: &ManagedAccountId,
    mode: RestoreMode,
) -> Result<()> {
    if matches!(mode, RestoreMode::Local) {
        return Ok(());
    }
    let bytes = templar_gateway_testing::wasm::released(
        templar_gateway_testing::ArtifactId::Market,
        released_market::RELEASE,
    )
    .await;
    let deploy = match mode {
        RestoreMode::Local => unreachable!(),
        RestoreMode::GlobalHash => Contract::deploy(market.0.clone())
            .use_global_hash(harness.deploy_global_contract(bytes).await?),
        RestoreMode::GlobalAccount => {
            let (publisher, signer) = harness
                .create_account_via_tx("migration-global", NearToken::from_near(100))
                .await?;
            Contract::deploy_global_contract_code(bytes)
                .as_account_id(publisher.clone())
                .with_signer(signer)
                .send_to(&harness.network)
                .await?
                .assert_success();
            Contract::deploy(market.0.clone()).use_global_account_id(publisher)
        }
    };
    deploy
        .without_init_call()
        .with_signer(templar_gateway_testing::test_signer())
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(&harness.network)
        .await?
        .assert_success();
    Ok(())
}

async fn deploy_governance(
    harness: &SandboxHarness,
    proxy: &AccountId,
    dao: &AccountId,
) -> Result<AccountId> {
    let (governance, signer) = harness
        .create_account_via_tx("migration-governance", NearToken::from_near(100))
        .await?;
    let ttl = "1000000000";
    Contract::deploy(governance.clone())
        .use_code(
            templar_gateway_testing::wasm::proxy_governance()
                .await
                .to_vec(),
        )
        .with_init_call(
            "new",
            json!({
                "proxy_oracle_id":proxy,"admin_id":dao,
                "policy":{"reflexive_ttls":{"set_policy":ttl,"set_role":ttl,"self_upgrade":ttl},
                    "default_target":{"ttl":ttl,"role":"Admin"},"method_policies":{}}
            }),
        )?
        .with_signer(signer)
        .send_to(&harness.network)
        .await?
        .assert_success();
    harness
        .execute(
            &ManagedAccountId(proxy.clone()),
            owner::ProposeOwner {
                contract_id: proxy.clone(),
                account_id: Some(governance.clone()),
            },
        )
        .await?;
    // Initial ownership handover is fixture setup, signed by the DAO account's
    // fixture deployment key; campaign governance always goes through proposals.
    let admin = ManagedAccountId(dao.clone());
    harness
        .execute(
            &admin,
            gov::CreateProposal {
                governance_id: governance.clone(),
                id: 0,
                operation: serde_json::from_value(json!({"TargetFunctionCall":{
                    "method_name":"own_accept_owner","args":STANDARD.encode(b"{}"),
                    "attached_deposit":"1","gas":"50000000000000"
                }}))?,
                requested_ttl: templar_common::Nanoseconds::zero(),
                encoding: ProposalEncoding::Json,
                gas: None,
            },
        )
        .await?;
    harness.fast_forward(5).await?;
    harness
        .execute(
            &admin,
            gov::ExecuteProposal {
                governance_id: governance.clone(),
                id: 0,
                gas: None,
            },
        )
        .await?;
    Ok(governance)
}

async fn deploy_destination(
    harness: &SandboxHarness,
    role: &str,
    mode: TokenMode,
) -> Result<Value> {
    match mode {
        TokenMode::Ft => {
            Ok(json!({"Nep141":harness.deploy_ft(&format!("converted-{role}"),role,role).await?}))
        }
        TokenMode::Mt => {
            let (account, signer) = harness
                .create_account_via_tx(&format!("converted-{role}"), NearToken::from_near(100))
                .await?;
            let token_id = format!("converted:{role}:fixture");
            Contract::deploy(account.clone())
                .use_code(templar_gateway_testing::wasm::mt().await.to_vec())
                .with_init_call("new", json!({"extra_token_ids":[token_id]}))?
                .with_signer(signer)
                .send_to(&harness.network)
                .await?
                .assert_success();
            Ok(json!({"Nep245":{"contract_id":account,"token_id":token_id}}))
        }
    }
}

fn decode_asset(asset: &Value) -> Result<FungibleAsset<BorrowAsset>> {
    Ok(FungibleAsset::deserialize(asset)?)
}

fn asset_cli(asset: &Value) -> Result<String> {
    Ok(decode_asset(asset)?.to_string())
}

fn token_reference(asset: &Value) -> Result<Value> {
    Ok(serde_json::to_value(token::TokenReference::from(
        &decode_asset(asset)?,
    ))?)
}

async fn metadata(harness: &SandboxHarness, asset: &Value) -> Result<Value> {
    let params = serde_json::to_string(&json!({"token":token_reference(asset)?}))?;
    cli(
        harness,
        &["read", "token.getMetadata", "--json", &params],
        None,
    )
    .await
}

async fn register_and_fund(
    harness: &SandboxHarness,
    user: &ManagedAccountId,
    asset: &Value,
    amount: u128,
) -> Result<()> {
    let asset = decode_asset(asset)?;
    harness
        .storage_deposit(user, asset.contract_id(), NearToken::from_millinear(10))
        .await?;
    // The independent NEP-145 controller sponsors this real MT account.
    // Mint funded output only; never fabricate old inventory.
    if amount > 0 {
        if let Some(token_id) = asset.nep245_token_id() {
            harness
                .mint_mt(user, asset.contract_id(), token_id, amount)
                .await?;
        } else {
            harness.mint(user, asset.contract_id(), amount).await?;
        }
    }
    Ok(())
}
