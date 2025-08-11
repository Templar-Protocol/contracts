use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use near_crypto::InMemorySigner;
use near_jsonrpc_client::JsonRpcClient;
use near_sdk::{
    json_types::U128,
    near,
    serde_json::{self, Value},
    AccountId,
};

use crate::{
    near::{get_access_key_data, RpcError, RpcResult},
    Network,
};

use super::{QuoteOutput, Swap};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct IntentsQuoteRequest {
    pub id: String,
    pub jsonrpc: String,
    pub method: String,
    pub params: Vec<IntentsQuoteParams>,
}

impl IntentsQuoteRequest {
    pub fn new(id: String, params: IntentsQuoteParams) -> Self {
        IntentsQuoteRequest {
            id,
            jsonrpc: "2.0".to_string(),
            method: "quote".to_string(),
            params: vec![params],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct IntentsQuoteParams {
    pub defuse_asset_identifier_in: AccountId,
    pub defuse_asset_identifier_out: AccountId,
    pub exact_amount_in: U128,
    pub min_deadline_ms: u64,
    pub wait_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct IntentsQuoteResponse {
    pub defuse_asset_identifier_in: AccountId,
    pub defuse_asset_identifier_out: AccountId,
    pub amount_in: U128,
    pub amount_out: U128,
    pub expiration_date: u64,
    pub quote_hash: String,
}

impl QuoteOutput for IntentsQuoteResponse {
    fn to_u128(&self) -> U128 {
        self.amount_out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct PublishIntentsRequest {
    pub id: String,
    pub jsonrpc: String,
    pub method: String,
    pub params: Vec<PublishIntentsParams>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct PublishIntentsParams {
    // depending on the method publish_intent or publish_intents argument is an array or single
    // object. Not sure if there is any overhead in using a multiple type here.
    // signed_datas: SignedData,
    pub signed_datas: Vec<SignedData>,
    pub quote_hashes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct SignedData {
    pub standard: String,
    pub payload: SwapMessage,
    pub public_key: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct SwapMessage {
    pub message: String,
    pub nonce: u64,
    pub recipient: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct IntentMessage {
    deadline: u128,
    intents: Intents,
    signer_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Intents {
    intent: String,
    diff: HashMap<String, String>,
    referral: Option<String>,
    memo: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IntentsSwap {
    pub solver_url: String,
    pub rpc_client: JsonRpcClient,
    pub http_client: reqwest::Client,
    pub signer: Arc<InMemorySigner>,
}

impl IntentsSwap {
    pub fn new(network: Network, rpc_client: JsonRpcClient, signer: Arc<InMemorySigner>) -> Self {
        Self {
            solver_url: match network {
                Network::Mainnet => "https://solver-relay-v2.chaindefuser.com/rpc".to_string(),
                Network::Testnet => panic!("IntentsSwap is not supported on Testnet"),
            },
            rpc_client,
            http_client: reqwest::Client::new(),
            signer,
        }
    }
}

#[async_trait::async_trait]
impl Swap for IntentsSwap {
    type QuoteOutput = IntentsQuoteResponse;
    type SwapOutput = Value;

    async fn quote(
        &self,
        from: &AccountId,
        to: &AccountId,
        amount: U128,
    ) -> RpcResult<Self::QuoteOutput> {
        let id = format!("quote-{from}-{to}-{amount:?}");
        let quote_request = IntentsQuoteRequest::new(
            id,
            IntentsQuoteParams {
                defuse_asset_identifier_in: from.clone(),
                defuse_asset_identifier_out: to.clone(),
                exact_amount_in: amount,
                min_deadline_ms: 60000,
                wait_ms: 500,
            },
        );

        let quote_response: IntentsQuoteResponse = self
            .http_client
            .post(self.solver_url.clone())
            .json(&quote_request)
            .send()
            .await
            .map_err(RpcError::SolverRequestError)?
            .json()
            .await
            .map_err(RpcError::SolverResponseDeserialization)?;

        if quote_response.quote_hash.is_empty() {
            Err(RpcError::NoQuoteHashReceived)
        } else {
            Ok(quote_response)
        }
    }

    async fn swap(
        &self,
        from: &AccountId,
        to: &AccountId,
        amount: U128,
    ) -> RpcResult<Self::SwapOutput> {
        let (nonce, _block_hash) = get_access_key_data(&self.rpc_client, &self.signer).await?;
        let quote = self.quote(from, to, amount).await?;

        let intent_message =
            build_intent_message(&quote, self.signer.account_id.clone(), None, None)?;

        let publish_request =
            make_publish_request(&self.signer, &intent_message, nonce, vec![quote.quote_hash])?;

        let publish_response = self
            .http_client
            .post(self.solver_url.clone())
            .json(&publish_request)
            .send()
            .await
            .map_err(RpcError::SolverRequestError)?
            .json()
            .await
            .map_err(RpcError::SolverResponseDeserialization)?;

        Ok(publish_response)
    }
}

fn build_intent_message(
    quote: &IntentsQuoteResponse,
    signer_id: AccountId,
    referral: Option<String>,
    memo: Option<String>,
) -> RpcResult<IntentMessage> {
    let mut token_diff_num: HashMap<String, i128> = HashMap::new();

    // Subtract input token (user gives this)
    let token_in = format!("nep141:{}", quote.defuse_asset_identifier_in);
    let amount_in: i128 = quote.amount_in.0.try_into().unwrap_or(0);
    *token_diff_num.entry(token_in).or_insert(0) -= amount_in;

    // Add output token (user receives this)
    let token_out = format!("nep141:{}", quote.defuse_asset_identifier_out);
    let amount_out: i128 = quote.amount_out.0.try_into().unwrap_or(0);
    *token_diff_num.entry(token_out).or_insert(0) += amount_out;

    let token_diff: HashMap<String, String> = token_diff_num
        .into_iter()
        .map(|(token, amount)| (token, amount.to_string()))
        .collect();

    let intents = Intents {
        intent: "token_diff".to_string(),
        diff: token_diff,
        referral,
        memo,
    };

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
    let deadline = now.as_millis() + 60_000;

    Ok(IntentMessage {
        deadline,
        intents,
        signer_id,
    })
}

fn make_publish_request(
    signer: &InMemorySigner,
    intent_message: &IntentMessage,
    nonce: u64,
    quote_hashes: Vec<String>,
) -> RpcResult<PublishIntentsRequest> {
    let message_str = serde_json::to_string(intent_message)?;

    let payload = SwapMessage {
        message: message_str,
        nonce,
        recipient: "intents.near".to_string(),
    };

    let payload_json = serde_json::to_string(&payload)?;

    let signed_data = SignedData {
        standard: "nep413".to_string(),
        payload,
        public_key: signer.public_key().to_string(),
        signature: signer.sign(payload_json.as_bytes()).to_string(),
    };

    Ok(PublishIntentsRequest {
        id: "dontcare".to_string(),
        jsonrpc: "2.0".to_string(),
        method: "publish_intents".to_string(),
        params: vec![PublishIntentsParams {
            signed_datas: vec![signed_data],
            quote_hashes,
        }],
    })
}
