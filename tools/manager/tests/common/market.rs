use anyhow::{ensure, Context as _, Result};
use near_api::Contract;
use near_token::NearToken;
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use templar_common::market::MarketConfiguration;
use templar_gateway_methods_spec::tx;
use templar_gateway_testing::{DeployedMarket, SandboxHarness};
use templar_gateway_types::{common::TxExecutionStatus, ManagedAccountId};

pub const RELEASE: &str = "1.6.1";
pub const WASM_SHA256: &str = "d75c2419527d816a8f5fcab6a585032799a38cf90e09a5a98992d5c84774562a";
pub const WASM_LENGTH: usize = 520_372;

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub struct SeededMarket {
    pub harness: SandboxHarness,
    pub market: DeployedMarket,
    pub observations: Value,
}

impl SeededMarket {
    #[allow(
        clippy::too_many_lines,
        reason = "one ordered fixture proves released initialization and real economic state"
    )]
    pub async fn new() -> Result<Self> {
        let harness = SandboxHarness::start_owned().await?;
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
        let borrow = harness.deploy_ft("layout-borrow", "Borrow", "B").await?;
        let collateral = harness
            .deploy_ft("layout-collateral", "Collateral", "C")
            .await?;
        let protocol = harness.create_user("layout-protocol").await?;
        let supplier = harness.create_user("layout-supplier").await?;
        let borrower = harness.create_user("layout-borrower").await?;
        let (market_id, signer) = harness
            .create_named_account("layout-market.testnet".parse()?, NearToken::from_near(100))
            .await?;
        let configuration: MarketConfiguration = serde_json::from_value(json!({
            "time_chunk_configuration": {"duration_ms":"1000"},
            "borrow_asset":{"Nep141":borrow},
            "collateral_asset":{"Nep141":collateral},
            "price_oracle_configuration":{
                "account_id":oracle,
                "collateral_asset_price_id":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "collateral_asset_decimals":24,
                "borrow_asset_price_id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "borrow_asset_decimals":24,
                "price_maximum_age_s":3600
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
        for token in [&market.borrow_ft_id, &market.collateral_ft_id] {
            let registration = harness
                .storage_deposit(&account, token, NearToken::from_millinear(10))
                .await?;
            let finalized = harness
                .client()?
                .read(tx::Get {
                    tx_hash: registration
                        .operation
                        .latest_tx_hash()
                        .context("market registration transaction hash")?,
                    sender_account_id: account.0.clone(),
                    wait_until: Some(TxExecutionStatus::Final),
                    encoding: tx::ValueEncoding::Json,
                })
                .await?;
            ensure!(
                finalized.status == tx::Status::Succeeded && finalized.failed_receipts.is_empty(),
                "market registration must finalize including native refunds before checkpoints"
            );
        }
        for user in [&supplier, &borrower, &protocol] {
            harness.fund_user(user, &market).await?;
            harness.storage_deposit_min(user, &market.market_id).await?;
        }
        harness.set_asset_prices(&market, 1.0, 1.0).await?;
        harness.supply(&supplier, &market, 100_000).await?;
        // Time passage is explicit: activation and yield must not depend on
        // incidental transaction/block cadence on the parent runner.
        harness.fast_forward(20).await?;
        harness.harvest_yield(&supplier, &market, None).await?;
        let initial_position = harness
            .get_supply_position(&market, &supplier.0)
            .await?
            .context("missing seeded supply position")?;
        ensure!(
            initial_position.get_deposit().incoming.is_empty(),
            "supply failed to activate"
        );
        harness.collateralize(&borrower, &market, 100_000).await?;
        harness.borrow(&borrower, &market, 10_000).await?;
        harness.fast_forward(20).await?;
        harness.repay(&borrower, &market, 2_000, None).await?;
        harness.fast_forward(20).await?;
        harness
            .accumulate_static_yield(&protocol, &market, None, None)
            .await?;
        harness
            .create_supply_withdrawal_request(&supplier, &market, 1_000)
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
        harness.fast_forward(10).await?;
        Ok(Self {
            harness,
            market,
            observations,
        })
    }
}
