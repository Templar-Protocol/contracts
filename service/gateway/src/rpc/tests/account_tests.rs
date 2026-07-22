use super::*;

#[tokio::test]
async fn account_get_endpoint_works_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;

    let result = stack
        .controller
        .request::<account::Get>(&account::Get {
            account_id: stack.harness.gateway_signer_account_id.0.clone(),
        })
        .await?;

    assert!(result.amount.as_yoctonear() > 0);
    assert!(result.storage_usage > 0);
    assert_eq!(
        result.code_hash,
        near_api::types::CryptoHash::default().to_string()
    );
    assert_eq!(result.locked, NearToken::from_yoctonear(0));

    stack.shutdown().await;
    Ok(())
}
