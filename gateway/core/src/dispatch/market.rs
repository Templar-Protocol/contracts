use futures::future::BoxFuture;
use near_account_id::AccountId;
use templar_common::{
    asset::{AssetClass, FungibleAsset},
    market::{DepositMsg, LiquidateMsg, MarketConfiguration, RepayAccountMsg},
};
use templar_gateway_types::{market, registry::DeployBody, ManagedAccountId};

use crate::{
    client::{
        market::{
            AccountIdArg, AccumulateStaticYieldArgs, AmountArg, ApplyInterestArgs, BatchLimitArg,
            GetBorrowPositionPendingInterestArgs, GetBorrowStatusArgs,
            GetSupplyPositionPendingYieldArgs, HarvestYieldArgs,
        },
        storage::{StorageBalanceBoundsView, StorageBalanceOfArgs, StorageDepositArgs},
        ContractWriteOptions,
    },
    dispatch::registry::plan_deploy_from_registry,
    operation::{OperationPlan, PlannedTransaction},
    GatewayResult, HasNearClient,
};
use crate::{DispatchRead, PlanWrite};

#[derive(serde::Serialize)]
struct MarketInitArgs {
    configuration: MarketConfiguration,
}

impl<C: HasNearClient> DispatchRead<C> for market::GetConfiguration {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.params.market_id)
                .get_configuration(())
                .await
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for market::GetCurrentSnapshot {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.params.market_id)
                .get_current_snapshot(())
                .await
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for market::GetFinalizedSnapshotsLen {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.params.market_id)
                .get_finalized_snapshots_len(())
                .await
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for market::ListFinalizedSnapshots {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.params.market_id)
                .list_finalized_snapshots(request.params.args)
                .await
                .map(|snapshots| market::ListFinalizedSnapshotsResult { snapshots })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for market::GetBorrowAssetMetrics {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.params.market_id)
                .get_borrow_asset_metrics(())
                .await
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for market::ListBorrowPositions {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.params.market_id)
                .list_borrow_positions(request.params.args)
                .await
                .map(|positions| market::ListBorrowPositionsResult { positions })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for market::GetBorrowPosition {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.params.market_id)
                .get_borrow_position(AccountIdArg {
                    account_id: request.params.account_id,
                })
                .await
                .map(|position| market::GetBorrowPositionResult { position })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for market::GetBorrowPositionPendingInterest {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            ctx.near_client()
                .market(params.market_id)
                .get_borrow_position_pending_interest(GetBorrowPositionPendingInterestArgs {
                    account_id: params.account_id,
                    snapshot_limit: params.snapshot_limit,
                })
                .await
                .map(|amount| market::GetBorrowPositionPendingInterestResult { amount })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for market::GetBorrowStatus {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            ctx.near_client()
                .market(params.market_id)
                .get_borrow_status(GetBorrowStatusArgs {
                    account_id: params.account_id,
                    oracle_response: params.oracle_response,
                })
                .await
                .map(|status| market::GetBorrowStatusResult { status })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for market::ListSupplyPositions {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.params.market_id)
                .list_supply_positions(request.params.args)
                .await
                .map(|positions| market::ListSupplyPositionsResult { positions })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for market::GetSupplyPosition {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.params.market_id)
                .get_supply_position(AccountIdArg {
                    account_id: request.params.account_id,
                })
                .await
                .map(|position| market::GetSupplyPositionResult { position })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for market::GetSupplyPositionPendingYield {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            ctx.near_client()
                .market(params.market_id)
                .get_supply_position_pending_yield(GetSupplyPositionPendingYieldArgs {
                    account_id: params.account_id,
                    snapshot_limit: params.snapshot_limit,
                })
                .await
                .map(|amount| market::GetSupplyPositionPendingYieldResult { amount })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for market::GetSupplyWithdrawalRequestStatus {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.params.market_id)
                .get_supply_withdrawal_request_status(AccountIdArg {
                    account_id: request.params.account_id,
                })
                .await
                .map(|status| market::GetSupplyWithdrawalRequestStatusResult { status })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for market::GetSupplyWithdrawalQueueStatus {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.params.market_id)
                .get_supply_withdrawal_queue_status(())
                .await
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for market::GetLastYieldRate {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.params.market_id)
                .get_last_yield_rate(())
                .await
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for market::GetStaticYield {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.params.market_id)
                .get_static_yield(AccountIdArg {
                    account_id: request.params.account_id,
                })
                .await
                .map(|accumulator| market::GetStaticYieldResult { accumulator })
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for market::Borrow {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.body.market_id)
                .borrow(
                    ContractWriteOptions::new(request.signer_account_id).tgas(300),
                    AmountArg {
                        amount: request.body.amount,
                    },
                )
                .map(OperationPlan::from)
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for market::Create {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            let body = request.body;
            let market_account_id = body
                .registry_id
                .0
                .sub_account(&body.name)
                .map_err(|error| crate::GatewayError::NearQuery(error.to_string()))?;
            let configuration = body.configuration;
            let mut steps = plan_deploy_from_registry(
                &ctx,
                request.signer_account_id.clone(),
                DeployBody {
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
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for market::Supply {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
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
                    body.market_id.0.clone(),
                )
                .await?
                {
                    steps.push(tx_result);
                }
            }

            if let Some(tx_result) = ensure_storage_registration(
                &ctx,
                request.signer_account_id.clone(),
                body.market_id.0.clone(),
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
                body.market_id.0,
                body.amount,
                &DepositMsg::Supply,
            )?);

            Ok(OperationPlan { steps })
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for market::WithdrawCollateral {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.body.market_id)
                .withdraw_collateral(
                    ContractWriteOptions::new(request.signer_account_id).tgas(300),
                    AmountArg {
                        amount: request.body.amount,
                    },
                )
                .map(OperationPlan::from)
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for market::ApplyInterest {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
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
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for market::Repay {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
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
                    body.market_id.0.clone(),
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
                body.market_id.0,
                body.amount,
                &deposit_msg,
            )?);

            Ok(OperationPlan { steps })
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for market::CreateSupplyWithdrawalRequest {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.body.market_id)
                .create_supply_withdrawal_request(
                    ContractWriteOptions::new(request.signer_account_id).tgas(300),
                    AmountArg {
                        amount: request.body.amount,
                    },
                )
                .map(OperationPlan::from)
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for market::CancelSupplyWithdrawalRequest {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.body.market_id)
                .cancel_supply_withdrawal_request(
                    ContractWriteOptions::new(request.signer_account_id).tgas(300),
                    (),
                )
                .map(OperationPlan::from)
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for market::ExecuteNextSupplyWithdrawalRequest {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.body.market_id)
                .execute_next_supply_withdrawal_request(
                    ContractWriteOptions::new(request.signer_account_id).tgas(300),
                    BatchLimitArg {
                        batch_limit: request.body.batch_limit,
                    },
                )
                .map(OperationPlan::from)
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for market::WithdrawSupply {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
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
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for market::Liquidate {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
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
                    body.market_id.0.clone(),
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
                body.market_id.0,
                body.liquidation_amount,
                &DepositMsg::Liquidate(LiquidateMsg {
                    account_id: body.account_id,
                    amount: body.collateral_amount,
                }),
            )?);

            Ok(OperationPlan { steps })
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for market::HarvestYield {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
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
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for market::AccumulateStaticYield {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
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
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for market::WithdrawStaticYield {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            ctx.near_client()
                .market(request.body.market_id)
                .withdraw_static_yield(
                    ContractWriteOptions::new(request.signer_account_id).tgas(300),
                    AmountArg {
                        amount: request.body.amount,
                    },
                )
                .map(OperationPlan::from)
        })
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
