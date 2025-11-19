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

#[derive(uniffi::Record, Debug, Clone)]
pub struct Delta {
    pub market: AccountId,
    pub amount: ForeignU128,
}

impl From<templar_common::vault::Delta> for Delta {
    fn from(value: templar_common::vault::Delta) -> Self {
        Delta {
            market: AccountId(value.market.to_string()),
            amount: serde_json::to_string(&value.amount).unwrap(),
        }
    }
}

impl From<Delta> for templar_common::vault::Delta {
    fn from(value: Delta) -> Self {
        println!("Delta {:?}", value);
        templar_common::vault::Delta {
            market: value.market.into(),
            amount: U128(u128::from_str(value.amount.as_str()).unwrap()),
        }
    }
}

#[derive(uniffi::Enum, Debug, Clone)]
pub enum AllocationDelta {
    Supply(Delta),
    Withdraw(Delta),
}

impl From<templar_common::vault::AllocationDelta> for AllocationDelta {
    fn from(value: templar_common::vault::AllocationDelta) -> Self {
        match value {
            templar_common::vault::AllocationDelta::Supply(delta) => {
                AllocationDelta::Supply(delta.into())
            }
            templar_common::vault::AllocationDelta::Withdraw(delta) => {
                AllocationDelta::Withdraw(delta.into())
            }
        }
    }
}

impl From<AllocationDelta> for templar_common::vault::AllocationDelta {
    fn from(value: AllocationDelta) -> Self {
        match value {
            AllocationDelta::Supply(delta) => {
                templar_common::vault::AllocationDelta::Supply(delta.into())
            }
            AllocationDelta::Withdraw(delta) => {
                templar_common::vault::AllocationDelta::Withdraw(delta.into())
            }
        }
    }
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

#[derive(Clone, Debug)]
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

    #[instrument(skip(self))]
    pub async fn reallocate(&self, delta: &AllocationDelta) -> Result<(), ErrorWrapper> {
        info!("Reallocating shares");
        let delta = templar_common::vault::AllocationDelta::from(delta.to_owned());
        self.call(
            &self.vault,
            "reallocate",
            (delta,),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Reallocate call submitted");
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

    #[instrument(skip(self, batch_limit))]
    pub async fn execute_market_withdrawal(
        &self,
        op_id: &u64,
        market_index: &u32,
        batch_limit: Option<u32>,
    ) -> Result<(), ErrorWrapper> {
        info!(
            "Executing market withdrawal op_id={} market_index={} batch_limit={:?}",
            op_id, market_index, batch_limit
        );
        self.call(
            &self.vault,
            "execute_market_withdrawal",
            (op_id.to_string(), market_index, batch_limit),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Execute market withdrawal call submitted");
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn get_withdrawing_op_id(&self) -> Result<u64, ErrorWrapper> {
        info!("Fetching withdrawing op id");
        let res = self
            .view(&self.vault, "get_withdrawing_op_id", (), self.timeout)
            .await
            .map_err(ErrorWrapper::from)?;
        info!("Withdrawing op id fetched: {}", res);
        Ok(res)
    }

    #[instrument(skip(self))]
    pub async fn has_pending_market_withdrawal(&self) -> Result<bool, ErrorWrapper> {
        info!("Checking pending market withdrawal");
        let res = self
            .view(
                &self.vault,
                "has_pending_market_withdrawal",
                (),
                self.timeout,
            )
            .await
            .map_err(ErrorWrapper::from)?;
        info!("Pending market withdrawal: {}", res);
        Ok(res)
    }

    #[instrument(skip(self))]
    pub async fn get_current_withdraw_request_id(&self) -> Result<u64, ErrorWrapper> {
        info!("Fetching current withdraw request id");
        let res = self
            .view(
                &self.vault,
                "get_current_withdraw_request_id",
                (),
                self.timeout,
            )
            .await
            .map_err(ErrorWrapper::from)?;
        info!("Current withdraw request id: {}", res);
        Ok(res)
    }

    #[instrument(skip(self))]
    pub async fn cancel_in_flight_withdrawal(&self) -> Result<(), ErrorWrapper> {
        info!("Cancelling inflight withdrawal");
        self.call(
            &self.vault,
            "cancel_inflight_withdrawal",
            (),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Cancel inflight withdrawal call submitted");
        Ok(())
    }

    #[instrument(skip(self, token))]
    pub async fn skim(&self, token: &AccountId) -> Result<(), ErrorWrapper> {
        info!("Skimming token");
        self.call(
            &self.vault,
            "skim",
            &NearAccountId::from(token.clone()),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Skim call submitted");
        Ok(())
    }

    #[instrument(skip(self, account))]
    pub async fn set_curator(&self, account: &AccountId) -> Result<(), ErrorWrapper> {
        info!("Setting curator");
        self.call(
            &self.vault,
            "set_curator",
            &NearAccountId::from(account.clone()),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Set curator call submitted");
        Ok(())
    }

    #[instrument(skip(self, account))]
    pub async fn set_is_allocator(
        &self,
        account: &AccountId,
        allowed: bool,
    ) -> Result<(), ErrorWrapper> {
        info!("Setting allocator role");
        self.call(
            &self.vault,
            "set_is_allocator",
            (NearAccountId::from(account.clone()), allowed),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Allocator role call submitted");
        Ok(())
    }

    #[instrument(skip(self, new_g))]
    pub async fn submit_guardian(&self, new_g: &AccountId) -> Result<(), ErrorWrapper> {
        info!("Submitting guardian");
        self.call(
            &self.vault,
            "submit_guardian",
            &NearAccountId::from(new_g.clone()),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Submit guardian call submitted");
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn accept_guardian(&self) -> Result<(), ErrorWrapper> {
        info!("Accepting guardian");
        self.call(&self.vault, "accept_guardian", (), None, None, self.timeout)
            .await
            .map_err(ErrorWrapper::from)?;
        info!("Accept guardian call submitted");
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn revoke_pending_guardian(&self) -> Result<(), ErrorWrapper> {
        info!("Revoking pending guardian");
        self.call(
            &self.vault,
            "revoke_pending_guardian",
            (),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Revoke pending guardian call submitted");
        Ok(())
    }

    #[instrument(skip(self, account))]
    pub async fn set_skim_recipient(&self, account: &AccountId) -> Result<(), ErrorWrapper> {
        info!("Setting skim recipient");
        self.call(
            &self.vault,
            "set_skim_recipient",
            &NearAccountId::from(account.clone()),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Set skim recipient call submitted");
        Ok(())
    }

    #[instrument(skip(self, account, deposit_yocto))]
    pub async fn set_fee_recipient(
        &self,
        account: &AccountId,
        deposit_yocto: &ForeignU128,
    ) -> Result<(), ErrorWrapper> {
        let deposit: u128 = deposit_yocto.parse().map_err(ErrorWrapper::from)?;
        info!("Setting fee recipient");
        self.call(
            &self.vault,
            "set_fee_recipient",
            &NearAccountId::from(account.clone()),
            None,
            Some(deposit),
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Set fee recipient call submitted");
        Ok(())
    }

    #[instrument(skip(self, fee))]
    pub async fn set_performance_fee(&self, fee: &ForeignU128) -> Result<(), ErrorWrapper> {
        let fee: U128 = serde_json::from_str(fee).map_err(ErrorWrapper::from)?;
        info!("Setting performance fee");
        self.call(
            &self.vault,
            "set_performance_fee",
            (fee,),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Set performance fee call submitted");
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn submit_timelock(&self, new_timelock_ns: u64) -> Result<(), ErrorWrapper> {
        info!("Submitting timelock");
        self.call(
            &self.vault,
            "submit_timelock",
            (U64::from(new_timelock_ns),),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Submit timelock call submitted");
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn accept_timelock(&self) -> Result<(), ErrorWrapper> {
        info!("Accepting timelock");
        self.call(&self.vault, "accept_timelock", (), None, None, self.timeout)
            .await
            .map_err(ErrorWrapper::from)?;
        info!("Accept timelock call submitted");
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn revoke_pending_timelock(&self) -> Result<(), ErrorWrapper> {
        info!("Revoking pending timelock");
        self.call(
            &self.vault,
            "revoke_pending_timelock",
            (),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Revoke pending timelock call submitted");
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn submit_cap(
        &self,
        market: &AccountId,
        new_cap: &ForeignU128,
    ) -> Result<(), ErrorWrapper> {
        let new_cap: U128 = serde_json::from_str(new_cap).map_err(ErrorWrapper::from)?;
        info!("Submitting cap change");
        self.call(
            &self.vault,
            "submit_cap",
            (NearAccountId::from(market.clone()), new_cap),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Submit cap call submitted");
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn accept_cap(&self, market: &AccountId) -> Result<(), ErrorWrapper> {
        info!("Accepting cap change");
        self.call(
            &self.vault,
            "accept_cap",
            (NearAccountId::from(market.clone()),),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Accept cap call submitted");
        Ok(())
    }

    #[instrument(skip(self, market))]
    pub async fn revoke_pending_cap(&self, market: &AccountId) -> Result<(), ErrorWrapper> {
        info!("Revoking pending cap");
        self.call(
            &self.vault,
            "revoke_pending_cap",
            (NearAccountId::from(market.clone()),),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Revoke pending cap call submitted");
        Ok(())
    }

    #[instrument(skip(self, market))]
    pub async fn submit_market_removal(&self, market: &AccountId) -> Result<(), ErrorWrapper> {
        info!("Submitting market removal");
        self.call(
            &self.vault,
            "submit_market_removal",
            (NearAccountId::from(market.clone()),),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Submit market removal call submitted");
        Ok(())
    }

    #[instrument(skip(self, market))]
    pub async fn revoke_pending_market_removal(
        &self,
        market: &AccountId,
    ) -> Result<(), ErrorWrapper> {
        info!("Revoking pending market removal");
        self.call(
            &self.vault,
            "revoke_pending_market_removal",
            (NearAccountId::from(market.clone()),),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Revoke pending market removal call submitted");
        Ok(())
    }

    #[instrument(skip(self, markets, deposit_yocto))]
    pub async fn set_supply_queue(
        &self,
        markets: &[AccountId],
        deposit_yocto: &ForeignU128,
    ) -> Result<(), ErrorWrapper> {
        let deposit: u128 = deposit_yocto.parse().map_err(ErrorWrapper::from)?;
        let markets: Vec<NearAccountId> =
            markets.iter().cloned().map(NearAccountId::from).collect();
        info!("Setting supply queue, len={}", markets.len());
        self.call(
            &self.vault,
            "set_supply_queue",
            (markets,),
            None,
            Some(deposit),
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Set supply queue call submitted");
        Ok(())
    }

    #[instrument(skip(self, queue))]
    pub async fn set_withdraw_queue(&self, queue: &[AccountId]) -> Result<(), ErrorWrapper> {
        let queue: Vec<NearAccountId> = queue.iter().cloned().map(NearAccountId::from).collect();
        info!("Setting withdraw queue, len={}", queue.len());
        self.call(
            &self.vault,
            "set_withdraw_queue",
            (queue,),
            None,
            None,
            self.timeout,
        )
        .await
        .map_err(ErrorWrapper::from)?;
        info!("Set withdraw queue call submitted");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::u128;

    use super::*;
    use near_crypto::{KeyType, SecretKey};
    use near_sdk::NearToken;
    use rstest::{fixture, rstest};

    #[fixture]
    fn vault() -> AccountId {
        tracing_subscriber::fmt::init();
        AccountId("metavault.topgunbakugo.testnet".to_string())
    }

    #[fixture]
    fn everything() -> AccountId {
    }

    #[fixture]
    fn testnet_rpc() -> String {
        "https://rpc.testnet.fastnear.com/".to_string()
    }

    #[fixture]
    fn sk() -> SecretKey {
    }

    #[rstest]
    fn account_id_conversion_happy_path(everything: AccountId) {
        let near_id: NearAccountId = everything.clone().into();
        assert_eq!(near_id.as_str(), "topgunbakugo.testnet");
    }

    #[test]
    fn error_wrapper_display_happy_path() {
        let err = ErrorWrapper::from(anyhow::anyhow!("boom"));
        let s = format!("{}", err);
        assert!(s.contains("boom"));
    }

    #[test]
    fn default_gas_is_nonzero() {
        assert!(super::DEFAULT_GAS > 0);
    }

    #[rstest]
    fn can_construct_client_happy_path(
        vault: AccountId,
        everything: AccountId,
        testnet_rpc: String,
        sk: SecretKey,
    ) {
        Client::new_client(testnet_rpc, &everything, &sk.to_string(), &vault, 5)
            .expect("Client should be created");
    }

    // The following async tests exercise the happy-path surfaces but are ignored by default
    // because they require a running NEAR RPC endpoint and appropriate accounts/contracts.
    #[rstest]
    #[tokio::test(flavor = "current_thread")]
    async fn view_methods_happy_path_smoke(vault: AccountId, testnet_rpc: String) {
        let sk = SecretKey::from_random(KeyType::ED25519);
        let signer_account = AccountId("alice.testnet".to_string());
        let client = Client::new_client(testnet_rpc, &signer_account, &sk.to_string(), &vault, 5)
            .expect("Client should be created");
        println!("Total Assets: {}", client.get_total_assets().await.unwrap());
        println!("Total Supply: {}", client.get_total_supply().await.unwrap());
        println!("Idle Balance: {}", client.get_idle_balance().await.unwrap());
        println!("Max Deposit: {}", client.get_max_deposit().await.unwrap());
    }

    #[rstest]
    #[tokio::test(flavor = "current_thread")]
    async fn redeem_happy_path_smoke(
        vault: AccountId,
        everything: AccountId,
        testnet_rpc: String,
        sk: SecretKey,
    ) {
        let client =
            Client::new_client(testnet_rpc, &everything, &sk.to_string(), &vault, 5).unwrap();
        let receiver = AccountId("topgunbakugo.testnet".to_string());
        client.redeem(&"1".to_string(), &receiver).await.unwrap();
    }

    #[rstest]
    #[tokio::test(flavor = "current_thread")]
    async fn execute_withdrawal_happy_path_smoke(
        vault: AccountId,
        everything: AccountId,
        testnet_rpc: String,
        sk: SecretKey,
    ) {
        let client =
            Client::new_client(testnet_rpc, &everything, &sk.to_string(), &vault, 5).unwrap();
        let route = vec![
            AccountId("market1.testnet".to_string()),
            AccountId("vault.testnet".to_string()),
        ];
        client.cancel_in_flight_withdrawal().await.unwrap();
        println!(
            "{:?}",
            client
                .execute_withdrawal(&route, Some(100.to_string()))
                .await
                .unwrap()
        );
    }

    #[test]
    fn governance_u128_serialization_happy_path() {
        let input = "\"12345\"".to_string();
        let u: U128 = serde_json::from_str(&input).unwrap();
        assert_eq!(u.0, 12345);
    }

    #[test]
    fn governance_u64_wrapper_happy_path() {
        let u = near_sdk::json_types::U64::from(42u64);
        assert_eq!(u.0, 42u64);
    }

    #[test]
    fn deposit_string_parse_happy_path() {
        let s = "1000000".to_string();
        let v: u128 = s.parse().unwrap();
        assert_eq!(v, 1_000_000u128);
    }

    // The following async tests are ignored by default as they require network and permissions.
    #[rstest]
    #[tokio::test(flavor = "current_thread")]
    async fn governance_smoke_calls_ignored(
        vault: AccountId,
        everything: AccountId,
        testnet_rpc: String,
        sk: SecretKey,
    ) {
        let client =
            Client::new_client(testnet_rpc, &everything, &sk.to_string(), &vault, 5).unwrap();

        println!("{:?}", client.accept_guardian().await.unwrap());
        println!("{:?}", client.revoke_pending_guardian().await.unwrap());

        println!("{:?}", client.submit_timelock(0).await.unwrap());
        println!("{:?}", client.accept_timelock().await.unwrap());
        println!("{:?}", client.revoke_pending_timelock().await.unwrap());
    }

    #[rstest]
    #[tokio::test(flavor = "current_thread")]
    async fn governance_set_supply_queue_ignored(
        vault: AccountId,
        everything: AccountId,
        testnet_rpc: String,
        sk: SecretKey,
    ) {
        let client =
            Client::new_client(testnet_rpc, &everything, &sk.to_string(), &vault, 5).unwrap();
        let markets: Vec<AccountId> = vec!["gh-65.templar-in-training.testnet".to_string().into()];

        let x = serde_json::to_string_pretty(&U128(u128::MAX)).unwrap();
        println!("{:?}", x);
        println!(
            "{:?}",
            client
                .set_supply_queue(&markets, &"0".to_string())
                .await
                .unwrap()
        );
    }

    #[rstest]
    #[tokio::test(flavor = "current_thread")]
    async fn full_happy(
        vault: AccountId,
        everything: AccountId,
        testnet_rpc: String,
        sk: SecretKey,
    ) {
        let client =
            Client::new_client(testnet_rpc, &everything, &sk.to_string(), &vault, 5).unwrap();
        let markets: Vec<AccountId> = vec!["usdt.registry.topgunbakugo.testnet".to_string().into()];

        let mkt = &markets[0];
        println!("market: {:?}", mkt);

        client
            .submit_cap(
                mkt,
                &serde_json::to_string_pretty(&U128(u128::MAX)).unwrap(),
            )
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        client.accept_cap(mkt).await.unwrap();

        let deposit = NearToken::from_near(1).as_yoctonear();

        println!("deposit: {:?}", deposit);

        println!(
            "{:?}",
            client
                .set_supply_queue(&markets, &deposit.to_string())
                .await
                .unwrap()
        );

        let delta = AllocationDelta::Supply(Delta {
            market: mkt.clone(),
            amount: 100.to_string(),
        });
        println!("{:?}", delta);
        println!("{:?}", client.reallocate(&delta).await.unwrap());
    }

    #[rstest]
    #[tokio::test(flavor = "current_thread")]
    async fn reallocate(
        vault: AccountId,
        everything: AccountId,
        testnet_rpc: String,
        sk: SecretKey,
    ) {
        let client =
            Client::new_client(testnet_rpc, &everything, &sk.to_string(), &vault, 5).unwrap();
        let markets: Vec<AccountId> = vec!["usdt.registry.topgunbakugo.testnet".to_string().into()];

        let mkt = &markets[0];
        println!("market: {:?}", mkt);

        let delta = AllocationDelta::Supply(Delta {
            market: mkt.clone(),
            amount: 100.to_string(),
        });
        println!("{:?}", delta);
        println!("{:?}", client.reallocate(&delta).await.unwrap());

        let delta = AllocationDelta::Withdraw(Delta {
            market: mkt.clone(),
            amount: 100.to_string(),
        });
        println!("{:?}", delta);
        println!("{:?}", client.reallocate(&delta).await.unwrap());

        // let (op_id, market_index) = client
        //     .execute_withdrawal(&vec![mkt.clone()], Some(100.to_string()))
        //     .await
        //     .unwrap();

        // client
        //     .execute_market_withdrawal(&op_id, &market_index, None)
        //     .await
        //     .unwrap();
    }
}
