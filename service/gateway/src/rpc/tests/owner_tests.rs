use super::*;

fn assert_signer_mismatch(
    error: impl std::fmt::Display,
    signer_id: &str,
    contract_id: &str,
    required_role: &str,
) {
    let message = error.to_string();
    for expected in [signer_id, contract_id, required_role] {
        assert!(
            message.contains(expected),
            "owner signer mismatch error must contain `{expected}`; got: {message}"
        );
    }
}

#[tokio::test]
async fn owner_endpoints_work_against_registry_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let contract_id = stack.harness.deploy_registry().await?;

    let current = stack
        .controller
        .request::<owner::GetOwner>(&owner::GetOwner {
            contract_id: contract_id.clone(),
        })
        .await?;
    assert_eq!(
        current.owner,
        Some(stack.harness.registry_signer_account_id.0.clone())
    );

    let error = stack
        .controller
        .request::<owner::ProposeOwner>(&WriteRequest {
            signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
            idempotency_key: None,
            body: owner::ProposeOwner {
                contract_id: contract_id.clone(),
                account_id: Some(stack.harness.gateway_signer_account_id.0.clone()),
            },
        })
        .await
        .expect_err("a non-owner must be rejected before proposing an owner");
    assert_signer_mismatch(
        error,
        stack.harness.cleanup_signer_account_id.0.as_str(),
        contract_id.as_str(),
        "current owner",
    );

    let error = stack
        .controller
        .request::<owner::RenounceOwner>(&WriteRequest {
            signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
            idempotency_key: None,
            body: owner::RenounceOwner {
                contract_id: contract_id.clone(),
            },
        })
        .await
        .expect_err("a non-owner must be rejected before renouncing ownership");
    assert_signer_mismatch(
        error,
        stack.harness.cleanup_signer_account_id.0.as_str(),
        contract_id.as_str(),
        "current owner",
    );

    let _ = stack
        .controller
        .request::<owner::ProposeOwner>(&WriteRequest {
            signer_account_id: stack.harness.registry_signer_account_id.clone(),
            idempotency_key: None,
            body: owner::ProposeOwner {
                contract_id: contract_id.clone(),
                account_id: Some(stack.harness.cleanup_signer_account_id.0.clone()),
            },
        })
        .await?;

    let proposed = stack
        .controller
        .request::<owner::GetProposedOwner>(&owner::GetProposedOwner {
            contract_id: contract_id.clone(),
        })
        .await?;
    assert_eq!(
        proposed.proposed_owner,
        Some(stack.harness.cleanup_signer_account_id.0.clone())
    );

    let error = stack
        .controller
        .request::<owner::AcceptOwner>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: owner::AcceptOwner {
                contract_id: contract_id.clone(),
            },
        })
        .await
        .expect_err("an account other than the proposed owner must be rejected before acceptance");
    assert_signer_mismatch(
        error,
        stack.harness.gateway_signer_account_id.0.as_str(),
        contract_id.as_str(),
        "proposed owner",
    );

    let _ = stack
        .controller
        .request::<owner::AcceptOwner>(&WriteRequest {
            signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
            idempotency_key: None,
            body: owner::AcceptOwner {
                contract_id: contract_id.clone(),
            },
        })
        .await?;

    let current = stack
        .controller
        .request::<owner::GetOwner>(&owner::GetOwner {
            contract_id: contract_id.clone(),
        })
        .await?;
    assert_eq!(
        current.owner,
        Some(stack.harness.cleanup_signer_account_id.0.clone())
    );

    let _ = stack
        .controller
        .request::<owner::RenounceOwner>(&WriteRequest {
            signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
            idempotency_key: None,
            body: owner::RenounceOwner {
                contract_id: contract_id.clone(),
            },
        })
        .await?;

    let current = stack
        .controller
        .request::<owner::GetOwner>(&owner::GetOwner { contract_id })
        .await?;
    assert_eq!(current.owner, None);

    stack.shutdown().await;
    Ok(())
}
