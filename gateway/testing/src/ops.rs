//! Test-driving operations layered on the [`SandboxHarness`].
//!
//! These wrap the in-process [`templar_gateway_client::Client`] so test bodies
//! read as terse domain actions (`harness.supply(&user, &market, 1_000)`)
//! rather than plan/sign/submit boilerplate — the direct-client equivalent of
//! the retired `test-utils` controllers. Reads and writes both flow through the
//! same gateway dispatch the RPC service uses, so tests exercise production code
//! paths.

use std::time::Duration;

use anyhow::{Context, Result};
use near_api::{
    types::{AccessKeyPermission, AccountId},
    Account,
};
use near_token::NearToken;
use templar_common::{
    asset::{AssetClass, BorrowAssetAmount, CollateralAssetAmount, FungibleAsset},
    borrow::{BorrowPosition, BorrowStatus},
    market::{HarvestYieldMode, MarketConfiguration},
    oracle::pyth::OracleResponse,
    price::Convert,
    snapshot::Snapshot,
    supply::SupplyPosition,
    vault::{AllocationDelta, Fees, MarketId, Restrictions, VaultConfiguration},
    withdrawal_queue::{WithdrawalQueueStatus, WithdrawalRequestStatus},
};
use templar_gateway_client::Client;
use templar_gateway_methods_spec::{
    account, chain, contract, ft, market, mt, pyth, registry, storage, tx, vault,
};
use templar_gateway_types::{
    common::{ContractArgs, Pagination, WriteOperationResult},
    operation::{ReceiptOutcome, ReceiptStatus},
    primitive::PublicKey,
    Base64Bytes, BlockSummary, ContractMethodName, ManagedAccountId, NearGas, OperationStatus,
    StepStatus,
};

use templar_primitives::{Nanoseconds, SU128, SU64};
use test_utils::to_price;

use crate::{sandbox::SandboxHarness, TEST_FINALITY_POLICY};

/// A market deployed by [`SandboxHarness::deploy_full_market`], with the asset
/// and oracle accounts resolved from its configuration for convenient access.
#[derive(Clone)]
pub struct DeployedMarket {
    pub market_id: AccountId,
    pub borrow_ft_id: AccountId,
    pub collateral_ft_id: AccountId,
    pub configuration: MarketConfiguration,
}

/// A vault deployed by [`SandboxHarness::deploy_vault_with_market`], wired to a
/// live [`DeployedMarket`] (the vault's underlying is the market's borrow asset)
/// with its market registered and capped in the supply queue — the harness
/// equivalent of the retired `setup_test! extract(vault, c, vault_curator)`.
///
/// `owner` and `curator` are distinct, signable accounts: `owner` drives
/// governance (fees, caps), `curator` drives allocation and withdrawals.
pub struct DeployedVault {
    pub vault_id: AccountId,
    pub market: DeployedMarket,
    pub owner: ManagedAccountId,
    pub curator: ManagedAccountId,
    pub sentinel: ManagedAccountId,
    pub configuration: VaultConfiguration,
}

impl SandboxHarness {
    /// Build a fresh in-process gateway [`Client`] over every account the
    /// harness can currently sign as. Rebuilt per call so newly-created users
    /// are always available; cheap (no network I/O) for tests.
    pub fn client(&self) -> Result<Client> {
        let mut builder =
            Client::builder(self.network.clone()).finality_policy(TEST_FINALITY_POLICY);
        for (account_id, managed) in self.signers_snapshot() {
            builder = builder.with_signer(account_id, managed.signer.clone());
        }
        builder
            .build()
            .map_err(|error| anyhow::anyhow!("failed to build gateway client: {error}"))
    }

    /// Create a funded sub-account with a unique id and register its signer so
    /// the harness can drive operations as it.
    pub async fn create_user(&self, prefix: &str) -> Result<ManagedAccountId> {
        let (account_id, _) = self
            .create_account(prefix, NearToken::from_near(100))
            .await?;
        Ok(ManagedAccountId(account_id))
    }

    /// Deploy a market (plus its FT pair and mock oracle) and resolve the asset
    /// account ids from its configuration.
    pub async fn deploy_full_market(&self) -> Result<DeployedMarket> {
        self.deploy_full_market_with(|_| {}).await
    }

    /// [`deploy_full_market`](Self::deploy_full_market) with a hook to customize
    /// the [`MarketConfiguration`] before deployment.
    pub async fn deploy_full_market_with(
        &self,
        customize: impl FnOnce(&mut MarketConfiguration),
    ) -> Result<DeployedMarket> {
        let (market_id, configuration) = self.deploy_market_with(customize).await?;
        Ok(Self::resolve_deployed_market(market_id, configuration))
    }

    /// [`deploy_full_market_with`](Self::deploy_full_market_with) but with each
    /// asset deployed as a NEP-141 token or a NEP-245 multi-token per
    /// `borrow_mt`/`collateral_mt` — for the standard-agnostic asset matrix.
    pub async fn deploy_full_market_std(
        &self,
        borrow_mt: bool,
        collateral_mt: bool,
        customize: impl FnOnce(&mut MarketConfiguration),
    ) -> Result<DeployedMarket> {
        let (market_id, configuration) = self
            .deploy_market_std(borrow_mt, collateral_mt, customize)
            .await?;
        Ok(Self::resolve_deployed_market(market_id, configuration))
    }

    /// [`deploy_full_market`](Self::deploy_full_market) but pointing the market at
    /// an existing `oracle_id` (e.g. a proxy oracle) instead of a fresh mock
    /// oracle, with a hook to customize the [`MarketConfiguration`].
    pub async fn deploy_full_market_with_oracle(
        &self,
        oracle_id: AccountId,
        customize: impl FnOnce(&mut MarketConfiguration),
    ) -> Result<DeployedMarket> {
        let (market_id, configuration) =
            self.deploy_market_with_oracle(oracle_id, customize).await?;
        Ok(Self::resolve_deployed_market(market_id, configuration))
    }

    /// Resolve the asset *contract* account ids from a deployed market's
    /// configuration into a [`DeployedMarket`]. Works for both NEP-141 and
    /// NEP-245 assets — the id is the token contract either way; the NEP-245
    /// `token_id` lives on the asset in `configuration`.
    fn resolve_deployed_market(
        market_id: AccountId,
        configuration: MarketConfiguration,
    ) -> DeployedMarket {
        let borrow_ft_id = configuration.borrow_asset.contract_id().to_owned();
        let collateral_ft_id = configuration.collateral_asset.contract_id().to_owned();
        DeployedMarket {
            market_id,
            borrow_ft_id,
            collateral_ft_id,
            configuration,
        }
    }

    /// Set the market's mock oracle prices for both assets (in whole units).
    pub async fn set_asset_prices(
        &self,
        market: &DeployedMarket,
        borrow_price: f64,
        collateral_price: f64,
    ) -> Result<()> {
        let oracle = &market.configuration.price_oracle_configuration;
        self.set_mock_oracle_pyth_price(
            oracle.account_id.clone(),
            oracle.borrow_asset_price_id,
            Some(to_price(borrow_price)),
        )
        .await?;
        self.set_mock_oracle_pyth_price(
            oracle.account_id.clone(),
            oracle.collateral_asset_price_id,
            Some(to_price(collateral_price)),
        )
        .await?;
        Ok(())
    }

    /// Set the market's mock oracle borrow-asset price to an exact pyth `Price`
    /// (explicit exponent), for tests exercising extreme/edge price values.
    pub async fn set_borrow_asset_price_exact(
        &self,
        market: &DeployedMarket,
        price: Option<templar_common::oracle::pyth::Price>,
    ) -> Result<()> {
        let oracle = &market.configuration.price_oracle_configuration;
        self.set_mock_oracle_pyth_price(
            oracle.account_id.clone(),
            oracle.borrow_asset_price_id,
            price,
        )
        .await
    }

    /// Set the market's mock oracle collateral-asset price to an exact pyth
    /// `Price` (explicit exponent).
    pub async fn set_collateral_asset_price_exact(
        &self,
        market: &DeployedMarket,
        price: Option<templar_common::oracle::pyth::Price>,
    ) -> Result<()> {
        let oracle = &market.configuration.price_oracle_configuration;
        self.set_mock_oracle_pyth_price(
            oracle.account_id.clone(),
            oracle.collateral_asset_price_id,
            price,
        )
        .await
    }

    /// A contract's storage-balance bounds (min/max registration deposit).
    pub async fn storage_balance_bounds(
        &self,
        contract_id: &AccountId,
    ) -> Result<templar_gateway_types::common::StorageBalanceBounds> {
        Ok(self
            .client()?
            .read(storage::GetBalanceBounds {
                contract_id: contract_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("storage_balance_bounds failed: {error}"))?
            .bounds)
    }

    /// Top up `user`'s storage deposit on `contract_id` by its minimum bound —
    /// the amount the market charges per new supply/borrow position. Unlike
    /// registration this is additive, so it covers a position re-created after a
    /// prior one (and its snapshot storage) was charged.
    pub async fn storage_deposit_min(
        &self,
        user: &ManagedAccountId,
        contract_id: &AccountId,
    ) -> Result<WriteOperationResult> {
        let min = self.storage_balance_bounds(contract_id).await?.min;
        self.storage_deposit(user, contract_id, min).await
    }

    /// Register `user` for storage on `contract_id`, paying `deposit`.
    pub async fn storage_deposit(
        &self,
        user: &ManagedAccountId,
        contract_id: &AccountId,
        deposit: NearToken,
    ) -> Result<WriteOperationResult> {
        self.execute(
            user,
            storage::Deposit {
                contract_id: contract_id.clone(),
                beneficiary_id: None,
                registration_only: false,
                deposit,
            },
        )
        .await
    }

    /// Mint `amount` of a mock NEP-141 token to `user` (the mock FT mints to
    /// its caller).
    pub async fn mint(
        &self,
        user: &ManagedAccountId,
        token_id: &AccountId,
        amount: u128,
    ) -> Result<WriteOperationResult> {
        self.execute(
            user,
            tx::FunctionCall {
                receiver_id: token_id.clone(),
                method_name: ContractMethodName("mint".to_owned()),
                args: ContractArgs::Json(serde_json::json!({ "amount": SU128::from(amount) })),
                gas: NearGas::from_tgas(20),
                deposit: NearToken::from_yoctonear(0),
            },
        )
        .await
    }

    /// Mint `amount` of a mock NEP-245 token to `user`. Unlike NEP-141 this
    /// takes a `token_id` and the mock auto-registers the holder, so no separate
    /// storage deposit is needed.
    pub async fn mint_mt(
        &self,
        user: &ManagedAccountId,
        contract_id: &AccountId,
        token_id: &str,
        amount: u128,
    ) -> Result<WriteOperationResult> {
        self.execute(
            user,
            tx::FunctionCall {
                receiver_id: contract_id.clone(),
                method_name: ContractMethodName("mint".to_owned()),
                args: ContractArgs::Json(
                    serde_json::json!({ "token_id": token_id, "amount": SU128::from(amount) }),
                ),
                gas: NearGas::from_tgas(20),
                deposit: NearToken::from_yoctonear(0),
            },
        )
        .await
    }

    /// Register `user` on both assets and mint it a large balance of each — the
    /// setup every borrowing/supplying user needs. Handles NEP-141 (storage
    /// register + mint) and NEP-245 (mint auto-registers) transparently.
    pub async fn fund_user(&self, user: &ManagedAccountId, market: &DeployedMarket) -> Result<()> {
        const MINT_AMOUNT: u128 = 100_000_000;
        self.fund_asset(user, &market.configuration.borrow_asset, MINT_AMOUNT)
            .await?;
        self.fund_asset(user, &market.configuration.collateral_asset, MINT_AMOUNT)
            .await?;
        Ok(())
    }

    /// Fund `user` with `amount` of a single asset, dispatching on its standard.
    async fn fund_asset<T: AssetClass>(
        &self,
        user: &ManagedAccountId,
        asset: &FungibleAsset<T>,
        amount: u128,
    ) -> Result<()> {
        if let Some(contract_id) = asset.clone().into_nep141() {
            let ft_registration = NearToken::from_near(1).saturating_div(100);
            self.storage_deposit(user, &contract_id, ft_registration)
                .await?;
            self.mint(user, &contract_id, amount).await?;
        } else if let Some((contract_id, token_id)) = asset.clone().into_nep245() {
            self.mint_mt(user, &contract_id, &token_id, amount).await?;
        }
        Ok(())
    }

    /// Deploy a vault wired to a fresh market with default configs. See
    /// [`deploy_vault_with_market_with`](Self::deploy_vault_with_market_with).
    pub async fn deploy_vault_with_market(&self) -> Result<DeployedVault> {
        self.deploy_vault_with_market_with(|_| {}, |_| {}).await
    }

    /// Deploy a market, then a vault whose underlying is that market's borrow
    /// asset, with distinct signable owner/curator/sentinel accounts. Registers
    /// the vault (and fee/skim recipients) for storage, then caps and enqueues
    /// the market through the gateway so allocation/withdrawal work. This is the
    /// harness analogue of the retired `setup_everything`; the hooks customize
    /// the market and vault configurations before deployment.
    pub async fn deploy_vault_with_market_with(
        &self,
        customize_market: impl FnOnce(&mut MarketConfiguration),
        customize_vault: impl FnOnce(&mut VaultConfiguration),
    ) -> Result<DeployedVault> {
        // The market and the vault's six accounts are independent — only the
        // configuration built below needs both — so deploy the market and mint
        // the accounts concurrently rather than one after the other.
        let operator = NearToken::from_near(100);
        let vault_accounts = [
            ("vault-owner", operator),
            ("vault-curator", operator),
            ("vault-sentinel", operator),
            ("vault-skim", operator),
            ("vault-fee", operator),
            ("vault", operator),
        ];
        let (market, accounts) = futures::try_join!(
            self.deploy_full_market_with(customize_market),
            self.create_accounts(&vault_accounts),
        )?;
        let [(owner_id, _), (curator_id, _), (sentinel_id, _), (skim_id, _), (fee_id, _), (vault_id, vault_signer)] =
            accounts.try_into().map_err(|_| {
                anyhow::anyhow!("create_accounts returned the wrong number of accounts")
            })?;

        // The vault's underlying MUST be the market's borrow asset for the two to
        // integrate. Guardian is unused by the ported tests, so reuse `owner`.
        let mut configuration = test_utils::vault_configuration(
            owner_id.clone(),
            curator_id.clone(),
            owner_id.clone(),
            sentinel_id.clone(),
            market.borrow_ft_id.clone(),
            skim_id.clone(),
            fee_id.clone(),
        );
        customize_vault(&mut configuration);

        // The market must be registered to receive its own assets — the vault
        // transfers underlying to it on allocation (mirrors `setup_everything`'s
        // `c.storage_deposits(mkt)`). Neither that nor the oracle prices involve
        // the vault, so all three run alongside its deployment; the calls within
        // each share one signer, so they stay ordered.
        let ft_registration = NearToken::from_near(1).saturating_div(100);
        let market_account = ManagedAccountId(market.market_id.clone());
        futures::try_join!(
            crate::sandbox::deploy_contract(
                &self.network,
                vault_id.clone(),
                vault_signer,
                crate::wasm::vault().await.to_vec(),
                "new",
                serde_json::json!({ "configuration": configuration.clone() }),
            ),
            self.set_asset_prices(&market, 1.0, 1.0),
            async {
                self.storage_deposit(&market_account, &market.borrow_ft_id, ft_registration)
                    .await?;
                self.storage_deposit(&market_account, &market.collateral_ft_id, ft_registration)
                    .await?;
                Ok(())
            },
        )?;

        // Storage opt-ins (mirrors `UnifiedVaultController::storage_deposits`): the
        // vault itself and the fee/skim recipients must be registered on the vault
        // share ledger, the market, and both FTs. One participant per task — each
        // signs as a different account, so they do not share a nonce.
        let owner = ManagedAccountId(owner_id);
        let vault_account = ManagedAccountId(vault_id.clone());
        let skim = ManagedAccountId(skim_id);
        let fee = ManagedAccountId(fee_id);
        let registrations = [&vault_account, &skim, &fee]
            .map(|account| self.register_for_vault(account, &vault_id, &market));
        for result in futures::future::join_all(registrations).await {
            result?;
        }

        // Cap and enqueue the market through the gateway, owner-signed.
        self.execute(
            &owner,
            vault::SubmitCap {
                vault_id: vault_id.clone(),
                market: market.market_id.clone(),
                new_cap: SU128::from(u128::MAX),
            },
        )
        .await?;
        self.execute(
            &owner,
            vault::AcceptCap {
                vault_id: vault_id.clone(),
                market: market.market_id.clone(),
            },
        )
        .await?;
        let market_id = self
            .vault_market_id_of(&vault_id, &market.market_id)
            .await?
            .context("market not registered on vault after accept_cap")?;
        self.execute(
            &owner,
            vault::SetSupplyQueue {
                vault_id: vault_id.clone(),
                markets: vec![market_id],
            },
        )
        .await?;

        Ok(DeployedVault {
            vault_id,
            market,
            owner,
            curator: ManagedAccountId(curator_id),
            sentinel: ManagedAccountId(sentinel_id),
            configuration,
        })
    }

    /// Register `account` for storage on the vault share ledger, the market, and
    /// both FTs — everything a vault participant (or the vault itself) needs.
    async fn register_for_vault(
        &self,
        account: &ManagedAccountId,
        vault_id: &AccountId,
        market: &DeployedMarket,
    ) -> Result<()> {
        let ft_registration = NearToken::from_near(1).saturating_div(100);
        // The vault's reported registration min covers registration only; holding
        // a share balance needs more, so over-deposit rather than the bare min.
        self.storage_deposit(
            account,
            vault_id,
            NearToken::from_near(1).saturating_div(20),
        )
        .await?;
        self.storage_deposit_min(account, &market.market_id).await?;
        self.storage_deposit(account, &market.borrow_ft_id, ft_registration)
            .await?;
        self.storage_deposit(account, &market.collateral_ft_id, ft_registration)
            .await?;
        Ok(())
    }

    /// Register `user` for vault participation and mint it underlying — the vault
    /// analogue of [`fund_user`](Self::fund_user), mirroring
    /// `UnifiedVaultController::init_account`.
    pub async fn vault_init_account(
        &self,
        user: &ManagedAccountId,
        vault: &DeployedVault,
    ) -> Result<()> {
        const MINT_AMOUNT: u128 = 100_000_000;
        self.register_for_vault(user, &vault.vault_id, &vault.market)
            .await?;
        self.mint(user, &vault.market.borrow_ft_id, MINT_AMOUNT)
            .await?;
        self.mint(user, &vault.market.collateral_ft_id, MINT_AMOUNT)
            .await?;
        Ok(())
    }

    /// The vault's internal market id for `market_account`, if registered.
    pub async fn vault_market_id_of(
        &self,
        vault_id: &AccountId,
        market_account: &AccountId,
    ) -> Result<Option<MarketId>> {
        Ok(self
            .client()?
            .read(vault::GetMarketIdOfAccount {
                vault_id: vault_id.clone(),
                market: market_account.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("get_market_id_of_account failed: {error}"))?
            .market_id)
    }

    /// Deposit underlying into the vault (mints shares to `user`).
    pub async fn vault_supply(
        &self,
        user: &ManagedAccountId,
        vault: &DeployedVault,
        amount: u128,
    ) -> Result<WriteOperationResult> {
        self.execute(
            user,
            vault::Deposit {
                vault_id: vault.vault_id.clone(),
                amount: SU128::from(amount),
            },
        )
        .await
    }

    /// Attempt a vault deposit, returning the (possibly refunded/failed)
    /// operation result for tests where the vault rejects it (e.g. while paused
    /// or for a blacklisted depositor — the deposit is an `ft_transfer_call`, so
    /// a rejecting `ft_on_transfer` refunds and the operation reports `Failed`).
    pub async fn try_vault_supply(
        &self,
        user: &ManagedAccountId,
        vault: &DeployedVault,
        amount: u128,
    ) -> Result<WriteOperationResult> {
        self.try_execute(
            user,
            vault::Deposit {
                vault_id: vault.vault_id.clone(),
                amount: SU128::from(amount),
            },
        )
        .await
    }

    /// Allocate or withdraw vault principal to/from a market (curator op).
    pub async fn vault_allocate(
        &self,
        allocator: &ManagedAccountId,
        vault: &DeployedVault,
        delta: AllocationDelta,
    ) -> Result<WriteOperationResult> {
        self.execute(
            allocator,
            vault::Allocate {
                vault_id: vault.vault_id.clone(),
                delta,
            },
        )
        .await
    }

    /// Vault total assets (idle + market principal).
    pub async fn vault_total_assets(&self, vault: &DeployedVault) -> Result<u128> {
        Ok(self
            .client()?
            .read(vault::GetTotalAssets {
                vault_id: vault.vault_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("get_total_assets failed: {error}"))?
            .0)
    }

    /// Vault total share supply.
    pub async fn vault_total_supply(&self, vault: &DeployedVault) -> Result<u128> {
        Ok(self
            .client()?
            .read(vault::GetTotalSupply {
                vault_id: vault.vault_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("get_total_supply failed: {error}"))?
            .0)
    }

    /// Vault idle balance (unallocated underlying).
    pub async fn vault_idle_balance(&self, vault: &DeployedVault) -> Result<u128> {
        Ok(self
            .client()?
            .read(vault::GetIdleBalance {
                vault_id: vault.vault_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("get_idle_balance failed: {error}"))?
            .0)
    }

    /// Withdraw underlying by asset amount (receiver defaults to `user`).
    pub async fn vault_withdraw(
        &self,
        user: &ManagedAccountId,
        vault: &DeployedVault,
        amount: u128,
        receiver: Option<AccountId>,
    ) -> Result<WriteOperationResult> {
        self.execute(
            user,
            vault::Withdraw {
                vault_id: vault.vault_id.clone(),
                amount: SU128::from(amount),
                receiver: receiver.unwrap_or_else(|| user.0.clone()),
            },
        )
        .await
    }

    /// Execute the next user withdrawal over `route` (market accounts).
    pub async fn vault_execute_withdrawal(
        &self,
        allocator: &ManagedAccountId,
        vault: &DeployedVault,
        route: &[AccountId],
    ) -> Result<WriteOperationResult> {
        let route = self.resolve_market_ids(vault, route).await?;
        self.execute(
            allocator,
            vault::ExecuteWithdrawal {
                vault_id: vault.vault_id.clone(),
                route,
            },
        )
        .await
    }

    /// Execute a market withdrawal step for the given op and market id.
    pub async fn vault_execute_market_withdrawal(
        &self,
        allocator: &ManagedAccountId,
        vault: &DeployedVault,
        op_id: u64,
        market_id: MarketId,
        batch_limit: Option<u32>,
    ) -> Result<WriteOperationResult> {
        self.execute(
            allocator,
            vault::ExecuteMarketWithdrawal {
                vault_id: vault.vault_id.clone(),
                op_id: SU64::from(op_id),
                market: market_id,
                batch_limit,
            },
        )
        .await
    }

    /// Execute an allocator rebalance withdrawal from `market` (a market account).
    pub async fn vault_execute_rebalance_withdrawal(
        &self,
        allocator: &ManagedAccountId,
        vault: &DeployedVault,
        market: &AccountId,
        batch_limit: Option<u32>,
    ) -> Result<WriteOperationResult> {
        let market_id = self
            .vault_market_id_of(&vault.vault_id, market)
            .await?
            .with_context(|| format!("unknown market: {market}"))?;
        self.execute(
            allocator,
            vault::ExecuteRebalanceWithdrawal {
                vault_id: vault.vault_id.clone(),
                market_id,
                batch_limit,
            },
        )
        .await
    }

    /// Recover the vault from a stuck withdrawing state.
    pub async fn vault_unbrick(
        &self,
        caller: &ManagedAccountId,
        vault: &DeployedVault,
    ) -> Result<WriteOperationResult> {
        self.execute(
            caller,
            vault::Unbrick {
                vault_id: vault.vault_id.clone(),
            },
        )
        .await
    }

    /// Resync the vault's idle balance from its underlying token balance.
    pub async fn vault_resync_idle_balance(
        &self,
        caller: &ManagedAccountId,
        vault: &DeployedVault,
    ) -> Result<WriteOperationResult> {
        self.execute(
            caller,
            vault::ResyncIdleBalance {
                vault_id: vault.vault_id.clone(),
            },
        )
        .await
    }

    /// Set the vault's supply queue from `markets` (market accounts).
    pub async fn vault_set_supply_queue(
        &self,
        caller: &ManagedAccountId,
        vault: &DeployedVault,
        markets: &[AccountId],
    ) -> Result<WriteOperationResult> {
        let markets = self.resolve_market_ids(vault, markets).await?;
        self.execute(
            caller,
            vault::SetSupplyQueue {
                vault_id: vault.vault_id.clone(),
                markets,
            },
        )
        .await
    }

    /// Set the vault fees (owner-gated governance).
    pub async fn vault_set_fees(
        &self,
        owner: &ManagedAccountId,
        vault: &DeployedVault,
        fees: Fees<SU128>,
    ) -> Result<WriteOperationResult> {
        self.execute(
            owner,
            vault::SetFees {
                vault_id: vault.vault_id.clone(),
                fees,
            },
        )
        .await
    }

    /// Submit a restrictions change (owner-gated, timelocked).
    pub async fn vault_set_restrictions(
        &self,
        owner: &ManagedAccountId,
        vault: &DeployedVault,
        restrictions: Option<Restrictions>,
    ) -> Result<WriteOperationResult> {
        self.execute(
            owner,
            vault::SetRestrictions {
                vault_id: vault.vault_id.clone(),
                restrictions,
            },
        )
        .await
    }

    /// Accept a pending restrictions change.
    pub async fn vault_accept_restrictions(
        &self,
        owner: &ManagedAccountId,
        vault: &DeployedVault,
    ) -> Result<WriteOperationResult> {
        self.execute(
            owner,
            vault::AcceptRestrictions {
                vault_id: vault.vault_id.clone(),
            },
        )
        .await
    }

    /// Submit a sentinel role change (owner-gated, timelocked).
    pub async fn vault_submit_sentinel(
        &self,
        owner: &ManagedAccountId,
        vault: &DeployedVault,
        account: &AccountId,
    ) -> Result<WriteOperationResult> {
        self.execute(
            owner,
            vault::SubmitSentinel {
                vault_id: vault.vault_id.clone(),
                account: account.clone(),
            },
        )
        .await
    }

    /// Accept a pending sentinel role change.
    pub async fn vault_accept_sentinel(
        &self,
        owner: &ManagedAccountId,
        vault: &DeployedVault,
    ) -> Result<WriteOperationResult> {
        self.execute(
            owner,
            vault::AcceptSentinel {
                vault_id: vault.vault_id.clone(),
            },
        )
        .await
    }

    /// Submit a supply-cap change for `market` (owner-gated; a decrease needs no
    /// timelock).
    pub async fn vault_submit_cap(
        &self,
        owner: &ManagedAccountId,
        vault: &DeployedVault,
        market: &AccountId,
        new_cap: u128,
    ) -> Result<WriteOperationResult> {
        self.execute(
            owner,
            vault::SubmitCap {
                vault_id: vault.vault_id.clone(),
                market: market.clone(),
                new_cap: SU128::from(new_cap),
            },
        )
        .await
    }

    /// Grant or revoke the allocator role for `account` (owner-gated).
    pub async fn vault_set_is_allocator(
        &self,
        owner: &ManagedAccountId,
        vault: &DeployedVault,
        account: &AccountId,
        allowed: bool,
    ) -> Result<WriteOperationResult> {
        self.execute(
            owner,
            vault::SetIsAllocator {
                vault_id: vault.vault_id.clone(),
                account: account.clone(),
                allowed,
            },
        )
        .await
    }

    /// The vault's current fees.
    pub async fn vault_get_fees(&self, vault: &DeployedVault) -> Result<Fees<SU128>> {
        self.client()?
            .read(vault::GetFees {
                vault_id: vault.vault_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("get_fees failed: {error}"))
    }

    /// The vault's current restrictions, if any.
    pub async fn vault_get_restrictions(
        &self,
        vault: &DeployedVault,
    ) -> Result<Option<Restrictions>> {
        Ok(self
            .client()?
            .read(vault::GetRestrictions {
                vault_id: vault.vault_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("get_restrictions failed: {error}"))?
            .restrictions)
    }

    /// The id of the in-flight user withdrawal op, if any.
    pub async fn vault_get_withdrawing_op_id(&self, vault: &DeployedVault) -> Result<Option<u64>> {
        Ok(self
            .client()?
            .read(vault::GetWithdrawingOpId {
                vault_id: vault.vault_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("get_withdrawing_op_id failed: {error}"))?
            .op_id
            .map(|id| id.0))
    }

    /// Resolve market account ids to the vault's internal [`MarketId`]s.
    async fn resolve_market_ids(
        &self,
        vault: &DeployedVault,
        markets: &[AccountId],
    ) -> Result<Vec<MarketId>> {
        let mut ids = Vec::with_capacity(markets.len());
        for market in markets {
            ids.push(
                self.vault_market_id_of(&vault.vault_id, market)
                    .await?
                    .with_context(|| format!("unknown market: {market}"))?,
            );
        }
        Ok(ids)
    }

    /// Supply borrow-asset liquidity to the market.
    pub async fn supply(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        amount: u128,
    ) -> Result<WriteOperationResult> {
        self.execute(
            user,
            market::Supply {
                market_id: market.market_id.clone(),
                amount: BorrowAssetAmount::new(amount),
            },
        )
        .await
    }

    /// Attempt to supply, returning the (possibly failed) operation result for
    /// tests that expect the contract to reject it.
    pub async fn try_supply(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        amount: u128,
    ) -> Result<WriteOperationResult> {
        self.try_execute(
            user,
            market::Supply {
                market_id: market.market_id.clone(),
                amount: BorrowAssetAmount::new(amount),
            },
        )
        .await
    }

    /// Supply, then harvest until the deposit is fully activated (no longer in
    /// the `incoming` bucket) — mirrors the old controller helper.
    pub async fn supply_and_harvest_until_activation(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        amount: u128,
    ) -> Result<()> {
        self.supply(user, market, amount).await?;
        // Guard against an unexpectedly never-activating deposit (bounded well
        // above any realistic snapshot count for a test).
        for _ in 0..1000 {
            if self
                .get_supply_position(market, &user.0)
                .await?
                .context("supply position missing after supply")?
                .get_deposit()
                .incoming
                .is_empty()
            {
                return Ok(());
            }
            self.harvest_yield(user, market, Some(user.0.clone()))
                .await?;
        }
        anyhow::bail!("supply deposit did not activate after 1000 harvests")
    }

    /// Deposit collateral into the market.
    pub async fn collateralize(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        amount: u128,
    ) -> Result<WriteOperationResult> {
        self.execute(
            user,
            market::Collateralize {
                market_id: market.market_id.clone(),
                amount: CollateralAssetAmount::new(amount),
            },
        )
        .await
    }

    /// Attempt to deposit collateral, returning the (possibly refunded/failed)
    /// operation result for tests where the contract rejects it.
    pub async fn try_collateralize(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        amount: u128,
    ) -> Result<WriteOperationResult> {
        self.try_execute(
            user,
            market::Collateralize {
                market_id: market.market_id.clone(),
                amount: CollateralAssetAmount::new(amount),
            },
        )
        .await
    }

    /// Borrow against deposited collateral.
    pub async fn borrow(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        amount: u128,
    ) -> Result<WriteOperationResult> {
        self.execute(
            user,
            market::Borrow {
                market_id: market.market_id.clone(),
                amount: BorrowAssetAmount::new(amount),
            },
        )
        .await
    }

    /// Attempt to borrow, returning the (possibly failed) operation result for
    /// tests that expect the contract to reject it.
    pub async fn try_borrow(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        amount: u128,
    ) -> Result<WriteOperationResult> {
        self.try_execute(
            user,
            market::Borrow {
                market_id: market.market_id.clone(),
                amount: BorrowAssetAmount::new(amount),
            },
        )
        .await
    }

    /// Attempt to withdraw collateral, returning the (possibly failed) operation
    /// result for tests that expect the contract to reject it.
    pub async fn try_withdraw_collateral(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        amount: u128,
    ) -> Result<WriteOperationResult> {
        self.try_execute(
            user,
            market::WithdrawCollateral {
                market_id: market.market_id.clone(),
                amount: CollateralAssetAmount::new(amount),
            },
        )
        .await
    }

    /// Apply accrued interest to a borrow position.
    pub async fn apply_interest(
        &self,
        caller: &ManagedAccountId,
        market: &DeployedMarket,
        account_id: Option<AccountId>,
        snapshot_limit: Option<u32>,
    ) -> Result<WriteOperationResult> {
        self.execute(
            caller,
            market::ApplyInterest {
                market_id: market.market_id.clone(),
                account_id,
                snapshot_limit,
            },
        )
        .await
    }

    /// Accumulate the statically-allocated yield for `account_id` (permissionless).
    pub async fn accumulate_static_yield(
        &self,
        caller: &ManagedAccountId,
        market: &DeployedMarket,
        account_id: Option<AccountId>,
        snapshot_limit: Option<u32>,
    ) -> Result<WriteOperationResult> {
        self.execute(
            caller,
            market::AccumulateStaticYield {
                market_id: market.market_id.clone(),
                account_id,
                snapshot_limit,
            },
        )
        .await
    }

    /// Withdraw the caller's accumulated static yield (`None` = all).
    pub async fn withdraw_static_yield(
        &self,
        recipient: &ManagedAccountId,
        market: &DeployedMarket,
        amount: Option<u128>,
    ) -> Result<WriteOperationResult> {
        self.execute(
            recipient,
            market::WithdrawStaticYield {
                market_id: market.market_id.clone(),
                amount: amount.map(BorrowAssetAmount::new),
            },
        )
        .await
    }

    /// Total accumulated static yield for `account_id` (borrow-denominated).
    pub async fn static_yield_total(
        &self,
        market: &DeployedMarket,
        account_id: &AccountId,
    ) -> Result<u128> {
        Ok(u128::from(
            self.client()?
                .read(market::GetStaticYield {
                    market_id: market.market_id.clone(),
                    account_id: account_id.clone(),
                })
                .await
                .map_err(|error| anyhow::anyhow!("static_yield failed: {error}"))?
                .borrow_asset_total(),
        ))
    }

    /// The static-yield record total for `account_id`, or `None` if the account
    /// has no record (distinguishing "no record" from "a record of zero").
    pub async fn static_yield_record(
        &self,
        market: &DeployedMarket,
        account_id: &AccountId,
    ) -> Result<Option<u128>> {
        Ok(self
            .client()?
            .read(market::GetStaticYield {
                market_id: market.market_id.clone(),
                account_id: account_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("static_yield failed: {error}"))?
            .record
            .map(|record| u128::from(record.borrow_asset_total())))
    }

    /// Attempt to withdraw static yield, returning the (possibly failed)
    /// operation result for tests where the transfer is expected to be rejected.
    pub async fn try_withdraw_static_yield(
        &self,
        recipient: &ManagedAccountId,
        market: &DeployedMarket,
        amount: Option<u128>,
    ) -> Result<WriteOperationResult> {
        self.try_execute(
            recipient,
            market::WithdrawStaticYield {
                market_id: market.market_id.clone(),
                amount: amount.map(BorrowAssetAmount::new),
            },
        )
        .await
    }

    /// List all supply positions, keyed by account.
    pub async fn list_supply_positions(
        &self,
        market: &DeployedMarket,
    ) -> Result<std::collections::HashMap<AccountId, SupplyPosition>> {
        Ok(self
            .client()?
            .read(market::ListSupplyPositions {
                market_id: market.market_id.clone(),
                args: Pagination::default(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("list_supply_positions failed: {error}"))?
            .positions)
    }

    /// List all borrow positions, keyed by account.
    pub async fn list_borrow_positions(
        &self,
        market: &DeployedMarket,
    ) -> Result<std::collections::HashMap<AccountId, BorrowPosition>> {
        Ok(self
            .client()?
            .read(market::ListBorrowPositions {
                market_id: market.market_id.clone(),
                args: Pagination::default(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("list_borrow_positions failed: {error}"))?
            .positions)
    }

    /// List the finalized snapshots.
    pub async fn list_finalized_snapshots(&self, market: &DeployedMarket) -> Result<Vec<Snapshot>> {
        Ok(self
            .client()?
            .read(market::ListFinalizedSnapshots {
                market_id: market.market_id.clone(),
                args: Pagination::default(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("list_finalized_snapshots failed: {error}"))?
            .snapshots)
    }

    /// Add a contract version (wasm) to a registry.
    pub async fn registry_add_version(
        &self,
        caller: &ManagedAccountId,
        registry_id: &AccountId,
        version_key: &str,
        deploy_mode: templar_common::registry::DeployMode,
        code: Vec<u8>,
        deposit: NearToken,
    ) -> Result<WriteOperationResult> {
        self.execute(
            caller,
            registry::AddVersion {
                registry_id: registry_id.clone(),
                version_key: version_key.to_owned(),
                deploy_mode,
                code: Base64Bytes(code),
                deposit,
            },
        )
        .await
    }

    /// Deploy a contract from a registry version. The deployed contract lives at
    /// the sub-account `{name}.{registry_id}`.
    #[allow(clippy::too_many_arguments)]
    pub async fn registry_deploy(
        &self,
        caller: &ManagedAccountId,
        registry_id: &AccountId,
        name: &str,
        version_key: &str,
        init_args: Vec<u8>,
        full_access_keys: Option<Vec<PublicKey>>,
        deposit: NearToken,
    ) -> Result<WriteOperationResult> {
        self.execute(
            caller,
            registry::Deploy {
                registry_id: registry_id.clone(),
                name: name.to_owned(),
                version_key: version_key.to_owned(),
                init_args: Base64Bytes(init_args),
                full_access_keys,
                deposit,
            },
        )
        .await
    }

    /// Read a market's configuration by account id (for markets not deployed via
    /// the harness, e.g. deployed through a registry).
    pub async fn get_configuration(&self, market_id: &AccountId) -> Result<MarketConfiguration> {
        self.client()?
            .read(market::GetConfiguration {
                market_id: market_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("get_configuration failed: {error}"))
    }

    /// List `account_id`'s access keys at the sandbox's optimistic query
    /// reference as `(public_key, is_full_access)`.
    pub async fn view_access_keys(&self, account_id: &AccountId) -> Result<Vec<(String, bool)>> {
        let keys = Account(account_id.clone())
            .list_keys()
            .at(TEST_FINALITY_POLICY.query_reference())
            .fetch_from(&self.network)
            .await?
            .data;
        Ok(keys
            .into_iter()
            .map(|(public_key, access_key)| {
                let full_access = matches!(access_key.permission, AccessKeyPermission::FullAccess);
                (public_key.to_string(), full_access)
            })
            .collect())
    }

    /// Patch raw contract storage entries (key/value byte pairs) on `account_id`
    /// via the `sandbox_patch_state` RPC. Works in both attach and owned mode
    /// (it only needs the node's RPC url, not an owned `Sandbox`).
    pub async fn patch_state(
        &self,
        account_id: &AccountId,
        entries: impl IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
    ) -> Result<()> {
        crate::sandbox_ext::patch_data(&self.network, account_id, entries).await
    }

    /// Liquidate an unhealthy borrow position (`liquidation_amount` of the borrow
    /// asset is supplied by `liquidator`).
    pub async fn liquidate(
        &self,
        liquidator: &ManagedAccountId,
        market: &DeployedMarket,
        account_id: &AccountId,
        liquidation_amount: u128,
        collateral_amount: Option<u128>,
    ) -> Result<WriteOperationResult> {
        self.execute(
            liquidator,
            market::Liquidate {
                market_id: market.market_id.clone(),
                account_id: account_id.clone(),
                liquidation_amount: BorrowAssetAmount::new(liquidation_amount),
                collateral_amount: collateral_amount.map(CollateralAssetAmount::new),
            },
        )
        .await
    }

    /// Attempt a liquidation, returning the (possibly refunded/failed) operation
    /// result for tests where the contract rejects it (liquidation pays via
    /// `ft_transfer_call`, so a rejected attempt is refunded).
    pub async fn try_liquidate(
        &self,
        liquidator: &ManagedAccountId,
        market: &DeployedMarket,
        account_id: &AccountId,
        liquidation_amount: u128,
        collateral_amount: Option<u128>,
    ) -> Result<WriteOperationResult> {
        self.try_execute(
            liquidator,
            market::Liquidate {
                market_id: market.market_id.clone(),
                account_id: account_id.clone(),
                liquidation_amount: BorrowAssetAmount::new(liquidation_amount),
                collateral_amount: collateral_amount.map(CollateralAssetAmount::new),
            },
        )
        .await
    }

    /// The liquidatable collateral for `account_id` and the borrow-asset amount a
    /// liquidator must pay for it at fair market value (mirrors the retired
    /// controller's `liquidatable_collateral_fmv`).
    pub async fn liquidatable_collateral_fmv(
        &self,
        market: &DeployedMarket,
        account_id: &AccountId,
    ) -> Result<(CollateralAssetAmount, BorrowAssetAmount)> {
        let collateral = self.liquidatable_collateral(market, account_id).await?;
        let prices = self.get_oracle_prices(market).await?;
        let price_pair = market
            .configuration
            .price_oracle_configuration
            .create_price_pair(&prices)
            .context("failed to create price pair")?;
        let pay = price_pair
            .convert(collateral)
            .to_u128_ceil()
            .context("price conversion overflow")?
            .max(1)
            .into();
        Ok((collateral, pay))
    }

    /// Like [`liquidatable_collateral_fmv`](Self::liquidatable_collateral_fmv) but
    /// discounting the pay amount by the configured maximum liquidator spread.
    pub async fn liquidatable_collateral_with_spread(
        &self,
        market: &DeployedMarket,
        account_id: &AccountId,
    ) -> Result<(CollateralAssetAmount, BorrowAssetAmount)> {
        let collateral = self.liquidatable_collateral(market, account_id).await?;
        let prices = self.get_oracle_prices(market).await?;
        let price_pair = market
            .configuration
            .price_oracle_configuration
            .create_price_pair(&prices)
            .context("failed to create price pair")?;
        let pay = (price_pair.convert(collateral)
            * (templar_common::Decimal::ONE - market.configuration.liquidation_maximum_spread))
            .to_u128_ceil()
            .context("price conversion overflow")?
            .max(1)
            .into();
        Ok((collateral, pay))
    }

    async fn liquidatable_collateral(
        &self,
        market: &DeployedMarket,
        account_id: &AccountId,
    ) -> Result<CollateralAssetAmount> {
        let prices = self.get_oracle_prices(market).await?;
        let price_pair = market
            .configuration
            .price_oracle_configuration
            .create_price_pair(&prices)
            .context("failed to create price pair")?;
        let position = self
            .get_borrow_position(market, account_id)
            .await?
            .context("borrow position missing")?;
        Ok(position.liquidatable_collateral(
            &price_pair,
            market.configuration.borrow_mcr_maintenance,
            market.configuration.liquidation_maximum_spread,
        ))
    }

    /// Transfer fungible tokens (plain NEP-141 `ft_transfer`, no receiver call).
    pub async fn ft_transfer(
        &self,
        user: &ManagedAccountId,
        token_id: &AccountId,
        receiver_id: &AccountId,
        amount: u128,
    ) -> Result<WriteOperationResult> {
        self.execute(
            user,
            ft::Transfer {
                contract_id: token_id.clone(),
                receiver_id: receiver_id.clone(),
                amount: SU128::from(amount),
                memo: None,
            },
        )
        .await
    }

    /// Repay a borrow position (the signer's own, when `account_id` is `None`).
    pub async fn repay(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        amount: u128,
        account_id: Option<AccountId>,
    ) -> Result<WriteOperationResult> {
        self.execute(
            user,
            market::Repay {
                market_id: market.market_id.clone(),
                amount: BorrowAssetAmount::new(amount),
                account_id,
            },
        )
        .await
    }

    /// Attempt to repay, returning the (possibly refunded/failed) operation
    /// result for tests where the contract rejects it (e.g. while liquidatable).
    pub async fn try_repay(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        amount: u128,
        account_id: Option<AccountId>,
    ) -> Result<WriteOperationResult> {
        self.try_execute(
            user,
            market::Repay {
                market_id: market.market_id.clone(),
                amount: BorrowAssetAmount::new(amount),
                account_id,
            },
        )
        .await
    }

    /// Harvest supply yield for `account_id` (default mode).
    pub async fn harvest_yield(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        account_id: Option<AccountId>,
    ) -> Result<WriteOperationResult> {
        self.harvest_yield_with_mode(user, market, account_id, Some(HarvestYieldMode::Default))
            .await
    }

    /// Harvest supply yield for `account_id` with an explicit harvest mode.
    pub async fn harvest_yield_with_mode(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        account_id: Option<AccountId>,
        mode: Option<HarvestYieldMode>,
    ) -> Result<WriteOperationResult> {
        self.execute(
            user,
            market::HarvestYield {
                market_id: market.market_id.clone(),
                account_id,
                mode,
            },
        )
        .await
    }

    /// Interest accrued on a borrow position but not yet realized into it.
    pub async fn get_borrow_position_pending_interest(
        &self,
        market: &DeployedMarket,
        account_id: &AccountId,
    ) -> Result<BorrowAssetAmount> {
        Ok(self
            .client()?
            .read(market::GetBorrowPositionPendingInterest {
                market_id: market.market_id.clone(),
                account_id: account_id.clone(),
                snapshot_limit: None,
            })
            .await
            .map_err(|error| {
                anyhow::anyhow!("get_borrow_position_pending_interest failed: {error}")
            })?
            .amount
            .unwrap_or_default())
    }

    /// Yield accrued to a supply position but not yet realized into it.
    pub async fn get_supply_position_pending_yield(
        &self,
        market: &DeployedMarket,
        account_id: &AccountId,
    ) -> Result<BorrowAssetAmount> {
        Ok(self
            .client()?
            .read(market::GetSupplyPositionPendingYield {
                market_id: market.market_id.clone(),
                account_id: account_id.clone(),
                snapshot_limit: None,
            })
            .await
            .map_err(|error| anyhow::anyhow!("get_supply_position_pending_yield failed: {error}"))?
            .amount
            .unwrap_or_default())
    }

    /// The market's current (unfinalized) snapshot.
    pub async fn get_current_snapshot(&self, market: &DeployedMarket) -> Result<Snapshot> {
        self.client()?
            .read(market::GetCurrentSnapshot {
                market_id: market.market_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("get_current_snapshot failed: {error}"))
    }

    /// An account's storage balance on a contract (`None` if unregistered).
    pub async fn storage_balance_of(
        &self,
        contract_id: &AccountId,
        account_id: &AccountId,
    ) -> Result<Option<templar_gateway_types::common::StorageBalance>> {
        Ok(self
            .client()?
            .read(storage::GetBalanceOf {
                contract_id: contract_id.clone(),
                account_id: account_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("storage_balance_of failed: {error}"))?
            .balance)
    }

    /// Withdraw collateral from a borrow position.
    pub async fn withdraw_collateral(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        amount: u128,
    ) -> Result<WriteOperationResult> {
        self.execute(
            user,
            market::WithdrawCollateral {
                market_id: market.market_id.clone(),
                amount: CollateralAssetAmount::new(amount),
            },
        )
        .await
    }

    /// Request a supply withdrawal (queued; executed by
    /// [`execute_next_supply_withdrawal_request`](Self::execute_next_supply_withdrawal_request)).
    pub async fn create_supply_withdrawal_request(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        amount: u128,
    ) -> Result<WriteOperationResult> {
        self.execute(
            user,
            market::CreateSupplyWithdrawalRequest {
                market_id: market.market_id.clone(),
                amount: BorrowAssetAmount::new(amount),
            },
        )
        .await
    }

    /// Attempt to enqueue a supply withdrawal, returning the (possibly failed)
    /// operation result for tests where the contract is expected to reject it.
    pub async fn try_create_supply_withdrawal_request(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        amount: u128,
    ) -> Result<WriteOperationResult> {
        self.try_execute(
            user,
            market::CreateSupplyWithdrawalRequest {
                market_id: market.market_id.clone(),
                amount: BorrowAssetAmount::new(amount),
            },
        )
        .await
    }

    /// Execute the next queued supply withdrawal request.
    pub async fn execute_next_supply_withdrawal_request(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        batch_limit: Option<u32>,
    ) -> Result<WriteOperationResult> {
        self.execute(
            user,
            market::ExecuteNextSupplyWithdrawalRequest {
                market_id: market.market_id.clone(),
                batch_limit,
            },
        )
        .await
    }

    /// [`execute_next_supply_withdrawal_request`](Self::execute_next_supply_withdrawal_request)
    /// without asserting receipt-level success.
    ///
    /// Dequeuing tolerates a payout transfer that cannot land (an unregistered
    /// recipient): the request is still removed from the queue. That is a failed
    /// receipt under an overall-successful transaction, which the strict path
    /// rejects — so a test exercising it must opt out and assert the failure it
    /// expects with [`failed_receipts`].
    pub async fn try_execute_next_supply_withdrawal_request(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        batch_limit: Option<u32>,
    ) -> Result<WriteOperationResult> {
        self.try_execute(
            user,
            market::ExecuteNextSupplyWithdrawalRequest {
                market_id: market.market_id.clone(),
                batch_limit,
            },
        )
        .await
    }

    /// Read a fungible token balance.
    pub async fn ft_balance_of(
        &self,
        token_id: &AccountId,
        account_id: &AccountId,
    ) -> Result<u128> {
        Ok(self
            .client()?
            .read(ft::GetBalanceOf {
                contract_id: token_id.clone(),
                account_id: account_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("ft_balance_of failed: {error}"))?
            .balance
            .0)
    }

    /// Read an account's balance of a market asset, dispatching on the asset's
    /// standard (NEP-141 `ft_balance_of` or NEP-245 `mt_balance_of`).
    pub async fn asset_balance_of<T: AssetClass>(
        &self,
        asset: &FungibleAsset<T>,
        account_id: &AccountId,
    ) -> Result<u128> {
        if let Some((contract_id, token_id)) = asset.clone().into_nep245() {
            Ok(self
                .client()?
                .read(mt::GetBalanceOf {
                    contract_id,
                    account_id: account_id.clone(),
                    token_id,
                })
                .await
                .map_err(|error| anyhow::anyhow!("mt_balance_of failed: {error}"))?
                .balance
                .0)
        } else if let Some(contract_id) = asset.clone().into_nep141() {
            self.ft_balance_of(&contract_id, account_id).await
        } else {
            anyhow::bail!("asset is neither NEP-141 nor NEP-245")
        }
    }

    /// Transfer fungible tokens and call the receiver (raw NEP-141
    /// `ft_transfer_call`). Unlike [`supply`](Self::supply)/etc. this does NOT
    /// pre-register the receiver, and does NOT assert success — use it to
    /// exercise the contract's own registration/validation on the deposit path
    /// (where a rejecting receiver makes `ft_on_transfer` fail and the FT refund,
    /// so the operation reports `Failed` despite the refund).
    pub async fn ft_transfer_call(
        &self,
        user: &ManagedAccountId,
        token_id: &AccountId,
        receiver_id: &AccountId,
        amount: u128,
        msg: String,
    ) -> Result<WriteOperationResult> {
        self.try_execute(
            user,
            ft::TransferCall {
                contract_id: token_id.clone(),
                receiver_id: receiver_id.clone(),
                amount: SU128::from(amount),
                msg,
                memo: None,
            },
        )
        .await
    }

    /// Unregister `user` from storage on `contract_id`.
    pub async fn storage_unregister(
        &self,
        user: &ManagedAccountId,
        contract_id: &AccountId,
        force: bool,
    ) -> Result<WriteOperationResult> {
        self.execute(
            user,
            storage::Unregister {
                contract_id: contract_id.clone(),
                force,
            },
        )
        .await
    }

    /// Advance the sandbox by `blocks` blocks via `sandbox_fast_forward` (over
    /// RPC, so it works in both owned and attach modes), for deterministic
    /// snapshot/time control instead of wall-clock waits.
    pub async fn fast_forward(&self, blocks: u64) -> Result<()> {
        let target = self.latest_block().await?.height + blocks;

        crate::sandbox_ext::fast_forward(&self.network, blocks).await?;

        let start = std::time::Instant::now();
        loop {
            if self.latest_block().await?.height >= target {
                return Ok(());
            }
            anyhow::ensure!(
                start.elapsed() < Duration::from_secs(30),
                "fast_forward timed out waiting for block {target}",
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The current *on-chain* time.
    ///
    /// Any test data whose freshness a contract validates (oracle publish
    /// times, TTLs) must be stamped against this, never the host clock:
    /// [`fast_forward`](Self::fast_forward) advances a node's chain clock
    /// permanently, and the sandbox pool reuses a node across the tests that
    /// pass through its slot — so by the time a test runs, chain time may be
    /// arbitrarily far ahead of wall-clock time, and a host-stamped "now" reads
    /// on-chain as ancient.
    pub async fn chain_timestamp(&self) -> Result<Nanoseconds> {
        Ok(Nanoseconds::from_ns(
            self.latest_block().await?.timestamp_ns,
        ))
    }

    async fn latest_block(&self) -> Result<BlockSummary> {
        self.client()?
            .read(chain::GetBlock::default())
            .await
            .map_err(|error| anyhow::anyhow!("failed to read latest block: {error}"))
    }

    /// Total gas burnt across every transaction an operation produced (each
    /// transaction plus its receipts), summed over the operation's steps. Read
    /// directly from each step's inline
    /// [`ExecutionOutcome`](templar_gateway_types::operation::ExecutionOutcome), whose
    /// `total_gas_burnt` already covers the transaction and all its receipts — no
    /// follow-up `tx` query needed. Used by the gas-regression tests.
    pub fn operation_gas_burnt(&self, result: &WriteOperationResult) -> u64 {
        result
            .operation
            .steps
            .iter()
            .filter_map(|step| match &step.status {
                StepStatus::Succeeded { outcome, .. } | StepStatus::Reverted { outcome, .. } => {
                    Some(outcome.total_gas_burnt.as_gas())
                }
                _ => None,
            })
            .sum()
    }

    /// Fetch the market's current oracle prices (the `OracleResponse` shape the
    /// market expects), by reading its oracle directly.
    pub async fn get_oracle_prices(&self, market: &DeployedMarket) -> Result<OracleResponse> {
        let oracle = &market.configuration.price_oracle_configuration;
        self.oracle_ema_prices(
            &oracle.account_id,
            vec![
                oracle.borrow_asset_price_id,
                oracle.collateral_asset_price_id,
            ],
            oracle.price_maximum_age_s.into(),
        )
        .await
    }

    /// Read an oracle's cached EMA prices no older than `age` seconds. Works for
    /// any contract serving the pyth read interface — a mock oracle, a real pyth
    /// oracle, or a proxy oracle reading back its own aggregation cache.
    ///
    /// Not usable on the LST oracle, whose `list_ema_prices_no_older_than`
    /// returns a `PromiseOrValue` and so cannot run as a view — drive that one
    /// through [`call_function_json`](Self::call_function_json).
    pub async fn oracle_ema_prices(
        &self,
        oracle_id: &AccountId,
        price_ids: Vec<templar_common::oracle::pyth::PriceIdentifier>,
        age: u64,
    ) -> Result<OracleResponse> {
        Ok(self
            .client()?
            .read(pyth::ListEmaPricesNoOlderThan {
                oracle_id: oracle_id.clone(),
                price_ids,
                age,
            })
            .await
            .map_err(|error| anyhow::anyhow!("oracle_ema_prices failed: {error}"))?
            .prices
            .into_iter()
            .map(|entry| (entry.price_id, entry.price))
            .collect())
    }

    /// Read a contract's stored/target state versions and its own
    /// `needs_migration` answer.
    pub async fn contract_state_version(
        &self,
        contract_id: &AccountId,
    ) -> Result<contract::GetStateVersionResult> {
        self.client()?
            .read(contract::GetStateVersion {
                contract_id: contract_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("contract_state_version failed: {error}"))
    }

    /// Read a contract's NEP-330 version string.
    pub async fn contract_version(&self, contract_id: &AccountId) -> Result<String> {
        Ok(self
            .client()?
            .read(contract::GetVersion {
                contract_id: contract_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("contract_version failed: {error}"))?
            .version_string)
    }

    /// The account's deployed code hash, as a stable string for equality checks
    /// (a change proves the code was actually replaced).
    pub async fn code_hash(&self, account_id: &AccountId) -> Result<String> {
        Ok(self
            .client()?
            .read(account::Get {
                account_id: account_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("code_hash failed: {error}"))?
            .code_hash)
    }

    /// Deploy raw wasm to `account_id` with no init call. Signed by the account
    /// itself, which is the only key that can deploy to it.
    pub async fn deploy_code(&self, account_id: &AccountId, code: Vec<u8>) -> Result<()> {
        self.execute(
            &ManagedAccountId(account_id.clone()),
            tx::DeployContract {
                account_id: account_id.clone(),
                code: Base64Bytes(code),
            },
        )
        .await?;
        Ok(())
    }

    /// Deploy wasm to `account_id` and run `method_name` in the same
    /// transaction, so a failing init reverts the deploy with it. Returns the
    /// operation result without asserting success — the atomicity tests need the
    /// failing case.
    pub async fn try_deploy_and_init(
        &self,
        account_id: &AccountId,
        code: Vec<u8>,
        method_name: &str,
        args: impl serde::Serialize,
    ) -> Result<WriteOperationResult> {
        self.try_execute(
            &ManagedAccountId(account_id.clone()),
            tx::DeployAndInit {
                account_id: account_id.clone(),
                code: Base64Bytes(code),
                method_name: ContractMethodName(method_name.to_owned()),
                args: ContractArgs::Json(serde_json::to_value(args)?),
                gas: NearGas::from_tgas(300),
                deposit: near_token::NearToken::from_yoctonear(0),
            },
        )
        .await
    }

    /// [`try_deploy_and_init`](Self::try_deploy_and_init), asserting success.
    pub async fn deploy_and_init(
        &self,
        account_id: &AccountId,
        code: Vec<u8>,
        method_name: &str,
        args: impl serde::Serialize,
    ) -> Result<()> {
        let result = self
            .try_deploy_and_init(account_id, code, method_name, args)
            .await?;
        anyhow::ensure!(
            result.operation.status == OperationStatus::Succeeded,
            "deploy+{method_name} on {account_id} failed: {}",
            result
                .operation
                .failure_message()
                .unwrap_or("<no failure message>"),
        );
        Ok(())
    }

    /// The generic write escape hatch: call `method_name` on `contract_id` as
    /// `signer`. For contract methods with no typed gateway operation — mock-only
    /// setters, and reads the contract exposes as promise-returning calls.
    pub async fn call_function(
        &self,
        signer: &ManagedAccountId,
        contract_id: &AccountId,
        method_name: &str,
        args: impl serde::Serialize,
    ) -> Result<WriteOperationResult> {
        self.execute(signer, Self::function_call(contract_id, method_name, args)?)
            .await
    }

    /// [`call_function`](Self::call_function) without the success assertion, for
    /// tests that expect the contract to reject the call.
    pub async fn try_call_function(
        &self,
        signer: &ManagedAccountId,
        contract_id: &AccountId,
        method_name: &str,
        args: impl serde::Serialize,
    ) -> Result<WriteOperationResult> {
        self.try_execute(signer, Self::function_call(contract_id, method_name, args)?)
            .await
    }

    /// [`call_function`](Self::call_function), deserializing the call's return
    /// value. This is how a `PromiseOrValue`-returning "read" (the LST oracle's
    /// `price_feed_exists` and `list_ema_prices_no_older_than`, which fan out to
    /// the underlying oracle and so cannot run as views) is read.
    pub async fn call_function_json<T: serde::de::DeserializeOwned>(
        &self,
        signer: &ManagedAccountId,
        contract_id: &AccountId,
        method_name: &str,
        args: impl serde::Serialize,
    ) -> Result<T> {
        let result = self
            .call_function(signer, contract_id, method_name, args)
            .await?;
        let bytes = result
            .operation
            .final_outcome()
            .and_then(|outcome| outcome.return_value.as_ref())
            .with_context(|| format!("{contract_id}.{method_name} returned no value"))?;
        serde_json::from_slice(&bytes.0)
            .with_context(|| format!("failed to decode {contract_id}.{method_name} return value"))
    }

    fn function_call(
        contract_id: &AccountId,
        method_name: &str,
        args: impl serde::Serialize,
    ) -> Result<tx::FunctionCall> {
        Ok(tx::FunctionCall {
            receiver_id: contract_id.clone(),
            method_name: ContractMethodName(method_name.to_owned()),
            args: ContractArgs::Json(serde_json::to_value(args)?),
            gas: NearGas::from_tgas(300),
            deposit: near_token::NearToken::from_yoctonear(0),
        })
    }

    /// Read an account's borrow status given an oracle response.
    pub async fn get_borrow_status(
        &self,
        market: &DeployedMarket,
        account_id: &AccountId,
        oracle_response: OracleResponse,
    ) -> Result<Option<BorrowStatus>> {
        Ok(self
            .client()?
            .read(market::GetBorrowStatus {
                market_id: market.market_id.clone(),
                account_id: account_id.clone(),
                oracle_response,
            })
            .await
            .map_err(|error| anyhow::anyhow!("get_borrow_status failed: {error}"))?
            .status)
    }

    /// Read a borrow position.
    pub async fn get_borrow_position(
        &self,
        market: &DeployedMarket,
        account_id: &AccountId,
    ) -> Result<Option<BorrowPosition>> {
        Ok(self
            .client()?
            .read(market::GetBorrowPosition {
                market_id: market.market_id.clone(),
                account_id: account_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("get_borrow_position failed: {error}"))?
            .position)
    }

    /// Read a supply position.
    pub async fn get_supply_position(
        &self,
        market: &DeployedMarket,
        account_id: &AccountId,
    ) -> Result<Option<SupplyPosition>> {
        Ok(self
            .client()?
            .read(market::GetSupplyPosition {
                market_id: market.market_id.clone(),
                account_id: account_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("get_supply_position failed: {error}"))?
            .position)
    }

    /// Read the supply withdrawal queue status.
    pub async fn supply_withdrawal_queue_status(
        &self,
        market: &DeployedMarket,
    ) -> Result<WithdrawalQueueStatus> {
        self.client()?
            .read(market::GetSupplyWithdrawalQueueStatus {
                market_id: market.market_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("supply_withdrawal_queue_status failed: {error}"))
    }

    /// Read an account's supply withdrawal request status.
    pub async fn supply_withdrawal_request_status(
        &self,
        market: &DeployedMarket,
        account_id: &AccountId,
    ) -> Result<Option<WithdrawalRequestStatus>> {
        Ok(self
            .client()?
            .read(market::GetSupplyWithdrawalRequestStatus {
                market_id: market.market_id.clone(),
                account_id: account_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("supply_withdrawal_request_status failed: {error}"))?
            .status)
    }

    /// Count finalized snapshots.
    pub async fn get_finalized_snapshots_len(&self, market: &DeployedMarket) -> Result<u32> {
        self.client()?
            .read(market::GetFinalizedSnapshotsLen {
                market_id: market.market_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("get_finalized_snapshots_len failed: {error}"))
    }

    /// Plan, sign, and submit a write operation as `signer`, asserting it
    /// succeeds. A contract panic does not surface as an `Err` from the gateway
    /// (the driver records the operation as `Failed` and returns `Ok`), so the
    /// status check here is what turns an unexpected on-chain failure into a
    /// test failure.
    ///
    /// Success is checked at *receipt* level, not just top level. Every
    /// supply/collateralize/repay/liquidate is an `ft_transfer_call`: if
    /// `ft_on_transfer` panics, the token catches it and refunds, and the
    /// transaction still reports top-level success. An operation that did
    /// nothing would otherwise satisfy this assertion, and the test would pass
    /// while exercising nothing. Tests that *expect* a rejection must use
    /// [`try_execute`](Self::try_execute) instead.
    pub async fn execute<Op>(
        &self,
        signer: &ManagedAccountId,
        op: Op,
    ) -> Result<WriteOperationResult>
    where
        Op: templar_gateway_types::MethodSpec<Output = WriteOperationResult>,
        templar_gateway_methods_dispatch::Dispatch:
            templar_gateway_core::PlanWrite<Op, templar_gateway_core::GatewayContext>,
    {
        let result = self.try_execute(signer, op).await?;
        anyhow::ensure!(
            result.operation.status == OperationStatus::Succeeded,
            "operation {} did not succeed (status: {:?}): {}",
            result.operation.id.0,
            result.operation.status,
            result
                .operation
                .failure_message()
                .unwrap_or("<no failure message>"),
        );

        let failed: Vec<_> = failed_receipts(&result).collect();
        anyhow::ensure!(
            failed.is_empty(),
            "operation {} reported top-level success but {} receipt(s) failed \
             (executed by: {}) — the call was refunded and did nothing",
            result.operation.id.0,
            failed.len(),
            failed
                .iter()
                .map(|receipt| receipt.contract_id.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        Ok(result)
    }

    /// Like [`execute`](Self::execute) but returns the operation result without
    /// asserting success — for tests that expect a contract rejection. Only
    /// errors on a planning/submission failure, not an on-chain one.
    pub async fn try_execute<Op>(
        &self,
        signer: &ManagedAccountId,
        op: Op,
    ) -> Result<WriteOperationResult>
    where
        Op: templar_gateway_types::MethodSpec<Output = WriteOperationResult>,
        templar_gateway_methods_dispatch::Dispatch:
            templar_gateway_core::PlanWrite<Op, templar_gateway_core::GatewayContext>,
    {
        self.client()?
            .execute_as(signer.clone(), op)
            .await
            .map_err(|error| anyhow::anyhow!("operation submission failed: {error}"))
    }
}

/// Every receipt in the operation that failed.
///
/// Top-level success is not receipt-level success (see
/// [`ExecutionOutcome`](templar_gateway_types::operation::ExecutionOutcome) and
/// `OperationStatus`): a rejected inner receipt can be refunded by the token
/// while the transaction still reports success. Tests asserting that a call was
/// *rejected* should check this rather than the operation status, which would be
/// `Succeeded`.
pub fn failed_receipts(result: &WriteOperationResult) -> impl Iterator<Item = &ReceiptOutcome> {
    result
        .operation
        .steps
        .iter()
        .filter_map(|step| match &step.status {
            StepStatus::Succeeded { outcome, .. } | StepStatus::Reverted { outcome, .. } => {
                Some(outcome)
            }
            StepStatus::NotStarted
            | StepStatus::Prepared { .. }
            | StepStatus::Submitted { .. }
            | StepStatus::Rejected { .. } => None,
        })
        .flat_map(|outcome| outcome.receipts.iter())
        .filter(|receipt| receipt.status == ReceiptStatus::Failed)
}
