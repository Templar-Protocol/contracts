use std::sync::Arc;

use crate::{
    near::{get_access_key_data, send_tx, serialize_and_encode, view, RpcResult},
    Network, DEFAULT_GAS,
};
use clap::ValueEnum;
use near_crypto::InMemorySigner;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    action::{Action, FunctionCallAction},
    transaction::{Transaction, TransactionV0},
    views::FinalExecutionStatus,
};
use near_sdk::{json_types::U128, near, serde_json, AccountId, NearToken};
use templar_common::asset::{AssetClass, FungibleAsset};

#[async_trait::async_trait]
pub trait Swap {
    /// Quotes the amount of `from` token to `to` token.
    async fn quote<A: AssetClass, B: AssetClass>(
        &self,
        from: FungibleAsset<A>,
        to: FungibleAsset<B>,
        amount: U128,
    ) -> RpcResult<U128>;

    /// Swaps `from` token to `to` token.
    async fn swap<A: AssetClass, B: AssetClass>(
        &self,
        from: FungibleAsset<A>,
        to: FungibleAsset<B>,
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
    pub fn new(input_token: AccountId, output_token: AccountId, output_amount: U128) -> Self {
        Self {
            pool_ids: vec![format!("{}|{}|100", input_token, output_token)],
            tag: format!("{}|100|{}", input_token, output_amount.0),
            input_token,
            output_token,
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
    pub fn new(input_token: &AccountId, output_token: AccountId, output_amount: U128) -> Self {
        Self::SwapByOutput {
            pool_ids: vec![format!("{}|{}|100", input_token, output_token)],
            output_token,
            output_amount,
            client_id: format!("{}|100|{}", input_token, output_amount.0),
        }
    }
}

#[async_trait::async_trait]
#[allow(
    clippy::expect_used,
    reason = "Rhea was mostly implemented for testing purposes, and don't expect it to be used in production."
)]
impl Swap for RheaSwap {
    async fn quote<A: AssetClass, B: AssetClass>(
        &self,
        from: FungibleAsset<A>,
        to: FungibleAsset<B>,
        amount: U128,
    ) -> RpcResult<U128> {
        let response: QuoteResponse = view(
            &self.client,
            self.contract.clone(),
            "quote_by_output",
            &QuoteRequest::new(
                from.into_nep141()
                    .expect("MT not yet supported on Rhea `from` assets"),
                to.into_nep141()
                    .expect("MT not yet supported on Rhea `to` assets"),
                amount,
            ),
        )
        .await?;
        Ok(response.amount)
    }

    async fn swap<A: AssetClass, B: AssetClass>(
        &self,
        from: FungibleAsset<A>,
        to: FungibleAsset<B>,
        amount: U128,
    ) -> RpcResult<FinalExecutionStatus> {
        let msg = SwapRequestMsg::new(
            &from
                .clone()
                .into_nep141()
                .expect("MT not yet supported on Rhea `from` assets"),
            to.into_nep141()
                .expect("MT not yet supported on Rhea `to` assets"),
            amount,
        );

        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

        let transfer_call_params =
            from.transfer_call_params(&self.contract, amount, &serde_json::to_string(&msg)?);
        let tx = Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: transfer_call_params.account_id,
            block_hash,
            signer_id: self.signer.account_id.clone(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: transfer_call_params.method_name,
                args: serialize_and_encode(transfer_call_params.args),
                gas: DEFAULT_GAS,
                deposit: NearToken::from_yoctonear(1).as_yoctonear(),
            }))],
        });

        send_tx(&self.client, &self.signer, 10, tx).await
    }
}
