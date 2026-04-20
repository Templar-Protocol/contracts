use blockchain_gateway_core::{
    common::{StorageBalance, StorageBalanceBounds},
    storage, ManagedAccountId,
};
use futures::future::BoxFuture;

use crate::{
    actor::{DispatchRead, DispatchWrite},
    client::storage::{StorageBalanceOfArgs, StorageDepositArgs, StorageUnregisterArgs},
    dispatch::{function_call_transaction_json, single_transaction_plan},
    GatewayContext, GatewayResult,
};

impl DispatchRead for storage::GetBalanceBounds {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.storage(request.params.contract_id)
                .storage_balance_bounds(())
                .await
                .map(|bounds| storage::GetBalanceBoundsResult {
                    bounds: StorageBalanceBounds {
                        min: bounds.min,
                        max: bounds.max,
                    },
                })
        })
    }
}

impl DispatchRead for storage::GetBalanceOf {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.storage(request.params.contract_id)
                .storage_balance_of(StorageBalanceOfArgs {
                    account_id: request.params.account_id,
                })
                .await
                .map(|balance| storage::GetBalanceOfResult {
                    balance: balance.map(|balance| StorageBalance {
                        total: balance.total,
                        available: balance.available,
                    }),
                })
        })
    }
}

impl DispatchWrite for storage::Deposit {
    fn uses_operation_planning() -> bool {
        true
    }

    fn signer_account_id(request: &Self::Input) -> &ManagedAccountId {
        &request.signer_account_id
    }

    fn plan(
        request: Self::Input,
        _ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<crate::operation::OperationPlan>> {
        Box::pin(async move {
            Ok(single_transaction_plan(
                request.wait_until,
                function_call_transaction_json(
                    request.signer_account_id,
                    request.body.contract_id,
                    "storage_deposit",
                    StorageDepositArgs {
                        account_id: request.body.beneficiary_id,
                        registration_only: request.body.registration_only,
                    },
                    blockchain_gateway_core::NearGas::from_tgas(100),
                    request.body.deposit,
                )?,
            ))
        })
    }
}

impl DispatchWrite for storage::Unregister {
    fn uses_operation_planning() -> bool {
        true
    }

    fn signer_account_id(request: &Self::Input) -> &ManagedAccountId {
        &request.signer_account_id
    }

    fn plan(
        request: Self::Input,
        _ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<crate::operation::OperationPlan>> {
        Box::pin(async move {
            Ok(single_transaction_plan(
                request.wait_until,
                function_call_transaction_json(
                    request.signer_account_id,
                    request.body.contract_id,
                    "storage_unregister",
                    StorageUnregisterArgs {
                        force: request.body.force,
                    },
                    blockchain_gateway_core::NearGas::from_tgas(100),
                    blockchain_gateway_core::NearToken::from_yoctonear(1),
                )?,
            ))
        })
    }
}

impl DispatchWrite for storage::EnsureDeposit {
    fn uses_operation_planning() -> bool {
        true
    }

    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<crate::operation::OperationPlan>> {
        Box::pin(async move {
            let body = request.body;
            let contract_id = body.contract_id.clone();
            let account_id = body.account_id.clone();

            let bounds = ctx
                .storage(contract_id.clone())
                .storage_balance_bounds(())
                .await?;
            let balance = ctx
                .storage(contract_id.clone())
                .storage_balance_of(StorageBalanceOfArgs {
                    account_id: account_id.clone(),
                })
                .await?;

            let plan = required_deposit(&body.mode, &bounds, balance.as_ref());

            if plan.deposit.is_zero() {
                return Ok(crate::operation::OperationPlan {
                    wait_until: request.wait_until,
                    steps: vec![],
                });
            }

            Ok(single_transaction_plan(
                request.wait_until,
                function_call_transaction_json(
                    request.signer_account_id,
                    body.contract_id,
                    "storage_deposit",
                    StorageDepositArgs {
                        account_id: Some(body.account_id),
                        registration_only: plan.registration_only,
                    },
                    blockchain_gateway_core::NearGas::from_tgas(100),
                    plan.deposit,
                )?,
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &ManagedAccountId {
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
