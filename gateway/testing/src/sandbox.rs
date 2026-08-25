use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, LazyLock, Mutex,
    },
    time::Duration,
};

use anyhow::{Context, Result};
use near_api::{
    types::{AccountId, CryptoHash},
    Account, Contract, NetworkConfig, SecretKey, Signer,
};
use near_sandbox::{
    config::{
        DEFAULT_GENESIS_ACCOUNT, DEFAULT_GENESIS_ACCOUNT_PRIVATE_KEY,
        DEFAULT_GENESIS_ACCOUNT_PUBLIC_KEY,
    },
    GenesisAccount, Sandbox, SandboxConfig,
};
use near_token::NearToken;
use templar_common::{
    asset::FungibleAsset,
    market::{MarketConfiguration, YieldWeights},
    oracle::{pyth::PriceIdentifier, redstone::config as redstone_config},
    vault::VaultConfiguration,
    Nanoseconds,
};
use templar_gateway_core::{NearClient, PooledSigner};
use templar_gateway_methods_spec::{
    lst_oracle, owner, proxy_oracle, proxy_oracle_governance as gov,
};
use templar_gateway_types::{ManagedAccountId, ProposalEncoding};
use templar_proxy_oracle_kernel::proxy::Proxy;
use templar_proxy_oracle_near_common::{
    input::Source, price_transformer::PriceTransformer, state::legacy::v0,
};
use templar_proxy_oracle_near_governance_common::{FunctionCall, GovernancePolicy, Operation};
use templar_pyth_lazer_adapter_contract::{ConfigArgs, TrustedSigner};
use templar_universal_account::{InitArgs, NEAR_TESTNET_CHAIN_ID};
use test_utils::{market_configuration, test_signer::TestSigner, vault_configuration};

use crate::{wasm::PoolInfo, TEST_FINALITY_POLICY};

/// The two token ids the mock NEP-245 contract (`crate::wasm::mt`) pre-creates
/// in its `new`; a market's MT borrow/collateral asset must reference these.
const MT_BORROW_TOKEN_ID: &str = "mt_borrow";
const MT_COLLATERAL_TOKEN_ID: &str = "mt_collateral";

/// Every `deploy_*` helper mints its own account and returns the id it actually
/// created. The caller cannot name it: in attached mode accounts are generated
/// sub-accounts of the sandbox root, so a caller-chosen id would address an
/// account that does not exist.
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
    signers: Mutex<HashMap<ManagedAccountId, PooledSigner>>,
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
        Self::start_on(connect().await?).await
    }

    /// Start on a **dedicated** `neard`, ignoring `NEAR_SANDBOX_RPC_URL`.
    ///
    /// A pooled node's chain clock runs ahead of wall-clock time once any test
    /// has `fast_forward`ed it (see [`SandboxHarness::chain_timestamp`]), which
    /// trips the relayer's universal-account `create` route: it ages the block
    /// reference with `SystemTime::elapsed()`, which is `Err` for a
    /// future-stamped block, and reports that as "too old".
    ///
    /// That check is *wrong* — a host whose clock merely lags the chain hits it
    /// too — but fixing it is a production change tracked in ENG-473. Until then
    /// the two tests that exercise the route need a pristine node.
    pub async fn start_owned() -> Result<Self> {
        Self::start_on(start_owned_node().await?).await
    }

    async fn start_on((sandbox, network): (Option<Sandbox>, NetworkConfig)) -> Result<Self> {
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

        // Every account this harness starts with, minted in one patch: the cost
        // is per `sandbox_patch_state` call, not per account.
        let operator = NearToken::from_near(100);
        let [gateway, cleanup, registry, universal_account, proxy_oracle, (ft_contract_id, ft_signer), (beneficiary_account_id, _)] =
            harness
                .create_accounts(&[
                    ("gateway", operator),
                    ("cleanup", operator),
                    ("registry", operator),
                    ("ua", operator),
                    ("proxy-oracle", operator),
                    ("mock-ft", operator),
                    ("beneficiary", NearToken::from_near(25)),
                ])
                .await?
                .try_into()
                .map_err(|_| {
                    anyhow::anyhow!("create_accounts returned the wrong number of accounts")
                })?;
        let gateway_signer_account_id = ManagedAccountId(gateway.0);
        let cleanup_signer_account_id = ManagedAccountId(cleanup.0);
        let registry_signer_account_id = ManagedAccountId(registry.0);
        let universal_account_signer_account_id = ManagedAccountId(universal_account.0);
        let proxy_oracle_signer_account_id = ManagedAccountId(proxy_oracle.0);

        deploy_contract(
            &harness.network,
            ft_contract_id.clone(),
            ft_signer,
            crate::wasm::ft().await.to_vec(),
            "new",
            serde_json::json!({
                "name": "Mock FT",
                "symbol": "MFT",
            }),
        )
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
    ///
    /// The account and its full-access key are minted directly into chain state
    /// via `sandbox_patch_state` — instant, zero blocks. For a test that asserts
    /// on account-creation behavior itself, use
    /// [`create_account_via_tx`](Self::create_account_via_tx), which creates the
    /// account with a real transaction.
    pub(crate) async fn create_account(
        &self,
        label: &str,
        balance: NearToken,
    ) -> Result<(AccountId, Arc<Signer>)> {
        let mut accounts = self.create_accounts(&[(label, balance)]).await?;
        accounts
            .pop()
            .context("create_accounts returned no account")
    }

    /// [`create_account`](Self::create_account) for several accounts at once,
    /// returning them in the order they were requested.
    ///
    /// One `sandbox_patch_state` call costs the same whether it carries one
    /// account or twenty, so every fixture that needs a set of accounts should
    /// mint them together rather than in a loop.
    pub(crate) async fn create_accounts(
        &self,
        accounts: &[(&str, NearToken)],
    ) -> Result<Vec<(AccountId, Arc<Signer>)>> {
        let accounts = accounts
            .iter()
            .map(|(label, balance)| Ok((self.unique_account_id(label)?, *balance)))
            .collect::<Result<Vec<_>>>()?;
        crate::sandbox_ext::create_accounts(&self.network, &accounts, &test_secret_key()?).await?;
        let mut registered = Vec::with_capacity(accounts.len());
        for (account_id, _) in accounts {
            registered.push(self.register_account(account_id).await);
        }
        Ok(registered)
    }

    /// Like `create_account` but mints the account with a
    /// real `create_account` transaction funded and signed by the per-process
    /// tenant root (not the genesis key). Kept for tests that assert on
    /// account-creation behavior; the patch-based `create_account` is the
    /// default and is far faster.
    pub async fn create_account_via_tx(
        &self,
        label: &str,
        balance: NearToken,
    ) -> Result<(AccountId, Arc<Signer>)> {
        let account_id = self.unique_account_id(label)?;
        let secret_key = test_secret_key()?;
        create_funded_account(
            &self.network,
            &self.tenant_root_id,
            &self.tenant_root_signer,
            &account_id,
            &secret_key,
            balance,
        )
        .await?;
        Ok(self.register_account(account_id).await)
    }

    /// Register a freshly-created account on the shared signer. near-api caches
    /// nonces per `(account_id, public_key)`, so one signer safely covers every
    /// harness account and preserves nonce continuity across gateway and raw
    /// optimistic test transactions.
    async fn register_account(&self, account_id: AccountId) -> (AccountId, Arc<Signer>) {
        let managed = ManagedAccountId(account_id.clone());
        self.register_signer(managed.clone(), test_pooled_signer(managed).await);
        (account_id, test_signer())
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
        NearClient::with_finality_policy(self.network.clone(), TEST_FINALITY_POLICY)
    }

    /// Snapshot of the gateway operator signers (and any on-demand accounts) as
    /// the [`PooledSigner`] map the runtime [`GatewayService`] expects.
    ///
    /// [`GatewayService`]: templar_gateway_runtime
    #[must_use]
    pub fn gateway_signers(&self) -> HashMap<ManagedAccountId, PooledSigner> {
        self.signers.lock().expect("signers mutex poisoned").clone()
    }

    /// Snapshot of every (account, signer) the harness can sign as.
    pub(crate) fn signers_snapshot(&self) -> HashMap<ManagedAccountId, PooledSigner> {
        self.gateway_signers()
    }

    /// Register a signer for an on-demand account.
    pub(crate) fn register_signer(&self, account_id: ManagedAccountId, signer: PooledSigner) {
        self.signers
            .lock()
            .expect("signers mutex poisoned")
            .insert(account_id, signer);
    }

    pub async fn ft_wasm(&self) -> Vec<u8> {
        crate::wasm::ft().await.to_vec()
    }
    pub async fn deploy_global_contract(&self, code: Vec<u8>) -> Result<CryptoHash> {
        let hash = CryptoHash::hash(&code);
        Contract::deploy_global_contract_code(code)
            .as_hash()
            .with_signer(self.tenant_root_id.clone(), self.tenant_root_signer.clone())
            .wait_until(TEST_FINALITY_POLICY.transaction_status())
            .send_to(&self.network)
            .await?
            .assert_success();
        Ok(hash)
    }

    pub async fn deploy_mt(&self, label: &str) -> Result<AccountId> {
        let (id, signer) = self
            .create_account(label, NearToken::from_near(100))
            .await?;
        deploy_contract(
            &self.network,
            id.clone(),
            signer,
            crate::wasm::mt().await.to_vec(),
            "new",
            serde_json::json!({}),
        )
        .await?;
        Ok(id)
    }

    pub async fn deploy_receiver(&self, label: &str) -> Result<AccountId> {
        let (id, signer) = self
            .create_account(label, NearToken::from_near(100))
            .await?;
        deploy_contract(
            &self.network,
            id.clone(),
            signer,
            crate::wasm::receiver().await.to_vec(),
            "new",
            serde_json::json!({}),
        )
        .await?;
        Ok(id)
    }

    pub async fn deploy_ref_finance(&self, label: &str, pools: Vec<PoolInfo>) -> Result<AccountId> {
        let (id, signer) = self
            .create_account(label, NearToken::from_near(100))
            .await?;
        deploy_contract(
            &self.network,
            id.clone(),
            signer,
            crate::wasm::ref_finance().await.to_vec(),
            "new",
            serde_json::json!({ "pools": pools }),
        )
        .await?;
        Ok(id)
    }

    pub async fn deploy_registry(&self) -> Result<AccountId> {
        self.deploy_registry_code(crate::wasm::registry().await.to_vec())
            .await
    }

    /// Deploy a *released* registry, for migration tests that have to start from the binary a
    /// live registry is actually running rather than from current source.
    pub async fn deploy_registry_version(&self, version: &str) -> Result<AccountId> {
        let code = crate::wasm::released(crate::ArtifactId::Registry, version).await;
        self.deploy_registry_code(code).await
    }

    async fn deploy_registry_code(&self, code: Vec<u8>) -> Result<AccountId> {
        let account_id = self.registry_signer_account_id.0.clone();
        deploy_contract(
            &self.network,
            account_id.clone(),
            test_signer(),
            code,
            "new",
            serde_json::json!({}),
        )
        .await?;
        Ok(account_id)
    }

    /// Replace the registry's code and run `migrate` in the same transaction, signed by a
    /// full-access key on the registry itself. Returns the gas the batch burnt.
    ///
    /// How a registry predating the `upgrade` method has to be upgraded — it has no such method,
    /// so the batch an operator signs is the only route. `migrate` is reachable because it admits
    /// its own account, which is what a full-access key on that account signs as.
    ///
    /// `gas` is on the `migrate` action, where the signer sets it directly rather than a contract
    /// constant carving it out — the reason this path can afford a migration `upgrade` could not.
    /// A sandbox caps a transaction well below mainnet, so a burn measured here is a lower bound
    /// on what is affordable, not on what is needed.
    pub async fn redeploy_registry_with_migrate(
        &self,
        code: Vec<u8>,
        migrate_args: impl serde::Serialize,
        gas: near_api::types::NearGas,
    ) -> Result<near_api::types::NearGas> {
        let result = self
            .try_deploy_and_init_with_gas(
                &self.registry_signer_account_id.0.clone(),
                code,
                templar_common::upgrade::MIGRATE_METHOD,
                migrate_args,
                gas,
            )
            .await?;
        // Raised to an `Err` rather than returned: a migration that must fail — a mismatched one —
        // is a case worth testing, and every caller here expects the batch to have landed.
        anyhow::ensure!(
            result.operation.status == templar_gateway_types::operation::OperationStatus::Succeeded,
            "registry deploy+migrate failed: {}",
            result
                .operation
                .failure_message()
                .unwrap_or("<no failure message>"),
        );
        Ok(near_api::types::NearGas::from_gas(
            self.operation_gas_burnt(&result),
        ))
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
        self.deploy_market_std(false, false, customize).await
    }

    /// [`deploy_market_with`](Self::deploy_market_with) but with each asset
    /// deployed as a NEP-141 fungible token or a NEP-245 multi-token, per
    /// `borrow_mt`/`collateral_mt` — exercises the standard-agnostic asset path.
    pub async fn deploy_market_std(
        &self,
        borrow_mt: bool,
        collateral_mt: bool,
        customize: impl FnOnce(&mut MarketConfiguration),
    ) -> Result<(AccountId, MarketConfiguration)> {
        self.deploy_market_parts(None, borrow_mt, collateral_mt, customize)
            .await
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
        self.deploy_market_with_oracle_std(oracle_id, false, false, customize)
            .await
    }

    /// [`deploy_market_with_oracle`](Self::deploy_market_with_oracle) with each
    /// asset deployed as a NEP-141 token or a NEP-245 multi-token per
    /// `borrow_mt`/`collateral_mt`.
    pub async fn deploy_market_with_oracle_std(
        &self,
        oracle_id: AccountId,
        borrow_mt: bool,
        collateral_mt: bool,
        customize: impl FnOnce(&mut MarketConfiguration),
    ) -> Result<(AccountId, MarketConfiguration)> {
        self.deploy_market_parts(Some(oracle_id), borrow_mt, collateral_mt, customize)
            .await
    }

    /// The market fixture: every account it needs minted in one patch, then all
    /// of its contracts deployed at once.
    ///
    /// The mock oracle (minted here unless `oracle_id` names an existing one),
    /// the borrow/collateral asset pair and the market are independent deploys —
    /// the market's `new` only records the asset and oracle *ids*, it never calls
    /// them — so nothing here has to wait on anything else.
    async fn deploy_market_parts(
        &self,
        oracle_id: Option<AccountId>,
        borrow_mt: bool,
        collateral_mt: bool,
        customize: impl FnOnce(&mut MarketConfiguration),
    ) -> Result<(AccountId, MarketConfiguration)> {
        let operator = NearToken::from_near(100);
        let mut labels = Vec::with_capacity(4);
        if oracle_id.is_none() {
            labels.push(("oracle", operator));
        }
        labels.extend([
            ("borrow-ft", operator),
            ("collateral-ft", operator),
            ("market", operator),
        ]);
        let mut minted = self.create_accounts(&labels).await?.into_iter();
        let mut next = |what: &str| {
            minted
                .next()
                .with_context(|| format!("no minted account for the market's {what}"))
        };
        // A caller-supplied oracle is used as-is; otherwise the first minted
        // account becomes a fresh mock oracle, deployed alongside the rest below.
        let (oracle_id, fresh_oracle) = if let Some(oracle_id) = oracle_id {
            (oracle_id, None)
        } else {
            let (oracle_id, signer) = next("oracle")?;
            (oracle_id.clone(), Some((oracle_id, signer)))
        };
        let (borrow_asset_id, borrow_signer) = next("borrow asset")?;
        let (collateral_asset_id, collateral_signer) = next("collateral asset")?;
        let (market_id, market_signer) = next("market")?;

        let mut configuration = market_configuration(
            oracle_id,
            borrow_asset_id.clone(),
            collateral_asset_id.clone(),
            self.gateway_signer_account_id.0.clone(),
            YieldWeights::new_with_supply_weight(1),
        );
        // `market_configuration` wraps both ids as NEP-141; re-wrap the MT ones
        // as NEP-245 referencing the mock's pre-created token ids.
        if borrow_mt {
            configuration.borrow_asset =
                FungibleAsset::nep245(borrow_asset_id.clone(), MT_BORROW_TOKEN_ID.to_owned());
        }
        if collateral_mt {
            configuration.collateral_asset = FungibleAsset::nep245(
                collateral_asset_id.clone(),
                MT_COLLATERAL_TOKEN_ID.to_owned(),
            );
        }
        customize(&mut configuration);

        futures::try_join!(
            async {
                match fresh_oracle {
                    Some((oracle_id, oracle_signer)) => {
                        deploy_contract(
                            &self.network,
                            oracle_id,
                            oracle_signer,
                            crate::wasm::mock_oracle().await.to_vec(),
                            "new",
                            serde_json::json!({}),
                        )
                        .await
                    }
                    None => Ok(()),
                }
            },
            self.deploy_market_asset(
                borrow_asset_id,
                borrow_signer,
                "Borrow FT",
                "BFT",
                borrow_mt,
            ),
            self.deploy_market_asset(
                collateral_asset_id,
                collateral_signer,
                "Collateral FT",
                "CFT",
                collateral_mt,
            ),
            deploy_contract(
                &self.network,
                market_id.clone(),
                market_signer,
                crate::wasm::market().await.to_vec(),
                "new",
                serde_json::json!({
                    "configuration": configuration.clone(),
                }),
            ),
        )?;

        Ok((market_id, configuration))
    }

    /// Deploy a market asset onto an already-minted account: a NEP-245
    /// multi-token when `mt`, else a NEP-141 fungible token.
    async fn deploy_market_asset(
        &self,
        account_id: AccountId,
        signer: Arc<Signer>,
        name: &str,
        symbol: &str,
        mt: bool,
    ) -> Result<()> {
        let (code, init_args) = if mt {
            (crate::wasm::mt().await.to_vec(), serde_json::json!({}))
        } else {
            (
                crate::wasm::ft().await.to_vec(),
                serde_json::json!({ "name": name, "symbol": symbol }),
            )
        };
        deploy_contract(&self.network, account_id, signer, code, "new", init_args).await
    }

    pub async fn deploy_vault(&self) -> Result<(AccountId, VaultConfiguration)> {
        let (vault_id, signer) = self
            .create_account("vault", NearToken::from_near(100))
            .await?;
        let configuration = vault_configuration(
            self.gateway_signer_account_id.0.clone(),
            self.gateway_signer_account_id.0.clone(),
            self.gateway_signer_account_id.0.clone(),
            self.gateway_signer_account_id.0.clone(),
            self.ft_contract_id.clone(),
            self.beneficiary_account_id.clone(),
            self.beneficiary_account_id.clone(),
        );

        deploy_contract(
            &self.network,
            vault_id.clone(),
            signer,
            crate::wasm::vault().await.to_vec(),
            "new",
            serde_json::json!({
                "configuration": configuration.clone(),
            }),
        )
        .await?;

        Ok((vault_id, configuration))
    }

    pub async fn deploy_universal_account(&self) -> Result<(AccountId, TestSigner)> {
        let account_id = self.universal_account_signer_account_id.0.clone();
        let signer = test_signer();

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
            crate::wasm::universal_account().await.to_vec(),
            "new",
            &init,
        )
        .await?;

        Ok((account_id, test_signer))
    }

    pub async fn deploy_proxy_oracle(&self) -> Result<AccountId> {
        let account_id = self.proxy_oracle_signer_account_id.0.clone();
        let signer = test_signer();

        deploy_contract(
            &self.network,
            account_id.clone(),
            signer,
            crate::wasm::proxy_oracle().await.to_vec(),
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
        let signer = test_signer();

        deploy_contract(
            &self.network,
            account_id.clone(),
            signer,
            crate::wasm::released(crate::ArtifactId::ProxyOracle, "0.1.0").await,
            "new",
            serde_json::json!({}),
        )
        .await?;

        Ok(account_id)
    }

    pub async fn deploy_mock_oracle(&self, label: &str) -> Result<AccountId> {
        let (id, signer) = self
            .create_account(label, NearToken::from_near(100))
            .await?;
        deploy_contract(
            &self.network,
            id.clone(),
            signer,
            crate::wasm::mock_oracle().await.to_vec(),
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
            crate::wasm::ft().await.to_vec(),
            "new",
            serde_json::json!({ "name": name, "symbol": symbol }),
        )
        .await?;
        Ok(id)
    }

    pub async fn deploy_redstone_adapter(&self, label: &str) -> Result<AccountId> {
        let (account_id, signer) = self
            .create_account(label, NearToken::from_near(100))
            .await?;
        let mut config = redstone_config::prod();
        config.max_timestamp_delay_ms = u64::MAX;
        config.max_timestamp_ahead_ms = u64::MAX;
        config.min_interval_between_updates_ms = 0;
        deploy_contract(
            &self.network,
            account_id.clone(),
            signer,
            crate::wasm::redstone_adapter().await.to_vec(),
            "new",
            serde_json::json!({
                "config": config,
                "admin_id": account_id,
            }),
        )
        .await?;
        Ok(account_id)
    }

    /// Deploy a Pyth Lazer adapter. The adapter is Lazer-native and feed-id-addressed; it
    /// is consumed by wrapping it in a proxy oracle as a `Lazer` source (by feed id), not by
    /// targeting it directly — tests use this to stand one up behind a proxy or to assert a bare
    /// adapter is rejected as a standalone oracle. The adapter owns itself (so the harness signer
    /// drives `admin_*`); the trusted signer is a throwaway key — gateway plans against it are
    /// inspected, not submitted, so no payload is verified.
    pub async fn deploy_pyth_lazer_adapter(&self, label: &str) -> Result<AccountId> {
        let (account_id, signer) = self
            .create_account(label, NearToken::from_near(100))
            .await?;
        let config = ConfigArgs {
            // The adapter never verifies a payload in these plan-only tests (plans are
            // inspected, not submitted), so any well-formed 32-byte key works.
            signers: vec![TrustedSigner {
                public_key: [7u8; 32],
                expires_at_s: u64::MAX,
            }],
            max_timestamp_delay_s: 600,
            max_timestamp_ahead_s: 600,
            allowed_channel_id: Some(1),
            update_fee: NearToken::from_yoctonear(0),
            max_feeds_per_update: 64,
        };
        deploy_contract(
            &self.network,
            account_id.clone(),
            signer,
            crate::wasm::pyth_lazer_adapter().await.to_vec(),
            "new",
            serde_json::json!({ "owner": account_id, "config": config }),
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
        self.call_function(
            &ManagedAccountId(oracle_id.clone()),
            &oracle_id,
            "set_pyth_price",
            SetPythPriceArgs {
                price_identifier,
                price,
            },
        )
        .await?;
        Ok(())
    }

    pub async fn set_mock_oracle_redstone_price(
        &self,
        oracle_id: AccountId,
        feed_id: templar_common::oracle::redstone::FeedId,
        data: Option<templar_common::oracle::redstone::FeedData>,
    ) -> Result<()> {
        self.call_function(
            &ManagedAccountId(oracle_id.clone()),
            &oracle_id,
            "set_redstone_price",
            SetRedstonePriceArgs { feed_id, data },
        )
        .await?;
        Ok(())
    }

    pub async fn set_mock_oracle_lazer_price(
        &self,
        oracle_id: AccountId,
        feed_id: u32,
        data: Option<templar_common::oracle::lazer::FeedData>,
    ) -> Result<()> {
        self.call_function(
            &ManagedAccountId(oracle_id.clone()),
            &oracle_id,
            "set_lazer_price",
            SetLazerPriceArgs { feed_id, data },
        )
        .await?;
        Ok(())
    }

    pub async fn deploy_lst_oracle(&self, label: &str, oracle_id: AccountId) -> Result<AccountId> {
        let (id, signer) = self
            .create_account(label, NearToken::from_near(100))
            .await?;
        deploy_contract(
            &self.network,
            id.clone(),
            signer,
            crate::wasm::lst_oracle().await.to_vec(),
            "new",
            serde_json::json!({ "oracle_id": oracle_id }),
        )
        .await?;
        Ok(id)
    }

    /// Create an LST price transformer via the owner-gated `create_transformer`.
    /// The oracle account owns itself after `deploy_lst_oracle`, so it signs as
    /// itself.
    pub async fn create_lst_transformer(
        &self,
        oracle_id: AccountId,
        price_identifier: PriceIdentifier,
        entry: PriceTransformer,
    ) -> Result<()> {
        self.execute(
            &ManagedAccountId(oracle_id.clone()),
            lst_oracle::CreateTransformer {
                oracle_id,
                price_identifier,
                entry,
            },
        )
        .await?;
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
        self.execute(
            &ManagedAccountId(oracle_id.clone()),
            proxy_oracle::AdminSetProxy {
                oracle_id,
                id: price_identifier,
                proxy,
            },
        )
        .await?;
        Ok(())
    }

    /// Refresh the proxy oracle's cached prices for `price_ids` by invoking
    /// `update_prices`, which fans out to each proxy's underlying sources and
    /// caches the aggregated result so a subsequent
    /// `list_ema_prices_no_older_than` read sees it. Signed as the oracle
    /// account (permissionless, but the call still needs a signer).
    pub async fn update_proxy_prices(
        &self,
        oracle_id: AccountId,
        price_ids: Vec<PriceIdentifier>,
    ) -> Result<()> {
        self.execute(
            &ManagedAccountId(oracle_id.clone()),
            proxy_oracle::UpdatePrices {
                oracle_id,
                price_ids,
            },
        )
        .await?;
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
            crate::wasm::proxy_governance().await.to_vec(),
            "new",
            serde_json::json!({
                "proxy_oracle_id": oracle_id,
                "admin_id": admin_id,
                "policy": zero_governance_policy(),
            }),
        )
        .await?;

        // Current owner (the oracle account) proposes the governance contract.
        self.execute(
            &ManagedAccountId(oracle_id.clone()),
            owner::ProposeOwner {
                contract_id: oracle_id.clone(),
                account_id: Some(governance_id.clone()),
            },
        )
        .await?;

        // Governance accepts ownership via a target-function-call proposal (id 0),
        // which fires `own_accept_owner` on the oracle as the governance contract.
        self.governance_target_call(&governance_id, &admin_id, 0, "own_accept_owner")
            .await?;

        Ok(governance_id)
    }

    /// Create and immediately execute a target-function-call governance proposal
    /// that calls `method_name` (no args, 1 yocto) on the proxy oracle.
    async fn governance_target_call(
        &self,
        governance_id: &AccountId,
        admin_id: &AccountId,
        proposal_id: u32,
        method_name: &str,
    ) -> Result<()> {
        let operation = Operation::TargetFunctionCall(FunctionCall {
            method_name: method_name.to_string(),
            args: near_sdk::json_types::Base64VecU8(b"{}".to_vec()),
            attached_deposit: near_sdk::json_types::U128(1),
            gas: near_sdk::Gas::from_tgas(50),
        });

        let admin = ManagedAccountId(admin_id.clone());
        self.execute(
            &admin,
            gov::CreateProposal {
                governance_id: governance_id.clone(),
                id: proposal_id,
                operation,
                requested_ttl: Nanoseconds::zero(),
                encoding: ProposalEncoding::Json,
            },
        )
        .await?;
        self.execute(
            &admin,
            gov::ExecuteProposal {
                governance_id: governance_id.clone(),
                id: proposal_id,
            },
        )
        .await?;
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
        let operation = v0::Operation::SetProxy {
            id: price_identifier,
            proxy: Some(proxy),
        };

        let signer = ManagedAccountId(oracle_id.clone());
        self.call_function_payable(
            &signer,
            &oracle_id,
            "gov_create",
            LegacyGovCreateArgs { id: 0, operation },
            NearToken::from_yoctonear(1),
        )
        .await?;
        self.call_function_payable(
            &signer,
            &oracle_id,
            "gov_execute",
            LegacyGovExecuteArgs { id: 0 },
            NearToken::from_yoctonear(1),
        )
        .await?;
        Ok(())
    }
}

#[derive(serde::Serialize)]
struct SetPythPriceArgs {
    price_identifier: PriceIdentifier,
    price: Option<templar_common::oracle::pyth::Price>,
}

#[derive(serde::Serialize)]
struct SetRedstonePriceArgs {
    feed_id: templar_common::oracle::redstone::FeedId,
    data: Option<templar_common::oracle::redstone::FeedData>,
}

#[derive(serde::Serialize)]
struct SetLazerPriceArgs {
    feed_id: u32,
    data: Option<templar_common::oracle::lazer::FeedData>,
}

/// `gov_create` args on a `< 0.2.0` oracle's built-in governance.
#[derive(serde::Serialize)]
struct LegacyGovCreateArgs {
    id: u32,
    operation: v0::Operation,
}

#[derive(serde::Serialize)]
struct LegacyGovExecuteArgs {
    id: u32,
}

fn zero_governance_policy() -> GovernancePolicy {
    GovernancePolicy::uniform(Nanoseconds::zero()).expect("zero is within bounds")
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
        start_owned_node().await
    }
}

/// Launch a dedicated `neard` for this harness.
async fn start_owned_node() -> Result<(Option<Sandbox>, NetworkConfig)> {
    let sandbox = Sandbox::start_sandbox_with_config(sandbox_config()).await?;
    let network = NetworkConfig::from_rpc_url("sandbox", sandbox.rpc_addr.parse()?);
    Ok((Some(sandbox), network))
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

/// `neard init --fast` block production delays, which every measurement of the
/// stock configuration was taken against.
const STOCK_MIN_BLOCK_MS: u64 = 120;
const STOCK_MAX_BLOCK_MS: u64 = 500;

/// Simulated time `sandbox_fast_forward` credits per block: nearcore's
/// `Client::sandbox_delta_time` advances the chain clock by
/// `delta_height × avg(min_block_production_delay, max_block_production_delay)`.
/// Every `fast_forward`-driven test was written against the stock average, so it
/// is held fixed while the real cadence changes.
const FAST_FORWARD_BLOCK_MS: u64 = (STOCK_MIN_BLOCK_MS + STOCK_MAX_BLOCK_MS) / 2;

/// Real block cadence. A sandbox is a single validator, so it approves its own
/// blocks immediately and produces the next one as soon as this delay elapses —
/// which makes it the floor under every node-backed test (a transaction awaited
/// at optimistic finality costs two blocks, a wait for final costs three).
const MIN_BLOCK_MS: u64 = 40;

/// Block cadence for launched nodes as `(min, max)` production delays in
/// milliseconds.
///
/// `min` is the real cadence (above); `NEAR_SANDBOX_BLOCK_MS` overrides it so a
/// constrained runner can be tuned without a code change. `max` is the timeout
/// for waiting on approvals from *other* validators — there are none, so it
/// never fires and costs no wall-clock time. It is derived to hold the average,
/// and hence [`FAST_FORWARD_BLOCK_MS`], fixed: lowering the real cadence must not
/// quietly shorten what a `fast_forward(N)` simulates.
fn block_delays_ms() -> (u64, u64) {
    let min = match std::env::var("NEAR_SANDBOX_BLOCK_MS") {
        // A malformed override must fail loudly, not silently fall back to the
        // fast default: CI pins this precisely because the fast cadence is
        // unreliable on a contended runner, so a typo'd value quietly restoring
        // it would resurface as flaky finality errors rather than a config error.
        Ok(value) => value
            .trim()
            .parse::<u64>()
            .unwrap_or_else(|_| {
                panic!(
                    "NEAR_SANDBOX_BLOCK_MS must be a whole number of milliseconds, got `{value}`"
                )
            })
            .clamp(1, FAST_FORWARD_BLOCK_MS),
        Err(_) => MIN_BLOCK_MS,
    };
    (min, 2 * FAST_FORWARD_BLOCK_MS - min)
}

/// Sandbox launch config shared by owned mode ([`SandboxHarness::start_owned`]) and the out-of-band
/// host (`bin/sandbox-host.rs`), so both nodes seed the `FUNDER_ACCOUNT_ID`
/// account identically and run the same block cadence.
#[must_use]
pub fn sandbox_config() -> SandboxConfig {
    let (min_block_ms, max_block_ms) = block_delays_ms();
    SandboxConfig {
        additional_config: Some(serde_json::json!({
            "consensus": {
                "min_block_production_delay": duration_json(min_block_ms),
                "max_block_production_delay": duration_json(max_block_ms),
            }
        })),
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

/// A `neard` config duration, which is serialized as seconds plus nanoseconds.
fn duration_json(ms: u64) -> serde_json::Value {
    serde_json::json!({
        "secs": ms / 1_000,
        "nanos": (ms % 1_000) * 1_000_000,
    })
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
    const MAX_ATTEMPTS: u32 = 5;
    let seq = TENANT_SEQ.fetch_add(1, Ordering::Relaxed);
    let account_id: AccountId = format!("t{}-{seq}.{FUNDER_ACCOUNT_ID}", std::process::id())
        .parse()
        .context("invalid tenant root id")?;
    let funder_id: AccountId = FUNDER_ACCOUNT_ID.parse().context("invalid funder id")?;
    let secret_key = test_secret_key()?;
    let public_key = secret_key.public_key();

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
            .with_public_key(public_key)
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

pub(crate) async fn deploy_contract(
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
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(network)
        .await?
        .assert_success();

    Ok(())
}

/// The fixed secret key every harness-created account is provisioned with
/// (signer accounts and contract accounts alike).
/// Exposed so external consumers can build their own gateway client against the
/// sandbox using the same key the harness deploys with.
pub fn test_secret_key() -> Result<SecretKey> {
    Ok("ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q"
        .parse()?)
}

/// A process-wide signer over the shared sandbox key. Reusing the same
/// `Arc<Signer>` preserves near-api's per-account nonce cache when sequential
/// test transactions wait only for optimistic execution.
pub fn test_signer() -> Arc<Signer> {
    static SIGNER: LazyLock<Arc<Signer>> = LazyLock::new(|| {
        Signer::from_secret_key(test_secret_key().expect("fixed test secret key is valid"))
            .expect("fixed test signer is valid")
    });

    Arc::clone(&SIGNER)
}

/// [`test_signer`] as a single gateway lane for `account_id`, sharing that
/// signer's nonce cache with direct near-api use.
pub async fn test_pooled_signer(account_id: impl Into<ManagedAccountId>) -> PooledSigner {
    PooledSigner::from_signer(account_id, test_signer())
        .await
        .expect("fixed test signer holds a key")
}
