use std::{
    collections::{HashMap, HashSet},
    fs::File,
    net::SocketAddr,
    path::PathBuf,
};

use axum::{Json, Router, extract::State, http::StatusCode, routing};
use near_crypto::SecretKey;
use near_fetch::signer::KeyRotatingSigner;
use near_primitives::{
    action::{Action, delegate::SignedDelegateAction},
    types::{AccountId, Gas},
    views::FinalExecutionOutcomeView,
};
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::serde_json;
use tokio::task::JoinSet;

use templar_common::{
    asset::{BorrowAsset, FungibleAsset},
    market::DepositMsg,
};
use templar_relayer::{
    Configuration, FtTransferCallArgs, MarketAccounts, MtTransferCallArgs, TransferCallArgs,
    client::NearClient,
};
use tracing::info;

#[derive(Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
struct Environment {
    #[serde(default = "default_port")]
    pub port: u16,
    pub database_url: String,
    #[serde(default = "default_rpc_url")]
    pub rpc_url: String,
    #[serde(default = "default_config")]
    pub config: PathBuf,
    pub registries: Vec<AccountId>,
    pub markets: Vec<AccountId>,
    pub account_id: AccountId,
    pub secret_keys: Vec<SecretKey>,
}

fn default_port() -> u16 {
    3000
}

fn default_rpc_url() -> String {
    "https://rpc.testnet.near.org/".to_string()
}

fn default_config() -> PathBuf {
    "./config.yaml".parse().unwrap()
}

#[derive(Debug, Clone)]
struct App {
    pub environment: Environment,
    pub configuration: Configuration,
    pub market_account_ids: HashMap<AccountId, MarketAccounts>,
    pub allowed_receiver_account_ids: HashSet<AccountId>,
    pub near_client: NearClient,
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
    pub fn check_and_calculate_gas(
        &self,
        signed_delegate_action: &SignedDelegateAction,
    ) -> Result<Gas, PreconditionError> {
        if !signed_delegate_action.verify() {
            return Err(PreconditionError::SignatureVerificationFailure);
        }

        let receiver_id = &signed_delegate_action.delegate_action.receiver_id;

        if !self.allowed_receiver_account_ids.contains(receiver_id) {
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
                    return Ok(v);
                } else {
                    return Err(v.len());
                }
            })
            .map_err(|index| PreconditionError::UnsupportedAction { index })?;

        if self.market_account_ids.contains_key(receiver_id) {
            // Calling a market contract directly.
            for (index, call) in calls.iter().enumerate() {
                if !self
                    .configuration
                    .allowed_methods
                    .contains(&call.method_name)
                {
                    return Err(PreconditionError::UnknownFunctionName {
                        name: call.method_name.to_owned(),
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

                let Some(market_account_ids) = self.market_account_ids.get(market_id) else {
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
    let environment: Environment = envy::from_env().unwrap();
    tracing_subscriber::fmt::init();

    let configuration: Configuration =
        serde_yaml::from_reader(File::open(&environment.config).unwrap()).unwrap();

    let near_client = NearClient::new(
        near_jsonrpc_client::JsonRpcClient::connect(&environment.rpc_url),
        KeyRotatingSigner::try_from_iter(
            environment
                .secret_keys
                .iter()
                .map(|sk| (environment.account_id.clone(), sk.clone())),
        )
        .unwrap(),
    );

    let mut markets = environment.markets.clone();

    // Load markets from registry...
    let mut set = JoinSet::new();
    for registry_id in &environment.registries {
        set.spawn({
            let near_client = near_client.clone();
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
            let near_client = near_client.clone();
            async move { near_client.load_market_accounts(&market).await }
        });
    }
    let market_accounts_vec = set.join_all().await;

    let mut market_account_ids = HashMap::new();
    let mut allowed_receiver_account_ids = HashSet::new();

    for market_accounts in market_accounts_vec {
        let market_id = market_accounts.account_id.clone();

        info!(
            "Loaded market {market_id} with borrow asset {} and collateral asset {}",
            market_accounts.borrow_asset, market_accounts.collateral_asset,
        );

        allowed_receiver_account_ids.insert(market_id);
        allowed_receiver_account_ids.insert(market_accounts.borrow_asset.contract_id().to_owned());
        allowed_receiver_account_ids
            .insert(market_accounts.collateral_asset.contract_id().to_owned());

        market_account_ids.insert(market_accounts.account_id.clone(), market_accounts);
    }

    let app = App {
        configuration,
        environment,
        market_account_ids,
        allowed_receiver_account_ids,
        near_client,
    };

    let addr = SocketAddr::from(([0, 0, 0, 0], app.environment.port));
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

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct RelayRequest {
    signed_delegate_action: SignedDelegateAction,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub enum RelayResponse {
    Success {
        execution: FinalExecutionOutcomeView,
    },
    Failure {
        error: String,
    },
    Rejected {
        reason: String,
    },
}

async fn relay(
    State(app): State<App>,
    Json(relay_request): Json<RelayRequest>,
) -> (StatusCode, Json<RelayResponse>) {
    match app.check_and_calculate_gas(&relay_request.signed_delegate_action) {
        Ok(gas) => {
            let tx_result = app
                .near_client
                .sign_and_send(relay_request.signed_delegate_action)
                .await;
            match tx_result {
                Ok(execution) => (StatusCode::OK, Json(RelayResponse::Success { execution })),
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
