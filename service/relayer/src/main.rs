#![allow(clippy::unwrap_used)]

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
};

use axum::{extract::State, http::StatusCode, routing, Json, Router};
use clap::Parser;
use near_crypto::SecretKey;
use near_fetch::signer::KeyRotatingSigner;
use near_primitives::{
    action::{delegate::SignedDelegateAction, Action},
    types::{AccountId, Gas},
};
use near_sdk::serde::Deserialize;
use near_sdk::serde_json;
use tokio::{sync::RwLock, task::JoinSet};

use templar_common::{
    asset::{BorrowAsset, FungibleAsset},
    market::DepositMsg,
};
use templar_relayer::{
    client::NearClient,
    message::{RelayRequest, RelayResponse},
    Configuration, FtTransferCallArgs, MarketAccounts, MtTransferCallArgs, TransferCallArgs,
};
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser, Debug, Clone)]
struct Args {
    /// Run the relayer on this port.
    #[arg(short, long, env = "PORT", default_value_t = 3000)]
    pub port: u16,
    /// Postgres database connection URL.
    #[arg(long, env = "DATABASE_URL", default_value = "DELETEME")]
    pub database_url: String,
    /// NEAR RPC connection URL.
    #[arg(long, env = "RPC_URL", default_value = "https://rpc.testnet.near.org")]
    pub rpc_url: String,
    /// Path to YAML configuration file.
    #[arg(short, long, env = "CONFIG", default_value = "./config.yaml")]
    pub config: PathBuf,
    /// Comma-separated list of registries to query for markets to monitor.
    #[arg(long, env = "REGISTRY", default_value = "[]")]
    pub registry: Vec<AccountId>,
    /// Comma-separated list of markets to monitor.
    #[arg(long, env = "MARKET")]
    pub market: Vec<AccountId>,
    /// Account ID of the NEAR account that the relayer controls.
    #[arg(short, long, env = "ACCOUNT_ID")]
    pub account_id: AccountId,
    /// Comma-separated list of private keys to use to sign transactions for the account that the relayer controls.
    #[arg(short = 'k', long, env = "SECRET_KEY")]
    pub secret_key: Vec<SecretKey>,
}

#[derive(Debug, Clone, Default)]
struct AccountData {
    pub market_account_ids: HashMap<AccountId, MarketAccounts>,
    pub allowed_receiver_account_ids: HashSet<AccountId>,
}

#[derive(Debug, Clone)]
struct App {
    pub args: Args,
    pub configuration: Configuration,
    pub accounts: Arc<RwLock<AccountData>>,
    pub near_client: NearClient,
}

impl App {
    pub fn new(args: Args, configuration: Configuration) -> Self {
        let near_client = NearClient::new(
            near_jsonrpc_client::JsonRpcClient::connect(&args.rpc_url),
            KeyRotatingSigner::try_from_iter(
                args.secret_key
                    .iter()
                    .map(|sk| (args.account_id.clone(), sk.clone())),
            )
            .unwrap(),
        );

        Self {
            args,
            configuration,
            accounts: Arc::new(RwLock::new(AccountData::default())),
            near_client,
        }
    }

    pub async fn load_markets(&mut self) {
        let mut markets = self.args.market.clone();

        // Load markets from registry...
        let mut set = JoinSet::new();
        for registry_id in &self.args.registry {
            set.spawn({
                let near_client = self.near_client.clone();
                let registry_id = registry_id.clone();
                async move {
                    near_client
                        .load_deployments_from_registry(&registry_id)
                        .await
                }
            });
        }
        markets.extend(set.join_all().await.into_iter().flatten());

        // ...and add any individual markets.
        let mut set = JoinSet::new();
        for market in markets {
            set.spawn({
                let near_client = self.near_client.clone();
                async move { near_client.load_market_accounts(&market).await }
            });
        }
        let market_accounts_vec = set.join_all().await;

        let mut market_account_ids = HashMap::new();
        let mut allowed_receiver_account_ids = HashSet::new();

        for market_accounts in market_accounts_vec.into_iter().flatten() {
            let market_id = market_accounts.account_id.clone();

            info!(
                "Loaded market {market_id} with borrow asset {} and collateral asset {}",
                market_accounts.borrow_asset, market_accounts.collateral_asset,
            );

            allowed_receiver_account_ids.insert(market_id);
            allowed_receiver_account_ids
                .insert(market_accounts.borrow_asset.contract_id().to_owned());
            allowed_receiver_account_ids
                .insert(market_accounts.collateral_asset.contract_id().to_owned());

            market_account_ids.insert(market_accounts.account_id.clone(), market_accounts);
        }

        let mut handle = self.accounts.write().await;
        handle.market_account_ids = market_account_ids;
        handle.allowed_receiver_account_ids = allowed_receiver_account_ids;
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Failed precondition: ")]
pub enum PreconditionError {
    #[error("Failed signature verification")]
    SignatureVerificationFailure,
    #[error("Unknown transaction receiver account ID {account_id}")]
    UnknownTransactionReceiverId { account_id: AccountId },
    #[error("Unsupported action at index {index}")]
    UnsupportedAction { index: usize },
    #[error("Argument deserialization failure at index {index}")]
    ArgumentDeserializationFailure { index: usize },
    #[error("Msg deserialization failure at index {index}")]
    MsgDeserializationFailure { index: usize },
    #[error("Unknown token transfer receiver account ID {account_id} at index {index}")]
    UnknownTransferReceiverId { account_id: AccountId, index: usize },
    #[error("Invalid message for asset at index {index}")]
    InvalidMsgForAsset { index: usize },
    #[error("Unknown function name `{name}` at index {index}")]
    UnknownFunctionName { name: String, index: usize },
}

impl App {
    pub async fn check_and_calculate_gas(
        &self,
        signed_delegate_action: &SignedDelegateAction,
    ) -> Result<Gas, PreconditionError> {
        if !signed_delegate_action.verify() {
            return Err(PreconditionError::SignatureVerificationFailure);
        }

        let receiver_id = &signed_delegate_action.delegate_action.receiver_id;
        let accounts = self.accounts.read().await;

        if !accounts.allowed_receiver_account_ids.contains(receiver_id) {
            return Err(PreconditionError::UnknownTransactionReceiverId {
                account_id: receiver_id.clone(),
            });
        }

        let actions = signed_delegate_action.delegate_action.get_actions();
        let len = actions.len();
        let calls = actions
            .into_iter()
            .try_fold(Vec::with_capacity(len), |mut v, action| {
                if let Action::FunctionCall(fc) = action {
                    v.push(fc);
                    Ok(v)
                } else {
                    Err(v.len())
                }
            })
            .map_err(|index| PreconditionError::UnsupportedAction { index })?;

        if accounts.market_account_ids.contains_key(receiver_id) {
            // Calling a market contract directly.
            for (index, call) in calls.iter().enumerate() {
                if !self
                    .configuration
                    .allowed_methods
                    .contains(&call.method_name)
                {
                    return Err(PreconditionError::UnknownFunctionName {
                        name: call.method_name.clone(),
                        index,
                    });
                }
            }
        } else {
            // Token contract transfer call to market.
            for (index, call) in calls.iter().enumerate() {
                fn deserialize_args<'de, T: Deserialize<'de>>(
                    slice: &'de [u8],
                    index: usize,
                ) -> Result<T, PreconditionError> {
                    serde_json::from_slice::<T>(slice)
                        .map_err(|_| PreconditionError::ArgumentDeserializationFailure { index })
                }

                let (args, asset) = match &call.method_name[..] {
                    "ft_transfer_call" => {
                        let args = deserialize_args::<FtTransferCallArgs>(&call.args, index)?;

                        (
                            Box::new(args) as Box<dyn TransferCallArgs>,
                            FungibleAsset::<BorrowAsset>::nep141(receiver_id.clone()),
                        )
                    }
                    "mt_transfer_call" => {
                        let args = deserialize_args::<MtTransferCallArgs>(&call.args, index)?;
                        let token_id = args.token_id.clone();

                        (
                            Box::new(args) as Box<dyn TransferCallArgs>,
                            FungibleAsset::<BorrowAsset>::nep245(receiver_id.clone(), token_id),
                        )
                    }
                    name => {
                        return Err(PreconditionError::UnknownFunctionName {
                            name: name.to_owned(),
                            index,
                        });
                    }
                };

                let market_id = args.receiver_id();

                let Some(market_account_ids) = accounts.market_account_ids.get(market_id) else {
                    return Err(PreconditionError::UnknownTransferReceiverId {
                        account_id: market_id.to_owned(),
                        index,
                    });
                };

                let Ok(msg) = serde_json::from_str::<DepositMsg>(args.msg()) else {
                    return Err(PreconditionError::MsgDeserializationFailure { index });
                };

                if market_account_ids.borrow_asset == asset {
                    match msg {
                        DepositMsg::Supply | DepositMsg::Repay => { /* ok */ }
                        _ => return Err(PreconditionError::InvalidMsgForAsset { index }),
                    }
                } else if market_account_ids.collateral_asset == asset.coerce() {
                    match msg {
                        DepositMsg::Collateralize => { /* ok */ }
                        _ => return Err(PreconditionError::InvalidMsgForAsset { index }),
                    }
                } else {
                    return Err(PreconditionError::UnknownTransactionReceiverId {
                        account_id: receiver_id.clone(),
                    });
                }
            }
        }

        Ok(calls.iter().map(|call| call.gas).sum())
    }
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let configuration: Configuration =
        serde_yaml::from_reader(File::open(&args.config).unwrap()).unwrap();

    let mut app = App::new(args, configuration);
    app.load_markets().await;

    let addr = SocketAddr::from(([0, 0, 0, 0], app.args.port));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    let router = Router::new()
        .route("/", routing::get(root))
        .route("/relay", routing::post(relay))
        .with_state(app);

    tracing::info!("Listening on {}", listener.local_addr().unwrap());

    axum::serve(listener, router).await.unwrap();
}

async fn root() -> &'static str {
    "Hello, World!"
}

async fn relay(
    State(app): State<App>,
    Json(relay_request): Json<RelayRequest>,
) -> (StatusCode, Json<RelayResponse>) {
    match app
        .check_and_calculate_gas(&relay_request.signed_delegate_action)
        .await
    {
        Ok(_gas) => {
            let tx_result = app
                .near_client
                .sign_and_send(relay_request.signed_delegate_action)
                .await;
            match tx_result {
                Ok(execution) => (
                    StatusCode::OK,
                    Json(RelayResponse::Success {
                        execution: Box::new(execution),
                    }),
                ),
                Err(e) => (
                    StatusCode::OK,
                    Json(RelayResponse::Failure {
                        error: e.to_string(),
                    }),
                ),
            }
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(RelayResponse::Rejected {
                reason: e.to_string(),
            }),
        ),
    }
}
