use async_trait::async_trait;
use near_account_id::AccountId;
use templar_common::{
    asset::{AssetClass, FungibleAsset},
    market::{DepositMsg, LiquidateMsg, MarketConfiguration, RepayAccountMsg},
};
use templar_gateway_core::{
    client::{
        market::{
            AccountIdArg, AccumulateStaticYieldArgs, AmountArg, ApplyInterestArgs, BatchLimitArg,
            GetBorrowPositionPendingInterestArgs, GetBorrowStatusArgs,
            GetSupplyPositionPendingYieldArgs, HarvestYieldArgs, StaticYieldRecord,
        },
        storage::{StorageBalanceBoundsView, StorageBalanceOfArgs, StorageDepositArgs},
        ContractWriteOptions,
    },
    DispatchRead, GatewayError, GatewayResult, HasNearClient, OperationPlan, PlanWrite,
    PlannedTransaction,
};
use templar_gateway_methods_spec::{market, registry::Deploy};
use templar_gateway_types::ManagedAccountId;

use crate::registry_impl::plan_deploy_from_registry;
use crate::Dispatch;

#[derive(serde::Serialize)]
struct MarketInitArgs {
    configuration: MarketConfiguration,
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<market::GetConfiguration, C> for Dispatch {
    async fn dispatch(
        request: market::GetConfiguration,
        ctx: C,
    ) -> GatewayResult<MarketConfiguration> {
        ctx.near_client()
            .market(request.market_id)
            .get_configuration(())
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<market::GetCurrentSnapshot, C> for Dispatch {
    async fn dispatch(
        request: market::GetCurrentSnapshot,
        ctx: C,
    ) -> GatewayResult<templar_common::snapshot::Snapshot> {
        ctx.near_client()
            .market(request.market_id)
            .get_current_snapshot(())
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<market::GetFinalizedSnapshotsLen, C> for Dispatch {
    async fn dispatch(request: market::GetFinalizedSnapshotsLen, ctx: C) -> GatewayResult<u32> {
        ctx.near_client()
            .market(request.market_id)
            .get_finalized_snapshots_len(())
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<market::ListFinalizedSnapshots, C> for Dispatch {
    async fn dispatch(
        request: market::ListFinalizedSnapshots,
        ctx: C,
    ) -> GatewayResult<market::ListFinalizedSnapshotsResult> {
        ctx.near_client()
            .market(request.market_id)
            .list_finalized_snapshots(request.args)
            .await
            .map(|snapshots| market::ListFinalizedSnapshotsResult { snapshots })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<market::GetBorrowAssetMetrics, C> for Dispatch {
    async fn dispatch(
        request: market::GetBorrowAssetMetrics,
        ctx: C,
    ) -> GatewayResult<templar_common::market::BorrowAssetMetrics> {
        ctx.near_client()
            .market(request.market_id)
            .get_borrow_asset_metrics(())
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<market::ListBorrowPositions, C> for Dispatch {
    async fn dispatch(
        request: market::ListBorrowPositions,
        ctx: C,
    ) -> GatewayResult<market::ListBorrowPositionsResult> {
        ctx.near_client()
            .market(request.market_id)
            .list_borrow_positions(request.args)
            .await
            .map(|positions| market::ListBorrowPositionsResult { positions })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<market::GetBorrowPosition, C> for Dispatch {
    async fn dispatch(
        request: market::GetBorrowPosition,
        ctx: C,
    ) -> GatewayResult<market::GetBorrowPositionResult> {
        ctx.near_client()
            .market(request.market_id)
            .get_borrow_position(AccountIdArg {
                account_id: request.account_id,
            })
            .await
            .map(|position| market::GetBorrowPositionResult { position })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<market::GetBorrowPositionPendingInterest, C> for Dispatch {
    async fn dispatch(
        request: market::GetBorrowPositionPendingInterest,
        ctx: C,
    ) -> GatewayResult<market::GetBorrowPositionPendingInterestResult> {
        let params = request;
        ctx.near_client()
            .market(params.market_id)
            .get_borrow_position_pending_interest(GetBorrowPositionPendingInterestArgs {
                account_id: params.account_id,
                snapshot_limit: params.snapshot_limit,
            })
            .await
            .map(|amount| market::GetBorrowPositionPendingInterestResult { amount })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<market::GetBorrowStatus, C> for Dispatch {
    async fn dispatch(
        request: market::GetBorrowStatus,
        ctx: C,
    ) -> GatewayResult<market::GetBorrowStatusResult> {
        let params = request;
        ctx.near_client()
            .market(params.market_id)
            .get_borrow_status(GetBorrowStatusArgs {
                account_id: params.account_id,
                oracle_response: params.oracle_response,
            })
            .await
            .map(|status| market::GetBorrowStatusResult { status })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<market::ListSupplyPositions, C> for Dispatch {
    async fn dispatch(
        request: market::ListSupplyPositions,
        ctx: C,
    ) -> GatewayResult<market::ListSupplyPositionsResult> {
        ctx.near_client()
            .market(request.market_id)
            .list_supply_positions(request.args)
            .await
            .map(|positions| market::ListSupplyPositionsResult { positions })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<market::GetSupplyPosition, C> for Dispatch {
    async fn dispatch(
        request: market::GetSupplyPosition,
        ctx: C,
    ) -> GatewayResult<market::GetSupplyPositionResult> {
        ctx.near_client()
            .market(request.market_id)
            .get_supply_position(AccountIdArg {
                account_id: request.account_id,
            })
            .await
            .map(|position| market::GetSupplyPositionResult { position })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<market::GetSupplyPositionPendingYield, C> for Dispatch {
    async fn dispatch(
        request: market::GetSupplyPositionPendingYield,
        ctx: C,
    ) -> GatewayResult<market::GetSupplyPositionPendingYieldResult> {
        let params = request;
        ctx.near_client()
            .market(params.market_id)
            .get_supply_position_pending_yield(GetSupplyPositionPendingYieldArgs {
                account_id: params.account_id,
                snapshot_limit: params.snapshot_limit,
            })
            .await
            .map(|amount| market::GetSupplyPositionPendingYieldResult { amount })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<market::GetSupplyWithdrawalRequestStatus, C> for Dispatch {
    async fn dispatch(
        request: market::GetSupplyWithdrawalRequestStatus,
        ctx: C,
    ) -> GatewayResult<market::GetSupplyWithdrawalRequestStatusResult> {
        ctx.near_client()
            .market(request.market_id)
            .get_supply_withdrawal_request_status(AccountIdArg {
                account_id: request.account_id,
            })
            .await
            .map(|status| market::GetSupplyWithdrawalRequestStatusResult { status })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<market::GetSupplyWithdrawalQueueStatus, C> for Dispatch {
    async fn dispatch(
        request: market::GetSupplyWithdrawalQueueStatus,
        ctx: C,
    ) -> GatewayResult<templar_common::withdrawal_queue::WithdrawalQueueStatus> {
        ctx.near_client()
            .market(request.market_id)
            .get_supply_withdrawal_queue_status(())
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<market::GetLastYieldRate, C> for Dispatch {
    async fn dispatch(
        request: market::GetLastYieldRate,
        ctx: C,
    ) -> GatewayResult<templar_common::Decimal> {
        ctx.near_client()
            .market(request.market_id)
            .get_last_yield_rate(())
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<market::GetStaticYield, C> for Dispatch {
    async fn dispatch(
        request: market::GetStaticYield,
        ctx: C,
    ) -> GatewayResult<market::GetStaticYieldResult> {
        ctx.near_client()
            .market(request.market_id)
            .get_static_yield(AccountIdArg {
                account_id: request.account_id,
            })
            .await
            .map(|record| market::GetStaticYieldResult {
                record: record.map(|record| match record {
                    StaticYieldRecord::Split { borrow_asset, .. } => market::StaticYield::Legacy {
                        borrow_asset_total: borrow_asset,
                    },
                    StaticYieldRecord::Accumulator(accumulator) => {
                        market::StaticYield::Accumulator { accumulator }
                    }
                }),
            })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<market::Borrow, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<market::Borrow>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        ctx.near_client()
            .market(request.body.market_id)
            .borrow(
                ContractWriteOptions::new(request.signer_account_id).tgas(300),
                AmountArg {
                    amount: request.body.amount,
                },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<market::Create, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<market::Create>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        let market_account_id = body
            .registry_id
            .sub_account(&body.name)
            .map_err(|error| GatewayError::NearQuery(error.to_string()))?;
        let configuration = body.configuration;
        let mut steps = plan_deploy_from_registry(
            &ctx,
            request.signer_account_id.clone(),
            Deploy {
                registry_id: body.registry_id,
                name: body.name,
                version_key: body.version_key,
                init_args: serde_json::to_vec(&MarketInitArgs {
                    configuration: configuration.clone(),
                })?
                .into(),
                full_access_keys: body.full_access_keys,
                deposit: body.deposit,
            },
        )
        .await?
        .steps;

        for asset_id in [
            configuration.borrow_asset.into_nep141(),
            configuration.collateral_asset.into_nep141(),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(tx_result) = ensure_storage_registration(
                &ctx,
                request.signer_account_id.clone(),
                asset_id,
                market_account_id.clone(),
            )
            .await?
            {
                steps.push(tx_result);
            }
        }

        Ok(OperationPlan { steps })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<market::Supply, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<market::Supply>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        let configuration = ctx
            .near_client()
            .market(body.market_id.clone())
            .cached_get_configuration()
            .await?;
        let mut steps = Vec::new();

        if let Some(asset_id) = configuration.borrow_asset.clone().into_nep141() {
            if let Some(tx_result) = ensure_storage_registration(
                &ctx,
                request.signer_account_id.clone(),
                asset_id,
                body.market_id.clone(),
            )
            .await?
            {
                steps.push(tx_result);
            }
        }

        if let Some(tx_result) = ensure_storage_registration(
            &ctx,
            request.signer_account_id.clone(),
            body.market_id.clone(),
            request.signer_account_id.0.clone(),
        )
        .await?
        {
            steps.push(tx_result);
        }

        steps.push(transfer_call_asset(
            &ctx,
            request.signer_account_id,
            configuration.borrow_asset,
            body.market_id,
            body.amount,
            &DepositMsg::Supply,
        )?);

        Ok(OperationPlan { steps })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<market::Collateralize, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<market::Collateralize>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        let configuration = ctx
            .near_client()
            .market(body.market_id.clone())
            .cached_get_configuration()
            .await?;
        let mut steps = Vec::new();

        if let Some(asset_id) = configuration.collateral_asset.clone().into_nep141() {
            if let Some(tx_result) = ensure_storage_registration(
                &ctx,
                request.signer_account_id.clone(),
                asset_id,
                body.market_id.clone(),
            )
            .await?
            {
                steps.push(tx_result);
            }
        }

        if let Some(tx_result) = ensure_storage_registration(
            &ctx,
            request.signer_account_id.clone(),
            body.market_id.clone(),
            request.signer_account_id.0.clone(),
        )
        .await?
        {
            steps.push(tx_result);
        }

        steps.push(transfer_call_asset(
            &ctx,
            request.signer_account_id,
            configuration.collateral_asset,
            body.market_id,
            body.amount,
            &DepositMsg::Collateralize,
        )?);

        Ok(OperationPlan { steps })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<market::WithdrawCollateral, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<market::WithdrawCollateral>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        ctx.near_client()
            .market(request.body.market_id)
            .withdraw_collateral(
                ContractWriteOptions::new(request.signer_account_id).tgas(300),
                AmountArg {
                    amount: request.body.amount,
                },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<market::ApplyInterest, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<market::ApplyInterest>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .market(body.market_id)
            .apply_interest(
                ContractWriteOptions::new(request.signer_account_id).tgas(300),
                ApplyInterestArgs {
                    account_id: body.account_id,
                    snapshot_limit: body.snapshot_limit,
                },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<market::Repay, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<market::Repay>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        let configuration = ctx
            .near_client()
            .market(body.market_id.clone())
            .cached_get_configuration()
            .await?;
        let deposit_msg = body.account_id.map_or(DepositMsg::Repay, |account_id| {
            DepositMsg::RepayAccount(RepayAccountMsg { account_id })
        });
        let mut steps = Vec::new();

        if let Some(asset_id) = configuration.borrow_asset.clone().into_nep141() {
            if let Some(tx_result) = ensure_storage_registration(
                &ctx,
                request.signer_account_id.clone(),
                asset_id,
                body.market_id.clone(),
            )
            .await?
            {
                steps.push(tx_result);
            }
        }

        steps.push(transfer_call_asset(
            &ctx,
            request.signer_account_id,
            configuration.borrow_asset,
            body.market_id,
            body.amount,
            &deposit_msg,
        )?);

        Ok(OperationPlan { steps })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<market::CreateSupplyWithdrawalRequest, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<market::CreateSupplyWithdrawalRequest>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        ctx.near_client()
            .market(request.body.market_id)
            .create_supply_withdrawal_request(
                ContractWriteOptions::new(request.signer_account_id).tgas(300),
                AmountArg {
                    amount: request.body.amount,
                },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<market::CancelSupplyWithdrawalRequest, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<market::CancelSupplyWithdrawalRequest>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        ctx.near_client()
            .market(request.body.market_id)
            .cancel_supply_withdrawal_request(
                ContractWriteOptions::new(request.signer_account_id).tgas(300),
                (),
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<market::ExecuteNextSupplyWithdrawalRequest, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<
            market::ExecuteNextSupplyWithdrawalRequest,
        >,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        ctx.near_client()
            .market(request.body.market_id)
            .execute_next_supply_withdrawal_request(
                ContractWriteOptions::new(request.signer_account_id).tgas(300),
                BatchLimitArg {
                    batch_limit: request.body.batch_limit,
                },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<market::WithdrawSupply, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<market::WithdrawSupply>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        let configuration = ctx
            .near_client()
            .market(body.market_id.clone())
            .cached_get_configuration()
            .await?;
        let queue_status = ctx
            .near_client()
            .market(body.market_id.clone())
            .get_supply_withdrawal_queue_status(())
            .await?;
        let mut steps = Vec::new();

        if let Some(asset_id) = configuration.borrow_asset.clone().into_nep141() {
            if let Some(tx_result) = ensure_storage_registration(
                &ctx,
                request.signer_account_id.clone(),
                asset_id,
                request.signer_account_id.0.clone(),
            )
            .await?
            {
                steps.push(tx_result);
            }
        }

        steps.push(
            ctx.near_client()
                .market(body.market_id.clone())
                .create_supply_withdrawal_request(
                    ContractWriteOptions::new(request.signer_account_id.clone()).tgas(300),
                    AmountArg {
                        amount: body.amount,
                    },
                )?,
        );

        if queue_status.length == 0 {
            steps.push(
                ctx.near_client()
                    .market(body.market_id)
                    .execute_next_supply_withdrawal_request(
                        ContractWriteOptions::new(request.signer_account_id).tgas(300),
                        BatchLimitArg {
                            batch_limit: body.batch_limit,
                        },
                    )?,
            );
        }

        Ok(OperationPlan { steps })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<market::Liquidate, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<market::Liquidate>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        let configuration = ctx
            .near_client()
            .market(body.market_id.clone())
            .cached_get_configuration()
            .await?;
        let mut steps = Vec::new();

        if let Some(asset_id) = configuration.borrow_asset.clone().into_nep141() {
            if let Some(tx_result) = ensure_storage_registration(
                &ctx,
                request.signer_account_id.clone(),
                asset_id,
                body.market_id.clone(),
            )
            .await?
            {
                steps.push(tx_result);
            }
        }

        if let Some(asset_id) = configuration.collateral_asset.clone().into_nep141() {
            if let Some(tx_result) = ensure_storage_registration(
                &ctx,
                request.signer_account_id.clone(),
                asset_id,
                request.signer_account_id.0.clone(),
            )
            .await?
            {
                steps.push(tx_result);
            }
        }

        steps.push(transfer_call_asset(
            &ctx,
            request.signer_account_id,
            configuration.borrow_asset,
            body.market_id,
            body.liquidation_amount,
            &DepositMsg::Liquidate(LiquidateMsg {
                account_id: body.account_id,
                amount: body.collateral_amount,
            }),
        )?);

        Ok(OperationPlan { steps })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<market::HarvestYield, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<market::HarvestYield>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .market(body.market_id)
            .harvest_yield(
                ContractWriteOptions::new(request.signer_account_id).tgas(300),
                HarvestYieldArgs {
                    account_id: body.account_id,
                    mode: body.mode,
                },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<market::AccumulateStaticYield, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<market::AccumulateStaticYield>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .market(body.market_id)
            .accumulate_static_yield(
                ContractWriteOptions::new(request.signer_account_id).tgas(300),
                AccumulateStaticYieldArgs {
                    account_id: body.account_id,
                    snapshot_limit: body.snapshot_limit,
                },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<market::WithdrawStaticYield, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<market::WithdrawStaticYield>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        ctx.near_client()
            .market(request.body.market_id)
            .withdraw_static_yield(
                ContractWriteOptions::new(request.signer_account_id).tgas(300),
                AmountArg {
                    amount: request.body.amount,
                },
            )
            .map(OperationPlan::from)
    }
}

async fn ensure_storage_registration<C: HasNearClient>(
    ctx: &C,
    signer_account_id: ManagedAccountId,
    contract_id: AccountId,
    account_id: AccountId,
) -> GatewayResult<Option<PlannedTransaction>> {
    let Some(bounds) = storage_balance_bounds_if_supported(ctx, contract_id.clone()).await? else {
        return Ok(None);
    };

    let balance = ctx
        .near_client()
        .storage(contract_id.clone())
        .storage_balance_of(StorageBalanceOfArgs {
            account_id: account_id.clone(),
        })
        .await?;

    if balance.is_some() {
        return Ok(None);
    }

    let tx_result = ctx.near_client().storage(contract_id).storage_deposit(
        ContractWriteOptions::new(signer_account_id)
            .tgas(100)
            .deposit(templar_gateway_types::NearToken::from_yoctonear(
                bounds.min.as_yoctonear(),
            )),
        StorageDepositArgs {
            account_id: Some(account_id),
            registration_only: true,
        },
    )?;
    Ok(Some(tx_result))
}

async fn storage_balance_bounds_if_supported<C: HasNearClient>(
    ctx: &C,
    contract_id: AccountId,
) -> GatewayResult<Option<StorageBalanceBoundsView>> {
    ctx.near_client()
        .storage(contract_id)
        .cached_storage_balance_bounds_if_supported()
        .await
}

fn transfer_call_asset<C: HasNearClient, T: AssetClass>(
    ctx: &C,
    signer_account_id: ManagedAccountId,
    asset: FungibleAsset<T>,
    receiver_id: AccountId,
    amount: impl Into<u128>,
    msg: &DepositMsg,
) -> GatewayResult<PlannedTransaction> {
    ctx.near_client().token(asset).transfer_call(
        ContractWriteOptions::new(signer_account_id)
            .tgas(300)
            .one_yocto(),
        receiver_id,
        amount,
        serde_json::to_string(msg)?,
    )
}
