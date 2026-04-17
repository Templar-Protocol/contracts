use std::sync::Arc;

use blockchain_gateway_core::storage;
use futures::future::BoxFuture;

use crate::{
    actor::{operation_outcome_from_transaction_result, DispatchRead, DispatchWrite, RpcMessage},
    client::{
        storage::{StorageBalanceOfArgs, StorageDepositArgs, StorageUnregisterArgs},
        ContractWriteOptions,
    },
    GatewayResult, NearClient,
};

impl DispatchRead for storage::GetBalanceBounds {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.params;
            client
                .storage(params.contract_id)
                .storage_balance_bounds(())
                .await
                .map(|bounds| storage::GetBalanceBoundsResult {
                    bounds: blockchain_gateway_core::common::StorageBalanceBounds {
                        min: bounds.min,
                        max: bounds.max,
                    },
                })
        })
    }
}

impl DispatchRead for storage::GetBalanceOf {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.params;
            client
                .storage(params.contract_id)
                .storage_balance_of(StorageBalanceOfArgs {
                    account_id: params.account_id,
                })
                .await
                .map(|balance| storage::GetBalanceOfResult {
                    balance: balance.map(|balance| {
                        blockchain_gateway_core::common::StorageBalance {
                            total: balance.total,
                            available: balance.available,
                        }
                    }),
                })
        })
    }
}

impl DispatchWrite for storage::Deposit {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let tx_result = client
                .storage(body.contract_id)
                .storage_deposit(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(100))
                        .deposit(body.deposit),
                    StorageDepositArgs {
                        account_id: body.beneficiary_id,
                        registration_only: body.registration_only,
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

impl DispatchWrite for storage::Unregister {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let tx_result = client
                .storage(body.contract_id)
                .storage_unregister(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(100))
                        .one_yocto(),
                    StorageUnregisterArgs { force: body.force },
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

impl DispatchWrite for storage::EnsureDeposit {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            crate::ops::storage::ensure_deposit(
                &client,
                request.signer_account_id,
                signer,
                request.wait_until,
                request.body,
            )
            .await
        })
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}
