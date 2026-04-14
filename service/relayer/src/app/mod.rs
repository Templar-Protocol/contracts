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
    hash::CryptoHash,
    transaction::SignedTransaction,
    views::{FinalExecutionOutcomeView, TxExecutionStatus},
};
use near_sdk::{serde_json, AccountId, AccountIdRef, NearToken};
use templar_common::{
    asset::{BorrowAsset, CollateralAsset},
    market::DepositMsg,
    oracle::{pyth, redstone},
};
use templar_proxy_oracle_kernel::request::OracleRequest;
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
        near::{Near, ViewError, STORAGE_DEPOSIT_GAS},
        oracle,
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
    pub pyth: oracle::Handle<oracle::PythSpec>,
    pub redstone: oracle::Handle<oracle::RedStoneSpec>,
    pub cache: Arc<Cache>,
    pub database: Database,
}

impl App {
    pub fn new(
        args: args::Configuration,
        kill: watch::Sender<()>,
    ) -> Result<Self, templar_redstone_bridge::BridgeError> {
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

        let pyth = oracle::PythSpec::handle(
            args.pyth.clone(),
            relay_near.clone(),
            cache.clone(),
            kill.clone(),
        );

        let redstone = oracle::RedStoneSpec::handle(
            args.redstone.clone(),
            relay_near.clone(),
            cache.clone(),
            kill.clone(),
        )?;

        tokio::spawn(broom::start(
            database.clone(),
            relay_near.clone(),
            args.broom_batch_size,
            Duration::from_secs(args.broom_interval_secs),
            kill,
        ));

        Ok(Self {
            args,
            accounts: Arc::new(RwLock::new(AccountData::default())),
            relay_near,
            ua_near,
            pyth,
            redstone,
            cache: Arc::new(cache),
            database,
        })
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
            tracing::debug!(%registry_id, "Loading from registry");
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
            tracing::debug!(%market, "Loading market");
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
                market_id = %market_accounts.account_id,
                borrow_asset = %market_accounts.borrow.asset,
                collateral_asset = %market_accounts.collateral.asset,
                oracle_id = %market_accounts.oracle_id,
                "Loaded market",
            );

            for (contract_id, allowed_methods) in [
                (
                    market_accounts.account_id.as_ref(),
                    self.args.relay.allowed_methods.as_slice(),
                ),
                (
                    market_accounts.borrow.asset.contract_id(),
                    &[market_accounts
                        .borrow
                        .asset
                        .transfer_call_method_name()
                        .to_string()],
                ),
                (
                    market_accounts.collateral.asset.contract_id(),
                    &[market_accounts
                        .collateral
                        .asset
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

    pub fn expand_market_related_contracts(
        accounts: &AccountData,
        interacted_contract_ids: &mut HashSet<AccountId>,
    ) {
        let additional_contract_ids = interacted_contract_ids
            .iter()
            .filter_map(|contract_id| accounts.market_data.get(contract_id))
            .flat_map(|market_data| {
                [
                    market_data.oracle_id.clone(),
                    market_data.borrow.asset.contract_id().to_owned(),
                    market_data.collateral.asset.contract_id().to_owned(),
                ]
            })
            .collect::<Vec<_>>();

        interacted_contract_ids.extend(additional_contract_ids);
    }

    pub fn resolve_market_ids(
        accounts: &AccountData,
        interacted_contract_ids: &HashSet<AccountId>,
    ) -> HashSet<AccountId> {
        interacted_contract_ids
            .iter()
            .filter(|contract_id| accounts.market_data.contains_key(*contract_id))
            .cloned()
            .collect()
    }

    fn derive_sda_interactions(
        accounts: &AccountData,
        receiver_id: &AccountId,
        additional_interactions: Vec<AccountId>,
    ) -> (HashSet<AccountId>, HashSet<AccountId>) {
        let interacted_contract_ids: HashSet<_> = std::iter::once(receiver_id.clone())
            .chain(additional_interactions)
            .collect();
        let market_ids = Self::resolve_market_ids(accounts, &interacted_contract_ids);
        (interacted_contract_ids, market_ids)
    }

    fn grouped_price_updates(
        accounts: &AccountData,
        market_ids: &HashSet<AccountId>,
    ) -> (
        HashMap<AccountId, HashSet<pyth::PriceIdentifier>>,
        HashMap<AccountId, HashSet<redstone::FeedId>>,
    ) {
        market_ids
            .iter()
            .filter_map(|market_id| accounts.market_data.get(market_id))
            .fold(
                (HashMap::new(), HashMap::new()),
                |(mut pyth, mut redstone), market_data| {
                    for request in market_data
                        .collateral
                        .update_oracle
                        .iter()
                        .chain(market_data.borrow.update_oracle.iter())
                    {
                        match request {
                            OracleRequest::Pyth(request) => {
                                pyth.entry(request.oracle_id.clone())
                                    .or_default()
                                    .insert(request.price_id);
                            }
                            OracleRequest::RedStone(request) => {
                                redstone
                                    .entry(request.oracle_id.clone())
                                    .or_default()
                                    .insert(request.price_id.clone());
                            }
                        }
                    }
                    (pyth, redstone)
                },
            )
    }

    async fn dispatch_grouped_price_updates<
        PythUpdate,
        PythFuture,
        RedstoneUpdate,
        RedstoneFuture,
    >(
        pyth_updates: HashMap<AccountId, HashSet<pyth::PriceIdentifier>>,
        redstone_updates: HashMap<AccountId, HashSet<redstone::FeedId>>,
        mut pyth_update: PythUpdate,
        mut redstone_update: RedstoneUpdate,
    ) -> Result<(), PriceUpdateError>
    where
        PythUpdate: FnMut(AccountId, Box<[pyth::PriceIdentifier]>) -> PythFuture,
        PythFuture: Future<Output = Result<Option<CryptoHash>, Arc<oracle::UpdateError>>>,
        RedstoneUpdate: FnMut(AccountId, Box<[redstone::FeedId]>) -> RedstoneFuture,
        RedstoneFuture: Future<Output = Result<Option<CryptoHash>, Arc<oracle::UpdateError>>>,
    {
        for (oracle_id, feed_ids) in pyth_updates {
            match pyth_update(oracle_id, feed_ids.into_iter().collect::<Vec<_>>().into()).await {
                Ok(Some(hash)) => {
                    tracing::debug!(%hash, "Oracle update transaction succeeded");
                }
                Ok(None) => {
                    tracing::debug!("No price updates needed");
                }
                Err(error) => {
                    tracing::error!(%error, "Oracle update failed");
                    return Err(PriceUpdateError::Oracle(error));
                }
            }
        }

        for (oracle_id, feed_ids) in redstone_updates {
            match redstone_update(oracle_id, feed_ids.into_iter().collect::<Vec<_>>().into()).await
            {
                Ok(Some(hash)) => {
                    tracing::debug!(%hash, "Oracle update transaction succeeded");
                }
                Ok(None) => {
                    tracing::debug!("No price updates needed");
                }
                Err(error) => {
                    tracing::error!(%error, "Oracle update failed");
                    return Err(PriceUpdateError::Oracle(error));
                }
            }
        }

        Ok(())
    }

    pub async fn update_market_prices(
        &self,
        market_ids: &HashSet<AccountId>,
    ) -> Result<(), PriceUpdateError> {
        let (pyth_updates, redstone_updates) = {
            let accounts = self.accounts.read().await;
            Self::grouped_price_updates(&accounts, market_ids)
        };

        let pyth = self.pyth.clone();
        let redstone = self.redstone.clone();

        Self::dispatch_grouped_price_updates(
            pyth_updates,
            redstone_updates,
            move |oracle_id, feed_ids| {
                let pyth = pyth.clone();
                async move { pyth.update(oracle_id, feed_ids).await }
            },
            move |oracle_id, feed_ids| {
                let redstone = redstone.clone();
                async move { redstone.update(oracle_id, feed_ids).await }
            },
        )
        .await
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
                if transfer.asset() == market_account_ids.borrow.asset {
                    if !msg.expects_borrow_asset() {
                        errors.push(FunctionCallRejectionReason::InvalidAssetForMsg {
                            index,
                            expected: market_account_ids.collateral.asset.to_string(),
                            received: transfer.asset::<BorrowAsset>().to_string(),
                        });
                    }
                } else if transfer.asset() == market_account_ids.collateral.asset {
                    if msg.expects_borrow_asset() {
                        errors.push(FunctionCallRejectionReason::InvalidAssetForMsg {
                            index,
                            expected: market_account_ids.borrow.asset.to_string(),
                            received: transfer.asset::<CollateralAsset>().to_string(),
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

    /// Check and calculate gas for a signed delegate action.
    ///
    /// # Errors
    ///
    /// - If the signature verification fails.
    /// - If the receiver ID is unknown.
    /// - If the action is not supported.
    /// - If the function name is not valid.
    /// - If the function arguments are invalid.
    /// - etc. See [`PayloadRejectionReason`] for more details.
    #[tracing::instrument(skip(self, signed_delegate_action), fields(
        sender_id = %signed_delegate_action.delegate_action.sender_id,
        receiver_id = %signed_delegate_action.delegate_action.receiver_id
    ))]
    pub async fn sda_check_and_calculate_gas(
        &self,
        signed_delegate_action: &SignedDelegateAction,
    ) -> Result<SdaCheckResult, PayloadRejectionReason> {
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

        let additional_interactions = self
            .actions_are_allowed(
                &accounts,
                receiver_id,
                &contract_data,
                calls.iter().map(Borrow::borrow),
            )
            .map_err(PayloadRejectionReason::FunctionCallRejection)?;

        let (interacted_contract_ids, market_ids) =
            Self::derive_sda_interactions(&accounts, receiver_id, additional_interactions);

        let gas_total = calls.iter().map(|call| call.gas.as_gas()).sum();

        Ok(SdaCheckResult {
            gas: near_sdk::Gas::from_gas(gas_total),
            contract_data,
            interacted_contract_ids,
            market_ids,
        })
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

    /// Perform a storage deposit top-up, charging the associated account
    /// accordingly with the amount of storage balance consumed.
    ///
    /// # Errors
    ///
    /// - If loading storage balance bounds from the contract fails.
    /// - If gas calculation fails.
    /// - If sending the transaction fails.
    /// - If resolving the final transaction status with the database fails.
    pub async fn storage_deposit_top_up(
        &self,
        contract_data: &ContractData,
        contract_id: AccountId,
        account_id: AccountId,
    ) -> Result<(), StorageDepositError> {
        let Some(storage_balance_bounds) = contract_data
            .storage_balance_bounds
            .as_ref()
            .filter(|b| !b.min.is_zero())
        else {
            return Ok(());
        };

        let storage_balance = self
            .relay_near
            .load_storage_balance_of(contract_id.clone(), &account_id)
            .await?;

        let available = storage_balance.map_or(NearToken::from_near(0), |s| s.available);

        let should_have_available = self
            .args
            .relay
            .storage_deposit_guarantee_minimum_available
            .max(storage_balance_bounds.min);

        let storage_deposit_amount = should_have_available.saturating_sub(available);

        if storage_deposit_amount.is_zero() {
            // No deposit necessary
            return Ok(());
        }

        let Some(cost_of_gas) = self
            .estimate_cost_of_gas(STORAGE_DEPOSIT_GAS)
            .await
            .map(|amount| amount.saturating_add(storage_deposit_amount))
        else {
            return Err(StorageDepositError::GasEstimationFailure);
        };

        let signed_transaction = self
            .relay_near
            .construct_storage_deposit_transaction(
                &self.cache,
                account_id.clone(),
                contract_id.clone(),
                storage_deposit_amount,
            )
            .await;

        let resolve_transaction = self
            .send_and_resolve_transaction(
                account_id,
                cost_of_gas,
                storage_deposit_amount,
                signed_transaction,
                TxExecutionStatus::Final,
            )
            .await?;

        // Resolve synchronously.
        resolve_transaction.await?;

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SdaCheckResult {
    pub gas: near_sdk::Gas,
    pub contract_data: ContractData,
    pub interacted_contract_ids: HashSet<AccountId>,
    pub market_ids: HashSet<AccountId>,
}

#[derive(Debug, thiserror::Error)]
pub enum StorageDepositError {
    #[error("RPC view error: {0}")]
    View(#[from] ViewError),
    #[error("Failed to estimate gas")]
    GasEstimationFailure,
    #[error("Error sending storage deposit: {0}")]
    Send(#[from] SendTransactionError),
    #[error("Error resolving storage deposit: {0}")]
    Resolve(#[from] ResolveTransactionError),
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

#[derive(Debug, thiserror::Error)]
pub enum PriceUpdateError {
    #[error("Oracle update failed: {0}")]
    Oracle(Arc<oracle::UpdateError>),
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use near_sdk::AccountId;
    use templar_common::{
        asset::FungibleAsset,
        oracle::{pyth::PriceIdentifier, redstone::FeedId},
    };
    use tokio::{sync::Notify, time::timeout};

    use super::*;
    use crate::{AccountData, AssetResolution, MarketData};

    fn account_id(value: &str) -> AccountId {
        value.parse().unwrap()
    }

    fn price_id(byte: u8) -> PriceIdentifier {
        PriceIdentifier([byte; 32])
    }

    #[test]
    fn resolve_market_ids_filters_non_markets() {
        let market_id = account_id("market.test.near");
        let mut accounts = AccountData::default();
        accounts.market_data.insert(
            market_id.clone(),
            MarketData {
                account_id: market_id.clone(),
                oracle_id: account_id("oracle.test.near"),
                price_oracle_configuration: templar_common::market::PriceOracleConfiguration {
                    account_id: account_id("oracle.test.near"),
                    collateral_asset_price_id: price_id(1),
                    collateral_asset_decimals: 24,
                    borrow_asset_price_id: price_id(2),
                    borrow_asset_decimals: 24,
                    price_maximum_age_s: 60,
                },
                collateral: AssetResolution {
                    asset: FungibleAsset::nep141(account_id("collateral.test.near")),
                    price_id: price_id(1),
                    update_oracle: HashSet::from_iter([OracleRequest::pyth(
                        account_id("oracle.test.near"),
                        price_id(1),
                    )]),
                },
                borrow: AssetResolution {
                    asset: FungibleAsset::nep141(account_id("borrow.test.near")),
                    price_id: price_id(2),
                    update_oracle: HashSet::from_iter([OracleRequest::redstone(
                        account_id("oracle.test.near"),
                        FeedId::from("BTC"),
                    )]),
                },
            },
        );

        let interacted_contract_ids = HashSet::from([
            market_id.clone(),
            account_id("borrow.test.near"),
            account_id("something-else.test.near"),
        ]);

        assert_eq!(
            App::resolve_market_ids(&accounts, &interacted_contract_ids),
            HashSet::from([market_id])
        );
    }

    #[test]
    fn grouped_price_updates_combines_market_requests() {
        let market_a = account_id("market-a.test.near");
        let market_b = account_id("market-b.test.near");
        let pyth_oracle = account_id("pyth.test.near");
        let redstone_oracle = account_id("redstone.test.near");

        let mut accounts = AccountData::default();
        accounts.market_data.insert(
            market_a.clone(),
            MarketData {
                account_id: market_a.clone(),
                oracle_id: pyth_oracle.clone(),
                price_oracle_configuration: templar_common::market::PriceOracleConfiguration {
                    account_id: pyth_oracle.clone(),
                    collateral_asset_price_id: price_id(1),
                    collateral_asset_decimals: 24,
                    borrow_asset_price_id: price_id(2),
                    borrow_asset_decimals: 24,
                    price_maximum_age_s: 60,
                },
                collateral: AssetResolution {
                    asset: FungibleAsset::nep141(account_id("collateral-a.test.near")),
                    price_id: price_id(1),
                    update_oracle: HashSet::from_iter([OracleRequest::pyth(
                        pyth_oracle.clone(),
                        price_id(11),
                    )]),
                },
                borrow: AssetResolution {
                    asset: FungibleAsset::nep141(account_id("borrow-a.test.near")),
                    price_id: price_id(2),
                    update_oracle: HashSet::from_iter([OracleRequest::pyth(
                        pyth_oracle.clone(),
                        price_id(12),
                    )]),
                },
            },
        );
        accounts.market_data.insert(
            market_b.clone(),
            MarketData {
                account_id: market_b.clone(),
                oracle_id: redstone_oracle.clone(),
                price_oracle_configuration: templar_common::market::PriceOracleConfiguration {
                    account_id: redstone_oracle.clone(),
                    collateral_asset_price_id: price_id(3),
                    collateral_asset_decimals: 24,
                    borrow_asset_price_id: price_id(4),
                    borrow_asset_decimals: 24,
                    price_maximum_age_s: 60,
                },
                collateral: AssetResolution {
                    asset: FungibleAsset::nep141(account_id("collateral-b.test.near")),
                    price_id: price_id(3),
                    update_oracle: HashSet::from_iter([OracleRequest::redstone(
                        redstone_oracle.clone(),
                        FeedId::from("ETH"),
                    )]),
                },
                borrow: AssetResolution {
                    asset: FungibleAsset::nep141(account_id("borrow-b.test.near")),
                    price_id: price_id(4),
                    update_oracle: HashSet::from_iter([OracleRequest::redstone(
                        redstone_oracle.clone(),
                        FeedId::from("BTC"),
                    )]),
                },
            },
        );

        let (pyth_updates, redstone_updates) =
            App::grouped_price_updates(&accounts, &HashSet::from([market_a, market_b]));

        assert_eq!(pyth_updates.len(), 1);
        assert_eq!(
            pyth_updates[&pyth_oracle],
            HashSet::from([price_id(11), price_id(12)])
        );
        assert_eq!(redstone_updates.len(), 1);
        assert_eq!(
            redstone_updates[&redstone_oracle],
            HashSet::from([FeedId::from("ETH"), FeedId::from("BTC")])
        );
    }

    #[test]
    fn derive_sda_interactions_keeps_only_contracts_the_sda_touches() {
        let market_id = account_id("market.test.near");
        let oracle_id = account_id("oracle.test.near");
        let borrow_asset_id = account_id("borrow.test.near");
        let collateral_asset_id = account_id("collateral.test.near");
        let mut accounts = AccountData::default();
        accounts.market_data.insert(
            market_id.clone(),
            MarketData {
                account_id: market_id.clone(),
                oracle_id: oracle_id.clone(),
                price_oracle_configuration: templar_common::market::PriceOracleConfiguration {
                    account_id: oracle_id,
                    collateral_asset_price_id: price_id(1),
                    collateral_asset_decimals: 24,
                    borrow_asset_price_id: price_id(2),
                    borrow_asset_decimals: 24,
                    price_maximum_age_s: 60,
                },
                collateral: AssetResolution {
                    asset: FungibleAsset::nep141(collateral_asset_id.clone()),
                    price_id: price_id(1),
                    update_oracle: HashSet::new(),
                },
                borrow: AssetResolution {
                    asset: FungibleAsset::nep141(borrow_asset_id),
                    price_id: price_id(2),
                    update_oracle: HashSet::new(),
                },
            },
        );

        let (interacted_contract_ids, market_ids) =
            App::derive_sda_interactions(&accounts, &market_id, vec![]);

        assert_eq!(interacted_contract_ids, HashSet::from([market_id.clone()]));
        assert_eq!(market_ids, HashSet::from([market_id]));
    }

    #[tokio::test]
    async fn dispatch_grouped_price_updates_waits_for_pyth_before_redstone() {
        let pyth_updates =
            HashMap::from([(account_id("pyth.test.near"), HashSet::from([price_id(1)]))]);
        let redstone_updates = HashMap::from([(
            account_id("redstone.test.near"),
            HashSet::from([FeedId::from("BTC")]),
        )]);

        let pyth_started = Arc::new(Notify::new());
        let pyth_release = Arc::new(Notify::new());
        let redstone_started = Arc::new(Notify::new());

        let task = tokio::spawn({
            let pyth_started = pyth_started.clone();
            let pyth_release = pyth_release.clone();
            let redstone_started = redstone_started.clone();

            App::dispatch_grouped_price_updates(
                pyth_updates,
                redstone_updates,
                move |_, _| {
                    let pyth_started = pyth_started.clone();
                    let pyth_release = pyth_release.clone();
                    async move {
                        pyth_started.notify_one();
                        pyth_release.notified().await;
                        Ok(None)
                    }
                },
                move |_, _| {
                    let redstone_started = redstone_started.clone();
                    async move {
                        redstone_started.notify_one();
                        Ok(None)
                    }
                },
            )
        });

        timeout(Duration::from_secs(1), pyth_started.notified())
            .await
            .unwrap();
        let redstone_started_while_pyth_blocked =
            timeout(Duration::from_millis(100), redstone_started.notified())
                .await
                .is_ok();

        pyth_release.notify_one();

        task.await.unwrap().unwrap();

        assert!(
            !redstone_started_while_pyth_blocked,
            "redstone update started before blocked pyth update completed"
        );
    }
}
