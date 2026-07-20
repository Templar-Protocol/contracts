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

// ENG-425: the gateway `vault.deposit` plan registers the depositor with
// `registration_only: true` + `min`, which leaves no storage balance for the
// share-mint's per-holder address-book entry, so the deposit reports top-level
// success but is silently refunded (zero shares, zero assets landed).
//
// This test pins that *actual* current behavior so it stays exercised in CI.
// When ENG-425 is fixed the deposit will land, this test will go red, and its
// downstream coverage — donation/resync accounting and the withdraw plan
// pre-registering its receiver on the underlying token — should be restored from
// git history at this revision (it cannot run while the deposit refunds).
#[tokio::test]
async fn vault_deposit_currently_refunds_eng425() -> Result<()> {
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

    // The deposit reports top-level success...
    exec(
        &stack,
        &signer,
        vault::Deposit {
            vault_id: vault_id.clone(),
            amount: SU128::from(100_u128),
        },
    )
    .await?;

    // ...but is refunded: no assets land (ENG-425). The deposit settles through
    // async callbacks, so give the refund time to land before asserting nothing
    // changed. `vault.deposit` moves idle balance and total assets together, so a
    // zero on both is the refund signature.
    tokio::time::sleep(Duration::from_secs(3)).await;
    anyhow::ensure!(
        vault_assets_match(&stack, &vault_id, 0).await?,
        "ENG-425 regressed: deposit was expected to refund (idle/total assets stay 0)"
    );

    stack.shutdown().await;
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
