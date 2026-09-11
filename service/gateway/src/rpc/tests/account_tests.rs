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

#[tokio::test]
async fn account_add_key_endpoint_installs_a_full_access_key() -> Result<()> {
    let stack = TestStack::start().await?;
    let public_key = templar_gateway_types::primitive::PublicKey::from(
        near_api::signer::generate_secret_key()?.public_key(),
    );

    let _ = stack
        .controller
        .request::<account::AddKey>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: account::AddKey {
                public_key: public_key.clone(),
            },
        })
        .await?;

    let key = stack
        .controller
        .request::<account::GetAccessKey>(&account::GetAccessKey {
            account_id: stack.harness.gateway_signer_account_id.0.clone(),
            public_key,
        })
        .await?;

    assert_eq!(key.permission, account::AccessKeyPermission::FullAccess);

    stack.shutdown().await;
    Ok(())
}
