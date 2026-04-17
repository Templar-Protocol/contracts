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
            let body = request.body;
            let contract_id = body.contract_id.clone();
            let account_id = body.account_id.clone();

            let bounds = client
                .storage(contract_id.clone())
                .storage_balance_bounds(())
                .await?;
            let balance = client
                .storage(contract_id.clone())
                .storage_balance_of(StorageBalanceOfArgs {
                    account_id: account_id.clone(),
                })
                .await?;

            let plan = required_deposit(&body.mode, &bounds, balance.as_ref());

            if plan.deposit.is_zero() {
                return Ok(storage::EnsureDepositResult::NoOp);
            }

            let signer_account_id = request.signer_account_id.clone();
            let tx_result = client
                .storage(contract_id.clone())
                .storage_deposit(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(100))
                        .deposit(plan.deposit),
                    StorageDepositArgs {
                        account_id: Some(account_id.clone()),
                        registration_only: plan.registration_only,
                    },
                )
                .await?;

            let balance_after = client
                .storage(contract_id)
                .storage_balance_of(StorageBalanceOfArgs { account_id })
                .await?;

            if !satisfies_mode(&body.mode, balance_after.as_ref()) {
                return Err(crate::GatewayError::NearQuery(
                    "storage deposit did not satisfy ensureDeposit requirement".to_owned(),
                ));
            }

            Ok(storage::EnsureDepositResult::Operation(
                operation_outcome_from_transaction_result(signer_account_id, tx_result),
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

struct DepositPlan {
    deposit: blockchain_gateway_core::NearToken,
    registration_only: bool,
}

impl DepositPlan {
    fn empty() -> Self {
        Self {
            deposit: blockchain_gateway_core::NearToken::ZERO,
            registration_only: false,
        }
    }

    fn new(deposit: blockchain_gateway_core::NearToken, registration_only: bool) -> Self {
        Self {
            deposit,
            registration_only,
        }
    }
}

fn required_deposit(
    mode: &storage::EnsureDepositMode,
    bounds: &near_contract_standards::storage_management::StorageBalanceBounds,
    balance: Option<&near_contract_standards::storage_management::StorageBalance>,
) -> DepositPlan {
    match (mode, balance) {
        (storage::EnsureDepositMode::Registered, Some(_)) => DepositPlan::empty(),
        (storage::EnsureDepositMode::Registered, None) => DepositPlan::new(bounds.min, true),
        (
            storage::EnsureDepositMode::MinimumTotal(amount)
            | storage::EnsureDepositMode::MinimumAvailable(amount),
            None,
        ) => DepositPlan::new(bounds.min.max(*amount), false),
        (storage::EnsureDepositMode::MinimumTotal(amount), Some(balance)) => {
            DepositPlan::new(amount.saturating_sub(balance.total), false)
        }
        (storage::EnsureDepositMode::MinimumAvailable(amount), Some(balance)) => {
            DepositPlan::new(amount.saturating_sub(balance.available), false)
        }
    }
}

fn satisfies_mode(
    mode: &storage::EnsureDepositMode,
    balance: Option<&near_contract_standards::storage_management::StorageBalance>,
) -> bool {
    let Some(balance) = balance else {
        return false;
    };
    match mode {
        storage::EnsureDepositMode::Registered => true,
        storage::EnsureDepositMode::MinimumTotal(amount) => {
            balance.total.as_yoctonear() >= amount.as_yoctonear()
        }
        storage::EnsureDepositMode::MinimumAvailable(amount) => {
            balance.available.as_yoctonear() >= amount.as_yoctonear()
        }
    }
}
