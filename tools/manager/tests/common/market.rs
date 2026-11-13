use anyhow::{ensure, Context as _, Result};
use near_api::{types::AccountId, Contract};
use near_token::NearToken;
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use templar_common::{interest_rate_strategy::InterestRateStrategy, market::MarketConfiguration};
use templar_gateway_methods_spec::tx;
use templar_gateway_testing::{sandbox_config, DeployedMarket, SandboxHarness};
use templar_gateway_types::{
    common::{TxExecutionStatus, WriteOperationResult},
    ManagedAccountId,
};

pub const RELEASE: &str = "1.6.1";
pub const WASM_SHA256: &str = "d75c2419527d816a8f5fcab6a585032799a38cf90e09a5a98992d5c84774562a";
pub const WASM_LENGTH: usize = 520_372;

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

// Optimistic height advancement is not receipt finality. Adverse DAO votes may
// intentionally fail downstream; seed setup separately requires full success.
pub async fn wait_for_finality(
    harness: &SandboxHarness,
    sender: &near_account_id::AccountId,
    result: &WriteOperationResult,
) -> Result<tx::GetResult> {
    let tx_hash = result
        .operation
        .latest_tx_hash()
        .context("fixture transaction hash")?;
    let finalized = harness
        .client()?
        .read(tx::Get {
            tx_hash,
            sender_account_id: sender.clone(),
            wait_until: Some(TxExecutionStatus::Final),
            encoding: tx::ValueEncoding::Json,
        })
        .await?;
    ensure!(
        finalized.status != tx::Status::Pending,
        "fixture transaction {tx_hash} from {sender} remains pending: {finalized:?}"
    );
    Ok(finalized)
}

pub async fn finalize(
    harness: &SandboxHarness,
    sender: &near_account_id::AccountId,
    result: &WriteOperationResult,
) -> Result<()> {
    let finalized = wait_for_finality(harness, sender, result).await?;
    ensure!(
        finalized.status == tx::Status::Succeeded && finalized.failed_receipts.is_empty(),
        "fixture write from {sender} must finalize all receipts: {finalized:?}"
    );
    Ok(())
}
#[derive(Clone, Copy, Debug, Default)]
pub enum SeedAssetMode {
    #[default]
    Ft,
    #[allow(
        dead_code,
        reason = "MT seeding is used only by the asset integration targets"
    )]
    Mt,
}

#[derive(Clone, Debug, Default)]
pub struct SeededMarketOptions {
    pub borrow: SeedAssetMode,
    pub collateral: SeedAssetMode,
    pub interest_rate_strategy: Option<InterestRateStrategy>,
    pub price_maximum_age_s: Option<u32>,
}

pub struct SeededMarket {
    pub harness: SandboxHarness,
    pub market: DeployedMarket,
    pub observations: Value,
}

impl SeededMarket {
    pub async fn new() -> Result<Self> {
        Self::with_assets(SeededMarketOptions::default()).await
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one ordered fixture proves released initialization and real economic state"
    )]
    pub async fn with_assets(options: SeededMarketOptions) -> Result<Self> {
        // Fast-init epochs are 60 blocks; retain history across bounded campaign replay.
        let mut node_config = sandbox_config();
        node_config
            .additional_config
            .get_or_insert_with(|| json!({}))["gc_num_epochs_to_keep"] = json!(1_000);
        let harness = SandboxHarness::start_owned_with_config(node_config).await?;
        let released = templar_gateway_testing::wasm::released(
            templar_gateway_testing::ArtifactId::Market,
            RELEASE,
        )
        .await;
        ensure!(
            released.len() == WASM_LENGTH && sha256(&released) == WASM_SHA256,
            "Market 1.6.1 release identity mismatch"
        );
        let oracle = harness.deploy_mock_oracle("layout-oracle").await?;
        let (borrow, borrow_asset) =
            deploy_asset(&harness, "layout-borrow", "mt_borrow", options.borrow).await?;
        let (collateral, collateral_asset) = deploy_asset(
            &harness,
            "layout-collateral",
            "mt_collateral",
            options.collateral,
        )
        .await?;
        let protocol = harness.create_user("layout-protocol").await?;
        let supplier = harness.create_user("layout-supplier").await?;
        let borrower = harness.create_user("layout-borrower").await?;
        let (market_id, signer) = harness
            .create_named_account("layout-market.testnet".parse()?, NearToken::from_near(100))
            .await?;
        let mut configuration: MarketConfiguration = serde_json::from_value(json!({
            "time_chunk_configuration": {"duration_ms":"1000"},
            "borrow_asset":borrow_asset,
            "collateral_asset":collateral_asset,
            "price_oracle_configuration":{
                "account_id":oracle,
                "collateral_asset_price_id":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "collateral_asset_decimals":24,
                "borrow_asset_price_id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "borrow_asset_decimals":24,
                "price_maximum_age_s":options.price_maximum_age_s.unwrap_or(3600)
            },
            "borrow_mcr_maintenance":"1.4",
            "borrow_mcr_liquidation":"1.3",
            "borrow_asset_maximum_usage_ratio":"0.99",
            "borrow_origination_fee":{"Proportional":"0.1"},
            "borrow_interest_rate_strategy":{"Linear":{"base":"0","top":"0"}},
            "borrow_maximum_duration_ms":null,
            "borrow_range":{"minimum":"1","maximum":null},
            "supply_range":{"minimum":"1","maximum":null},
            "supply_withdrawal_range":{"minimum":"1","maximum":null},
            "supply_withdrawal_fee":{"fee":{"Flat":"0"},"duration":"0","behavior":"Fixed"},
            "yield_weights":{"supply":4,"static":{(protocol.0.to_string()):1}},
            "protocol_account_id":protocol.0,
            "liquidation_maximum_spread":"0.1"
        }))?;
        if let Some(strategy) = options.interest_rate_strategy {
            configuration.borrow_interest_rate_strategy = strategy;
        }
        configuration.validate()?;
        // Only released bytes ever occupy this market account; current-source
        // market Wasm is deliberately not used even during initialization.
        Contract::deploy(market_id.clone())
            .use_code(released)
            .with_init_call("new", json!({"configuration":configuration}))?
            .with_signer(signer)
            .wait_until(templar_gateway_core::FinalityPolicy::Final.transaction_status())
            .send_to(&harness.network)
            .await?
            .assert_success();
        let market = DeployedMarket {
            market_id,
            borrow_ft_id: borrow,
            collateral_ft_id: collateral,
            configuration,
        };
        let account = ManagedAccountId(market.market_id.clone());
        for (mode, token) in [
            (options.borrow, &market.borrow_ft_id),
            (options.collateral, &market.collateral_ft_id),
        ] {
            if matches!(mode, SeedAssetMode::Ft) {
                register_market_ft(&harness, &account, token).await?;
            }
        }
        for user in [&supplier, &borrower, &protocol] {
            harness.fund_user(user, &market).await?;
            finalize(
                &harness,
                &user.0,
                &harness.storage_deposit_min(user, &market.market_id).await?,
            )
            .await?;
        }
        harness.set_asset_prices(&market, 1.0, 1.0).await?;
        finalize(
            &harness,
            &supplier.0,
            &harness.supply(&supplier, &market, 100_000).await?,
        )
        .await?;
        // Time passage is explicit: activation and yield must not depend on
        // incidental transaction/block cadence on the parent runner.
        harness.fast_forward(20).await?;
        finalize(
            &harness,
            &supplier.0,
            &harness.harvest_yield(&supplier, &market, None).await?,
        )
        .await?;
        let initial_position = harness
            .get_supply_position(&market, &supplier.0)
            .await?
            .context("missing seeded supply position")?;
        ensure!(
            initial_position.get_deposit().incoming.is_empty(),
            "supply failed to activate"
        );
        finalize(
            &harness,
            &borrower.0,
            &harness.collateralize(&borrower, &market, 100_000).await?,
        )
        .await?;
        finalize(
            &harness,
            &borrower.0,
            &harness.borrow(&borrower, &market, 10_000).await?,
        )
        .await?;
        harness.fast_forward(20).await?;
        finalize(
            &harness,
            &borrower.0,
            &harness.repay(&borrower, &market, 2_000, None).await?,
        )
        .await?;
        harness.fast_forward(20).await?;
        finalize(
            &harness,
            &protocol.0,
            &harness
                .accumulate_static_yield(&protocol, &market, None, None)
                .await?,
        )
        .await?;
        finalize(
            &harness,
            &supplier.0,
            &harness
                .create_supply_withdrawal_request(&supplier, &market, 1_000)
                .await?,
        )
        .await?;
        let supply = harness
            .get_supply_position(&market, &supplier.0)
            .await?
            .context("seeded supplier disappeared")?;
        let borrow = harness
            .get_borrow_position(&market, &borrower.0)
            .await?
            .context("seeded borrower disappeared")?;
        ensure!(
            !supply.get_deposit().active.is_zero(),
            "released fixture must contain active supplier principal"
        );
        ensure!(
            !borrow.get_borrow_asset_principal().is_zero(),
            "released fixture must contain borrower principal"
        );
        ensure!(
            !borrow.get_total_collateral_amount().is_zero(),
            "released fixture must contain borrower collateral"
        );
        let queue = harness.supply_withdrawal_queue_status(&market).await?;
        let withdrawal = harness
            .supply_withdrawal_request_status(&market, &supplier.0)
            .await?
            .context("seeded queue request disappeared")?;
        let static_yield = harness.static_yield_total(&market, &protocol.0).await?;
        ensure!(
            static_yield > 0,
            "released fixture must contain nonzero static yield"
        );
        ensure!(
            queue.length > 0,
            "released fixture must contain a queued withdrawal"
        );
        let observations = json!({
            "supplier":{"account_id":supplier.0,"position":supply},
            "borrower":{"account_id":borrower.0,"position":borrow},
            "queue":{"status":queue,"request":withdrawal},
            "static_yield":{"account_id":protocol.0,"amount":static_yield.to_string()}
        });
        Ok(Self {
            harness,
            market,
            observations,
        })
    }

    #[allow(
        dead_code,
        reason = "shared-feed peers are used only by migration cohort cases"
    )]
    /// Deploy a second released market on the same chain, with the exact same
    /// feeds and assets but independent nonempty positions and queue.
    pub async fn deploy_peer(&self, label: &str) -> Result<DeployedMarket> {
        let harness = &self.harness;
        let (market_id, signer) = harness
            .create_named_account(
                format!("{label}.testnet").parse()?,
                NearToken::from_near(100),
            )
            .await?;
        let configuration = self.market.configuration.clone();
        Contract::deploy(market_id.clone())
            .use_code(
                templar_gateway_testing::wasm::released(
                    templar_gateway_testing::ArtifactId::Market,
                    RELEASE,
                )
                .await,
            )
            .with_init_call("new", json!({"configuration":configuration}))?
            .with_signer(signer)
            .wait_until(templar_gateway_core::FinalityPolicy::Final.transaction_status())
            .send_to(&harness.network)
            .await?
            .assert_success();
        let market = DeployedMarket {
            market_id,
            borrow_ft_id: self.market.borrow_ft_id.clone(),
            collateral_ft_id: self.market.collateral_ft_id.clone(),
            configuration,
        };
        let controlled = ManagedAccountId(market.market_id.clone());
        for asset in [
            serde_json::to_value(&market.configuration.borrow_asset)?,
            serde_json::to_value(&market.configuration.collateral_asset)?,
        ] {
            if let Some(contract) = asset["Nep141"].as_str() {
                register_market_ft(harness, &controlled, &contract.parse()?).await?;
            }
        }
        let supplier = harness.create_user(&format!("{label}-supplier")).await?;
        let borrower = harness.create_user(&format!("{label}-borrower")).await?;
        for user in [&supplier, &borrower] {
            harness.fund_user(user, &market).await?;
            finalize(
                harness,
                &user.0,
                &harness.storage_deposit_min(user, &market.market_id).await?,
            )
            .await?;
        }
        finalize(
            harness,
            &supplier.0,
            &harness.supply(&supplier, &market, 50_000).await?,
        )
        .await?;
        harness.fast_forward(20).await?;
        finalize(
            harness,
            &supplier.0,
            &harness.harvest_yield(&supplier, &market, None).await?,
        )
        .await?;
        finalize(
            harness,
            &borrower.0,
            &harness.collateralize(&borrower, &market, 50_000).await?,
        )
        .await?;
        finalize(
            harness,
            &borrower.0,
            &harness.borrow(&borrower, &market, 5_000).await?,
        )
        .await?;
        harness.fast_forward(20).await?;
        finalize(
            harness,
            &borrower.0,
            &harness.repay(&borrower, &market, 1_000, None).await?,
        )
        .await?;
        finalize(
            harness,
            &supplier.0,
            &harness
                .create_supply_withdrawal_request(&supplier, &market, 500)
                .await?,
        )
        .await?;
        ensure!(
            harness
                .supply_withdrawal_queue_status(&market)
                .await?
                .length
                > 0,
            "shared-feed market queue must be populated"
        );
        Ok(market)
    }
}

async fn register_market_ft(
    harness: &SandboxHarness,
    market: &ManagedAccountId,
    contract: &AccountId,
) -> Result<()> {
    let registration = harness
        .storage_deposit(market, contract, NearToken::from_millinear(10))
        .await?;
    finalize(harness, &market.0, &registration).await
}

async fn deploy_asset(
    harness: &SandboxHarness,
    label: &str,
    token_id: &str,
    mode: SeedAssetMode,
) -> Result<(near_account_id::AccountId, Value)> {
    let account = match mode {
        SeedAssetMode::Ft => harness.deploy_ft(label, label, label).await?,
        SeedAssetMode::Mt => harness.deploy_mt(label).await?,
    };
    let asset = match mode {
        SeedAssetMode::Ft => json!({"Nep141": account}),
        SeedAssetMode::Mt => json!({"Nep245": {"contract_id": account, "token_id": token_id}}),
    };
    Ok((account, asset))
}
