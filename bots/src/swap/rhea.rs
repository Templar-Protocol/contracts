use std::sync::Arc;

use near_crypto::InMemorySigner;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    action::{Action, FunctionCallAction},
    transaction::{Transaction, TransactionV0},
    views::FinalExecutionStatus,
};
use near_sdk::{json_types::U128, near, serde_json::json, AccountId, NearToken};

use crate::{
    near::{get_access_key_data, send_tx, serialize_and_encode, view, RpcResult},
    Network, DEFAULT_GAS,
};

use super::{QuoteOutput, Swap};

#[derive(Debug, Clone)]
pub struct RheaSwap {
    pub contract: AccountId,
    pub client: JsonRpcClient,
    pub signer: Arc<InMemorySigner>,
}

impl RheaSwap {
    #[allow(
        clippy::unwrap_used,
        reason = "We know the contract IDs are valid NEAR account IDs."
    )]
    pub fn new(network: Network, client: JsonRpcClient, signer: Arc<InMemorySigner>) -> Self {
        Self {
            contract: match network {
                Network::Mainnet => "dclv2.ref-labs.near".parse().unwrap(),
                Network::Testnet => "dclv2.ref-dev.testnet".parse().unwrap(),
            },
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
pub struct QuoteResponse {
    amount: U128,
    tag: String,
}

impl QuoteOutput for QuoteResponse {
    fn to_u128(&self) -> U128 {
        self.amount
    }
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
        pool_ids: Vec<String>,
        output_token: AccountId,
        output_amount: U128,
        client_id: String,
    ) -> Self {
        Self::SwapByOutput {
            pool_ids,
            output_token,
            output_amount,
            client_id,
        }
    }
}

#[async_trait::async_trait]
impl Swap for RheaSwap {
    type QuoteOutput = QuoteResponse;
    type SwapOutput = FinalExecutionStatus;

    async fn quote(
        &self,
        from: &AccountId,
        to: &AccountId,
        amount: U128,
    ) -> RpcResult<Self::QuoteOutput> {
        let response: QuoteResponse = view(
            &self.client,
            self.contract.clone(),
            "quote_by_output",
            &QuoteRequest::new(from.clone(), to.clone(), amount),
        )
        .await?;
        Ok(response)
    }

    async fn swap(
        &self,
        from: &AccountId,
        to: &AccountId,
        amount: U128,
    ) -> RpcResult<Self::SwapOutput> {
        let msg = SwapRequestMsg::new(
            vec![format!("{}|{}|100", from, to)],
            to.clone(),
            amount,
            format!("{}|100|{}", from, amount.0),
        );

        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

        let tx = Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: to.clone(),
            block_hash,
            signer_id: self.signer.account_id.clone(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: "ft_transfer_call".to_string(),
                args: serialize_and_encode(json!({
                    "receiver_id": self.contract,
                    "amount": amount,
                    "msg": msg,
                })),
                gas: DEFAULT_GAS,
                deposit: NearToken::from_yoctonear(1).as_yoctonear(),
            }))],
        });

        send_tx(&self.client, &self.signer, 10, tx).await
    }
}
