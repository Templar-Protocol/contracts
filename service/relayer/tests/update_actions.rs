use std::{path::Path, time::Duration};

use near_primitives::action::Action;
use near_sdk::NearToken;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_relayer::{
    app::args,
    client::oracle::{PythSpec, RedStoneSpec, Spec},
};
use tokio::sync::watch;

#[tokio::test]
async fn requires_network_pyth() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();

    let pyth_args = args::PythConfig {
        hermes_url: "https://hermes-beta.pyth.network".to_string(),
        refresh: Duration::from_secs(25),
        update_gas: near_sdk::Gas::from_tgas(300),
        update_deposit: NearToken::from_near(1).saturating_div(100),
        timeout: Duration::from_secs(10),
    };

    let handle = PythSpec::new(pyth_args.clone());

    let price_id = PriceIdentifier(
        hex::decode("f9c0172ba10dfa4d19088d94f5bf61d3b54d5bd7483a322a982e1373ee8ea31b")
            .unwrap()
            .try_into()
            .unwrap(),
    );

    let actions = match handle.update_actions(&[price_id]).await {
        Ok(actions) => actions,
        Err(error)
            if error.is_timeout()
                || error
                    .status()
                    .is_some_and(|status| status.is_server_error()) =>
        {
            eprintln!("Skipping transient Pyth network failure: {error}");
            return;
        }
        Err(error) => panic!("Pyth update_actions failed: {error}"),
    };
    assert_eq!(actions.len(), 1);
    let Action::FunctionCall(fc) = &actions[0] else {
        panic!("Unexpected action type: {:?}", &actions[0]);
    };
    assert_eq!(fc.method_name, "update_price_feeds");
    assert!(!fc.args.is_empty());

    eprintln!("{actions:?}");
}

#[tokio::test]
async fn requires_network_redstone() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();

    let redstone_args = args::RedStoneConfig {
        refresh: Duration::from_secs(25),
        update_gas: near_sdk::Gas::from_tgas(300),
        update_deposit: NearToken::from_near(0),
        node_path: Path::new("node").to_owned(),
    };

    let kill = watch::Sender::default();

    let spec =
        RedStoneSpec::new(redstone_args, kill.clone()).expect("Failed to create RedStoneSpec");

    let actions = spec
        .update_actions(&["ETH".into(), "BTC".into()])
        .await
        .unwrap();
    kill.send(()).unwrap();

    eprintln!("{actions:?}");
    assert_eq!(actions.len(), 1);
    let Action::FunctionCall(fc) = &actions[0] else {
        panic!("Unexpected action type: {:?}", &actions[0]);
    };
    assert_eq!(fc.method_name, "write_prices");
    assert!(!fc.args.is_empty());
}
