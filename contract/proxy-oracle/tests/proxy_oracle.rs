use std::collections::{HashMap, HashSet};

use near_sdk::{
    json_types::{I64, U64},
    test_utils::VMContextBuilder,
    testing_env, AccountIdRef, Gas, NearToken,
};
use near_workspaces::{network::Sandbox, Worker};

use templar_common::{
    governance::Proposal,
    oracle::{
        price_transformer::{self, ProxyPriceTransformer},
        proxy::{
            governance::{Operation, ProxyGovernanceInterface},
            Proxy, Source,
        },
        pyth::{self, PriceIdentifier, PythTimestamp},
        redstone::FeedData,
        OracleRequest,
    },
    primitive_types,
    time::Nanoseconds,
};
use templar_proxy_oracle_contract::Contract;
use test_utils::{
    accounts,
    controller::proxy_oracle::ProxyOracleController,
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
            // Skip unknown.
            continue;
        };

        for entry in proxy.entries {
            let request = match entry.source {
                Source::Request(request) => request,
                Source::Transformer(transformer) => {
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

#[rstest::rstest]
#[case::success(10 * 1000)]
#[should_panic = "TTL not yet elapsed for proposal"]
#[case::fail(0)]
#[should_panic = "TTL not yet elapsed for proposal"]
#[case::fail(10 * 1000 - 1)]
pub fn governance_ttl(#[case] delay_ms: u64) {
    let mut context = VMContextBuilder::new()
        .attached_deposit(NearToken::from_yoctonear(1))
        .block_timestamp(1_000_000)
        .predecessor_account_id("owner.near".parse().unwrap())
        .build();
    testing_env!(context.clone());

    let mut c = Contract::new();

    assert_eq!(c.gov_count(), 0);
    assert_eq!(c.gov_next_id(), 0);
    assert_eq!(c.gov_get(0), None);
    assert_eq!(c.gov_list(None, None), Vec::<u32>::new());
    assert_eq!(c.gov_ttl_ns(), Nanoseconds::zero());

    let proposal = c.gov_create(
        0,
        Operation::SetActionTtl {
            new_ttl: Nanoseconds::from_secs(10),
        },
    );

    let expected = Proposal {
        operation: Operation::SetActionTtl {
            new_ttl: Nanoseconds::from_secs(10),
        },
        ttl: Nanoseconds::zero(),
        created_at: Nanoseconds::from_ms(1),
        created_by: "owner.near".parse().unwrap(),
    };

    assert_eq!(proposal, expected);
    assert_eq!(c.gov_get(0).unwrap(), expected);
    assert_eq!(c.gov_list(Some(0), Some(1)), vec![0]);
    assert_eq!(c.gov_list(None, None), vec![0]);
    assert_eq!(c.gov_count(), 1);
    assert_eq!(c.gov_next_id(), 1);
    assert_eq!(c.gov_ttl_ns(), Nanoseconds::zero());

    c.gov_execute(0);
    assert_eq!(c.gov_get(0), None);
    assert_eq!(c.gov_list(Some(0), Some(1)), Vec::<u32>::new());
    assert_eq!(c.gov_list(None, None), Vec::<u32>::new());
    assert_eq!(c.gov_count(), 0);
    assert_eq!(c.gov_next_id(), 1);
    assert_eq!(c.gov_ttl_ns(), Nanoseconds::from_secs(10));

    let proxy_id = PriceIdentifier([0x01_u8; 32]);
    let proxy_def = Proxy::median_low([OracleRequest::pyth(
        "pyth-oracle.near".parse().unwrap(),
        CRYPTO_BTC_USD,
    )
    .into()]);

    let proposal = c.gov_create(
        1,
        Operation::SetProxy {
            id: proxy_id,
            proxy: Some(proxy_def.clone()),
        },
    );
    let expected = Proposal {
        operation: Operation::SetProxy {
            id: proxy_id,
            proxy: Some(proxy_def),
        },
        ttl: Nanoseconds::from_secs(10),
        created_at: Nanoseconds::from_ms(1),
        created_by: "owner.near".parse().unwrap(),
    };
    assert_eq!(proposal, expected);
    assert_eq!(c.gov_get(1).unwrap(), expected);
    assert_eq!(c.gov_list(Some(0), Some(2)), vec![1]);
    assert_eq!(c.gov_list(None, None), vec![1]);
    assert_eq!(c.gov_count(), 1);
    assert_eq!(c.gov_next_id(), 2);
    assert_eq!(c.gov_ttl_ns(), Nanoseconds::from_secs(10));

    context.block_timestamp += delay_ms * 1_000_000;
    testing_env!(context.clone());

    c.gov_execute(1);
}

#[test]
#[should_panic = "Empty proxy definition is not allowed"]
fn governance_rejects_empty_proxy_definition_on_create() {
    let context = VMContextBuilder::new()
        .attached_deposit(NearToken::from_yoctonear(1))
        .build();
    testing_env!(context);

    let mut c = Contract::new();
    c.gov_create(
        0,
        Operation::SetProxy {
            id: PriceIdentifier([0xFF; 32]),
            proxy: Some(Proxy::median_low([])),
        },
    );
}

#[allow(clippy::unwrap_used)]
#[test]
pub fn gas() {
    let context = VMContextBuilder::new()
        .attached_deposit(NearToken::from_yoctonear(1))
        .build();
    testing_env!(context.clone());

    let mut c = Contract::new();

    let proxy_btc = Proxy::median_low([
        OracleRequest::pyth("pyth-oracle.near".parse().unwrap(), CRYPTO_BTC_USD).into(),
        OracleRequest::redstone("redstone-adapter.near".parse().unwrap(), "BTC").into(),
    ]);
    let proxy_btc_id = PriceIdentifier([0x01_u8; 32]);

    let proxy_usdc = Proxy::median_low([
        OracleRequest::pyth(
            "pyth-oracle.near".parse().unwrap(),
            pyth_price_id::stable::CRYPTO_USDC_USD,
        )
        .into(),
        OracleRequest::redstone("redstone-adapter.near".parse().unwrap(), "USDC").into(),
    ]);
    let proxy_usdc_id = PriceIdentifier([0x02_u8; 32]);

    let proxy_wnear = Proxy::median_low([ProxyPriceTransformer::lst(
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
    let proxy_wnear_id = PriceIdentifier([0x03_u8; 32]);

    let price_ids = vec![proxy_btc_id, proxy_usdc_id, proxy_wnear_id];

    c.gov_create(
        0,
        Operation::SetProxy {
            id: proxy_btc_id,
            proxy: Some(proxy_btc.clone()),
        },
    );
    c.gov_create(
        1,
        Operation::SetProxy {
            id: proxy_usdc_id,
            proxy: Some(proxy_usdc.clone()),
        },
    );
    c.gov_create(
        2,
        Operation::SetProxy {
            id: proxy_wnear_id,
            proxy: Some(proxy_wnear.clone()),
        },
    );
    c.gov_execute(0);
    c.gov_execute(1);
    c.gov_execute(2);
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

#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
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
    let pyth_oracle = MockOracleController::deploy(pyth_oracle);
    let pyth_oracle2 = MockOracleController::deploy(pyth_oracle2);
    let redstone_adapter = MockOracleController::deploy(redstone_adapter);
    let proxy_oracle = ProxyOracleController::deploy(proxy_oracle);
    let (pyth_oracle, pyth_oracle2, redstone_adapter, proxy_oracle) =
        tokio::join!(pyth_oracle, pyth_oracle2, redstone_adapter, proxy_oracle);

    let list_proxies = proxy_oracle.list_proxies(None, None).await;
    assert_eq!(list_proxies, vec![]);

    macro_rules! set {
        (pyth . $id: ident = $val: literal) => {
            set!(
                pyth.$id = Some(pyth::Price {
                    price: I64($val),
                    conf: U64(0),
                    expo: 0,
                    publish_time: PythTimestamp::from_secs(
                        std::time::UNIX_EPOCH.elapsed().unwrap().as_secs() as i64
                    ),
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
                    publish_time: PythTimestamp::from_secs(
                        std::time::UNIX_EPOCH.elapsed().unwrap().as_secs() as i64
                    ),
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
                    package_timestamp: templar_common::time::Nanoseconds::from_ms(
                        std::time::UNIX_EPOCH.elapsed().unwrap().as_millis() as u64
                    ),
                    write_timestamp: templar_common::time::Nanoseconds::from_ms(
                        std::time::UNIX_EPOCH.elapsed().unwrap().as_millis() as u64
                    ),
                })
            )
        };
        (redstone . $id: ident = $val: expr) => {
            redstone_adapter.set_redstone_price(&actor, stringify!($id), $val)
        };
    }

    let btc_proxy_def = Proxy::median_low([
        OracleRequest::pyth(pyth_oracle.id().clone(), CRYPTO_BTC_USD).into(),
        OracleRequest::redstone(redstone_adapter.id().clone(), "BTC").into(),
        OracleRequest::pyth(pyth_oracle2.id().clone(), CRYPTO_BTC_USD).into(),
    ]);
    let btc_proxy_id = PriceIdentifier([0x01_u8; 32]);
    let just_pyth_btc =
        Proxy::median_low([OracleRequest::pyth(pyth_oracle.id().clone(), CRYPTO_BTC_USD).into()]);
    let just_pyth_btc_id = PriceIdentifier([0x02_u8; 32]);
    let just_redstone_eth =
        Proxy::median_low([OracleRequest::redstone(redstone_adapter.id().clone(), "ETH").into()]);
    let just_redstone_eth_id = PriceIdentifier([0x03_u8; 32]);

    proxy_oracle
        .set_proxy(
            proxy_oracle.account(),
            btc_proxy_id,
            Some(btc_proxy_def.clone()),
        )
        .await;
    proxy_oracle
        .set_proxy(
            proxy_oracle.account(),
            just_pyth_btc_id,
            Some(just_pyth_btc.clone()),
        )
        .await;
    proxy_oracle
        .set_proxy(
            proxy_oracle.account(),
            just_redstone_eth_id,
            Some(just_redstone_eth.clone()),
        )
        .await;

    assert_eq!(
        proxy_oracle.list_proxies(None, None).await,
        vec![btc_proxy_id, just_pyth_btc_id, just_redstone_eth_id],
    );
    assert_eq!(
        proxy_oracle.get_proxy(btc_proxy_id).await.unwrap(),
        btc_proxy_def,
    );

    let result = proxy_oracle
        .list_ema_prices_no_older_than(&actor, vec![btc_proxy_id, CRYPTO_BTC_USD], 60_u32)
        .await;
    assert_eq!(result, HashMap::from_iter([(btc_proxy_id, None)]));

    set!(redstone.BTC = 100_000).await;
    let result = proxy_oracle
        .list_ema_prices_no_older_than(&actor, vec![btc_proxy_id, CRYPTO_BTC_USD], 60_u32)
        .await;
    assert_eq!(result.len(), 1);
    assert_eq!(
        result.get(&btc_proxy_id).unwrap().as_ref().map(norm_price),
        Some(100_000),
    );

    set!(pyth.CRYPTO_BTC_USD = 90_000).await;
    set!(redstone.ETH = 1_800).await;
    let result = proxy_oracle
        .list_ema_prices_no_older_than(
            &actor,
            vec![
                btc_proxy_id,
                CRYPTO_BTC_USD,
                btc_proxy_id,
                CRYPTO_BTC_USD,
                CRYPTO_BTC_USD,
                just_pyth_btc_id,
                just_redstone_eth_id,
            ],
            60_u32,
        )
        .await;
    assert_eq!(result.len(), 3);
    assert_eq!(
        result.get(&btc_proxy_id).unwrap().as_ref().map(norm_price),
        Some(90_000),
    );
    assert_eq!(
        result
            .get(&just_pyth_btc_id)
            .unwrap()
            .as_ref()
            .map(norm_price),
        Some(90_000),
    );
    assert_eq!(
        result
            .get(&just_redstone_eth_id)
            .unwrap()
            .as_ref()
            .map(norm_price),
        Some(1_800),
    );

    // Set second Pyth oracle
    set!(pyth2.CRYPTO_BTC_USD = 80_000).await;
    let result = proxy_oracle
        .list_ema_prices_no_older_than(&actor, vec![btc_proxy_id, CRYPTO_BTC_USD], 60_u32)
        .await;
    assert_eq!(result.len(), 1);
    assert_eq!(
        result.get(&btc_proxy_id).unwrap().as_ref().map(norm_price),
        Some(90_000),
    );

    set!(pyth.CRYPTO_BTC_USD = None).await;
    set!(redstone.BTC = None).await;
    let result = proxy_oracle
        .list_ema_prices_no_older_than(&actor, vec![btc_proxy_id, CRYPTO_BTC_USD], 60_u32)
        .await;
    assert_eq!(result.len(), 1);
    assert_eq!(
        result.get(&btc_proxy_id).unwrap().as_ref().map(norm_price),
        Some(80_000),
    );
}
