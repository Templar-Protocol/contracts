use std::sync::Arc;

use clap::ValueEnum;
use near_crypto::InMemorySigner;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    action::{Action, FunctionCallAction},
    transaction::{Transaction, TransactionV0},
    views::FinalExecutionStatus,
};
use near_sdk::{json_types::U128, near, AccountId, NearToken, serde_json};

use crate::{
    near::{get_access_key_data, send_tx, serialize_and_encode, view, RpcResult},
    Network, DEFAULT_GAS,
};

use crate::liquidator::AssetSpec;

#[async_trait::async_trait]
pub trait Swap {
    /// Quotes the amount needed to swap from `from_asset` to obtain `output_amount` of `to_asset`.
    async fn quote(
        &self,
        from_asset: &AssetSpec,
        to_asset: &AssetSpec,
        output_amount: U128,
    ) -> RpcResult<U128>;

    /// Swaps `amount` of `from_asset` to `to_asset`.
    async fn swap(
        &self,
        from_asset: &AssetSpec,
        to_asset: &AssetSpec,
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
        from_asset: &AssetSpec,
        to_asset: &AssetSpec,
        output_amount: U128,
    ) -> Self {
        let input_token = from_asset.contract_id().clone();
        let output_token = to_asset.contract_id().clone();
        
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
    pub fn new(
        from_asset: &AssetSpec,
        to_asset: &AssetSpec,
        output_amount: U128,
    ) -> Self {
        let input_token = from_asset.contract_id().clone();
        let output_token = to_asset.contract_id().clone();
        
        Self::SwapByOutput {
            pool_ids: vec![format!("{}|{}|100", input_token, output_token)],
            output_token,
            output_amount,
            client_id: format!("{}|100|{}", input_token, output_amount.0),
        }
    }
}

#[async_trait::async_trait]
impl Swap for RheaSwap {
    async fn quote(
        &self,
        from_asset: &AssetSpec,
        to_asset: &AssetSpec,
        output_amount: U128,
    ) -> RpcResult<U128> {
        // TODO: For now, Rhea only supports NEP-141 tokens
        if !from_asset.is_nep141() || !to_asset.is_nep141() {
            return Err(crate::near::RpcError::ValidationError(
                "RheaSwap currently only supports NEP-141 tokens".to_string(),
            ));
        }

        let response: QuoteResponse = view(
            &self.client,
            self.contract.clone(),
            "quote_by_output",
            &QuoteRequest::new(from_asset, to_asset, output_amount),
        )
        .await?;
        Ok(response.amount)
    }

    async fn swap(
        &self,
        from_asset: &AssetSpec,
        to_asset: &AssetSpec,
        amount: U128,
    ) -> RpcResult<FinalExecutionStatus> {
        // TODO: For now, Rhea only supports NEP-141 tokens
        if !from_asset.is_nep141() || !to_asset.is_nep141() {
            return Err(crate::near::RpcError::ValidationError(
                "RheaSwap currently only supports NEP-141 tokens".to_string(),
            ));
        }

        let msg = SwapRequestMsg::new(from_asset, to_asset, amount);
        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

        let msg_string = serde_json::to_string(&msg)
            .map_err(|e| crate::near::RpcError::SerializationError(
                format!("Failed to serialize swap message: {e}")
            ))?;

        let tx = Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: from_asset.contract_id().clone(),
            block_hash,
            signer_id: self.signer.account_id.clone(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: from_asset.transfer_method().to_string(),
                args: serialize_and_encode(from_asset.transfer_args(
                    &self.contract,
                    amount,
                    Some(&msg_string),
                )),
                gas: DEFAULT_GAS,
                deposit: NearToken::from_yoctonear(1).as_yoctonear(),
            }))],
        });

        send_tx(&self.client, &self.signer, 10, tx).await
    }
}
