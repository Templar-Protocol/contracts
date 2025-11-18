use std::{
    borrow::Borrow,
    collections::{hash_map::Entry, HashMap, HashSet},
    future::Future,
    sync::Arc,
    time::Duration,
};

use near_crypto::InMemorySigner;
use near_jsonrpc_client::{errors::JsonRpcError, methods::tx::RpcTransactionError};
use near_primitives::{
    action::{delegate::SignedDelegateAction, Action, FunctionCallAction},
    transaction::SignedTransaction,
    views::{FinalExecutionOutcomeView, TxExecutionStatus},
};
use near_sdk::{serde_json, AccountId, AccountIdRef, NearToken};
use templar_common::market::DepositMsg;
use tokio::{
    sync::{watch, RwLock},
    task::JoinSet,
};

use crate::{
    broom,
    cache::Cache,
    client::{
        database::{
            error::{RecordTransactionError, SetPendingTransactionError},
            Database,
        },
        near::Near,
        pyth::Pyth,
    },
    error::{FunctionCallRejectionReason, PayloadRejectionReason},
    AccountData, AssetTransfer, ContractData,
};

pub mod args;
pub use args::Configuration;

#[derive(Debug, Clone)]
pub struct App {
    pub args: args::Configuration,
    pub accounts: Arc<RwLock<AccountData>>,
    pub relay_near: Near,
    pub ua_near: Near,
    pub pyth: Pyth,
    pub cache: Arc<Cache>,
    pub database: Database,
}

impl App {
    pub fn new(args: args::Configuration, kill: watch::Sender<()>) -> Self {
        let relay_near = Near::new(
            near_jsonrpc_client::JsonRpcClient::connect(&args.rpc_url),
            args.relay.account_id.clone(),
            args.relay
                .secret_key
                .iter()
                .map(|s| InMemorySigner::from_secret_key(args.relay.account_id.clone(), s.clone()))
                .collect(),
        );

        let ua_near = Near::new(
            near_jsonrpc_client::JsonRpcClient::connect(&args.rpc_url),
            args.ua.account_id.clone(),
            args.ua
                .secret_key
                .iter()
                .map(|s| InMemorySigner::from_secret_key(args.ua.account_id.clone(), s.clone()))
                .collect(),
        );

        #[allow(clippy::unwrap_used)]
        let database = Database::new(&args.database_url, kill.clone()).unwrap();

        let cache = Cache::new(relay_near.clone(), args.cache.clone(), kill.clone());

        let pyth = Pyth::new(
            args.pyth.clone(),
            relay_near.clone(),
            cache.clone(),
            kill.clone(),
        );

        tokio::spawn(broom::start(
            database.clone(),
            relay_near.clone(),
            args.broom_batch_size,
            Duration::from_secs(args.broom_interval_secs),
            kill,
        ));

        Self {
            args,
            accounts: Arc::new(RwLock::new(AccountData::default())),
            relay_near,
            ua_near,
            pyth,
            cache: Arc::new(cache),
            database,
        }
    }

    #[tracing::instrument(skip(self), fields(gas = %gas))]
    pub async fn estimate_cost_of_gas(&self, gas: near_sdk::Gas) -> Option<NearToken> {
        const TERA: u128 = near_sdk::Gas::from_tgas(1).as_gas() as u128;

        let price_per_tgas = self.cache.gas_price().await;
        let result = price_per_tgas
            .checked_mul(u128::from(gas.as_gas()))
            .and_then(|x| x.checked_div(TERA));

        tracing::debug!(cost = ?result, "Estimated gas cost");
        result
    }

    #[allow(clippy::too_many_lines, reason = "procedural")]
    #[tracing::instrument(skip(self))]
    pub async fn load_markets(&mut self) {
        tracing::info!("Loading markets from registry and individual sources");
        let mut markets = self.args.monitor.market.clone();

        // Load markets from registry...
        let mut set = JoinSet::new();
        for registry_id in &self.args.monitor.registry {
            set.spawn({
                let near = self.relay_near.clone();
                let registry_id = registry_id.clone();
                async move {
                    near.load_deployments_from_registry(registry_id.clone())
                        .await
                        .unwrap_or_else(|e| {
                            tracing::warn!(
                                "Failed to load deployments from registry {registry_id}: {e}"
                            );
                            vec![]
                        })
                }
            });
        }
        markets.extend(set.join_all().await.into_iter().flatten());

        // ...and add any individual markets.
        let mut set = JoinSet::new();
        for market in markets {
            set.spawn({
                let near = self.relay_near.clone();
                async move {
                    match near.load_market_accounts(market.clone()).await {
                        Ok(market_accounts) => Some(market_accounts),
                        Err(e) => {
                            tracing::warn!("Failed to load accounts for market {market}: {e}");
                            None
                        }
                    }
                }
            });
        }
        let market_accounts_vec = set.join_all().await;

        let mut markets = HashMap::new();
        let mut allowed_contracts = HashMap::new();
        if let Some(intents_id) = self.args.relay.intents_id.clone() {
            allowed_contracts.insert(
                intents_id,
                ContractData {
                    storage_balance_bounds: None,
                    allowed_methods: self
                        .args
                        .relay
                        .intents_allowed_methods
                        .iter()
                        .cloned()
                        .collect(),
                },
            );
        }

        for market_accounts in market_accounts_vec.into_iter().flatten() {
            tracing::info!(
                "Loaded market {} with borrow asset {} and collateral asset {}, querying oracle {}",
                market_accounts.account_id,
                market_accounts.borrow_asset,
                market_accounts.collateral_asset,
                market_accounts.oracle_id,
            );

            for (contract_id, allowed_methods) in [
                (
                    market_accounts.account_id.as_ref(),
                    self.args.relay.allowed_methods.as_slice(),
                ),
                (
                    market_accounts.borrow_asset.contract_id(),
                    &[market_accounts
                        .borrow_asset
                        .transfer_call_method_name()
                        .to_string()],
                ),
                (
                    market_accounts.collateral_asset.contract_id(),
                    &[market_accounts
                        .collateral_asset
                        .transfer_call_method_name()
                        .to_string()],
                ),
                (
                    &market_accounts.oracle_id,
                    &self.args.relay.oracle_allowed_methods,
                ),
            ] {
                match allowed_contracts.entry(contract_id.to_owned()) {
                    Entry::Vacant(e) => {
                        let storage_balance_bounds = self
                            .relay_near
                            .load_storage_balance_bounds(contract_id.to_owned())
                            .await
                            .ok();

                        tracing::info!(
                            "Loaded storage balance bounds for contract {}: {}",
                            contract_id,
                            storage_balance_bounds
                                .as_ref()
                                .map_or(NearToken::from_near(0), |bounds| bounds.min),
                        );

                        e.insert(ContractData {
                            storage_balance_bounds,
                            allowed_methods: allowed_methods.iter().cloned().collect(),
                        });
                    }
                    Entry::Occupied(mut e) => {
                        e.get_mut()
                            .allowed_methods
                            .extend(allowed_methods.iter().cloned());
                    }
                }
            }

            markets.insert(market_accounts.account_id.clone(), market_accounts);
        }

        let mut handle = self.accounts.write().await;
        handle.market_data = markets;
        handle.allowed_contract_data = allowed_contracts;
    }

    /// Checks that the all of the function call actions are allowed for the specific receiver.
    ///
    /// Returns a list of other accounts that this action will probably interact with in addition to the receiver.
    ///
    /// # Errors
    ///
    /// - If the receiver is not known.
    /// - If any of the function call actions are not allowed.
    #[tracing::instrument(skip(self, accounts, contract_data, calls))]
    pub fn actions_are_allowed<'a>(
        &self,
        accounts: &AccountData,
        receiver_id: &AccountIdRef,
        contract_data: &ContractData,
        calls: impl IntoIterator<Item = &'a FunctionCallAction>,
    ) -> Result<Vec<AccountId>, Vec<FunctionCallRejectionReason>> {
        let mut other_interactions = Vec::new();
        let mut errors = vec![];

        for (index, call) in calls.into_iter().enumerate() {
            if !contract_data.allowed_methods.contains(&call.method_name) {
                errors.push(FunctionCallRejectionReason::UnknownFunctionName {
                    index,
                    function_name: call.method_name.clone(),
                });
            }

            if let Ok(transfer) = AssetTransfer::parse(receiver_id.to_owned(), call) {
                let market_id = transfer.token_receiver_id();
                other_interactions.push(market_id.to_owned());

                let Some(market_account_ids) = accounts.market_data.get(market_id) else {
                    errors.push(FunctionCallRejectionReason::UnknownTransferReceiverId {
                        account_id: market_id.to_owned(),
                        index,
                    });
                    continue;
                };

                let msg = transfer.args.msg();
                let Ok(msg) = serde_json::from_str::<DepositMsg>(msg) else {
                    errors.push(FunctionCallRejectionReason::MsgDeserializationFailure {
                        index,
                        msg: msg.to_string(),
                    });
                    continue;
                };

                #[allow(clippy::unwrap_used, reason = "DepositMsg serialization is infallible")]
                if transfer.asset() == market_account_ids.borrow_asset {
                    if !matches!(msg, DepositMsg::Supply | DepositMsg::Repay) {
                        errors.push(FunctionCallRejectionReason::InvalidMsgForAsset {
                            index,
                            expected: "\"Supply\" or \"Repay\"".to_string(),
                            actual: serde_json::to_string(&msg).unwrap(),
                        });
                    }
                } else if transfer.asset() == market_account_ids.collateral_asset {
                    if !matches!(msg, DepositMsg::Collateralize) {
                        errors.push(FunctionCallRejectionReason::InvalidMsgForAsset {
                            index,
                            expected: "\"Collateralize\"".to_string(),
                            actual: serde_json::to_string(&msg).unwrap(),
                        });
                    }
                } else {
                    // Not a standard-compliant function call
                }
            }
        }

        if errors.is_empty() {
            Ok(other_interactions)
        } else {
            Err(errors)
        }
    }

    pub(crate) fn ua_interacted_contracts_and_gas(
        &self,
        accounts: &AccountData,
        payload: &[templar_universal_account::transaction::Transaction],
    ) -> Result<(HashSet<AccountId>, near_sdk::Gas), Vec<PayloadRejectionReason>> {
        let mut errors = vec![];
        let mut gas = near_sdk::Gas::from_tgas(self.args.ua.execute_tgas).as_gas();
        let mut interacted = HashSet::with_capacity(payload.len());

        for transaction in payload {
            let receiver_id = &transaction.receiver_id;
            if !accounts.allowed_contract_data.contains_key(receiver_id) {
                errors.push(PayloadRejectionReason::UnknownTransactionReceiverId {
                    account_id: receiver_id.clone(),
                });
            }
            let mut calls = Vec::with_capacity(transaction.actions.len());
            for (index, action) in transaction.actions.iter().enumerate() {
                match action {
                    templar_universal_account::transaction::Action::FunctionCall(call)
                    | templar_universal_account::transaction::Action::FunctionCallWeight {
                        call,
                        ..
                    } => calls.push(FunctionCallAction::from((**call).clone())),
                    _ => errors.push(PayloadRejectionReason::UnsupportedAction { index }),
                }
            }
            let Some(contract_data) = accounts.allowed_contract_data.get(receiver_id) else {
                errors.push(PayloadRejectionReason::UnknownTransactionReceiverId {
                    account_id: receiver_id.clone(),
                });
                continue;
            };

            interacted.insert(receiver_id.clone());
            let probably_interacted = match self.actions_are_allowed(
                accounts,
                receiver_id,
                contract_data,
                calls.iter(),
            ) {
                Ok(a) => a,
                Err(e) => {
                    errors.push(PayloadRejectionReason::FunctionCallRejection(e));
                    continue;
                }
            };
            interacted.extend(probably_interacted);
            if let Some(market_data) = accounts.market_data.get(receiver_id) {
                interacted.insert(market_data.oracle_id.clone());
                interacted.insert(market_data.borrow_asset.contract_id().to_owned());
                interacted.insert(market_data.collateral_asset.contract_id().to_owned());
            }
            gas += calls.iter().map(|f| f.gas).sum::<u64>();
        }

        if errors.is_empty() {
            Ok((interacted, near_sdk::Gas::from_gas(gas)))
        } else {
            Err(errors)
        }
    }

    /// Check and calculate gas for a signed delegate action.
    ///
    /// # Errors
    ///
    /// - If the signature verification fails.
    /// - If the receiver ID is unknown.
    /// - If the action is not supported.
    /// - If the function name is not valid.
    /// - If the function arguments are invalid.
    /// - etc. See [`PreconditionError`] for more details.
    #[tracing::instrument(skip(self, signed_delegate_action), fields(
        sender_id = %signed_delegate_action.delegate_action.sender_id,
        receiver_id = %signed_delegate_action.delegate_action.receiver_id
    ))]
    pub async fn sda_check_and_calculate_gas(
        &self,
        signed_delegate_action: &SignedDelegateAction,
    ) -> Result<(near_sdk::Gas, ContractData), PayloadRejectionReason> {
        tracing::debug!("Checking and calculating gas for delegate action");
        if !signed_delegate_action.verify() {
            return Err(PayloadRejectionReason::SignatureVerificationFailure);
        }

        let receiver_id = &signed_delegate_action.delegate_action.receiver_id;
        let accounts = self.accounts.read().await;

        let Some(contract_data) = accounts.allowed_contract_data.get(receiver_id).cloned() else {
            return Err(PayloadRejectionReason::UnknownTransactionReceiverId {
                account_id: receiver_id.clone(),
            });
        };

        let actions = signed_delegate_action.delegate_action.get_actions();
        let len = actions.len();
        let calls = actions
            .into_iter()
            .enumerate()
            .try_fold(Vec::with_capacity(len), |mut v, (i, action)| {
                if let Action::FunctionCall(fc) = action {
                    v.push(fc);
                    Ok(v)
                } else {
                    Err(i)
                }
            })
            .map_err(|index| PayloadRejectionReason::UnsupportedAction { index })?;

        self.actions_are_allowed(
            &accounts,
            receiver_id,
            &contract_data,
            calls.iter().map(Borrow::borrow),
        )
        .map_err(PayloadRejectionReason::FunctionCallRejection)?;

        let gas_total = calls.iter().map(|call| call.gas).sum();

        Ok((near_sdk::Gas::from_gas(gas_total), contract_data))
    }

    /// # Errors
    ///
    /// - When sending the transaction
    /// - When resolving the transaction in the database
    #[tracing::instrument(skip(self, signed_transaction), fields(
        account_id = %account_id,
        gas_cost_estimate = %gas_cost_estimate,
        spend_within_transaction = %spend_within_transaction,
        transaction_hash = tracing::field::Empty
    ))]
    pub async fn send_and_resolve_transaction(
        &self,
        account_id: AccountId,
        gas_cost_estimate: NearToken,
        spend_within_transaction: NearToken,
        signed_transaction: SignedTransaction,
        wait_until: TxExecutionStatus,
    ) -> Result<
        impl Future<Output = Result<FinalExecutionOutcomeView, ResolveTransactionError>>,
        SendTransactionError,
    > {
        let transaction_hash = signed_transaction.get_hash();
        tracing::Span::current().record(
            "transaction_hash",
            tracing::field::display(&transaction_hash),
        );
        tracing::info!("Sending and resolving transaction");

        self.database
            .set_pending_transaction(
                &account_id,
                gas_cost_estimate,
                spend_within_transaction,
                transaction_hash,
            )
            .await?;

        let result = self
            .relay_near
            .send_transaction(signed_transaction, wait_until)
            .await;

        let result = match result {
            Ok(result) => result,
            Err(e) => {
                // Some sort of RPC error: remove the pending transaction record.
                self.database
                    .remove_pending_transaction(&account_id)
                    .await?;
                return Err(e.into());
            }
        };

        let near = self.relay_near.clone();
        let database = self.database.clone();

        Ok(async move {
            let status = if let Some(outcome) = result.final_execution_outcome {
                outcome.into_outcome()
            } else {
                near.fetch_transaction_status(account_id.clone(), transaction_hash)
                    .await?
            };

            database.record_transaction(&account_id, &status).await?;

            Ok(status)
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SendTransactionError {
    #[error("RPC error: {0}")]
    Rpc(#[from] JsonRpcError<RpcTransactionError>),
    #[error("Set pending transaction error: {0}")]
    SetPendingTransaction(#[from] SetPendingTransactionError),
    #[error("Remove pending transaction error: {0}")]
    RemovePendingTransaction(#[from] sqlx::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveTransactionError {
    #[error("RPC error: {0}")]
    Rpc(#[from] JsonRpcError<RpcTransactionError>),
    #[error("Record transaction error: {0}")]
    RecordTransaction(#[from] RecordTransactionError),
}
