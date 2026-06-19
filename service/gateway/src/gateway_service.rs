use std::{collections::HashMap, marker::PhantomData, sync::Arc};

use actix::Addr;
use templar_gateway_core::{
    DispatchRead, GatewayContext, GatewayResult, HasIdempotencyKey, HasNearClient,
    HasSignerAccountId, NearOperationExecutor, NearTransactionSigner, OperationDriver, PlanWrite,
    SharedOperationStore,
};
use templar_gateway_runtime::{
    map_mailbox_error, spawn_runtime, GatewayRuntime, ManagedSigner, ReadActor, RpcMessage,
};
use templar_gateway_types::{
    common::WriteOperationResult, ManagedAccountId, MethodSpec, OperationId, OperationRecord,
};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct GatewayService<ContextType: Clone + Send + std::marker::Unpin + 'static = GatewayContext>
{
    inner: Arc<GatewayInner<ContextType>>,
    runtime: Arc<Mutex<Option<GatewayRuntime>>>,
}

struct GatewayInner<ContextType: Clone + Send + std::marker::Unpin + 'static> {
    context: ContextType,
    driver: OperationDriver,
    read: Addr<ReadActor<ContextType>>,
}

impl<ContextType> GatewayService<ContextType>
where
    ContextType: HasNearClient + Clone + Send + std::marker::Unpin + 'static,
{
    pub fn spawn(
        context: ContextType,
        signers: HashMap<ManagedAccountId, ManagedSigner>,
        store: SharedOperationStore,
    ) -> GatewayResult<Self> {
        let signer_count = signers.len();
        let signers = signers
            .into_iter()
            .map(|(account_id, signer)| (account_id, signer.signer))
            .collect();

        let signer = NearTransactionSigner::new(context.near_client().network().clone(), signers);
        let executor = NearOperationExecutor::new(context.near_client().network().clone());
        let driver = OperationDriver::new(store, Arc::new(signer), Arc::new(executor));

        let (runtime, read) = spawn_runtime(context.clone())?;
        tracing::debug!(signer_count, "gateway service runtime initialized");

        let service = Self {
            inner: Arc::new(GatewayInner {
                context,
                driver,
                read,
            }),
            runtime: Arc::new(Mutex::new(Some(runtime))),
        };

        tokio::spawn({
            let driver = service.inner.driver.clone();
            async move {
                if let Err(error) = driver.resume_incomplete_operations().await {
                    tracing::warn!(
                        error = %error,
                        "failed to list or persist incomplete gateway operations during startup recovery"
                    );
                }
            }
        });

        Ok(service)
    }

    pub async fn shutdown(self) {
        if let Some(runtime) = self.runtime.lock().await.take() {
            runtime.shutdown();
        }
    }

    pub async fn request_read<Request, Impl>(
        &self,
        params: Request::Input,
    ) -> GatewayResult<Request::Output>
    where
        Request: MethodSpec + 'static,
        Impl: DispatchRead<Request, ContextType>,
        ReadActor<ContextType>: actix::Actor<Context = actix::Context<ReadActor<ContextType>>>
            + actix::Handler<RpcMessage<Request, Impl>>,
    {
        self.inner
            .read
            .send(RpcMessage(params, PhantomData))
            .await
            .map_err(|error| map_mailbox_error(error, "read-actor"))?
    }

    pub async fn get_operation(
        &self,
        operation_id: &OperationId,
    ) -> GatewayResult<Option<OperationRecord>> {
        self.inner.driver.get_operation(operation_id).await
    }

    pub async fn request_write<Request, Impl>(
        &self,
        params: Request::Input,
    ) -> GatewayResult<Request::Output>
    where
        Request: MethodSpec<Output = WriteOperationResult> + 'static,
        Request::Input: HasIdempotencyKey + HasSignerAccountId,
        Impl: PlanWrite<Request, ContextType>,
    {
        tracing::debug!(
            rpc_method = Request::RPC_METHOD,
            signer_account_id = %params.signer_account_id().0,
            has_idempotency_key = params.idempotency_key().is_some(),
            "planning gateway write request"
        );
        let plan = Impl::plan(params.clone(), self.inner.context.clone()).await?;
        tracing::debug!(
            rpc_method = Request::RPC_METHOD,
            step_count = plan.steps.len(),
            "planned gateway write request"
        );
        self.inner
            .driver
            .complete_write(Request::RPC_METHOD, params, plan)
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use super::*;
    use anyhow::Result;
    use near_api::{types::AccountId, Contract, NetworkConfig, SecretKey, Signer};
    use near_sandbox::Sandbox;
    use near_token::NearToken as SandboxNearToken;
    use templar_gateway_core::{CreateOperationResult, GatewayContext, GatewayError};
    use templar_gateway_methods_dispatch::Dispatch;
    use templar_gateway_methods_spec::tx;
    use templar_gateway_types::{
        common::{ContractArgs, WriteRequest},
        ContractMethodName, IdempotencyKey, MethodSpec, NearGas, NearToken, OperationStatus,
    };
    use test_utils::FtController;

    struct TestHarness {
        _sandbox: Sandbox,
        gateway_signer_account_id: ManagedAccountId,
        network: NetworkConfig,
        ft_contract_id: AccountId,
    }

    async fn start_service() -> Result<(TestHarness, GatewayService)> {
        let sandbox = Sandbox::start_sandbox().await?;
        let network = NetworkConfig::from_rpc_url("sandbox", sandbox.rpc_addr.parse()?);

        let gateway_signer_account_id = ManagedAccountId("gateway.near".parse()?);
        let gateway_secret_key = test_secret_key()?;
        sandbox
            .create_account(gateway_signer_account_id.0.clone())
            .initial_balance(SandboxNearToken::from_near(100))
            .public_key(gateway_secret_key.public_key().to_string())
            .send()
            .await?;

        let gateway_signers = HashMap::from([(
            gateway_signer_account_id.clone(),
            ManagedSigner::new([gateway_secret_key])
                .await
                .expect("failed to create managed signer"),
        )]);

        let ft_contract_id: AccountId = "mock-ft.near".parse()?;
        let ft_signer =
            create_account_signer(&sandbox, &ft_contract_id, SandboxNearToken::from_near(100))
                .await?;
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
        let service = GatewayService::spawn(
            context,
            gateway_signers,
            Arc::new(templar_gateway_store::MemoryStore::new()),
        )?;

        Ok((
            TestHarness {
                _sandbox: sandbox,
                gateway_signer_account_id,
                network,
                ft_contract_id,
            },
            service,
        ))
    }

    #[tokio::test]
    async fn idempotency_conflict_is_rejected_for_different_payloads() -> Result<()> {
        let (harness, service) = start_service().await?;

        let first = service
            .request_write::<tx::FunctionCall, Dispatch>(WriteRequest {
                signer_account_id: harness.gateway_signer_account_id.clone(),
                idempotency_key: Some(IdempotencyKey("same-key".to_owned())),
                body: tx::FunctionCall {
                    receiver_id: harness.ft_contract_id.clone(),
                    method_name: ContractMethodName("set_redemption_rate".to_owned()),
                    args: ContractArgs::Json(serde_json::json!({
                        "redemption_rate": NearToken::from_near(2).as_yoctonear().to_string(),
                    })),
                    gas: NearGas::from_tgas(100),
                    deposit: NearToken::from_yoctonear(0),
                },
            })
            .await?;

        let second = service
            .request_write::<tx::FunctionCall, Dispatch>(WriteRequest {
                signer_account_id: harness.gateway_signer_account_id.clone(),
                idempotency_key: Some(IdempotencyKey("same-key".to_owned())),
                body: tx::FunctionCall {
                    receiver_id: harness.ft_contract_id.clone(),
                    method_name: ContractMethodName("set_redemption_rate".to_owned()),
                    args: ContractArgs::Json(serde_json::json!({
                        "redemption_rate": NearToken::from_near(3).as_yoctonear().to_string(),
                    })),
                    gas: NearGas::from_tgas(100),
                    deposit: NearToken::from_yoctonear(0),
                },
            })
            .await;

        assert!(matches!(second, Err(GatewayError::IdempotencyConflict)));
        assert_eq!(first.operation.steps.len(), 1);
        service.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn recovery_reconciles_submitted_step_to_success() -> Result<()> {
        let (harness, service) = start_service().await?;
        let request = WriteRequest {
            signer_account_id: harness.gateway_signer_account_id.clone(),
            idempotency_key: Some(IdempotencyKey("recovery-key".to_owned())),
            body: tx::FunctionCall {
                receiver_id: harness.ft_contract_id.clone(),
                method_name: ContractMethodName("set_redemption_rate".to_owned()),
                args: ContractArgs::Json(serde_json::json!({
                    "redemption_rate": NearToken::from_near(4).as_yoctonear().to_string(),
                })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::from_yoctonear(0),
            },
        };

        let plan = <Dispatch as PlanWrite<tx::FunctionCall, GatewayContext>>::plan(
            request.clone(),
            service.inner.context.clone(),
        )
        .await?;
        let operation = match service
            .inner
            .driver
            .create_planned_operation(tx::FunctionCall::RPC_METHOD, &request, plan)
            .await?
        {
            CreateOperationResult::Created(operation) => operation,
            CreateOperationResult::Existing(_) => panic!("expected new operation"),
        };

        service
            .inner
            .driver
            .submit_next_step_unchecked(operation)
            .await?;

        service.inner.driver.resume_incomplete_operations().await?;

        let stored = service
            .inner
            .driver
            .get_by_idempotency_key(&IdempotencyKey("recovery-key".to_owned()))
            .await?
            .expect("stored operation should exist");
        assert_eq!(stored.status(), OperationStatus::Succeeded);
        assert!(stored.current_step.is_none());
        assert_eq!(stored.succeeded_steps.len(), 1);

        service.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn multi_step_operation_records_full_succeeded_sequence() -> Result<()> {
        let (harness, service) = start_service().await?;
        let first_request = WriteRequest {
            signer_account_id: harness.gateway_signer_account_id.clone(),
            idempotency_key: Some(IdempotencyKey("multi-step-sequence".to_owned())),
            body: tx::FunctionCall {
                receiver_id: harness.ft_contract_id.clone(),
                method_name: ContractMethodName("set_redemption_rate".to_owned()),
                args: ContractArgs::Json(serde_json::json!({
                    "redemption_rate": NearToken::from_near(2).as_yoctonear().to_string(),
                })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::from_yoctonear(0),
            },
        };
        let second_request = WriteRequest {
            signer_account_id: harness.gateway_signer_account_id.clone(),
            idempotency_key: Some(IdempotencyKey("multi-step-sequence".to_owned())),
            body: tx::FunctionCall {
                receiver_id: harness.ft_contract_id.clone(),
                method_name: ContractMethodName("set_redemption_rate".to_owned()),
                args: ContractArgs::Json(serde_json::json!({
                    "redemption_rate": NearToken::from_near(3).as_yoctonear().to_string(),
                })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::from_yoctonear(0),
            },
        };

        let mut first_plan = <Dispatch as PlanWrite<tx::FunctionCall, GatewayContext>>::plan(
            first_request.clone(),
            service.inner.context.clone(),
        )
        .await?;
        let second_plan = <Dispatch as PlanWrite<tx::FunctionCall, GatewayContext>>::plan(
            second_request,
            service.inner.context.clone(),
        )
        .await?;
        first_plan.push(
            second_plan
                .steps
                .into_iter()
                .next()
                .expect("second step should exist"),
        );

        let operation = match service
            .inner
            .driver
            .create_planned_operation(tx::FunctionCall::RPC_METHOD, &first_request, first_plan)
            .await?
        {
            CreateOperationResult::Created(operation) => operation,
            CreateOperationResult::Existing(_) => panic!("expected new operation"),
        };
        let stored = service
            .inner
            .driver
            .execute_remaining_steps(operation)
            .await?;

        assert_eq!(stored.status(), OperationStatus::Succeeded);
        assert!(stored.current_step.is_none());
        assert!(stored.remaining_steps.is_empty());
        assert_eq!(stored.succeeded_steps.len(), 2);

        let rate: near_api::Data<String> = Contract(harness.ft_contract_id.clone())
            .call_function("redemption_rate", ())
            .read_only()
            .fetch_from(&harness.network)
            .await?;
        assert_eq!(
            rate.data,
            NearToken::from_near(3).as_yoctonear().to_string()
        );

        service.shutdown().await;
        Ok(())
    }

    async fn create_account_signer(
        sandbox: &Sandbox,
        account_id: &AccountId,
        initial_balance: SandboxNearToken,
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
        account_id: AccountId,
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
}
