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
            body: oracle_updates::UpdatePythBody {
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
            body: oracle_updates::UpdateRedStoneBody {
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

    let _ = stack
        .controller
        .request::<proxy_oracle_governance::Create>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            body: proxy_oracle_governance::CreateBody {
                oracle_id: proxy_oracle_id.clone(),
                id: 0,
                operation: templar_proxy_oracle_near_governance_common::Operation::SetProxy {
                    id: proxy_direct_id,
                    proxy: Some(Proxy::median_low(
                        [OracleRequest::pyth(
                            direct_oracle_id.clone(),
                            test_utils::DEFAULT_BORROW_PRICE_ID,
                        )
                        .into()],
                        FreshnessFilter::empty(),
                    )),
                },
            },
        })
        .await?;
    let _ = stack
        .controller
        .request::<proxy_oracle_governance::Execute>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            body: proxy_oracle_governance::ExecuteBody {
                oracle_id: proxy_oracle_id.clone(),
                id: 0,
            },
        })
        .await?;

    let _ = stack
        .controller
        .request::<proxy_oracle_governance::Create>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            body: proxy_oracle_governance::CreateBody {
                oracle_id: proxy_oracle_id.clone(),
                id: 1,
                operation: templar_proxy_oracle_near_governance_common::Operation::SetProxy {
                    id: proxy_redstone_id,
                    proxy: Some(Proxy::median_low(
                        [OracleRequest::redstone(redstone_oracle_id.clone(), "BTC").into()],
                        FreshnessFilter::empty(),
                    )),
                },
            },
        })
        .await?;
    let _ = stack
        .controller
        .request::<proxy_oracle_governance::Execute>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            body: proxy_oracle_governance::ExecuteBody {
                oracle_id: proxy_oracle_id.clone(),
                id: 1,
            },
        })
        .await?;

    let update_result = stack
        .controller
        .request::<oracle_updates::UpdatePrices>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: oracle_updates::UpdatePricesBody {
                oracle_id: proxy_oracle_id,
                price_ids: vec![proxy_direct_id, proxy_redstone_id],
            },
        })
        .await?;
    assert_eq!(
        update_result.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );
    assert_eq!(update_result.operation.steps.len(), 2);

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

    let _ = stack
        .controller
        .request::<proxy_oracle_governance::Create>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            body: proxy_oracle_governance::CreateBody {
                oracle_id: proxy_oracle_id.clone(),
                id: 0,
                operation: templar_proxy_oracle_near_governance_common::Operation::SetProxy {
                    id: proxy_direct_id,
                    proxy: Some(Proxy::median_low(
                        [OracleRequest::pyth(direct_oracle_id.clone(), direct_price_id).into()],
                        FreshnessFilter::empty(),
                    )),
                },
            },
        })
        .await?;
    let _ = stack
        .controller
        .request::<proxy_oracle_governance::Execute>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            body: proxy_oracle_governance::ExecuteBody {
                oracle_id: proxy_oracle_id.clone(),
                id: 0,
            },
        })
        .await?;

    let _ = stack
        .controller
        .request::<proxy_oracle_governance::Create>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            body: proxy_oracle_governance::CreateBody {
                oracle_id: proxy_oracle_id.clone(),
                id: 1,
                operation: templar_proxy_oracle_near_governance_common::Operation::SetProxy {
                    id: proxy_redstone_id,
                    proxy: Some(Proxy::median_low(
                        [OracleRequest::redstone(direct_oracle_id.clone(), "BTC").into()],
                        FreshnessFilter::empty(),
                    )),
                },
            },
        })
        .await?;
    let _ = stack
        .controller
        .request::<proxy_oracle_governance::Execute>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            body: proxy_oracle_governance::ExecuteBody {
                oracle_id: proxy_oracle_id.clone(),
                id: 1,
            },
        })
        .await?;

    let direct = stack
        .controller
        .request::<oracle::GetPriceResolutionDependencies>(&ReadRequest {
            params: oracle::GetPriceResolutionDependenciesParams {
                oracle_id: direct_oracle_id.clone(),
                price_id: direct_price_id,
            },
        })
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
        .request::<oracle::GetPriceResolutionDependencies>(&ReadRequest {
            params: oracle::GetPriceResolutionDependenciesParams {
                oracle_id: lst_oracle_id.clone(),
                price_id: transformed_price_id,
            },
        })
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
        .request::<oracle::GetPriceResolutionDependencies>(&ReadRequest {
            params: oracle::GetPriceResolutionDependenciesParams {
                oracle_id: proxy_oracle_id.clone(),
                price_id: proxy_direct_id,
            },
        })
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
            body: tx::FunctionCallBody {
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
        .request::<oracle::ResolvePrices>(&ReadRequest {
            params: oracle::ResolvePricesParams {
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
            },
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
        .request::<oracle::ResolvePrice>(&ReadRequest {
            params: oracle::ResolvePriceParams {
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
            },
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
        .request::<oracle::GetPrices>(&ReadRequest {
            params: oracle::GetPricesParams {
                oracle_id: lst_oracle_id,
                price_ids: vec![direct_price_id, transformed_price_id],
                age: 60,
            },
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
        .request::<oracle::GetPrice>(&ReadRequest {
            params: oracle::GetPriceParams {
                oracle_id: direct_oracle_id,
                price_id: direct_price_id,
                age: 60,
            },
        })
        .await?;
    assert!(one_on_chain.price.is_some());

    stack.shutdown().await;
    Ok(())
}
