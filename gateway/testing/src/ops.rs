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
    borrow::{BorrowPosition, BorrowStatus},
    market::{HarvestYieldMode, MarketConfiguration},
    oracle::pyth::OracleResponse,
    supply::SupplyPosition,
    withdrawal_queue::{WithdrawalQueueStatus, WithdrawalRequestStatus},
};
use templar_gateway_client::Client;
use templar_gateway_methods_spec::{ft, market, storage, tx};
use templar_gateway_types::{
    common::{ContractArgs, WriteOperationResult},
    ContractMethodName, ManagedAccountId, NearGas, OperationStatus, U128,
};

use test_utils::to_price;

use crate::sandbox::SandboxHarness;

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

    /// Top up `user`'s storage deposit on `contract_id` by its minimum bound —
    /// the amount the market charges per new supply/borrow position. Unlike
    /// registration this is additive, so it covers a position re-created after a
    /// prior one (and its snapshot storage) was charged.
    pub async fn storage_deposit_min(
        &self,
        user: &ManagedAccountId,
        contract_id: &AccountId,
    ) -> Result<WriteOperationResult> {
        let bounds = self
            .client()?
            .read(storage::GetBalanceBounds {
                contract_id: contract_id.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("storage_balance_bounds failed: {error}"))?
            .bounds;
        self.storage_deposit(user, contract_id, bounds.min).await
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
                amount: U128(amount),
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
                amount: U128(amount),
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
        let url = self.network.rpc_endpoints[0].url.clone();
        let client = reqwest::Client::new();
        let target = rpc_block_height(&client, &url).await? + blocks;
        client
            .post(url.clone())
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": "fast_forward",
                "method": "sandbox_fast_forward",
                "params": { "delta_height": blocks },
            }))
            .send()
            .await?
            .error_for_status()?;

        let start = std::time::Instant::now();
        loop {
            if rpc_block_height(&client, &url).await? >= target {
                return Ok(());
            }
            anyhow::ensure!(
                start.elapsed() < std::time::Duration::from_secs(30),
                "fast_forward timed out waiting for block {target}",
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    /// Fetch the market's current oracle prices (the `OracleResponse` shape the
    /// market expects), by reading the mock oracle directly.
    pub async fn get_oracle_prices(&self, market: &DeployedMarket) -> Result<OracleResponse> {
        let oracle = &market.configuration.price_oracle_configuration;
        let args = serde_json::to_vec(&serde_json::json!({
            "price_ids": [oracle.borrow_asset_price_id, oracle.collateral_asset_price_id],
            "age": oracle.price_maximum_age_s,
        }))?;
        self.gateway_client()
            .contract(oracle.account_id.clone())
            .view_function::<OracleResponse>("list_ema_prices_no_older_than", args)
            .await
            .map_err(|error| anyhow::anyhow!("get_oracle_prices failed: {error}"))
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
    async fn execute<Op>(&self, signer: &ManagedAccountId, op: Op) -> Result<WriteOperationResult>
    where
        Op: templar_gateway_types::MethodSpec<Output = WriteOperationResult>,
        templar_gateway_methods_dispatch::Dispatch:
            templar_gateway_core::PlanWrite<Op, templar_gateway_core::GatewayContext>,
    {
        let result = self.try_execute(signer, op).await?;
        anyhow::ensure!(
            result.operation.status == OperationStatus::Succeeded,
            "operation {} did not succeed (status: {:?})",
            result.operation.id.0,
            result.operation.status,
        );
        Ok(result)
    }

    /// Like [`execute`](Self::execute) but returns the operation result without
    /// asserting success — for tests that expect a contract rejection. Only
    /// errors on a planning/submission failure, not an on-chain one.
    async fn try_execute<Op>(
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

/// Query the current final block height via JSON-RPC.
async fn rpc_block_height(client: &reqwest::Client, url: &reqwest::Url) -> Result<u64> {
    let response: serde_json::Value = client
        .post(url.clone())
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": "block",
            "method": "block",
            "params": { "finality": "final" },
        }))
        .send()
        .await?
        .json()
        .await?;
    response["result"]["header"]["height"]
        .as_u64()
        .context("missing block height in RPC response")
}
