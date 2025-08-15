#![allow(clippy::unwrap_used)]

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
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
use tokio::{sync::RwLock, task::JoinSet};

use templar_common::market::DepositMsg;
use templar_relayer::{
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
struct GasPriceCache {
    price_per_tgas: NearToken,
    updated_at: SystemTime,
}

impl GasPriceCache {
    pub fn is_valid(&self, expires_s: u64) -> bool {
        self.updated_at
            .elapsed()
            .is_ok_and(|elapsed| elapsed < Duration::from_secs(expires_s))
    }

    pub fn price_gas(&self, gas: u64) -> NearToken {
        self.price_per_tgas
            .saturating_mul(u128::from(near_sdk::Gas::from_gas(gas).as_tgas()))
    }
}

#[derive(Debug, Clone)]
struct App {
    pub args: Args,
    pub configuration: Configuration,
    pub accounts: Arc<RwLock<AccountData>>,
    pub near: Near,
    pub gas_price: Arc<RwLock<GasPriceCache>>,
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

        Self {
            args,
            configuration,
            accounts: Arc::new(RwLock::new(AccountData::default())),
            near,
            gas_price: Arc::new(RwLock::new(GasPriceCache {
                price_per_tgas: NearToken::from_near(0),
                updated_at: UNIX_EPOCH,
            })),
            database,
        }
    }

    pub async fn price_gas(&self, gas: Gas) -> NearToken {
        let read_handle = self.gas_price.read().await;
        if !read_handle.price_per_tgas.is_zero()
            && read_handle.is_valid(self.configuration.gas_price_expires_secs)
        {
            return read_handle.price_gas(gas);
        }
        drop(read_handle);

        // Refresh gas price
        let mut write_handle = self.gas_price.write().await;
        if !write_handle.is_valid(self.configuration.gas_price_expires_secs) {
            info!("Refreshing gas price");
            match self.near.fetch_gas_price().await {
                Ok(price) => {
                    write_handle.price_per_tgas = price;
                    write_handle.updated_at = SystemTime::now();
                }
                Err(e) => {
                    tracing::error!("Failed to fetch gas price, using fallback: {e}");
                    write_handle.price_per_tgas = self.configuration.fallback_yocto_per_tgas;
                }
            }
        }
        write_handle.price_gas(gas)
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
        Ok(gas) => {
            let account_id = relay_request
                .signed_delegate_action
                .delegate_action
                .sender_id
                .clone();

            let gas_spend = app.price_gas(gas).await;

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

            if available_allowance < gas_spend {
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
                .set_pending_transaction(&account_id, gas_spend, transaction_hash)
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

            let result = app
                .database
                .record_transaction(
                    &account_id,
                    transaction_hash,
                    NearToken::from_yoctonear(tx_result.tokens_burnt()),
                    succeeded,
                )
                .await;

            if let Err(e) = result {
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
