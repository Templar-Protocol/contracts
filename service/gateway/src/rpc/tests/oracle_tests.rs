use super::*;
use templar_gateway_methods_spec::lazer as lazer_methods;

#[tokio::test]
async fn oracle_update_endpoints_work_against_sandbox() -> Result<()> {
    let hermes = start_mock_hermes_server("cafebabe").await?;
    let stack = TestStack::start_with_oracle_update_config(hermes.uri().parse()?).await?;

    let pyth_oracle_id = stack.harness.deploy_mock_oracle("pyth-oracle").await?;
    let redstone_oracle_id = stack
        .harness
        .deploy_redstone_adapter("redstone-oracle")
        .await?;

    let pyth_result = stack
        .controller
        .request::<oracle_updates::UpdatePyth>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: oracle_updates::UpdatePyth {
                oracle_id: pyth_oracle_id.clone(),
                price_ids: vec![PriceIdentifier([0x11; 32])],
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
        serde_json::Value::String("cafebabe".to_owned()),
        "the adapter must receive the VAA the gateway fetched from Hermes"
    );

    let redstone_result = stack
        .controller
        .request::<oracle_updates::UpdateRedStone>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: oracle_updates::UpdateRedStone {
                oracle_id: redstone_oracle_id.clone(),
                feed_ids: vec!["BTC".into()],
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
        redstone_oracle_id.clone(),
        "read_price_data",
        serde_json::json!({ "feed_ids": ["BTC"] }),
    )
    .await?;
    assert_ne!(
        redstone_prices["BTC"]["price"],
        serde_json::Value::String("0".to_owned())
    );

    // No feeds is nothing to fetch and nothing to write: a terminal, step-less no-op
    // rather than an empty `redstone.writePrices` on chain. The CLI cannot reach this
    // (`--feed-id` is required); a raw JSON-RPC caller can.
    let empty_result = stack
        .controller
        .request::<oracle_updates::UpdateRedStone>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: oracle_updates::UpdateRedStone {
                oracle_id: redstone_oracle_id,
                feed_ids: vec![],
            },
        })
        .await?;
    assert_eq!(
        empty_result.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );
    assert!(
        empty_result.operation.steps.is_empty(),
        "an empty update must plan no steps, got {:?}",
        empty_result.operation.steps
    );

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn oracle_update_prices_endpoint_resolves_and_updates_dependencies() -> Result<()> {
    let hermes = start_mock_hermes_server("cafebabe").await?;
    let stack = TestStack::start_with_oracle_update_config(hermes.uri().parse()?).await?;

    let direct_oracle_id = stack.harness.deploy_mock_oracle("composed-pyth").await?;
    let redstone_oracle_id = stack
        .harness
        .deploy_redstone_adapter("composed-redstone")
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
    let direct_oracle_id = stack.harness.deploy_mock_oracle("direct-oracle").await?;
    let lst_oracle_id = stack
        .harness
        .deploy_lst_oracle("lst-oracle", direct_oracle_id.clone())
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

    assert_eq!(prices.len(), 2);
    assert_eq!(prices[0].price_id, proxy_direct_id);
    assert_same_pyth_price_value(prices[0].price.clone(), &pyth_price(100.0));
    assert_eq!(prices[1].price_id, proxy_redstone_id);
    assert_same_pyth_price_value(
        prices[1].price.clone(),
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
    assert!(one_price.is_some());

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

    assert_eq!(on_chain.len(), 2);
    assert_eq!(on_chain[0].price_id, direct_price_id);
    let direct = on_chain[0]
        .price
        .clone()
        .expect("direct price should resolve");
    let expected = pyth_price(123.0);
    assert_eq!(direct.price, expected.price);
    assert_eq!(direct.conf, expected.conf);
    assert_eq!(direct.expo, expected.expo);
    assert!(on_chain[1].price.is_some());

    let one_on_chain = stack
        .controller
        .request::<oracle::GetPrice>(&oracle::GetPrice {
            oracle_id: direct_oracle_id,
            price_id: direct_price_id,
            age: 60,
        })
        .await?;
    assert!(one_on_chain.is_some());

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn oracle_resolve_rejects_bare_pyth_lazer_adapter_as_standalone_oracle() -> Result<()> {
    let stack = TestStack::start().await?;

    let price_id = PriceIdentifier([0x66; 32]);
    let adapter_id = stack
        .harness
        .deploy_pyth_lazer_adapter("resolve-pyth-lazer")
        .await?;

    // A Pyth Lazer adapter is not a standalone oracle: it is consumed only as a proxy `Lazer`
    // source. Both the dependency query and price resolution must reject a bare adapter,
    // pointing the operator at a proxy wrapper.
    let deps_error = stack
        .controller
        .request::<oracle::GetPriceResolutionDependencies>(
            &oracle::GetPriceResolutionDependencies {
                oracle_id: adapter_id.clone(),
                price_id,
            },
        )
        .await
        .expect_err("getPriceResolutionDependencies must reject a bare Pyth Lazer adapter");
    assert!(
        format!("{deps_error}").contains("proxy"),
        "error must direct the operator to wrap the adapter in a proxy; got: {deps_error}"
    );

    let resolve_error = stack
        .controller
        .request::<oracle::ResolvePrice>(&oracle::ResolvePrice {
            oracle_id: adapter_id.clone(),
            price_id,
            age: 60,
            pyth: vec![],
            redstone: vec![],
            lazer: vec![oracle::LazerOraclePrices {
                oracle_id: adapter_id.clone(),
                response: vec![oracle::LazerPriceEntry {
                    feed_id: 11,
                    data: lazer_feed(321.0),
                }],
            }],
        })
        .await
        .expect_err("resolvePrice must reject a bare Pyth Lazer adapter");
    assert!(
        format!("{resolve_error}").contains("standalone oracle"),
        "error must explain the adapter is not a standalone oracle; got: {resolve_error}"
    );

    stack.shutdown().await;
    Ok(())
}

// Signed ETH/BTC fixture from common::oracle::redstone::adapter::tests::output.
const REDSTONE_VALID_PAYLOAD: &[u8] = &hex_literal::hex!("45544800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002d9030a710019c56f0bec0000000200000015d1cb1a708c63264741b00ce097176e45f708914b8cfdca26b079877a70604e25aa0bcfa3a41df8212eddd51db3496b95c7c3dc4caa9ac9705602af0515db1b31c45544800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002d9028ed04019c56f0bec000000020000001dcaf484941c0d206f1898185b953c6a92d7fd188b347505c0f5beb2030e06e3e1b2f7dfb45929ac7676136af93fee7f14a614b40fa4dc2d1e625dbece02eaca21c45544800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002d9028ed04019c56f0bec00000002000000199bd54930138268baad2869e9ceb99b6bc67cd6b8a4cc98e05f0b1cd9b7f07066008208399a728fac3d1dc3ca407cb8199a0209377bceb0c48f2cc3d756078051b4254430000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006179a92ab8c019c56f0bec000000020000001f08af53ed34046f7f64cc02ffb7973252954d7c395e440693c896bffdbc2de1e31cf5675bf66583d3e3438f5002ae9c10870d4dc45de05c560b239aa3a2d50a41b425443000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000617a1187473019c56f0bec0000000200000011b96dc2763a692e3245ce4f1b0c16ea245c240204e99ebd323b340e58bfb14fb5f0465ce11b8dd52ff839547cc949d20e4e8ba0be43dd6417cade2a8ebfd8c9e1c425443000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000617a1187473019c56f0bec00000002000000114a02710892325b13afc74bbd350dd9ec80342b2d6c0c94df7b7a60dbf67a1b91b182fa4555e0e0db91e6258b279f00b7eeb8f5de9930e352d5321a6b8b64a031c00063137373039383531343539383223302e392e30237374656c6c61722d636f6e6e6563746f72000025000002ed57011e0000");

fn fixture_feed(id: u32, full: bool) -> pyth_lazer_protocol::payload::PayloadFeedData {
    use pyth_lazer_protocol::{payload::PayloadPropertyValue as Property, Price, PriceFeedId};
    let mut properties = vec![
        Property::Price(Some(Price::from_mantissa(123_456).unwrap())),
        Property::Confidence(Some(Price::from_mantissa(50).unwrap())),
        Property::Exponent(-8),
    ];
    if full {
        properties.extend([
            Property::EmaPrice(Some(Price::from_mantissa(123_000).unwrap())),
            Property::EmaConfidence(Some(Price::from_mantissa(40).unwrap())),
        ]);
    }
    pyth_lazer_protocol::payload::PayloadFeedData {
        feed_id: PriceFeedId(id),
        properties,
    }
}

fn signed_lazer_fixture(
    seed: u8,
    timestamp: Nanoseconds,
    feeds: Vec<pyth_lazer_protocol::payload::PayloadFeedData>,
    corrupt: bool,
) -> Result<Vec<u8>> {
    use ed25519_dalek::{Signer, SigningKey};
    use pyth_lazer_protocol::{
        message::SolanaMessage, payload::PayloadData, time::TimestampUs, ChannelId,
    };
    let key = SigningKey::from_bytes(&[seed; 32]);
    let mut payload = Vec::new();
    PayloadData {
        timestamp_us: TimestampUs::from_micros(timestamp.as_ns() / 1_000),
        channel_id: ChannelId::REAL_TIME,
        feeds,
    }
    .serialize::<byteorder::LE>(&mut payload)?;
    let mut message = SolanaMessage {
        signature: key.sign(&payload).to_bytes(),
        public_key: key.verifying_key().to_bytes(),
        payload,
    };
    if corrupt {
        message.signature[0] ^= 1;
    }
    let mut bytes = Vec::new();
    message.serialize(&mut bytes)?;
    Ok(bytes)
}

async fn deploy_fixture_adapters(
    harness: &SandboxHarness,
) -> Result<(near_account_id::AccountId, near_account_id::AccountId)> {
    let lazer_id = harness.deploy_pyth_lazer_adapter("provider-lazer").await?;
    let redstone_id = harness.deploy_redstone_adapter("provider-redstone").await?;
    let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let setup = harness
        .call_function_payable(
            &templar_gateway_types::ManagedAccountId(lazer_id.clone()),
            &lazer_id,
            "admin_set_signer",
            serde_json::json!({
                "public_key": hex::encode(key.verifying_key().to_bytes()), "expires_at_s": u64::MAX
            }),
            near_token::NearToken::from_yoctonear(1),
        )
        .await?;
    assert_eq!(
        setup.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );
    Ok((lazer_id, redstone_id))
}

#[tokio::test]
async fn oracle_provider_updates_match_stored_prices() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let (lazer_id, redstone_id) = deploy_fixture_adapters(&harness).await?;
    let timestamp = harness.chain_timestamp().await?;
    let payload = signed_lazer_fixture(
        7,
        timestamp,
        vec![
            fixture_feed(7, true),
            fixture_feed(8, false),
            fixture_feed(9, true),
        ],
        false,
    )?;
    let stack = TestStack::start_with_sources(
        harness,
        Url::parse("http://127.0.0.1:1")?,
        FakeLazerSource::with_payload(payload),
        TestRedStoneSource::Fixture(Ok(REDSTONE_VALID_PAYLOAD.to_vec())),
    )
    .await?;
    let outcome = async {
        let lazer_read = lazer_methods::GetFeedsData {
            oracle_id: lazer_id.clone(),
            feed_ids: vec![7, 8, 10],
        };
        let before = stack
            .controller
            .request::<lazer_methods::GetFeedsData>(&lazer_read)
            .await?;
        assert_eq!(before, HashMap::from([(7, None), (8, None), (10, None)]));
        let preview = stack
            .controller
            .request::<oracle_updates::GetLazerUpdate>(&oracle_updates::GetLazerUpdate {
                oracle_id: lazer_id.clone(),
                feed_ids: lazer_read.feed_ids.clone(),
            })
            .await?;
        assert_eq!(
            preview,
            HashMap::from([
                (
                    7,
                    Some(templar_common::oracle::lazer::FeedData {
                        price: I64(123_456),
                        conf: U64(50),
                        ema: templar_common::oracle::lazer::EmaData {
                            price: I64(123_000),
                            conf: U64(40)
                        },
                        expo: -8,
                        publish_time_ns: Nanoseconds::from_ns(timestamp.as_ns() / 1_000 * 1_000),
                    })
                ),
                (8, None),
                (10, None),
            ])
        );
        assert_eq!(
            stack
                .controller
                .request::<lazer_methods::GetFeedsData>(&lazer_read)
                .await?,
            before
        );
        let written = stack
            .controller
            .request::<oracle_updates::UpdateLazer>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                body: oracle_updates::UpdateLazer {
                    oracle_id: lazer_id,
                    feed_ids: lazer_read.feed_ids.clone(),
                },
            })
            .await?;
        assert_eq!(
            written.operation.status,
            templar_gateway_types::OperationStatus::Succeeded
        );
        assert_eq!(
            stack
                .controller
                .request::<lazer_methods::GetFeedsData>(&lazer_read)
                .await?,
            preview
        );
        println!("Lazer preview={preview:?}; write={written:?}");

        let redstone_read = redstone::ReadPriceData {
            oracle_id: redstone_id.clone(),
            feed_ids: vec!["BTC".into(), "ETH".into(), "BTC".into()],
        };
        assert_eq!(
            stack
                .controller
                .request::<redstone::ReadPriceData>(&redstone_read)
                .await?,
            vec![]
        );
        let lower = stack
            .context
            .near_client()
            .chain()
            .block(None)
            .await?
            .timestamp_ns;
        let preview = stack
            .controller
            .request::<oracle_updates::GetRedStoneUpdate>(&oracle_updates::GetRedStoneUpdate {
                oracle_id: redstone_id.clone(),
                feed_ids: redstone_read.feed_ids.clone(),
            })
            .await?;
        let upper = stack
            .context
            .near_client()
            .chain()
            .block(None)
            .await?
            .timestamp_ns;
        assert_eq!(
            preview
                .iter()
                .map(|entry| (&entry.feed_id, entry.data.price))
                .collect::<Vec<_>>(),
            redstone_read
                .feed_ids
                .iter()
                .zip([
                    U256::from(6_698_556_748_915_u64).into(),
                    U256::from(195_692_129_540_u64).into(),
                    U256::from(6_698_556_748_915_u64).into(),
                ])
                .collect::<Vec<_>>()
        );
        for entry in &preview {
            assert_eq!(entry.data.write_timestamp, preview[0].data.write_timestamp);
            assert_eq!(
                entry.data.package_timestamp,
                Nanoseconds::from_ms(1_770_985_144_000)
            );
            assert!((lower..=upper).contains(&entry.data.write_timestamp.as_ns()));
        }
        assert_eq!(
            stack
                .controller
                .request::<redstone::ReadPriceData>(&redstone_read)
                .await?,
            vec![]
        );
        let written = stack
            .controller
            .request::<oracle_updates::UpdateRedStone>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                body: oracle_updates::UpdateRedStone {
                    oracle_id: redstone_id,
                    feed_ids: vec!["ETH".into(), "BTC".into()],
                },
            })
            .await?;
        assert_eq!(
            written.operation.status,
            templar_gateway_types::OperationStatus::Succeeded
        );
        let stored = stack
            .controller
            .request::<redstone::ReadPriceData>(&redstone_read)
            .await?;
        let comparable = |entries: Vec<redstone::PriceDataEntry>| {
            entries
                .into_iter()
                .map(|entry| {
                    (
                        entry.feed_id,
                        entry.data.price,
                        entry.data.package_timestamp,
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(comparable(stored), comparable(preview.clone()));
        println!("RedStone preview={preview:?}; write={written:?}");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    stack.shutdown().await;
    outcome
}

#[tokio::test]
async fn oracle_provider_updates_reject_invalid_payloads() -> Result<()> {
    use fake_lazer_source::{FakeLazerError, TestRedStoneError};
    for row in 0..6 {
        let harness = SandboxHarness::start().await?;
        let (lazer_id, redstone_id) = deploy_fixture_adapters(&harness).await?;
        let timestamp = harness.chain_timestamp().await?;
        let valid = signed_lazer_fixture(7, timestamp, vec![fixture_feed(7, true)], false)?;
        let setup = harness
            .call_function_payable(
                &templar_gateway_types::ManagedAccountId(lazer_id.clone()),
                &lazer_id,
                "update_price_feeds",
                serde_json::json!({ "payload": near_sdk::json_types::Base64VecU8(valid.clone()) }),
                near_token::NearToken::from_near(1),
            )
            .await?;
        assert_eq!(
            setup.operation.status,
            templar_gateway_types::OperationStatus::Succeeded
        );
        let setup = harness.call_function_payable(
            &templar_gateway_types::ManagedAccountId(redstone_id.clone()), &redstone_id, "write_prices",
            serde_json::json!({ "feed_ids": ["ETH", "BTC"], "payload": near_sdk::json_types::Base64VecU8(REDSTONE_VALID_PAYLOAD.to_vec()) }),
            near_token::NearToken::from_yoctonear(0),
        ).await?;
        assert_eq!(
            setup.operation.status,
            templar_gateway_types::OperationStatus::Succeeded
        );
        let lazer_source = match row {
            0 => FakeLazerSource::with_payload(signed_lazer_fixture(
                9,
                timestamp,
                vec![fixture_feed(7, true)],
                false,
            )?),
            1 => FakeLazerSource::with_payload(signed_lazer_fixture(
                7,
                timestamp,
                vec![fixture_feed(7, true)],
                true,
            )?),
            4 => FakeLazerSource::failing(FakeLazerError::CacheMiss),
            _ => FakeLazerSource::with_payload(valid),
        };
        let redstone_source = TestRedStoneSource::Fixture(match row {
            2 => Ok(vec![0]),
            5 => Err(TestRedStoneError("fixture unavailable".into())),
            _ => Ok(REDSTONE_VALID_PAYLOAD.to_vec()),
        });
        let stack = TestStack::start_with_sources(
            harness,
            Url::parse("http://127.0.0.1:1")?,
            lazer_source,
            redstone_source,
        )
        .await?;
        let outcome = async {
            let lazer_read = lazer_methods::GetFeedsData {
                oracle_id: lazer_id.clone(),
                feed_ids: vec![7],
            };
            let redstone_read = redstone::ReadPriceData {
                oracle_id: redstone_id.clone(),
                feed_ids: vec!["ETH".into(), "BTC".into()],
            };
            let before_lazer = stack
                .controller
                .request::<lazer_methods::GetFeedsData>(&lazer_read)
                .await?;
            let before_redstone = stack
                .controller
                .request::<redstone::ReadPriceData>(&redstone_read)
                .await?;
            if matches!(row, 0 | 1 | 4) {
                let error = stack
                    .controller
                    .request::<oracle_updates::GetLazerUpdate>(&oracle_updates::GetLazerUpdate {
                        oracle_id: lazer_id.clone(),
                        feed_ids: vec![7],
                    })
                    .await
                    .expect_err(
                        "invalid/unavailable provider data must not fall back to stored Lazer",
                    );
                let expected = match row {
                    0 => "signer is not trusted or has expired",
                    1 => "ed25519 signature verification failed",
                    4 => "Pyth Lazer cache miss",
                    _ => unreachable!(),
                };
                assert!(error.to_string().contains(expected), "{error}");
                println!("Lazer failure row {row}: {error}");
            } else {
                let error = stack
                    .controller
                    .request::<oracle_updates::GetRedStoneUpdate>(
                        &oracle_updates::GetRedStoneUpdate {
                            oracle_id: redstone_id.clone(),
                            feed_ids: vec![if row == 3 { "MISSING" } else { "ETH" }.into()],
                        },
                    )
                    .await
                    .expect_err(
                        "invalid/unavailable provider data must not fall back to stored RedStone",
                    );
                let expected = match row {
                    2 => "buffer overflow",
                    3 => "missing requested feed MISSING",
                    5 => "fixture unavailable",
                    _ => unreachable!(),
                };
                assert!(error.to_string().contains(expected), "{error}");
                println!("RedStone failure row {row}: {error}");
            }
            assert_eq!(
                stack
                    .controller
                    .request::<lazer_methods::GetFeedsData>(&lazer_read)
                    .await?,
                before_lazer
            );
            assert_eq!(
                stack
                    .controller
                    .request::<redstone::ReadPriceData>(&redstone_read)
                    .await?,
                before_redstone
            );
            assert_eq!(
                stack
                    .controller
                    .request::<oracle_updates::GetLazerUpdate>(&oracle_updates::GetLazerUpdate {
                        oracle_id: lazer_id,
                        feed_ids: vec![]
                    },)
                    .await?,
                HashMap::new()
            );
            assert_eq!(
                stack
                    .controller
                    .request::<oracle_updates::GetRedStoneUpdate>(
                        &oracle_updates::GetRedStoneUpdate {
                            oracle_id: redstone_id,
                            feed_ids: vec![]
                        },
                    )
                    .await?,
                vec![]
            );
            Ok::<_, anyhow::Error>(())
        }
        .await;
        stack.shutdown().await;
        outcome?;
    }
    Ok(())
}
