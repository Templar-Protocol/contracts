use std::sync::Arc;

use blockchain_gateway_core::market;
use futures::future::BoxFuture;

use crate::{
    actor::{operation_outcome_from_transaction_result, DispatchRead, DispatchWrite},
    client::{
        market::{
            AccountIdArg, AccumulateStaticYieldArgs, AmountArg, ApplyInterestArgs, BatchLimitArg,
            GetBorrowPositionPendingInterestArgs, GetBorrowStatusArgs,
            GetSupplyPositionPendingYieldArgs, HarvestYieldArgs,
        },
        ContractWriteOptions,
    },
    GatewayResult, NearClient,
};

impl DispatchRead for market::GetConfiguration {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(request.params.market_id)
                .get_configuration(())
                .await
        })
    }
}

impl DispatchRead for market::GetCurrentSnapshot {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(request.params.market_id)
                .get_current_snapshot(())
                .await
        })
    }
}

impl DispatchRead for market::GetFinalizedSnapshotsLen {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(request.params.market_id)
                .get_finalized_snapshots_len(())
                .await
        })
    }
}

impl DispatchRead for market::ListFinalizedSnapshots {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(request.params.market_id)
                .list_finalized_snapshots(request.params.args)
                .await
                .map(|snapshots| market::ListFinalizedSnapshotsResult { snapshots })
        })
    }
}

impl DispatchRead for market::GetBorrowAssetMetrics {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(request.params.market_id)
                .get_borrow_asset_metrics(())
                .await
        })
    }
}

impl DispatchRead for market::ListBorrowPositions {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(request.params.market_id)
                .list_borrow_positions(request.params.args)
                .await
                .map(|positions| market::ListBorrowPositionsResult { positions })
        })
    }
}

impl DispatchRead for market::GetBorrowPosition {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(request.params.market_id)
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
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            client
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

impl DispatchRead for market::GetBorrowStatus {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            client
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

impl DispatchRead for market::ListSupplyPositions {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(request.params.market_id)
                .list_supply_positions(request.params.args)
                .await
                .map(|positions| market::ListSupplyPositionsResult { positions })
        })
    }
}

impl DispatchRead for market::GetSupplyPosition {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(request.params.market_id)
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
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            client
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

impl DispatchRead for market::GetSupplyWithdrawalRequestStatus {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(request.params.market_id)
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
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(request.params.market_id)
                .get_supply_withdrawal_queue_status(())
                .await
        })
    }
}

impl DispatchRead for market::GetLastYieldRate {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(request.params.market_id)
                .get_last_yield_rate(())
                .await
        })
    }
}

impl DispatchRead for market::GetStaticYield {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(request.params.market_id)
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
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = client
                .market(request.body.market_id)
                .borrow(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(300)),
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

impl DispatchWrite for market::WithdrawCollateral {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = client
                .market(request.body.market_id)
                .withdraw_collateral(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(300)),
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
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let tx_result = client
                .market(body.market_id)
                .apply_interest(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(300)),
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

impl DispatchWrite for market::CreateSupplyWithdrawalRequest {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = client
                .market(request.body.market_id)
                .create_supply_withdrawal_request(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(300)),
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
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = client
                .market(request.body.market_id)
                .cancel_supply_withdrawal_request(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(300)),
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
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = client
                .market(request.body.market_id)
                .execute_next_supply_withdrawal_request(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(300)),
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

impl DispatchWrite for market::HarvestYield {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let tx_result = client
                .market(body.market_id)
                .harvest_yield(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(300)),
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
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let tx_result = client
                .market(body.market_id)
                .accumulate_static_yield(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(300)),
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
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = client
                .market(request.body.market_id)
                .withdraw_static_yield(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(300)),
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
