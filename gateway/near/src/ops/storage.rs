use blockchain_gateway_core::{
    storage::{self, EnsureDepositMode},
    NearToken,
};
use near_contract_standards::storage_management::{StorageBalance, StorageBalanceBounds};

use crate::{
    actor::operation_outcome_from_transaction_result,
    client::{
        storage::{StorageBalanceOfArgs, StorageDepositArgs},
        ContractWriteOptions,
    },
    GatewayError, GatewayResult, NearClient,
};

pub async fn ensure_deposit(
    client: &NearClient,
    signer_account_id: blockchain_gateway_core::ManagedAccountId,
    signer: std::sync::Arc<near_api::Signer>,
    wait_until: blockchain_gateway_core::common::TxExecutionStatus,
    body: storage::EnsureDepositBody,
) -> GatewayResult<storage::EnsureDepositResult> {
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

    let tx_result = client
        .storage(contract_id.clone())
        .storage_deposit(
            ContractWriteOptions::new(signer_account_id.clone(), signer)
                .wait_until(wait_until)
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
        return Err(GatewayError::NearQuery(
            "storage deposit did not satisfy ensureDeposit requirement".to_owned(),
        ));
    }

    Ok(storage::EnsureDepositResult::Operation(
        operation_outcome_from_transaction_result(signer_account_id, tx_result),
    ))
}

struct DepositPlan {
    deposit: NearToken,
    registration_only: bool,
}

impl DepositPlan {
    fn empty() -> Self {
        Self {
            deposit: NearToken::ZERO,
            registration_only: false,
        }
    }

    fn new(deposit: NearToken, registration_only: bool) -> Self {
        Self {
            deposit,
            registration_only,
        }
    }
}

fn required_deposit(
    mode: &EnsureDepositMode,
    bounds: &StorageBalanceBounds,
    balance: Option<&StorageBalance>,
) -> DepositPlan {
    match (mode, balance) {
        (EnsureDepositMode::Registered, Some(_)) => DepositPlan::empty(),
        (EnsureDepositMode::Registered, None) => DepositPlan::new(bounds.min, true),
        (
            EnsureDepositMode::MinimumTotal(amount) | EnsureDepositMode::MinimumAvailable(amount),
            None,
        ) => DepositPlan::new(bounds.min.max(*amount), false),
        (EnsureDepositMode::MinimumTotal(amount), Some(balance)) => {
            DepositPlan::new(amount.saturating_sub(balance.total), false)
        }
        (EnsureDepositMode::MinimumAvailable(amount), Some(balance)) => {
            DepositPlan::new(amount.saturating_sub(balance.available), false)
        }
    }
}

fn satisfies_mode(mode: &EnsureDepositMode, balance: Option<&StorageBalance>) -> bool {
    let Some(balance) = balance else {
        return false;
    };
    match mode {
        EnsureDepositMode::Registered => true,
        EnsureDepositMode::MinimumTotal(amount) => {
            balance.total.as_yoctonear() >= amount.as_yoctonear()
        }
        EnsureDepositMode::MinimumAvailable(amount) => {
            balance.available.as_yoctonear() >= amount.as_yoctonear()
        }
    }
}
