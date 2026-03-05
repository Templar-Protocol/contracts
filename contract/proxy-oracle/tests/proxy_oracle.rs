use std::collections::{HashMap, HashSet};

use near_sdk::{
    json_types::{I64, U64},
    test_utils::VMContextBuilder,
    testing_env, AccountIdRef, Gas, NearToken,
};
use near_workspaces::{network::Sandbox, Worker};

use templar_common::{
    oracle::{
        price_transformer::{self, ProxyPriceTransformer},
        proxy::{Proxy, ProxyEntry},
        pyth::{self, PriceIdentifier},
        redstone::FeedData,
        OracleRequest,
    },
    primitive_types,
};
use templar_proxy_oracle_contract::Contract;
use test_utils::{
    accounts,
    controller::proxy_oracle::ProxyOracleController,
    print_execution,
    pyth_price_id::{self, stable::CRYPTO_BTC_USD},
    worker, ContractController, MockOracleController,
};

fn norm_price(price: &pyth::Price) -> u64 {
    #[allow(clippy::unwrap_used, reason = "test should panic on negative price")]
    let p = u64::try_from(price.price.0).unwrap();
    let f = 10u64.pow(price.expo.unsigned_abs());
    if price.expo.is_negative() {
        p / f
    } else {
        p * f
    }
}

fn estimate_gas(c: &Contract, price_ids: &[PriceIdentifier]) -> near_sdk::Gas {
    let mut total = Contract::GAS_FOR_LIST_00_ENTRY;

    let mut pyth = HashSet::new();
    let mut redstone = HashSet::new();

    for price_id in price_ids {
        let Some(proxy) = c.proxies.get(price_id) else {
            pyth.insert(c.passthrough_pyth_id.clone());
            continue;
        };

        for entry in proxy.0 {
            let request = match entry {
                ProxyEntry::Request(request) => request,
                ProxyEntry::Transformer(transformer) => {
                    total = total.saturating_add(Gas::from_gas(transformer.call.gas.0));
                    transformer.request
                }
            };

            match request {
                OracleRequest::Pyth(p) => {
                    pyth.insert(p.oracle_id);
                }
                OracleRequest::RedStone(p) => {
                    redstone.insert(p.oracle_id);
                }
            }
        }
    }

    total = total.saturating_add(Contract::GAS_FOR_PYTH_REQUEST.saturating_mul(pyth.len() as u64));
    total = total
        .saturating_add(Contract::GAS_FOR_REDSONE_REQUEST.saturating_mul(redstone.len() as u64));
    total = total.saturating_add(Contract::GAS_FOR_LIST_01_CALLBACK);

    total
}

#[allow(clippy::unwrap_used)]
#[test]
pub fn gas() {
    let context = VMContextBuilder::new()
        .attached_deposit(NearToken::from_yoctonear(1))
        .build();
    testing_env!(context.clone());

    let mut c = Contract::new("pyth-oracle.near".parse().unwrap());

    let proxy_btc = Proxy(vec![
        OracleRequest::pyth("pyth-oracle.near".parse().unwrap(), CRYPTO_BTC_USD).into(),
        OracleRequest::redstone("redstone-adapter.near".parse().unwrap(), "BTC").into(),
    ]);

    let proxy_usdc = Proxy(vec![
        OracleRequest::pyth(
            "pyth-oracle.near".parse().unwrap(),
            pyth_price_id::stable::CRYPTO_USDC_USD,
        )
        .into(),
        OracleRequest::redstone("redstone-adapter.near".parse().unwrap(), "USDC").into(),
    ]);

    let proxy_wnear = Proxy(vec![ProxyPriceTransformer::lst(
        OracleRequest::pyth(
            "pyth-oracle.near".parse().unwrap(),
            pyth_price_id::stable::CRYPTO_NEAR_USD,
        ),
        24,
        price_transformer::Call::new_simple(
            AccountIdRef::new_or_panic("wrap.near"),
            "redemption_rate",
        ),
    )
    .into()]);

    let price_ids = vec![
        proxy_btc.id().unwrap(),
        proxy_usdc.id().unwrap(),
        proxy_wnear.id().unwrap(),
    ];

    c.add_proxy(proxy_btc);
    c.add_proxy(proxy_usdc);
    c.add_proxy(proxy_wnear);
    let gas = estimate_gas(&c, &price_ids);
    eprintln!("Gas used: {gas}");
    assert!(gas <= Gas::from_tgas(15));

    c.list_ema_prices_no_older_than(price_ids, 60);

    for receipt in near_sdk::test_utils::get_created_receipts() {
        println!("Receipt to {}", receipt.receiver_id);
        for action in &receipt.actions {
            use near_sdk::mock::MockAction;

            match action {
                MockAction::CreateReceipt {
                    receipt_indices,
                    receiver_id,
                } => {
                    println!("  CreateReceipt to {receiver_id}");
                    for receipt_index in receipt_indices {
                        println!("    Receipt index: {receipt_index}");
                    }
                }
                MockAction::FunctionCallWeight {
                    method_name,
                    args,
                    attached_deposit,
                    prepaid_gas,
                    gas_weight,
                    ..
                } => {
                    println!("  FunctionCall to '{}' with args '{}', attached_deposit {}, prepaid_gas {}, gas_weight {:?}",
                        String::from_utf8_lossy(method_name), String::from_utf8_lossy(args), attached_deposit, prepaid_gas, gas_weight);
                }
                MockAction::Transfer { deposit, .. } => {
                    println!("  Transfer of {deposit} yoctoNEAR");
                }
                _ => {
                    println!("  Other action: {action:?}");
                }
            }
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
#[rstest::rstest]
#[tokio::test]
pub async fn proxy_oracle(#[future(awt)] worker: Worker<Sandbox>) {
    accounts!(
        worker,
        actor,
        redstone_adapter,
        proxy_oracle,
        pyth_oracle,
        pyth_oracle2
    );
    let pyth_oracle_id = pyth_oracle.id().clone();
    let pyth_oracle = MockOracleController::deploy(pyth_oracle);
    let pyth_oracle2 = MockOracleController::deploy(pyth_oracle2);
    let redstone_adapter = MockOracleController::deploy(redstone_adapter);
    let proxy_oracle = ProxyOracleController::deploy(proxy_oracle, pyth_oracle_id);
    let (pyth_oracle, pyth_oracle2, redstone_adapter, proxy_oracle) =
        tokio::join!(pyth_oracle, pyth_oracle2, redstone_adapter, proxy_oracle);

    let passthrough_pyth_id = proxy_oracle.passthrough_pyth_id().await;
    assert_eq!(&passthrough_pyth_id, pyth_oracle.id());

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
        (pyth2 . $id: ident = $val: literal) => {
            set!(
                pyth2.$id = Some(pyth::Price {
                    price: I64($val),
                    conf: U64(0),
                    expo: 0,
                    publish_time: std::time::UNIX_EPOCH.elapsed().unwrap().as_millis() as i64,
                })
            )
        };
        (pyth2 . $id: ident = $val: expr) => {
            pyth_oracle2.set_pyth_price(&actor, pyth_price_id::stable::$id, $val)
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
        OracleRequest::pyth(pyth_oracle.id().clone(), CRYPTO_BTC_USD).into(),
        OracleRequest::redstone(redstone_adapter.id().clone(), "BTC").into(),
        OracleRequest::pyth(pyth_oracle2.id().clone(), CRYPTO_BTC_USD).into(),
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
        .list_ema_prices_no_older_than_exec(&actor, vec![btc_proxy_id, CRYPTO_BTC_USD], 60_u32)
        .await;
    print_execution(&result);

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

    // Set second Pyth oracle
    set!(pyth2.CRYPTO_BTC_USD = 80_000).await;
    let result = proxy_oracle
        .list_ema_prices_no_older_than_exec(&actor, vec![btc_proxy_id, CRYPTO_BTC_USD], 60_u32)
        .await;
    print_execution(&result);
    let result = proxy_oracle
        .list_ema_prices_no_older_than(&actor, vec![btc_proxy_id, CRYPTO_BTC_USD], 60_u32)
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

    set!(pyth.CRYPTO_BTC_USD = None).await;
    set!(redstone.BTC = None).await;
    let result = proxy_oracle
        .list_ema_prices_no_older_than(&actor, vec![btc_proxy_id, CRYPTO_BTC_USD], 60_u32)
        .await;
    assert_eq!(
        result.get(&btc_proxy_id).unwrap().as_ref().map(norm_price),
        Some(80_000),
    );
    assert_eq!(
        result
            .get(&CRYPTO_BTC_USD)
            .unwrap()
            .as_ref()
            .map(norm_price),
        None,
    );
}
