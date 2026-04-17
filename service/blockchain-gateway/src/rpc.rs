use blockchain_gateway_core::{
    account, contract, ft, market, registry, storage, tx, universal_account,
};
use blockchain_gateway_near::{
    actor::{DispatchRead, DispatchWrite},
    GatewayError, GatewayService,
};
use jsonrpsee::{
    core::{RegisterMethodError, RpcResult},
    types::ErrorObjectOwned,
    RpcModule,
};

const GATEWAY_SERVER_ERROR_CODE: i32 = -32000;

#[allow(clippy::needless_pass_by_value)]
fn map_gateway_error(error: GatewayError) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(GATEWAY_SERVER_ERROR_CODE, error.to_string(), None::<()>)
}

fn register_write<Spec: DispatchWrite>(
    module: &mut RpcModule<GatewayService>,
) -> Result<(), RegisterMethodError> {
    module.register_async_method(Spec::RPC_METHOD, move |params, service, _| async move {
        let params: Spec::Input = params.parse()?;
        let result = service
            .request_write::<Spec>(params)
            .await
            .map_err(map_gateway_error)?;
        RpcResult::Ok(result)
    })?;

    Ok(())
}

fn register_read<Spec: DispatchRead>(
    module: &mut RpcModule<GatewayService>,
) -> Result<(), RegisterMethodError> {
    module.register_async_method(Spec::RPC_METHOD, move |params, service, _| async move {
        let params: Spec::Input = params.parse()?;
        let result = service
            .request_read::<Spec>(params)
            .await
            .map_err(map_gateway_error)?;
        RpcResult::Ok(result)
    })?;

    Ok(())
}

pub fn attach_gateway(
    service: GatewayService,
) -> Result<RpcModule<GatewayService>, RegisterMethodError> {
    let mut m = RpcModule::new(service);

    register_read::<account::Get>(&mut m)?;
    register_write::<account::Delete>(&mut m)?;
    register_read::<contract::ViewFunction>(&mut m)?;
    register_read::<contract::GetVersion>(&mut m)?;
    register_read::<ft::GetBalanceOf>(&mut m)?;
    register_write::<ft::Transfer>(&mut m)?;
    register_read::<market::GetConfiguration>(&mut m)?;
    register_read::<market::ListBorrowPositions>(&mut m)?;
    register_read::<registry::GetDeployment>(&mut m)?;
    register_read::<registry::ListDeployments>(&mut m)?;
    register_read::<registry::ListVersions>(&mut m)?;
    register_write::<registry::AddVersion>(&mut m)?;
    register_write::<registry::RemoveVersion>(&mut m)?;
    register_write::<registry::Deploy>(&mut m)?;
    register_read::<storage::GetBalanceBounds>(&mut m)?;
    register_read::<storage::GetBalanceOf>(&mut m)?;
    register_write::<storage::Deposit>(&mut m)?;
    register_write::<storage::EnsureDeposit>(&mut m)?;
    register_write::<storage::Unregister>(&mut m)?;
    register_read::<tx::Get>(&mut m)?;
    register_write::<tx::FunctionCall>(&mut m)?;
    register_read::<universal_account::GetKey>(&mut m)?;
    register_write::<universal_account::Execute>(&mut m)?;
    register_write::<universal_account::CreateAccount>(&mut m)?;

    Ok(m)
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use blockchain_gateway_core::{
        account,
        common::{ContractArgs, ReadRequest, WriteRequest},
        contract, ft, storage, tx, Base64Bytes, ContractMethodName, CryptoHash, NearGas, NearToken,
    };
    use blockchain_gateway_testing::{SandboxHarness, TestController};
    use jsonrpsee::server::{ServerBuilder, ServerHandle};
    use templar_universal_account::{
        KeyParameters, NEAR_TESTNET_CHAIN_ID,
        authentication::Payload,
        authentication::with_raw_string::WithRawString,
        transaction::{FunctionCallAction, Transaction},
    };

    use super::*;
    struct TestStack {
        harness: SandboxHarness,
        gateway: GatewayService,
        handle: ServerHandle,
        controller: TestController,
    }

    impl TestStack {
        async fn start() -> Result<Self> {
            let harness = SandboxHarness::start().await?;
            let gateway =
                GatewayService::spawn(harness.gateway_client(), harness.gateway_signers.clone());

            let server = ServerBuilder::default().build("127.0.0.1:0").await?;
            let local_addr = server.local_addr()?;
            let module = attach_gateway(gateway.clone())?;
            let handle = server.start(module);
            let controller = TestController::new(format!("http://{local_addr}"));

            Ok(Self {
                harness,
                gateway,
                handle,
                controller,
            })
        }

        async fn shutdown(self) {
            self.handle
                .stop()
                .expect("gateway test server should stop cleanly");
            self.handle.stopped().await;
            self.gateway.shutdown().await;
        }
    }

    async fn register_gateway_signer_for_ft(
        stack: &TestStack,
    ) -> Result<storage::GetBalanceBoundsResult> {
        let bounds = stack
            .controller
            .request::<storage::GetBalanceBounds>(&ReadRequest {
                params: storage::GetBalanceBoundsParams {
                    contract_id: stack.harness.ft_contract_id.clone(),
                },
            })
            .await?;

        let _ = stack
            .controller
            .request::<storage::Deposit>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: storage::DepositBody {
                    contract_id: stack.harness.ft_contract_id.clone(),
                    beneficiary_id: None,
                    registration_only: false,
                    deposit: NearToken::from_near(1),
                },
            })
            .await?;

        Ok(bounds)
    }

    fn tx_hash(result: &blockchain_gateway_core::common::WriteOperationResult) -> CryptoHash {
        let hash = result.outcome.operation.steps[0]
            .tx_hash
            .clone()
            .expect("transaction hash should be present for final execution");
        CryptoHash(hash.parse().expect("transaction hash should parse"))
    }

    #[tokio::test]
    async fn chain_view_account_endpoint_works_against_sandbox() -> Result<()> {
        let stack = TestStack::start().await?;

        let result = stack
            .controller
            .request::<account::Get>(&ReadRequest {
                params: account::GetParams {
                    account_id: stack.harness.gateway_signer_account_id.0.clone(),
                },
            })
            .await?;

        assert!(result.amount.as_yoctonear() > 0);
        assert!(result.storage_usage > 0);

        stack.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn tx_function_call_and_view_function_endpoints_work_against_sandbox() -> Result<()> {
        let stack = TestStack::start().await?;

        let _ = stack
            .controller
            .request::<tx::FunctionCall>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: tx::FunctionCallBody {
                    receiver_id: stack.harness.ft_contract_id.clone(),
                    method_name: ContractMethodName("set_redemption_rate".to_owned()),
                    args: ContractArgs::Json(serde_json::json!({
                        "redemption_rate": NearToken::from_near(2).as_yoctonear().to_string(),
                    })),
                    gas: NearGas::from_tgas(100),
                    deposit: NearToken::from_yoctonear(0),
                },
            })
            .await?;

        let counter = stack
            .controller
            .request::<contract::ViewFunction>(&ReadRequest {
                params: contract::ViewFunctionParams {
                    contract_id: stack.harness.ft_contract_id.clone(),
                    method_name: ContractMethodName("redemption_rate".to_owned()),
                    args: ContractArgs::Raw(Base64Bytes(Vec::new())),
                },
            })
            .await?;

        assert_eq!(
            counter.value,
            serde_json::json!(NearToken::from_near(2).as_yoctonear().to_string())
        );

        stack.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn storage_and_get_transaction_endpoints_work_against_sandbox() -> Result<()> {
        let stack = TestStack::start().await?;

        let _bounds = register_gateway_signer_for_ft(&stack).await?;

        let balance_before = stack
            .controller
            .request::<storage::GetBalanceOf>(&ReadRequest {
                params: storage::GetBalanceOfParams {
                    contract_id: stack.harness.ft_contract_id.clone(),
                    account_id: stack.harness.gateway_signer_account_id.0.clone(),
                },
            })
            .await?;

        assert!(balance_before.balance.is_some());

        let beneficiary_deposit = stack
            .controller
            .request::<storage::Deposit>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: storage::DepositBody {
                    contract_id: stack.harness.ft_contract_id.clone(),
                    beneficiary_id: Some(stack.harness.beneficiary_account_id.clone()),
                    registration_only: true,
                    deposit: NearToken::from_near(1),
                },
            })
            .await?;

        let beneficiary_balance = stack
            .controller
            .request::<storage::GetBalanceOf>(&ReadRequest {
                params: storage::GetBalanceOfParams {
                    contract_id: stack.harness.ft_contract_id.clone(),
                    account_id: stack.harness.beneficiary_account_id.clone(),
                },
            })
            .await?;

        assert!(beneficiary_balance.balance.is_some());

        let deposit_transaction = stack
            .controller
            .request::<tx::Get>(&ReadRequest {
                params: tx::GetParams {
                    tx_hash: tx_hash(&beneficiary_deposit),
                    sender_account_id: stack.harness.gateway_signer_account_id.0.clone(),
                    wait_until: None,
                    encoding: tx::ValueEncoding::Json,
                },
            })
            .await?;

        assert_eq!(deposit_transaction.status, tx::Status::Succeeded);
        assert!(matches!(
            deposit_transaction.return_value,
            Some(tx::ReturnValue::Json(_))
        ));

        let mint_transaction = stack
            .controller
            .request::<tx::FunctionCall>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: tx::FunctionCallBody {
                    receiver_id: stack.harness.ft_contract_id.clone(),
                    method_name: ContractMethodName("mint".to_owned()),
                    args: ContractArgs::Json(serde_json::json!({ "amount": "1" })),
                    gas: NearGas::from_tgas(100),
                    deposit: NearToken::from_yoctonear(0),
                },
            })
            .await?;

        let mint_status = stack
            .controller
            .request::<tx::Get>(&ReadRequest {
                params: tx::GetParams {
                    tx_hash: tx_hash(&mint_transaction),
                    sender_account_id: stack.harness.gateway_signer_account_id.0.clone(),
                    wait_until: None,
                    encoding: tx::ValueEncoding::Json,
                },
            })
            .await?;

        assert_eq!(mint_status.status, tx::Status::Succeeded);

        let transfer_transaction = stack
            .controller
            .request::<tx::FunctionCall>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: tx::FunctionCallBody {
                    receiver_id: stack.harness.ft_contract_id.clone(),
                    method_name: ContractMethodName("ft_transfer".to_owned()),
                    args: ContractArgs::Json(serde_json::json!({
                        "receiver_id": stack.harness.beneficiary_account_id,
                        "amount": "1",
                    })),
                    gas: NearGas::from_tgas(100),
                    deposit: NearToken::from_yoctonear(1),
                },
            })
            .await?;

        let transfer_result = stack
            .controller
            .request::<tx::Get>(&ReadRequest {
                params: tx::GetParams {
                    tx_hash: tx_hash(&transfer_transaction),
                    sender_account_id: stack.harness.gateway_signer_account_id.0.clone(),
                    wait_until: None,
                    encoding: tx::ValueEncoding::Json,
                },
            })
            .await?;

        assert_eq!(transfer_result.status, tx::Status::Succeeded);
        assert!(!transfer_result.logs.is_empty());

        let unregister_transaction = stack
            .controller
            .request::<tx::FunctionCall>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: tx::FunctionCallBody {
                    receiver_id: stack.harness.ft_contract_id.clone(),
                    method_name: ContractMethodName("patch_storage_unregister".to_owned()),
                    args: ContractArgs::Json(serde_json::json!({ "force": false })),
                    gas: NearGas::from_tgas(100),
                    deposit: NearToken::from_yoctonear(1),
                },
            })
            .await?;

        let unregister_result = stack
            .controller
            .request::<tx::Get>(&ReadRequest {
                params: tx::GetParams {
                    tx_hash: tx_hash(&unregister_transaction),
                    sender_account_id: stack.harness.gateway_signer_account_id.0.clone(),
                    wait_until: None,
                    encoding: tx::ValueEncoding::Base64,
                },
            })
            .await?;

        assert_eq!(unregister_result.status, tx::Status::Succeeded);
        assert!(matches!(
            unregister_result.return_value,
            Some(tx::ReturnValue::Base64(_))
        ));

        stack.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn storage_ensure_deposit_endpoint_supports_noop_and_operation() -> Result<()> {
        let stack = TestStack::start().await?;

        let first = stack
            .controller
            .request::<storage::EnsureDeposit>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: storage::EnsureDepositBody {
                    contract_id: stack.harness.ft_contract_id.clone(),
                    account_id: stack.harness.beneficiary_account_id.clone(),
                    mode: storage::EnsureDepositMode::Registered,
                },
            })
            .await?;

        assert!(matches!(first, storage::EnsureDepositResult::Operation(_)));

        let second = stack
            .controller
            .request::<storage::EnsureDeposit>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: storage::EnsureDepositBody {
                    contract_id: stack.harness.ft_contract_id.clone(),
                    account_id: stack.harness.beneficiary_account_id.clone(),
                    mode: storage::EnsureDepositMode::Registered,
                },
            })
            .await?;

        assert!(matches!(second, storage::EnsureDepositResult::NoOp));

        stack.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn registry_endpoints_work_against_sandbox() -> Result<()> {
        let stack = TestStack::start().await?;
        let registry_id = stack.harness.deploy_registry().await?;

        let version_key = "mock-ft@1.0.0".to_owned();
        let write_result = stack
            .controller
            .request::<registry::AddVersion>(&WriteRequest {
                signer_account_id: stack.harness.registry_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: registry::AddVersionBody {
                    registry_id: registry_id.clone(),
                    version_key: version_key.clone(),
                    deploy_mode: templar_common::registry::DeployMode::Normal,
                    code: Base64Bytes(stack.harness.ft_wasm().await),
                    deposit: NearToken::from_yoctonear(1),
                },
            })
            .await?;
        eprintln!("{write_result:?}");

        let versions = stack
            .controller
            .request::<registry::ListVersions>(&ReadRequest {
                params: registry::ListVersionsParams {
                    registry_id: registry_id.clone(),
                    args: blockchain_gateway_core::common::Pagination::default(),
                },
            })
            .await?;

        assert_eq!(versions.values, vec![version_key.clone()]);

        let deploy = stack
            .controller
            .request::<registry::Deploy>(&WriteRequest {
                signer_account_id: stack.harness.registry_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: registry::DeployBody {
                    registry_id: registry_id.clone(),
                    name: "deployed-ft".to_owned(),
                    version_key: version_key.clone(),
                    init_args: Base64Bytes(serde_json::to_vec(&serde_json::json!({
                        "name": "Deployed FT",
                        "symbol": "DFT",
                    }))?),
                    full_access_keys: None,
                    deposit: NearToken::from_near(6),
                },
            })
            .await?;

        let deployed_account_id: near_account_id::AccountId =
            format!("deployed-ft.{}", registry_id.0)
                .parse()
                .expect("deployed registry subaccount should be valid");

        let deployment = stack
            .controller
            .request::<registry::GetDeployment>(&ReadRequest {
                params: registry::GetDeploymentParams {
                    registry_id: registry_id.clone(),
                    account_id: deployed_account_id.clone(),
                },
            })
            .await?;

        let deployments = stack
            .controller
            .request::<registry::ListDeployments>(&ReadRequest {
                params: registry::ListDeploymentsParams {
                    registry_id: registry_id.clone(),
                    args: blockchain_gateway_core::common::Pagination::default(),
                },
            })
            .await?;

        let version = stack
            .controller
            .request::<contract::GetVersion>(&ReadRequest {
                params: contract::GetVersionParams {
                    contract_id: deployed_account_id,
                },
            })
            .await?;

        let _ = stack
            .controller
            .request::<registry::RemoveVersion>(&WriteRequest {
                signer_account_id: stack.harness.registry_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: registry::RemoveVersionBody {
                    registry_id: registry_id.clone(),
                    version_key: version_key.clone(),
                },
            })
            .await?;

        assert_eq!(
            deployments.account_ids,
            vec![format!("deployed-ft.{}", registry_id.0).parse::<near_account_id::AccountId>()?]
        );
        assert!(deployment.deployment.is_some());
        assert!(!version.version_string.is_empty());
        assert_eq!(
            deploy.outcome.operation.status,
            blockchain_gateway_core::OperationStatus::Succeeded
        );

        stack.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn market_endpoints_work_against_sandbox() -> Result<()> {
        let stack = TestStack::start().await?;
        let (market_id, configuration) = stack.harness.deploy_market().await?;

        let returned_configuration = stack
            .controller
            .request::<market::GetConfiguration>(&ReadRequest {
                params: market::GetConfigurationParams {
                    market_id: market_id.clone(),
                },
            })
            .await?;

        let borrow_positions = stack
            .controller
            .request::<market::ListBorrowPositions>(&ReadRequest {
                params: market::ListBorrowPositionsParams {
                    market_id,
                    args: blockchain_gateway_core::common::Pagination::default(),
                },
            })
            .await?;

        assert_eq!(returned_configuration, configuration);
        assert!(borrow_positions.positions.is_empty());

        stack.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn universal_account_get_key_endpoint_works_against_sandbox() -> Result<()> {
        let stack = TestStack::start().await?;
        let (account_id, signer) = stack.harness.deploy_universal_account().await?;

        let result = stack
            .controller
            .request::<universal_account::GetKey>(&ReadRequest {
                params: universal_account::GetKeyParams {
                    account_id,
                    key: signer.id(),
                },
            })
            .await?;

        assert!(result.parameters.is_some());

        stack.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn universal_account_write_endpoints_work_against_sandbox() -> Result<()> {
        let stack = TestStack::start().await?;
        let (account_id, signer) = stack.harness.deploy_universal_account().await?;

        let key = stack
            .controller
            .request::<universal_account::GetKey>(&ReadRequest {
                params: universal_account::GetKeyParams {
                    account_id: account_id.clone(),
                    key: signer.id(),
                },
            })
            .await?
            .parameters
            .expect("deployed universal account should expose its key parameters");

        let payload = WithRawString::from_parsed(Payload::new(
            templar_universal_account::PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
                .with_key_parameters(KeyParameters {
                    block_height: key.block_height.into(),
                    index: key.index.into(),
                    nonce: (key.nonce + 1).into(),
                })
                .verifying_contract(account_id.0.clone())
                .build_salt(),
            vec![Transaction {
                receiver_id: stack.harness.ft_contract_id.clone(),
                actions: vec![FunctionCallAction::new(
                    "increment",
                    b"{}",
                    NearToken::from_near(0),
                    near_sdk::Gas::from_tgas(3),
                )
                .into()]
                .into(),
            }]
            .into(),
        ));

        let _ = stack
            .controller
            .request::<universal_account::Execute>(&WriteRequest {
                signer_account_id: stack.harness.universal_account_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: universal_account::ExecuteBody {
                    account_id: account_id.clone(),
                    args: signer.execute_args(payload),
                },
            })
            .await?;

        let counter = stack
            .controller
            .request::<contract::ViewFunction>(&ReadRequest {
                params: contract::ViewFunctionParams {
                    contract_id: stack.harness.ft_contract_id.clone(),
                    method_name: ContractMethodName("get_counter".to_owned()),
                    args: ContractArgs::Json(serde_json::json!({
                        "account_id": account_id.0,
                    })),
                },
            })
            .await?;

        assert_eq!(counter.value, serde_json::json!(1));

        let registry_id = stack.harness.deploy_registry().await?;
        let _ = stack
            .controller
            .request::<registry::AddVersion>(&WriteRequest {
                signer_account_id: stack.harness.registry_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: registry::AddVersionBody {
                    registry_id: registry_id.clone(),
                    version_key: "ua@1.0.0".to_owned(),
                    deploy_mode: templar_common::registry::DeployMode::Normal,
                    code: Base64Bytes(test_utils::UniversalAccountController::wasm().await.to_vec()),
                    deposit: NearToken::from_yoctonear(1),
                },
            })
            .await?;

        let create = stack
            .controller
            .request::<universal_account::CreateAccount>(&WriteRequest {
                signer_account_id: stack.harness.registry_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: universal_account::CreateAccountBody {
                    registry_id: registry_id.clone(),
                    account_name: "ua-created".to_owned(),
                    version_key: "ua@1.0.0".to_owned(),
                    key: signer.id(),
                    chain_id: blockchain_gateway_core::U128(NEAR_TESTNET_CHAIN_ID),
                    execute: None,
                    full_access_keys: None,
                    deposit: NearToken::from_near(20),
                },
            })
            .await?;

        assert_eq!(
            create.outcome.operation.status,
            blockchain_gateway_core::OperationStatus::Succeeded
        );

        let created_account_id: near_account_id::AccountId = format!("ua-created.{}", registry_id.0)
            .parse()
            .expect("created universal account id should be valid");

        let created_key = stack
            .controller
            .request::<universal_account::GetKey>(&ReadRequest {
                params: universal_account::GetKeyParams {
                    account_id: blockchain_gateway_core::UniversalAccountId(created_account_id),
                    key: signer.id(),
                },
            })
            .await?;

        assert!(created_key.parameters.is_some());

        stack.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn tx_transfer_unregister_and_account_delete_endpoints_work_against_sandbox() -> Result<()>
    {
        let stack = TestStack::start().await?;

        let _ = register_gateway_signer_for_ft(&stack).await?;

        let _ = stack
            .controller
            .request::<storage::Deposit>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: storage::DepositBody {
                    contract_id: stack.harness.ft_contract_id.clone(),
                    beneficiary_id: Some(stack.harness.beneficiary_account_id.clone()),
                    registration_only: false,
                    deposit: NearToken::from_near(1),
                },
            })
            .await?;

        let _ = stack
            .controller
            .request::<tx::FunctionCall>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: tx::FunctionCallBody {
                    receiver_id: stack.harness.ft_contract_id.clone(),
                    method_name: ContractMethodName("mint".to_owned()),
                    args: ContractArgs::Json(serde_json::json!({ "amount": "3" })),
                    gas: NearGas::from_tgas(100),
                    deposit: NearToken::from_yoctonear(0),
                },
            })
            .await?;

        let _ = stack
            .controller
            .request::<ft::Transfer>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: ft::TransferBody {
                    contract_id: stack.harness.ft_contract_id.clone(),
                    receiver_id: stack.harness.beneficiary_account_id.clone(),
                    amount: blockchain_gateway_core::U128(3),
                },
            })
            .await?;

        let balance = stack
            .controller
            .request::<ft::GetBalanceOf>(&ReadRequest {
                params: ft::GetBalanceOfParams {
                    contract_id: stack.harness.ft_contract_id.clone(),
                    account_id: stack.harness.gateway_signer_account_id.0.clone(),
                },
            })
            .await?;

        assert_eq!(balance.balance, blockchain_gateway_core::U128(0));

        let _ = stack
            .controller
            .request::<storage::Unregister>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: storage::UnregisterBody {
                    contract_id: stack.harness.ft_contract_id.clone(),
                    force: false,
                },
            })
            .await?;

        let storage_balance = stack
            .controller
            .request::<storage::GetBalanceOf>(&ReadRequest {
                params: storage::GetBalanceOfParams {
                    contract_id: stack.harness.ft_contract_id.clone(),
                    account_id: stack.harness.gateway_signer_account_id.0.clone(),
                },
            })
            .await?;

        assert!(storage_balance.balance.is_none());

        let _ = stack
            .controller
            .request::<account::Delete>(&WriteRequest {
                signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: account::DeleteBody {
                    beneficiary_id: stack.harness.beneficiary_account_id.clone(),
                },
            })
            .await?;

        let deleted = stack
            .controller
            .request::<tx::Get>(&ReadRequest {
                params: tx::GetParams {
                    tx_hash: CryptoHash(
                        "11111111111111111111111111111111"
                            .parse()
                            .expect("valid dummy hash"),
                    ),
                    sender_account_id: stack.harness.cleanup_signer_account_id.0.clone(),
                    wait_until: Some(blockchain_gateway_core::common::TxExecutionStatus::None),
                    encoding: tx::ValueEncoding::Json,
                },
            })
            .await;

        assert!(deleted.is_err());

        stack.shutdown().await;
        Ok(())
    }
}
