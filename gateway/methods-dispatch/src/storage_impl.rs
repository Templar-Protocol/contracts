use async_trait::async_trait;
use templar_gateway_core::{
    client::{
        storage::{
            StorageBalanceBoundsView, StorageBalanceOfArgs, StorageDepositArgs,
            StorageUnregisterArgs,
        },
        ContractWriteOptions,
    },
    DispatchRead, GatewayResult, HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::storage;
use templar_gateway_types::common::{StorageBalance, StorageBalanceBounds};

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<storage::GetBalanceBounds, C> for Dispatch {
    async fn dispatch(
        request: storage::GetBalanceBounds,
        ctx: C,
    ) -> GatewayResult<storage::GetBalanceBoundsResult> {
        ctx.near_client()
            .storage(request.contract_id)
            .cached_storage_balance_bounds()
            .await
            .map(|bounds| storage::GetBalanceBoundsResult {
                bounds: StorageBalanceBounds {
                    min: bounds.min,
                    max: bounds.max,
                },
            })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<storage::GetBalanceOf, C> for Dispatch {
    async fn dispatch(
        request: storage::GetBalanceOf,
        ctx: C,
    ) -> GatewayResult<storage::GetBalanceOfResult> {
        ctx.near_client()
            .storage(request.contract_id)
            .storage_balance_of(StorageBalanceOfArgs {
                account_id: request.account_id,
            })
            .await
            .map(|balance| storage::GetBalanceOfResult {
                balance: balance.map(|balance| StorageBalance {
                    total: balance.total,
                    available: balance.available,
                }),
            })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<storage::Deposit, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<storage::Deposit>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        ctx.near_client()
            .storage(request.body.contract_id)
            .storage_deposit(
                ContractWriteOptions::new(request.signer_account_id)
                    .tgas(100)
                    .deposit(request.body.deposit),
                StorageDepositArgs {
                    account_id: request.body.beneficiary_id,
                    registration_only: request.body.registration_only,
                },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<storage::Unregister, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<storage::Unregister>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        ctx.near_client()
            .storage(request.body.contract_id)
            .storage_unregister(
                ContractWriteOptions::new(request.signer_account_id)
                    .tgas(100)
                    .one_yocto(),
                StorageUnregisterArgs {
                    force: request.body.force,
                },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<storage::EnsureDeposit, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<storage::EnsureDeposit>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        let contract_id = body.contract_id.clone();
        let account_id = body.account_id.clone();

        let bounds = ctx
            .near_client()
            .storage(contract_id.clone())
            .cached_storage_balance_bounds()
            .await?;
        let balance = ctx
            .near_client()
            .storage(contract_id.clone())
            .storage_balance_of(StorageBalanceOfArgs {
                account_id: account_id.clone(),
            })
            .await?;

        let plan = required_deposit(&body.mode, &bounds, balance.as_ref());

        if plan.deposit.is_zero() {
            return Ok(OperationPlan { steps: vec![] });
        }

        ctx.near_client()
            .storage(body.contract_id)
            .storage_deposit(
                ContractWriteOptions::new(request.signer_account_id)
                    .tgas(100)
                    .deposit(plan.deposit),
                StorageDepositArgs {
                    account_id: Some(body.account_id),
                    registration_only: plan.registration_only,
                },
            )
            .map(OperationPlan::from)
    }
}

struct DepositPlan {
    deposit: templar_gateway_types::NearToken,
    registration_only: bool,
}

impl DepositPlan {
    fn empty() -> Self {
        Self {
            deposit: templar_gateway_types::NearToken::ZERO,
            registration_only: false,
        }
    }

    fn new(deposit: templar_gateway_types::NearToken, registration_only: bool) -> Self {
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
