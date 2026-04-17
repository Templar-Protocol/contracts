use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use blockchain_gateway_core::{ManagedAccountId, RegistryId, UniversalAccountId};
use blockchain_gateway_near::{ManagedSigner, NearClient};
use near_api::{types::AccountId, Contract, NetworkConfig, SecretKey, Signer};
use near_sandbox::Sandbox;
use near_token::NearToken;
use templar_common::{market::MarketConfiguration, market::YieldWeights};
use templar_universal_account::{InitArgs, NEAR_TESTNET_CHAIN_ID};
use test_utils::{
    market_configuration, test_signer::TestSigner, FtController, MarketController,
    MockOracleController, ProxyOracleController, RegistryController, UniversalAccountController,
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
    pub ft_contract_id: AccountId,
    pub beneficiary_account_id: AccountId,
}

impl SandboxHarness {
    pub async fn start() -> Result<Self> {
        let sandbox = Sandbox::start_sandbox().await?;
        let network = NetworkConfig::from_rpc_url("sandbox", sandbox.rpc_addr.parse()?);

        let gateway_signer_account_id = ManagedAccountId("gateway.near".parse()?);
        let gateway_secret_key = test_secret_key()?;
        sandbox
            .create_account(gateway_signer_account_id.0.clone())
            .initial_balance(NearToken::from_near(100))
            .public_key(gateway_secret_key.public_key().to_string())
            .send()
            .await?;

        let gateway_signer = ManagedSigner::new([gateway_secret_key])
            .await
            .context("failed to initialize gateway signer")?;

        let cleanup_signer_account_id = ManagedAccountId("cleanup.near".parse()?);
        let cleanup_secret_key = test_secret_key()?;
        sandbox
            .create_account(cleanup_signer_account_id.0.clone())
            .initial_balance(NearToken::from_near(100))
            .public_key(cleanup_secret_key.public_key().to_string())
            .send()
            .await?;
        let cleanup_signer = ManagedSigner::new([cleanup_secret_key])
            .await
            .context("failed to initialize cleanup signer")?;

        let registry_signer_account_id = ManagedAccountId("registry.near".parse()?);
        let registry_secret_key = test_secret_key()?;
        let registry_signer = ManagedSigner::new([registry_secret_key])
            .await
            .context("failed to initialize registry signer")?;

        let universal_account_signer_account_id = ManagedAccountId("ua.near".parse()?);
        let universal_account_secret_key = test_secret_key()?;
        sandbox
            .create_account(universal_account_signer_account_id.0.clone())
            .initial_balance(NearToken::from_near(100))
            .public_key(universal_account_secret_key.public_key().to_string())
            .send()
            .await?;
        let universal_account_signer = ManagedSigner::new([universal_account_secret_key])
            .await
            .context("failed to initialize universal account signer")?;

        let proxy_oracle_signer_account_id = ManagedAccountId("proxy-oracle.near".parse()?);
        let proxy_oracle_secret_key = test_secret_key()?;
        sandbox
            .create_account(proxy_oracle_signer_account_id.0.clone())
            .initial_balance(NearToken::from_near(100))
            .public_key(proxy_oracle_secret_key.public_key().to_string())
            .send()
            .await?;
        let proxy_oracle_signer = ManagedSigner::new([proxy_oracle_secret_key])
            .await
            .context("failed to initialize proxy oracle signer")?;

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

    pub async fn deploy_registry(&self) -> Result<RegistryId> {
        let account_id: AccountId = "registry.near".parse()?;
        let signer =
            create_account_signer(&self.sandbox, &account_id, NearToken::from_near(100)).await?;
        deploy_contract(
            &self.network,
            account_id.clone(),
            signer,
            RegistryController::wasm().await.to_vec(),
            "new",
            serde_json::json!({}),
        )
        .await?;
        Ok(RegistryId(account_id))
    }

    pub async fn deploy_market(
        &self,
    ) -> Result<(blockchain_gateway_core::MarketId, MarketConfiguration)> {
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

        Ok((blockchain_gateway_core::MarketId(market_id), configuration))
    }

    pub async fn deploy_universal_account(&self) -> Result<(UniversalAccountId, TestSigner)> {
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

        Ok((UniversalAccountId(account_id), test_signer))
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
