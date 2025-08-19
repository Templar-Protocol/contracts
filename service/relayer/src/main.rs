#![allow(clippy::unwrap_used)]

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use axum::{extract::State, http::StatusCode, routing, Json, Router};
use clap::Parser;
use near_crypto::{InMemorySigner, SecretKey};
use near_primitives::{
    action::{delegate::SignedDelegateAction, Action},
    types::{AccountId, Gas},
    views::FinalExecutionStatus,
};
use near_sdk::{serde_json, NearToken};
use tokio::{signal, sync::RwLock, task::JoinSet};

use templar_common::market::DepositMsg;
use templar_relayer::{
    cache::{Cache, CacheHandle},
    client::{database::Database, near::Near},
    error::PreconditionError,
    message::{RelayRequest, RelayResponse},
    AssetTransfer, Configuration, MarketAccounts,
};
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser, Debug, Clone)]
struct Args {
    /// Run the relayer on this port.
    #[arg(short, long, env = "PORT", default_value_t = 3000)]
    pub port: u16,
    /// Postgres database connection URL.
    #[arg(long, env = "DATABASE_URL")]
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
    pub near: Near,
    pub cache: Arc<CacheHandle>,
    pub database: Database,
}

impl App {
    pub fn new(args: Args, configuration: Configuration) -> Self {
        let near = Near::new(
            near_jsonrpc_client::JsonRpcClient::connect(&args.rpc_url),
            args.account_id.clone(),
            args.secret_key
                .iter()
                .map(|s| InMemorySigner::from_secret_key(args.account_id.clone(), s.clone()).into())
                .collect(),
        );

        let database = Database::new(&args.database_url).unwrap();

        let cache = Cache::start(
            near.clone(),
            Duration::from_secs(configuration.gas_price_refresh_secs),
            Duration::from_secs(configuration.nonce_refresh_secs),
        );

        Self {
            args,
            configuration,
            accounts: Arc::new(RwLock::new(AccountData::default())),
            near,
            cache: Arc::new(cache),
            database,
        }
    }

    pub async fn estimate_cost_of_gas(&self, gas: Gas) -> Option<NearToken> {
        const TERA: u128 = near_sdk::Gas::from_tgas(1).as_gas() as u128;

        let price_per_tgas = self.cache.gas_price().await;
        price_per_tgas
            .checked_mul(u128::from(gas))?
            .checked_div(TERA)
    }

    pub async fn load_markets(&mut self) {
        let mut markets = self.args.market.clone();

        // Load markets from registry...
        let mut set = JoinSet::new();
        for registry_id in &self.args.registry {
            set.spawn({
                let near = self.near.clone();
                let registry_id = registry_id.clone();
                async move {
                    match near.load_deployments_from_registry(&registry_id).await {
                        Ok(deployments) => deployments,
                        Err(e) => {
                            warn!("Failed to load deployments from registry {registry_id}: {e}");
                            vec![]
                        }
                    }
                }
            });
        }
        markets.extend(set.join_all().await.into_iter().flatten());

        // ...and add any individual markets.
        let mut set = JoinSet::new();
        for market in markets {
            set.spawn({
                let near = self.near.clone();
                async move {
                    match near.load_market_accounts(&market).await {
                        Ok(market_accounts) => Some(market_accounts),
                        Err(e) => {
                            warn!("Failed to load accounts for market {market}: {e}");
                            None
                        }
                    }
                }
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
                let transfer = AssetTransfer::parse(call, index, receiver_id.clone())?;
                let market_id = transfer.token_receiver_id();

                let Some(market_account_ids) = accounts.market_account_ids.get(market_id) else {
                    return Err(PreconditionError::UnknownTransferReceiverId {
                        account_id: market_id.to_owned(),
                        index,
                    });
                };

                let Ok(msg) = serde_json::from_str::<DepositMsg>(transfer.args.msg()) else {
                    return Err(PreconditionError::MsgDeserializationFailure { index });
                };

                if transfer.asset() == market_account_ids.borrow_asset {
                    if !matches!(msg, DepositMsg::Supply | DepositMsg::Repay) {
                        return Err(PreconditionError::InvalidMsgForAsset { index });
                    }
                } else if transfer.asset() == market_account_ids.collateral_asset {
                    if !matches!(msg, DepositMsg::Collateralize) {
                        return Err(PreconditionError::InvalidMsgForAsset { index });
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

    let database = app.database.clone();

    let addr = SocketAddr::from(([0, 0, 0, 0], app.args.port));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    let router = Router::new()
        .route("/", routing::get(root))
        .route("/relay", routing::post(relay))
        .with_state(app);

    tracing::info!("Listening on {}", listener.local_addr().unwrap());

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal(database))
        .await
        .unwrap();
}

// https://github.com/tokio-rs/axum/blob/9ec85d69703a9065a1098bb43bd93113695d5ade/examples/graceful-shutdown/src/main.rs
#[allow(clippy::expect_used)]
async fn shutdown_signal(database: Database) {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    database.close().await;
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
        Ok(gas) => {
            let account_id = relay_request
                .signed_delegate_action
                .delegate_action
                .sender_id
                .clone();

            let Some(cost_of_gas) = app.estimate_cost_of_gas(gas).await else {
                error!("Failed to estimate cost of gas: {gas}");
                return RelayResponse::failure("Failed to estimate cost of gas");
            };

            let available_allowance = match app
                .database
                .get_available_allowance_or_create(
                    &account_id,
                    app.configuration.starting_allowance_yocto,
                )
                .await
            {
                Ok(available) => available,
                Err(e) => {
                    error!("Database error trying to obtain available balance: {e}");
                    return RelayResponse::failure("Database Error");
                }
            };

            if available_allowance < cost_of_gas {
                return RelayResponse::rejected("Insufficient allowance");
            }

            let signed_transaction = match app
                .near
                .construct_delegate_transaction(relay_request.signed_delegate_action)
                .await
            {
                Ok(tx) => tx,
                Err(e) => {
                    error!("Error constructing delegate transaction: {e}");
                    return RelayResponse::failure(e);
                }
            };

            let transaction_hash = signed_transaction.get_hash();

            if let Err(e) = app
                .database
                .set_pending_transaction(&account_id, cost_of_gas, transaction_hash)
                .await
            {
                return RelayResponse::rejected(e);
            }

            let tx_result = match app.near.send_transaction(signed_transaction).await {
                Ok(r) => r,
                Err(e) => {
                    error!("Send transaction failure: {e}");
                    return RelayResponse::failure(e);
                }
            };

            let succeeded = matches!(tx_result.status, FinalExecutionStatus::SuccessValue(_));

            if let Err(e) = app
                .database
                .record_transaction(
                    &account_id,
                    transaction_hash,
                    NearToken::from_yoctonear(tx_result.tokens_burnt()),
                    succeeded,
                )
                .await
            {
                error!("Error recording transaction after submitting to blockchain: {e}");
            }

            RelayResponse::success(tx_result)
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(RelayResponse::Rejected {
                reason: e.to_string(),
            }),
        ),
    }
}
