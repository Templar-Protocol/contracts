use std::{path::Path, time::Duration};

use near_crypto::InMemorySigner;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::views::TxExecutionStatus;
use near_sdk::NearToken;
use near_workspaces::{network::Sandbox, Worker};
use templar_common::oracle::{
    pyth::PriceIdentifier,
    redstone::{self, FeedId},
};
use templar_proxy_oracle_kernel::proxy::{FreshnessFilter, Proxy};
use templar_proxy_oracle_near_common::request::OracleRequest;
use templar_relayer::{
    app::args,
    cache::Cache,
    client::{
        near::Near,
        oracle::{RedStoneSpec, Spec},
    },
};
use test_utils::*;
use tokio::sync::watch;

const ETH_PRICE_ID: PriceIdentifier = PriceIdentifier([0xe7_u8; 32]);
const BTC_PRICE_ID: PriceIdentifier = PriceIdentifier([0xb7_u8; 32]);

#[rstest::rstest]
#[tokio::test]
async fn redstone(#[future(awt)] worker: Worker<Sandbox>) {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();

    accounts!(worker, redstone_oracle, proxy_oracle);

    let jsonrpc_client = JsonRpcClient::connect(worker.rpc_addr());

    let oracle_signer = InMemorySigner::from_secret_key(
        redstone_oracle.id().clone(),
        redstone_oracle.secret_key().to_string().parse().unwrap(),
    );

    let near = Near::new(
        jsonrpc_client,
        redstone_oracle.id().clone(),
        vec![oracle_signer],
    );

    let kill = watch::Sender::default();

    let cache = Cache::new(
        near.clone(),
        args::Cache {
            gas_price_refresh: Duration::from_secs(10),
            nonce_refresh: Duration::from_secs(10),
        },
        kill.clone(),
    );

    let redstone_oracle =
        RedStoneAdapterController::deploy(redstone_oracle, redstone::config::prod()).await;

    let redstone_eth_id = FeedId::from("ETH");
    let redstone_btc_id = FeedId::from("BTC");

    let proxy_oracle = ProxyOracleController::deploy(proxy_oracle).await;
    proxy_oracle
        .admin_set_proxy(
            proxy_oracle.account(),
            ETH_PRICE_ID,
            Some(Proxy::median_low(
                [
                    OracleRequest::redstone(redstone_oracle.id().clone(), redstone_eth_id.clone())
                        .into(),
                ],
                FreshnessFilter::empty(),
            )),
        )
        .await;
    proxy_oracle
        .admin_set_proxy(
            proxy_oracle.account(),
            BTC_PRICE_ID,
            Some(Proxy::median_low(
                [
                    OracleRequest::redstone(redstone_oracle.id().clone(), redstone_btc_id.clone())
                        .into(),
                ],
                FreshnessFilter::empty(),
            )),
        )
        .await;

    let redstone_args = args::RedStoneConfig {
        refresh: Duration::from_secs(25),
        update_gas: near_sdk::Gas::from_tgas(300),
        update_deposit: NearToken::from_near(0),
        node_path: Path::new("node").to_owned(),
    };

    let spec =
        RedStoneSpec::new(redstone_args, kill.clone()).expect("Failed to create RedStoneSpec");

    let price_data_before = redstone_oracle
        .read_price_data(vec![redstone_eth_id.clone(), redstone_btc_id.clone()])
        .await;

    assert_eq!(price_data_before.get(&redstone_eth_id), None);
    assert_eq!(price_data_before.get(&redstone_btc_id), None);

    let actions = spec
        .update_actions(&[redstone_eth_id.clone(), redstone_btc_id.clone()])
        .await
        .unwrap();

    let signed_transaction = near
        .sign_transaction(&cache, redstone_oracle.id().clone(), actions)
        .await;
    near.send_transaction(signed_transaction, TxExecutionStatus::Final)
        .await
        .unwrap();

    let price_data_after = redstone_oracle
        .read_price_data(vec![redstone_eth_id.clone(), redstone_btc_id.clone()])
        .await;

    println!("{price_data_after:#?}");

    assert_ne!(price_data_after.get(&redstone_eth_id), None);
    assert_ne!(price_data_after.get(&redstone_btc_id), None);

    proxy_oracle
        .update_prices(proxy_oracle.account(), vec![ETH_PRICE_ID, BTC_PRICE_ID])
        .await;

    let r = proxy_oracle
        .list_ema_prices_no_older_than_exec(
            proxy_oracle.account(),
            vec![ETH_PRICE_ID, BTC_PRICE_ID],
            60u32,
        )
        .await;

    print_execution(&r);

    let proxy_prices = proxy_oracle
        .list_ema_prices_no_older_than(
            proxy_oracle.account(),
            vec![ETH_PRICE_ID, BTC_PRICE_ID],
            60u32,
        )
        .await;

    println!("{proxy_prices:#?}");

    assert!(proxy_prices.get(&ETH_PRICE_ID).is_some_and(|p| p.is_some()));
    assert!(proxy_prices.get(&BTC_PRICE_ID).is_some_and(|p| p.is_some()));

    kill.send(()).unwrap();
}
