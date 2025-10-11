use std::sync::Arc;

use clap::ValueEnum;
use near_crypto::Signer;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    action::Action,
    transaction::{Transaction, TransactionV0},
    views::FinalExecutionStatus,
};
use near_sdk::{json_types::U128, near, serde_json, AccountId};

use crate::{
    near::{get_access_key_data, send_tx, view, AppError, AppResult},
    Network,
};
use templar_common::asset::{AssetClass, FungibleAsset};

#[async_trait::async_trait]
pub trait Swap {
    /// Quotes the amount needed to swap from `from_asset` to obtain `output_amount` of `to_asset`.
    async fn quote<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        output_amount: U128,
    ) -> AppResult<U128>;

    /// Swaps `amount` of `from_asset` to `to_asset`.
    async fn swap<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        amount: U128,
    ) -> AppResult<FinalExecutionStatus>;
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
    pub signer: Arc<Signer>,
}

impl RheaSwap {
    pub fn new(contract: AccountId, client: JsonRpcClient, signer: Arc<Signer>) -> Self {
        Self {
            contract,
            client,
            signer,
        }
    }

    /// Default fee tier for `RheaSwap` DCL pools (0.2%)
    const DEFAULT_FEE: u32 = 2000;
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
struct QuoteRequest {
    pool_ids: Vec<String>,
    input_token: AccountId,
    output_token: AccountId,
    output_amount: U128,
    tag: Option<String>,
}

impl QuoteRequest {
    pub fn new<F: AssetClass, T: AssetClass>(
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        output_amount: U128,
        fee: u32,
    ) -> Self {
        let input_token: AccountId = from_asset.contract_id().into();
        let output_token: AccountId = to_asset.contract_id().into();

        // Create pool ID in the format: input_token|output_token|fee
        let pool_id = format!("{input_token}|{output_token}|{fee}");

        Self {
            pool_ids: vec![pool_id],
            tag: None,
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
    tag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
enum SwapRequestMsg {
    SwapByOutput {
        pool_ids: Vec<String>,
        output_token: AccountId,
        output_amount: U128,
    },
}

impl SwapRequestMsg {
    pub fn new<F: AssetClass, T: AssetClass>(
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        output_amount: U128,
        fee: u32,
    ) -> Self {
        let input_token: AccountId = from_asset.contract_id().into();
        let output_token: AccountId = to_asset.contract_id().into();

        // Create pool ID in the format: input_token|output_token|fee
        let pool_id = format!("{input_token}|{output_token}|{fee}");

        Self::SwapByOutput {
            pool_ids: vec![pool_id],
            output_token,
            output_amount,
        }
    }
}

#[async_trait::async_trait]
impl Swap for RheaSwap {
    async fn quote<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        output_amount: U128,
    ) -> AppResult<U128> {
        // TODO: For now, Rhea only supports NEP-141 tokens
        if from_asset.clone().into_nep141().is_none() || to_asset.clone().into_nep141().is_none() {
            return Err(AppError::ValidationError(
                "RheaSwap currently only supports NEP-141 tokens".to_string(),
            ));
        }

        let response: QuoteResponse = view(
            &self.client,
            self.contract.clone(),
            "quote_by_output",
            &QuoteRequest::new(from_asset, to_asset, output_amount, Self::DEFAULT_FEE),
        )
        .await?;
        Ok(response.amount)
    }

    async fn swap<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        amount: U128,
    ) -> AppResult<FinalExecutionStatus> {
        // TODO: For now, Rhea only supports NEP-141 tokens
        if from_asset.clone().into_nep141().is_none() || to_asset.clone().into_nep141().is_none() {
            return Err(AppError::ValidationError(
                "RheaSwap currently only supports NEP-141 tokens".to_string(),
            ));
        }

        let msg = SwapRequestMsg::new(from_asset, to_asset, amount, Self::DEFAULT_FEE);
        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

        let msg_string = serde_json::to_string(&msg).map_err(|e| {
            AppError::SerializationError(format!("Failed to serialize swap message: {e}"))
        })?;

        let tx = Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: from_asset.contract_id().into(),
            block_hash,
            signer_id: self.signer.get_account_id(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(
                from_asset.transfer_call_action(&self.contract, amount.into(), &msg_string),
            ))],
        });

        send_tx(&self.client, &self.signer, 10, tx)
            .await
            .map_err(AppError::from)
    }
}
