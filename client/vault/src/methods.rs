//! Macro for generating vault client methods to reduce duplication.
//!
//! This macro generates the common vault methods for any client that implements
//! the required helper methods.

/// Generate vault view and call methods for a client.
///
/// The client must have these helper methods:
/// - `vault_view_u128(&self, method: &str, args: impl Serialize) -> Result<ForeignU128, ErrorWrapper>`
/// - `vault_call(&self, method: &str, args: impl Serialize) -> Result<(), ErrorWrapper>`
/// - `vault_call_with(&self, method: &str, args: impl Serialize, gas: Option<Gas>, deposit: Option<u128>) -> Result<(), ErrorWrapper>`
/// - `vault_call_returning<T>(&self, method: &str, args: impl Serialize, gas: Option<Gas>, deposit: Option<u128>) -> Result<T, ErrorWrapper>`
/// - `view<T>(&self, account_id: &NearAccountId, method: &str, args: impl Serialize) -> Result<T>`
/// - `near_id(&self, id: &AccountId) -> Result<NearAccountId, ErrorWrapper>`
/// - `vault` field of type `NearAccountId`
#[macro_export]
macro_rules! impl_vault_methods {
    ($client:ty) => {
        // =====================================================================
        // Simple U128 View Methods (no args)
        // =====================================================================

        #[uniffi::export(async_runtime = "tokio")]
        impl $client {
            #[instrument(skip(self))]
            pub async fn get_total_assets(&self) -> Result<ForeignU128, ErrorWrapper> {
                self.vault_view_u128("get_total_assets", ()).await
            }

            #[instrument(skip(self))]
            pub async fn get_last_total_assets(&self) -> Result<ForeignU128, ErrorWrapper> {
                self.vault_view_u128("get_last_total_assets", ()).await
            }

            #[instrument(skip(self))]
            pub async fn get_idle_balance(&self) -> Result<ForeignU128, ErrorWrapper> {
                self.vault_view_u128("get_idle_balance", ()).await
            }

            #[instrument(skip(self))]
            pub async fn get_total_supply(&self) -> Result<ForeignU128, ErrorWrapper> {
                self.vault_view_u128("get_total_supply", ()).await
            }

            #[instrument(skip(self))]
            pub async fn get_max_deposit(&self) -> Result<ForeignU128, ErrorWrapper> {
                self.vault_view_u128("get_max_deposit", ()).await
            }

            #[instrument(skip(self))]
            pub async fn get_max_single_market_deposit(&self) -> Result<ForeignU128, ErrorWrapper> {
                self.vault_view_u128("get_max_single_market_deposit", ()).await
            }
        }

        // =====================================================================
        // U128 View Methods (with args)
        // =====================================================================

        #[uniffi::export(async_runtime = "tokio")]
        impl $client {
            #[instrument(skip(self, assets))]
            pub async fn convert_to_shares(&self, assets: &ForeignU128) -> Result<ForeignU128, ErrorWrapper> {
                let assets = U128($crate::parse_u128(assets)?);
                self.vault_view_u128("convert_to_shares", (assets,)).await
            }

            #[instrument(skip(self, shares))]
            pub async fn convert_to_assets(&self, shares: &ForeignU128) -> Result<ForeignU128, ErrorWrapper> {
                let shares = U128($crate::parse_u128(shares)?);
                self.vault_view_u128("convert_to_assets", (shares,)).await
            }

            #[instrument(skip(self, assets))]
            pub async fn preview_deposit(&self, assets: &ForeignU128) -> Result<ForeignU128, ErrorWrapper> {
                let assets = U128($crate::parse_u128(assets)?);
                self.vault_view_u128("preview_deposit", (assets,)).await
            }

            #[instrument(skip(self, shares))]
            pub async fn preview_mint(&self, shares: &ForeignU128) -> Result<ForeignU128, ErrorWrapper> {
                let shares = U128($crate::parse_u128(shares)?);
                self.vault_view_u128("preview_mint", (shares,)).await
            }

            #[instrument(skip(self, assets))]
            pub async fn preview_withdraw(&self, assets: &ForeignU128) -> Result<ForeignU128, ErrorWrapper> {
                let assets = U128($crate::parse_u128(assets)?);
                self.vault_view_u128("preview_withdraw", (assets,)).await
            }

            #[instrument(skip(self, shares))]
            pub async fn preview_redeem(&self, shares: &ForeignU128) -> Result<ForeignU128, ErrorWrapper> {
                let shares = U128($crate::parse_u128(shares)?);
                self.vault_view_u128("preview_redeem", (shares,)).await
            }
        }

        // =====================================================================
        // Typed View Methods
        // =====================================================================

        #[uniffi::export(async_runtime = "tokio")]
        impl $client {
            #[instrument(skip(self))]
            pub async fn get_configuration(&self) -> Result<VaultConfiguration, ErrorWrapper> {
                let cfg = self
                    .view::<templar_common::vault::VaultConfiguration>(&self.vault, "get_configuration", ())
                    .await
                    .map_err(ErrorWrapper::from)?;
                Ok(cfg.into())
            }

            #[instrument(skip(self))]
            pub async fn get_fee_anchor(&self) -> Result<FeeAccrualAnchor, ErrorWrapper> {
                let anchor = self
                    .view::<templar_common::vault::FeeAccrualAnchor>(&self.vault, "get_fee_anchor", ())
                    .await
                    .map_err(ErrorWrapper::from)?;
                Ok(anchor.into())
            }

            #[instrument(skip(self))]
            pub async fn get_fees(&self) -> Result<Fees, ErrorWrapper> {
                let fees = self
                    .view::<templar_common::vault::Fees<U128>>(&self.vault, "get_fees", ())
                    .await
                    .map_err(ErrorWrapper::from)?;
                Ok(fees.into())
            }

            #[instrument(skip(self))]
            pub async fn get_restrictions(&self) -> Result<Option<Restrictions>, ErrorWrapper> {
                let r = self
                    .view::<Option<templar_common::vault::Restrictions>>(&self.vault, "get_restrictions", ())
                    .await
                    .map_err(ErrorWrapper::from)?;
                Ok(r.map(Into::into))
            }

            #[instrument(skip(self))]
            pub async fn get_withdrawing_op_id(&self) -> Result<Option<u64>, ErrorWrapper> {
                let res = self
                    .view::<Option<U64>>(&self.vault, "get_withdrawing_op_id", ())
                    .await
                    .map_err(ErrorWrapper::from)?;
                Ok(res.map(|u| u.0))
            }

            #[instrument(skip(self))]
            pub async fn has_pending_market_withdrawal(&self) -> Result<bool, ErrorWrapper> {
                self.view(&self.vault, "has_pending_market_withdrawal", ())
                    .await
                    .map_err(ErrorWrapper::from)
            }

            #[instrument(skip(self))]
            pub async fn get_current_withdraw_request_id(&self) -> Result<Option<u64>, ErrorWrapper> {
                let res = self
                    .view::<Option<U64>>(&self.vault, "get_current_withdraw_request_id", ())
                    .await
                    .map_err(ErrorWrapper::from)?;
                Ok(res.map(|u| u.0))
            }

            #[instrument(skip(self))]
            pub async fn queue_tail(&self) -> Result<u64, ErrorWrapper> {
                self.view(&self.vault, "queue_tail", ())
                    .await
                    .map_err(ErrorWrapper::from)
            }

            #[instrument(skip(self))]
            pub async fn peek_next_pending_withdrawal_id(&self) -> Result<Option<u64>, ErrorWrapper> {
                self.view(&self.vault, "peek_next_pending_withdrawal_id", ())
                    .await
                    .map_err(ErrorWrapper::from)
            }

            #[instrument(skip(self))]
            pub async fn build_real_assets_report(&self) -> Result<RealAssetsReport, ErrorWrapper> {
                let res = self
                    .view::<templar_common::vault::RealAssetsReport>(&self.vault, "build_real_assets_report", ())
                    .await
                    .map_err(ErrorWrapper::from)?;
                Ok(res.into())
            }
        }

        // =====================================================================
        // Simple Call Methods (no args, no deposit)
        // =====================================================================

        #[uniffi::export(async_runtime = "tokio")]
        impl $client {
            #[instrument(skip(self))]
            pub async fn accept_guardian(&self) -> Result<(), ErrorWrapper> {
                self.vault_call("accept_guardian", ()).await
            }

            #[instrument(skip(self))]
            pub async fn revoke_pending_guardian(&self) -> Result<(), ErrorWrapper> {
                self.vault_call("revoke_pending_guardian", ()).await
            }

            #[instrument(skip(self))]
            pub async fn accept_sentinel(&self) -> Result<(), ErrorWrapper> {
                self.vault_call("accept_sentinel", ()).await
            }

            #[instrument(skip(self))]
            pub async fn revoke_pending_sentinel(&self) -> Result<(), ErrorWrapper> {
                self.vault_call("revoke_pending_sentinel", ()).await
            }

            #[instrument(skip(self))]
            pub async fn accept_fees(&self) -> Result<(), ErrorWrapper> {
                self.vault_call("accept_fees", ()).await
            }

            #[instrument(skip(self))]
            pub async fn revoke_pending_fees(&self) -> Result<(), ErrorWrapper> {
                self.vault_call("revoke_pending_fees", ()).await
            }

            #[instrument(skip(self))]
            pub async fn accept_timelock(&self) -> Result<(), ErrorWrapper> {
                self.vault_call("accept_timelock", ()).await
            }

            #[instrument(skip(self))]
            pub async fn revoke_pending_timelock(&self) -> Result<(), ErrorWrapper> {
                self.vault_call("revoke_pending_timelock", ()).await
            }

            #[instrument(skip(self))]
            pub async fn accept_restrictions(&self) -> Result<(), ErrorWrapper> {
                self.vault_call("accept_restrictions", ()).await
            }

            #[instrument(skip(self))]
            pub async fn revoke_pending_restrictions(&self) -> Result<(), ErrorWrapper> {
                self.vault_call("revoke_pending_restrictions", ()).await
            }

            #[instrument(skip(self))]
            pub async fn unbrick(&self) -> Result<(), ErrorWrapper> {
                self.vault_call("unbrick", ()).await
            }
        }

        // =====================================================================
        // Call Methods (with args)
        // =====================================================================

        #[uniffi::export(async_runtime = "tokio")]
        impl $client {
            #[instrument(skip(self, shares, receiver, deposit_yocto))]
            pub async fn redeem(
                &self,
                shares: &ForeignU128,
                receiver: &AccountId,
                deposit_yocto: &ForeignU128,
            ) -> Result<(), ErrorWrapper> {
                let shares = U128($crate::parse_u128(shares)?);
                let deposit = $crate::parse_u128(deposit_yocto)?;
                self.vault_call_with("redeem", (shares, self.near_id(receiver)?), None, Some(deposit)).await
            }

            #[instrument(skip(self, assets, receiver, deposit_yocto))]
            pub async fn withdraw(
                &self,
                assets: &ForeignU128,
                receiver: &AccountId,
                deposit_yocto: &ForeignU128,
            ) -> Result<(), ErrorWrapper> {
                let assets = U128($crate::parse_u128(assets)?);
                let deposit = $crate::parse_u128(deposit_yocto)?;
                self.vault_call_with("withdraw", (assets, self.near_id(receiver)?), None, Some(deposit)).await
            }

            #[instrument(skip(self, delta))]
            pub async fn reallocate(&self, delta: &AllocationDelta) -> Result<(), ErrorWrapper> {
                let delta = templar_common::vault::AllocationDelta::try_from(delta.clone())?;
                self.vault_call("reallocate", (delta,)).await
            }

            #[instrument(skip(self, route))]
            pub async fn execute_withdrawal(&self, route: &[MarketId]) -> Result<(), ErrorWrapper> {
                let route: Vec<templar_common::vault::MarketId> = route.iter().copied().map(Into::into).collect();
                self.vault_call("execute_withdrawal", (route,)).await
            }

            #[instrument(skip(self, op_id, market, batch_limit))]
            pub async fn execute_market_withdrawal(
                &self,
                op_id: u64,
                market: MarketId,
                batch_limit: Option<u32>,
            ) -> Result<(), ErrorWrapper> {
                self.vault_call(
                    "execute_market_withdrawal",
                    (U64::from(op_id), templar_common::vault::MarketId::from(market), batch_limit),
                ).await
            }

            #[instrument(skip(self, market_id, batch_limit))]
            pub async fn execute_rebalance_withdrawal(
                &self,
                market_id: MarketId,
                batch_limit: Option<u32>,
            ) -> Result<(), ErrorWrapper> {
                self.vault_call(
                    "execute_rebalance_withdrawal",
                    (templar_common::vault::MarketId::from(market_id), batch_limit),
                ).await
            }

            #[instrument(skip(self, token))]
            pub async fn skim(&self, token: &AccountId) -> Result<(), ErrorWrapper> {
                self.vault_call("skim", (self.near_id(token)?,)).await
            }

            #[instrument(skip(self, markets))]
            pub async fn refresh_markets(&self, markets: &[MarketId]) -> Result<RealAssetsReport, ErrorWrapper> {
                let markets: Vec<templar_common::vault::MarketId> = markets.iter().copied().map(Into::into).collect();
                let report: templar_common::vault::RealAssetsReport = self
                    .vault_call_returning("refresh_markets", (markets,), None, None)
                    .await?;
                Ok(report.into())
            }

            #[instrument(skip(self, account))]
            pub async fn set_curator(&self, account: &AccountId) -> Result<(), ErrorWrapper> {
                self.vault_call("set_curator", (self.near_id(account)?,)).await
            }

            #[instrument(skip(self, account))]
            pub async fn set_is_allocator(&self, account: &AccountId, allowed: bool) -> Result<(), ErrorWrapper> {
                self.vault_call("set_is_allocator", (self.near_id(account)?, allowed)).await
            }

            #[instrument(skip(self, new_g))]
            pub async fn submit_guardian(&self, new_g: &AccountId) -> Result<(), ErrorWrapper> {
                self.vault_call("submit_guardian", (self.near_id(new_g)?,)).await
            }

            #[instrument(skip(self, new_s))]
            pub async fn submit_sentinel(&self, new_s: &AccountId) -> Result<(), ErrorWrapper> {
                self.vault_call("submit_sentinel", (self.near_id(new_s)?,)).await
            }

            #[instrument(skip(self, account))]
            pub async fn set_skim_recipient(&self, account: &AccountId) -> Result<(), ErrorWrapper> {
                self.vault_call("set_skim_recipient", (self.near_id(account)?,)).await
            }

            #[instrument(skip(self, fees))]
            pub async fn set_fees(&self, fees: Fees) -> Result<(), ErrorWrapper> {
                let fees: templar_common::vault::Fees<U128> = fees.try_into()?;
                self.vault_call("set_fees", (fees,)).await
            }

            #[instrument(skip(self, new_timelock_ns, kind))]
            pub async fn submit_timelock(&self, new_timelock_ns: u64, kind: Option<TimelockKind>) -> Result<(), ErrorWrapper> {
                self.vault_call("submit_timelock", (U64::from(new_timelock_ns), kind)).await
            }

            #[instrument(skip(self, market, new_cap))]
            pub async fn submit_cap(&self, market: &AccountId, new_cap: &ForeignU128) -> Result<(), ErrorWrapper> {
                let new_cap = U128($crate::parse_u128(new_cap)?);
                self.vault_call("submit_cap", (self.near_id(market)?, new_cap)).await
            }

            #[instrument(skip(self, market))]
            pub async fn accept_cap(&self, market: &AccountId) -> Result<(), ErrorWrapper> {
                self.vault_call("accept_cap", (self.near_id(market)?,)).await
            }

            #[instrument(skip(self, market))]
            pub async fn revoke_pending_cap(&self, market: &AccountId) -> Result<(), ErrorWrapper> {
                self.vault_call("revoke_pending_cap", (self.near_id(market)?,)).await
            }

            #[instrument(skip(self, update))]
            pub async fn submit_cap_group_update(&self, update: CapGroupUpdate) -> Result<(), ErrorWrapper> {
                let update: templar_common::vault::CapGroupUpdate = update.try_into()?;
                self.vault_call("submit_cap_group_update", (update,)).await
            }

            #[instrument(skip(self, update))]
            pub async fn accept_cap_group_update(&self, update: CapGroupUpdateKey) -> Result<(), ErrorWrapper> {
                let key: templar_common::vault::CapGroupUpdateKey = update.into();
                self.vault_call("accept_cap_group_update", (key,)).await
            }

            #[instrument(skip(self, update))]
            pub async fn revoke_pending_cap_group_update(&self, update: CapGroupUpdateKey) -> Result<(), ErrorWrapper> {
                let key: templar_common::vault::CapGroupUpdateKey = update.into();
                self.vault_call("revoke_pending_cap_group_update", (key,)).await
            }

            #[instrument(skip(self, restrictions))]
            pub async fn set_restrictions(&self, restrictions: Option<Restrictions>) -> Result<(), ErrorWrapper> {
                let r: Option<templar_common::vault::Restrictions> = restrictions.map(TryInto::try_into).transpose()?;
                self.vault_call("set_restrictions", (r,)).await
            }

            #[instrument(skip(self, market))]
            pub async fn submit_market_removal(&self, market: &AccountId) -> Result<(), ErrorWrapper> {
                self.vault_call("submit_market_removal", (self.near_id(market)?,)).await
            }

            #[instrument(skip(self, market))]
            pub async fn accept_market_removal(&self, market: &AccountId) -> Result<(), ErrorWrapper> {
                self.vault_call("accept_market_removal", (self.near_id(market)?,)).await
            }

            #[instrument(skip(self, market))]
            pub async fn revoke_pending_market_removal(&self, market: &AccountId) -> Result<(), ErrorWrapper> {
                self.vault_call("revoke_pending_market_removal", (self.near_id(market)?,)).await
            }

            #[instrument(skip(self, markets, deposit_yocto))]
            pub async fn set_supply_queue(&self, markets: &[MarketId], deposit_yocto: &ForeignU128) -> Result<(), ErrorWrapper> {
                let deposit = $crate::parse_u128(deposit_yocto)?;
                let markets: Vec<templar_common::vault::MarketId> = markets.iter().copied().map(Into::into).collect();
                self.vault_call_with("set_supply_queue", (markets,), None, Some(deposit)).await
            }

            #[instrument(skip(self, method_name))]
            pub async fn abdicate(&self, method_name: String) -> Result<(), ErrorWrapper> {
                self.vault_call("abdicate", (method_name,)).await
            }
        }
    };
}
