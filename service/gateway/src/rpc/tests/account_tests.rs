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
async fn account_add_key_and_delete_key_endpoints_manage_both_permissions() -> Result<()> {
    let stack = TestStack::start().await?;
    let account_id = stack.harness.gateway_signer_account_id.clone();
    let fresh_key = || -> Result<templar_gateway_types::primitive::PublicKey> {
        Ok(near_api::signer::generate_secret_key()?.public_key().into())
    };
    let full_access = fresh_key()?;
    let function_call = fresh_key()?;
    let restricted = account::AccessKeyPermission::FunctionCall {
        allowance: Some(NearToken::from_near(1)),
        receiver_id: stack.harness.ft_contract_id.clone(),
        method_names: vec![ContractMethodName("ft_transfer".to_owned())],
    };

    for (public_key, permission) in [
        (
            full_access.clone(),
            account::AccessKeyPermission::FullAccess,
        ),
        (function_call.clone(), restricted.clone()),
    ] {
        let _ = stack
            .controller
            .request::<account::AddKey>(&WriteRequest {
                signer_account_id: account_id.clone(),
                idempotency_key: None,
                body: account::AddKey {
                    public_key,
                    permission,
                },
            })
            .await?;
    }

    let installed = |public_key: templar_gateway_types::primitive::PublicKey| async {
        stack
            .controller
            .request::<account::GetAccessKey>(&account::GetAccessKey {
                account_id: account_id.0.clone(),
                public_key,
            })
            .await
    };
    assert_eq!(
        installed(full_access.clone()).await?.permission,
        account::AccessKeyPermission::FullAccess
    );
    assert_eq!(
        installed(function_call.clone()).await?.permission,
        restricted
    );

    // No transaction lands between the two reads, so both see the same keys.
    let listed_in_chain_order = || async {
        let listed = stack
            .controller
            .request::<account::ListAccessKeys>(&account::ListAccessKeys {
                account_id: account_id.0.clone(),
            })
            .await?
            .keys;
        let chain = near_api::Account(account_id.0.clone())
            .list_keys()
            .at(TEST_FINALITY_POLICY.query_reference())
            .fetch_from(&stack.harness.network)
            .await?
            .data;
        assert_eq!(
            listed
                .iter()
                .map(|key| (key.public_key.clone(), key.nonce))
                .collect::<Vec<_>>(),
            chain
                .into_iter()
                .map(|(public_key, key)| (public_key.into(), key.nonce.0))
                .collect::<Vec<_>>(),
        );
        Ok::<_, anyhow::Error>(listed)
    };
    let permission_of = |listed: &[account::AccessKeyEntry], public_key| {
        listed
            .iter()
            .find(|key| key.public_key == public_key)
            .map(|key| key.permission.clone())
    };
    let before_delete = listed_in_chain_order().await?;
    assert_eq!(
        permission_of(&before_delete, full_access.clone()),
        Some(account::AccessKeyPermission::FullAccess)
    );
    assert_eq!(
        permission_of(&before_delete, function_call.clone()),
        Some(restricted.clone())
    );

    let _ = stack
        .controller
        .request::<account::DeleteKey>(&WriteRequest {
            signer_account_id: account_id.clone(),
            idempotency_key: None,
            body: account::DeleteKey {
                public_key: function_call.clone(),
            },
        })
        .await?;

    assert!(installed(function_call.clone()).await.is_err());
    assert_eq!(
        permission_of(&listed_in_chain_order().await?, function_call),
        None
    );
    assert_eq!(
        installed(full_access).await?.permission,
        account::AccessKeyPermission::FullAccess
    );

    stack.shutdown().await;
    Ok(())
}
