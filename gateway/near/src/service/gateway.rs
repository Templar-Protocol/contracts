use std::{collections::HashMap, sync::Arc};

use actix::Addr;
use blockchain_gateway_core::{
    operation::OperationStatus, rpc::common::WriteOperationResult, ManagedAccountId,
};
use near_api::advanced::{
    tx_rpc::{TransactionStatusRef, TransactionStatusRpc},
    RequestBuilder, TransactionStatusHandler,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::{
    actor::{
        map_mailbox_error, DispatchRead, HasIdempotencyKey, HasSignerAccountId, ManagedSigner,
        PlanWrite, ReadActor, RpcMessage, WriteActors,
    },
    operation::{CurrentStep, CurrentStepRef, StoredOperation},
    store::{MemoryOperationStore, SharedOperationStore},
    GatewayContext, GatewayResult,
};

use super::runtime::{spawn_runtime, GatewayRuntime};

#[derive(Clone)]
pub struct GatewayService {
    inner: Arc<GatewayInner>,
    runtime: Arc<Mutex<Option<GatewayRuntime>>>,
}

struct GatewayInner {
    context: GatewayContext,
    read: Addr<ReadActor>,
    write: WriteActors,
    store: SharedOperationStore,
}

impl GatewayService {
    pub fn spawn(
        context: GatewayContext,
        signers: HashMap<ManagedAccountId, ManagedSigner>,
    ) -> Self {
        Self::spawn_with_store(context, signers, Arc::new(MemoryOperationStore::new()))
    }

    pub fn spawn_with_store(
        context: GatewayContext,
        signers: HashMap<ManagedAccountId, ManagedSigner>,
        store: SharedOperationStore,
    ) -> Self {
        let (runtime, read, write) = spawn_runtime(context.clone(), signers);

        let service = Self {
            inner: Arc::new(GatewayInner {
                context,
                read,
                write,
                store,
            }),
            runtime: Arc::new(Mutex::new(Some(runtime))),
        };

        tokio::spawn({
            let service = service.clone();
            async move {
                let _ = service.resume_incomplete_operations().await;
            }
        });

        service
    }

    pub async fn shutdown(self) {
        if let Some(runtime) = self.runtime.lock().await.take() {
            runtime.shutdown();
        }
    }

    pub async fn request_read<Request>(
        &self,
        params: Request::Input,
    ) -> GatewayResult<Request::Output>
    where
        Request: DispatchRead,
        ReadActor: actix::Handler<RpcMessage<Request>>,
    {
        self.inner
            .read
            .send(RpcMessage(params))
            .await
            .map_err(|error| map_mailbox_error(error, "read-actor"))?
    }

    pub async fn get_operation(
        &self,
        operation_id: &blockchain_gateway_core::OperationId,
    ) -> GatewayResult<Option<blockchain_gateway_core::OperationRecord>> {
        Ok(self
            .inner
            .store
            .get_by_id(operation_id)
            .await?
            .map(|operation| operation.operation_record()))
    }

    pub async fn request_write<Request>(
        &self,
        params: Request::Input,
    ) -> GatewayResult<Request::Output>
    where
        Request: PlanWrite,
    {
        let request_payload = serde_json::to_vec(&params)?;
        let fingerprint = make_request_fingerprint(Request::RPC_METHOD, &params)?;
        if let Some(idempotency_key) = params.idempotency_key() {
            if let Some(existing) = self
                .inner
                .store
                .get_by_idempotency_key(idempotency_key)
                .await?
            {
                if existing.request_fingerprint_hash != fingerprint {
                    return Err(crate::GatewayError::IdempotencyConflict);
                }
                return Ok(operation_result_from_stored(existing));
            }
        }

        let plan = Request::plan(params.clone(), self.read_context()).await?;
        let operation = self
            .inner
            .store
            .create_operation(
                Request::RPC_METHOD,
                params.signer_account_id().to_owned(),
                params.idempotency_key().cloned(),
                fingerprint,
                request_payload,
                plan,
            )
            .await?;

        let operation = self.execute_remaining_steps(operation).await?;
        Ok(operation_result_from_stored(operation))
    }

    async fn resume_incomplete_operations(&self) -> GatewayResult<()> {
        for mut operation in self.inner.store.list_incomplete_operations().await? {
            if matches!(operation.current_step, Some(CurrentStep::Submitted { .. })) {
                if let Some(CurrentStep::Submitted {
                    transaction,
                    tx_hash,
                }) = operation.current_step.take()
                {
                    match self
                        .query_submitted_transaction(&transaction.signer_account_id, tx_hash)
                        .await
                    {
                        Ok(execution) if execution.is_success() => {
                            operation
                                .succeeded_steps
                                .push(crate::operation::SucceededStep {
                                    transaction,
                                    tx_hash,
                                });
                            operation.current_step = None;
                        }
                        Ok(_) | Err(_) => {
                            operation.current_step = Some(CurrentStep::Failed {
                                transaction,
                                tx_hash: Some(tx_hash),
                            });
                        }
                    }
                }
                self.inner.store.save_operation(operation).await?;
                continue;
            }

            let _ = self.execute_remaining_steps(operation).await;
        }
        Ok(())
    }

    async fn execute_remaining_steps(
        &self,
        mut operation: StoredOperation,
    ) -> GatewayResult<StoredOperation> {
        while matches!(
            operation.status(),
            OperationStatus::Pending | OperationStatus::InProgress
        ) {
            operation = self.execute_next_step(operation).await?;
            if operation.status() == OperationStatus::Failed {
                break;
            }
        }
        Ok(operation)
    }

    async fn execute_next_step(
        &self,
        mut operation: StoredOperation,
    ) -> GatewayResult<StoredOperation> {
        if operation.current_step_is_failed() {
            return Ok(operation);
        }

        if let Some(pending) = operation.begin_next_preparation(self.inner.store.clone()) {
            let prepared = self
                .inner
                .write
                .prepare_planned_transaction(pending.transaction().clone())
                .await?;
            pending.finish(prepared).await?;
        }

        match operation.current(self.inner.store.clone()) {
            Some(CurrentStepRef::Prepared(prepared_step)) => {
                let wait_until = prepared_step.wait_until();
                let (signed_transaction, submitted_step) = prepared_step.submit().await?;
                let signer_account_id = submitted_step.transaction().signer_account_id.clone();
                let tx_hash = submitted_step.tx_hash();

                match self
                    .inner
                    .write
                    .submit_signed_transaction(&signer_account_id, signed_transaction, wait_until)
                    .await
                {
                    Ok(tx_result) => {
                        if let Some(full) = tx_result.into_full() {
                            let final_hash = full.outcome().transaction_hash.into();
                            if full.is_success() {
                                submitted_step.succeed(final_hash).await?;
                            } else {
                                submitted_step.fail(Some(final_hash)).await?;
                            }
                        }
                    }
                    Err(error) => {
                        submitted_step.fail(Some(tx_hash)).await?;
                        return Err(error);
                    }
                }
            }
            Some(CurrentStepRef::Submitted(submitted_step)) => {
                let tx_hash = submitted_step.tx_hash();
                submitted_step.fail(Some(tx_hash)).await?;
            }
            Some(CurrentStepRef::Failed) => {}
            None => {}
        }

        Ok(operation)
    }

    fn read_context(&self) -> GatewayContext {
        self.inner.context.clone()
    }

    async fn query_submitted_transaction(
        &self,
        signer_account_id: &ManagedAccountId,
        tx_hash: blockchain_gateway_core::CryptoHash,
    ) -> GatewayResult<near_api::types::transaction::result::ExecutionFinalResult> {
        RequestBuilder::new(
            TransactionStatusRpc,
            TransactionStatusRef {
                sender_account_id: signer_account_id.0.clone(),
                tx_hash: tx_hash.0,
                wait_until: near_api::types::TxExecutionStatus::Final,
            },
            TransactionStatusHandler,
        )
        .fetch_from(self.inner.context.network())
        .await
        .map_err(|error| crate::GatewayError::NearTransaction(error.to_string()))
    }
}

fn operation_result_from_stored(operation: StoredOperation) -> WriteOperationResult {
    WriteOperationResult {
        operation: operation.operation_record(),
    }
}

fn make_request_fingerprint<T: Serialize>(method: &str, params: &T) -> GatewayResult<[u8; 32]> {
    let payload = serde_json_canonicalizer::to_vec(&serde_json::json!({
        "method": method,
        "params": params,
    }))?;
    Ok(Sha256::digest(payload).into())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::Path, sync::Arc};

    use anyhow::Result;
    use blockchain_gateway_core::{
        common::{ContractArgs, WriteRequest},
        tx, ContractMethodName, IdempotencyKey, MethodSpec, NearGas, NearToken,
    };
    use near_api::{types::AccountId, Contract, NetworkConfig, SecretKey, Signer};
    use near_sandbox::Sandbox;
    use near_token::NearToken as SandboxNearToken;
    use test_utils::FtController;
    use url::Url;

    use super::*;

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

        let context = crate::GatewayContext::new(
            network.clone(),
            Url::parse("https://hermes-beta.pyth.network")?,
            Path::new("node"),
        )?;
        let service = GatewayService::spawn(context, gateway_signers);

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
            .request_write::<tx::FunctionCall>(WriteRequest {
                signer_account_id: harness.gateway_signer_account_id.clone(),
                idempotency_key: Some(IdempotencyKey("same-key".to_owned())),
                body: tx::FunctionCallBody {
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
            .request_write::<tx::FunctionCall>(WriteRequest {
                signer_account_id: harness.gateway_signer_account_id.clone(),
                idempotency_key: Some(IdempotencyKey("same-key".to_owned())),
                body: tx::FunctionCallBody {
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

        assert!(matches!(
            second,
            Err(crate::GatewayError::IdempotencyConflict)
        ));
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
            body: tx::FunctionCallBody {
                receiver_id: harness.ft_contract_id.clone(),
                method_name: ContractMethodName("set_redemption_rate".to_owned()),
                args: ContractArgs::Json(serde_json::json!({
                    "redemption_rate": NearToken::from_near(4).as_yoctonear().to_string(),
                })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::from_yoctonear(0),
            },
        };

        let fingerprint = make_request_fingerprint(tx::FunctionCall::RPC_METHOD, &request)?;
        let payload = serde_json::to_vec(&request)?;
        let plan =
            <tx::FunctionCall as PlanWrite>::plan(request.clone(), service.read_context()).await?;
        let mut operation = service
            .inner
            .store
            .create_operation(
                tx::FunctionCall::RPC_METHOD,
                request.signer_account_id().to_owned(),
                request.idempotency_key().cloned(),
                fingerprint,
                payload,
                plan,
            )
            .await?;

        let pending = operation
            .begin_next_preparation(service.inner.store.clone())
            .expect("step should be preparable");
        let prepared = service
            .inner
            .write
            .prepare_planned_transaction(pending.transaction().clone())
            .await?;
        pending.finish(prepared).await?;

        let (signed_transaction, submitted_step) =
            match operation.current(service.inner.store.clone()) {
                Some(CurrentStepRef::Prepared(prepared_step)) => prepared_step.submit().await?,
                _ => panic!("expected prepared current step"),
            };
        let signer_account_id = submitted_step.transaction().signer_account_id.clone();
        let wait_until = submitted_step.transaction().wait_until;

        let _ = service
            .inner
            .write
            .submit_signed_transaction(&signer_account_id, signed_transaction, wait_until)
            .await?;
        drop(submitted_step);

        service.resume_incomplete_operations().await?;

        let stored = service
            .inner
            .store
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
            body: tx::FunctionCallBody {
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
            body: tx::FunctionCallBody {
                receiver_id: harness.ft_contract_id.clone(),
                method_name: ContractMethodName("set_redemption_rate".to_owned()),
                args: ContractArgs::Json(serde_json::json!({
                    "redemption_rate": NearToken::from_near(3).as_yoctonear().to_string(),
                })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::from_yoctonear(0),
            },
        };

        let mut first_plan =
            <tx::FunctionCall as PlanWrite>::plan(first_request.clone(), service.read_context())
                .await?;
        let second_plan =
            <tx::FunctionCall as PlanWrite>::plan(second_request, service.read_context()).await?;
        first_plan.push(
            second_plan
                .steps
                .into_iter()
                .next()
                .expect("second step should exist"),
        );

        let fingerprint = make_request_fingerprint(tx::FunctionCall::RPC_METHOD, &first_request)?;
        let payload = serde_json::to_vec(&first_request)?;
        let operation = service
            .inner
            .store
            .create_operation(
                tx::FunctionCall::RPC_METHOD,
                harness.gateway_signer_account_id.clone(),
                Some(IdempotencyKey("multi-step-sequence".to_owned())),
                fingerprint,
                payload,
                first_plan,
            )
            .await?;
        let stored = service.execute_remaining_steps(operation).await?;

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
