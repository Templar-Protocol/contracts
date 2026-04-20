use blockchain_gateway_core::{
    account, contract, ft, market, oracle, proxy_oracle, proxy_oracle_governance,
    proxy_oracle_owner, registry, storage, tx, universal_account,
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
    register_read::<market::GetCurrentSnapshot>(&mut m)?;
    register_read::<market::GetFinalizedSnapshotsLen>(&mut m)?;
    register_read::<market::ListFinalizedSnapshots>(&mut m)?;
    register_read::<market::GetBorrowAssetMetrics>(&mut m)?;
    register_read::<market::ListBorrowPositions>(&mut m)?;
    register_read::<market::GetBorrowPosition>(&mut m)?;
    register_read::<market::GetBorrowPositionPendingInterest>(&mut m)?;
    register_read::<market::GetBorrowStatus>(&mut m)?;
    register_read::<market::ListSupplyPositions>(&mut m)?;
    register_read::<market::GetSupplyPosition>(&mut m)?;
    register_read::<market::GetSupplyPositionPendingYield>(&mut m)?;
    register_read::<market::GetSupplyWithdrawalRequestStatus>(&mut m)?;
    register_read::<market::GetSupplyWithdrawalQueueStatus>(&mut m)?;
    register_read::<market::GetLastYieldRate>(&mut m)?;
    register_read::<market::GetStaticYield>(&mut m)?;
    register_write::<market::Borrow>(&mut m)?;
    register_write::<market::Supply>(&mut m)?;
    register_write::<market::WithdrawCollateral>(&mut m)?;
    register_write::<market::ApplyInterest>(&mut m)?;
    register_write::<market::Repay>(&mut m)?;
    register_write::<market::CreateSupplyWithdrawalRequest>(&mut m)?;
    register_write::<market::CancelSupplyWithdrawalRequest>(&mut m)?;
    register_write::<market::ExecuteNextSupplyWithdrawalRequest>(&mut m)?;
    register_write::<market::WithdrawSupply>(&mut m)?;
    register_write::<market::Liquidate>(&mut m)?;
    register_write::<market::HarvestYield>(&mut m)?;
    register_write::<market::AccumulateStaticYield>(&mut m)?;
    register_write::<market::WithdrawStaticYield>(&mut m)?;
    register_read::<oracle::GetKind>(&mut m)?;
    register_read::<oracle::GetPriceResolutionDependencies>(&mut m)?;
    register_read::<oracle::ResolvePrice>(&mut m)?;
    register_read::<oracle::ResolvePrices>(&mut m)?;
    register_read::<oracle::GetPrice>(&mut m)?;
    register_read::<oracle::GetPrices>(&mut m)?;
    register_write::<oracle::UpdatePyth>(&mut m)?;
    register_write::<oracle::UpdateRedStone>(&mut m)?;
    register_write::<oracle::UpdatePrices>(&mut m)?;
    register_read::<proxy_oracle::ListProxies>(&mut m)?;
    register_read::<proxy_oracle::GetProxy>(&mut m)?;
    register_read::<proxy_oracle::PriceFeedExists>(&mut m)?;
    register_read::<proxy_oracle_governance::GetNextId>(&mut m)?;
    register_read::<proxy_oracle_governance::GetTtl>(&mut m)?;
    register_read::<proxy_oracle_governance::GetCount>(&mut m)?;
    register_read::<proxy_oracle_governance::List>(&mut m)?;
    register_read::<proxy_oracle_governance::Get>(&mut m)?;
    register_write::<proxy_oracle_governance::Create>(&mut m)?;
    register_write::<proxy_oracle_governance::Cancel>(&mut m)?;
    register_write::<proxy_oracle_governance::Execute>(&mut m)?;
    register_read::<proxy_oracle_owner::GetOwner>(&mut m)?;
    register_read::<proxy_oracle_owner::GetProposedOwner>(&mut m)?;
    register_write::<proxy_oracle_owner::ProposeOwner>(&mut m)?;
    register_write::<proxy_oracle_owner::AcceptOwner>(&mut m)?;
    register_write::<proxy_oracle_owner::RenounceOwner>(&mut m)?;
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
    use std::{collections::HashMap, path::Path};

    use anyhow::Result;
    use blockchain_gateway_core::{
        account,
        common::{ContractArgs, ReadRequest, WriteRequest},
        contract, ft, market, oracle, proxy_oracle, proxy_oracle_governance, proxy_oracle_owner,
        registry, storage, tx, universal_account, Base64Bytes, ContractMethodName, CryptoHash,
        NearGas, NearToken,
    };
    use blockchain_gateway_near::GatewayContext;
    use blockchain_gateway_testing::{SandboxHarness, TestController};
    use jsonrpsee::server::{ServerBuilder, ServerHandle};
    use near_sdk::json_types::{I64, U64};
    use templar_common::market::DepositMsg;
    use templar_common::oracle::{
        price_transformer::{self, PriceTransformer},
        proxy::Proxy,
        pyth::{PriceIdentifier, PythTimestamp},
        redstone::FeedData,
        OracleRequest,
    };
    use templar_common::primitive_types::U256;
    use templar_common::time::Nanoseconds;
    use templar_universal_account::{
        authentication::with_raw_string::WithRawString,
        authentication::Payload,
        transaction::{FunctionCallAction, Transaction},
        KeyParameters, NEAR_TESTNET_CHAIN_ID,
    };
    use url::Url;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
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
            Self::start_with_oracle_update_config(
                "https://hermes-beta.pyth.network".parse().unwrap(),
            )
            .await
        }

        async fn start_with_oracle_update_config(pyth_hermes_url: Url) -> Result<Self> {
            let harness = SandboxHarness::start().await?;
            let context =
                GatewayContext::new(harness.network.clone(), pyth_hermes_url, Path::new(&"node"))?;
            let gateway = GatewayService::spawn(context, harness.gateway_signers.clone());

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

    fn redstone_bridge_payload_hex() -> &'static str {
        "45544800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002d9030a710019c56f0bec0000000200000015d1cb1a708c63264741b00ce097176e45f708914b8cfdca26b079877a70604e25aa0bcfa3a41df8212eddd51db3496b95c7c3dc4caa9ac9705602af0515db1b31c45544800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002d9028ed04019c56f0bec000000020000001dcaf484941c0d206f1898185b953c6a92d7fd188b347505c0f5beb2030e06e3e1b2f7dfb45929ac7676136af93fee7f14a614b40fa4dc2d1e625dbece02eaca21c45544800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002d9028ed04019c56f0bec00000002000000199bd54930138268baad2869e9ceb99b6bc67cd6b8a4cc98e05f0b1cd9b7f07066008208399a728fac3d1dc3ca407cb8199a0209377bceb0c48f2cc3d756078051b4254430000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006179a92ab8c019c56f0bec000000020000001f08af53ed34046f7f64cc02ffb7973252954d7c395e440693c896bffdbc2de1e31cf5675bf66583d3e3438f5002ae9c10870d4dc45de05c560b239aa3a2d50a41b425443000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000617a1187473019c56f0bec0000000200000011b96dc2763a692e3245ce4f1b0c16ea245c240204e99ebd323b340e58bfb14fb5f0465ce11b8dd52ff839547cc949d20e4e8ba0be43dd6417cade2a8ebfd8c9e1c425443000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000617a1187473019c56f0bec00000002000000114a02710892325b13afc74bbd350dd9ec80342b2d6c0c94df7b7a60dbf67a1b91b182fa4555e0e0db91e6258b279f00b7eeb8f5de9930e352d5321a6b8b64a031c00063137373039383531343539383223302e392e30237374656c6c61722d636f6e6e6563746f72000025000002ed57011e0000"
    }

    async fn start_mock_hermes_server(vaa_hex: &str) -> Result<MockServer> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/updates/price/latest"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "binary": {
                    "data": [vaa_hex],
                }
            })))
            .mount(&server)
            .await;
        Ok(server)
    }

    async fn view_contract_json(
        stack: &TestStack,
        contract_id: near_account_id::AccountId,
        method_name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value> {
        Ok(stack
            .controller
            .request::<contract::ViewFunction>(&ReadRequest {
                params: contract::ViewFunctionParams {
                    contract_id,
                    method_name: ContractMethodName(method_name.to_owned()),
                    args: ContractArgs::Json(args),
                },
            })
            .await?
            .value)
    }

    async fn call_function(
        stack: &TestStack,
        signer_account_id: blockchain_gateway_core::ManagedAccountId,
        receiver_id: near_account_id::AccountId,
        method_name: &str,
        args: serde_json::Value,
        gas_tgas: u64,
        deposit_yocto: u128,
    ) -> Result<blockchain_gateway_core::common::WriteOperationResult> {
        stack
            .controller
            .request::<tx::FunctionCall>(&WriteRequest {
                signer_account_id,
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: tx::FunctionCallBody {
                    receiver_id,
                    method_name: ContractMethodName(method_name.to_owned()),
                    args: ContractArgs::Json(args),
                    gas: NearGas::from_tgas(gas_tgas),
                    deposit: NearToken::from_yoctonear(deposit_yocto),
                },
            })
            .await
    }

    async fn ensure_registered(
        stack: &TestStack,
        signer_account_id: blockchain_gateway_core::ManagedAccountId,
        contract_id: near_account_id::AccountId,
        account_id: near_account_id::AccountId,
    ) -> Result<()> {
        let _ = stack
            .controller
            .request::<storage::EnsureDeposit>(&WriteRequest {
                signer_account_id,
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: storage::EnsureDepositBody {
                    contract_id,
                    account_id,
                    mode: storage::EnsureDepositMode::Registered,
                },
            })
            .await?;
        Ok(())
    }

    async fn ft_balance(
        stack: &TestStack,
        contract_id: near_account_id::AccountId,
        account_id: near_account_id::AccountId,
    ) -> Result<u128> {
        let value = view_contract_json(
            stack,
            contract_id,
            "ft_balance_of",
            serde_json::json!({ "account_id": account_id }),
        )
        .await?;
        Ok(value
            .as_str()
            .expect("ft_balance_of should serialize as string")
            .parse()?)
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
                    code: Base64Bytes(
                        test_utils::UniversalAccountController::wasm()
                            .await
                            .to_vec(),
                    ),
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

        let created_account_id: near_account_id::AccountId =
            format!("ua-created.{}", registry_id.0)
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

    #[tokio::test]
    async fn market_extended_endpoints_work_against_sandbox() -> Result<()> {
        let stack = TestStack::start().await?;
        let (market_id, _configuration) = stack.harness.deploy_market().await?;

        let _ = stack
            .controller
            .request::<market::GetCurrentSnapshot>(&ReadRequest {
                params: market::GetCurrentSnapshotParams {
                    market_id: market_id.clone(),
                },
            })
            .await?;
        let finalized_len = stack
            .controller
            .request::<market::GetFinalizedSnapshotsLen>(&ReadRequest {
                params: market::GetFinalizedSnapshotsLenParams {
                    market_id: market_id.clone(),
                },
            })
            .await?;
        let finalized = stack
            .controller
            .request::<market::ListFinalizedSnapshots>(&ReadRequest {
                params: market::ListFinalizedSnapshotsParams {
                    market_id: market_id.clone(),
                    args: blockchain_gateway_core::common::Pagination::default(),
                },
            })
            .await?;
        let metrics = stack
            .controller
            .request::<market::GetBorrowAssetMetrics>(&ReadRequest {
                params: market::GetBorrowAssetMetricsParams {
                    market_id: market_id.clone(),
                },
            })
            .await?;
        let empty_borrow_position = stack
            .controller
            .request::<market::GetBorrowPosition>(&ReadRequest {
                params: market::GetBorrowPositionParams {
                    market_id: market_id.clone(),
                    account_id: stack.harness.gateway_signer_account_id.0.clone(),
                },
            })
            .await?;
        let empty_borrow_interest = stack
            .controller
            .request::<market::GetBorrowPositionPendingInterest>(&ReadRequest {
                params: market::GetBorrowPositionPendingInterestParams {
                    market_id: market_id.clone(),
                    account_id: stack.harness.gateway_signer_account_id.0.clone(),
                    snapshot_limit: Some(1),
                },
            })
            .await?;
        let empty_borrow_status = stack
            .controller
            .request::<market::GetBorrowStatus>(&ReadRequest {
                params: market::GetBorrowStatusParams {
                    market_id: market_id.clone(),
                    account_id: stack.harness.gateway_signer_account_id.0.clone(),
                    oracle_response: templar_common::oracle::pyth::OracleResponse::new(),
                },
            })
            .await?;
        let supply_positions = stack
            .controller
            .request::<market::ListSupplyPositions>(&ReadRequest {
                params: market::ListSupplyPositionsParams {
                    market_id: market_id.clone(),
                    args: blockchain_gateway_core::common::Pagination::default(),
                },
            })
            .await?;
        let empty_supply_position = stack
            .controller
            .request::<market::GetSupplyPosition>(&ReadRequest {
                params: market::GetSupplyPositionParams {
                    market_id: market_id.clone(),
                    account_id: stack.harness.gateway_signer_account_id.0.clone(),
                },
            })
            .await?;
        let empty_supply_yield = stack
            .controller
            .request::<market::GetSupplyPositionPendingYield>(&ReadRequest {
                params: market::GetSupplyPositionPendingYieldParams {
                    market_id: market_id.clone(),
                    account_id: stack.harness.gateway_signer_account_id.0.clone(),
                    snapshot_limit: Some(1),
                },
            })
            .await?;
        let empty_withdrawal_request = stack
            .controller
            .request::<market::GetSupplyWithdrawalRequestStatus>(&ReadRequest {
                params: market::GetSupplyWithdrawalRequestStatusParams {
                    market_id: market_id.clone(),
                    account_id: stack.harness.gateway_signer_account_id.0.clone(),
                },
            })
            .await?;
        let queue = stack
            .controller
            .request::<market::GetSupplyWithdrawalQueueStatus>(&ReadRequest {
                params: market::GetSupplyWithdrawalQueueStatusParams {
                    market_id: market_id.clone(),
                },
            })
            .await?;
        let last_yield = stack
            .controller
            .request::<market::GetLastYieldRate>(&ReadRequest {
                params: market::GetLastYieldRateParams {
                    market_id: market_id.clone(),
                },
            })
            .await?;
        let static_yield = stack
            .controller
            .request::<market::GetStaticYield>(&ReadRequest {
                params: market::GetStaticYieldParams {
                    market_id: market_id.clone(),
                    account_id: stack.harness.gateway_signer_account_id.0.clone(),
                },
            })
            .await?;
        let _ = stack
            .controller
            .request::<market::ApplyInterest>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: market::ApplyInterestBody {
                    market_id: market_id.clone(),
                    account_id: None,
                    snapshot_limit: Some(1),
                },
            })
            .await?;
        let _ = stack
            .controller
            .request::<market::AccumulateStaticYield>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: market::AccumulateStaticYieldBody {
                    market_id,
                    account_id: Some(stack.harness.gateway_signer_account_id.0.clone()),
                    snapshot_limit: Some(1),
                },
            })
            .await?;

        assert_eq!(finalized_len as usize, finalized.snapshots.len());
        assert!(empty_borrow_position.position.is_none());
        assert!(empty_borrow_interest.amount.is_none());
        assert!(empty_borrow_status.status.is_none());
        assert!(supply_positions.positions.is_empty());
        assert!(empty_supply_position.position.is_none());
        assert!(empty_supply_yield.amount.is_none());
        assert!(empty_withdrawal_request.status.is_none());
        assert_eq!(
            queue.depth,
            templar_common::asset::BorrowAssetAmount::zero()
        );
        assert_eq!(last_yield, templar_common::number::Decimal::ZERO);
        assert!(static_yield.accumulator.is_none());
        assert_eq!(
            metrics.borrowed,
            templar_common::asset::BorrowAssetAmount::zero()
        );

        stack.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn market_composed_operations_work_against_sandbox() -> Result<()> {
        let stack = TestStack::start().await?;
        let (market_id, configuration) = stack.harness.deploy_market().await?;
        let borrow_asset_id = configuration
            .borrow_asset
            .clone()
            .into_nep141()
            .expect("sandbox market should use NEP-141 borrow asset");
        let collateral_asset_id = configuration
            .collateral_asset
            .clone()
            .into_nep141()
            .expect("sandbox market should use NEP-141 collateral asset");

        stack
            .harness
            .set_mock_oracle_pyth_price(
                configuration.price_oracle_configuration.account_id.clone(),
                configuration
                    .price_oracle_configuration
                    .borrow_asset_price_id,
                Some(test_utils::to_price(1.0)),
            )
            .await?;
        stack
            .harness
            .set_mock_oracle_pyth_price(
                configuration.price_oracle_configuration.account_id.clone(),
                configuration
                    .price_oracle_configuration
                    .collateral_asset_price_id,
                Some(test_utils::to_price(2.0)),
            )
            .await?;

        for signer_account_id in [
            stack.harness.gateway_signer_account_id.clone(),
            stack.harness.cleanup_signer_account_id.clone(),
        ] {
            ensure_registered(
                &stack,
                signer_account_id.clone(),
                borrow_asset_id.clone(),
                signer_account_id.0.clone(),
            )
            .await?;
            ensure_registered(
                &stack,
                signer_account_id.clone(),
                collateral_asset_id.clone(),
                signer_account_id.0.clone(),
            )
            .await?;
            ensure_registered(
                &stack,
                signer_account_id.clone(),
                market_id.0.clone(),
                signer_account_id.0.clone(),
            )
            .await?;
        }
        ensure_registered(
            &stack,
            stack.harness.gateway_signer_account_id.clone(),
            borrow_asset_id.clone(),
            market_id.0.clone(),
        )
        .await?;
        ensure_registered(
            &stack,
            stack.harness.gateway_signer_account_id.clone(),
            collateral_asset_id.clone(),
            market_id.0.clone(),
        )
        .await?;

        let _ = call_function(
            &stack,
            stack.harness.gateway_signer_account_id.clone(),
            borrow_asset_id.clone(),
            "mint",
            serde_json::json!({ "amount": "200000" }),
            100,
            0,
        )
        .await?;
        let _ = call_function(
            &stack,
            stack.harness.gateway_signer_account_id.clone(),
            collateral_asset_id.clone(),
            "mint",
            serde_json::json!({ "amount": "500000" }),
            100,
            0,
        )
        .await?;
        let _ = call_function(
            &stack,
            stack.harness.cleanup_signer_account_id.clone(),
            borrow_asset_id.clone(),
            "mint",
            serde_json::json!({ "amount": "200000" }),
            100,
            0,
        )
        .await?;
        let _ = call_function(
            &stack,
            stack.harness.cleanup_signer_account_id.clone(),
            collateral_asset_id.clone(),
            "mint",
            serde_json::json!({ "amount": "500000" }),
            100,
            0,
        )
        .await?;

        let supply = stack
            .controller
            .request::<market::Supply>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: market::SupplyBody {
                    market_id: market_id.clone(),
                    amount: 100_000u128.into(),
                },
            })
            .await?;
        assert_eq!(
            supply.outcome.operation.status,
            blockchain_gateway_core::OperationStatus::Succeeded
        );
        assert_eq!(supply.outcome.operation.steps.len(), 1);

        let mut supply_is_active = false;
        for _ in 0..10 {
            let _ = stack
                .controller
                .request::<market::HarvestYield>(&WriteRequest {
                    signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                    idempotency_key: None,
                    wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                    body: market::HarvestYieldBody {
                        market_id: market_id.clone(),
                        account_id: None,
                        mode: None,
                    },
                })
                .await?;
            let position = stack
                .controller
                .request::<market::GetSupplyPosition>(&ReadRequest {
                    params: market::GetSupplyPositionParams {
                        market_id: market_id.clone(),
                        account_id: stack.harness.gateway_signer_account_id.0.clone(),
                    },
                })
                .await?;
            if position
                .position
                .as_ref()
                .is_some_and(|position| position.get_deposit().incoming.is_empty())
            {
                supply_is_active = true;
                break;
            }
        }
        assert!(supply_is_active);

        let _ = call_function(
            &stack,
            stack.harness.cleanup_signer_account_id.clone(),
            collateral_asset_id.clone(),
            "ft_transfer_call",
            serde_json::json!({
                "receiver_id": market_id.0.clone(),
                "amount": "200000",
                "msg": serde_json::to_string(&DepositMsg::Collateralize)?,
            }),
            300,
            1,
        )
        .await?;
        let _ = stack
            .controller
            .request::<market::Borrow>(&WriteRequest {
                signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: market::BorrowBody {
                    market_id: market_id.clone(),
                    amount: 60_000u128.into(),
                },
            })
            .await?;

        let repay = stack
            .controller
            .request::<market::Repay>(&WriteRequest {
                signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: market::RepayBody {
                    market_id: market_id.clone(),
                    amount: 10_000u128.into(),
                    account_id: None,
                },
            })
            .await?;
        assert_eq!(
            repay.outcome.operation.status,
            blockchain_gateway_core::OperationStatus::Succeeded
        );

        stack
            .harness
            .set_mock_oracle_pyth_price(
                configuration.price_oracle_configuration.account_id.clone(),
                configuration
                    .price_oracle_configuration
                    .collateral_asset_price_id,
                Some(test_utils::to_price(0.05)),
            )
            .await?;

        let borrow_position_before_liquidation = stack
            .controller
            .request::<market::GetBorrowPosition>(&ReadRequest {
                params: market::GetBorrowPositionParams {
                    market_id: market_id.clone(),
                    account_id: stack.harness.cleanup_signer_account_id.0.clone(),
                },
            })
            .await?
            .position
            .expect("borrower should have a borrow position before liquidation");
        let liability_before_liquidation =
            borrow_position_before_liquidation.get_total_borrow_asset_liability();
        let liquidation_oracle_response = HashMap::from([
            (
                configuration
                    .price_oracle_configuration
                    .borrow_asset_price_id,
                Some(test_utils::to_price(1.0)),
            ),
            (
                configuration
                    .price_oracle_configuration
                    .collateral_asset_price_id,
                Some(test_utils::to_price(0.05)),
            ),
        ]);
        let liquidation_price_pair = configuration
            .price_oracle_configuration
            .create_price_pair(&liquidation_oracle_response)?;
        let liquidatable_collateral = borrow_position_before_liquidation.liquidatable_collateral(
            &liquidation_price_pair,
            configuration.borrow_mcr_maintenance,
            configuration.liquidation_maximum_spread,
        );
        let liquidation_amount = configuration
            .minimum_acceptable_liquidation_amount(liquidatable_collateral, &liquidation_price_pair)
            .expect("liquidation amount should be derivable");
        let liquidator_borrow_balance_before = ft_balance(
            &stack,
            borrow_asset_id.clone(),
            stack.harness.gateway_signer_account_id.0.clone(),
        )
        .await?;
        let liquidate = stack
            .controller
            .request::<market::Liquidate>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: market::LiquidateBody {
                    market_id: market_id.clone(),
                    account_id: stack.harness.cleanup_signer_account_id.0.clone(),
                    liquidation_amount,
                    collateral_amount: Some(liquidatable_collateral),
                },
            })
            .await?;
        assert_eq!(
            liquidate.outcome.operation.status,
            blockchain_gateway_core::OperationStatus::Succeeded
        );
        let liquidator_borrow_balance_after = ft_balance(
            &stack,
            borrow_asset_id.clone(),
            stack.harness.gateway_signer_account_id.0.clone(),
        )
        .await?;
        assert!(liquidator_borrow_balance_after < liquidator_borrow_balance_before);

        let withdraw_supply = stack
            .controller
            .request::<market::WithdrawSupply>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: market::WithdrawSupplyBody {
                    market_id: market_id.clone(),
                    amount: 20_000u128.into(),
                    batch_limit: None,
                },
            })
            .await?;
        assert_eq!(
            withdraw_supply.outcome.operation.status,
            blockchain_gateway_core::OperationStatus::Succeeded
        );
        assert_eq!(withdraw_supply.outcome.operation.steps.len(), 2);

        let supply_request = stack
            .controller
            .request::<market::GetSupplyWithdrawalRequestStatus>(&ReadRequest {
                params: market::GetSupplyWithdrawalRequestStatusParams {
                    market_id: market_id.clone(),
                    account_id: stack.harness.gateway_signer_account_id.0.clone(),
                },
            })
            .await?;
        assert!(supply_request.status.is_none());

        let borrow_position = stack
            .controller
            .request::<market::GetBorrowPosition>(&ReadRequest {
                params: market::GetBorrowPositionParams {
                    market_id,
                    account_id: stack.harness.cleanup_signer_account_id.0.clone(),
                },
            })
            .await?;
        let borrow_position = borrow_position
            .position
            .expect("borrower should still have a borrow position after partial liquidation");
        let liability_after_liquidation = borrow_position.get_total_borrow_asset_liability();
        assert!(
            liability_after_liquidation <= liability_before_liquidation,
            "liquidation should not increase liability"
        );

        stack.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn proxy_oracle_endpoints_work_against_sandbox() -> Result<()> {
        let stack = TestStack::start().await?;
        let oracle_id = stack.harness.deploy_proxy_oracle().await?;

        let owner = stack
            .controller
            .request::<proxy_oracle_owner::GetOwner>(&ReadRequest {
                params: proxy_oracle_owner::GetOwnerParams {
                    oracle_id: oracle_id.clone(),
                },
            })
            .await?;
        assert_eq!(
            owner.owner,
            Some(stack.harness.proxy_oracle_signer_account_id.0.clone())
        );

        let next_id = stack
            .controller
            .request::<proxy_oracle_governance::GetNextId>(&ReadRequest {
                params: proxy_oracle_governance::GetNextIdParams {
                    oracle_id: oracle_id.clone(),
                },
            })
            .await?;
        assert_eq!(next_id, 0);
        let ttl = stack
            .controller
            .request::<proxy_oracle_governance::GetTtl>(&ReadRequest {
                params: proxy_oracle_governance::GetTtlParams {
                    oracle_id: oracle_id.clone(),
                },
            })
            .await?;
        assert_eq!(ttl.ttl_ns, templar_common::time::Nanoseconds::zero());
        let count = stack
            .controller
            .request::<proxy_oracle_governance::GetCount>(&ReadRequest {
                params: proxy_oracle_governance::GetCountParams {
                    oracle_id: oracle_id.clone(),
                },
            })
            .await?;
        assert_eq!(count, 0);

        let list = stack
            .controller
            .request::<proxy_oracle::ListProxies>(&ReadRequest {
                params: proxy_oracle::ListProxiesParams {
                    oracle_id: oracle_id.clone(),
                    offset: None,
                    count: None,
                },
            })
            .await?;
        assert!(list.proxies.is_empty());

        let price_id = templar_common::oracle::pyth::PriceIdentifier([0xaa; 32]);
        let proxy = templar_common::oracle::proxy::Proxy::median_low([
            templar_common::oracle::OracleRequest::pyth(
                "pyth.near".parse().expect("valid oracle id"),
                templar_common::oracle::pyth::PriceIdentifier([0xbb; 32]),
            )
            .into(),
        ]);

        let _ = stack
            .controller
            .request::<proxy_oracle_governance::Create>(&WriteRequest {
                signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: proxy_oracle_governance::CreateBody {
                    oracle_id: oracle_id.clone(),
                    id: 0,
                    operation: templar_common::oracle::proxy::governance::Operation::SetProxy {
                        id: price_id,
                        proxy: Some(proxy.clone()),
                    },
                },
            })
            .await?;

        let proposal = stack
            .controller
            .request::<proxy_oracle_governance::Get>(&ReadRequest {
                params: proxy_oracle_governance::GetParams {
                    oracle_id: oracle_id.clone(),
                    id: 0,
                },
            })
            .await?;
        assert!(proposal.proposal.is_some());
        let ids = stack
            .controller
            .request::<proxy_oracle_governance::List>(&ReadRequest {
                params: proxy_oracle_governance::ListParams {
                    oracle_id: oracle_id.clone(),
                    offset: None,
                    count: None,
                },
            })
            .await?;
        assert_eq!(ids.ids, vec![0]);

        let _ = stack
            .controller
            .request::<proxy_oracle_governance::Execute>(&WriteRequest {
                signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: proxy_oracle_governance::ExecuteBody {
                    oracle_id: oracle_id.clone(),
                    id: 0,
                },
            })
            .await?;

        let got_proxy = stack
            .controller
            .request::<proxy_oracle::GetProxy>(&ReadRequest {
                params: proxy_oracle::GetProxyParams {
                    oracle_id: oracle_id.clone(),
                    id: price_id,
                },
            })
            .await?;
        assert_eq!(got_proxy.proxy, Some(proxy));

        let exists = stack
            .controller
            .request::<proxy_oracle::PriceFeedExists>(&ReadRequest {
                params: proxy_oracle::PriceFeedExistsParams {
                    oracle_id: oracle_id.clone(),
                    price_identifier: price_id,
                },
            })
            .await?;
        assert!(exists.exists);

        let _ = stack
            .controller
            .request::<proxy_oracle_owner::ProposeOwner>(&WriteRequest {
                signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: proxy_oracle_owner::ProposeOwnerBody {
                    oracle_id: oracle_id.clone(),
                    account_id: Some(stack.harness.cleanup_signer_account_id.0.clone()),
                },
            })
            .await?;

        let proposed = stack
            .controller
            .request::<proxy_oracle_owner::GetProposedOwner>(&ReadRequest {
                params: proxy_oracle_owner::GetProposedOwnerParams {
                    oracle_id: oracle_id.clone(),
                },
            })
            .await?;
        assert_eq!(
            proposed.proposed_owner,
            Some(stack.harness.cleanup_signer_account_id.0.clone())
        );

        let _ = stack
            .controller
            .request::<proxy_oracle_owner::AcceptOwner>(&WriteRequest {
                signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: proxy_oracle_owner::AcceptOwnerBody {
                    oracle_id: oracle_id.clone(),
                },
            })
            .await?;

        let owner = stack
            .controller
            .request::<proxy_oracle_owner::GetOwner>(&ReadRequest {
                params: proxy_oracle_owner::GetOwnerParams { oracle_id },
            })
            .await?;
        assert_eq!(
            owner.owner,
            Some(stack.harness.cleanup_signer_account_id.0.clone())
        );

        let _ = stack
            .controller
            .request::<proxy_oracle_governance::Create>(&WriteRequest {
                signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: proxy_oracle_governance::CreateBody {
                    oracle_id: stack.harness.proxy_oracle_signer_account_id.0.clone(),
                    id: 1,
                    operation: templar_common::oracle::proxy::governance::Operation::SetActionTtl {
                        new_ttl: templar_common::time::Nanoseconds::from_secs(1),
                    },
                },
            })
            .await?;
        let _ = stack
            .controller
            .request::<proxy_oracle_governance::Cancel>(&WriteRequest {
                signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: proxy_oracle_governance::CancelBody {
                    oracle_id: stack.harness.proxy_oracle_signer_account_id.0.clone(),
                    id: 1,
                },
            })
            .await?;
        let cancelled = stack
            .controller
            .request::<proxy_oracle_governance::Get>(&ReadRequest {
                params: proxy_oracle_governance::GetParams {
                    oracle_id: stack.harness.proxy_oracle_signer_account_id.0.clone(),
                    id: 1,
                },
            })
            .await?;
        assert!(cancelled.proposal.is_none());

        let _ = stack
            .controller
            .request::<proxy_oracle_owner::RenounceOwner>(&WriteRequest {
                signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: proxy_oracle_owner::RenounceOwnerBody {
                    oracle_id: stack.harness.proxy_oracle_signer_account_id.0.clone(),
                },
            })
            .await?;
        let owner = stack
            .controller
            .request::<proxy_oracle_owner::GetOwner>(&ReadRequest {
                params: proxy_oracle_owner::GetOwnerParams {
                    oracle_id: stack.harness.proxy_oracle_signer_account_id.0.clone(),
                },
            })
            .await?;
        assert_eq!(owner.owner, None);

        stack.shutdown().await;
        Ok(())
    }

    fn pyth_price(price: f64) -> templar_common::oracle::pyth::Price {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        templar_common::oracle::pyth::Price {
            price: I64((price * 10000.0) as i64),
            conf: U64(0),
            expo: -4,
            publish_time: PythTimestamp::from_ms(now_ms),
        }
    }

    fn redstone_price(price: f64) -> FeedData {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let now_ms = Nanoseconds::from_ms(now_ms);
        FeedData {
            price: U256::from((price * 1e8) as u128).into(),
            package_timestamp: now_ms,
            write_timestamp: now_ms,
        }
    }

    fn assert_same_pyth_price_value(
        actual: Option<templar_common::oracle::pyth::Price>,
        expected: templar_common::oracle::pyth::Price,
    ) {
        let actual = actual.expect("expected price to be present");
        assert_eq!(actual.price, expected.price);
        assert_eq!(actual.conf, expected.conf);
        assert_eq!(actual.expo, expected.expo);
    }

    #[tokio::test]
    async fn oracle_update_endpoints_work_against_sandbox() -> Result<()> {
        let hermes = start_mock_hermes_server("cafebabe").await?;
        let stack = TestStack::start_with_oracle_update_config(hermes.uri().parse()?).await?;

        let pyth_oracle_id = stack
            .harness
            .deploy_mock_oracle("pyth-oracle.near".parse()?)
            .await?;
        let redstone_oracle_id = stack
            .harness
            .deploy_redstone_adapter("redstone-oracle.near".parse()?)
            .await?;

        let pyth_result = stack
            .controller
            .request::<oracle::UpdatePyth>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: oracle::UpdatePythBody {
                    oracle_id: pyth_oracle_id.clone(),
                    vaa: Base64Bytes(vec![0xde, 0xad, 0xbe, 0xef]),
                },
            })
            .await?;
        assert_eq!(
            pyth_result.outcome.operation.status,
            blockchain_gateway_core::OperationStatus::Succeeded
        );
        assert_eq!(pyth_result.outcome.operation.steps.len(), 1);

        let last_pyth_update = view_contract_json(
            &stack,
            pyth_oracle_id.clone(),
            "last_pyth_update_data",
            serde_json::Value::Null,
        )
        .await?;
        assert_eq!(
            last_pyth_update,
            serde_json::Value::String("deadbeef".to_owned())
        );

        let redstone_result = stack
            .controller
            .request::<oracle::UpdateRedStone>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: oracle::UpdateRedStoneBody {
                    oracle_id: redstone_oracle_id.clone(),
                    feed_id: "BTC".into(),
                },
            })
            .await?;
        assert_eq!(
            redstone_result.outcome.operation.status,
            blockchain_gateway_core::OperationStatus::Succeeded
        );
        assert_eq!(redstone_result.outcome.operation.steps.len(), 1);

        let redstone_prices = view_contract_json(
            &stack,
            redstone_oracle_id,
            "read_price_data",
            serde_json::json!({ "feed_ids": ["BTC"] }),
        )
        .await?;
        assert_ne!(
            redstone_prices["BTC"]["price"],
            serde_json::Value::String("0".to_owned())
        );

        stack.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn oracle_update_prices_endpoint_resolves_and_updates_dependencies() -> Result<()> {
        let hermes = start_mock_hermes_server("cafebabe").await?;
        let stack = TestStack::start_with_oracle_update_config(hermes.uri().parse()?).await?;

        let direct_oracle_id = stack
            .harness
            .deploy_mock_oracle("composed-pyth.near".parse()?)
            .await?;
        let redstone_oracle_id = stack
            .harness
            .deploy_redstone_adapter("composed-redstone.near".parse()?)
            .await?;
        let proxy_oracle_id = stack.harness.deploy_proxy_oracle().await?;

        let proxy_direct_id = PriceIdentifier([0x11; 32]);
        let proxy_redstone_id = PriceIdentifier([0x22; 32]);

        let _ = stack
            .controller
            .request::<proxy_oracle_governance::Create>(&WriteRequest {
                signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: proxy_oracle_governance::CreateBody {
                    oracle_id: proxy_oracle_id.clone(),
                    id: 0,
                    operation: templar_common::oracle::proxy::governance::Operation::SetProxy {
                        id: proxy_direct_id,
                        proxy: Some(Proxy::median_low([OracleRequest::pyth(
                            direct_oracle_id.clone(),
                            test_utils::DEFAULT_BORROW_PRICE_ID,
                        )
                        .into()])),
                    },
                },
            })
            .await?;
        let _ = stack
            .controller
            .request::<proxy_oracle_governance::Execute>(&WriteRequest {
                signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: proxy_oracle_governance::ExecuteBody {
                    oracle_id: proxy_oracle_id.clone(),
                    id: 0,
                },
            })
            .await?;

        let _ = stack
            .controller
            .request::<proxy_oracle_governance::Create>(&WriteRequest {
                signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: proxy_oracle_governance::CreateBody {
                    oracle_id: proxy_oracle_id.clone(),
                    id: 1,
                    operation: templar_common::oracle::proxy::governance::Operation::SetProxy {
                        id: proxy_redstone_id,
                        proxy: Some(Proxy::median_low([OracleRequest::redstone(
                            redstone_oracle_id.clone(),
                            "BTC",
                        )
                        .into()])),
                    },
                },
            })
            .await?;
        let _ = stack
            .controller
            .request::<proxy_oracle_governance::Execute>(&WriteRequest {
                signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: proxy_oracle_governance::ExecuteBody {
                    oracle_id: proxy_oracle_id.clone(),
                    id: 1,
                },
            })
            .await?;

        let update_result = stack
            .controller
            .request::<oracle::UpdatePrices>(&WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: oracle::UpdatePricesBody {
                    oracle_id: proxy_oracle_id,
                    price_ids: vec![proxy_direct_id, proxy_redstone_id],
                },
            })
            .await?;
        assert_eq!(
            update_result.outcome.operation.status,
            blockchain_gateway_core::OperationStatus::Succeeded
        );
        assert_eq!(update_result.outcome.operation.steps.len(), 2);

        let pyth_update_count = view_contract_json(
            &stack,
            direct_oracle_id.clone(),
            "pyth_update_count",
            serde_json::Value::Null,
        )
        .await?;
        assert_eq!(pyth_update_count, serde_json::Value::String("1".to_owned()));

        let last_pyth_update = view_contract_json(
            &stack,
            direct_oracle_id,
            "last_pyth_update_data",
            serde_json::Value::Null,
        )
        .await?;
        assert_eq!(
            last_pyth_update,
            serde_json::Value::String("cafebabe".to_owned())
        );

        let redstone_prices = view_contract_json(
            &stack,
            redstone_oracle_id,
            "read_price_data",
            serde_json::json!({ "feed_ids": ["BTC"] }),
        )
        .await?;
        assert_ne!(
            redstone_prices["BTC"]["price"],
            serde_json::Value::String("0".to_owned())
        );

        stack.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn oracle_resolution_endpoints_work_against_sandbox() -> Result<()> {
        let stack = TestStack::start().await?;
        let direct_oracle_id = stack
            .harness
            .deploy_mock_oracle("direct-oracle.near".parse()?)
            .await?;
        let lst_oracle_id = stack
            .harness
            .deploy_lst_oracle("lst-oracle.near".parse()?, direct_oracle_id.clone())
            .await?;
        let proxy_oracle_id = stack.harness.deploy_proxy_oracle().await?;

        let direct_price_id = test_utils::DEFAULT_BORROW_PRICE_ID;
        let transformed_price_id = PriceIdentifier([0xa6; 32]);
        let proxy_direct_id = PriceIdentifier([0x01; 32]);
        let proxy_redstone_id = PriceIdentifier([0x02; 32]);

        stack
            .harness
            .create_lst_transformer(
                lst_oracle_id.clone(),
                transformed_price_id,
                PriceTransformer::lst(
                    direct_price_id,
                    24,
                    price_transformer::Call {
                        account_id: stack.harness.ft_contract_id.clone(),
                        method_name: "redemption_rate".to_owned(),
                        args: near_sdk::json_types::Base64VecU8(serde_json::to_vec(
                            &serde_json::Value::Null,
                        )?),
                        gas: near_sdk::json_types::U64(near_sdk::Gas::from_tgas(3).as_gas()),
                    },
                ),
            )
            .await?;

        let _ = stack
            .controller
            .request::<proxy_oracle_governance::Create>(&WriteRequest {
                signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: proxy_oracle_governance::CreateBody {
                    oracle_id: proxy_oracle_id.clone(),
                    id: 0,
                    operation: templar_common::oracle::proxy::governance::Operation::SetProxy {
                        id: proxy_direct_id,
                        proxy: Some(Proxy::median_low([OracleRequest::pyth(
                            direct_oracle_id.clone(),
                            direct_price_id,
                        )
                        .into()])),
                    },
                },
            })
            .await?;
        let _ = stack
            .controller
            .request::<proxy_oracle_governance::Execute>(&WriteRequest {
                signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: proxy_oracle_governance::ExecuteBody {
                    oracle_id: proxy_oracle_id.clone(),
                    id: 0,
                },
            })
            .await?;

        let _ = stack
            .controller
            .request::<proxy_oracle_governance::Create>(&WriteRequest {
                signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: proxy_oracle_governance::CreateBody {
                    oracle_id: proxy_oracle_id.clone(),
                    id: 1,
                    operation: templar_common::oracle::proxy::governance::Operation::SetProxy {
                        id: proxy_redstone_id,
                        proxy: Some(Proxy::median_low([OracleRequest::redstone(
                            direct_oracle_id.clone(),
                            "BTC",
                        )
                        .into()])),
                    },
                },
            })
            .await?;
        let _ = stack
            .controller
            .request::<proxy_oracle_governance::Execute>(&WriteRequest {
                signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
                idempotency_key: None,
                wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
                body: proxy_oracle_governance::ExecuteBody {
                    oracle_id: proxy_oracle_id.clone(),
                    id: 1,
                },
            })
            .await?;

        let direct = stack
            .controller
            .request::<oracle::GetPriceResolutionDependencies>(&ReadRequest {
                params: oracle::GetPriceResolutionDependenciesParams {
                    oracle_id: direct_oracle_id.clone(),
                    price_id: direct_price_id,
                },
            })
            .await?;
        assert_eq!(direct.kind, oracle::OracleContractKind::Direct);
        assert_eq!(
            direct.requests,
            vec![OracleRequest::pyth(
                direct_oracle_id.clone(),
                direct_price_id
            )]
        );

        let lst = stack
            .controller
            .request::<oracle::GetPriceResolutionDependencies>(&ReadRequest {
                params: oracle::GetPriceResolutionDependenciesParams {
                    oracle_id: lst_oracle_id.clone(),
                    price_id: transformed_price_id,
                },
            })
            .await?;
        assert_eq!(
            lst.kind,
            oracle::OracleContractKind::Lst {
                pyth_id: direct_oracle_id.clone()
            }
        );
        assert_eq!(
            lst.requests,
            vec![OracleRequest::pyth(
                direct_oracle_id.clone(),
                direct_price_id
            )]
        );

        let proxy = stack
            .controller
            .request::<oracle::GetPriceResolutionDependencies>(&ReadRequest {
                params: oracle::GetPriceResolutionDependenciesParams {
                    oracle_id: proxy_oracle_id.clone(),
                    price_id: proxy_direct_id,
                },
            })
            .await?;
        assert_eq!(proxy.kind, oracle::OracleContractKind::Proxy);
        assert_eq!(
            proxy.requests,
            vec![OracleRequest::pyth(
                direct_oracle_id.clone(),
                direct_price_id
            )]
        );

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

        let prices = stack
            .controller
            .request::<oracle::ResolvePrices>(&ReadRequest {
                params: oracle::ResolvePricesParams {
                    oracle_id: proxy_oracle_id,
                    price_ids: vec![proxy_direct_id, proxy_redstone_id],
                    age: 60,
                    pyth: vec![oracle::PythOraclePrices {
                        oracle_id: direct_oracle_id.clone(),
                        response: [(direct_price_id, Some(pyth_price(100.0)))]
                            .into_iter()
                            .collect(),
                    }],
                    redstone: vec![oracle::RedStoneOraclePrices {
                        oracle_id: direct_oracle_id.clone(),
                        response: vec![oracle::RedStonePriceEntry {
                            feed_id: "BTC".into(),
                            data: redstone_price(42.0),
                        }],
                    }],
                },
            })
            .await?;

        assert_eq!(prices.prices.len(), 2);
        assert_eq!(prices.prices[0].price_id, proxy_direct_id);
        assert_same_pyth_price_value(prices.prices[0].price.clone(), pyth_price(100.0));
        assert_eq!(prices.prices[1].price_id, proxy_redstone_id);
        assert_same_pyth_price_value(
            prices.prices[1].price.clone(),
            redstone_price(42.0)
                .to_pyth_price()
                .expect("redstone price should convert to pyth price"),
        );

        let one_price = stack
            .controller
            .request::<oracle::ResolvePrice>(&ReadRequest {
                params: oracle::ResolvePriceParams {
                    oracle_id: lst_oracle_id.clone(),
                    price_id: transformed_price_id,
                    age: 60,
                    pyth: vec![oracle::PythOraclePrices {
                        oracle_id: direct_oracle_id.clone(),
                        response: [(direct_price_id, Some(pyth_price(100.0)))]
                            .into_iter()
                            .collect(),
                    }],
                    redstone: vec![],
                },
            })
            .await?;
        assert!(one_price.price.is_some());

        stack
            .harness
            .set_mock_oracle_pyth_price(
                direct_oracle_id.clone(),
                direct_price_id,
                Some(pyth_price(123.0)),
            )
            .await?;
        stack
            .harness
            .set_mock_oracle_redstone_price(
                direct_oracle_id.clone(),
                "BTC".into(),
                Some(redstone_price(55.0)),
            )
            .await?;

        let on_chain = stack
            .controller
            .request::<oracle::GetPrices>(&ReadRequest {
                params: oracle::GetPricesParams {
                    oracle_id: lst_oracle_id,
                    price_ids: vec![direct_price_id, transformed_price_id],
                    age: 60,
                },
            })
            .await?;

        assert_eq!(on_chain.prices.len(), 2);
        assert_eq!(on_chain.prices[0].price_id, direct_price_id);
        let direct = on_chain.prices[0]
            .price
            .clone()
            .expect("direct price should resolve");
        let expected = pyth_price(123.0);
        assert_eq!(direct.price, expected.price);
        assert_eq!(direct.conf, expected.conf);
        assert_eq!(direct.expo, expected.expo);
        assert!(on_chain.prices[1].price.is_some());

        let one_on_chain = stack
            .controller
            .request::<oracle::GetPrice>(&ReadRequest {
                params: oracle::GetPriceParams {
                    oracle_id: direct_oracle_id,
                    price_id: direct_price_id,
                    age: 60,
                },
            })
            .await?;
        assert!(one_on_chain.price.is_some());

        stack.shutdown().await;
        Ok(())
    }
}
