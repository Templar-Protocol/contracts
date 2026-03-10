use std::{path::Path, time::Duration};

use near_crypto::InMemorySigner;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::views::TxExecutionStatus;
use near_sdk::NearToken;
use near_workspaces::{network::Sandbox, Worker};
use templar_common::oracle::redstone::{self, FeedId};
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

#[rstest::rstest]
#[tokio::test]
async fn redstone(#[future(awt)] worker: Worker<Sandbox>) {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();

    accounts!(worker, redstone_oracle);

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

    let redstone_args = args::RedStoneConfig {
        refresh: Duration::from_secs(25),
        update_gas: near_sdk::Gas::from_tgas(300),
        update_deposit: NearToken::from_near(0),
        node_path: Path::new("node").to_owned(),
        bridge_path: "./redstone-bridge/dist/index.js".parse().unwrap(),
    };

    let spec = RedStoneSpec::new(redstone_args, kill.clone());

    let eth = FeedId::from("ETH");
    let btc = FeedId::from("BTC");

    let price_data_before = redstone_oracle
        .read_price_data(vec![eth.clone(), btc.clone()])
        .await;

    assert_eq!(price_data_before.get(&eth), None);
    assert_eq!(price_data_before.get(&btc), None);

    let actions = spec
        .update_actions(&[eth.clone(), btc.clone()])
        .await
        .unwrap();

    let signed_transaction = near
        .sign_transaction(&cache, redstone_oracle.id().clone(), actions)
        .await;
    near.send_transaction(signed_transaction, TxExecutionStatus::Final)
        .await
        .unwrap();

    let price_data_after = redstone_oracle
        .read_price_data(vec![eth.clone(), btc.clone()])
        .await;

    println!("{price_data_after:#?}");

    assert_ne!(price_data_after.get(&eth), None);
    assert_ne!(price_data_after.get(&btc), None);

    kill.send(()).unwrap();
}
