#![allow(
    clippy::unwrap_used,
    reason = "Invalid local RPC fixtures and unexpected resolver errors must fail these tests immediately"
)]

use near_api::{NetworkConfig, RPCEndpoint};
use templar_common::{
    oracle::pyth::{Price, PriceIdentifier, PythTimestamp},
    Decimal,
};
use templar_gateway_core::{DispatchRead, GatewayContext};
use templar_gateway_methods_dispatch::Dispatch;
use templar_gateway_methods_spec::oracle::{
    PythOraclePrices, ResolvePrice, ResolvePriceAllSources, ResolvePrices, ResolvedPrice,
};
use templar_proxy_oracle_kernel::proxy::{FreshnessFilter, Proxy};
use templar_proxy_oracle_near_common::{
    input::{ProxyPriceTransformer, Source},
    price_transformer::{Call, PriceTransformer},
    request::OracleRequest,
};
use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

const PRICE_ID: PriceIdentifier = PriceIdentifier([1; 32]);
const FALLBACK_ID: PriceIdentifier = PriceIdentifier([2; 32]);
const ZERO_HASH: &str = "11111111111111111111111111111111";

#[derive(Clone)]
enum FixtureOracle {
    Proxy(Proxy<Source>),
    Direct,
    Lst(Option<PriceTransformer>),
}

// Mock only the external contract-view boundary. Classification, request lookup,
// transformations, conversion, aggregation, and dispatch all run production code.
async fn rpc_context(
    oracle: FixtureOracle,
    transformer_input: Decimal,
) -> (GatewayContext, MockServer) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &wiremock::Request| {
            let rpc: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(rpc["method"], "query");
            assert_eq!(rpc["params"]["request_type"], "call_function");
            let view = rpc["params"]["method_name"].as_str().unwrap();
            let value = match (&oracle, view) {
                (FixtureOracle::Proxy(_), "list_proxies")
                | (FixtureOracle::Lst(_), "list_transformers") => Some(serde_json::json!([])),
                (FixtureOracle::Proxy(proxy), "get_proxy") => {
                    Some(serde_json::to_value(proxy).unwrap())
                }
                (FixtureOracle::Proxy(_), "contract_source_metadata") => {
                    Some(serde_json::json!({"version": "0.2.0", "standards": []}))
                }
                (FixtureOracle::Direct, "list_ema_prices_unsafe") => Some(serde_json::json!({})),
                (FixtureOracle::Lst(_), "oracle_id") => Some(serde_json::json!("source.near")),
                (FixtureOracle::Lst(transformer), "get_transformer") => {
                    Some(serde_json::to_value(transformer).unwrap())
                }
                (_, "get_rate") => Some(serde_json::to_value(transformer_input).unwrap()),
                _ => None,
            };
            let response = value.map_or_else(
                || {
                    serde_json::json!({"error": {
                        "code": -32000,
                        "message": "Server error",
                        "data": "MethodResolveError(MethodNotFound)",
                        "name": "HANDLER_ERROR",
                        "cause": {
                            "name": "CONTRACT_EXECUTION_ERROR",
                            "info": {"error_message": "MethodResolveError(MethodNotFound)"}
                        }
                    }})
                },
                |value| {
                    serde_json::json!({"result": {
                        "block_hash": ZERO_HASH,
                        "block_height": 1,
                        "logs": [],
                        "result": serde_json::to_vec(&value).unwrap()
                    }})
                },
            );
            let mut response = response;
            response["jsonrpc"] = serde_json::json!("2.0");
            response["id"] = rpc["id"].clone();
            ResponseTemplate::new(200).set_body_json(response)
        })
        .mount(&server)
        .await;
    let mut network = NetworkConfig::from_rpc_url("test", server.uri().parse().unwrap());
    network.rpc_endpoints[0] = RPCEndpoint::new(server.uri().parse().unwrap()).with_retries(1);
    (GatewayContext::new(network).unwrap(), server)
}

fn price(mantissa: i64) -> Price {
    Price {
        price: mantissa.into(),
        conf: 3.into(),
        expo: -8,
        publish_time: PythTimestamp::from_secs(1_000),
    }
}

fn request(primary: Option<Price>) -> ResolvePrice {
    ResolvePrice {
        oracle_id: "oracle.near".parse().unwrap(),
        price_id: PRICE_ID,
        // Fixed old prices stay fresh without a wall-clock-sensitive test boundary.
        age: 10_000_000_000,
        pyth: vec![PythOraclePrices {
            oracle_id: "source.near".parse().unwrap(),
            response: [(PRICE_ID, primary), (FALLBACK_ID, Some(price(777)))]
                .into_iter()
                .collect(),
        }],
        redstone: vec![],
        lazer: vec![],
    }
}

fn transformer_call() -> Call {
    let account_id: near_account_id::AccountId = "rate.near".parse().unwrap();
    Call::new_simple(account_id.as_ref(), "get_rate")
}

fn transformed_source(decimals: u32) -> Source {
    Source::Transformer(ProxyPriceTransformer::lst(
        OracleRequest::pyth("source.near".parse().unwrap(), PRICE_ID),
        decimals,
        transformer_call(),
    ))
}

fn priority(decimals: u32, transformed_first: bool) -> FixtureOracle {
    let transformed = transformed_source(decimals);
    let fallback = Source::Request(OracleRequest::pyth(
        "source.near".parse().unwrap(),
        FALLBACK_ID,
    ));
    let sources = if transformed_first {
        [transformed, fallback]
    } else {
        [fallback, transformed]
    };
    FixtureOracle::Proxy(Proxy::priority(sources, FreshnessFilter::empty()))
}

async fn strict(ctx: GatewayContext, request: ResolvePrice) -> Option<Price> {
    <Dispatch as DispatchRead<ResolvePriceAllSources, GatewayContext>>::dispatch(
        ResolvePriceAllSources { request },
        ctx,
    )
    .await
    .unwrap()
}

#[rstest::rstest]
#[case::failed_transform(39, Decimal::from(2u32), Some(price(123)), true, (777, 3))]
#[case::failed_unused_transform(39, Decimal::from(2u32), Some(price(123)), false, (777, 3))]
#[case::zero_transform(0, Decimal::ZERO, Some(price(123)), true, (0, 0))]
#[case::negative_transform(0, Decimal::from(2u32), Some(price(-123)), true, (-246, 6))]
#[case::missing_source(0, Decimal::from(2u32), None, true, (777, 3))]
#[tokio::test]
async fn all_sources_rejects_hidden_partial_resolution(
    #[case] decimals: u32,
    #[case] input: Decimal,
    #[case] primary: Option<Price>,
    #[case] transformed_first: bool,
    #[case] canonical_expected: (i64, u64),
) {
    let (ctx, _server) = rpc_context(priority(decimals, transformed_first), input).await;
    let request = request(primary);
    assert_eq!(strict(ctx.clone(), request.clone()).await, None);
    let canonical = <Dispatch as DispatchRead<ResolvePrice, GatewayContext>>::dispatch(
        request.clone(),
        ctx.clone(),
    )
    .await
    .unwrap();
    let mut expected = price(canonical_expected.0);
    expected.conf = canonical_expected.1.into();
    let expected = Some(expected);
    assert_eq!(canonical, expected);
    let canonical_batch = <Dispatch as DispatchRead<ResolvePrices, GatewayContext>>::dispatch(
        ResolvePrices {
            oracle_id: request.oracle_id,
            price_ids: vec![request.price_id],
            age: request.age,
            pyth: request.pyth,
            redstone: request.redstone,
            lazer: request.lazer,
        },
        ctx,
    )
    .await
    .unwrap();
    assert_eq!(
        canonical_batch,
        vec![ResolvedPrice {
            price_id: PRICE_ID,
            price: expected,
        }]
    );
}

#[tokio::test]
async fn all_sources_healthy_uses_canonical_transformation_and_aggregation() {
    let (ctx, _server) = rpc_context(priority(0, true), Decimal::from(2u32)).await;
    let request = request(Some(price(123)));
    let mut expected = price(246);
    expected.conf = 6.into();
    let strict_price = strict(ctx.clone(), request.clone()).await;
    assert_eq!(strict_price, Some(expected));
    assert_eq!(
        strict_price,
        <Dispatch as DispatchRead<ResolvePrice, GatewayContext>>::dispatch(request, ctx)
            .await
            .unwrap()
    );
}

#[rstest::rstest]
#[case::direct_zero(FixtureOracle::Direct, Decimal::from(1u32), 0, None)]
#[case::direct_negative(FixtureOracle::Direct, Decimal::from(1u32), -1, None)]
#[case::direct_positive(FixtureOracle::Direct, Decimal::from(1u32), 123, Some(123))]
#[case::lst_passthrough_zero(FixtureOracle::Lst(None), Decimal::from(1u32), 0, None)]
#[case::lst_passthrough_positive(FixtureOracle::Lst(None), Decimal::from(1u32), 123, Some(123))]
#[case::lst_transformed_zero(
    FixtureOracle::Lst(Some(PriceTransformer::lst(PRICE_ID, 0, transformer_call()))),
    Decimal::ZERO,
    123,
    None
)]
#[case::lst_transformed_negative(FixtureOracle::Lst(Some(PriceTransformer::lst(PRICE_ID, 0, transformer_call()))), Decimal::from(2u32), -123, None)]
#[case::lst_transformed_positive(
    FixtureOracle::Lst(Some(PriceTransformer::lst(PRICE_ID, 0, transformer_call()))),
    Decimal::from(2u32),
    123,
    Some(246)
)]
#[tokio::test]
async fn all_sources_requires_positive_direct_and_lst_results(
    #[case] oracle: FixtureOracle,
    #[case] input: Decimal,
    #[case] mantissa: i64,
    #[case] expected_mantissa: Option<i64>,
) {
    let mut request = request(Some(price(mantissa)));
    if matches!(&oracle, FixtureOracle::Direct) {
        request.pyth[0].oracle_id = request.oracle_id.clone();
    }
    let (ctx, _server) = rpc_context(oracle, input).await;
    let result = strict(ctx, request).await;
    assert_eq!(result.map(|price| price.price.0), expected_mantissa);
}

#[tokio::test]
async fn all_sources_preserves_source_age_rejection() {
    let (ctx, _server) = rpc_context(priority(0, true), Decimal::from(2u32)).await;
    let mut request = request(Some(price(123)));
    request.age = 0;
    assert_eq!(strict(ctx, request).await, None);
}
