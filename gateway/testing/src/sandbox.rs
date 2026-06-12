use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use near_api::{types::AccountId, Contract, NetworkConfig, SecretKey, Signer};
use near_sandbox::Sandbox;
use near_token::NearToken;
use templar_common::{
    market::{MarketConfiguration, YieldWeights},
    oracle::{pyth::PriceIdentifier, redstone::config as redstone_config},
};
use templar_gateway_core::NearClient;
use templar_gateway_runtime::ManagedSigner;
use templar_gateway_types::ManagedAccountId;
use templar_proxy_oracle_near_common::price_transformer::PriceTransformer;
use templar_universal_account::{InitArgs, NEAR_TESTNET_CHAIN_ID};
use test_utils::{
    controller::{lst_oracle::LstOracleController, ref_finance::PoolInfo},
    market_configuration,
    test_signer::TestSigner,
    FtController, MarketController, MockOracleController, ProxyOracleController,
    ReceiverController, RedStoneAdapterController, RefFinanceController, RegistryController,
    UniversalAccountController,
};

pub struct SandboxHarness {
    pub sandbox: Sandbox,
    pub network: NetworkConfig,
    pub gateway_signer_account_id: ManagedAccountId,
    pub cleanup_signer_account_id: ManagedAccountId,
    pub registry_signer_account_id: ManagedAccountId,
    pub universal_account_signer_account_id: ManagedAccountId,
    pub proxy_oracle_signer_account_id: ManagedAccountId,
    pub gateway_signers: HashMap<ManagedAccountId, ManagedSigner>,
    registry_signer: Arc<Signer>,
    pub ft_contract_id: AccountId,
    pub beneficiary_account_id: AccountId,
}

impl SandboxHarness {
    pub async fn start() -> Result<Self> {
        let sandbox = Sandbox::start_sandbox().await?;
        let network = NetworkConfig::from_rpc_url("sandbox", sandbox.rpc_addr.parse()?);

        let (gateway_signer_account_id, gateway_signer) =
            create_managed_signer_account(&sandbox, "gateway.near", "gateway").await?;
        let (cleanup_signer_account_id, cleanup_signer) =
            create_managed_signer_account(&sandbox, "cleanup.near", "cleanup").await?;
        let (registry_signer_account_id, registry_signer) =
            create_managed_signer_account(&sandbox, "registry.near", "registry").await?;
        let registry_deploy_signer = registry_signer.signer.clone();

        let (universal_account_signer_account_id, universal_account_signer) =
            create_managed_signer_account(&sandbox, "ua.near", "universal account").await?;
        let (proxy_oracle_signer_account_id, proxy_oracle_signer) =
            create_managed_signer_account(&sandbox, "proxy-oracle.near", "proxy oracle").await?;

        let gateway_signers = HashMap::from([
            (gateway_signer_account_id.clone(), gateway_signer),
            (cleanup_signer_account_id.clone(), cleanup_signer),
            (registry_signer_account_id.clone(), registry_signer),
            (
                universal_account_signer_account_id.clone(),
                universal_account_signer,
            ),
            (proxy_oracle_signer_account_id.clone(), proxy_oracle_signer),
        ]);

        let ft_contract_id: AccountId = "mock-ft.near".parse()?;
        let ft_signer =
            create_account_signer(&sandbox, &ft_contract_id, NearToken::from_near(100)).await?;
        deploy_contract(
            &network,
            ft_contract_id.clone(),
            ft_signer,
            FtController::wasm().await.to_vec(),
            "new",
            serde_json::json!({
                "name": "Mock FT",
                "symbol": "MFT",
            }),
        )
        .await?;

        let beneficiary_account_id: AccountId = "beneficiary.near".parse()?;
        sandbox
            .create_account(beneficiary_account_id.clone())
            .initial_balance(NearToken::from_near(25))
            .send()
            .await?;

        Ok(Self {
            sandbox,
            network,
            gateway_signer_account_id,
            cleanup_signer_account_id,
            registry_signer_account_id,
            universal_account_signer_account_id,
            proxy_oracle_signer_account_id,
            gateway_signers,
            registry_signer: registry_deploy_signer,
            ft_contract_id,
            beneficiary_account_id,
        })
    }

    pub fn gateway_client(&self) -> NearClient {
        NearClient::new(self.network.clone())
    }

    pub async fn ft_wasm(&self) -> Vec<u8> {
        FtController::wasm().await.to_vec()
    }

    pub async fn deploy_mt(&self, account_id: AccountId) -> Result<AccountId> {
        let signer =
            create_account_signer(&self.sandbox, &account_id, NearToken::from_near(100)).await?;
        deploy_contract(
            &self.network,
            account_id.clone(),
            signer,
            test_utils::controller::mt::MtController::wasm()
                .await
                .to_vec(),
            "new",
            serde_json::json!({}),
        )
        .await?;
        Ok(account_id)
    }

    pub async fn deploy_receiver(&self, account_id: AccountId) -> Result<AccountId> {
        let signer =
            create_account_signer(&self.sandbox, &account_id, NearToken::from_near(100)).await?;
        deploy_contract(
            &self.network,
            account_id.clone(),
            signer,
            ReceiverController::wasm().await.to_vec(),
            "new",
            serde_json::json!({}),
        )
        .await?;
        Ok(account_id)
    }

    pub async fn deploy_ref_finance(
        &self,
        account_id: AccountId,
        pools: Vec<PoolInfo>,
    ) -> Result<AccountId> {
        let signer =
            create_account_signer(&self.sandbox, &account_id, NearToken::from_near(100)).await?;
        deploy_contract(
            &self.network,
            account_id.clone(),
            signer,
            RefFinanceController::wasm().await.to_vec(),
            "new",
            serde_json::json!({ "pools": pools }),
        )
        .await?;
        Ok(account_id)
    }

    pub async fn deploy_registry(&self) -> Result<AccountId> {
        let account_id: AccountId = "registry.near".parse()?;
        deploy_contract(
            &self.network,
            account_id.clone(),
            self.registry_signer.clone(),
            RegistryController::wasm().await.to_vec(),
            "new",
            serde_json::json!({}),
        )
        .await?;
        Ok(account_id)
    }

    pub async fn deploy_market(&self) -> Result<(AccountId, MarketConfiguration)> {
        let borrow_asset_id: AccountId = "borrow-ft.near".parse()?;
        let collateral_asset_id: AccountId = "collateral-ft.near".parse()?;
        let oracle_id: AccountId = "oracle.near".parse()?;
        let market_id: AccountId = "market.near".parse()?;

        let borrow_signer =
            create_account_signer(&self.sandbox, &borrow_asset_id, NearToken::from_near(100))
                .await?;
        deploy_contract(
            &self.network,
            borrow_asset_id.clone(),
            borrow_signer,
            FtController::wasm().await.to_vec(),
            "new",
            serde_json::json!({ "name": "Borrow FT", "symbol": "BFT" }),
        )
        .await?;

        let collateral_signer = create_account_signer(
            &self.sandbox,
            &collateral_asset_id,
            NearToken::from_near(100),
        )
        .await?;
        deploy_contract(
            &self.network,
            collateral_asset_id.clone(),
            collateral_signer,
            FtController::wasm().await.to_vec(),
            "new",
            serde_json::json!({ "name": "Collateral FT", "symbol": "CFT" }),
        )
        .await?;

        let oracle_signer =
            create_account_signer(&self.sandbox, &oracle_id, NearToken::from_near(100)).await?;
        deploy_contract(
            &self.network,
            oracle_id.clone(),
            oracle_signer,
            MockOracleController::wasm().await.to_vec(),
            "new",
            serde_json::json!({}),
        )
        .await?;

        let configuration = market_configuration(
            oracle_id,
            borrow_asset_id,
            collateral_asset_id,
            self.gateway_signer_account_id.0.clone(),
            YieldWeights::new_with_supply_weight(1),
        );

        let market_signer =
            create_account_signer(&self.sandbox, &market_id, NearToken::from_near(100)).await?;
        deploy_contract(
            &self.network,
            market_id.clone(),
            market_signer,
            MarketController::wasm().await.to_vec(),
            "new",
            serde_json::json!({
                "configuration": configuration.clone(),
            }),
        )
        .await?;

        Ok((market_id, configuration))
    }

    pub async fn deploy_universal_account(&self) -> Result<(AccountId, TestSigner)> {
        let account_id = self.universal_account_signer_account_id.0.clone();
        let signer = Signer::from_secret_key(test_secret_key()?)
            .context("failed to initialize universal account deploy signer")?;

        let test_signer = TestSigner::fixed_passkey([0x11; 32]);
        let init = InitArgs {
            key: test_signer.id(),
            chain_id: NEAR_TESTNET_CHAIN_ID.into(),
            execute: None,
        };

        deploy_contract(
            &self.network,
            account_id.clone(),
            signer,
            UniversalAccountController::wasm().await.to_vec(),
            "new",
            &init,
        )
        .await?;

        Ok((account_id, test_signer))
    }

    pub async fn deploy_proxy_oracle(&self) -> Result<AccountId> {
        let account_id = self.proxy_oracle_signer_account_id.0.clone();
        let signer = Signer::from_secret_key(test_secret_key()?)
            .context("failed to initialize proxy oracle deploy signer")?;

        deploy_contract(
            &self.network,
            account_id.clone(),
            signer,
            ProxyOracleController::wasm().await.to_vec(),
            "new",
            serde_json::json!({}),
        )
        .await?;

        Ok(account_id)
    }

    pub async fn deploy_mock_oracle(&self, account_id: AccountId) -> Result<AccountId> {
        let signer =
            create_account_signer(&self.sandbox, &account_id, NearToken::from_near(100)).await?;
        deploy_contract(
            &self.network,
            account_id.clone(),
            signer,
            MockOracleController::wasm().await.to_vec(),
            "new",
            serde_json::json!({}),
        )
        .await?;
        Ok(account_id)
    }

    pub async fn deploy_redstone_adapter(&self, account_id: AccountId) -> Result<AccountId> {
        let signer =
            create_account_signer(&self.sandbox, &account_id, NearToken::from_near(100)).await?;
        let mut config = redstone_config::prod();
        config.max_timestamp_delay_ms = u64::MAX;
        config.max_timestamp_ahead_ms = u64::MAX;
        config.min_interval_between_updates_ms = 0;
        deploy_contract(
            &self.network,
            account_id.clone(),
            signer,
            RedStoneAdapterController::wasm().await.to_vec(),
            "new",
            serde_json::json!({
                "config": config,
            }),
        )
        .await?;
        Ok(account_id)
    }

    pub async fn set_mock_oracle_pyth_price(
        &self,
        oracle_id: AccountId,
        price_identifier: PriceIdentifier,
        price: Option<templar_common::oracle::pyth::Price>,
    ) -> Result<()> {
        let signer = Signer::from_secret_key(test_secret_key()?)
            .context("failed to initialize mock oracle signer")?;
        Contract(oracle_id.clone())
            .call_function(
                "set_pyth_price",
                serde_json::json!({
                    "price_identifier": price_identifier,
                    "price": price,
                }),
            )
            .transaction()
            .gas(near_sdk::Gas::from_tgas(100))
            .with_signer(oracle_id, signer)
            .send_to(&self.network)
            .await?
            .assert_success();
        Ok(())
    }

    pub async fn set_mock_oracle_redstone_price(
        &self,
        oracle_id: AccountId,
        feed_id: templar_common::oracle::redstone::FeedId,
        data: Option<templar_common::oracle::redstone::FeedData>,
    ) -> Result<()> {
        let signer = Signer::from_secret_key(test_secret_key()?)
            .context("failed to initialize mock oracle signer")?;
        Contract(oracle_id.clone())
            .call_function(
                "set_redstone_price",
                serde_json::json!({
                    "feed_id": feed_id,
                    "data": data,
                }),
            )
            .transaction()
            .gas(near_sdk::Gas::from_tgas(100))
            .with_signer(oracle_id, signer)
            .send_to(&self.network)
            .await?
            .assert_success();
        Ok(())
    }

    pub async fn deploy_lst_oracle(
        &self,
        account_id: AccountId,
        oracle_id: AccountId,
    ) -> Result<AccountId> {
        let signer =
            create_account_signer(&self.sandbox, &account_id, NearToken::from_near(100)).await?;
        deploy_contract(
            &self.network,
            account_id.clone(),
            signer,
            LstOracleController::wasm().await.to_vec(),
            "new",
            serde_json::json!({ "oracle_id": oracle_id }),
        )
        .await?;
        Ok(account_id)
    }

    pub async fn create_lst_transformer(
        &self,
        oracle_id: AccountId,
        price_identifier: PriceIdentifier,
        entry: PriceTransformer,
    ) -> Result<()> {
        let signer = Signer::from_secret_key(test_secret_key()?)
            .context("failed to initialize lst oracle signer")?;
        Contract(oracle_id.clone())
            .call_function(
                "create_transformer",
                serde_json::json!({
                    "price_identifier": price_identifier,
                    "entry": entry,
                }),
            )
            .transaction()
            .deposit(NearToken::from_yoctonear(1))
            .gas(near_sdk::Gas::from_tgas(100))
            .with_signer(oracle_id, signer)
            .send_to(&self.network)
            .await?
            .assert_success();
        Ok(())
    }
}

async fn create_account_signer(
    sandbox: &Sandbox,
    account_id: &AccountId,
    initial_balance: NearToken,
) -> Result<Arc<Signer>> {
    let secret_key = test_secret_key()?;
    sandbox
        .create_account(account_id.clone())
        .initial_balance(initial_balance)
        .public_key(secret_key.public_key().to_string())
        .send()
        .await?;

    Signer::from_secret_key(secret_key).context("failed to initialize account signer")
}

async fn create_managed_signer_account(
    sandbox: &Sandbox,
    account_id: &str,
    label: &str,
) -> Result<(ManagedAccountId, ManagedSigner)> {
    let account_id = ManagedAccountId(account_id.parse()?);
    let secret_key = test_secret_key()?;
    sandbox
        .create_account(account_id.0.clone())
        .initial_balance(NearToken::from_near(100))
        .public_key(secret_key.public_key().to_string())
        .send()
        .await?;
    let signer = ManagedSigner::new([secret_key])
        .await
        .with_context(|| format!("failed to initialize {label} signer"))?;
    Ok((account_id, signer))
}

async fn deploy_contract(
    network: &NetworkConfig,
    account_id: AccountId,
    signer: Arc<Signer>,
    code: Vec<u8>,
    init_method: &str,
    init_args: impl serde::Serialize,
) -> Result<()> {
    Contract::deploy(account_id)
        .use_code(code)
        .with_init_call(init_method, init_args)?
        .with_signer(signer)
        .send_to(network)
        .await?
        .assert_success();

    Ok(())
}

fn test_secret_key() -> Result<SecretKey> {
    Ok("ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q"
        .parse()?)
}
