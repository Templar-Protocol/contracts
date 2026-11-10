use anyhow::{Context as _, Result};
use templar_gateway_methods_spec::{account, tx};
use templar_gateway_testing::SandboxHarness;
use templar_gateway_types::{common::TxExecutionStatus, ActionInput, NearToken};

#[tokio::test]
async fn reported_spendable_near_can_be_transferred_without_spending_storage_backing() -> Result<()>
{
    let harness = SandboxHarness::start().await?;
    let sender = harness.create_user("spendable-sender").await?;
    let beneficiary = harness.create_user("spendable-beneficiary").await?;
    let client = harness.client()?;
    let before = client
        .read(account::Get {
            account_id: sender.0.clone(),
        })
        .await?;
    let available = client
        .read(account::GetSpendableBalance {
            account_id: sender.0.clone(),
        })
        .await?;
    let backing = before
        .amount
        .as_yoctonear()
        .checked_sub(available.as_yoctonear())
        .context("spendable balance cannot exceed liquid NEAR")?;
    // Leave less liquid cash than the storage stake: treating `amount` as spendable
    // would make the real protocol reject this transfer for insufficient backing.
    anyhow::ensure!(
        backing > 0,
        "the real access-key/account records require storage backing"
    );
    let reserve = backing / 2;
    let transfer = available
        .as_yoctonear()
        .checked_sub(reserve)
        .context("funded sender")?;
    let operation = harness
        .execute(
            &sender,
            tx::Batch {
                receiver_id: beneficiary.0,
                actions: vec![ActionInput::Transfer {
                    deposit: NearToken::from_yoctonear(transfer),
                }],
            },
        )
        .await?;
    let hash = operation
        .operation
        .latest_tx_hash()
        .context("native transfer hash")?;
    let finality = client
        .read(tx::Get {
            tx_hash: hash,
            sender_account_id: sender.0.clone(),
            wait_until: Some(TxExecutionStatus::Final),
            encoding: tx::ValueEncoding::Json,
        })
        .await?;
    assert_eq!(finality.status, tx::Status::Succeeded);
    assert!(finality.failed_receipts.is_empty());
    let remaining = client
        .read(account::GetSpendableBalance {
            account_id: sender.0,
        })
        .await?;
    assert_eq!(
        remaining.as_yoctonear(),
        reserve
            .checked_sub(finality.tokens_burnt.as_yoctonear())
            .context("fixture reserve must cover native transfer gas")?
    );
    Ok(())
}
