use std::{
    collections::{hash_map::Entry, HashMap},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use clap::Parser;
use near_crypto::{InMemorySigner, SecretKey};
use near_primitives::action::{delegate::SignedDelegateAction, Action};
use near_sdk::{
    serde::{Deserialize, Serialize},
    serde_json, AccountId, NearToken,
};
use templar_common::market::DepositMsg;
use tokio::{sync::RwLock, task::JoinSet};
use tracing::{info, warn};

use crate::{
    broom::Broom,
    cache::Cache,
    client::{database::Database, near::Near},
    error::PreconditionError,
    AccountData, AssetTransfer, ContractData,
};

#[derive(Parser, Debug, Clone)]
pub struct Args {
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct Configuration {
    pub allowed_methods: Vec<String>,
    pub starting_allowance_yocto: NearToken,
    pub gas_price_refresh_secs: u64,
    pub nonce_refresh_secs: u64,
}

#[derive(Debug, Clone)]
pub struct App {
    pub args: Args,
    pub configuration: Configuration,
    pub accounts: Arc<RwLock<AccountData>>,
    pub near: Near,
    pub cache: Arc<Cache>,
    /// This field is only relevant for its Drop implementation, which shuts down the Broom.
    _broom: Arc<Broom>,
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

        #[allow(clippy::unwrap_used)]
        let database = Database::new(&args.database_url).unwrap();

        let cache = Cache::new(
            near.clone(),
            Duration::from_secs(configuration.gas_price_refresh_secs),
            Duration::from_secs(configuration.nonce_refresh_secs),
        );

        let broom = Broom::new(database.clone(), near.clone(), 16, Duration::from_secs(10));

        Self {
            args,
            configuration,
            accounts: Arc::new(RwLock::new(AccountData::default())),
            near,
            cache: Arc::new(cache),
            _broom: Arc::new(broom),
            database,
        }
    }

    pub async fn estimate_cost_of_gas(&self, gas: u64) -> Option<NearToken> {
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
                    match near
                        .load_deployments_from_registry(registry_id.clone())
                        .await
                    {
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
                    match near.load_market_accounts(market.clone()).await {
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

        let mut markets = HashMap::new();
        let mut allowed_contracts = HashMap::new();

        for market_accounts in market_accounts_vec.into_iter().flatten() {
            let market_id = market_accounts.account_id.clone();

            info!(
                "Loaded market {market_id} with borrow asset {} and collateral asset {}",
                market_accounts.borrow_asset, market_accounts.collateral_asset,
            );

            for contract_id in [
                market_id,
                market_accounts.borrow_asset.contract_id().to_owned(),
                market_accounts.collateral_asset.contract_id().to_owned(),
            ] {
                if let Entry::Vacant(e) = allowed_contracts.entry(contract_id.clone()) {
                    let storage_balance_bounds = self
                        .near
                        .load_storage_balance_bounds(contract_id.clone())
                        .await
                        .ok();

                    info!(
                        "Loaded storage balance bounds for contract {}: {}",
                        contract_id,
                        storage_balance_bounds
                            .as_ref()
                            .map_or(NearToken::from_near(0), |bounds| bounds.min),
                    );

                    e.insert(ContractData {
                        storage_balance_bounds,
                    });
                }
            }

            markets.insert(market_accounts.account_id.clone(), market_accounts);
        }

        let mut handle = self.accounts.write().await;
        handle.market_data = markets;
        handle.allowed_contract_data = allowed_contracts;
    }

    /// Check and calculate gas for a signed delegate action.
    ///
    /// # Errors
    ///
    /// - If the signature verification fails.
    /// - If the receiver ID is unknown.
    /// - If the action is not supported.
    /// - If the function name is not valid.
    /// - If the function arguments are invalid.
    /// - etc. See [`PreconditionError`] for more details.
    pub async fn check_and_calculate_gas(
        &self,
        signed_delegate_action: &SignedDelegateAction,
    ) -> Result<u64, PreconditionError> {
        if !signed_delegate_action.verify() {
            return Err(PreconditionError::SignatureVerificationFailure);
        }

        let receiver_id = &signed_delegate_action.delegate_action.receiver_id;
        let accounts = self.accounts.read().await;

        if !accounts.allowed_contract_data.contains_key(receiver_id) {
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

        if accounts.market_data.contains_key(receiver_id) {
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

                let Some(market_account_ids) = accounts.market_data.get(market_id) else {
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
