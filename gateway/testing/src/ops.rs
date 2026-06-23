//! Test-driving operations layered on the [`SandboxHarness`].
//!
//! These wrap the in-process [`templar_gateway_client::Client`] so test bodies
//! read as terse domain actions (`harness.supply(&user, &market, 1_000)`)
//! rather than plan/sign/submit boilerplate — the direct-client equivalent of
//! the retired `test-utils` controllers. Reads and writes both flow through the
//! same gateway dispatch the RPC service uses, so tests exercise production code
//! paths.

use anyhow::{Context, Result};
use near_api::types::AccountId;
use near_token::NearToken;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
    market::{HarvestYieldMode, MarketConfiguration},
    supply::SupplyPosition,
};
use templar_gateway_client::Client;
use templar_gateway_methods_spec::{market, storage, tx};
use templar_gateway_runtime::ManagedSigner;
use templar_gateway_types::{
    common::{ContractArgs, WriteOperationResult},
    ContractMethodName, ManagedAccountId, NearGas, U128,
};

use crate::sandbox::{test_secret_key, SandboxHarness};

/// A market deployed by [`SandboxHarness::deploy_full_market`], with the asset
/// and oracle accounts resolved from its configuration for convenient access.
pub struct DeployedMarket {
    pub market_id: AccountId,
    pub borrow_ft_id: AccountId,
    pub collateral_ft_id: AccountId,
    pub configuration: MarketConfiguration,
}

impl SandboxHarness {
    /// Build a fresh in-process gateway [`Client`] over every account the
    /// harness can currently sign as. Rebuilt per call so newly-created users
    /// are always available; cheap (no network I/O) for tests.
    pub fn client(&self) -> Result<Client> {
        let mut builder = Client::builder(self.network.clone());
        for (account_id, managed) in self.signers_snapshot() {
            builder = builder.signer(account_id, managed.signer.clone());
        }
        builder
            .build()
            .map_err(|error| anyhow::anyhow!("failed to build gateway client: {error}"))
    }

    /// Create a funded sub-account with a unique id and register its signer so
    /// the harness can drive operations as it. Owned-mode (top-level `*.near`).
    pub async fn create_user(&self, prefix: &str) -> Result<ManagedAccountId> {
        let seq = self.next_account_seq();
        let account_id: AccountId = format!("{prefix}-{seq}.near").parse()?;
        let secret_key = test_secret_key()?;
        self.sandbox
            .create_account(account_id.clone())
            .initial_balance(NearToken::from_near(100))
            .public_key(secret_key.public_key().to_string())
            .send()
            .await?;

        let managed = ManagedSigner::new([secret_key])
            .await
            .context("failed to initialize user signer")?;
        let managed_account_id = ManagedAccountId(account_id);
        self.register_signer(managed_account_id.clone(), managed);
        Ok(managed_account_id)
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
        let borrow_ft_id = configuration
            .borrow_asset
            .clone()
            .into_nep141()
            .context("borrow asset is not a NEP-141 token")?;
        let collateral_ft_id = configuration
            .collateral_asset
            .clone()
            .into_nep141()
            .context("collateral asset is not a NEP-141 token")?;
        Ok(DeployedMarket {
            market_id,
            borrow_ft_id,
            collateral_ft_id,
            configuration,
        })
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

    /// Mint `amount` of a mock fungible token to `user` (the mock FT mints to
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
                args: ContractArgs::Json(serde_json::json!({ "amount": U128(amount) })),
                gas: NearGas::from_tgas(20),
                deposit: NearToken::from_yoctonear(0),
            },
        )
        .await
    }

    /// Register `user` on the market and both FTs, then mint it a large balance
    /// of both assets — the setup every borrowing/supplying user needs.
    pub async fn fund_user(&self, user: &ManagedAccountId, market: &DeployedMarket) -> Result<()> {
        const MINT_AMOUNT: u128 = 100_000_000;
        let ft_registration = NearToken::from_near(1).saturating_div(100);

        self.storage_deposit(user, &market.borrow_ft_id, ft_registration)
            .await?;
        self.storage_deposit(user, &market.collateral_ft_id, ft_registration)
            .await?;
        self.mint(user, &market.borrow_ft_id, MINT_AMOUNT).await?;
        self.mint(user, &market.collateral_ft_id, MINT_AMOUNT)
            .await?;
        Ok(())
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

    /// Supply, then harvest until the deposit is fully activated (no longer in
    /// the `incoming` bucket) — mirrors the old controller helper.
    pub async fn supply_and_harvest_until_activation(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        amount: u128,
    ) -> Result<()> {
        self.supply(user, market, amount).await?;
        while !self
            .get_supply_position(market, &user.0)
            .await?
            .context("supply position missing after supply")?
            .get_deposit()
            .incoming
            .is_empty()
        {
            self.harvest_yield(user, market, Some(user.0.clone()))
                .await?;
        }
        Ok(())
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

    /// Harvest supply yield for `account_id`.
    pub async fn harvest_yield(
        &self,
        user: &ManagedAccountId,
        market: &DeployedMarket,
        account_id: Option<AccountId>,
    ) -> Result<WriteOperationResult> {
        self.execute(
            user,
            market::HarvestYield {
                market_id: market.market_id.clone(),
                account_id,
                mode: Some(HarvestYieldMode::Default),
            },
        )
        .await
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

    /// Count finalized snapshots.
    pub async fn get_finalized_snapshots_len(&self, market: &DeployedMarket) -> Result<u32> {
        self.client()?
            .read(market::GetFinalizedSnapshotsLen {
                market_id: market.market_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("get_finalized_snapshots_len failed: {error}"))
    }

    /// Plan, sign, and submit a write operation as `signer`, asserting success.
    async fn execute<Op>(&self, signer: &ManagedAccountId, op: Op) -> Result<WriteOperationResult>
    where
        Op: templar_gateway_types::MethodSpec<Output = WriteOperationResult>,
        templar_gateway_methods_dispatch::Dispatch:
            templar_gateway_core::PlanWrite<Op, templar_gateway_core::GatewayContext>,
    {
        let result = self
            .client()?
            .execute_as(signer.clone(), op)
            .await
            .map_err(|error| anyhow::anyhow!("operation failed: {error}"))?;
        Ok(result)
    }
}
