use std::sync::Arc;

use blockchain_gateway_core::market;
use futures::future::BoxFuture;
use near_api::types::transaction::result::TransactionResult;
use templar_common::{
    asset::FungibleAsset,
    market::{DepositMsg, LiquidateMsg, RepayAccountMsg},
};

use crate::{
    actor::{
        operation_outcome_from_transaction_result, operation_outcome_from_transaction_results,
        DispatchRead, DispatchWrite,
    },
    client::{
        market::{
            AccountIdArg, AccumulateStaticYieldArgs, AmountArg, ApplyInterestArgs, BatchLimitArg,
            GetBorrowPositionPendingInterestArgs, GetBorrowStatusArgs,
            GetSupplyPositionPendingYieldArgs, HarvestYieldArgs,
        },
        storage::{StorageBalanceOfArgs, StorageDepositArgs},
        ContractWriteOptions,
    },
    GatewayContext, GatewayResult,
};

impl DispatchRead for market::GetConfiguration {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.market(request.params.market_id)
                .get_configuration(())
                .await
        })
    }
}

impl DispatchRead for market::GetCurrentSnapshot {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.market(request.params.market_id)
                .get_current_snapshot(())
                .await
        })
    }
}

impl DispatchRead for market::GetFinalizedSnapshotsLen {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.market(request.params.market_id)
                .get_finalized_snapshots_len(())
                .await
        })
    }
}

impl DispatchRead for market::ListFinalizedSnapshots {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.market(request.params.market_id)
                .list_finalized_snapshots(request.params.args)
                .await
                .map(|snapshots| market::ListFinalizedSnapshotsResult { snapshots })
        })
    }
}

impl DispatchRead for market::GetBorrowAssetMetrics {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.market(request.params.market_id)
                .get_borrow_asset_metrics(())
                .await
        })
    }
}

impl DispatchRead for market::ListBorrowPositions {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.market(request.params.market_id)
                .list_borrow_positions(request.params.args)
                .await
                .map(|positions| market::ListBorrowPositionsResult { positions })
        })
    }
}

impl DispatchRead for market::GetBorrowPosition {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.market(request.params.market_id)
                .get_borrow_position(AccountIdArg {
                    account_id: request.params.account_id,
                })
                .await
                .map(|position| market::GetBorrowPositionResult { position })
        })
    }
}

impl DispatchRead for market::GetBorrowPositionPendingInterest {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            ctx.market(params.market_id)
                .get_borrow_position_pending_interest(GetBorrowPositionPendingInterestArgs {
                    account_id: params.account_id,
                    snapshot_limit: params.snapshot_limit,
                })
                .await
                .map(|amount| market::GetBorrowPositionPendingInterestResult { amount })
        })
    }
}

impl DispatchRead for market::GetBorrowStatus {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            ctx.market(params.market_id)
                .get_borrow_status(GetBorrowStatusArgs {
                    account_id: params.account_id,
                    oracle_response: params.oracle_response,
                })
                .await
                .map(|status| market::GetBorrowStatusResult { status })
        })
    }
}

impl DispatchRead for market::ListSupplyPositions {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.market(request.params.market_id)
                .list_supply_positions(request.params.args)
                .await
                .map(|positions| market::ListSupplyPositionsResult { positions })
        })
    }
}

impl DispatchRead for market::GetSupplyPosition {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.market(request.params.market_id)
                .get_supply_position(AccountIdArg {
                    account_id: request.params.account_id,
                })
                .await
                .map(|position| market::GetSupplyPositionResult { position })
        })
    }
}

impl DispatchRead for market::GetSupplyPositionPendingYield {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            ctx.market(params.market_id)
                .get_supply_position_pending_yield(GetSupplyPositionPendingYieldArgs {
                    account_id: params.account_id,
                    snapshot_limit: params.snapshot_limit,
                })
                .await
                .map(|amount| market::GetSupplyPositionPendingYieldResult { amount })
        })
    }
}

impl DispatchRead for market::GetSupplyWithdrawalRequestStatus {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.market(request.params.market_id)
                .get_supply_withdrawal_request_status(AccountIdArg {
                    account_id: request.params.account_id,
                })
                .await
                .map(|status| market::GetSupplyWithdrawalRequestStatusResult { status })
        })
    }
}

impl DispatchRead for market::GetSupplyWithdrawalQueueStatus {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.market(request.params.market_id)
                .get_supply_withdrawal_queue_status(())
                .await
        })
    }
}

impl DispatchRead for market::GetLastYieldRate {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.market(request.params.market_id)
                .get_last_yield_rate(())
                .await
        })
    }
}

impl DispatchRead for market::GetStaticYield {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.market(request.params.market_id)
                .get_static_yield(AccountIdArg {
                    account_id: request.params.account_id,
                })
                .await
                .map(|accumulator| market::GetStaticYieldResult { accumulator })
        })
    }
}

impl DispatchWrite for market::Borrow {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = ctx
                .market(request.body.market_id)
                .borrow(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .tgas(300),
                    AmountArg {
                        amount: request.body.amount,
                    },
                )
                .await?;
            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }
    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for market::Supply {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let body = request.body;
            let signer_account_id = request.signer_account_id.clone();
            let configuration = ctx
                .market(body.market_id.clone())
                .get_configuration(())
                .await?;
            let mut tx_results = Vec::new();

            if let Some(asset_id) = configuration.borrow_asset.clone().into_nep141() {
                if let Some(tx_result) = ensure_storage_registration(
                    &ctx,
                    request.signer_account_id.clone(),
                    signer.clone(),
                    request.wait_until,
                    asset_id,
                    body.market_id.0.clone(),
                )
                .await?
                {
                    tx_results.push(tx_result);
                }
            }

            if let Some(tx_result) = ensure_storage_registration(
                &ctx,
                request.signer_account_id.clone(),
                signer.clone(),
                request.wait_until,
                body.market_id.0.clone(),
                signer_account_id.0.clone(),
            )
            .await?
            {
                tx_results.push(tx_result);
            }

            tx_results.push(
                transfer_call_asset(
                    &ctx,
                    request.signer_account_id,
                    signer,
                    request.wait_until,
                    configuration.borrow_asset,
                    body.market_id.0,
                    body.amount,
                    &DepositMsg::Supply,
                )
                .await?,
            );

            Ok(operation_outcome_from_transaction_results(
                signer_account_id,
                tx_results,
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for market::WithdrawCollateral {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = ctx
                .market(request.body.market_id)
                .withdraw_collateral(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .tgas(300),
                    AmountArg {
                        amount: request.body.amount,
                    },
                )
                .await?;
            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }
    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for market::ApplyInterest {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let tx_result = ctx
                .market(body.market_id)
                .apply_interest(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .tgas(300),
                    ApplyInterestArgs {
                        account_id: body.account_id,
                        snapshot_limit: body.snapshot_limit,
                    },
                )
                .await?;
            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }
    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for market::Repay {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let body = request.body;
            let signer_account_id = request.signer_account_id.clone();
            let configuration = ctx
                .market(body.market_id.clone())
                .get_configuration(())
                .await?;
            let deposit_msg = body.account_id.map_or(DepositMsg::Repay, |account_id| {
                DepositMsg::RepayAccount(RepayAccountMsg { account_id })
            });
            let mut tx_results = Vec::new();

            if let Some(asset_id) = configuration.borrow_asset.clone().into_nep141() {
                if let Some(tx_result) = ensure_storage_registration(
                    &ctx,
                    request.signer_account_id.clone(),
                    signer.clone(),
                    request.wait_until,
                    asset_id,
                    body.market_id.0.clone(),
                )
                .await?
                {
                    tx_results.push(tx_result);
                }
            }

            tx_results.push(
                transfer_call_asset(
                    &ctx,
                    request.signer_account_id,
                    signer,
                    request.wait_until,
                    configuration.borrow_asset,
                    body.market_id.0,
                    body.amount,
                    &deposit_msg,
                )
                .await?,
            );

            Ok(operation_outcome_from_transaction_results(
                signer_account_id,
                tx_results,
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for market::CreateSupplyWithdrawalRequest {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = ctx
                .market(request.body.market_id)
                .create_supply_withdrawal_request(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .tgas(300),
                    AmountArg {
                        amount: request.body.amount,
                    },
                )
                .await?;
            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }
    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for market::CancelSupplyWithdrawalRequest {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = ctx
                .market(request.body.market_id)
                .cancel_supply_withdrawal_request(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .tgas(300),
                    (),
                )
                .await?;
            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }
    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for market::ExecuteNextSupplyWithdrawalRequest {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = ctx
                .market(request.body.market_id)
                .execute_next_supply_withdrawal_request(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .tgas(300),
                    BatchLimitArg {
                        batch_limit: request.body.batch_limit,
                    },
                )
                .await?;
            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }
    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for market::WithdrawSupply {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let body = request.body;
            let signer_account_id = request.signer_account_id.clone();
            let configuration = ctx
                .market(body.market_id.clone())
                .get_configuration(())
                .await?;
            let queue_status = ctx
                .market(body.market_id.clone())
                .get_supply_withdrawal_queue_status(())
                .await?;
            let mut tx_results = Vec::new();

            if let Some(asset_id) = configuration.borrow_asset.clone().into_nep141() {
                if let Some(tx_result) = ensure_storage_registration(
                    &ctx,
                    request.signer_account_id.clone(),
                    signer.clone(),
                    request.wait_until,
                    asset_id,
                    signer_account_id.0.clone(),
                )
                .await?
                {
                    tx_results.push(tx_result);
                }
            }

            tx_results.push(
                ctx.market(body.market_id.clone())
                    .create_supply_withdrawal_request(
                        ContractWriteOptions::new(
                            request.signer_account_id.clone(),
                            signer.clone(),
                        )
                        .wait_until(request.wait_until)
                        .tgas(300),
                        AmountArg {
                            amount: body.amount,
                        },
                    )
                    .await?,
            );

            if queue_status.length == 0 {
                tx_results.push(
                    ctx.market(body.market_id)
                        .execute_next_supply_withdrawal_request(
                            ContractWriteOptions::new(request.signer_account_id, signer)
                                .wait_until(request.wait_until)
                                .tgas(300),
                            BatchLimitArg {
                                batch_limit: body.batch_limit,
                            },
                        )
                        .await?,
                );
            }

            Ok(operation_outcome_from_transaction_results(
                signer_account_id,
                tx_results,
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for market::Liquidate {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let body = request.body;
            let signer_account_id = request.signer_account_id.clone();
            let configuration = ctx
                .market(body.market_id.clone())
                .get_configuration(())
                .await?;
            let mut tx_results = Vec::new();

            if let Some(asset_id) = configuration.borrow_asset.clone().into_nep141() {
                if let Some(tx_result) = ensure_storage_registration(
                    &ctx,
                    request.signer_account_id.clone(),
                    signer.clone(),
                    request.wait_until,
                    asset_id,
                    body.market_id.0.clone(),
                )
                .await?
                {
                    tx_results.push(tx_result);
                }
            }

            if let Some(asset_id) = configuration.collateral_asset.clone().into_nep141() {
                if let Some(tx_result) = ensure_storage_registration(
                    &ctx,
                    request.signer_account_id.clone(),
                    signer.clone(),
                    request.wait_until,
                    asset_id,
                    signer_account_id.0.clone(),
                )
                .await?
                {
                    tx_results.push(tx_result);
                }
            }

            tx_results.push(
                transfer_call_asset(
                    &ctx,
                    request.signer_account_id,
                    signer,
                    request.wait_until,
                    configuration.borrow_asset,
                    body.market_id.0,
                    body.liquidation_amount,
                    &DepositMsg::Liquidate(LiquidateMsg {
                        account_id: body.account_id,
                        amount: body.collateral_amount,
                    }),
                )
                .await?,
            );

            Ok(operation_outcome_from_transaction_results(
                signer_account_id,
                tx_results,
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for market::HarvestYield {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let tx_result = ctx
                .market(body.market_id)
                .harvest_yield(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .tgas(300),
                    HarvestYieldArgs {
                        account_id: body.account_id,
                        mode: body.mode,
                    },
                )
                .await?;
            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }
    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for market::AccumulateStaticYield {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let tx_result = ctx
                .market(body.market_id)
                .accumulate_static_yield(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .tgas(300),
                    AccumulateStaticYieldArgs {
                        account_id: body.account_id,
                        snapshot_limit: body.snapshot_limit,
                    },
                )
                .await?;
            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }
    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for market::WithdrawStaticYield {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = ctx
                .market(request.body.market_id)
                .withdraw_static_yield(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .tgas(300),
                    AmountArg {
                        amount: request.body.amount,
                    },
                )
                .await?;
            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }
    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

async fn ensure_storage_registration(
    ctx: &GatewayContext,
    signer_account_id: blockchain_gateway_core::ManagedAccountId,
    signer: Arc<near_api::Signer>,
    wait_until: blockchain_gateway_core::common::TxExecutionStatus,
    contract_id: near_account_id::AccountId,
    account_id: near_account_id::AccountId,
) -> GatewayResult<Option<TransactionResult>> {
    let balance = ctx
        .storage(contract_id.clone())
        .storage_balance_of(StorageBalanceOfArgs {
            account_id: account_id.clone(),
        })
        .await?;

    if balance.is_some() {
        return Ok(None);
    }

    let bounds = ctx
        .storage(contract_id.clone())
        .storage_balance_bounds(())
        .await?;
    let tx_result = ctx
        .storage(contract_id)
        .storage_deposit(
            ContractWriteOptions::new(signer_account_id, signer)
                .wait_until(wait_until)
                .tgas(100)
                .deposit(blockchain_gateway_core::NearToken::from_yoctonear(
                    bounds.min.as_yoctonear(),
                )),
            StorageDepositArgs {
                account_id: Some(account_id),
                registration_only: true,
            },
        )
        .await?;
    Ok(Some(tx_result))
}

async fn transfer_call_asset<T: templar_common::asset::AssetClass>(
    ctx: &GatewayContext,
    signer_account_id: blockchain_gateway_core::ManagedAccountId,
    signer: Arc<near_api::Signer>,
    wait_until: blockchain_gateway_core::common::TxExecutionStatus,
    asset: FungibleAsset<T>,
    receiver_id: near_account_id::AccountId,
    amount: impl Into<u128>,
    msg: &impl serde::Serialize,
) -> GatewayResult<TransactionResult> {
    ctx.token(asset)
        .transfer_call(
            ContractWriteOptions::new(signer_account_id, signer)
                .wait_until(wait_until)
                .tgas(300)
                .one_yocto(),
            receiver_id,
            amount,
            serde_json::to_string(msg)?,
        )
        .await
}
