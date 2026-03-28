use std::{
    collections::{HashMap, HashSet},
    sync::{atomic::AtomicUsize, Arc},
    time::{SystemTime, UNIX_EPOCH},
};

use near_crypto::{PublicKey, Signer};
use near_jsonrpc_client::{
    errors::{JsonRpcError, JsonRpcServerError},
    methods::{
        self,
        block::RpcBlockError,
        gas_price::RpcGasPriceError,
        query::RpcQueryError,
        tx::{RpcTransactionError, RpcTransactionResponse},
    },
    JsonRpcClient,
};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::{
    action::{delegate::SignedDelegateAction, FunctionCallAction},
    hash::CryptoHash,
    transaction::{SignedTransaction, Transaction, TransactionV0},
    types::{BlockId, BlockReference, Finality},
    views::{FinalExecutionOutcomeView, QueryRequest, TxExecutionStatus},
};
use near_sdk::{
    json_types::Base64VecU8,
    serde::{de::DeserializeOwned, Deserialize, Serialize},
    serde_json::{self, json},
    AccountId, AccountIdRef, Gas, NearToken,
};
use near_sdk_contract_tools::standard::nep145::{StorageBalance, StorageBalanceBounds};

use templar_common::{
    market::MarketConfiguration,
    number::Decimal,
    oracle::{
        price_transformer::{Call, PriceTransformer},
        proxy::{Proxy, Source},
        pyth::{self, PriceIdentifier},
        redstone, OracleRequest,
    },
    time::Nanoseconds,
};
use templar_universal_account::{KeyId, KeyParameters, PayloadExecutionParameters};

use crate::{cache::Cache, AssetResolution, MarketData, ViewMarketPrices};

pub const STORAGE_DEPOSIT_GAS: Gas = Gas::from_tgas(5);
pub const DEPLOY_GAS: Gas = Gas::from_tgas(50);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct DeployArgs {
    name: String,
    version_key: String,
    init_args: Base64VecU8,
    #[serde(skip_serializing_if = "Option::is_none")]
    full_access_keys: Option<Vec<near_sdk::PublicKey>>,
}

impl DeployArgs {
    /// # Panics
    ///
    /// - On `init_args` serialization error.
    #[allow(clippy::unwrap_used)]
    pub fn new(
        name: String,
        version_key: String,
        init_args: &impl Serialize,
        full_access_keys: Option<Vec<near_sdk::PublicKey>>,
    ) -> Self {
        Self {
            name,
            version_key,
            init_args: Base64VecU8(near_sdk::serde_json::to_vec(init_args).unwrap()),
            full_access_keys,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Near {
    client: JsonRpcClient,
    account_id: AccountId,
    signers: Arc<Vec<Signer>>,
    signer_ix: Arc<AtomicUsize>,
}

#[derive(Debug, thiserror::Error)]
pub enum NearError {
    #[error("Rpc error: {0}")]
    RpcError(#[from] Box<dyn std::error::Error>),
    #[error("Parse error: {0}")]
    ParseError(#[from] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ViewError {
    #[error("Rpc error: {0}")]
    Rpc(#[from] JsonRpcError<RpcQueryError>),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl ViewError {
    pub fn is_method_not_found(&self) -> bool {
        matches!(
            self,
            ViewError::Rpc(JsonRpcError::ServerError(JsonRpcServerError::HandlerError(
                RpcQueryError::ContractExecutionError { vm_error, .. }
            ))) if vm_error.contains("MethodNotFound")
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoadMarketAccountsError {
    #[error("View error: {0}")]
    View(#[from] ViewError),
    #[error(transparent)]
    ResolvePriceIdentifier(#[from] ResolvePriceIdentifierError),
}

#[derive(Debug, thiserror::Error)]
pub enum ResolvePriceIdentifierError {
    #[error("View error: {0}")]
    View(#[from] ViewError),
    #[error(transparent)]
    NotFound(#[from] NotFoundError),
}

#[derive(Debug, thiserror::Error)]
#[error("Price identifier not defined on oracle {oracle_id}: {price_identifier}")]
pub struct NotFoundError {
    pub oracle_id: AccountId,
    pub price_identifier: PriceIdentifier,
}

#[derive(Debug, thiserror::Error)]
pub enum SourcePriceError {
    #[error("Source returned no price")]
    Missing,
    #[error("Failed to convert publish_time")]
    InvalidPublishTime,
    #[error("Price is stale")]
    Stale,
}

#[derive(Debug, Clone)]
pub struct FetchNonce {
    pub nonce: u64,
    pub block_height: u64,
    pub block_hash: CryptoHash,
}

#[allow(clippy::unwrap_used)]
impl Near {
    pub fn new(client: JsonRpcClient, account_id: AccountId, signers: Vec<Signer>) -> Self {
        Self {
            client,
            account_id,
            signers: Arc::new(signers),
            signer_ix: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// # Errors
    ///
    /// - RPC errors
    #[tracing::instrument(skip(self), name = "fetch_gas_price")]
    pub async fn fetch_gas_price(&self) -> Result<NearToken, JsonRpcError<RpcGasPriceError>> {
        let method = methods::gas_price::RpcGasPriceRequest { block_id: None };
        let response = self.client.call(method).await?;
        let price = NearToken::from_yoctonear(response.gas_price);
        tracing::trace!(gas_price = %price, "Fetched gas price");
        Ok(price)
    }

    /// # Errors
    ///
    /// - RPC errors
    #[tracing::instrument(skip(self))]
    pub async fn fetch_protocol_config(
        &self,
    ) -> Result<
        methods::EXPERIMENTAL_protocol_config::RpcProtocolConfigResponse,
        JsonRpcError<methods::EXPERIMENTAL_protocol_config::RpcProtocolConfigError>,
    > {
        let method = methods::EXPERIMENTAL_protocol_config::RpcProtocolConfigRequest {
            block_reference: BlockReference::latest(),
        };
        let response = self.client.call(method).await?;
        tracing::trace!(protocol_config = ?response, "Fetched protocol config");
        Ok(response)
    }

    /// # Errors
    ///
    /// - RPC errors
    pub async fn fetch_block_timestamp_ms(
        &self,
        block_hash: CryptoHash,
    ) -> Result<u64, JsonRpcError<RpcBlockError>> {
        let response = self
            .client
            .call(methods::block::RpcBlockRequest {
                block_reference: BlockId::Hash(block_hash).into(),
            })
            .await?;

        Ok(response.header.timestamp_nanosec / 1_000_000)
    }

    /// # Errors
    ///
    /// - RPC errors
    #[tracing::instrument(skip(self), fields(account_id = %account_id, transaction_hash = %transaction_hash))]
    pub async fn fetch_transaction_status(
        &self,
        account_id: AccountId,
        transaction_hash: CryptoHash,
    ) -> Result<FinalExecutionOutcomeView, JsonRpcError<RpcTransactionError>> {
        tracing::debug!("Fetching transaction status");
        let response = self
            .client
            .call(methods::tx::RpcTransactionStatusRequest {
                transaction_info: methods::tx::TransactionInfo::TransactionId {
                    tx_hash: transaction_hash,
                    sender_account_id: account_id,
                },
                wait_until: near_primitives::views::TxExecutionStatus::Final,
            })
            .await?;

        #[allow(
            clippy::unwrap_used,
            reason = "TxExecutionStatus::Final guarantees outcome is not None"
        )]
        let outcome = response.final_execution_outcome.unwrap().into_outcome();
        tracing::debug!(status = ?outcome.status, "Transaction status fetched");
        Ok(outcome)
    }

    pub fn next_signer(&self) -> &Signer {
        use std::sync::atomic::Ordering;
        let i = match self
            .signer_ix
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |ix| {
                Some((ix + 1) % self.signers.len())
            }) {
            Ok(i) | Err(i) => i,
        };
        &self.signers[i]
    }

    /// # Errors
    ///
    /// - RPC errors
    pub async fn fetch_nonce(
        &self,
        account_id: AccountId,
        public_key: PublicKey,
    ) -> Result<FetchNonce, JsonRpcError<RpcQueryError>> {
        let response = self
            .client
            .call(methods::query::RpcQueryRequest {
                block_reference: Finality::Final.into(),
                request: QueryRequest::ViewAccessKey {
                    account_id,
                    public_key,
                },
            })
            .await?;

        let QueryResponseKind::AccessKey(access_key) = response.kind else {
            unimplemented!("Invalid response kind");
        };

        Ok(FetchNonce {
            nonce: access_key.nonce,
            block_hash: response.block_hash,
            block_height: response.block_height,
        })
    }

    /// # Errors
    ///
    /// - RPC errors
    pub async fn fetch_near_balance(
        &self,
        account_id: AccountId,
    ) -> Result<NearToken, JsonRpcError<RpcQueryError>> {
        let response = self
            .client
            .call(methods::query::RpcQueryRequest {
                block_reference: Finality::Final.into(),
                request: QueryRequest::ViewAccount { account_id },
            })
            .await?;

        let QueryResponseKind::ViewAccount(account) = response.kind else {
            unreachable!("Invalid response kind");
        };

        Ok(NearToken::from_yoctonear(
            account.amount.saturating_sub(account.locked),
        ))
    }

    /// # Errors
    ///
    /// - RPC errors for nonce query
    #[must_use]
    pub async fn construct_delegate_transaction(
        &self,
        cache: &Cache,
        signed_delegate_action: SignedDelegateAction,
    ) -> SignedTransaction {
        let delegate_receiver_id = signed_delegate_action.delegate_action.sender_id.clone();
        let signer = self.next_signer();
        let public_key = signer.public_key();

        let (nonce, block_hash) = cache
            .nonce(self.account_id.clone(), public_key.clone())
            .await;

        Transaction::V0(TransactionV0 {
            signer_id: self.account_id.clone(),
            public_key,
            nonce,
            receiver_id: delegate_receiver_id,
            block_hash,
            actions: vec![signed_delegate_action.into()],
        })
        .sign(signer)
    }

    /// Constructs a storage deposit transaction for the given account and contract.
    ///
    /// # Errors
    ///
    /// - RPC transaction error
    #[must_use]
    pub async fn construct_storage_deposit_transaction(
        &self,
        cache: &Cache,
        account_id: AccountId,
        contract_id: AccountId,
        amount: NearToken,
    ) -> SignedTransaction {
        let signer = self.next_signer();
        let public_key = signer.public_key();

        let (nonce, block_hash) = cache
            .nonce(self.account_id.clone(), public_key.clone())
            .await;

        let action = FunctionCallAction {
            method_name: "storage_deposit".to_string(),
            args: serde_json::to_vec(&json!({
                "account_id": account_id,
            }))
            .unwrap(),
            gas: STORAGE_DEPOSIT_GAS.as_gas(),
            deposit: amount.as_yoctonear(),
        };

        Transaction::V0(TransactionV0 {
            signer_id: self.account_id.clone(),
            public_key,
            nonce,
            receiver_id: contract_id,
            block_hash,
            actions: vec![action.into()],
        })
        .sign(signer)
    }

    /// Deploy a version of a contract from a registry.
    #[must_use]
    pub async fn construct_deploy_from_registry_transaction(
        &self,
        cache: &Cache,
        registry_id: AccountId,
        args: &DeployArgs,
    ) -> SignedTransaction {
        let signer = self.next_signer();
        let public_key = signer.public_key();

        let (nonce, block_hash) = cache
            .nonce(self.account_id.clone(), public_key.clone())
            .await;

        let action = FunctionCallAction {
            method_name: "deploy".to_string(),
            args: serde_json::to_vec(args).unwrap(),
            gas: DEPLOY_GAS.as_gas(),
            deposit: 0,
        };

        Transaction::V0(TransactionV0 {
            signer_id: self.account_id.clone(),
            public_key,
            nonce,
            receiver_id: registry_id,
            block_hash,
            actions: vec![action.into()],
        })
        .sign(signer)
    }

    #[must_use]
    pub async fn construct_ua_execute_transaction(
        &self,
        cache: &Cache,
        ua_account_id: AccountId,
        args: &serde_json::Value,
        gas: u64,
    ) -> SignedTransaction {
        let signer = self.next_signer();
        let public_key = signer.public_key();

        let (nonce, block_hash) = cache
            .nonce(self.account_id.clone(), public_key.clone())
            .await;

        let action = FunctionCallAction {
            method_name: "execute".to_string(),
            args: serde_json::to_vec(&json!({ "args": args })).unwrap(),
            gas,
            deposit: 0,
        };

        Transaction::V0(TransactionV0 {
            signer_id: self.account_id.clone(),
            public_key,
            nonce,
            receiver_id: ua_account_id,
            block_hash,
            actions: vec![action.into()],
        })
        .sign(signer)
    }

    #[must_use]
    pub async fn sign_transaction(
        &self,
        cache: &Cache,
        receiver_id: AccountId,
        actions: Vec<near_primitives::action::Action>,
    ) -> SignedTransaction {
        let signer = self.next_signer();
        let public_key = signer.public_key();

        let (nonce, block_hash) = cache
            .nonce(self.account_id.clone(), public_key.clone())
            .await;

        Transaction::V0(TransactionV0 {
            signer_id: self.account_id.clone(),
            public_key,
            nonce,
            receiver_id,
            block_hash,
            actions,
        })
        .sign(signer)
    }

    /// # Errors
    ///
    /// - RPC errors
    #[tracing::instrument(skip(self, signed_transaction), fields(
        transaction_hash = %signed_transaction.get_hash(),
        wait_until = ?wait_until
    ))]
    pub async fn send_transaction(
        &self,
        signed_transaction: SignedTransaction,
        wait_until: TxExecutionStatus,
    ) -> Result<RpcTransactionResponse, JsonRpcError<RpcTransactionError>> {
        tracing::info!("Sending transaction to NEAR");
        let result = self
            .client
            .call(methods::send_tx::RpcSendTransactionRequest {
                signed_transaction,
                wait_until,
            })
            .await;

        match &result {
            Ok(_) => tracing::info!("Transaction sent successfully"),
            Err(e) => tracing::error!(error = %e, "Failed to send transaction"),
        }

        result
    }

    pub async fn view_raw(
        &self,
        account_id: AccountId,
        method_name: String,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, JsonRpcError<RpcQueryError>> {
        let result = self
            .client
            .call(methods::query::RpcQueryRequest {
                block_reference: Finality::Final.into(),
                request: QueryRequest::CallFunction {
                    account_id,
                    method_name,
                    args: args.into(),
                },
            })
            .await?;

        let QueryResponseKind::CallResult(result) = result.kind else {
            unimplemented!("Invalid response kind");
        };

        Ok(result.result)
    }

    pub async fn view<T: DeserializeOwned>(
        &self,
        account_id: AccountId,
        method_name: impl Into<String>,
        args: impl Serialize,
    ) -> Result<T, ViewError> {
        let raw_result = self
            .view_raw(account_id, method_name.into(), serde_json::to_vec(&args)?)
            .await?;

        Ok(serde_json::from_slice(&raw_result)?)
    }

    /// # Errors
    ///
    /// - Serialization/deserialization errors
    /// - RPC errors
    pub async fn load_deployments_from_registry(
        &self,
        registry_id: AccountId,
    ) -> Result<Vec<AccountId>, ViewError> {
        self.view(registry_id, "list_deployments", json!({})).await
    }

    /// # Errors
    ///
    /// - Serialization/deserialization errors
    /// - RPC errors
    pub async fn load_versions_from_registry(
        &self,
        registry_id: AccountId,
    ) -> Result<Vec<String>, ViewError> {
        self.view(registry_id, "list_versions", json!({})).await
    }

    /// # Errors
    ///
    /// - RPC errors
    pub async fn load_storage_balance_bounds(
        &self,
        contract_id: AccountId,
    ) -> Result<StorageBalanceBounds, ViewError> {
        self.view(contract_id, "storage_balance_bounds", json!({}))
            .await
    }

    /// # Errors
    ///
    /// - RPC errors
    pub async fn load_storage_balance_of(
        &self,
        contract_id: AccountId,
        account_id: &AccountIdRef,
    ) -> Result<Option<StorageBalance>, ViewError> {
        self.view(
            contract_id,
            "storage_balance_of",
            &json!({ "account_id": account_id }),
        )
        .await
    }

    /// # Errors
    ///
    /// - Serialization/deserialization errors
    /// - RPC errors
    pub async fn load_market_accounts(
        &self,
        market_id: AccountId,
    ) -> Result<MarketData, LoadMarketAccountsError> {
        let config = self
            .view::<MarketConfiguration>(market_id.clone(), "get_configuration", json!({}))
            .await?;

        let oracle_id = config.price_oracle_configuration.account_id.clone();

        let borrow_request = self
            .resolve_price_identifier(
                oracle_id.clone(),
                config.price_oracle_configuration.borrow_asset_price_id,
            )
            .await?;
        let collateral_request = self
            .resolve_price_identifier(
                oracle_id.clone(),
                config.price_oracle_configuration.collateral_asset_price_id,
            )
            .await?;

        Ok(MarketData {
            account_id: market_id.clone(),
            oracle_id,
            price_oracle_configuration: config.price_oracle_configuration.clone(),
            collateral: AssetResolution {
                asset: config.collateral_asset.clone(),
                price_id: config.price_oracle_configuration.collateral_asset_price_id,
                update_oracle: collateral_request,
            },
            borrow: AssetResolution {
                asset: config.borrow_asset.clone(),
                price_id: config.price_oracle_configuration.borrow_asset_price_id,
                update_oracle: borrow_request,
            },
        })
    }

    async fn fetch_transformer_input(&self, call: Call) -> Result<Decimal, ViewError> {
        let bytes = self
            .view_raw(call.account_id, call.method_name, call.args.0)
            .await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn current_time_ms() -> Nanoseconds {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        Nanoseconds::from_ms(u64::try_from(now).unwrap_or(u64::MAX))
    }

    async fn get_transformer(
        &self,
        oracle_id: AccountId,
        price_identifier: PriceIdentifier,
    ) -> Result<Option<PriceTransformer>, ViewError> {
        self.view(
            oracle_id,
            "get_transformer",
            json!({ "price_identifier": price_identifier }),
        )
        .await
    }

    async fn get_proxy(
        &self,
        oracle_id: AccountId,
        price_identifier: PriceIdentifier,
    ) -> Result<Option<Proxy>, ViewError> {
        self.view(oracle_id, "get_proxy", json!({ "id": price_identifier }))
            .await
    }

    #[tracing::instrument(skip(self), level = "debug")]
    async fn fetch_oracle_request(
        &self,
        request: OracleRequest,
        max_age: Nanoseconds,
    ) -> Result<Option<pyth::Price>, ViewError> {
        let fetched_price = match &request {
            OracleRequest::Pyth(request) => self
                .view::<pyth::OracleResponse>(
                    request.oracle_id.clone(),
                    "list_ema_prices_no_older_than",
                    json!({
                        "price_ids": [request.price_id],
                        "age": max_age.as_secs(),
                    }),
                )
                .await?
                .remove(&request.price_id)
                .flatten(),
            OracleRequest::RedStone(request) => self
                .view::<HashMap<redstone::FeedId, redstone::FeedData>>(
                    request.oracle_id.clone(),
                    "read_price_data",
                    json!({
                        "feed_ids": [request.price_id.clone()],
                    }),
                )
                .await?
                .remove(&request.price_id)
                .and_then(|feed| feed.to_pyth_price()),
        };

        Ok(fetched_price)
    }

    #[tracing::instrument(skip(self), level = "debug")]
    async fn resolve_price(
        &self,
        oracle_id: AccountId,
        price_identifier: PriceIdentifier,
        max_age: Nanoseconds,
    ) -> Result<Option<pyth::Price>, ViewError> {
        match self.query_oracle_type(oracle_id.clone()).await? {
            OracleType::PythDirect => {
                self.resolve_price_with_pyth(oracle_id, price_identifier, max_age)
                    .await
            }
            OracleType::PythLst { pyth_id } => {
                self.resolve_price_with_lst(oracle_id, pyth_id, price_identifier, max_age)
                    .await
            }
            OracleType::Proxy => {
                self.resolve_price_with_proxy(oracle_id, price_identifier, max_age)
                    .await
            }
        }
    }

    #[tracing::instrument(skip(self), level = "debug")]
    async fn resolve_price_with_pyth(
        &self,
        oracle_id: AccountId,
        price_identifier: PriceIdentifier,
        max_age: Nanoseconds,
    ) -> Result<Option<pyth::Price>, ViewError> {
        let final_price = self
            .fetch_oracle_request(OracleRequest::pyth(oracle_id, price_identifier), max_age)
            .await?;

        Ok(final_price)
    }

    #[tracing::instrument(skip(self), level = "debug")]
    async fn resolve_price_with_lst(
        &self,
        oracle_id: AccountId,
        pyth_id: AccountId,
        price_identifier: PriceIdentifier,
        max_age: Nanoseconds,
    ) -> Result<Option<pyth::Price>, ViewError> {
        let transformer = self.get_transformer(oracle_id, price_identifier).await?;

        let price = match transformer {
            Some(transformer) => {
                let price = self
                    .fetch_oracle_request(
                        OracleRequest::pyth(pyth_id, transformer.price_id),
                        max_age,
                    )
                    .await?;

                let input = self.fetch_transformer_input(transformer.call).await?;

                price.and_then(|price| transformer.action.apply(price, input))
            }
            None => {
                self.fetch_oracle_request(OracleRequest::pyth(pyth_id, price_identifier), max_age)
                    .await?
            }
        };

        Ok(price)
    }

    #[tracing::instrument(skip(self), level = "debug")]
    async fn resolve_price_with_proxy(
        &self,
        oracle_id: AccountId,
        price_identifier: PriceIdentifier,
        max_age: Nanoseconds,
    ) -> Result<Option<pyth::Price>, ViewError> {
        let Some(proxy) = self.get_proxy(oracle_id.clone(), price_identifier).await? else {
            return Ok(None);
        };

        let mut prices = vec![];
        for entry in &proxy.entries {
            if let Some(price) = self.resolve_proxy_entry_price(entry, max_age).await? {
                prices.push((price, entry.weight));
            }
        }

        let aggregated_price = proxy.aggregator.aggregate(&prices, Self::current_time_ms());

        Ok(aggregated_price.map(Into::into))
    }

    #[tracing::instrument(skip(self), level = "debug")]
    async fn resolve_proxy_entry_price(
        &self,
        entry: &templar_common::oracle::proxy::Entry,
        max_age: Nanoseconds,
    ) -> Result<Option<pyth::Price>, ViewError> {
        match &entry.source {
            Source::Request(request) => self.fetch_oracle_request(request.clone(), max_age).await,
            Source::Transformer(t) => {
                let Some(price) = self
                    .fetch_oracle_request(t.request.clone(), max_age)
                    .await?
                else {
                    return Ok(None);
                };

                let input = self.fetch_transformer_input(t.call.clone()).await?;
                Ok(t.action.apply(price, input))
            }
        }
    }

    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn load_market_prices(
        &self,
        market: &MarketData,
    ) -> Result<ViewMarketPrices, ViewError> {
        let oracle_config = &market.price_oracle_configuration;
        let max_age = Nanoseconds::from_secs(u64::from(oracle_config.price_maximum_age_s));

        let borrow = self.resolve_price(
            oracle_config.account_id.clone(),
            oracle_config.borrow_asset_price_id,
            max_age,
        );
        let collateral = self.resolve_price(
            oracle_config.account_id.clone(),
            oracle_config.collateral_asset_price_id,
            max_age,
        );
        let (borrow, collateral) = tokio::join!(borrow, collateral);

        Ok(ViewMarketPrices {
            borrow: borrow?,
            collateral: collateral?,
        })
    }

    #[tracing::instrument(skip(self), level = "debug")]
    async fn query_oracle_type(&self, oracle_id: AccountId) -> Result<OracleType, ViewError> {
        let test_proxy = self
            .view::<Vec<PriceIdentifier>>(oracle_id.clone(), "list_proxies", json!({ "count": 1 }))
            .await;

        match test_proxy {
            Ok(_) => {
                tracing::debug!("Oracle supports proxy interface, treating as proxy oracle");

                return Ok(OracleType::Proxy);
            }
            Err(e) if e.is_method_not_found() => {
                tracing::debug!("Not a proxy oracle");
            }
            Err(error) => {
                tracing::debug!(%error, "RPC error when querying for proxy interface");
                return Err(error);
            }
        }

        let test_lst = self
            .view::<Vec<PriceIdentifier>>(
                oracle_id.clone(),
                "list_transformers",
                json!({ "count": 1 }),
            )
            .await;

        match test_lst {
            Ok(_) => {
                tracing::debug!("Oracle supports transformer interface, treating as LST oracle");

                let pyth_id = self
                    .view::<AccountId>(oracle_id.clone(), "oracle_id", json!({}))
                    .await?;

                return Ok(OracleType::PythLst { pyth_id });
            }
            Err(e) if e.is_method_not_found() => {
                tracing::debug!("Not an LST oracle");
            }
            Err(error) => {
                tracing::debug!(%error, "RPC error when querying for LST interface");
                return Err(error);
            }
        }

        Ok(OracleType::PythDirect)
    }

    /// Returns the oracle and price ID that should be updated in order to
    /// update the given price identifier for the given oracle contract.
    #[tracing::instrument(level = "debug", skip_all, fields(oracle_id = %oracle_id, price_identifier = %price_identifier))]
    pub async fn resolve_price_identifier(
        &self,
        oracle_id: AccountId,
        price_identifier: PriceIdentifier,
    ) -> Result<HashSet<OracleRequest>, ResolvePriceIdentifierError> {
        fn one_pyth(oracle_id: AccountId, price_id: PriceIdentifier) -> HashSet<OracleRequest> {
            HashSet::from_iter([OracleRequest::pyth(oracle_id, price_id)])
        }

        match self.query_oracle_type(oracle_id.clone()).await? {
            OracleType::PythDirect => {
                tracing::debug!("Price ID resolved: direct Pyth oracle contract");
                Ok(one_pyth(oracle_id, price_identifier))
            }
            OracleType::PythLst { pyth_id } => {
                if let Some(transformer) = self
                    .view::<Option<PriceTransformer>>(
                        oracle_id.clone(),
                        "get_transformer",
                        json!({ "price_identifier": price_identifier }),
                    )
                    .await?
                {
                    tracing::debug!("Price ID resolved: LST oracle contract: transformed");
                    Ok(one_pyth(pyth_id, transformer.price_id))
                } else {
                    tracing::debug!("Price ID resolved: LST oracle contract: passthrough");
                    Ok(one_pyth(pyth_id, price_identifier))
                }
            }
            OracleType::Proxy => {
                tracing::debug!("Price ID resolved: Proxy oracle contract");

                if let Some(proxy) = self
                    .view::<Option<Proxy>>(
                        oracle_id.clone(),
                        "get_proxy",
                        json!({ "id": price_identifier }),
                    )
                    .await?
                {
                    let requests = proxy
                        .entries
                        .into_iter()
                        .map(|entry| match entry.source {
                            Source::Transformer(transformer) => transformer.request,
                            Source::Request(request) => request,
                        })
                        .collect::<HashSet<_>>();
                    if requests.is_empty() {
                        tracing::error!("Proxy oracle contract returned empty proxy definition");
                        return Err(ResolvePriceIdentifierError::NotFound(NotFoundError {
                            oracle_id,
                            price_identifier,
                        }));
                    }
                    Ok(requests)
                } else {
                    tracing::debug!("Price ID not found on proxy oracle contract");
                    Err(ResolvePriceIdentifierError::NotFound(NotFoundError {
                        oracle_id,
                        price_identifier,
                    }))
                }
            }
        }
    }

    /// # Errors
    ///
    /// - RPC errors
    pub async fn load_ua_key(
        &self,
        ua_account_id: AccountId,
        key: KeyId,
    ) -> Result<Option<PayloadExecutionParameters>, ViewError> {
        let view = self
            .view::<Option<VersionedKeyParameters>>(
                ua_account_id.clone(),
                "get_key",
                json!({ "key": key }),
            )
            .await?;

        Ok(view.map(|v| match v {
            VersionedKeyParameters::V1(p) => p,
            VersionedKeyParameters::V0(p) => PayloadExecutionParameters::builder_empty()
                .with_key_parameters(p)
                .verifying_contract(ua_account_id)
                .build(),
        }))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub enum VersionedKeyParameters {
    #[serde(untagged)]
    V1(PayloadExecutionParameters),
    #[serde(untagged)]
    V0(KeyParameters),
}

#[derive(Debug)]
pub enum OracleType {
    PythDirect,
    PythLst { pyth_id: AccountId },
    Proxy,
}
