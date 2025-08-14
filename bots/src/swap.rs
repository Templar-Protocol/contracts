use std::sync::Arc;

use clap::ValueEnum;
use near_crypto::InMemorySigner;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    action::{Action, FunctionCallAction},
    transaction::{Transaction, TransactionV0},
    views::FinalExecutionStatus,
};
use near_sdk::{json_types::U128, near, serde_json, serde_json::json, AccountId, NearToken};

use crate::types::FungibleAssetKind;
use crate::{
    near::{get_access_key_data, send_tx, serialize_and_encode, view, RpcResult},
    Network, DEFAULT_GAS,
};

#[async_trait::async_trait]
pub trait Swap {
    /// Quotes the amount of `from` token to `to` token.
    async fn quote(
        &self,
        from: &FungibleAssetKind,
        to: &FungibleAssetKind,
        amount: U128,
    ) -> RpcResult<U128>;

    /// Swaps `from` token to `to` token.
    async fn swap(
        &self,
        from: &FungibleAssetKind,
        to: &FungibleAssetKind,
        amount: U128,
    ) -> RpcResult<FinalExecutionStatus>;
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SwapType {
    RheaSwap,
}

impl SwapType {
    #[must_use]
    #[allow(
        clippy::unwrap_used,
        reason = "We know the contract IDs are valid NEAR account IDs."
    )]
    pub fn account_id(self, network: Network) -> AccountId {
        match self {
            SwapType::RheaSwap => match network {
                Network::Mainnet => "dclv2.ref-labs.near".parse().unwrap(),
                Network::Testnet => "dclv2.ref-dev.testnet".parse().unwrap(),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct RheaSwap {
    pub contract: AccountId,
    pub client: JsonRpcClient,
    pub signer: Arc<InMemorySigner>,
}

impl RheaSwap {
    pub fn new(contract: AccountId, client: JsonRpcClient, signer: Arc<InMemorySigner>) -> Self {
        Self {
            contract,
            client,
            signer,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
struct QuoteRequest {
    pool_ids: Vec<String>,
    input_token: AccountId,
    output_token: AccountId,
    output_amount: U128,
    tag: String,
}

impl QuoteRequest {
    pub fn new(
        input_token: &FungibleAssetKind,
        output_token: &FungibleAssetKind,
        output_amount: U128,
    ) -> Self {
        let input_contract = input_token.account_id();
        let output_contract = output_token.account_id();

        Self {
            pool_ids: vec![format!("{}|{}|100", input_contract, output_contract)],
            tag: format!("{}|100|{}", input_contract, output_amount.0),
            input_token: input_contract.clone(),
            output_token: output_contract.clone(),
            output_amount,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
struct QuoteResponse {
    amount: U128,
    tag: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
enum SwapRequestMsg {
    SwapByOutput {
        pool_ids: Vec<String>,
        output_token: AccountId,
        output_amount: U128,
        client_id: String,
    },
}

impl SwapRequestMsg {
    pub fn new(
        input_token: &FungibleAssetKind,
        output_token: &FungibleAssetKind,
        output_amount: U128,
    ) -> Self {
        let input_contract = input_token.account_id();
        let output_contract = output_token.account_id();

        Self::SwapByOutput {
            pool_ids: vec![format!("{}|{}|100", input_contract, output_contract)],
            output_token: output_contract.clone(),
            output_amount,
            client_id: format!("{}|100|{}", input_contract, output_amount.0),
        }
    }
}

#[async_trait::async_trait]
impl Swap for RheaSwap {
    async fn quote(
        &self,
        from: &FungibleAssetKind,
        to: &FungibleAssetKind,
        amount: U128,
    ) -> RpcResult<U128> {
        let response: QuoteResponse = view(
            &self.client,
            self.contract.clone(),
            "quote_by_output",
            &QuoteRequest::new(from, to, amount),
        )
        .await?;
        Ok(response.amount)
    }

    async fn swap(
        &self,
        from: &FungibleAssetKind,
        to: &FungibleAssetKind, // Fixed: now takes FungibleAssetKind
        amount: U128,
    ) -> RpcResult<FinalExecutionStatus> {
        let msg = SwapRequestMsg::new(from, to, amount);

        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

        // Need to determine which contract to call for the transfer
        // For swaps, we typically transfer the input token to the swap contract
        let (receiver_id, method_name, transfer_args) = match from {
            FungibleAssetKind::Nep141(account_id) => (
                account_id.clone(),
                "ft_transfer_call".to_string(),
                json!({
                    "receiver_id": self.contract,
                    "amount": amount,
                    "msg": serde_json::to_string(&msg)?,
                }),
            ),
            FungibleAssetKind::Nep245 {
                account_id,
                token_id,
            } => (
                account_id.clone(),
                "mt_transfer_call".to_string(),
                json!({
                    "receiver_id": self.contract,
                    "token_id": token_id,
                    "amount": amount,
                    "msg": serde_json::to_string(&msg)?,
                }),
            ),
        };

        let tx = Transaction::V0(TransactionV0 {
            nonce,
            receiver_id,
            block_hash,
            signer_id: self.signer.account_id.clone(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name,
                args: serialize_and_encode(transfer_args),
                gas: DEFAULT_GAS,
                deposit: NearToken::from_yoctonear(1).as_yoctonear(),
            }))],
        });

        send_tx(&self.client, &self.signer, 10, tx).await
    }
}
