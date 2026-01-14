#![allow(clippy::unwrap_used)]

use clap::Parser;
use near_crypto::SecretKey;
use near_jsonrpc_client::methods;
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::{
    action::{
        delegate::{DelegateAction, SignedDelegateAction},
        Action, FunctionCallAction,
    },
    types::Finality,
};
use near_sdk::{base64::prelude::*, serde_json, AccountId, NearToken};

#[derive(Parser, Debug, Clone)]
struct Args {
    #[arg(
        long = "rpc",
        env = "RPC_URL",
        default_value = "https://test.rpc.fastnear.com/"
    )]
    pub rpc_url: String,
    #[arg(short, long)]
    pub account_id: AccountId,
    #[arg(short, long)]
    pub secret_key: SecretKey,
    #[arg(short, long)]
    pub receiver_id: AccountId,
    #[arg(short, long)]
    pub method_name: String,
    #[allow(clippy::struct_field_names)]
    #[arg(long, default_value = "{}")]
    pub args: String,
    #[arg(long, default_value_t = 50)]
    pub tgas: u64,
    #[arg(long, short, default_value_t = NearToken::ZERO)]
    pub deposit: NearToken,
}

#[tokio::main]
pub async fn main() {
    let args = Args::parse();

    let near = near_jsonrpc_client::JsonRpcClient::connect(args.rpc_url);

    let call_args = serde_json::from_str::<serde_json::Value>(&args.args)
        .ok()
        .and_then(|json| serde_json::to_vec(&json).ok())
        .or_else(|| BASE64_STANDARD.decode(&args.args).ok())
        .unwrap();

    eprintln!("Fetching nonce");

    let result = near
        .call(methods::query::RpcQueryRequest {
            block_reference: Finality::Final.into(),
            request: near_primitives::views::QueryRequest::ViewAccessKey {
                account_id: args.account_id.clone(),
                public_key: args.secret_key.public_key(),
            },
        })
        .await
        .unwrap();

    let QueryResponseKind::AccessKey(access_key) = result.kind else {
        unimplemented!()
    };

    eprintln!("Constructing delegate action");

    let delegate_action = DelegateAction {
        sender_id: args.account_id.clone(),
        receiver_id: args.receiver_id.clone(),
        actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: args.method_name,
            args: call_args,
            gas: near_primitives::gas::Gas::from_teragas(args.tgas),
            deposit: args.deposit,
        }))
        .try_into()
        .unwrap()],
        nonce: access_key.nonce + 1,
        max_block_height: result.block_height + 60 * 60,
        public_key: args.secret_key.public_key(),
    };

    let signature = args.secret_key.sign(&delegate_action.get_nep461_hash().0);

    let sda = SignedDelegateAction {
        delegate_action,
        signature,
    };

    let serialized = near_sdk::borsh::to_vec(&sda).unwrap();

    let base64_encoded = BASE64_STANDARD.encode(&serialized);

    eprintln!("Base64-encoded signed delegate action:");
    println!("{base64_encoded}");
}
