use blockchain_gateway_core::{chain, market, registry, storage, tx, universal_account};
use blockchain_gateway_near::{
    actor::{read::ReadRpcRequest, write::WriteRpcRequest},
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

fn register_write<Spec: WriteRpcRequest>(
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

fn register_read<Spec: ReadRpcRequest>(
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

    register_read::<chain::ViewAccount>(&mut m)?;
    register_read::<chain::ViewFunction>(&mut m)?;
    register_read::<chain::GetTransaction>(&mut m)?;
    register_read::<registry::ListDeployments>(&mut m)?;
    register_read::<registry::ListVersions>(&mut m)?;
    register_read::<market::GetConfiguration>(&mut m)?;
    register_read::<market::ListBorrowPositions>(&mut m)?;
    register_read::<storage::GetBalanceBounds>(&mut m)?;
    register_read::<storage::GetBalanceOf>(&mut m)?;
    register_write::<storage::Deposit>(&mut m)?;
    register_read::<universal_account::GetKey>(&mut m)?;
    register_write::<tx::FunctionCall>(&mut m)?;

    Ok(m)
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use blockchain_gateway_core::{
        chain::{self, GetTransactionParams, TransactionReturnValue, TransactionStatus},
        common::{ContractArgs, ReadRequest, WriteRequest},
        storage, tx, Base64Bytes, ContractMethodName, CryptoHash, NearGas, NearToken,
    };
    use blockchain_gateway_testing::{SandboxHarness, TestController};
    use jsonrpsee::server::{ServerBuilder, ServerHandle};

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
                    args: storage::GetBalanceBoundsArgs {},
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
            .request::<chain::ViewAccount>(&ReadRequest {
                params: chain::ViewAccountParams {
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
            .request::<chain::ViewFunction>(&ReadRequest {
                params: chain::ViewFunctionParams {
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
                    args: storage::GetBalanceOfArgs {
                        account_id: stack.harness.gateway_signer_account_id.0.clone(),
                    },
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
                    args: storage::GetBalanceOfArgs {
                        account_id: stack.harness.beneficiary_account_id.clone(),
                    },
                },
            })
            .await?;

        assert!(beneficiary_balance.balance.is_some());

        let deposit_transaction = stack
            .controller
            .request::<chain::GetTransaction>(&ReadRequest {
                params: GetTransactionParams {
                    tx_hash: tx_hash(&beneficiary_deposit),
                    sender_account_id: stack.harness.gateway_signer_account_id.0.clone(),
                    wait_until: None,
                    encoding: chain::ValueEncoding::Json,
                },
            })
            .await?;

        assert_eq!(deposit_transaction.status, TransactionStatus::Succeeded);
        assert!(matches!(
            deposit_transaction.return_value,
            Some(TransactionReturnValue::Json(_))
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
            .request::<chain::GetTransaction>(&ReadRequest {
                params: GetTransactionParams {
                    tx_hash: tx_hash(&mint_transaction),
                    sender_account_id: stack.harness.gateway_signer_account_id.0.clone(),
                    wait_until: None,
                    encoding: chain::ValueEncoding::Json,
                },
            })
            .await?;

        assert_eq!(mint_status.status, TransactionStatus::Succeeded);

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
            .request::<chain::GetTransaction>(&ReadRequest {
                params: GetTransactionParams {
                    tx_hash: tx_hash(&transfer_transaction),
                    sender_account_id: stack.harness.gateway_signer_account_id.0.clone(),
                    wait_until: None,
                    encoding: chain::ValueEncoding::Json,
                },
            })
            .await?;

        assert_eq!(transfer_result.status, TransactionStatus::Succeeded);
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
            .request::<chain::GetTransaction>(&ReadRequest {
                params: GetTransactionParams {
                    tx_hash: tx_hash(&unregister_transaction),
                    sender_account_id: stack.harness.gateway_signer_account_id.0.clone(),
                    wait_until: None,
                    encoding: chain::ValueEncoding::Base64,
                },
            })
            .await?;

        assert_eq!(unregister_result.status, TransactionStatus::Succeeded);
        assert!(matches!(
            unregister_result.return_value,
            Some(TransactionReturnValue::Base64(_))
        ));

        stack.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn registry_endpoints_work_against_sandbox() -> Result<()> {
        let stack = TestStack::start().await?;
        let registry_id = stack.harness.deploy_registry().await?;

        let versions = stack
            .controller
            .request::<registry::ListVersions>(&ReadRequest {
                params: registry::ListVersionsParams {
                    registry_id: registry_id.clone(),
                    args: blockchain_gateway_core::common::Pagination::default(),
                },
            })
            .await?;

        let deployments = stack
            .controller
            .request::<registry::ListDeployments>(&ReadRequest {
                params: registry::ListDeploymentsParams {
                    registry_id,
                    args: blockchain_gateway_core::common::Pagination::default(),
                },
            })
            .await?;

        assert!(versions.values.is_empty());
        assert!(deployments.account_ids.is_empty());

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
        let (account_id, key) = stack.harness.deploy_universal_account().await?;

        let result = stack
            .controller
            .request::<universal_account::GetKey>(&ReadRequest {
                params: universal_account::GetKeyParams {
                    account_id,
                    args: universal_account::GetKeyArgs { key },
                },
            })
            .await?;

        assert!(result.parameters.is_some());

        stack.shutdown().await;
        Ok(())
    }
}
