use std::sync::Arc;

use anyhow::Result;
use near_api::{Contract, NetworkConfig, SecretKey, Signer};
use near_token::NearToken;
use templar_gateway_core::{
    DispatchRead, ExecuteOperation, GatewayContext, NearOperationExecutor, NearTransactionSigner,
    PlanWrite, SignTransaction,
};
use templar_gateway_methods_dispatch::Dispatch;
use templar_gateway_methods_spec::{account, tx};
use templar_gateway_types::{
    common::{ContractArgs, WriteRequest},
    ContractMethodName, ManagedAccountId, NearGas,
};
use test_utils::FtController;

#[tokio::test]
async fn core_can_be_used_directly_without_runtime() -> Result<()> {
    let sandbox = near_sandbox::Sandbox::start_sandbox().await?;
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
        FtController::wasm().await.to_vec(),
        "new",
        serde_json::json!({
            "name": "Mock FT",
            "symbol": "MFT",
        }),
    )
    .await?;

    let context = GatewayContext::new(network.clone())?;

    let account = <Dispatch as DispatchRead<account::Get, GatewayContext>>::dispatch(
        account::Get {
            account_id: signer_account_id.0.clone(),
        },
        context.clone(),
    )
    .await?;

    assert_eq!(
        account.code_hash,
        near_api::types::CryptoHash::default().to_string()
    );
    assert_eq!(account.locked, NearToken::from_yoctonear(0));

    let plan = <Dispatch as PlanWrite<tx::FunctionCall, GatewayContext>>::plan(
        WriteRequest {
            signer_account_id: signer_account_id.clone(),
            idempotency_key: None,
            body: tx::FunctionCall {
                receiver_id: ft_contract_id.clone(),
                method_name: ContractMethodName("set_redemption_rate".to_owned()),
                args: ContractArgs::Json(serde_json::json!({
                    "redemption_rate": NearToken::from_near(2).as_yoctonear().to_string(),
                })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::from_yoctonear(0),
            },
        },
        context.clone(),
    )
    .await?;

    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.steps[0].signer_account_id, signer_account_id);
    assert_eq!(plan.steps[0].receiver_id, ft_contract_id);
    assert_eq!(plan.steps[0].actions.len(), 1);

    let transaction_signer = NearTransactionSigner::new(
        network.clone(),
        std::collections::HashMap::from([(signer_account_id.clone(), signer)]),
    );
    let operation_executor = NearOperationExecutor::new(network.clone());
    let prepared = transaction_signer
        .sign_transaction(plan.steps[0].clone())
        .await?;
    let result = operation_executor
        .submit_transaction(prepared.signed_transaction, prepared.transaction.wait_until)
        .await?;

    assert!(result.is_success());

    let rate: near_api::Data<String> = Contract(ft_contract_id)
        .call_function("redemption_rate", ())
        .read_only()
        .fetch_from(&network)
        .await?;
    assert_eq!(
        rate.data,
        NearToken::from_near(2).as_yoctonear().to_string()
    );

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
        .send_to(network)
        .await?
        .assert_success();
    Ok(())
}

fn test_secret_key() -> Result<SecretKey> {
    Ok("ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q"
        .parse()?)
}
