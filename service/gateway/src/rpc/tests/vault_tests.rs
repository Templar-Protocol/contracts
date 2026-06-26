use super::*;

use std::time::Duration;

use anyhow::Context as _;
use templar_common::SU128;
use templar_gateway_types::{common::WriteOperationResult, MethodSpec, OperationStatus};

/// Submit a write through the gateway RPC and assert it reached a terminal
/// `Succeeded` status.
async fn exec<S>(
    stack: &TestStack,
    signer: &templar_gateway_types::ManagedAccountId,
    body: S,
) -> Result<()>
where
    S: MethodSpec<Output = WriteOperationResult> + serde::Serialize,
{
    let result = stack
        .controller
        .request::<S>(&WriteRequest {
            signer_account_id: signer.clone(),
            idempotency_key: None,
            body,
        })
        .await?;
    anyhow::ensure!(
        result.operation.status == OperationStatus::Succeeded,
        "gateway write did not succeed: {:?}",
        result.operation
    );
    Ok(())
}

#[tokio::test]
async fn vault_deposit_donate_resync_and_withdraw_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let (market_account, _) = stack.harness.deploy_market().await?;
    let (vault_id, _) = stack.harness.deploy_vault().await?;
    let signer = stack.harness.gateway_signer_account_id.clone();

    // The depositor needs an underlying balance. Storage registration of both
    // the vault (on the underlying token) and the depositor (on the share token)
    // is handled inside the `vault.deposit` plan, so we don't pre-register here.
    let _ = register_gateway_signer_for_ft(&stack).await?;

    let _ = stack
        .controller
        .request::<tx::FunctionCall>(&WriteRequest {
            signer_account_id: signer.clone(),
            idempotency_key: None,
            body: tx::FunctionCall {
                receiver_id: stack.harness.ft_contract_id.clone(),
                method_name: ContractMethodName("mint".to_owned()),
                args: ContractArgs::Json(serde_json::json!({ "amount": "1000" })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::ZERO,
            },
        })
        .await?;

    // Enable the market in the vault so deposits have somewhere to land.
    exec(
        &stack,
        &signer,
        vault::SubmitCap {
            vault_id: vault_id.clone(),
            market: market_account.clone(),
            new_cap: SU128::from(1_000_u128),
        },
    )
    .await?;
    exec(
        &stack,
        &signer,
        vault::AcceptCap {
            vault_id: vault_id.clone(),
            market: market_account.clone(),
        },
    )
    .await?;

    let market_id = stack
        .controller
        .request::<vault::GetMarketIdOfAccount>(&vault::GetMarketIdOfAccount {
            vault_id: vault_id.clone(),
            market: market_account.clone(),
        })
        .await?
        .market_id
        .context("market should be registered in the vault")?;
    exec(
        &stack,
        &signer,
        vault::SetSupplyQueue {
            vault_id: vault_id.clone(),
            markets: vec![market_id],
        },
    )
    .await?;

    // Deposit moves idle balance and total assets together.
    exec(
        &stack,
        &signer,
        vault::Deposit {
            vault_id: vault_id.clone(),
            amount: SU128::from(100_u128),
        },
    )
    .await?;
    assert_vault_assets_eventually(&stack, &vault_id, 100).await?;

    // A bare token transfer (donation) only counts after an explicit resync.
    exec(
        &stack,
        &signer,
        token::Transfer {
            token: token::TokenReference::Ft {
                contract_id: stack.harness.ft_contract_id.clone(),
            },
            receiver_id: vault_id.clone(),
            amount: SU128::from(25_u128),
            memo: None,
        },
    )
    .await?;
    exec(
        &stack,
        &signer,
        vault::ResyncIdleBalance {
            vault_id: vault_id.clone(),
        },
    )
    .await?;
    assert_vault_assets_eventually(&stack, &vault_id, 125).await?;

    // A withdrawal pays out via the underlying token's `ft_transfer`, which fails
    // unless the receiver is registered there. The `vault.withdraw` plan must
    // pre-register the receiver, so a withdrawal to a fresh account succeeds and
    // leaves that account registered on the underlying token.
    let receiver = stack.harness.beneficiary_account_id.clone();
    let receiver_balance_before = stack
        .controller
        .request::<storage::GetBalanceOf>(&storage::GetBalanceOf {
            contract_id: stack.harness.ft_contract_id.clone(),
            account_id: receiver.clone(),
        })
        .await?;
    anyhow::ensure!(
        receiver_balance_before.balance.is_none(),
        "withdrawal receiver should start unregistered on the underlying token"
    );

    exec(
        &stack,
        &signer,
        vault::Withdraw {
            vault_id: vault_id.clone(),
            amount: SU128::from(10_u128),
            receiver: receiver.clone(),
        },
    )
    .await?;

    let receiver_balance_after = stack
        .controller
        .request::<storage::GetBalanceOf>(&storage::GetBalanceOf {
            contract_id: stack.harness.ft_contract_id.clone(),
            account_id: receiver.clone(),
        })
        .await?;
    anyhow::ensure!(
        receiver_balance_after.balance.is_some(),
        "withdrawal plan should have registered the receiver on the underlying token"
    );

    stack.shutdown().await;
    Ok(())
}

/// Deposit and resync settle through async callbacks, so poll the views for a
/// short window before giving up.
async fn assert_vault_assets_eventually(
    stack: &TestStack,
    vault_id: &near_account_id::AccountId,
    expected: u128,
) -> Result<()> {
    for _ in 0..40 {
        if vault_assets_match(stack, vault_id, expected)
            .await
            .unwrap_or(false)
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    anyhow::ensure!(
        vault_assets_match(stack, vault_id, expected).await?,
        "vault idle balance and total assets never reached {expected}"
    );
    Ok(())
}

async fn vault_assets_match(
    stack: &TestStack,
    vault_id: &near_account_id::AccountId,
    expected: u128,
) -> Result<bool> {
    let idle = stack
        .controller
        .request::<vault::GetIdleBalance>(&vault::GetIdleBalance {
            vault_id: vault_id.clone(),
        })
        .await?;
    let total = stack
        .controller
        .request::<vault::GetTotalAssets>(&vault::GetTotalAssets {
            vault_id: vault_id.clone(),
        })
        .await?;
    Ok(idle.0 == expected && total.0 == expected)
}
