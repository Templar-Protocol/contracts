use std::{
    borrow::Borrow,
    collections::{hash_map::Entry, HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use near_primitives::action::{delegate::SignedDelegateAction, Action, FunctionCallAction};
use near_sdk::{serde_json, AccountId, AccountIdRef, Gas, NearToken};
use templar_common::{
    asset::{BorrowAsset, CollateralAsset},
    market::DepositMsg,
    oracle::pyth,
};
use templar_gateway_client::{collect_paginated, Client as GatewayClient, NetworkConfigBuilder};
use templar_gateway_core::{
    FinalityPolicy, GatewayContext, GatewayError, GatewayResult, PlanWrite,
};
use templar_gateway_methods_dispatch::Dispatch;
use templar_gateway_methods_spec::{chain, contract, market, registry, storage};
use templar_gateway_oracle_updates_dispatch::{
    build_oracle_updates_context, Dispatch as OracleUpdatesDispatch, OracleUpdatesContext,
};
use templar_gateway_oracle_updates_spec::oracle::UpdatePrices;
use templar_gateway_types::{
    common::{Pagination, WriteOperationResult, WriteRequest},
    ContractKind, CryptoHash, IdempotencyKey, MethodSpec, OperationStatus,
};
use templar_proxy_oracle_near_common::cache::CachedProxyPriceStatus;
use tokio::{
    sync::{watch, RwLock},
    task::JoinSet,
};
use uuid::Uuid;

use crate::{
    broom,
    client::{
        database::{error::LockError, AccountedStatus, Database},
        oracle,
    },
    error::{FunctionCallRejectionReason, PayloadRejectionReason},
    AccountData, AssetTransfer, ContractData,
};

pub mod args;
pub use args::Configuration;

/// Gas attached to a `storage_deposit` call (matches the gateway's
/// `storage.deposit` plan); used to size the allowance lock estimate.
const STORAGE_DEPOSIT_GAS: Gas = Gas::from_tgas(100);

/// The oracle-updates flavor of the gateway client: bound to the oracle-updates
/// dispatcher and the layered payload-source context, sharing the methods
/// client's operation driver.
type OracleUpdatesClient = GatewayClient<OracleUpdatesDispatch, OracleUpdatesContext>;

#[derive(Debug, Clone)]
pub struct App {
    pub args: args::Configuration,
    pub accounts: Arc<RwLock<AccountData>>,
    /// In-process gateway client — the relayer's sole chain interface. Reads
    /// (market/oracle/storage/chain) and writes (meta-tx relay, UA execute/
    /// create, storage deposit) all go through its typed specs.
    pub gateway: GatewayClient,
    /// Oracle-updates client sharing the methods `gateway`'s operation driver
    /// (store, signer pool, idempotency, recovery). Drives `oracle.updatePrices`,
    /// which resolves each market oracle's dependencies and pulls the underlying
    /// Pyth/RedStone/Lazer payloads in-process.
    pub oracle_updates: OracleUpdatesClient,
    pub database: Database,
}

impl App {
    pub async fn new(args: args::Configuration, kill: watch::Sender<()>) -> anyhow::Result<Self> {
        Self::new_with_finality_policy(args, kill, FinalityPolicy::default()).await
    }

    /// Construct the relayer with an explicit gateway finality policy.
    ///
    /// Production startup uses [`FinalityPolicy::default`]; sandbox integration
    /// tests select optimistic execution and matching optimistic reads.
    pub async fn new_with_finality_policy(
        args: args::Configuration,
        kill: watch::Sender<()>,
        finality_policy: FinalityPolicy,
    ) -> anyhow::Result<Self> {
        Self::new_with_finality_policy_and_signers(args, kill, finality_policy, []).await
    }

    /// Construct the relayer with explicit gateway signer instances.
    ///
    /// This preserves near-api nonce caches when a caller performed optimistic
    /// setup transactions with those same signers before starting the relayer.
    pub async fn new_with_finality_policy_and_signers(
        args: args::Configuration,
        kill: watch::Sender<()>,
        finality_policy: FinalityPolicy,
        signers: impl IntoIterator<Item = (near_api::types::AccountId, Arc<near_api::Signer>)>,
    ) -> anyhow::Result<Self> {
        // Build through `NetworkConfigBuilder` so a FastNear-style `?apiKey=…`
        // embedded in `RPC_URL` is routed to the endpoint's auth header instead of
        // being left in the query string (which would produce malformed/401 RPC
        // requests). The relayer has no separate RPC API-key option.
        let network = NetworkConfigBuilder::from_url("relayer", args.rpc_url.parse()?).build();

        // Persist gateway operations in the relayer database so idempotency and
        // replay survive restarts.
        let gateway_store = templar_gateway_store::PostgresStore::new(&args.database_url)?;
        gateway_store.migrate().await?;

        // The relay account signs with a rotating multi-key pool; the UA account
        // is registered for the universal-account creation path.
        let mut builder = GatewayClient::builder(network)
            .finality_policy(finality_policy)
            .store(Arc::new(gateway_store));
        let signers: HashMap<_, _> = signers.into_iter().collect();
        if !signers.contains_key(&args.relay.account_id) && !args.relay.secret_key.is_empty() {
            builder = builder
                .secret_keys(args.relay.account_id.clone(), args.relay.secret_key.clone())
                .await?;
        }
        if !signers.contains_key(&args.ua.account_id) && !args.ua.secret_key.is_empty() {
            builder = builder
                .secret_keys(args.ua.account_id.clone(), args.ua.secret_key.clone())
                .await?;
        }
        for (account_id, signer) in signers {
            builder = builder.with_signer(account_id, signer);
        }
        // The methods client and the oracle-updates client share one operation
        // driver (store, signer pool, executor) and a base context, so idempotency,
        // replay, the startup-recovery sweep, and the NEAR read cache span both.
        // The oracle client layers the in-process Pyth/RedStone/Lazer payload
        // sources onto a clone of the base context.
        let (base_context, driver, signer_account_ids) = builder.build_parts()?;
        let gateway = GatewayClient::from_parts(
            base_context.clone(),
            driver.clone(),
            signer_account_ids.clone(),
        );
        let oracle_context =
            build_oracle_updates_context(base_context, args.oracle_sources.build()?)?;
        let oracle_updates =
            OracleUpdatesClient::from_parts(oracle_context, driver, signer_account_ids);

        let database = Database::new(&args.database_url, kill.clone())?;

        // Drive any operation a previous run left mid-flight to a terminal
        // outcome, once, before serving — so the broom (and live requests) only
        // ever settle terminal operations. Doing this while serving would race
        // the synchronous execute path on the same operation. Fail fast if it
        // can't: the container restarts us (retrying recovery) rather than
        // serving with charges that can never settle — unless the operator opted
        // into starting anyway for a persistently wedged recovery.
        if let Err(error) = gateway.resume_incomplete_operations().await {
            if args.ignore_startup_recovery_failure {
                tracing::warn!(
                    %error,
                    "Failed to resume incomplete gateway operations at startup; serving \
                     anyway (IGNORE_STARTUP_RECOVERY_FAILURE set). Charges left mid-flight \
                     will not settle until recovery next succeeds.",
                );
            } else {
                return Err(anyhow::Error::from(error).context(
                    "Failed to resume incomplete gateway operations at startup \
                     (set IGNORE_STARTUP_RECOVERY_FAILURE to start anyway)",
                ));
            }
        }

        tokio::spawn(broom::start(
            database.clone(),
            gateway.clone(),
            args.broom_batch_size,
            Duration::from_secs(args.broom_interval_secs),
            kill,
        ));

        Ok(Self {
            args,
            accounts: Arc::new(RwLock::new(AccountData::default())),
            gateway,
            oracle_updates,
            database,
        })
    }

    /// Estimate the NEAR cost of `gas` at the current gas price.
    ///
    /// This sizes the allowance lock before submitting; the lock is reconciled
    /// against the actual `tokens_burnt` afterwards, so a coarse estimate only
    /// affects the pre-flight affordability gate, never the amount charged.
    #[tracing::instrument(skip(self), fields(gas = %gas))]
    pub async fn estimate_cost_of_gas(&self, gas: Gas) -> Option<NearToken> {
        let gas_price = match self.gateway.read(chain::GetBlock::new()).await {
            Ok(result) => result.gas_price,
            Err(error) => {
                tracing::error!(%error, "Failed to fetch gas price");
                return None;
            }
        };

        // `gas_price` is yoctoNEAR per unit of gas.
        let result = gas_price.checked_mul(u128::from(gas.as_gas()));

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
                let gateway = self.gateway.clone();
                let registry_id = registry_id.clone();
                async move {
                    load_registry_deployments(&gateway, registry_id.clone())
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

        // ...and load each market's configuration to build the relay allowlist.
        // Configuration is read fresh (not cached) — it only seeds the static
        // method/storage allowlist and the set of known market accounts; the
        // per-request paths re-resolve assets and oracle dependencies on demand.
        let mut set = JoinSet::new();
        for market in markets {
            tracing::debug!(%market, "Loading market");
            set.spawn({
                let gateway = self.gateway.clone();
                async move {
                    match gateway
                        .read(market::GetConfiguration::new(market.clone()))
                        .await
                    {
                        Ok(configuration) => Some((market, configuration)),
                        Err(e) => {
                            tracing::warn!("Failed to load configuration for market {market}: {e}");
                            None
                        }
                    }
                }
            });
        }
        let market_configurations = set.join_all().await;

        let mut market_ids = HashSet::new();
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

        for (market_id, configuration) in market_configurations.into_iter().flatten() {
            let oracle_id = configuration.price_oracle_configuration.account_id.clone();
            tracing::info!(
                %market_id,
                borrow_asset = %configuration.borrow_asset,
                collateral_asset = %configuration.collateral_asset,
                %oracle_id,
                "Loaded market",
            );

            for (contract_id, allowed_methods) in [
                (
                    market_id.clone(),
                    self.args.relay.allowed_methods.as_slice(),
                ),
                (
                    configuration.borrow_asset.contract_id().to_owned(),
                    &[configuration
                        .borrow_asset
                        .transfer_call_method_name()
                        .to_string()][..],
                ),
                (
                    configuration.collateral_asset.contract_id().to_owned(),
                    &[configuration
                        .collateral_asset
                        .transfer_call_method_name()
                        .to_string()][..],
                ),
                (
                    oracle_id.clone(),
                    self.args.relay.oracle_allowed_methods.as_slice(),
                ),
            ] {
                match allowed_contracts.entry(contract_id.clone()) {
                    Entry::Vacant(e) => {
                        // `None` for contracts that don't implement storage
                        // management (or are momentarily unreachable); those are
                        // simply never storage-deposited.
                        let storage_balance_bounds = self
                            .gateway
                            .read(storage::GetBalanceBounds::new(contract_id.clone()))
                            .await
                            .ok()
                            .map(|result| result.bounds);

                        tracing::info!(
                            "Loaded storage balance bounds for contract {contract_id}: {}",
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

            market_ids.insert(market_id);
        }

        let mut handle = self.accounts.write().await;
        handle.market_ids = market_ids;
        handle.allowed_contract_data = allowed_contracts;
    }

    /// Expand a set of interacted contracts to also include each interacted
    /// market's oracle and asset contracts, resolved on demand. Used to widen
    /// the set of contracts eligible for storage deposits.
    pub async fn expand_market_related_contracts(
        &self,
        market_ids: &HashSet<AccountId>,
        interacted_contract_ids: &mut HashSet<AccountId>,
    ) {
        let mut additional = Vec::new();
        for market_id in interacted_contract_ids
            .iter()
            .filter(|id| market_ids.contains(*id))
        {
            match self
                .gateway
                .read(market::GetConfiguration::new(market_id.clone()))
                .await
            {
                Ok(configuration) => {
                    additional.push(configuration.price_oracle_configuration.account_id);
                    additional.push(configuration.borrow_asset.contract_id().to_owned());
                    additional.push(configuration.collateral_asset.contract_id().to_owned());
                }
                Err(e) => {
                    tracing::warn!(%market_id, "Failed to expand market contracts: {e}");
                }
            }
        }
        interacted_contract_ids.extend(additional);
    }

    pub fn resolve_market_ids(
        known_market_ids: &HashSet<AccountId>,
        interacted_contract_ids: &HashSet<AccountId>,
    ) -> HashSet<AccountId> {
        interacted_contract_ids
            .iter()
            .filter(|contract_id| known_market_ids.contains(*contract_id))
            .cloned()
            .collect()
    }

    fn derive_sda_interactions(
        known_market_ids: &HashSet<AccountId>,
        receiver_id: &AccountId,
        additional_interactions: Vec<AccountId>,
    ) -> (HashSet<AccountId>, HashSet<AccountId>) {
        let interacted_contract_ids: HashSet<_> = std::iter::once(receiver_id.clone())
            .chain(additional_interactions)
            .collect();
        let market_ids = Self::resolve_market_ids(known_market_ids, &interacted_contract_ids);
        (interacted_contract_ids, market_ids)
    }

    /// Refresh, on demand, every market oracle's prices before touching the given
    /// markets.
    ///
    /// For each market the configuration is read fresh through the gateway (cached
    /// server-side), then a single `oracle.updatePrices` resolves the oracle's
    /// Direct/Proxy/LST dependencies, pulls the underlying Pyth/RedStone/Lazer
    /// payloads in-process, and — for a proxy oracle — re-aggregates its cached
    /// prices, all as one store-backed operation. If a proxy price comes back
    /// non-`Accepted` we log loudly but still relay, so the user's transaction lands
    /// on-chain with a precise rejection rather than an opaque relayer error.
    pub async fn update_market_prices(
        &self,
        market_ids: &HashSet<AccountId>,
    ) -> Result<(), PriceUpdateError> {
        for market_id in market_ids {
            let configuration = self
                .gateway
                .read(market::GetConfiguration::new(market_id.clone()))
                .await
                .map_err(PriceUpdateError::Resolve)?;
            let oracle_cfg = configuration.price_oracle_configuration;
            let oracle_id = oracle_cfg.account_id;
            let price_ids = vec![
                oracle_cfg.collateral_asset_price_id,
                oracle_cfg.borrow_asset_price_id,
            ];

            let result = self
                .oracle_updates
                .execute_as(
                    self.args.relay.account_id.clone(),
                    UpdatePrices {
                        oracle_id: oracle_id.clone(),
                        price_ids: price_ids.clone(),
                    },
                )
                .await
                .map_err(oracle::UpdateError::from)
                .map_err(Arc::new)
                .map_err(PriceUpdateError::Oracle)?;

            if result.operation.status != OperationStatus::Succeeded {
                return Err(PriceUpdateError::Oracle(Arc::new(
                    oracle::UpdateError::NotSucceeded {
                        operation_id: result.operation.id.0,
                        status: result.operation.status,
                    },
                )));
            }

            // A proxy oracle's re-aggregation (the operation's final step) returns a
            // per-price status map and succeeds even when a price is
            // circuit-breaker-blocked or failed to resolve. We do NOT abort the relay
            // on a non-`Accepted` price: the user has a valid, already-paid-for
            // request, and blocking it here trades a precise on-chain rejection (with a
            // tx hash the user can reference) for an opaque transient relayer error.
            // Instead, log loudly and let the transaction land, where the market
            // contract surfaces the real reason. Direct/LST oracles have no such step.
            let kind = self
                .gateway
                .read(contract::GetKind {
                    contract_id: oracle_id.clone(),
                })
                .await
                .map_err(PriceUpdateError::Resolve)?;
            if matches!(kind.kind, ContractKind::ProxyOracle) {
                if let Err(error) = proxy_prices_all_accepted(
                    &price_ids,
                    result
                        .operation
                        .final_outcome()
                        .and_then(|outcome| outcome.return_value.as_deref()),
                ) {
                    tracing::warn!(
                        %market_id,
                        %oracle_id,
                        %error,
                        "Proxy oracle prices were not all accepted after update; relaying the \
                         user transaction anyway so it lands on-chain with a precise rejection \
                         instead of a transient relayer error"
                    );
                }
            }
        }

        Ok(())
    }

    /// Checks that the all of the function call actions are allowed for the specific receiver.
    ///
    /// Returns a list of other accounts that this action will probably interact with in addition to the receiver.
    ///
    /// # Errors
    ///
    /// - If the receiver is not known.
    /// - If any of the function call actions are not allowed.
    #[tracing::instrument(skip(self, known_market_ids, contract_data, calls))]
    pub async fn actions_are_allowed<'a>(
        &self,
        known_market_ids: &HashSet<AccountId>,
        receiver_id: &AccountIdRef,
        contract_data: &ContractData,
        calls: impl IntoIterator<Item = &'a FunctionCallAction>,
    ) -> Result<Vec<AccountId>, ActionAllowanceError> {
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

                if !known_market_ids.contains(market_id) {
                    errors.push(FunctionCallRejectionReason::UnknownTransferReceiverId {
                        account_id: market_id.to_owned(),
                        index,
                    });
                    continue;
                }

                // Resolve the market's assets on demand to validate the deposit.
                let configuration = self
                    .gateway
                    .read(market::GetConfiguration {
                        market_id: market_id.to_owned(),
                    })
                    .await
                    .map_err(|source| ActionAllowanceError::MarketConfigurationRead {
                        account_id: market_id.to_owned(),
                        source,
                    })?;

                let msg = transfer.args.msg();
                let Ok(msg) = serde_json::from_str::<DepositMsg>(msg) else {
                    errors.push(FunctionCallRejectionReason::MsgDeserializationFailure {
                        index,
                        msg: msg.to_string(),
                    });
                    continue;
                };

                if transfer.asset() == configuration.borrow_asset {
                    if !msg.expects_borrow_asset() {
                        errors.push(FunctionCallRejectionReason::InvalidAssetForMsg {
                            index,
                            expected: configuration.collateral_asset.to_string(),
                            received: transfer.asset::<BorrowAsset>().to_string(),
                        });
                    }
                } else if transfer.asset() == configuration.collateral_asset {
                    if msg.expects_borrow_asset() {
                        errors.push(FunctionCallRejectionReason::InvalidAssetForMsg {
                            index,
                            expected: configuration.borrow_asset.to_string(),
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
            Err(ActionAllowanceError::Rejected(errors))
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
    ) -> Result<SdaCheckResult, SdaCheckError> {
        tracing::debug!("Checking and calculating gas for delegate action");
        if !signed_delegate_action.verify() {
            return Err(SdaCheckError::Rejected(
                PayloadRejectionReason::SignatureVerificationFailure,
            ));
        }

        let receiver_id = &signed_delegate_action.delegate_action.receiver_id;
        let accounts = self.accounts.read().await;

        let Some(contract_data) = accounts.allowed_contract_data.get(receiver_id).cloned() else {
            return Err(SdaCheckError::Rejected(
                PayloadRejectionReason::UnknownTransactionReceiverId {
                    account_id: receiver_id.clone(),
                },
            ));
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
            .map_err(|index| {
                SdaCheckError::Rejected(PayloadRejectionReason::UnsupportedAction { index })
            })?;

        let additional_interactions = self
            .actions_are_allowed(
                &accounts.market_ids,
                receiver_id,
                &contract_data,
                calls.iter().map(Borrow::borrow),
            )
            .await
            .map_err(SdaCheckError::from)?;

        let (interacted_contract_ids, market_ids) = Self::derive_sda_interactions(
            &accounts.market_ids,
            receiver_id,
            additional_interactions,
        );

        let gas_total = calls.iter().map(|call| call.gas.as_gas()).sum();

        Ok(SdaCheckResult {
            gas: near_sdk::Gas::from_gas(gas_total),
            contract_data,
            interacted_contract_ids,
            market_ids,
        })
    }

    /// Submit a write through the gateway and reconcile the charged account's
    /// allowance against the operation's actual cost.
    ///
    /// The allowance is locked up front under a freshly generated gateway
    /// idempotency key — the on-chain hash isn't known until the gateway submits,
    /// so the key is the charge's identity. The gateway drives the operation to a
    /// terminal outcome and returns its record, from which the charge is settled
    /// directly (the recorded `tokens_burnt`, plus the locked deposit on success).
    ///
    /// On a gateway error this path reconciles against any operation the gateway
    /// persisted for the idempotency key. If the gateway failed before persisting
    /// an operation, the lock is released immediately. If the lookup fails, the
    /// broom remains the backstop for later reconciliation.
    ///
    /// `account_id` is the account charged for the operation; `signer_account_id`
    /// is the relayer-controlled account that signs and pays (relay or UA).
    ///
    /// # Errors
    ///
    /// - Locking or settling the allowance in the database
    /// - Gateway execution
    /// - The operation completing without producing a transaction
    #[tracing::instrument(skip(self, body), fields(
        account_id = %account_id,
        signer_account_id = %signer_account_id,
        gas_cost_estimate = %gas_cost_estimate,
        spend_within_transaction = %spend_within_transaction,
        transaction_hash = tracing::field::Empty,
    ))]
    pub async fn execute_and_account<S>(
        &self,
        account_id: AccountId,
        signer_account_id: AccountId,
        gas_cost_estimate: NearToken,
        spend_within_transaction: NearToken,
        body: S,
    ) -> Result<AccountedExecution, SubmitError>
    where
        S: MethodSpec<Output = WriteOperationResult> + Send,
        Dispatch: PlanWrite<S, GatewayContext>,
    {
        tracing::info!("Submitting and accounting for transaction");
        let operation_key = Uuid::new_v4();
        self.database
            .create_account(&account_id, self.args.relay.starting_allowance_yocto)
            .await?;

        self.database
            .lock_pending(
                &account_id,
                gas_cost_estimate,
                spend_within_transaction,
                operation_key,
            )
            .await?;

        let idempotency_key = IdempotencyKey(operation_key.to_string());
        let result = match self
            .gateway
            .execute_request(WriteRequest {
                signer_account_id: signer_account_id.into(),
                idempotency_key: Some(idempotency_key.clone()),
                body,
            })
            .await
        {
            Ok(result) => result,
            Err(error) => {
                // Resolve the charge from whatever the gateway persisted, using the
                // same rule the broom applies: a planning/presign failure leaves no
                // operation (nothing reached the chain) so the reservation is released
                // now rather than stranded until the broom's delayed sweep; a persisted
                // operation is settled or deferred by its status. If the lookup
                // itself fails we cannot tell, so leave the charge for the broom.
                if let Ok(operation) = self
                    .gateway
                    .operation_by_idempotency_key(&idempotency_key)
                    .await
                {
                    self.database
                        .resolve_charge(&account_id, operation_key, operation.as_ref())
                        .await?;
                }
                return Err(SubmitError::Gateway(error));
            }
        };

        let operation = result.operation;

        // Resolve the on-chain hash before charging, so the no-transaction error
        // path leaves the charge untouched for the broom to reconcile rather than
        // charging and then returning an error (a degenerate terminal operation
        // without a hash should never settle on the hot path).
        let Some(tx_hash) = operation.latest_tx_hash() else {
            return Err(SubmitError::NoTransaction {
                operation_id: operation.id.0,
            });
        };
        let native_tx_hash = from_gateway_hash(&tx_hash);
        tracing::Span::current().record("transaction_hash", tracing::field::display(&tx_hash));

        // Resolve the charge from the operation's recorded status. A terminal
        // operation settles now; a still in-flight one is left locked for the
        // broom, so its (not-yet-final) cost is never lost.
        let status = self
            .database
            .resolve_charge(&account_id, operation_key, Some(&operation))
            .await?;

        Ok(AccountedExecution {
            transaction_hash: native_tx_hash,
            status,
        })
    }

    /// Ensure `account_id` holds the relayer's guaranteed-minimum storage
    /// balance on `contract_id`, charging the account for any deposit made.
    ///
    /// Contracts without storage management (`storage_balance_bounds` absent or
    /// zero) are skipped.
    ///
    /// # Errors
    ///
    /// - Reading the account's storage balance through the gateway
    /// - Estimating gas
    /// - Submitting or accounting for the deposit
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
            .gateway
            .read(storage::GetBalanceOf {
                contract_id: contract_id.clone(),
                account_id: account_id.clone(),
            })
            .await?
            .balance;

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

        let Some(cost_of_gas) = self.estimate_cost_of_gas(STORAGE_DEPOSIT_GAS).await else {
            return Err(StorageDepositError::GasEstimationFailure);
        };

        let execution = self
            .execute_and_account(
                account_id.clone(),
                self.args.relay.account_id.clone(),
                cost_of_gas,
                storage_deposit_amount,
                storage::Deposit {
                    contract_id,
                    beneficiary_id: Some(account_id),
                    registration_only: false,
                    deposit: storage_deposit_amount,
                },
            )
            .await?;

        // Only a confirmed deposit registers storage; the caller (e.g. relay) must
        // not proceed as if storage is in place otherwise. A reverted deposit
        // failed outright; a still-pending one isn't confirmed yet.
        match execution.status {
            AccountedStatus::Succeeded => Ok(()),
            AccountedStatus::Failed => Err(StorageDepositError::Reverted {
                transaction_hash: execution.transaction_hash,
            }),
            AccountedStatus::Pending => Err(StorageDepositError::NotFinalized {
                transaction_hash: execution.transaction_hash,
            }),
        }
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
pub enum ActionAllowanceError {
    #[error("Function call rejected")]
    Rejected(Vec<FunctionCallRejectionReason>),
    #[error("Failed to load configuration for known market {account_id}: {source}")]
    MarketConfigurationRead {
        account_id: AccountId,
        source: GatewayError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum SdaCheckError {
    #[error(transparent)]
    Rejected(PayloadRejectionReason),
    #[error("Failed to load configuration for known market {account_id}: {source}")]
    MarketConfigurationRead {
        account_id: AccountId,
        source: GatewayError,
    },
}

impl From<ActionAllowanceError> for SdaCheckError {
    fn from(error: ActionAllowanceError) -> Self {
        match error {
            ActionAllowanceError::Rejected(reasons) => {
                Self::Rejected(PayloadRejectionReason::FunctionCallRejection(reasons))
            }
            ActionAllowanceError::MarketConfigurationRead { account_id, source } => {
                Self::MarketConfigurationRead { account_id, source }
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StorageDepositError {
    #[error("Gateway read error: {0}")]
    Gateway(#[from] GatewayError),
    #[error("Failed to estimate gas")]
    GasEstimationFailure,
    #[error("Error submitting storage deposit: {0}")]
    Submit(#[from] SubmitError),
    #[error("Storage deposit reverted on chain (transaction {transaction_hash})")]
    Reverted {
        transaction_hash: near_primitives::hash::CryptoHash,
    },
    #[error("Storage deposit not yet finalized (transaction {transaction_hash})")]
    NotFinalized {
        transaction_hash: near_primitives::hash::CryptoHash,
    },
}

/// The result of [`App::execute_and_account`]: the on-chain transaction hash and
/// the operation's disposition. `status` lets callers distinguish a terminal
/// success from an on-chain failure and from a still-in-flight operation (whose
/// charge is deferred to the broom) — e.g. UA create must not claim success for a
/// reverted deploy, and storage deposit must not proceed unless it is confirmed.
#[derive(Debug, Clone, Copy)]
pub struct AccountedExecution {
    pub transaction_hash: near_primitives::hash::CryptoHash,
    pub status: AccountedStatus,
}

/// Failure submitting a gateway write and settling its allowance charge.
#[derive(Debug, thiserror::Error)]
pub enum SubmitError {
    #[error("Lock allowance error: {0}")]
    Lock(#[from] LockError),
    #[error("Gateway execution error: {0}")]
    Gateway(GatewayError),
    #[error("Operation {operation_id} completed without a transaction")]
    NoTransaction { operation_id: String },
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum PriceUpdateError {
    #[error("Oracle update failed: {0}")]
    Oracle(Arc<oracle::UpdateError>),
    #[error("Failed to resolve price-update dependencies: {0}")]
    Resolve(GatewayError),
}

/// Convert a native transaction/block hash into the gateway's hash type. Both
/// are 32-byte hashes, so this is a total byte-for-byte move.
#[must_use]
pub fn to_gateway_hash(hash: &near_primitives::hash::CryptoHash) -> CryptoHash {
    CryptoHash::from(near_api::CryptoHash(hash.0))
}

/// Convert a gateway hash into the native transaction-hash type. Both are
/// 32-byte hashes, so this is a total byte-for-byte move.
#[must_use]
pub fn from_gateway_hash(hash: &CryptoHash) -> near_primitives::hash::CryptoHash {
    near_primitives::hash::CryptoHash(hash.0 .0)
}

/// Page size for draining a registry's deployment list through the gateway.
#[allow(
    clippy::unwrap_used,
    reason = "compile-time const; a zero literal would fail to compile"
)]
const REGISTRY_PAGE_SIZE: std::num::NonZeroU32 = std::num::NonZeroU32::new(100).unwrap();

/// Check a proxy oracle's re-aggregation result: every requested price should come
/// back `Accepted`. A missing return value, a missing entry, or any non-`Accepted`
/// status is unavailable — checking only the returned entries would let an omitted
/// price slip by. `return_value` is the operation's final-step return value (the
/// proxy `update_prices` status map). The `Err` describes what was unavailable; the
/// caller logs it rather than blocking the relay.
fn proxy_prices_all_accepted(
    requested: &[pyth::PriceIdentifier],
    return_value: Option<&[u8]>,
) -> Result<(), oracle::UpdateError> {
    let Some(return_value) = return_value else {
        return Err(oracle::UpdateError::ProxyPricesUnavailable {
            price_ids: requested.to_vec(),
        });
    };
    let statuses: HashMap<pyth::PriceIdentifier, CachedProxyPriceStatus> =
        serde_json::from_slice(return_value).map_err(oracle::UpdateError::ProxyResultDecode)?;
    let unavailable: Vec<pyth::PriceIdentifier> = requested
        .iter()
        .copied()
        .filter(|id| {
            !matches!(
                statuses.get(id),
                Some(CachedProxyPriceStatus::Accepted { .. })
            )
        })
        .collect();
    if unavailable.is_empty() {
        Ok(())
    } else {
        Err(oracle::UpdateError::ProxyPricesUnavailable {
            price_ids: unavailable,
        })
    }
}

/// Drain every deployment account from a registry via the gateway.
async fn load_registry_deployments(
    gateway: &GatewayClient,
    registry_id: AccountId,
) -> GatewayResult<Vec<AccountId>> {
    collect_paginated(REGISTRY_PAGE_SIZE, move |offset, count| {
        let gateway = gateway.clone();
        let registry_id = registry_id.clone();
        async move {
            let result = gateway
                .read(registry::ListDeployments {
                    registry_id,
                    args: Pagination {
                        offset: Some(offset),
                        limit: Some(count),
                    },
                })
                .await?;
            Ok(result.account_ids)
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use near_sdk::AccountId;
    use templar_common::{oracle::pyth::PriceIdentifier, Nanoseconds};
    use templar_proxy_oracle_kernel::Price;

    use super::*;

    fn account_id(value: &str) -> AccountId {
        value.parse().unwrap()
    }

    fn price_id(byte: u8) -> PriceIdentifier {
        PriceIdentifier([byte; 32])
    }

    fn accepted() -> CachedProxyPriceStatus {
        CachedProxyPriceStatus::Accepted {
            price: Price {
                price: 1,
                conf: 0,
                expo: 0,
                publish_time_ns: Nanoseconds::zero(),
            },
        }
    }

    fn status_map_bytes(entries: &[(PriceIdentifier, CachedProxyPriceStatus)]) -> Vec<u8> {
        let map: HashMap<PriceIdentifier, CachedProxyPriceStatus> =
            entries.iter().cloned().collect();
        serde_json::to_vec(&map).unwrap()
    }

    #[test]
    fn resolve_market_ids_filters_non_markets() {
        let market_id = account_id("market.test.near");
        let accounts = AccountData {
            market_ids: HashSet::from([market_id.clone()]),
            ..Default::default()
        };

        let interacted_contract_ids = HashSet::from([
            market_id.clone(),
            account_id("borrow.test.near"),
            account_id("something-else.test.near"),
        ]);

        assert_eq!(
            App::resolve_market_ids(&accounts.market_ids, &interacted_contract_ids),
            HashSet::from([market_id])
        );
    }

    #[test]
    fn derive_sda_interactions_keeps_only_contracts_the_sda_touches() {
        let market_id = account_id("market.test.near");
        let accounts = AccountData {
            market_ids: HashSet::from([market_id.clone()]),
            ..Default::default()
        };

        let (interacted_contract_ids, market_ids) =
            App::derive_sda_interactions(&accounts.market_ids, &market_id, vec![]);

        assert_eq!(interacted_contract_ids, HashSet::from([market_id.clone()]));
        assert_eq!(market_ids, HashSet::from([market_id]));
    }

    #[test]
    fn proxy_guard_passes_when_every_requested_price_is_accepted() {
        let ids = [price_id(1), price_id(2)];
        let bytes = status_map_bytes(&[(price_id(1), accepted()), (price_id(2), accepted())]);

        proxy_prices_all_accepted(&ids, Some(&bytes)).expect("all-accepted result must pass");
    }

    #[test]
    fn proxy_guard_missing_return_value_marks_all_unavailable() {
        let ids = [price_id(1), price_id(2)];

        let error = proxy_prices_all_accepted(&ids, None)
            .expect_err("a missing return value must be treated as unavailable");

        assert!(matches!(
            error,
            oracle::UpdateError::ProxyPricesUnavailable { price_ids } if price_ids == ids.to_vec()
        ));
    }

    #[test]
    fn proxy_guard_flags_omitted_and_non_accepted_prices() {
        let ids = [price_id(1), price_id(2)];
        // `price_id(1)` is omitted entirely; `price_id(2)` is present but not accepted.
        let bytes = status_map_bytes(&[(
            price_id(2),
            CachedProxyPriceStatus::ResolveFailed {
                message: "boom".to_owned(),
            },
        )]);

        let error = proxy_prices_all_accepted(&ids, Some(&bytes))
            .expect_err("an omitted or non-accepted price must be unavailable");

        assert!(matches!(
            error,
            oracle::UpdateError::ProxyPricesUnavailable { price_ids }
                if price_ids == vec![price_id(1), price_id(2)]
        ));
    }

    #[test]
    fn proxy_guard_malformed_result_is_a_decode_error() {
        let ids = [price_id(1)];

        let error = proxy_prices_all_accepted(&ids, Some(b"not json"))
            .expect_err("an undecodable status map must surface as a decode error");

        assert!(matches!(error, oracle::UpdateError::ProxyResultDecode(_)));
    }
}
