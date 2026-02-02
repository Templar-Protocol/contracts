use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};

use clap::Parser;
use near_crypto::InMemorySigner;
use near_jsonrpc_client::JsonRpcClient;
use templar_accumulator::{rpc::list_all_deployments, Accumulator, Args};
use tracing::{error, info};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    info!("Starting accumulator bot with args: {args}");
    run_service(args, std::future::pending()).await
}

async fn run_service_with_client(
    args: Args,
    client: JsonRpcClient,
    signer: Arc<near_crypto::Signer>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let registries = args.registries.clone();
    let timeout = args.timeout;
    let concurrency = args.concurrency;

    let mut refresh_ticker =
        tokio::time::interval(Duration::from_secs(args.registry_refresh_interval));
    let mut accumulate_ticker = tokio::time::interval(Duration::from_secs(args.interval));
    let mut static_accumulate_ticker =
        tokio::time::interval(Duration::from_secs(args.static_interval));
    let mut accumulators = list_all_deployments(client.clone(), registries.clone(), concurrency)
        .await?
        .into_iter()
        .map(|market| {
            (
                market.clone(),
                Accumulator::new(client.clone(), signer.clone(), market, timeout),
            )
        })
        .collect::<HashMap<_, _>>();

    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            () = &mut shutdown => {
                info!("Shutdown signal received, stopping accumulator bot");
                break;
            }
            _ = refresh_ticker.tick() => {
                info!("Refreshing registry deployments");
                let Ok(all_markets) =
                    list_all_deployments(client.clone(), registries.clone(), concurrency)
                        .await else {
                    error!("Failed to list deployments, keeping existing ones");
                    continue;
                };
                info!("Found {} deployments", all_markets.len());
                for market in all_markets {
                    accumulators.entry(market.clone()).or_insert_with(|| {
                        Accumulator::new(client.clone(), signer.clone(), market, timeout)
                    });
                }
            }
            _ = accumulate_ticker.tick() => {
                for (market, accumulator) in &accumulators {
                    info!("Running accumulation for market: {market}");
                    accumulator.run_borrow_accumulations(concurrency).await?;
                }

                info!("Accumulation job done");
            }
            _ = static_accumulate_ticker.tick() => {
                for (market, accumulator) in &accumulators {
                    info!("Running static accumulation for market: {market}");
                    accumulator.run_static_accumulations(concurrency).await?;
                }

                info!("Static accumulation job done");
            }
        }
    }

    Ok(())
}

async fn run_service(
    args: Args,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let rpc_url = args
        .rpc_url
        .as_deref()
        .unwrap_or_else(|| args.network.rpc_url());
    let client = JsonRpcClient::connect(rpc_url);
    let signer = Arc::new(InMemorySigner::from_secret_key(
        args.signer_account.clone(),
        args.signer_key.clone(),
    ));

    run_service_with_client(args, client, signer, shutdown).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_crypto::{InMemorySigner, KeyType, SecretKey};
    use near_jsonrpc_primitives::types::query::{QueryResponseKind, RpcQueryResponse};
    use near_primitives::hash::CryptoHash;
    use near_sdk::AccountId;
    use std::collections::HashMap;
    use std::env;
    use std::str::FromStr;
    use tokio::time::{self, Duration};
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, Request, ResponseTemplate,
    };

    use near_sdk::serde_json::{json, Value as JsonValue};
    use templar_accumulator::rpc::Network;

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

    fn call_result_response(result_bytes: Vec<u8>) -> RpcQueryResponse {
        RpcQueryResponse {
            kind: QueryResponseKind::CallResult(near_primitives::views::CallResult {
                result: result_bytes,
                logs: Vec::new(),
            }),
            block_height: 1,
            block_hash: CryptoHash::default(),
        }
    }

    fn sample_configuration() -> templar_common::market::MarketConfiguration {
        use templar_common::{
            asset::FungibleAsset,
            dec,
            fee::{Fee, TimeBasedFee},
            interest_rate_strategy::InterestRateStrategy,
            market::{PriceOracleConfiguration, YieldWeights},
            number::Decimal,
            oracle::pyth::PriceIdentifier,
            time_chunk::TimeChunkConfiguration,
        };

        templar_common::market::MarketConfiguration {
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
            yield_weights: YieldWeights::new_with_supply_weight(100),
            protocol_account_id: "protocol.testnet".parse().unwrap(),
            liquidation_maximum_spread: Decimal::from_str("0.05").unwrap(),
        }
    }

    #[tokio::test]
    async fn service_loop_triggers_a_branche() {
        time::pause();

        let server = MockServer::start().await;
        let deployment_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let borrow_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let static_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let deployment_calls_clone = Arc::clone(&deployment_calls);
        let borrow_calls_clone = Arc::clone(&borrow_calls);
        let static_calls_clone = Arc::clone(&static_calls);
        let configuration = sample_configuration();

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(move |req: &Request| match parse_method(req).as_str() {
                "query" => {
                    let (params, id) = parse_query_request(req);
                    let method_name = params
                        .get("method_name")
                        .and_then(JsonValue::as_str)
                        .expect("method name");
                    match method_name {
                        "list_deployments" => {
                            deployment_calls_clone
                                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            let payload = call_result_response(
                                near_sdk::serde_json::to_vec(&vec!["market.testnet"]).unwrap(),
                            );
                            ResponseTemplate::new(200)
                                .set_body_json(rpc_success_response(&json!(payload), &id))
                        }
                        "list_borrow_positions" => {
                            borrow_calls_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            let empty: templar_accumulator::rpc::BorrowPositions = HashMap::new();
                            let payload =
                                call_result_response(near_sdk::serde_json::to_vec(&empty).unwrap());
                            ResponseTemplate::new(200)
                                .set_body_json(rpc_success_response(&json!(payload), &id))
                        }
                        "get_configuration" => {
                            static_calls_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            let payload = call_result_response(
                                templar_accumulator::rpc::serialize_and_encode(&configuration),
                            );
                            ResponseTemplate::new(200)
                                .set_body_json(rpc_success_response(&json!(payload), &id))
                        }
                        other => panic!("Unexpected method name {other}"),
                    }
                }
                other => panic!("Unexpected RPC method {other}"),
            })
            .mount(&server)
            .await;

        let args = Args {
            registries: vec!["registry.testnet".parse().unwrap()],
            signer_key: SecretKey::from_random(KeyType::ED25519),
            signer_account: "signer.testnet".parse().unwrap(),
            network: Network::Testnet,
            rpc_url: None,
            timeout: 5,
            interval: 1,
            static_interval: 2,
            registry_refresh_interval: 3,
            concurrency: 2,
        };
        let client = JsonRpcClient::connect(server.uri());
        let signer = Arc::new(InMemorySigner::from_secret_key(
            args.signer_account.clone(),
            args.signer_key.clone(),
        ));

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(run_service_with_client(args, client, signer, async move {
            let _ = shutdown_rx.await;
        }));

        time::advance(Duration::from_secs(7)).await;
        let _ = shutdown_tx.send(());
        time::advance(Duration::from_secs(1)).await;

        handle.await.unwrap().unwrap();

        assert!(
            deployment_calls.load(std::sync::atomic::Ordering::SeqCst)
                + borrow_calls.load(std::sync::atomic::Ordering::SeqCst)
                + static_calls.load(std::sync::atomic::Ordering::SeqCst)
                >= 1
        );
    }

    #[test]
    fn registries_env_is_space_delimited() {
        let sk = SecretKey::from_random(KeyType::ED25519);
        let original_regs = env::var("REGISTRIES_ACCOUNT_IDS").ok();
        let original_signer = env::var("SIGNER_ACCOUNT_ID").ok();
        let original_key = env::var("SIGNER_KEY").ok();

        env::set_var("REGISTRIES_ACCOUNT_IDS", "one.testnet two.testnet");
        env::set_var("SIGNER_ACCOUNT_ID", "signer.testnet");
        env::set_var("SIGNER_KEY", sk.to_string());

        let args = Args::parse_from(["accumulator"]);
        let expected: Vec<AccountId> = vec![
            "one.testnet".parse().unwrap(),
            "two.testnet".parse().unwrap(),
        ];

        assert_eq!(args.registries, expected);

        if let Some(val) = original_regs {
            env::set_var("REGISTRIES_ACCOUNT_IDS", val);
        } else {
            env::remove_var("REGISTRIES_ACCOUNT_IDS");
        }
        if let Some(val) = original_signer {
            env::set_var("SIGNER_ACCOUNT_ID", val);
        } else {
            env::remove_var("SIGNER_ACCOUNT_ID");
        }
        if let Some(val) = original_key {
            env::set_var("SIGNER_KEY", val);
        } else {
            env::remove_var("SIGNER_KEY");
        }
    }
}
