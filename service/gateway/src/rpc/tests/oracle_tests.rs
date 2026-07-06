use super::*;

#[tokio::test]
async fn oracle_update_endpoints_work_against_sandbox() -> Result<()> {
    let hermes = start_mock_hermes_server("cafebabe").await?;
    let stack = TestStack::start_with_oracle_update_config(hermes.uri().parse()?).await?;

    let pyth_oracle_id = stack
        .harness
        .deploy_mock_oracle("pyth-oracle.near".parse()?)
        .await?;
    let redstone_oracle_id = stack
        .harness
        .deploy_redstone_adapter("redstone-oracle.near".parse()?)
        .await?;

    let pyth_result = stack
        .controller
        .request::<oracle_updates::UpdatePyth>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: oracle_updates::UpdatePyth {
                oracle_id: pyth_oracle_id.clone(),
                vaa: Base64Bytes(vec![0xde, 0xad, 0xbe, 0xef]),
            },
        })
        .await?;
    assert_eq!(
        pyth_result.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );
    assert_eq!(pyth_result.operation.steps.len(), 1);

    let last_pyth_update = view_contract_json(
        &stack,
        pyth_oracle_id.clone(),
        "last_pyth_update_data",
        serde_json::Value::Null,
    )
    .await?;
    assert_eq!(
        last_pyth_update,
        serde_json::Value::String("deadbeef".to_owned())
    );

    let redstone_result = stack
        .controller
        .request::<oracle_updates::UpdateRedStone>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: oracle_updates::UpdateRedStone {
                oracle_id: redstone_oracle_id.clone(),
                feed_id: "BTC".into(),
            },
        })
        .await?;
    assert_eq!(
        redstone_result.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );
    assert_eq!(redstone_result.operation.steps.len(), 1);

    let redstone_prices = view_contract_json(
        &stack,
        redstone_oracle_id,
        "read_price_data",
        serde_json::json!({ "feed_ids": ["BTC"] }),
    )
    .await?;
    assert_ne!(
        redstone_prices["BTC"]["price"],
        serde_json::Value::String("0".to_owned())
    );

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn oracle_update_prices_endpoint_resolves_and_updates_dependencies() -> Result<()> {
    let hermes = start_mock_hermes_server("cafebabe").await?;
    let stack = TestStack::start_with_oracle_update_config(hermes.uri().parse()?).await?;

    let direct_oracle_id = stack
        .harness
        .deploy_mock_oracle("composed-pyth.near".parse()?)
        .await?;
    let redstone_oracle_id = stack
        .harness
        .deploy_redstone_adapter("composed-redstone.near".parse()?)
        .await?;
    let proxy_oracle_id = stack.harness.deploy_proxy_oracle().await?;

    let proxy_direct_id = PriceIdentifier([0x11; 32]);
    let proxy_redstone_id = PriceIdentifier([0x22; 32]);

    stack
        .harness
        .admin_set_proxy(
            proxy_oracle_id.clone(),
            proxy_direct_id,
            Some(Proxy::median_low(
                [OracleRequest::pyth(
                    direct_oracle_id.clone(),
                    test_utils::DEFAULT_BORROW_PRICE_ID,
                )
                .into()],
                FreshnessFilter::empty(),
            )),
        )
        .await?;
    stack
        .harness
        .admin_set_proxy(
            proxy_oracle_id.clone(),
            proxy_redstone_id,
            Some(Proxy::median_low(
                [OracleRequest::redstone(redstone_oracle_id.clone(), "BTC").into()],
                FreshnessFilter::empty(),
            )),
        )
        .await?;

    let update_result = stack
        .controller
        .request::<oracle_updates::UpdatePrices>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: oracle_updates::UpdatePrices {
                oracle_id: proxy_oracle_id.clone(),
                price_ids: vec![proxy_direct_id, proxy_redstone_id],
            },
        })
        .await?;
    assert_eq!(
        update_result.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );
    // Two underlying updates (pyth + redstone) plus the proxy re-aggregation step the
    // gateway appends for a proxy oracle.
    assert_eq!(update_result.operation.steps.len(), 3);

    let pyth_update_count = view_contract_json(
        &stack,
        direct_oracle_id.clone(),
        "pyth_update_count",
        serde_json::Value::Null,
    )
    .await?;
    assert_eq!(pyth_update_count, serde_json::Value::String("1".to_owned()));

    let last_pyth_update = view_contract_json(
        &stack,
        direct_oracle_id,
        "last_pyth_update_data",
        serde_json::Value::Null,
    )
    .await?;
    assert_eq!(
        last_pyth_update,
        serde_json::Value::String("cafebabe".to_owned())
    );

    let redstone_prices = view_contract_json(
        &stack,
        redstone_oracle_id,
        "read_price_data",
        serde_json::json!({ "feed_ids": ["BTC"] }),
    )
    .await?;
    assert_ne!(
        redstone_prices["BTC"]["price"],
        serde_json::Value::String("0".to_owned())
    );

    // The appended proxy step re-aggregated the proxy's cache from the fresh underlying
    // prices, so the proxy now serves a cached price for the requested feed.
    let cached_proxy_price = view_contract_json(
        &stack,
        proxy_oracle_id,
        "get_cached_proxy_price",
        serde_json::json!({ "id": proxy_direct_id }),
    )
    .await?;
    assert_ne!(cached_proxy_price, serde_json::Value::Null);

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn oracle_resolution_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let direct_oracle_id = stack
        .harness
        .deploy_mock_oracle("direct-oracle.near".parse()?)
        .await?;
    let lst_oracle_id = stack
        .harness
        .deploy_lst_oracle("lst-oracle.near".parse()?, direct_oracle_id.clone())
        .await?;
    let proxy_oracle_id = stack.harness.deploy_proxy_oracle().await?;

    let direct_price_id = test_utils::DEFAULT_BORROW_PRICE_ID;
    let transformed_price_id = PriceIdentifier([0xa6; 32]);
    let proxy_direct_id = PriceIdentifier([0x01; 32]);
    let proxy_redstone_id = PriceIdentifier([0x02; 32]);

    stack
        .harness
        .create_lst_transformer(
            lst_oracle_id.clone(),
            transformed_price_id,
            PriceTransformer::lst(
                direct_price_id,
                24,
                price_transformer::Call {
                    account_id: stack.harness.ft_contract_id.clone(),
                    method_name: "redemption_rate".to_owned(),
                    args: near_sdk::json_types::Base64VecU8(serde_json::to_vec(
                        &serde_json::Value::Null,
                    )?),
                    gas: near_sdk::json_types::U64(near_sdk::Gas::from_tgas(3).as_gas()),
                },
            ),
        )
        .await?;

    stack
        .harness
        .admin_set_proxy(
            proxy_oracle_id.clone(),
            proxy_direct_id,
            Some(Proxy::median_low(
                [OracleRequest::pyth(direct_oracle_id.clone(), direct_price_id).into()],
                FreshnessFilter::empty(),
            )),
        )
        .await?;
    stack
        .harness
        .admin_set_proxy(
            proxy_oracle_id.clone(),
            proxy_redstone_id,
            Some(Proxy::median_low(
                [OracleRequest::redstone(direct_oracle_id.clone(), "BTC").into()],
                FreshnessFilter::empty(),
            )),
        )
        .await?;

    let direct = stack
        .controller
        .request::<oracle::GetPriceResolutionDependencies>(
            &oracle::GetPriceResolutionDependencies {
                oracle_id: direct_oracle_id.clone(),
                price_id: direct_price_id,
            },
        )
        .await?;
    assert_eq!(direct.kind, oracle::OracleContractKind::Direct);
    assert_eq!(
        direct.requests,
        vec![OracleRequest::pyth(
            direct_oracle_id.clone(),
            direct_price_id
        )]
    );

    let lst = stack
        .controller
        .request::<oracle::GetPriceResolutionDependencies>(
            &oracle::GetPriceResolutionDependencies {
                oracle_id: lst_oracle_id.clone(),
                price_id: transformed_price_id,
            },
        )
        .await?;
    assert_eq!(
        lst.kind,
        oracle::OracleContractKind::Lst {
            pyth_id: direct_oracle_id.clone()
        }
    );
    assert_eq!(
        lst.requests,
        vec![OracleRequest::pyth(
            direct_oracle_id.clone(),
            direct_price_id
        )]
    );

    let proxy = stack
        .controller
        .request::<oracle::GetPriceResolutionDependencies>(
            &oracle::GetPriceResolutionDependencies {
                oracle_id: proxy_oracle_id.clone(),
                price_id: proxy_direct_id,
            },
        )
        .await?;
    assert_eq!(proxy.kind, oracle::OracleContractKind::Proxy);
    assert_eq!(
        proxy.requests,
        vec![OracleRequest::pyth(
            direct_oracle_id.clone(),
            direct_price_id
        )]
    );

    let _ = stack
        .controller
        .request::<tx::FunctionCall>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: tx::FunctionCall {
                receiver_id: stack.harness.ft_contract_id.clone(),
                method_name: ContractMethodName("set_redemption_rate".to_owned()),
                args: ContractArgs::Json(serde_json::json!({
                    "redemption_rate": NearToken::from_near(2).as_yoctonear().to_string(),
                })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::from_yoctonear(0),
            },
        })
        .await?;

    let prices = stack
        .controller
        .request::<oracle::ResolvePrices>(&oracle::ResolvePrices {
            oracle_id: proxy_oracle_id,
            price_ids: vec![proxy_direct_id, proxy_redstone_id],
            age: 60,
            pyth: vec![oracle::PythOraclePrices {
                oracle_id: direct_oracle_id.clone(),
                response: [(direct_price_id, Some(pyth_price(100.0)))]
                    .into_iter()
                    .collect(),
            }],
            redstone: vec![oracle::RedStoneOraclePrices {
                oracle_id: direct_oracle_id.clone(),
                response: vec![oracle::RedStonePriceEntry {
                    feed_id: "BTC".into(),
                    data: redstone_price(42.0),
                }],
            }],
            lazer: vec![],
        })
        .await?;

    assert_eq!(prices.prices.len(), 2);
    assert_eq!(prices.prices[0].price_id, proxy_direct_id);
    assert_same_pyth_price_value(prices.prices[0].price.clone(), &pyth_price(100.0));
    assert_eq!(prices.prices[1].price_id, proxy_redstone_id);
    assert_same_pyth_price_value(
        prices.prices[1].price.clone(),
        &redstone_price(42.0)
            .to_pyth_price()
            .expect("redstone price should convert to pyth price"),
    );

    let one_price = stack
        .controller
        .request::<oracle::ResolvePrice>(&oracle::ResolvePrice {
            oracle_id: lst_oracle_id.clone(),
            price_id: transformed_price_id,
            age: 60,
            pyth: vec![oracle::PythOraclePrices {
                oracle_id: direct_oracle_id.clone(),
                response: [(direct_price_id, Some(pyth_price(100.0)))]
                    .into_iter()
                    .collect(),
            }],
            redstone: vec![],
            lazer: vec![],
        })
        .await?;
    assert!(one_price.price.is_some());

    stack
        .harness
        .set_mock_oracle_pyth_price(
            direct_oracle_id.clone(),
            direct_price_id,
            Some(pyth_price(123.0)),
        )
        .await?;
    stack
        .harness
        .set_mock_oracle_redstone_price(
            direct_oracle_id.clone(),
            "BTC".into(),
            Some(redstone_price(55.0)),
        )
        .await?;

    let on_chain = stack
        .controller
        .request::<oracle::GetPrices>(&oracle::GetPrices {
            oracle_id: lst_oracle_id,
            price_ids: vec![direct_price_id, transformed_price_id],
            age: 60,
        })
        .await?;

    assert_eq!(on_chain.prices.len(), 2);
    assert_eq!(on_chain.prices[0].price_id, direct_price_id);
    let direct = on_chain.prices[0]
        .price
        .clone()
        .expect("direct price should resolve");
    let expected = pyth_price(123.0);
    assert_eq!(direct.price, expected.price);
    assert_eq!(direct.conf, expected.conf);
    assert_eq!(direct.expo, expected.expo);
    assert!(on_chain.prices[1].price.is_some());

    let one_on_chain = stack
        .controller
        .request::<oracle::GetPrice>(&oracle::GetPrice {
            oracle_id: direct_oracle_id,
            price_id: direct_price_id,
            age: 60,
        })
        .await?;
    assert!(one_on_chain.price.is_some());

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn oracle_resolve_price_for_direct_pyth_pro_adapter_consumes_lazer_input() -> Result<()> {
    let stack = TestStack::start().await?;

    let adapter_id: near_account_id::AccountId = "resolve-pyth-pro.near".parse()?;
    let price_id = PriceIdentifier([0x66; 32]);
    let feed_id = 11u32;
    stack
        .harness
        .deploy_pyth_pro_adapter(adapter_id.clone(), price_id, feed_id)
        .await?;

    // A direct Pyth Pro adapter resolves to a Lazer feed, so a caller following
    // `getPriceResolutionDependencies` supplies the price under `lazer` keyed by feed id;
    // `resolvePrice` must consume it there rather than looking for a Pyth-keyed response.
    let resolved = stack
        .controller
        .request::<oracle::ResolvePrice>(&oracle::ResolvePrice {
            oracle_id: adapter_id.clone(),
            price_id,
            age: 60,
            pyth: vec![],
            redstone: vec![],
            lazer: vec![oracle::LazerOraclePrices {
                oracle_id: adapter_id.clone(),
                response: [(feed_id, Some(pyth_price(321.0)))].into_iter().collect(),
            }],
        })
        .await?;
    assert_same_pyth_price_value(resolved.price, &pyth_price(321.0));

    // Regression guard: a Pyth-shaped input (keyed by `PriceIdentifier`) must NOT resolve —
    // proving the adapter is consumed via its Lazer feed, not the classic Pyth path.
    let not_resolved = stack
        .controller
        .request::<oracle::ResolvePrice>(&oracle::ResolvePrice {
            oracle_id: adapter_id.clone(),
            price_id,
            age: 60,
            pyth: vec![oracle::PythOraclePrices {
                oracle_id: adapter_id,
                response: [(price_id, Some(pyth_price(321.0)))].into_iter().collect(),
            }],
            redstone: vec![],
            lazer: vec![],
        })
        .await?;
    assert!(
        not_resolved.price.is_none(),
        "a direct Pyth Pro adapter must not resolve from a Pyth-keyed input"
    );

    stack.shutdown().await;
    Ok(())
}
