use std::collections::HashMap;

use near_sdk::json_types::{I64, U64};
use near_workspaces::{network::Sandbox, Worker};
use templar_common::{
    oracle::{
        proxy::{Oracle, Proxy, ProxyEntry},
        pyth,
        redstone::FeedData,
    },
    primitive_types,
};
use test_utils::{
    accounts,
    controller::proxy_oracle::ProxyOracleController,
    pyth_price_id::{self, stable::CRYPTO_BTC_USD},
    worker, ContractController, MockOracleController,
};

fn norm_price(price: &pyth::Price) -> u64 {
    u64::try_from(price.price.0).ok().map_or(0, |p| {
        let f = 10u64.pow(price.expo.unsigned_abs());
        if price.expo.is_negative() {
            p / f
        } else {
            p * f
        }
    })
}

#[allow(clippy::cast_possible_truncation)]
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
                    publish_time: std::time::UNIX_EPOCH.elapsed().unwrap().as_millis() as i64,
                })
            )
        };
        (pyth . $id: ident = $val: expr) => {
            pyth_oracle.set_pyth_price(&actor, pyth_price_id::stable::$id, $val)
        };
        (redstone . $id: ident = $val: literal) => {
            set!(
                redstone.$id = Some(FeedData {
                    price: primitive_types::U256::from($val * 100_000_000_u128).into(),
                    package_timestamp: U64(
                        std::time::UNIX_EPOCH.elapsed().unwrap().as_millis() as u64
                    ),
                    write_timestamp: U64(
                        std::time::UNIX_EPOCH.elapsed().unwrap().as_millis() as u64
                    ),
                })
            )
        };
        (redstone . $id: ident = $val: expr) => {
            redstone_adapter.set_redstone_price(&actor, stringify!($id), $val)
        };
    }

    let btc_proxy_def = Proxy(vec![
        ProxyEntry::Pyth(CRYPTO_BTC_USD),
        ProxyEntry::RedStone("BTC".into()),
    ]);

    let btc_proxy_id = btc_proxy_def.id().unwrap();

    let result = proxy_oracle
        .add_proxy(proxy_oracle.account(), btc_proxy_def.clone())
        .await;

    assert_eq!(result, btc_proxy_id, "should return correct ID");

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
        .list_ema_prices_no_older_than(&actor, vec![btc_proxy_id, CRYPTO_BTC_USD], 60_u32)
        .await;
    assert_eq!(
        result.get(&btc_proxy_id).unwrap().as_ref().map(norm_price),
        Some(100_000),
    );
    assert!(result.get(&CRYPTO_BTC_USD).unwrap().is_none());

    // Pyth appears first on the list
    set!(pyth.CRYPTO_BTC_USD = 90_000).await;
    let result = proxy_oracle
        .list_ema_prices_no_older_than(
            &actor,
            vec![
                btc_proxy_id,
                CRYPTO_BTC_USD,
                btc_proxy_id,
                CRYPTO_BTC_USD,
                CRYPTO_BTC_USD,
            ],
            60_u32,
        )
        .await;
    assert_eq!(
        result.get(&btc_proxy_id).unwrap().as_ref().map(norm_price),
        Some(90_000),
    );
    assert_eq!(
        result
            .get(&CRYPTO_BTC_USD)
            .unwrap()
            .as_ref()
            .map(norm_price),
        Some(90_000),
    );
}
