use std::{collections::HashMap, sync::Arc};

use clap::Parser;
use futures::{StreamExt, TryStreamExt};
use near_crypto::{SecretKey, Signer};
use near_jsonrpc_client::{
    errors::JsonRpcError,
    methods::{query::RpcQueryError, tx::RpcTransactionError},
    JsonRpcClient,
};
use near_primitives::{
    action::{Action, FunctionCallAction},
    hash::CryptoHash,
    transaction::{Transaction, TransactionV0},
};
use near_sdk::{serde_json::json, AccountId};
use rpc::{get_contract_version, is_v1_0_0};
use templar_common::market::MarketConfiguration;
use tracing::{debug, error, info, instrument};

pub mod rpc;

use crate::rpc::{
    get_access_key_data, send_tx, serialize_and_encode, view, BorrowPositions, Network, DEFAULT_GAS,
};

pub type AccumulatorResult<T = ()> = Result<T, AccumulatorError>;

/// Errors that can occur during accumulations
#[derive(Debug, thiserror::Error)]
pub enum AccumulatorError {
    /// Failed to query view method
    #[error("Failed to query view method: {0}")]
    ViewMethodError(#[from] JsonRpcError<RpcQueryError>),
    /// Failed to get access key data
    #[error("Failed to get access key data: {0}")]
    AccessKeyDataError(JsonRpcError<RpcQueryError>),
    /// Got wrong response kind from RPC
    #[error("Got wrong response kind from RPC: {0}")]
    WrongResponseKind(String),
    /// Failed to send transaction
    #[error("Failed to send transaction: {0}")]
    SendTransactionError(#[from] JsonRpcError<RpcTransactionError>),
    /// Failed to deserialize response
    #[error("Failed to deserialize response: {0}")]
    DeserializeError(#[from] near_sdk::serde_json::Error),
    /// Timeout exceeded
    #[error("Timeout exceeded after {0}s (waited {1}s)")]
    TimeoutError(u64, u64),
    /// No outcome for transaction
    #[error("No outcome for transaction: {0}")]
    NoOutcome(String),
}

#[derive(Debug, Clone, Parser)]
pub struct Args {
    /// Registries to run accumulator for
    #[arg(short, long, env = "REGISTRIES_ACCOUNT_IDS", value_delimiter = ' ')]
    pub registries: Vec<AccountId>,
    /// Signer key to use for signing transactions
    #[arg(short = 'k', long, env = "SIGNER_KEY")]
    pub signer_key: SecretKey,
    /// Signer 'Account'
    #[arg(short, long, env = "SIGNER_ACCOUNT_ID")]
    pub signer_account: AccountId,
    /// Network to run accumulator on
    #[arg(short, long, env = "NETWORK", default_value_t = Network::Testnet)]
    pub network: Network,
    /// Custom RPC URL (overrides default network RPC)
    #[arg(long, env = "RPC_URL")]
    pub rpc_url: Option<String>,
    /// Timeout for transactions
    #[arg(short, long, env = "TIMEOUT", default_value_t = 60)]
    pub timeout: u64,
    /// Interval between accumulations in seconds
    #[arg(short, long, default_value_t = 600, env = "INTERVAL")]
    pub interval: u64,
    /// Interval between static accumulations in seconds
    #[arg(long, default_value_t = 86_400, env = "STATIC_INTERVAL")]
    pub static_interval: u64,
    /// Registry refresh interval in seconds
    #[arg(
        short = 'R',
        long,
        default_value_t = 3600,
        env = "REGISTRY_REFRESH_INTERVAL"
    )]
    pub registry_refresh_interval: u64,
    /// Concurrency for accumulation tasks
    #[arg(short, long, default_value_t = 4, env = "CONCURRENCY")]
    pub concurrency: usize,
}

impl std::fmt::Display for Args {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "registries: {:?}\nsigner_account: {}\nnetwork: {}\ntimeout: {}\ninterval: {}\nstatic_interval: {}\nregistry_refresh_interval: {}\nconcurrency: {}",
            self.registries,
            self.signer_account,
            self.network,
            self.timeout,
            self.interval,
            self.static_interval,
            self.registry_refresh_interval,
            self.concurrency
        )
    }
}

pub struct Accumulator {
    client: JsonRpcClient,
    signer: Arc<Signer>,
    pub market: AccountId,
    timeout: u64,
}

impl Accumulator {
    #[must_use]
    pub fn new(
        client: JsonRpcClient,
        signer: Arc<Signer>,
        market: AccountId,
        timeout: u64,
    ) -> Self {
        Self {
            client,
            signer,
            market,
            timeout,
        }
    }

    fn create_tx(
        &self,
        borrow: &AccountId,
        nonce: u64,
        block_hash: CryptoHash,
        method_name: String,
    ) -> Transaction {
        Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: self.market.clone(),
            block_hash,
            signer_id: self.signer.get_account_id(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name,
                args: serialize_and_encode(json!({
                    "account_id": borrow,
                })),
                gas: DEFAULT_GAS,
                deposit: 0,
            }))],
        })
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn accumulate(&self, borrow: AccountId, method: &str) -> AccumulatorResult {
        info!("Starting accumulation for market: {}", self.market);

        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

        let accumulate_tx = self.create_tx(&borrow, nonce, block_hash, method.to_owned());

        match send_tx(&self.client, &self.signer, self.timeout, accumulate_tx).await {
            Ok(_) => {
                info!("Accumulation successful");
            }
            Err(e) => {
                error!("Accumulation failed: {e}");
            }
        }

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_borrows(&self) -> AccumulatorResult<BorrowPositions> {
        let mut all_positions: BorrowPositions = HashMap::new();

        let page_size = 100;
        let mut current_offset = 0;
        let mut params = json!({
            "offset": current_offset,
            "count": page_size,
        });

        loop {
            let page = view::<BorrowPositions>(
                &self.client,
                self.market.clone(),
                "list_borrow_positions",
                params.clone(),
            )
            .await?;
            let fetched = page.len();
            all_positions.extend(page);
            current_offset += page_size;
            params["offset"] = current_offset.into();

            if fetched < page_size {
                break;
            }
        }

        Ok(all_positions)
    }

    #[instrument(skip(self), level = "info")]
    pub async fn run_borrow_accumulations(&self, concurrency: usize) -> AccumulatorResult {
        let borrows = match self.get_borrows().await {
            Ok(borrows) => borrows,
            Err(err) => {
                error!("Failed to fetch borrows for {}: {err}", self.market);
                return Ok(());
            }
        };

        if borrows.is_empty() {
            return Ok(());
        }

        futures::stream::iter(borrows)
            .map(|(account_id, _)| async move {
                if let Err(err) = self.accumulate(account_id.clone(), "apply_interest").await {
                    error!(
                        "Borrow accumulation failed for market {} account {}: {err}",
                        self.market, account_id
                    );
                }
            })
            .buffer_unordered(concurrency)
            .for_each(|_| async {})
            .await;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn supports_static_yield(&self) -> AccumulatorResult<bool> {
        let Some(version) = get_contract_version(&self.client, &self.market).await else {
            return Ok(false);
        };

        Ok(!is_v1_0_0(&version))
    }

    #[instrument(skip(self), level = "info")]
    pub async fn run_static_accumulations(&self, concurrency: usize) -> AccumulatorResult {
        if !self.supports_static_yield().await? {
            debug!(
                "{} market does not support static yield accumulation",
                self.market
            );
            return Ok(());
        }

        let static_accounts = match self.get_static_accounts().await {
            Ok(accounts) => accounts,
            Err(err) => {
                error!("Failed to fetch static accounts for {}: {err}", self.market);
                return Ok(());
            }
        };

        if static_accounts.is_empty() {
            return Ok(());
        }

        futures::stream::iter(static_accounts)
            .map(|account_id| async move {
                if let Err(err) = self
                    .accumulate(account_id.clone(), "accumulate_static_yield")
                    .await
                {
                    error!(
                        "Static accumulation failed for market {} account {}: {err}",
                        self.market, account_id
                    );
                }
            })
            .buffer_unordered(concurrency)
            .for_each(|_| async {})
            .await;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_static_accounts(&self) -> AccumulatorResult<Vec<AccountId>> {
        let configuration: MarketConfiguration = view(
            &self.client,
            self.market.clone(),
            "get_configuration",
            json!({}),
        )
        .await?;

        Ok(configuration
            .yield_weights
            .r#static
            .keys()
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::ContractSourceMetadata;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use base64::Engine;
    use near_crypto::{InMemorySigner, KeyType};
    use near_jsonrpc_client::methods::tx::RpcTransactionResponse;
    use near_jsonrpc_primitives::types::query::{QueryResponseKind, RpcQueryResponse};
    use near_primitives::borsh::BorshDeserialize;
    use near_primitives::views::{
        ExecutionMetadataView, ExecutionOutcomeView, ExecutionOutcomeWithIdView,
        ExecutionStatusView, FinalExecutionOutcomeView, FinalExecutionOutcomeViewEnum,
        FinalExecutionStatus, SignedTransactionView, TxExecutionStatus,
    };
    use near_primitives::{hash::CryptoHash, views::CallResult};
    use near_sdk::serde_json::{json, Value as JsonValue};
    use std::collections::HashSet;
    use std::str::FromStr;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use templar_common::{
        asset::FungibleAsset,
        borrow::BorrowPosition,
        dec,
        fee::{Fee, TimeBasedFee},
        interest_rate_strategy::InterestRateStrategy,
        market::{PriceOracleConfiguration, YieldWeights},
        number::Decimal,
        oracle::pyth::PriceIdentifier,
        time_chunk::TimeChunkConfiguration,
    };
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, Request, ResponseTemplate,
    };

    fn build_accumulator(server: &MockServer, market_id: &str) -> Accumulator {
        let client = JsonRpcClient::connect(server.uri());
        let signer = Arc::new(InMemorySigner::from_secret_key(
            market_id.parse().unwrap(),
            SecretKey::from_random(KeyType::ED25519),
        ));

        Accumulator::new(client, signer, market_id.parse().unwrap(), 10)
    }

    fn rpc_success_response(payload: &JsonValue, id: &JsonValue) -> JsonValue {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": payload,
        })
    }

    fn parse_method(request: &Request) -> String {
        let body: JsonValue =
            near_sdk::serde_json::from_slice(&request.body).expect("request body to be valid json");
        body.get("method")
            .and_then(JsonValue::as_str)
            .expect("method to exist")
            .to_owned()
    }

    fn call_result_response(result_bytes: Vec<u8>) -> RpcQueryResponse {
        RpcQueryResponse {
            kind: QueryResponseKind::CallResult(CallResult {
                result: result_bytes,
                logs: Vec::new(),
            }),
            block_height: 1,
            block_hash: CryptoHash::default(),
        }
    }

    fn access_key_response(nonce: u64) -> RpcQueryResponse {
        RpcQueryResponse {
            kind: QueryResponseKind::AccessKey(near_primitives::views::AccessKeyView {
                nonce,
                permission: near_primitives::views::AccessKeyPermissionView::FullAccess,
            }),
            block_height: 1,
            block_hash: CryptoHash::default(),
        }
    }

    fn parse_query_request(request: &Request) -> (JsonValue, JsonValue) {
        let body: JsonValue =
            near_sdk::serde_json::from_slice(&request.body).expect("request body to be valid json");
        let params = body
            .get("params")
            .cloned()
            .expect("query params to exist in request");
        let id = body.get("id").cloned().unwrap_or_else(|| json!("1"));

        (params, id)
    }

    fn decode_args(params: &JsonValue) -> JsonValue {
        let args_base64 = params
            .get("args_base64")
            .and_then(JsonValue::as_str)
            .expect("args_base64 present");
        let decoded = BASE64_STANDARD
            .decode(args_base64)
            .expect("args_base64 to decode");

        near_sdk::serde_json::from_slice(&decoded).expect("decoded args to be valid json")
    }

    fn decode_signed_tx(request: &Request) -> near_primitives::transaction::SignedTransaction {
        let body: JsonValue =
            near_sdk::serde_json::from_slice(&request.body).expect("request body to be valid json");
        let signed_tx_b64 = body["params"]["signed_tx_base64"]
            .as_str()
            .expect("signed_tx_base64 present");
        let raw = near_primitives::serialize::from_base64(signed_tx_b64).expect("valid base64");

        near_primitives::transaction::SignedTransaction::try_from_slice(&raw)
            .expect("signed tx to deserialize")
    }

    fn sample_configuration() -> MarketConfiguration {
        MarketConfiguration {
            time_chunk_configuration: TimeChunkConfiguration::new(1),
            borrow_asset: FungibleAsset::nep141("borrow.testnet".parse().unwrap()),
            collateral_asset: FungibleAsset::nep141("collateral.testnet".parse().unwrap()),
            price_oracle_configuration: PriceOracleConfiguration {
                account_id: "oracle.testnet".parse().unwrap(),
                collateral_asset_price_id: PriceIdentifier([1; 32]),
                collateral_asset_decimals: 24,
                borrow_asset_price_id: PriceIdentifier([2; 32]),
                borrow_asset_decimals: 24,
                price_maximum_age_s: 60,
            },
            borrow_mcr_maintenance: Decimal::from_str("1.25").unwrap(),
            borrow_mcr_liquidation: Decimal::from_str("1.2").unwrap(),
            borrow_asset_maximum_usage_ratio: Decimal::from_str("0.9").unwrap(),
            borrow_origination_fee: Fee::Proportional(Decimal::from_str("0.01").unwrap()),
            borrow_interest_rate_strategy: InterestRateStrategy::piecewise(
                Decimal::ZERO,
                dec!("0.8"),
                dec!("0.02"),
                dec!("0.5"),
            )
            .unwrap(),
            borrow_maximum_duration_ms: None,
            borrow_range: (1, None).try_into().unwrap(),
            supply_range: (1, None).try_into().unwrap(),
            supply_withdrawal_range: (1, None).try_into().unwrap(),
            supply_withdrawal_fee: TimeBasedFee::zero(),
            yield_weights: YieldWeights::new_with_supply_weight(100)
                .with_static("static.one.testnet".parse().unwrap(), 50)
                .with_static("static.two.testnet".parse().unwrap(), 25),
            protocol_account_id: "protocol.testnet".parse().unwrap(),
            liquidation_maximum_spread: Decimal::from_str("0.05").unwrap(),
        }
    }

    #[tokio::test]
    async fn get_borrows_paginates_until_short_page() {
        let server = MockServer::start().await;
        let accumulator = build_accumulator(&server, "market.testnet");
        let first_page = (0..100)
            .map(|idx| {
                (
                    format!("user{idx}.testnet").parse().unwrap(),
                    BorrowPosition::new(0),
                )
            })
            .collect::<BorrowPositions>();
        let second_page = (100..102)
            .map(|idx| {
                (
                    format!("user{idx}.testnet").parse().unwrap(),
                    BorrowPosition::new(0),
                )
            })
            .collect::<BorrowPositions>();
        let first_page = Arc::new(first_page);
        let second_page = Arc::new(second_page);
        let calls = Arc::new(AtomicUsize::new(0));

        let first_page_clone = Arc::clone(&first_page);
        let second_page_clone = Arc::clone(&second_page);
        let calls_clone = Arc::clone(&calls);
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(move |req: &Request| {
                let (params, id) = parse_query_request(req);
                assert_eq!(
                    params.get("method_name").and_then(JsonValue::as_str),
                    Some("list_borrow_positions")
                );
                let offset = decode_args(&params)
                    .get("offset")
                    .and_then(JsonValue::as_u64)
                    .expect("offset to be present");
                calls_clone.fetch_add(1, Ordering::SeqCst);
                let page = if offset == 0 {
                    Arc::clone(&first_page_clone)
                } else {
                    Arc::clone(&second_page_clone)
                };

                let payload = call_result_response(serialize_and_encode(&*page));
                ResponseTemplate::new(200).set_body_json(rpc_success_response(&json!(payload), &id))
            })
            .mount(&server)
            .await;

        let borrows = accumulator.get_borrows().await.unwrap();

        assert_eq!(borrows.len(), 102);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(borrows.contains_key(&AccountId::from_str("user0.testnet").unwrap()));
        assert!(borrows.contains_key(&AccountId::from_str("user101.testnet").unwrap()));
    }

    #[tokio::test]
    async fn run_borrow_accumulations_returns_early_on_empty_page() {
        let server = MockServer::start().await;
        let accumulator = build_accumulator(&server, "market.testnet");
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(move |req: &Request| {
                let (params, id) = parse_query_request(req);
                assert_eq!(
                    params.get("method_name").and_then(JsonValue::as_str),
                    Some("list_borrow_positions")
                );
                calls_clone.fetch_add(1, Ordering::SeqCst);

                let payload = call_result_response(serialize_and_encode(BorrowPositions::new()));
                ResponseTemplate::new(200).set_body_json(rpc_success_response(&json!(payload), &id))
            })
            .mount(&server)
            .await;

        accumulator
            .run_borrow_accumulations(/*concurrency=*/ 4)
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn accumulate_sends_correct_payload_and_succeeds() {
        let server = MockServer::start().await;
        let accumulator = build_accumulator(&server, "market.testnet");
        let borrows = BorrowPositions::from_iter(vec![
            ("alice.testnet".parse().unwrap(), BorrowPosition::new(0)),
            ("bob.testnet".parse().unwrap(), BorrowPosition::new(0)),
        ]);
        let sent_to = Arc::new(Mutex::new(Vec::new()));
        let sent_to_clone = Arc::clone(&sent_to);
        let access_key_calls = Arc::new(AtomicUsize::new(0));
        let access_key_calls_clone = Arc::clone(&access_key_calls);

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(move |req: &Request| match parse_method(req).as_str() {
                "query" => {
                    let (params, id) = parse_query_request(req);
                    if params.get("method_name").and_then(JsonValue::as_str)
                        == Some("list_borrow_positions")
                    {
                        let payload = call_result_response(serialize_and_encode(&borrows));
                        return ResponseTemplate::new(200)
                            .set_body_json(rpc_success_response(&json!(payload), &id));
                    }

                    access_key_calls_clone.fetch_add(1, Ordering::SeqCst);
                    let payload = access_key_response(10);
                    ResponseTemplate::new(200)
                        .set_body_json(rpc_success_response(&json!(payload), &id))
                }
                "send_tx" => {
                    let id = near_sdk::serde_json::from_slice::<JsonValue>(&req.body)
                        .unwrap()
                        .get("id")
                        .cloned()
                        .unwrap_or_else(|| json!("1"));
                    let signed_tx = decode_signed_tx(req);
                    let transaction = signed_tx.transaction.clone();
                    let Transaction::V0(tx) = transaction else {
                        panic!("unexpected transaction variant");
                    };
                    assert_eq!(tx.receiver_id.as_str(), "market.testnet");
                    assert_eq!(tx.actions.len(), 1);
                    let Action::FunctionCall(action) = tx.actions.into_iter().next().unwrap()
                    else {
                        panic!("expected function call action");
                    };
                    assert_eq!(action.method_name, "apply_interest");
                    let args: JsonValue =
                        near_sdk::serde_json::from_slice(&action.args).expect("decode args");
                    let account = args
                        .get("account_id")
                        .and_then(JsonValue::as_str)
                        .expect("account id in args")
                        .to_owned();
                    sent_to_clone.lock().unwrap().push(account);

                    let outcome = FinalExecutionOutcomeView {
                        status: FinalExecutionStatus::SuccessValue(Vec::new()),
                        transaction: SignedTransactionView::from(signed_tx.clone()),
                        transaction_outcome: ExecutionOutcomeWithIdView {
                            proof: Vec::new(),
                            block_hash: CryptoHash::default(),
                            id: CryptoHash::default(),
                            outcome: ExecutionOutcomeView {
                                logs: Vec::new(),
                                receipt_ids: Vec::new(),
                                gas_burnt: 0,
                                tokens_burnt: 0,
                                executor_id: "market.testnet".parse().unwrap(),
                                status: ExecutionStatusView::SuccessValue(Vec::new()),
                                metadata: ExecutionMetadataView::default(),
                            },
                        },
                        receipts_outcome: Vec::new(),
                    };
                    let payload = RpcTransactionResponse {
                        final_execution_outcome: Some(
                            FinalExecutionOutcomeViewEnum::FinalExecutionOutcome(outcome),
                        ),
                        final_execution_status: TxExecutionStatus::Final,
                    };

                    ResponseTemplate::new(200)
                        .set_body_json(rpc_success_response(&json!(payload), &id))
                }
                method => panic!("Unexpected method {method}"),
            })
            .mount(&server)
            .await;

        accumulator
            .run_borrow_accumulations(/*concurrency=*/ 2)
            .await
            .unwrap();

        let mut sent = sent_to.lock().unwrap().clone();
        sent.sort();
        assert_eq!(
            sent,
            vec!["alice.testnet".to_string(), "bob.testnet".to_string()]
        );
        assert_eq!(access_key_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn run_static_accumulations_skip_for_v1_0_0_market() {
        let server = MockServer::start().await;
        let accumulator = build_accumulator(&server, "market.testnet");
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(move |req: &Request| match parse_method(req).as_str() {
                "query" => {
                    let (params, id) = parse_query_request(req);
                    assert_eq!(
                        params.get("method_name").and_then(JsonValue::as_str),
                        Some("contract_source_metadata")
                    );
                    calls_clone.fetch_add(1, Ordering::SeqCst);

                    let metadata = ContractSourceMetadata {
                        version: "1.0.0".to_string(),
                        link: None,
                        standards: None,
                    };
                    let payload = call_result_response(serialize_and_encode(&metadata));
                    ResponseTemplate::new(200)
                        .set_body_json(rpc_success_response(&json!(payload), &id))
                }
                other => panic!("Unexpected rpc method {other}"),
            })
            .mount(&server)
            .await;

        accumulator
            .run_static_accumulations(/*concurrency=*/ 2)
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn get_borrows_errors_on_wrong_response_kind() {
        let server = MockServer::start().await;
        let accumulator = build_accumulator(&server, "market.testnet");
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(move |req: &Request| {
                let (params, id) = parse_query_request(req);
                assert_eq!(
                    params.get("method_name").and_then(JsonValue::as_str),
                    Some("list_borrow_positions")
                );
                calls_clone.fetch_add(1, Ordering::SeqCst);
                let payload = access_key_response(1);
                ResponseTemplate::new(200).set_body_json(rpc_success_response(&json!(payload), &id))
            })
            .mount(&server)
            .await;

        let result = accumulator.get_borrows().await;
        assert!(result.is_err(), "Expected error on RPC wrong response kind");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn get_static_accounts_returns_configured_accounts() {
        let server = MockServer::start().await;
        let accumulator = build_accumulator(&server, "market.testnet");
        let configuration = sample_configuration();
        let configuration_for_mock = configuration.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(move |req: &Request| {
                let (params, id) = parse_query_request(req);
                assert_eq!(
                    params.get("method_name").and_then(JsonValue::as_str),
                    Some("get_configuration")
                );
                calls_clone.fetch_add(1, Ordering::SeqCst);

                let payload = call_result_response(serialize_and_encode(&configuration_for_mock));
                ResponseTemplate::new(200).set_body_json(rpc_success_response(&json!(payload), &id))
            })
            .mount(&server)
            .await;

        let static_accounts = accumulator.get_static_accounts().await.unwrap();
        let expected: HashSet<_> = configuration
            .yield_weights
            .r#static
            .keys()
            .cloned()
            .collect();
        let actual: HashSet<_> = static_accounts.into_iter().collect();

        assert_eq!(actual, expected);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
