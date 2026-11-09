use anyhow::{Context as _, Result};
use near_api::types::transaction::{
    actions::{Action, FunctionCallAction},
    PrepopulateTransaction,
};
use serde_json::json;
use templar_gateway_methods_spec::{account, chain, tx};
use templar_gateway_testing::{test_secret_key, test_signer, SandboxHarness};
use templar_gateway_types::{
    common::{ContractArgs, TxExecutionStatus},
    ActionInput, ContractMethodName, NearGas, NearToken,
};

#[tokio::test]
async fn transaction_inspection_preserves_original_hash_and_top_level_action_indices() -> Result<()>
{
    let harness = SandboxHarness::start().await?;
    let ft = harness
        .deploy_ft("inspection-ft", "Inspection", "INSP")
        .await?;
    let signer = harness.create_user("inspection-signer").await?;
    let args = json!({"redemption_rate":"43"});
    let gas = NearGas::from_tgas(5);
    let operation = harness
        .execute(
            &signer,
            tx::Batch {
                receiver_id: ft.clone(),
                actions: vec![
                    ActionInput::Transfer {
                        deposit: NearToken::from_yoctonear(1),
                    },
                    ActionInput::FunctionCall {
                        method_name: ContractMethodName("set_redemption_rate".to_owned()),
                        args: ContractArgs::Json(args.clone()),
                        gas,
                        deposit: NearToken::from_yoctonear(0),
                    },
                ],
            },
        )
        .await?;
    let hash = operation
        .operation
        .latest_tx_hash()
        .context("submitted batch hash")?;
    let client = harness.client()?;
    let request = tx::Get {
        tx_hash: hash,
        sender_account_id: signer.0.clone(),
        wait_until: Some(TxExecutionStatus::Final),
        encoding: tx::ValueEncoding::Json,
    };
    let result = client.read(request.clone()).await?;
    assert_eq!(result.status, tx::Status::Succeeded);
    assert!(result.failed_receipts.is_empty());
    let inspection = result.transaction.context("final transaction inspection")?;
    assert_eq!(inspection.hash, hash);
    assert_eq!(inspection.signer_id, signer.0);
    assert_eq!(inspection.receiver_id, ft);
    assert_eq!(inspection.action_count, 2);
    assert_eq!(
        inspection.function_calls,
        vec![tx::FunctionCallInspection {
            index: 1,
            method_name: ContractMethodName("set_redemption_rate".to_owned()),
            args: serde_json::to_vec(&args)?.into(),
            gas,
            deposit: NearToken::from_yoctonear(0),
        }]
    );
    assert!(inspection.delegate_actions.is_empty());
    let unrelated = harness.create_user("inspection-unrelated").await?;
    assert!(client
        .read(tx::Get {
            sender_account_id: unrelated.0,
            ..request
        })
        .await
        .is_err());
    Ok(())
}

#[tokio::test]
async fn inspected_delegate_hash_verifies_the_chain_accepted_signature_without_flattening_calls(
) -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let ft = harness.deploy_ft("delegate-ft", "Delegate", "DLGT").await?;
    let sender = harness.create_user("delegate-sender").await?;
    let relayer = harness.create_user("delegate-relayer").await?;
    let public_key = test_secret_key()?.public_key();
    let client = harness.client()?;
    let nonce = client
        .read(account::GetAccessKey {
            account_id: sender.0.clone(),
            public_key: public_key.into(),
        })
        .await?
        .nonce
        + 1;
    let block = client
        .read(serde_json::from_value::<chain::GetBlock>(json!({}))?)
        .await?;
    let signed = test_signer()
        .sign_meta(
            PrepopulateTransaction {
                signer_id: sender.0.clone(),
                receiver_id: ft,
                actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                    method_name: "set_redemption_rate".to_owned(),
                    args: serde_json::to_vec(&json!({"redemption_rate":"42"}))?,
                    gas: NearGas::from_tgas(5),
                    deposit: NearToken::from_yoctonear(0),
                }))],
            },
            public_key,
            nonce,
            block.hash.into(),
            block.height + 100,
        )
        .await?;
    let signature = signed.signature.clone();
    let operation = harness
        .execute(
            &relayer,
            tx::RelaySignedDelegateAction {
                signed_delegate_action: signed.into(),
            },
        )
        .await?;
    let hash = operation
        .operation
        .latest_tx_hash()
        .context("submitted delegate hash")?;
    let result = client
        .read(tx::Get {
            tx_hash: hash,
            sender_account_id: relayer.0.clone(),
            wait_until: Some(TxExecutionStatus::Final),
            encoding: tx::ValueEncoding::Json,
        })
        .await?;
    assert_eq!(result.status, tx::Status::Succeeded);
    assert!(result.failed_receipts.is_empty());
    let inspection = result.transaction.context("final delegate inspection")?;
    assert_eq!(inspection.hash, hash);
    assert_eq!(inspection.signer_id, relayer.0);
    assert_eq!(inspection.receiver_id, sender.0);
    assert_eq!(inspection.action_count, 1);
    assert!(inspection.function_calls.is_empty());
    assert_eq!(inspection.delegate_actions.len(), 1);
    let authority = &inspection.delegate_actions[0];
    assert_eq!(authority.index, 0);
    assert!(signature.verify(authority.payload_hash.0, public_key));
    Ok(())
}
