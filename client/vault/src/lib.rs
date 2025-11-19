// TODO: near client setup with signer
// TODO: expose client methods for things usually requiring actions
// TODO: expose stream callbacks for handling events
// TODO: pyo3
// TODO: wasm
// TODO: ensure we zeroize secrets
//

use std::{
    fmt::Display,
    str::FromStr,
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use near_account_id::AccountId as NearAccountId;
use near_crypto::{InMemorySigner, SecretKey, Signer};
use near_jsonrpc_client::{
    methods::{
        query::RpcQueryRequest,
        send_tx::RpcSendTransactionRequest,
        tx::{RpcTransactionError, RpcTransactionStatusRequest, TransactionInfo},
    },
    JsonRpcClient,
};
use near_jsonrpc_primitives::{errors::RpcError, types::query::QueryResponseKind};
use near_primitives::{
    action::{Action, FunctionCallAction},
    hash::CryptoHash,
    transaction::{SignedTransaction, Transaction, TransactionV0},
    types::{BlockReference, Gas},
    views::{FinalExecutionStatus, QueryRequest, TxExecutionStatus},
};
use near_sdk::json_types::{U128, U64};
use serde::{de::DeserializeOwned, Serialize};
use templar_common::vault::VaultConfiguration;
use tracing::{debug, info, instrument, warn};

uniffi::setup_scaffolding!();

#[derive(uniffi::Enum)]
pub enum Event {}

impl From<templar_common::vault::Event> for Event {
    fn from(value: templar_common::vault::Event) -> Self {
        todo!()
    }
}

#[derive(uniffi::Record)]
pub struct Delta {
    pub market: AccountId,
    pub amount: ForeignU128,
}


#[uniffi::export(callback_interface)]
pub trait EventHandler {
    fn handle(&self, event: Event);
}
pub const DEFAULT_GAS: Gas = 300 * 1e12 as u64;
pub const MAX_POLL_INTERVAL_MILLIS: u64 = 1000;

#[derive(uniffi::Object)]
pub struct Client {
    inner: JsonRpcClient,
    signer: Signer,
    pub vault: NearAccountId,
    timeout: u64,
}
impl Client {
    #[instrument(skip(signer), fields(vault = %vault, timeout))]
    pub fn new(inner: JsonRpcClient, signer: Signer, vault: NearAccountId, timeout: u64) -> Self {
        info!("Constructing Client");
        Self {
            inner,
            signer,
            vault,
            timeout,
        }
    }

    /// Get access key data (nonce and block hash) for transaction signing.
    ///
    /// # Arguments
    ///
    /// * `client` - JSON-RPC client instance
    /// * `signer` - Signer with the account and key to query
    ///
    /// # Returns
    ///
    /// Tuple of (nonce, block_hash) to use when constructing a transaction
    #[instrument(skip(self))]
    pub async fn get_access_key_data(&self) -> Result<(u64, CryptoHash)> {
        info!("Querying access key data");
        let access_key_query_response = self
            .inner
            .call(RpcQueryRequest {
                block_reference: BlockReference::latest(),
                request: QueryRequest::ViewAccessKey {
                    account_id: self.signer.get_account_id(),
                    public_key: self.signer.public_key().clone(),
                },
            })
            .await?;

        let nonce = match access_key_query_response.kind {
            QueryResponseKind::AccessKey(access_key) => access_key.nonce + 1,
            _ => {
                bail!(
                    "Expected AccessKey got {:?}",
                    access_key_query_response.kind
                );
            }
        };
        let block_hash = access_key_query_response.block_hash;

        debug!("Got access key data (nonce={})", nonce);
        Ok((nonce, block_hash))
    }

    #[instrument(skip(self, args), fields(account_id = %account_id, method = function_name, timeout))]
    pub async fn view<T: DeserializeOwned>(
        &self,
        account_id: &NearAccountId,
        function_name: &str,
        args: impl Serialize,
        timeout: u64,
    ) -> Result<T> {
        info!("Starting view call");
        let response = tokio::time::timeout(
            Duration::from_secs(timeout),
            self.inner.call(RpcQueryRequest {
                block_reference: BlockReference::latest(),
                request: QueryRequest::CallFunction {
                    account_id: account_id.clone(),
                    method_name: function_name.to_owned(),
                    args: serde_json::to_vec(&args)?.into(),
                },
            }),
        )
        .await??;

        let QueryResponseKind::CallResult(result) = response.kind else {
            bail!("Expected CallResult got {:?}", response.kind);
        };

        let value = serde_json::from_slice(&result.result)?;
        info!("View call succeeded");
        Ok(value)
    }

    #[instrument(skip(self, args), fields(account_id = %account_id, method = function_name, gas = ?gas, deposit = ?deposit, timeout))]
    pub async fn call(
        &self,
        account_id: &NearAccountId,
        function_name: &str,
        args: impl Serialize,
        gas: Option<Gas>,
        deposit: Option<u128>,
        timeout: u64,
    ) -> Result<FinalExecutionStatus> {
        info!("Submitting call transaction");
        let (nonce, block_hash) = self.get_access_key_data().await?;

        let tx = Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: account_id.clone(),
            block_hash,
            signer_id: self.signer.get_account_id(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: function_name.to_string(),
                args: serde_json::to_vec(&args)?,
                gas: gas.unwrap_or(DEFAULT_GAS),
                deposit: deposit.unwrap_or(0),
            }))],
        });

        let (tx_hash, _size) = tx.get_hash_and_size();

        let called_at = Instant::now();
        let signature = self.signer.sign(tx_hash.as_ref());
        let deadline = called_at + Duration::from_secs(timeout);
        let result = match self
            .inner
            .call(RpcSendTransactionRequest {
                signed_transaction: SignedTransaction::new(signature, tx),
                wait_until: TxExecutionStatus::Final,
            })
            .await
        {
            Ok(res) => res,
            Err(e) => {
                warn!(
                    "Send transaction error: {:?}. Starting status polling until deadline.",
                    e
                );
                loop {
                    if !matches!(e.handler_error(), Some(RpcTransactionError::TimeoutError)) {
                        return Err(e.into());
                    }

                    // Poll with exponential backoff
                    let mut poll_interval = Duration::from_millis(500);

                    loop {
                        if Instant::now() >= deadline {
                            warn!("Transaction polling deadline exceeded, aborting");
                            bail!("Transaction timed out");
                        }

                        tokio::time::sleep(poll_interval).await;
                        debug!("Polling transaction status...");

                        // Exponential backoff up to MAX_POLL_INTERVAL
                        poll_interval = std::cmp::min(
                            poll_interval * 2,
                            Duration::from_millis(MAX_POLL_INTERVAL_MILLIS),
                        );

                        let status = self
                            .inner
                            .call(RpcTransactionStatusRequest {
                                transaction_info: TransactionInfo::TransactionId {
                                    sender_account_id: self.signer.get_account_id(),
                                    tx_hash,
                                },
                                wait_until: TxExecutionStatus::Final,
                            })
                            .await;

                        let Err(e) = status else {
                            break;
                        };

                        if !matches!(e.handler_error(), Some(RpcTransactionError::TimeoutError)) {
                            warn!("Transaction status error: {:?}", e);
                            return Err(e.into());
                        }
                    }
                }
            }
        };

        let Some(outcome) = result.final_execution_outcome else {
            bail!("No outcome {}", tx_hash);
        };

        let status = outcome.into_outcome().status;
        info!("Transaction executed");
        debug!("Final execution status: {:?}", status);
        if let FinalExecutionStatus::Failure(tx_err) = &status {
            bail!("Transaction failed: {:?}", tx_err);
        }
        Ok(status)
    }
}

#[derive(uniffi::Error, Debug)]
pub enum ErrorWrapper {
    Wrapped(String),
}

impl Display for ErrorWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorWrapper::Wrapped(err) => write!(f, "Error: {}", err),
        }
    }
}

impl<T: Into<anyhow::Error>> From<T> for ErrorWrapper {
    fn from(err: T) -> Self {
        ErrorWrapper::Wrapped(err.into().to_string())
    }
}

#[derive(Clone)]
pub struct AccountId(String);

uniffi::custom_type!(AccountId, String);

impl From<AccountId> for near_account_id::AccountId {
    fn from(value: AccountId) -> Self {
        near_account_id::AccountId::from_str(&value.0).expect("Invalid AccountId")
    }
}

impl From<String> for AccountId {
    fn from(value: String) -> Self {
        AccountId(value)
    }
}

impl From<AccountId> for String {
    fn from(value: AccountId) -> Self {
        value.0
    }
}

type ForeignU128 = String;

// TODO: records with From<libtype>

#[uniffi::export]
impl Client {
    #[uniffi::constructor]
    #[instrument(skip(signer_key, signer_account, vault), fields(rpc_url = %rpc_url, timeout))]
    pub fn new_client(
        rpc_url: String,
        signer_account: &AccountId,
        signer_key: &str,
        vault: &AccountId,
        timeout: u64,
    ) -> Result<Self, ErrorWrapper> {
        info!("Connecting JSON-RPC client");
        let inner = JsonRpcClient::connect(rpc_url);

        let signer = InMemorySigner::from_secret_key(
            NearAccountId::from(signer_account.clone()),
            SecretKey::from_str(signer_key).map_err(ErrorWrapper::from)?,
        );

        let vault: NearAccountId = NearAccountId::from(vault.clone());

        info!("Client created for vault");
        Ok(Self {
            inner,
            signer,
            vault,
            timeout,
        })
    }

    // pub async fn get_configuration(&self) -> Result<VaultConfiguration> {
    //     todo!()
    // }

    #[instrument(skip(self))]
    pub async fn get_total_assets(&self) -> Result<ForeignU128, ErrorWrapper> {
        info!("Fetching total assets");
        let res = self
            .view::<U128>(&self.vault, "get_total_assets", (), self.timeout)
            .await
            .and_then(|u| serde_json::to_string(&u).map_err(Into::into))
            .map_err(ErrorWrapper::from)?;
        info!("Fetched total assets");
        Ok(res)
    }

    #[instrument(skip(self))]
    pub async fn get_idle_balance(&self) -> Result<ForeignU128, ErrorWrapper> {
        info!("Fetching idle balance");
        let res = self
            .view::<U128>(&self.vault, "get_idle_balance", (), self.timeout)
            .await
            .and_then(|u| serde_json::to_string(&u).map_err(Into::into))
            .map_err(ErrorWrapper::from)?;
        info!("Fetched idle balance");
        Ok(res)
    }

    #[instrument(skip(self))]
    pub async fn get_total_supply(&self) -> Result<ForeignU128, ErrorWrapper> {
        info!("Fetching total supply");
        let res = self
            .view::<U128>(&self.vault, "get_total_supply", (), self.timeout)
            .await
            .and_then(|u| serde_json::to_string(&u).map_err(Into::into))
            .map_err(ErrorWrapper::from)?;
        info!("Fetched total supply");
        Ok(res)
    }

    #[instrument(skip(self))]
    pub async fn get_max_deposit(&self) -> Result<ForeignU128, ErrorWrapper> {
        info!("Fetching max deposit");
        let res = self
            .view::<U128>(&self.vault, "get_max_deposit", (), self.timeout)
            .await
            .and_then(|u| serde_json::to_string(&u).map_err(Into::into))
            .map_err(ErrorWrapper::from)?;
        info!("Fetched max deposit");
        Ok(res)
    }

    #[instrument(skip(self, assets))]
    pub async fn convert_to_shares(
        &self,
        assets: &ForeignU128,
    ) -> Result<ForeignU128, ErrorWrapper> {
        let assets: U128 = serde_json::from_str(assets).map_err(ErrorWrapper::from)?;
        info!("Converting assets to shares");
        let res = self
            .view::<U128>(&self.vault, "convert_to_shares", (assets,), self.timeout)
            .await
            .and_then(|u| serde_json::to_string(&u).map_err(Into::into))
            .map_err(ErrorWrapper::from)?;
        info!("Converted assets to shares");
        Ok(res)
    }

    #[instrument(skip(self, shares))]
    pub async fn convert_to_assets(
        &self,
        shares: &ForeignU128,
    ) -> Result<ForeignU128, ErrorWrapper> {
        let shares: U128 = serde_json::from_str(shares).map_err(ErrorWrapper::from)?;
        info!("Converting shares to assets");
        let res = self
            .view::<U128>(&self.vault, "convert_to_assets", (shares,), self.timeout)
            .await
            .and_then(|u| serde_json::to_string(&u).map_err(Into::into))
            .map_err(ErrorWrapper::from)?;
        info!("Converted shares to assets");
        Ok(res)
    }

    #[instrument(skip(self, assets))]
    pub async fn preview_deposit(&self, assets: &ForeignU128) -> Result<ForeignU128, ErrorWrapper> {
        let assets: U128 = serde_json::from_str(assets).map_err(ErrorWrapper::from)?;
        info!("Previewing deposit");
        let res = self
            .view::<U128>(&self.vault, "preview_deposit", (assets,), self.timeout)
            .await
            .and_then(|u| serde_json::to_string(&u).map_err(Into::into))
            .map_err(ErrorWrapper::from)?;
        info!("Preview deposit completed");
        Ok(res)
    }

    #[instrument(skip(self, shares))]
    pub async fn preview_mint(&self, shares: &ForeignU128) -> Result<ForeignU128, ErrorWrapper> {
        let shares: U128 = serde_json::from_str(shares).map_err(ErrorWrapper::from)?;
        info!("Previewing mint");
        let res = self
            .view::<U128>(&self.vault, "preview_mint", (shares,), self.timeout)
            .await
            .and_then(|u| serde_json::to_string(&u).map_err(Into::into))
            .map_err(ErrorWrapper::from)?;
        info!("Preview mint completed");
        Ok(res)
    }

    #[instrument(skip(self, shares))]
    pub async fn preview_withdraw(
        &self,
        shares: &ForeignU128,
    ) -> Result<ForeignU128, ErrorWrapper> {
        let shares: U128 = serde_json::from_str(shares).map_err(ErrorWrapper::from)?;
        info!("Previewing withdraw");
        let res = self
            .view::<U128>(&self.vault, "preview_withdraw", (shares,), self.timeout)
            .await
            .and_then(|u| serde_json::to_string(&u).map_err(Into::into))
            .map_err(ErrorWrapper::from)?;
        info!("Preview withdraw completed");
        Ok(res)
    }

    #[instrument(skip(self, shares))]
    pub async fn preview_redeem(&self, shares: &ForeignU128) -> Result<ForeignU128, ErrorWrapper> {
        let shares: U128 = serde_json::from_str(shares).map_err(ErrorWrapper::from)?;
        info!("Previewing redeem");
        let res = self
            .view::<U128>(&self.vault, "preview_redeem", (shares,), self.timeout)
            .await
            .and_then(|u| serde_json::to_string(&u).map_err(Into::into))
            .map_err(ErrorWrapper::from)?;
        info!("Preview redeem completed");
        Ok(res)
    }

    #[instrument(skip(self, shares, receiver))]
    pub async fn redeem(
        &self,
        shares: &ForeignU128,
        receiver: &AccountId,
    ) -> Result<(), ErrorWrapper> {
        let shares: U128 = serde_json::from_str(shares).map_err(ErrorWrapper::from)?;
        info!("Redeeming shares");
        self.call(
            &self.vault,
            "redeem",
            (shares, NearAccountId::from(receiver.clone())),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Redeem call submitted");
        Ok(())
    }


    #[instrument(skip(self, route))]
    pub async fn execute_withdrawal(&self, route: &[AccountId]) -> Result<(), ErrorWrapper> {
        let route: Vec<NearAccountId> = route
            .iter()
            .cloned()
            .map(|id| NearAccountId::from(id))
            .collect();
        info!("Executing withdrawal with route length {}", route.len());

        self.call(
            &self.vault,
            "execute_withdrawal",
            (route,),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Execute withdrawal call submitted");
        Ok(())
    }
