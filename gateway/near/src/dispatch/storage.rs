use blockchain_gateway_core::{
    common::{StorageBalance, StorageBalanceBounds},
    storage,
};
use futures::future::BoxFuture;

use crate::{
    actor::{DispatchRead, PlanWrite},
    client::{
        storage::{
            StorageBalanceBoundsView, StorageBalanceOfArgs, StorageDepositArgs,
            StorageUnregisterArgs,
        },
        ContractWriteOptions,
    },
    dispatch::single_transaction_plan,
    GatewayContext, GatewayResult,
};

impl DispatchRead for storage::GetBalanceBounds {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.storage(request.params.contract_id)
                .cached_storage_balance_bounds()
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

impl PlanWrite for storage::Deposit {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<crate::operation::OperationPlan>> {
        Box::pin(async move {
            Ok(single_transaction_plan(
                ctx.storage(request.body.contract_id).storage_deposit(
                    ContractWriteOptions::new(request.signer_account_id)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(100))
                        .deposit(request.body.deposit),
                    StorageDepositArgs {
                        account_id: request.body.beneficiary_id,
                        registration_only: request.body.registration_only,
                    },
                )?,
            ))
        })
    }
}

impl PlanWrite for storage::Unregister {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<crate::operation::OperationPlan>> {
        Box::pin(async move {
            Ok(single_transaction_plan(
                ctx.storage(request.body.contract_id).storage_unregister(
                    ContractWriteOptions::new(request.signer_account_id)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(100))
                        .deposit(blockchain_gateway_core::NearToken::from_yoctonear(1)),
                    StorageUnregisterArgs {
                        force: request.body.force,
                    },
                )?,
            ))
        })
    }
}

impl PlanWrite for storage::EnsureDeposit {
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
                .cached_storage_balance_bounds()
                .await?;
            let balance = ctx
                .storage(contract_id.clone())
                .storage_balance_of(StorageBalanceOfArgs {
                    account_id: account_id.clone(),
                })
                .await?;

            let plan = required_deposit(&body.mode, &bounds, balance.as_ref());

            if plan.deposit.is_zero() {
                return Ok(crate::operation::OperationPlan { steps: vec![] });
            }

            Ok(single_transaction_plan(
                ctx.storage(body.contract_id).storage_deposit(
                    ContractWriteOptions::new(request.signer_account_id)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(100))
                        .deposit(plan.deposit),
                    StorageDepositArgs {
                        account_id: Some(body.account_id),
                        registration_only: plan.registration_only,
                    },
                )?,
            ))
        })
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
    bounds: &StorageBalanceBoundsView,
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
