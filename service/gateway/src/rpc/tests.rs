#![allow(
    clippy::too_many_lines,
    reason = "RPC integration tests are end-to-end scenarios and are easier to audit as single flows"
)]

mod account_tests;
mod contract_tests;
mod ft_tests;
mod lst_oracle_tests;
mod market_tests;
mod mt_tests;
mod oracle_tests;
mod proxy_oracle_tests;
mod pyth_tests;
mod redstone_tests;
mod ref_finance_tests;
mod registry_tests;
mod storage_tests;
mod token_tests;
mod tx_tests;
mod universal_account_tests;

use super::*;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use jsonrpsee::server::{ServerBuilder, ServerHandle};
use near_sdk::json_types::{I64, U64};
use templar_common::market::DepositMsg;
use templar_common::oracle::{
    pyth::{PriceIdentifier, PythTimestamp},
    redstone::FeedData,
};
use templar_common::primitive_types::U256;
use templar_common::Nanoseconds;
use templar_gateway_core::GatewayContext;
use templar_gateway_methods_spec::{
    account, contract, ft, lst_oracle, market, mt, oracle, proxy_oracle, proxy_oracle_governance,
    proxy_oracle_owner, pyth, redstone, ref_finance, registry, storage, token, tx,
    universal_account,
};
use templar_gateway_oracle_updates_dispatch::{
    GatewayContextBuilderOracleExt, WithPythSource, WithRedStoneSource,
};
use templar_gateway_oracle_updates_spec::oracle as oracle_updates;
use templar_gateway_store::MemoryStore;
use templar_gateway_testing::{SandboxHarness, TestController};
use templar_gateway_types::{
    common::{ContractArgs, WriteRequest},
    Base64Bytes, ContractMethodName, CryptoHash, NearGas, NearToken,
};
use templar_proxy_oracle_kernel::proxy::{FreshnessFilter, Proxy};
use templar_proxy_oracle_near_common::price_transformer::{self, PriceTransformer};
use templar_proxy_oracle_near_common::request::OracleRequest;
use templar_universal_account::{
    authentication::with_raw_string::WithRawString,
    authentication::Payload,
    transaction::{FunctionCallAction, Transaction},
    KeyParameters, NEAR_TESTNET_CHAIN_ID,
};
use url::Url;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

type TestContext = WithRedStoneSource<WithPythSource<GatewayContext>>;

struct TestStack {
    harness: SandboxHarness,
    gateway: GatewayService<TestContext>,
    handle: ServerHandle,
    controller: TestController,
}

impl TestStack {
    async fn start() -> Result<Self> {
        Self::start_with_oracle_update_config("https://hermes-beta.pyth.network".parse().unwrap())
            .await
    }

    async fn start_with_oracle_update_config(pyth_hermes_url: Url) -> Result<Self> {
        let harness = SandboxHarness::start().await?;
        let context = GatewayContext::builder(harness.network.clone())
            .with_pyth_source(pyth_hermes_url)
            .with_redstone_source(std::path::Path::new("node"))?
            .build();
        let gateway = GatewayService::spawn(
            context,
            harness.gateway_signers.clone(),
            Arc::new(MemoryStore::new()),
        )?;

        let server = ServerBuilder::default().build("127.0.0.1:0").await?;
        let local_addr = server.local_addr()?;
        let module = attach_gateway(gateway.clone())?;
        let handle = server.start(module);
        let controller = TestController::new(format!("http://{local_addr}"));

        Ok(Self {
            harness,
            gateway,
            handle,
            controller,
        })
    }

    async fn shutdown(self) {
        self.handle
            .stop()
            .expect("gateway test server should stop cleanly");
        self.handle.stopped().await;
        self.gateway.shutdown().await;
    }
}

async fn register_gateway_signer_for_ft(
    stack: &TestStack,
) -> Result<storage::GetBalanceBoundsResult> {
    register_ft_account(stack, stack.harness.gateway_signer_account_id.0.clone()).await
}

async fn register_ft_account(
    stack: &TestStack,
    account_id: near_account_id::AccountId,
) -> Result<storage::GetBalanceBoundsResult> {
    let bounds = stack
        .controller
        .request::<storage::GetBalanceBounds>(&storage::GetBalanceBounds {
            contract_id: stack.harness.ft_contract_id.clone(),
        })
        .await?;

    let _ = stack
        .controller
        .request::<storage::Deposit>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: storage::Deposit {
                contract_id: stack.harness.ft_contract_id.clone(),
                beneficiary_id: Some(account_id),
                registration_only: false,
                deposit: NearToken::from_near(1),
            },
        })
        .await?;

    Ok(bounds)
}

fn tx_hash(result: &templar_gateway_types::common::WriteOperationResult) -> CryptoHash {
    result.operation.steps[0]
        .status
        .tx_hash()
        .expect("transaction hash should be present for final execution")
}

async fn start_mock_hermes_server(vaa_hex: &str) -> Result<MockServer> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/updates/price/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "binary": {
                "data": [vaa_hex],
            }
        })))
        .mount(&server)
        .await;
    Ok(server)
}

fn pyth_price(price: f64) -> templar_common::oracle::pyth::Price {
    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX);
    let scaled_price = ((price * 10000.0).round().to_string())
        .parse::<i64>()
        .unwrap_or_default();
    templar_common::oracle::pyth::Price {
        price: I64(scaled_price),
        conf: U64(0),
        expo: -4,
        publish_time: PythTimestamp::from_ms(now_ms),
    }
}

fn redstone_price(price: f64) -> FeedData {
    let now_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX);
    let now_ms = Nanoseconds::from_ms(now_ms);
    let scaled_price = ((price * 1e8).round().to_string())
        .parse::<u128>()
        .unwrap_or_default();
    FeedData {
        price: U256::from(scaled_price).into(),
        package_timestamp: now_ms,
        write_timestamp: now_ms,
    }
}

fn assert_same_pyth_price_value(
    actual: Option<templar_common::oracle::pyth::Price>,
    expected: &templar_common::oracle::pyth::Price,
) {
    let actual = actual.expect("expected price to be present");
    assert_eq!(actual.price, expected.price);
    assert_eq!(actual.conf, expected.conf);
    assert_eq!(actual.expo, expected.expo);
}
async fn view_contract_json(
    stack: &TestStack,
    contract_id: near_account_id::AccountId,
    method_name: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value> {
    Ok(stack
        .controller
        .request::<contract::ViewFunction>(&contract::ViewFunction {
            contract_id,
            method_name: ContractMethodName(method_name.to_owned()),
            args: ContractArgs::Json(args),
        })
        .await?
        .value)
}
