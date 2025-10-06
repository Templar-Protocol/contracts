use std::{
    collections::{hash_map::Entry, HashMap},
    future::Future,
    sync::Arc,
    time::Duration,
};

use near_crypto::InMemorySigner;
use near_jsonrpc_client::{errors::JsonRpcError, methods::tx::RpcTransactionError};
use near_primitives::{
    action::{delegate::SignedDelegateAction, Action},
    transaction::SignedTransaction,
    views::{FinalExecutionOutcomeView, TxExecutionStatus},
};
use near_sdk::{serde_json, AccountId, NearToken};
use templar_common::market::DepositMsg;
use tokio::{
    sync::{watch, RwLock},
    task::JoinSet,
};
use tracing::{error, info, warn};

use crate::{
    broom,
    cache::Cache,
    client::{
        database::{
            error::{RecordTransactionError, SetPendingTransactionError},
            Database,
        },
        near::Near,
    },
    error::PreconditionError,
    AccountData, AssetTransfer, ContractData,
};

mod args;
pub use args::Configuration;

#[derive(Debug, Clone)]
pub struct App {
    pub args: args::Configuration,
    pub accounts: Arc<RwLock<AccountData>>,
    pub relay_near: Near,
    pub ua_near: Near,
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

        let cache = Cache::new(
            relay_near.clone(),
            Duration::from_secs(args.cache_gas_price_secs),
            Duration::from_secs(args.cache_nonce_secs),
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
            cache: Arc::new(cache),
            database,
        }
    }

    pub async fn estimate_cost_of_gas(&self, gas: u64) -> Option<NearToken> {
        const TERA: u128 = near_sdk::Gas::from_tgas(1).as_gas() as u128;

        let price_per_tgas = self.cache.gas_price().await;
        price_per_tgas
            .checked_mul(u128::from(gas))?
            .checked_div(TERA)
    }

    pub async fn load_markets(&mut self) {
        let mut markets = self.args.monitor.market.clone();

        // Load markets from registry...
        let mut set = JoinSet::new();
        for registry_id in &self.args.monitor.registry {
            set.spawn({
                let near = self.relay_near.clone();
                let registry_id = registry_id.clone();
                async move {
                    match near
                        .load_deployments_from_registry(registry_id.clone())
                        .await
                    {
                        Ok(deployments) => deployments,
                        Err(e) => {
                            warn!("Failed to load deployments from registry {registry_id}: {e}");
                            vec![]
                        }
                    }
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
                            warn!("Failed to load accounts for market {market}: {e}");
                            None
                        }
                    }
                }
            });
        }
        let market_accounts_vec = set.join_all().await;

        let mut markets = HashMap::new();
        let mut allowed_contracts = HashMap::new();

        for market_accounts in market_accounts_vec.into_iter().flatten() {
            let market_id = market_accounts.account_id.clone();

            info!(
                "Loaded market {market_id} with borrow asset {} and collateral asset {}",
                market_accounts.borrow_asset, market_accounts.collateral_asset,
            );

            for contract_id in [
                market_id,
                market_accounts.borrow_asset.contract_id().to_owned(),
                market_accounts.collateral_asset.contract_id().to_owned(),
            ] {
                if let Entry::Vacant(e) = allowed_contracts.entry(contract_id.clone()) {
                    let storage_balance_bounds = self
                        .relay_near
                        .load_storage_balance_bounds(contract_id.clone())
                        .await
                        .ok();

                    info!(
                        "Loaded storage balance bounds for contract {}: {}",
                        contract_id,
                        storage_balance_bounds
                            .as_ref()
                            .map_or(NearToken::from_near(0), |bounds| bounds.min),
                    );

                    e.insert(ContractData {
                        storage_balance_bounds,
                    });
                }
            }

            markets.insert(market_accounts.account_id.clone(), market_accounts);
        }

        let mut handle = self.accounts.write().await;
        handle.market_data = markets;
        handle.allowed_contract_data = allowed_contracts;
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
    pub async fn check_and_calculate_gas(
        &self,
        signed_delegate_action: &SignedDelegateAction,
    ) -> Result<(u64, ContractData), PreconditionError> {
        if !signed_delegate_action.verify() {
            return Err(PreconditionError::SignatureVerificationFailure);
        }

        let receiver_id = &signed_delegate_action.delegate_action.receiver_id;
        let accounts = self.accounts.read().await;

        let Some(contract_data) = accounts.allowed_contract_data.get(receiver_id).cloned() else {
            return Err(PreconditionError::UnknownTransactionReceiverId {
                account_id: receiver_id.clone(),
            });
        };

        let actions = signed_delegate_action.delegate_action.get_actions();
        let len = actions.len();
        let calls = actions
            .into_iter()
            .try_fold(Vec::with_capacity(len), |mut v, action| {
                if let Action::FunctionCall(fc) = action {
                    v.push(fc);
                    Ok(v)
                } else {
                    Err((v.len(), action))
                }
            })
            .map_err(|(index, action)| PreconditionError::UnsupportedAction { index, action })?;

        if accounts.market_data.contains_key(receiver_id) {
            // Calling a market contract directly.
            for (index, call) in calls.iter().enumerate() {
                if !self.args.relay.allowed_methods.contains(&call.method_name) {
                    return Err(PreconditionError::UnknownFunctionName {
                        name: call.method_name.clone(),
                        index,
                    });
                }
            }
        } else {
            // Token contract transfer call to market.
            for (index, call) in calls.iter().enumerate() {
                let transfer = AssetTransfer::parse(call, index, receiver_id.clone())?;
                let market_id = transfer.token_receiver_id();

                let Some(market_account_ids) = accounts.market_data.get(market_id) else {
                    return Err(PreconditionError::UnknownTransferReceiverId {
                        account_id: market_id.to_owned(),
                        index,
                    });
                };

                let msg = transfer.args.msg();
                let Ok(msg) = serde_json::from_str::<DepositMsg>(msg) else {
                    return Err(PreconditionError::MsgDeserializationFailure {
                        index,
                        msg: msg.to_string(),
                    });
                };

                #[allow(clippy::unwrap_used, reason = "DepositMsg serialization is infallible")]
                if transfer.asset() == market_account_ids.borrow_asset {
                    if !matches!(msg, DepositMsg::Supply | DepositMsg::Repay) {
                        return Err(PreconditionError::InvalidMsgForAsset {
                            index,
                            expected: "\"Supply\" or \"Repay\"".to_string(),
                            actual: serde_json::to_string(&msg).unwrap(),
                        });
                    }
                } else if transfer.asset() == market_account_ids.collateral_asset {
                    if !matches!(msg, DepositMsg::Collateralize) {
                        return Err(PreconditionError::InvalidMsgForAsset {
                            index,
                            expected: "\"Collateralize\"".to_string(),
                            actual: serde_json::to_string(&msg).unwrap(),
                        });
                    }
                } else {
                    return Err(PreconditionError::UnknownTransactionReceiverId {
                        account_id: receiver_id.clone(),
                    });
                }
            }
        }

        let gas_total = calls.iter().map(|call| call.gas).sum();

        Ok((gas_total, contract_data))
    }

    /// # Errors
    ///
    /// - When sending the transaction
    /// - When resolving the transaction in the database
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
            .await?;

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
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveTransactionError {
    #[error("RPC error: {0}")]
    Rpc(#[from] JsonRpcError<RpcTransactionError>),
    #[error("Record transaction error: {0}")]
    RecordTransaction(#[from] RecordTransactionError),
}
