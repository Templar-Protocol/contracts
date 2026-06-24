use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use anyhow::{Context, Result};
use near_api::{types::AccountId, Account, Contract, NetworkConfig, SecretKey, Signer};
use near_sandbox::{
    config::{
        DEFAULT_GENESIS_ACCOUNT, DEFAULT_GENESIS_ACCOUNT_PRIVATE_KEY,
        DEFAULT_GENESIS_ACCOUNT_PUBLIC_KEY,
    },
    GenesisAccount, Sandbox, SandboxConfig,
};
use near_token::NearToken;
use templar_common::{
    market::{MarketConfiguration, YieldWeights},
    oracle::{pyth::PriceIdentifier, redstone::config as redstone_config},
    Nanoseconds,
};
use templar_gateway_core::NearClient;
use templar_gateway_runtime::ManagedSigner;
use templar_gateway_types::ManagedAccountId;
use templar_proxy_oracle_kernel::proxy::Proxy;
use templar_proxy_oracle_near_common::{
    input::Source, price_transformer::PriceTransformer, state::legacy::v0,
};
use templar_proxy_oracle_near_governance_common::{Operation, TtlConfig};
use templar_universal_account::{InitArgs, NEAR_TESTNET_CHAIN_ID};
use test_utils::{
    controller::{lst_oracle::LstOracleController, ref_finance::PoolInfo},
    market_configuration,
    test_signer::TestSigner,
    FtController, GovernanceContractController, MarketController, MockOracleController,
    ProxyOracleController, ReceiverController, RedStoneAdapterController, RefFinanceController,
    RegistryController, UniversalAccountController,
};

pub struct SandboxHarness {
    /// The owned `neard` process in owned mode; `None` in attach mode, where
    /// `neard` runs out-of-band and we only hold an RPC connection. Held purely
    /// to keep the process alive for the harness lifetime (dropping a `Sandbox`
    /// kills its process), hence never read directly.
    #[allow(dead_code, reason = "RAII handle keeping owned neard alive")]
    sandbox: Option<Sandbox>,
    pub network: NetworkConfig,
    /// Per-process intermediate root account, created once from the genesis key.
    /// Every working (sub-)account is funded and signed by this account instead
    /// of the genesis root, so the heavily-shared genesis key's nonce is touched
    /// only once per test process. This account's own key nonce is touched only
    /// by this process, removing the cross-process nonce contention that signing
    /// every account with the single genesis key would create on a shared node.
    tenant_root_id: AccountId,
    tenant_root_signer: Arc<Signer>,
    pub gateway_signer_account_id: ManagedAccountId,
    pub cleanup_signer_account_id: ManagedAccountId,
    pub registry_signer_account_id: ManagedAccountId,
    pub universal_account_signer_account_id: ManagedAccountId,
    pub proxy_oracle_signer_account_id: ManagedAccountId,
    pub ft_contract_id: AccountId,
    pub beneficiary_account_id: AccountId,
    /// Every account the harness can sign as: the gateway operator accounts
    /// seeded at [`start`](Self::start), plus accounts created on demand during
    /// a test (users, contracts). Used both to seed the gateway service under
    /// test (see [`Self::gateway_signers`]) and to drive the direct
    /// [`Client`](templar_gateway_client::Client).
    signers: Mutex<HashMap<ManagedAccountId, ManagedSigner>>,
    /// Monotonic counter for minting unique account ids within this harness.
    account_seq: AtomicU64,
}

impl SandboxHarness {
    /// Start a harness. In **attach** mode (`NEAR_SANDBOX_RPC_URL` set) it
    /// connects to an out-of-band `neard` over RPC and creates only its own
    /// uniquely-named sub-accounts, so many harnesses can share one node. In
    /// **owned** mode (default) it launches a dedicated `neard`. Either way,
    /// accounts are `*.sandbox` sub-accounts created via near-api against the
    /// genesis root.
    pub async fn start() -> Result<Self> {
        let (sandbox, network) = connect().await?;
        let root_signer = Signer::from_secret_key(genesis_secret_key()?)
            .context("failed to initialize genesis root signer")?;
        let (tenant_root_id, tenant_root_signer) =
            create_tenant_root(&network, &root_signer).await?;
        let signers = Mutex::new(HashMap::new());
        let account_seq = AtomicU64::new(0);

        let harness = Self {
            sandbox,
            network,
            tenant_root_id,
            tenant_root_signer,
            // Operator id fields are filled in after the partial harness exists
            // so account creation can go through `Self::create_account`.
            gateway_signer_account_id: ManagedAccountId(DEFAULT_GENESIS_ACCOUNT.to_owned()),
            cleanup_signer_account_id: ManagedAccountId(DEFAULT_GENESIS_ACCOUNT.to_owned()),
            registry_signer_account_id: ManagedAccountId(DEFAULT_GENESIS_ACCOUNT.to_owned()),
            universal_account_signer_account_id: ManagedAccountId(
                DEFAULT_GENESIS_ACCOUNT.to_owned(),
            ),
            proxy_oracle_signer_account_id: ManagedAccountId(DEFAULT_GENESIS_ACCOUNT.to_owned()),
            ft_contract_id: DEFAULT_GENESIS_ACCOUNT.to_owned(),
            beneficiary_account_id: DEFAULT_GENESIS_ACCOUNT.to_owned(),
            signers,
            account_seq,
        };

        let operator = NearToken::from_near(100);
        let gateway_signer_account_id = harness.create_managed_account("gateway", operator).await?;
        let cleanup_signer_account_id = harness.create_managed_account("cleanup", operator).await?;
        let registry_signer_account_id =
            harness.create_managed_account("registry", operator).await?;
        let universal_account_signer_account_id =
            harness.create_managed_account("ua", operator).await?;
        let proxy_oracle_signer_account_id = harness
            .create_managed_account("proxy-oracle", operator)
            .await?;

        let (ft_contract_id, ft_signer) = harness.create_account("mock-ft", operator).await?;
        deploy_contract(
            &harness.network,
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

        let (beneficiary_account_id, _) = harness
            .create_account("beneficiary", NearToken::from_near(25))
            .await?;

        Ok(Self {
            gateway_signer_account_id,
            cleanup_signer_account_id,
            registry_signer_account_id,
            universal_account_signer_account_id,
            proxy_oracle_signer_account_id,
            ft_contract_id,
            beneficiary_account_id,
            ..harness
        })
    }

    /// Create a uniquely-named funded `*.sandbox` sub-account, register its
    /// signer, and return its id plus a signer for it.
    pub(crate) async fn create_account(
        &self,
        label: &str,
        balance: NearToken,
    ) -> Result<(AccountId, Arc<Signer>)> {
        let account_id = self.unique_account_id(label)?;
        let secret_key = test_secret_key()?;
        // Fund and sign with the per-process tenant root, not the genesis key.
        create_funded_account(
            &self.network,
            &self.tenant_root_id,
            &self.tenant_root_signer,
            &account_id,
            &secret_key,
            balance,
        )
        .await?;

        let managed = ManagedSigner::new([secret_key])
            .await
            .with_context(|| format!("failed to initialize {label} signer"))?;
        let signer = managed.signer.clone();
        self.register_signer(ManagedAccountId(account_id.clone()), managed);
        Ok((account_id, signer))
    }

    /// Like [`create_account`](Self::create_account) but returns the
    /// [`ManagedAccountId`] for operator accounts stored on the harness.
    async fn create_managed_account(
        &self,
        label: &str,
        balance: NearToken,
    ) -> Result<ManagedAccountId> {
        let (account_id, _) = self.create_account(label, balance).await?;
        Ok(ManagedAccountId(account_id))
    }

    /// A unique `{label}-{seq}.{tenant_root}` id. The per-harness `seq` keeps
    /// accounts distinct within one process; nesting under the per-process tenant
    /// root keeps them distinct across the parallel test processes that share an
    /// attached node.
    fn unique_account_id(&self, label: &str) -> Result<AccountId> {
        let seq = self.account_seq.fetch_add(1, Ordering::Relaxed);
        format!("{label}-{seq}.{}", self.tenant_root_id)
            .parse()
            .with_context(|| format!("invalid account id for label `{label}`"))
    }

    pub fn gateway_client(&self) -> NearClient {
        NearClient::new(self.network.clone())
    }

    /// Snapshot of the gateway operator signers (and any on-demand accounts) as
    /// the [`ManagedSigner`] map the runtime [`GatewayService`] expects.
    ///
    /// [`GatewayService`]: templar_gateway_runtime
    #[must_use]
    pub fn gateway_signers(&self) -> HashMap<ManagedAccountId, ManagedSigner> {
        self.signers.lock().expect("signers mutex poisoned").clone()
    }

    /// Snapshot of every (account, signer) the harness can sign as.
    pub(crate) fn signers_snapshot(&self) -> HashMap<ManagedAccountId, ManagedSigner> {
        self.signers.lock().expect("signers mutex poisoned").clone()
    }

    /// Register a signer for an on-demand account.
    pub(crate) fn register_signer(&self, account_id: ManagedAccountId, signer: ManagedSigner) {
        self.signers
            .lock()
            .expect("signers mutex poisoned")
            .insert(account_id, signer);
    }

    pub async fn ft_wasm(&self) -> Vec<u8> {
        FtController::wasm().await.to_vec()
    }

    pub async fn deploy_mt(&self, account_id: AccountId) -> Result<AccountId> {
        let (id, signer) = self
            .create_account(label_of(&account_id), NearToken::from_near(100))
            .await?;
        deploy_contract(
            &self.network,
            id.clone(),
            signer,
            test_utils::controller::mt::MtController::wasm()
                .await
                .to_vec(),
            "new",
            serde_json::json!({}),
        )
        .await?;
        Ok(id)
    }

    pub async fn deploy_receiver(&self, account_id: AccountId) -> Result<AccountId> {
        let (id, signer) = self
            .create_account(label_of(&account_id), NearToken::from_near(100))
            .await?;
        deploy_contract(
            &self.network,
            id.clone(),
            signer,
            ReceiverController::wasm().await.to_vec(),
            "new",
            serde_json::json!({}),
        )
        .await?;
        Ok(id)
    }

    pub async fn deploy_ref_finance(
        &self,
        account_id: AccountId,
        pools: Vec<PoolInfo>,
    ) -> Result<AccountId> {
        let (id, signer) = self
            .create_account(label_of(&account_id), NearToken::from_near(100))
            .await?;
        deploy_contract(
            &self.network,
            id.clone(),
            signer,
            RefFinanceController::wasm().await.to_vec(),
            "new",
            serde_json::json!({ "pools": pools }),
        )
        .await?;
        Ok(id)
    }

    pub async fn deploy_registry(&self) -> Result<AccountId> {
        let account_id = self.registry_signer_account_id.0.clone();
        deploy_contract(
            &self.network,
            account_id.clone(),
            account_signer()?,
            RegistryController::wasm().await.to_vec(),
            "new",
            serde_json::json!({}),
        )
        .await?;
        Ok(account_id)
    }

    pub async fn deploy_market(&self) -> Result<(AccountId, MarketConfiguration)> {
        self.deploy_market_with(|_| {}).await
    }

    /// Deploy a market (plus its FT pair and mock oracle), applying `customize`
    /// to the [`MarketConfiguration`] before deployment.
    pub async fn deploy_market_with(
        &self,
        customize: impl FnOnce(&mut MarketConfiguration),
    ) -> Result<(AccountId, MarketConfiguration)> {
        let (oracle_id, oracle_signer) = self
            .create_account("oracle", NearToken::from_near(100))
            .await?;
        deploy_contract(
            &self.network,
            oracle_id.clone(),
            oracle_signer,
            MockOracleController::wasm().await.to_vec(),
            "new",
            serde_json::json!({}),
        )
        .await?;

        self.deploy_market_with_oracle(oracle_id, customize).await
    }

    /// Deploy a market (plus its FT pair) pointing at an existing `oracle_id`
    /// instead of a freshly-deployed mock oracle — e.g. a proxy oracle that
    /// aggregates other oracles. Applies `customize` to the
    /// [`MarketConfiguration`] before deployment.
    pub async fn deploy_market_with_oracle(
        &self,
        oracle_id: AccountId,
        customize: impl FnOnce(&mut MarketConfiguration),
    ) -> Result<(AccountId, MarketConfiguration)> {
        let balance = NearToken::from_near(100);
        let (borrow_asset_id, borrow_signer) = self.create_account("borrow-ft", balance).await?;
        deploy_contract(
            &self.network,
            borrow_asset_id.clone(),
            borrow_signer,
            FtController::wasm().await.to_vec(),
            "new",
            serde_json::json!({ "name": "Borrow FT", "symbol": "BFT" }),
        )
        .await?;

        let (collateral_asset_id, collateral_signer) =
            self.create_account("collateral-ft", balance).await?;
        deploy_contract(
            &self.network,
            collateral_asset_id.clone(),
            collateral_signer,
            FtController::wasm().await.to_vec(),
            "new",
            serde_json::json!({ "name": "Collateral FT", "symbol": "CFT" }),
        )
        .await?;

        let mut configuration = market_configuration(
            oracle_id,
            borrow_asset_id,
            collateral_asset_id,
            self.gateway_signer_account_id.0.clone(),
            YieldWeights::new_with_supply_weight(1),
        );
        customize(&mut configuration);

        let (market_id, market_signer) = self.create_account("market", balance).await?;
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

    /// Deploy the legacy (`0.1.0`, pre-kernelization) proxy oracle wasm, whose
    /// `get_proxy` returns the `v0::Proxy` shape and whose governance is built in.
    pub async fn deploy_legacy_v0_proxy_oracle(&self) -> Result<AccountId> {
        let account_id = self.proxy_oracle_signer_account_id.0.clone();
        let signer = Signer::from_secret_key(test_secret_key()?)
            .context("failed to initialize legacy proxy oracle deploy signer")?;

        deploy_contract(
            &self.network,
            account_id.clone(),
            signer,
            ProxyOracleController::wasm_v0().to_vec(),
            "new",
            serde_json::json!({}),
        )
        .await?;

        Ok(account_id)
    }

    pub async fn deploy_mock_oracle(&self, account_id: AccountId) -> Result<AccountId> {
        let (id, signer) = self
            .create_account(label_of(&account_id), NearToken::from_near(100))
            .await?;
        deploy_contract(
            &self.network,
            id.clone(),
            signer,
            MockOracleController::wasm().await.to_vec(),
            "new",
            serde_json::json!({}),
        )
        .await?;
        Ok(id)
    }

    /// Deploy a standalone mock fungible token (NEP-141) and return its id.
    pub async fn deploy_ft(&self, label: &str, name: &str, symbol: &str) -> Result<AccountId> {
        let (id, signer) = self
            .create_account(label, NearToken::from_near(100))
            .await?;
        deploy_contract(
            &self.network,
            id.clone(),
            signer,
            FtController::wasm().await.to_vec(),
            "new",
            serde_json::json!({ "name": name, "symbol": symbol }),
        )
        .await?;
        Ok(id)
    }

    pub async fn deploy_redstone_adapter(&self, account_id: AccountId) -> Result<AccountId> {
        let (account_id, signer) = self
            .create_account(label_of(&account_id), NearToken::from_near(100))
            .await?;
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
        let (id, signer) = self
            .create_account(label_of(&account_id), NearToken::from_near(100))
            .await?;
        deploy_contract(
            &self.network,
            id.clone(),
            signer,
            LstOracleController::wasm().await.to_vec(),
            "new",
            serde_json::json!({ "oracle_id": oracle_id }),
        )
        .await?;
        Ok(id)
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

    /// Set a proxy definition directly via the owner-gated `admin_set_proxy`
    /// (kernelized `>= 0.2.0` oracle). The oracle account is its own owner after
    /// `deploy_proxy_oracle`, so it signs as itself.
    pub async fn admin_set_proxy(
        &self,
        oracle_id: AccountId,
        price_identifier: PriceIdentifier,
        proxy: Option<Proxy<Source>>,
    ) -> Result<()> {
        let signer = Signer::from_secret_key(test_secret_key()?)
            .context("failed to initialize admin_set_proxy signer")?;
        Contract(oracle_id.clone())
            .call_function(
                "admin_set_proxy",
                serde_json::json!({ "id": price_identifier, "proxy": proxy }),
            )
            .transaction()
            .gas(near_sdk::Gas::from_tgas(100))
            .with_signer(oracle_id, signer)
            .send_to(&self.network)
            .await?
            .assert_success();
        Ok(())
    }

    /// Refresh the proxy oracle's cached prices for `price_ids` by invoking
    /// `update_prices`, which fans out to each proxy's underlying sources and
    /// caches the aggregated result so a subsequent
    /// `list_ema_prices_no_older_than` read sees it. Signed as the oracle
    /// account (permissionless, but the call still needs a signer). Generously
    /// gassed since it triggers a cross-contract fan-out per proxy.
    pub async fn update_proxy_prices(
        &self,
        oracle_id: AccountId,
        price_ids: Vec<PriceIdentifier>,
    ) -> Result<()> {
        let signer = Signer::from_secret_key(test_secret_key()?)
            .context("failed to initialize update_prices signer")?;
        Contract(oracle_id.clone())
            .call_function(
                "update_prices",
                serde_json::json!({ "price_ids": price_ids }),
            )
            .transaction()
            .gas(near_sdk::Gas::from_tgas(300))
            .with_signer(oracle_id, signer)
            .send_to(&self.network)
            .await?
            .assert_success();
        Ok(())
    }

    /// Deploy a governance contract for `oracle_id` (admin = `admin_id`, all TTLs
    /// zero for immediate execution) and transfer oracle ownership to it, so the
    /// governance contract can drive the oracle's `admin_*` methods. Consumes
    /// governance proposal id `0` for the ownership handover. Returns the
    /// governance contract account id.
    pub async fn deploy_governance_contract(
        &self,
        oracle_id: AccountId,
        admin_id: AccountId,
    ) -> Result<AccountId> {
        let (governance_id, deploy_signer) = self
            .create_account("governance", NearToken::from_near(100))
            .await?;
        deploy_contract(
            &self.network,
            governance_id.clone(),
            deploy_signer,
            GovernanceContractController::wasm().await.to_vec(),
            "new",
            serde_json::json!({
                "proxy_oracle_id": oracle_id,
                "admin_id": admin_id,
                "ttls": zero_ttl_config(),
            }),
        )
        .await?;

        // Current owner (the oracle account) proposes the governance contract.
        let owner_signer = Signer::from_secret_key(test_secret_key()?)
            .context("failed to initialize ownership-transfer signer")?;
        Contract(oracle_id.clone())
            .call_function(
                "own_propose_owner",
                serde_json::json!({ "account_id": governance_id }),
            )
            .transaction()
            .deposit(NearToken::from_yoctonear(1))
            .gas(near_sdk::Gas::from_tgas(50))
            .with_signer(oracle_id.clone(), owner_signer)
            .send_to(&self.network)
            .await?
            .assert_success();

        // Governance accepts ownership via an AdminFunctionCall proposal (id 0),
        // which fires `own_accept_owner` on the oracle as the governance contract.
        self.governance_admin_function_call(&governance_id, &admin_id, 0, "own_accept_owner")
            .await?;

        Ok(governance_id)
    }

    /// Create and immediately execute an `AdminFunctionCall` governance proposal
    /// that calls `method_name` (no args, 1 yocto) on the proxy oracle.
    async fn governance_admin_function_call(
        &self,
        governance_id: &AccountId,
        admin_id: &AccountId,
        proposal_id: u32,
        method_name: &str,
    ) -> Result<()> {
        let operation = Operation::AdminFunctionCall {
            method_name: method_name.to_string(),
            args: near_sdk::json_types::Base64VecU8(b"{}".to_vec()),
            attached_deposit: near_sdk::json_types::U128(1),
            gas: near_sdk::Gas::from_tgas(50),
        };
        let admin_signer = Signer::from_secret_key(test_secret_key()?)
            .context("failed to initialize governance admin signer")?;

        Contract(governance_id.clone())
            .call_function(
                "create_proposal",
                serde_json::json!({
                    "id": proposal_id,
                    "operation": operation,
                    "requested_ttl": Nanoseconds::zero(),
                }),
            )
            .transaction()
            .deposit(NearToken::from_yoctonear(1))
            .gas(near_sdk::Gas::from_tgas(100))
            .with_signer(admin_id.clone(), admin_signer.clone())
            .send_to(&self.network)
            .await?
            .assert_success();

        Contract(governance_id.clone())
            .call_function("execute_proposal", serde_json::json!({ "id": proposal_id }))
            .transaction()
            .deposit(NearToken::from_yoctonear(1))
            .gas(near_sdk::Gas::from_tgas(100))
            .with_signer(admin_id.clone(), admin_signer)
            .send_to(&self.network)
            .await?
            .assert_success();

        Ok(())
    }

    /// Seed a proxy on a legacy (`< 0.2.0`) oracle, whose only path to set a
    /// proxy is its built-in, owner-gated governance (`gov_create` + `gov_execute`
    /// with a TTL of zero). The oracle account is its own owner.
    pub async fn seed_legacy_v0_proxy(
        &self,
        oracle_id: AccountId,
        price_identifier: PriceIdentifier,
        proxy: v0::Proxy,
    ) -> Result<()> {
        let owner_signer = Signer::from_secret_key(test_secret_key()?)
            .context("failed to initialize legacy proxy signer")?;
        let operation = v0::Operation::SetProxy {
            id: price_identifier,
            proxy: Some(proxy),
        };

        Contract(oracle_id.clone())
            .call_function(
                "gov_create",
                serde_json::json!({ "id": 0, "operation": operation }),
            )
            .transaction()
            .deposit(NearToken::from_yoctonear(1))
            .gas(near_sdk::Gas::from_tgas(100))
            .with_signer(oracle_id.clone(), owner_signer.clone())
            .send_to(&self.network)
            .await?
            .assert_success();

        Contract(oracle_id.clone())
            .call_function("gov_execute", serde_json::json!({ "id": 0 }))
            .transaction()
            .deposit(NearToken::from_yoctonear(1))
            .gas(near_sdk::Gas::from_tgas(100))
            .with_signer(oracle_id, owner_signer)
            .send_to(&self.network)
            .await?
            .assert_success();

        Ok(())
    }
}

fn zero_ttl_config() -> TtlConfig {
    let zero = Nanoseconds::zero();
    TtlConfig {
        set_proxy: zero,
        configure_circuit_breakers: zero,
        add_circuit_breaker: zero,
        remove_circuit_breaker: zero,
        set_manual_trip: zero,
        rearm: zero,
        set_enforced: zero,
        set_action_ttl: zero,
        set_role: zero,
        admin_upgrade: zero,
        admin_function_call: zero,
    }
}

/// Choose the harness mode from the environment. `NEAR_SANDBOX_RPC_URL` set →
/// attach to an out-of-band node (no owned `Sandbox`); otherwise launch one.
async fn connect() -> Result<(Option<Sandbox>, NetworkConfig)> {
    if let Some(rpc_url) = attach_rpc_url()? {
        let network = NetworkConfig::from_rpc_url(
            "sandbox",
            rpc_url
                .parse()
                .with_context(|| format!("invalid sandbox RPC url: {rpc_url}"))?,
        );
        Ok((None, network))
    } else {
        let sandbox = Sandbox::start_sandbox_with_config(sandbox_config()).await?;
        let network = NetworkConfig::from_rpc_url("sandbox", sandbox.rpc_addr.parse()?);
        Ok((Some(sandbox), network))
    }
}

/// The RPC url to attach to in attach mode, or `None` for owned mode.
///
/// Under the nextest `sandbox` profile the setup script starts a pool of nodes
/// and exports `NEAR_SANDBOX_RPC_URL_<i>` per node. A test reads its
/// `NEXTEST_TEST_GLOBAL_SLOT` and attaches to that slot's node, giving it
/// exclusive use of it — so `fast_forward` and chain state stay isolated from
/// other concurrently-running tests, which one shared node could not guarantee.
/// Falls back to the single `NEAR_SANDBOX_RPC_URL` for manual/non-nextest runs.
fn attach_rpc_url() -> Result<Option<String>> {
    if let Ok(slot) = std::env::var("NEXTEST_TEST_GLOBAL_SLOT") {
        let var = format!("NEAR_SANDBOX_RPC_URL_{slot}");
        if let Ok(url) = std::env::var(&var) {
            return Ok(Some(url));
        }
        // In a pooled run every slot must map to a node; a missing one means the
        // pool is smaller than the profile's test-threads — fail loudly rather
        // than silently sharing a node (which would break time isolation).
        if std::env::var("NEAR_SANDBOX_RPC_URL_0").is_ok() {
            anyhow::bail!(
                "no pooled sandbox node for slot {slot} ({var} unset): \
                 SANDBOX_NODE_COUNT must be >= the sandbox profile's test-threads"
            );
        }
    }
    Ok(std::env::var("NEAR_SANDBOX_RPC_URL").ok())
}

/// The high-balance genesis account every harness funds its accounts from.
///
/// The default genesis `sandbox` account holds only 10_000 NEAR — a long run
/// against one shared node exhausts it, because each test locks funds in
/// accounts that outlive it. This account is seeded with a very large balance so
/// the shared node never runs dry. It reuses the default genesis keypair, so the
/// existing genesis signer can sign for it.
pub(crate) const FUNDER_ACCOUNT_ID: &str = "funder";

/// Sandbox launch config shared by owned mode ([`connect`]) and the out-of-band
/// host (`bin/sandbox-host.rs`), so both nodes seed the [`FUNDER_ACCOUNT_ID`]
/// account identically.
#[must_use]
pub fn sandbox_config() -> SandboxConfig {
    SandboxConfig {
        additional_accounts: vec![GenesisAccount {
            account_id: FUNDER_ACCOUNT_ID
                .parse()
                .expect("funder account id is valid"),
            public_key: DEFAULT_GENESIS_ACCOUNT_PUBLIC_KEY.to_string(),
            private_key: DEFAULT_GENESIS_ACCOUNT_PRIVATE_KEY.to_string(),
            balance: NearToken::from_near(100_000_000),
        }],
        ..SandboxConfig::default()
    }
}

/// Create `account_id` as a sub-account of `funder_id`, funded with `balance`
/// and a full-access key derived from `secret_key`, signed by `funder_signer`.
///
/// Working accounts are funded by the per-process tenant root, whose key nonce
/// is touched only by this process — so there is no cross-process contention
/// here and no retry is needed (cf. [`create_tenant_root`], the single
/// genesis-signed creation per process).
async fn create_funded_account(
    network: &NetworkConfig,
    funder_id: &AccountId,
    funder_signer: &Arc<Signer>,
    account_id: &AccountId,
    secret_key: &SecretKey,
    balance: NearToken,
) -> Result<()> {
    Account::create_account(account_id.clone())
        .fund_myself(funder_id.clone(), balance)
        .with_public_key(secret_key.public_key())
        .with_signer(funder_signer.clone())
        .send_to(network)
        .await
        .with_context(|| format!("failed to create account {account_id}"))?
        .assert_success();
    Ok(())
}

/// Create this process's intermediate root account from the genesis key, and
/// return it with a signer over its own key.
///
/// This is the *only* genesis-signed transaction per test process, and thus the
/// only point of cross-process nonce contention on the shared genesis key: many
/// processes touch that one key, and a process can read a nonce that does not
/// yet reflect another process's just-submitted creation, surfacing as
/// `InvalidNonce`/`InvalidTransaction`. Such a transaction is rejected at
/// submission (it never enters the mempool, so it cannot pile up as a pending
/// tx); we simply re-issue it a few times, rebuilding the genesis signer so its
/// nonce cache re-queries the chain. Every *other* account this process creates
/// is funded by the returned tenant root and needs no retry.
async fn create_tenant_root(
    network: &NetworkConfig,
    genesis_signer: &Arc<Signer>,
) -> Result<(AccountId, Arc<Signer>)> {
    static TENANT_SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = TENANT_SEQ.fetch_add(1, Ordering::Relaxed);
    let account_id: AccountId = format!("t{}-{seq}.{FUNDER_ACCOUNT_ID}", std::process::id())
        .parse()
        .context("invalid tenant root id")?;
    let funder_id: AccountId = FUNDER_ACCOUNT_ID.parse().context("invalid funder id")?;
    let secret_key = test_secret_key()?;
    let public_key = secret_key.public_key();

    const MAX_ATTEMPTS: u32 = 5;
    for attempt in 1..=MAX_ATTEMPTS {
        // First attempt reuses the passed signer; retries rebuild it so its
        // nonce cache re-queries the chain after the contending tx finalized.
        let signer = if attempt == 1 {
            genesis_signer.clone()
        } else {
            Signer::from_secret_key(genesis_secret_key()?)
                .context("failed to rebuild genesis signer")?
        };
        let result = Account::create_account(account_id.clone())
            .fund_myself(funder_id.clone(), NearToken::from_near(5_000))
            .with_public_key(public_key.clone())
            .with_signer(signer)
            .send_to(network)
            .await;

        match result {
            Ok(outcome) => {
                outcome.assert_success();
                let tenant_signer = Signer::from_secret_key(secret_key)
                    .context("failed to initialize tenant root signer")?;
                return Ok((account_id, tenant_signer));
            }
            Err(error) => {
                let message = error.to_string();
                let retriable = message.contains("InvalidNonce")
                    || message.contains("InvalidTransaction")
                    || message.contains("nonce")
                    || message.contains("Expired");
                if attempt == MAX_ATTEMPTS || !retriable {
                    return Err(anyhow::Error::new(error)
                        .context(format!("failed to create tenant root {account_id}")));
                }
                tokio::time::sleep(Duration::from_millis(250 * u64::from(attempt))).await;
            }
        }
    }
    unreachable!("create_tenant_root loop always returns")
}

/// The genesis root account's secret key (deterministic across sandbox runs).
fn genesis_secret_key() -> Result<SecretKey> {
    DEFAULT_GENESIS_ACCOUNT_PRIVATE_KEY
        .parse()
        .context("failed to parse genesis private key")
}

/// A signer over the deterministic test key, valid for any harness-created
/// account (they all share that key).
fn account_signer() -> Result<Arc<Signer>> {
    Signer::from_secret_key(test_secret_key()?).context("failed to initialize account signer")
}

/// The label segment of a requested id (e.g. `kind-pyth.near` → `kind-pyth`),
/// used to namespace caller-supplied contract ids into the harness instance.
fn label_of(account_id: &AccountId) -> &str {
    account_id.as_str().split('.').next().unwrap_or("account")
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

pub(crate) fn test_secret_key() -> Result<SecretKey> {
    Ok("ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q"
        .parse()?)
}
