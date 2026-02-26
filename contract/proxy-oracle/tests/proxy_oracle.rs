use std::collections::HashMap;

use near_sdk::json_types::{I64, U64};
use near_workspaces::{network::Sandbox, Worker};
use templar_common::{
    oracle::{
        proxy::{Oracle, Proxy},
        pyth::{self, PriceIdentifier},
        redstone::feed_data::FeedData,
        OraclePriceId,
    },
    primitive_types,
};
use test_utils::{
    accounts,
    controller::proxy_oracle::ProxyOracleController,
    pyth_price_id::{self, stable::CRYPTO_BTC_USD},
    worker, ContractController, MockOracleController,
};

#[rstest::rstest]
#[tokio::test]
pub async fn proxy_oracle(#[future(awt)] worker: Worker<Sandbox>) {
    accounts!(worker, actor, redstone_adapter, proxy_oracle, pyth_oracle);
    let pyth_oracle = MockOracleController::deploy(pyth_oracle).await;
    let redstone_adapter = MockOracleController::deploy(redstone_adapter).await;
    let proxy_oracle =
        ProxyOracleController::deploy(proxy_oracle, pyth_oracle.id(), redstone_adapter.id()).await;

    let pyth_id = proxy_oracle.oracle_id(Oracle::Pyth).await;
    assert_eq!(&pyth_id, pyth_oracle.id());
    let redstone_id = proxy_oracle.oracle_id(Oracle::RedStone).await;
    assert_eq!(&redstone_id, redstone_adapter.id());

    let list_proxies = proxy_oracle.list_proxies(None, None).await;
    assert_eq!(list_proxies, vec![]);

    macro_rules! set {
        (pyth . $id: ident = $val: literal) => {
            set!(
                pyth.$id = Some(pyth::Price {
                    price: I64($val),
                    conf: U64(0),
                    expo: 0,
                    publish_time: 0,
                })
            )
        };
        (pyth . $id: ident = $val: expr) => {
            pyth_oracle.set_pyth_price(&actor, pyth_price_id::stable::$id, $val)
        };
        (redstone . $id: ident = $val: literal) => {
            set!(
                redstone.$id = Some(FeedData {
                    price: primitive_types::U256::from($val * 100_000_000).into(),
                    package_timestamp: U64(0),
                    write_timestamp: U64(0),
                })
            )
        };
        (redstone . $id: ident = $val: expr) => {
            redstone_adapter.set_redstone_price(&actor, stringify!($id), $val)
        };
    }

    let btc_proxy_id = PriceIdentifier(hex_literal::hex!(
        "b7c0000000000000000000000000000000000000000000000000000000000000"
    ));

    let btc_proxy_def = Proxy::list([
        OraclePriceId::Pyth(CRYPTO_BTC_USD),
        OraclePriceId::RedStone("BTC".to_string()),
    ]);

    proxy_oracle
        .add_proxy(proxy_oracle.account(), btc_proxy_id, btc_proxy_def.clone())
        .await;

    assert_eq!(
        proxy_oracle.list_proxies(None, None).await,
        vec![btc_proxy_id],
    );
    assert_eq!(
        proxy_oracle.get_proxy(btc_proxy_id).await.unwrap(),
        btc_proxy_def,
    );

    let result = proxy_oracle
        .list_ema_prices_no_older_than(&actor, vec![btc_proxy_id, CRYPTO_BTC_USD], 60_u32)
        .await;
    assert_eq!(
        result,
        HashMap::from_iter([(btc_proxy_id, None), (CRYPTO_BTC_USD, None)])
    );

    set!(redstone.BTC = 100_000).await;
    let result = proxy_oracle
        .list_ema_prices_no_older_than(&actor, vec![btc_proxy_id], 60_u32)
        .await;
    assert_eq!(
        result,
        HashMap::from_iter([
            (
                btc_proxy_id,
                Some(pyth::Price {
                    price: I64(100_000),
                    conf: U64(0),
                    expo: 0,
                    publish_time: 0,
                }),
            ),
            (CRYPTO_BTC_USD, None)
        ])
    );

    // Pyth appears first on the list
    set!(pyth.CRYPTO_BTC_USD = 90_000).await;
    let result = proxy_oracle
        .list_ema_prices_no_older_than(&actor, vec![btc_proxy_id], 60_u32)
        .await;
    assert_eq!(
        result,
        HashMap::from_iter([
            (
                btc_proxy_id,
                Some(pyth::Price {
                    price: I64(90_000),
                    conf: U64(0),
                    expo: 0,
                    publish_time: 0,
                }),
            ),
            (
                CRYPTO_BTC_USD,
                Some(pyth::Price {
                    price: I64(90_000),
                    conf: U64(0),
                    expo: 0,
                    publish_time: 0,
                }),
            ),
        ]),
    );

    // set!(redstone.ETH = 1_000).await;
}
