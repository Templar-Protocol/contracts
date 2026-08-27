use std::sync::Arc;

use anyhow::Result;
use near_api::{Contract, NetworkConfig, SecretKey, Signer};
use near_token::NearToken;
use templar_gateway_core::{
    ExecuteOperation, FinalityPolicy, GatewayContext, NearClient, NearOperationExecutor,
    NearTransactionSigner, PlanWrite, PooledSigner, SignTransaction,
};
use templar_gateway_methods_dispatch::Dispatch;
use templar_gateway_methods_spec::tx;
use templar_gateway_testing::{wasm as testing_wasm, TEST_FINALITY_POLICY};
use templar_gateway_types::{
    common::{ContractArgs, WriteRequest},
    ContractMethodName, ManagedAccountId, NearGas,
};

#[allow(
    clippy::too_many_lines,
    reason = "the integration test exercises all immediate-read finality paths"
)]
#[tokio::test]
async fn core_finality_policies_keep_immediate_reads_consistent() -> Result<()> {
    // Share the harness's launch config so this owned node runs the same block
    // cadence as the rest of the gate rather than the near-sandbox default.
    let sandbox =
        near_sandbox::Sandbox::start_sandbox_with_config(templar_gateway_testing::sandbox_config())
            .await?;
    let network = NetworkConfig::from_rpc_url("sandbox", sandbox.rpc_addr.parse()?);

    let signer_account_id = ManagedAccountId("library-user.near".parse()?);
    let signer =
        create_account_signer(&sandbox, &signer_account_id.0, NearToken::from_near(25)).await?;

    let ft_contract_id = "mock-ft.near".parse()?;
    let ft_signer =
        create_account_signer(&sandbox, &ft_contract_id, NearToken::from_near(25)).await?;
    deploy_contract(
        &network,
        ft_contract_id.clone(),
        ft_signer,
        testing_wasm::ft().await.to_vec(),
        "new",
        serde_json::json!({
            "name": "Mock FT",
            "symbol": "MFT",
        }),
    )
    .await?;

    let near = NearClient::with_finality_policy(network.clone(), TEST_FINALITY_POLICY);
    assert!(!near
        .contract(ft_contract_id.clone())
        .code()
        .await?
        .is_empty());
    assert!(!near
        .contract(ft_contract_id.clone())
        .state_with_prefix(Vec::new())
        .await?
        .is_empty());

    let limits = near.chain().protocol_limits().await?;
    assert!(limits.max_transaction_size > 0);
    assert!(limits.max_total_prepaid_gas.as_gas() > 0);

    let transaction_signer = NearTransactionSigner::new(
        network.clone(),
        std::collections::HashMap::from([(
            signer_account_id.clone(),
            PooledSigner::from_signer(signer_account_id.clone(), signer).await?,
        )]),
    );

    for (finality_policy, rate) in [
        (FinalityPolicy::Executed, 2),
        (FinalityPolicy::Final, 3),
        (FinalityPolicy::ExecutedOptimistic, 4),
    ] {
        let near = NearClient::with_finality_policy(network.clone(), finality_policy);
        let context = GatewayContext::from_near_client(near.clone());
        let plan = <Dispatch as PlanWrite<tx::FunctionCall, GatewayContext>>::plan(
            WriteRequest {
                signer_account_id: signer_account_id.clone(),
                idempotency_key: None,
                body: tx::FunctionCall {
                    receiver_id: ft_contract_id.clone(),
                    method_name: ContractMethodName("set_redemption_rate".to_owned()),
                    args: ContractArgs::Json(serde_json::json!({
                        "redemption_rate": NearToken::from_near(rate)
                            .as_yoctonear()
                            .to_string(),
                    })),
                    gas: NearGas::from_tgas(100),
                    deposit: NearToken::from_yoctonear(0),
                },
            },
            context,
        )
        .await?;

        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].signer_account_id, signer_account_id);
        assert_eq!(plan.steps[0].receiver_id, ft_contract_id);
        assert_eq!(plan.steps[0].actions.len(), 1);

        let operation_executor =
            NearOperationExecutor::with_finality_policy(network.clone(), finality_policy);
        let lease = transaction_signer
            .lease_next_signing_key(&signer_account_id)
            .await?;
        let prepared = transaction_signer
            .sign_transaction(&lease, plan.steps[0].clone())
            .await?;
        let result = operation_executor
            .submit_transaction(prepared.signed_transaction)
            .await?;

        assert!(
            result
                .expect("submission should carry a full outcome")
                .is_success
        );

        let observed: String = near
            .contract(ft_contract_id.clone())
            .view_function(
                "redemption_rate",
                serde_json::to_vec(&serde_json::json!({}))?,
            )
            .await?;
        assert_eq!(
            observed,
            NearToken::from_near(rate).as_yoctonear().to_string()
        );
    }

    Ok(())
}

async fn create_account_signer(
    sandbox: &near_sandbox::Sandbox,
    account_id: &near_api::types::AccountId,
    initial_balance: NearToken,
) -> Result<Arc<Signer>> {
    let secret_key = test_secret_key()?;
    sandbox
        .create_account(account_id.clone())
        .initial_balance(initial_balance)
        .public_key(secret_key.public_key().to_string())
        .send()
        .await?;
    Ok(Signer::from_secret_key(secret_key)?)
}

async fn deploy_contract(
    network: &NetworkConfig,
    account_id: near_api::types::AccountId,
    signer: Arc<Signer>,
    code: Vec<u8>,
    init_method: &str,
    init_args: impl serde::Serialize,
) -> Result<()> {
    Contract::deploy(account_id)
        .use_code(code)
        .with_init_call(init_method, init_args)?
        .with_signer(signer)
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(network)
        .await?
        .assert_success();
    Ok(())
}

fn test_secret_key() -> Result<SecretKey> {
    Ok("ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q"
        .parse()?)
}
