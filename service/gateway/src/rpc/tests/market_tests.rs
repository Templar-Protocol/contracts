use super::*;

async fn call_function(
    stack: &TestStack,
    signer_account_id: templar_gateway_types::ManagedAccountId,
    receiver_id: near_account_id::AccountId,
    method_name: &str,
    args: serde_json::Value,
    gas_tgas: u64,
    deposit_yocto: u128,
) -> Result<templar_gateway_types::common::WriteOperationResult> {
    stack
        .controller
        .request::<tx::FunctionCall>(&WriteRequest {
            signer_account_id,
            idempotency_key: None,
            body: tx::FunctionCallBody {
                receiver_id,
                method_name: ContractMethodName(method_name.to_owned()),
                args: ContractArgs::Json(args),
                gas: NearGas::from_tgas(gas_tgas),
                deposit: NearToken::from_yoctonear(deposit_yocto),
            },
        })
        .await
}

async fn ensure_registered(
    stack: &TestStack,
    signer_account_id: templar_gateway_types::ManagedAccountId,
    contract_id: near_account_id::AccountId,
    account_id: near_account_id::AccountId,
) -> Result<()> {
    let _ = stack
        .controller
        .request::<storage::EnsureDeposit>(&WriteRequest {
            signer_account_id,
            idempotency_key: None,
            body: storage::EnsureDepositBody {
                contract_id,
                account_id,
                mode: storage::EnsureDepositMode::Registered,
            },
        })
        .await?;
    Ok(())
}

async fn ft_balance(
    stack: &TestStack,
    contract_id: near_account_id::AccountId,
    account_id: near_account_id::AccountId,
) -> Result<u128> {
    let value = view_contract_json(
        stack,
        contract_id,
        "ft_balance_of",
        serde_json::json!({ "account_id": account_id }),
    )
    .await?;
    Ok(value
        .as_str()
        .expect("ft_balance_of should serialize as string")
        .parse()?)
}

#[tokio::test]
async fn market_composed_operations_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let (market_id, configuration) = stack.harness.deploy_market().await?;
    let borrow_asset_id = configuration
        .borrow_asset
        .clone()
        .into_nep141()
        .expect("sandbox market should use NEP-141 borrow asset");
    let collateral_asset_id = configuration
        .collateral_asset
        .clone()
        .into_nep141()
        .expect("sandbox market should use NEP-141 collateral asset");

    stack
        .harness
        .set_mock_oracle_pyth_price(
            configuration.price_oracle_configuration.account_id.clone(),
            configuration
                .price_oracle_configuration
                .borrow_asset_price_id,
            Some(test_utils::to_price(1.0)),
        )
        .await?;
    stack
        .harness
        .set_mock_oracle_pyth_price(
            configuration.price_oracle_configuration.account_id.clone(),
            configuration
                .price_oracle_configuration
                .collateral_asset_price_id,
            Some(test_utils::to_price(2.0)),
        )
        .await?;

    for signer_account_id in [
        stack.harness.gateway_signer_account_id.clone(),
        stack.harness.cleanup_signer_account_id.clone(),
    ] {
        ensure_registered(
            &stack,
            signer_account_id.clone(),
            borrow_asset_id.clone(),
            signer_account_id.0.clone(),
        )
        .await?;
        ensure_registered(
            &stack,
            signer_account_id.clone(),
            collateral_asset_id.clone(),
            signer_account_id.0.clone(),
        )
        .await?;
        ensure_registered(
            &stack,
            signer_account_id.clone(),
            market_id.clone(),
            signer_account_id.0.clone(),
        )
        .await?;
    }
    ensure_registered(
        &stack,
        stack.harness.gateway_signer_account_id.clone(),
        borrow_asset_id.clone(),
        market_id.clone(),
    )
    .await?;
    ensure_registered(
        &stack,
        stack.harness.gateway_signer_account_id.clone(),
        collateral_asset_id.clone(),
        market_id.clone(),
    )
    .await?;

    let _ = call_function(
        &stack,
        stack.harness.gateway_signer_account_id.clone(),
        borrow_asset_id.clone(),
        "mint",
        serde_json::json!({ "amount": "200000" }),
        100,
        0,
    )
    .await?;
    let _ = call_function(
        &stack,
        stack.harness.gateway_signer_account_id.clone(),
        collateral_asset_id.clone(),
        "mint",
        serde_json::json!({ "amount": "500000" }),
        100,
        0,
    )
    .await?;
    let _ = call_function(
        &stack,
        stack.harness.cleanup_signer_account_id.clone(),
        borrow_asset_id.clone(),
        "mint",
        serde_json::json!({ "amount": "200000" }),
        100,
        0,
    )
    .await?;
    let _ = call_function(
        &stack,
        stack.harness.cleanup_signer_account_id.clone(),
        collateral_asset_id.clone(),
        "mint",
        serde_json::json!({ "amount": "500000" }),
        100,
        0,
    )
    .await?;

    let supply = stack
        .controller
        .request::<market::Supply>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: market::SupplyBody {
                market_id: market_id.clone(),
                amount: 100_000u128.into(),
            },
        })
        .await?;
    assert_eq!(
        supply.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );
    assert_eq!(supply.operation.steps.len(), 1);

    let mut supply_is_active = false;
    for _ in 0..10 {
        let _ = stack
            .controller
            .request::<market::HarvestYield>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                body: market::HarvestYieldBody {
                    market_id: market_id.clone(),
                    account_id: None,
                    mode: None,
                },
            })
            .await?;
        let position = stack
            .controller
            .request::<market::GetSupplyPosition>(&ReadRequest {
                params: market::GetSupplyPositionParams {
                    market_id: market_id.clone(),
                    account_id: stack.harness.gateway_signer_account_id.0.clone(),
                },
            })
            .await?;
        if position
            .position
            .as_ref()
            .is_some_and(|position| position.get_deposit().incoming.is_empty())
        {
            supply_is_active = true;
            break;
        }
    }
    assert!(supply_is_active);

    let _ = call_function(
        &stack,
        stack.harness.cleanup_signer_account_id.clone(),
        collateral_asset_id.clone(),
        "ft_transfer_call",
        serde_json::json!({
            "receiver_id": market_id.clone(),
            "amount": "200000",
            "msg": serde_json::to_string(&DepositMsg::Collateralize)?,
        }),
        300,
        1,
    )
    .await?;
    let _ = stack
        .controller
        .request::<market::Borrow>(&WriteRequest {
            signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
            idempotency_key: None,
            body: market::BorrowBody {
                market_id: market_id.clone(),
                amount: 60_000u128.into(),
            },
        })
        .await?;

    let repay = stack
        .controller
        .request::<market::Repay>(&WriteRequest {
            signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
            idempotency_key: None,
            body: market::RepayBody {
                market_id: market_id.clone(),
                amount: 10_000u128.into(),
                account_id: None,
            },
        })
        .await?;
    assert_eq!(
        repay.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );

    stack
        .harness
        .set_mock_oracle_pyth_price(
            configuration.price_oracle_configuration.account_id.clone(),
            configuration
                .price_oracle_configuration
                .collateral_asset_price_id,
            Some(test_utils::to_price(0.05)),
        )
        .await?;

    let borrow_position_before_liquidation = stack
        .controller
        .request::<market::GetBorrowPosition>(&ReadRequest {
            params: market::GetBorrowPositionParams {
                market_id: market_id.clone(),
                account_id: stack.harness.cleanup_signer_account_id.0.clone(),
            },
        })
        .await?
        .position
        .expect("borrower should have a borrow position before liquidation");
    let liability_before_liquidation =
        borrow_position_before_liquidation.get_total_borrow_asset_liability();
    let liquidation_oracle_response = HashMap::from([
        (
            configuration
                .price_oracle_configuration
                .borrow_asset_price_id,
            Some(test_utils::to_price(1.0)),
        ),
        (
            configuration
                .price_oracle_configuration
                .collateral_asset_price_id,
            Some(test_utils::to_price(0.05)),
        ),
    ]);
    let liquidation_price_pair = configuration
        .price_oracle_configuration
        .create_price_pair(&liquidation_oracle_response)?;
    let liquidatable_collateral = borrow_position_before_liquidation.liquidatable_collateral(
        &liquidation_price_pair,
        configuration.borrow_mcr_maintenance,
        configuration.liquidation_maximum_spread,
    );
    let liquidation_amount = configuration
        .minimum_acceptable_liquidation_amount(liquidatable_collateral, &liquidation_price_pair)
        .expect("liquidation amount should be derivable");
    let liquidator_borrow_balance_before = ft_balance(
        &stack,
        borrow_asset_id.clone(),
        stack.harness.gateway_signer_account_id.0.clone(),
    )
    .await?;
    let liquidate = stack
        .controller
        .request::<market::Liquidate>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: market::LiquidateBody {
                market_id: market_id.clone(),
                account_id: stack.harness.cleanup_signer_account_id.0.clone(),
                liquidation_amount,
                collateral_amount: Some(liquidatable_collateral),
            },
        })
        .await?;
    assert_eq!(
        liquidate.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );
    let liquidator_borrow_balance_after = ft_balance(
        &stack,
        borrow_asset_id.clone(),
        stack.harness.gateway_signer_account_id.0.clone(),
    )
    .await?;
    assert!(liquidator_borrow_balance_after < liquidator_borrow_balance_before);

    let withdraw_supply = stack
        .controller
        .request::<market::WithdrawSupply>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: market::WithdrawSupplyBody {
                market_id: market_id.clone(),
                amount: 20_000u128.into(),
                batch_limit: None,
            },
        })
        .await?;
    assert_eq!(
        withdraw_supply.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );
    assert_eq!(withdraw_supply.operation.steps.len(), 2);

    let supply_request = stack
        .controller
        .request::<market::GetSupplyWithdrawalRequestStatus>(&ReadRequest {
            params: market::GetSupplyWithdrawalRequestStatusParams {
                market_id: market_id.clone(),
                account_id: stack.harness.gateway_signer_account_id.0.clone(),
            },
        })
        .await?;
    assert!(supply_request.status.is_none());

    let borrow_position = stack
        .controller
        .request::<market::GetBorrowPosition>(&ReadRequest {
            params: market::GetBorrowPositionParams {
                market_id,
                account_id: stack.harness.cleanup_signer_account_id.0.clone(),
            },
        })
        .await?;
    let borrow_position = borrow_position
        .position
        .expect("borrower should still have a borrow position after partial liquidation");
    let liability_after_liquidation = borrow_position.get_total_borrow_asset_liability();
    assert!(
        liability_after_liquidation <= liability_before_liquidation,
        "liquidation should not increase liability"
    );

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn market_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let (market_id, configuration) = stack.harness.deploy_market().await?;

    let returned_configuration = stack
        .controller
        .request::<market::GetConfiguration>(&ReadRequest {
            params: market::GetConfigurationParams {
                market_id: market_id.clone(),
            },
        })
        .await?;

    let borrow_positions = stack
        .controller
        .request::<market::ListBorrowPositions>(&ReadRequest {
            params: market::ListBorrowPositionsParams {
                market_id,
                args: templar_gateway_types::common::Pagination::default(),
            },
        })
        .await?;

    assert_eq!(returned_configuration, configuration);
    assert!(borrow_positions.positions.is_empty());

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn market_create_endpoint_deploys_from_registry_and_registers_tokens() -> Result<()> {
    let stack = TestStack::start().await?;
    let (_existing_market_id, configuration) = stack.harness.deploy_market().await?;
    let registry_id = stack.harness.deploy_registry().await?;

    let _ = stack
        .controller
        .request::<registry::AddVersion>(&WriteRequest {
            signer_account_id: stack.harness.registry_signer_account_id.clone(),
            idempotency_key: None,
            body: registry::AddVersionBody {
                registry_id: registry_id.clone(),
                version_key: "market@1.0.0".to_owned(),
                deploy_mode: templar_common::registry::DeployMode::Normal,
                code: Base64Bytes(test_utils::MarketController::wasm().await.to_vec()),
                deposit: NearToken::from_yoctonear(1),
            },
        })
        .await?;

    let create = stack
        .controller
        .request::<market::Create>(&WriteRequest {
            signer_account_id: stack.harness.registry_signer_account_id.clone(),
            idempotency_key: None,
            body: market::CreateBody {
                registry_id: registry_id.clone(),
                name: "market-created".to_owned(),
                version_key: "market@1.0.0".to_owned(),
                configuration: configuration.clone(),
                full_access_keys: None,
                deposit: NearToken::from_near(20),
            },
        })
        .await?;

    assert_eq!(
        create.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );
    assert_eq!(create.operation.steps.len(), 3);

    let market_account_id = registry_id
        .sub_account("market-created")
        .expect("created market id should be valid");
    let market_id = market_account_id.clone();

    let returned_configuration = stack
        .controller
        .request::<market::GetConfiguration>(&ReadRequest {
            params: market::GetConfigurationParams {
                market_id: market_id.clone(),
            },
        })
        .await?;
    assert_eq!(returned_configuration, configuration);

    for contract_id in [
        configuration
            .borrow_asset
            .clone()
            .into_nep141()
            .expect("borrow asset should be NEP-141"),
        configuration
            .collateral_asset
            .clone()
            .into_nep141()
            .expect("collateral asset should be NEP-141"),
    ] {
        let storage_balance = stack
            .controller
            .request::<storage::GetBalanceOf>(&ReadRequest {
                params: storage::GetBalanceOfParams {
                    contract_id,
                    account_id: market_account_id.clone(),
                },
            })
            .await?;
        assert!(storage_balance.balance.is_some());
    }

    let deployment = stack
        .controller
        .request::<registry::GetDeployment>(&ReadRequest {
            params: registry::GetDeploymentParams {
                registry_id,
                account_id: market_account_id,
            },
        })
        .await?;
    assert!(deployment.deployment.is_some());

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn market_extended_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let (market_id, _configuration) = stack.harness.deploy_market().await?;

    let _ = stack
        .controller
        .request::<market::GetCurrentSnapshot>(&ReadRequest {
            params: market::GetCurrentSnapshotParams {
                market_id: market_id.clone(),
            },
        })
        .await?;
    let finalized_len = stack
        .controller
        .request::<market::GetFinalizedSnapshotsLen>(&ReadRequest {
            params: market::GetFinalizedSnapshotsLenParams {
                market_id: market_id.clone(),
            },
        })
        .await?;
    let finalized = stack
        .controller
        .request::<market::ListFinalizedSnapshots>(&ReadRequest {
            params: market::ListFinalizedSnapshotsParams {
                market_id: market_id.clone(),
                args: templar_gateway_types::common::Pagination::default(),
            },
        })
        .await?;
    let metrics = stack
        .controller
        .request::<market::GetBorrowAssetMetrics>(&ReadRequest {
            params: market::GetBorrowAssetMetricsParams {
                market_id: market_id.clone(),
            },
        })
        .await?;
    let empty_borrow_position = stack
        .controller
        .request::<market::GetBorrowPosition>(&ReadRequest {
            params: market::GetBorrowPositionParams {
                market_id: market_id.clone(),
                account_id: stack.harness.gateway_signer_account_id.0.clone(),
            },
        })
        .await?;
    let empty_borrow_interest = stack
        .controller
        .request::<market::GetBorrowPositionPendingInterest>(&ReadRequest {
            params: market::GetBorrowPositionPendingInterestParams {
                market_id: market_id.clone(),
                account_id: stack.harness.gateway_signer_account_id.0.clone(),
                snapshot_limit: Some(1),
            },
        })
        .await?;
    let empty_borrow_status = stack
        .controller
        .request::<market::GetBorrowStatus>(&ReadRequest {
            params: market::GetBorrowStatusParams {
                market_id: market_id.clone(),
                account_id: stack.harness.gateway_signer_account_id.0.clone(),
                oracle_response: templar_common::oracle::pyth::OracleResponse::new(),
            },
        })
        .await?;
    let supply_positions = stack
        .controller
        .request::<market::ListSupplyPositions>(&ReadRequest {
            params: market::ListSupplyPositionsParams {
                market_id: market_id.clone(),
                args: templar_gateway_types::common::Pagination::default(),
            },
        })
        .await?;
    let empty_supply_position = stack
        .controller
        .request::<market::GetSupplyPosition>(&ReadRequest {
            params: market::GetSupplyPositionParams {
                market_id: market_id.clone(),
                account_id: stack.harness.gateway_signer_account_id.0.clone(),
            },
        })
        .await?;
    let empty_supply_yield = stack
        .controller
        .request::<market::GetSupplyPositionPendingYield>(&ReadRequest {
            params: market::GetSupplyPositionPendingYieldParams {
                market_id: market_id.clone(),
                account_id: stack.harness.gateway_signer_account_id.0.clone(),
                snapshot_limit: Some(1),
            },
        })
        .await?;
    let empty_withdrawal_request = stack
        .controller
        .request::<market::GetSupplyWithdrawalRequestStatus>(&ReadRequest {
            params: market::GetSupplyWithdrawalRequestStatusParams {
                market_id: market_id.clone(),
                account_id: stack.harness.gateway_signer_account_id.0.clone(),
            },
        })
        .await?;
    let queue = stack
        .controller
        .request::<market::GetSupplyWithdrawalQueueStatus>(&ReadRequest {
            params: market::GetSupplyWithdrawalQueueStatusParams {
                market_id: market_id.clone(),
            },
        })
        .await?;
    let last_yield = stack
        .controller
        .request::<market::GetLastYieldRate>(&ReadRequest {
            params: market::GetLastYieldRateParams {
                market_id: market_id.clone(),
            },
        })
        .await?;
    let static_yield = stack
        .controller
        .request::<market::GetStaticYield>(&ReadRequest {
            params: market::GetStaticYieldParams {
                market_id: market_id.clone(),
                account_id: stack.harness.gateway_signer_account_id.0.clone(),
            },
        })
        .await?;
    let _ = stack
        .controller
        .request::<market::ApplyInterest>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: market::ApplyInterestBody {
                market_id: market_id.clone(),
                account_id: None,
                snapshot_limit: Some(1),
            },
        })
        .await?;
    let _ = stack
        .controller
        .request::<market::AccumulateStaticYield>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: market::AccumulateStaticYieldBody {
                market_id,
                account_id: Some(stack.harness.gateway_signer_account_id.0.clone()),
                snapshot_limit: Some(1),
            },
        })
        .await?;

    assert_eq!(finalized_len as usize, finalized.snapshots.len());
    assert!(empty_borrow_position.position.is_none());
    assert!(empty_borrow_interest.amount.is_none());
    assert!(empty_borrow_status.status.is_none());
    assert!(supply_positions.positions.is_empty());
    assert!(empty_supply_position.position.is_none());
    assert!(empty_supply_yield.amount.is_none());
    assert!(empty_withdrawal_request.status.is_none());
    assert_eq!(
        queue.depth,
        templar_common::asset::BorrowAssetAmount::zero()
    );
    assert_eq!(last_yield, templar_common::number::Decimal::ZERO);
    assert!(static_yield.accumulator.is_none());
    assert_eq!(
        metrics.borrowed,
        templar_common::asset::BorrowAssetAmount::zero()
    );

    stack.shutdown().await;
    Ok(())
}
