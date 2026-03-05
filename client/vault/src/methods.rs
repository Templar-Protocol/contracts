//! Macro for generating vault client methods to reduce duplication.
//!
//! This macro generates the common vault methods for any client that implements
//! the required helper methods.

/// Generate view cache management methods for a client.
///
/// The client must have these fields:
/// - `view_cache: RwLock<Option<ViewCache>>`
#[macro_export]
macro_rules! impl_view_cache_methods {
    ($client:ty) => {
        impl $client {
            pub fn enable_view_cache(
                &self,
                capacity: u32,
                ttl_seconds: u64,
            ) -> Result<(), $crate::ErrorWrapper> {
                use std::time::Duration;
                use $crate::lock_ext::RwLockExt;

                if capacity == 0 {
                    *self.view_cache.write_or_poison()? = None;
                    return Ok(());
                }

                let cache = $crate::ViewCache::builder()
                    .max_capacity(u64::from(capacity))
                    .time_to_live(Duration::from_secs(ttl_seconds))
                    .build();

                *self.view_cache.write_or_poison()? = Some(cache);
                Ok(())
            }

            pub fn disable_view_cache(&self) -> Result<(), $crate::ErrorWrapper> {
                use $crate::lock_ext::RwLockExt;
                *self.view_cache.write_or_poison()? = None;
                Ok(())
            }

            pub async fn clear_view_cache(&self) -> Result<(), $crate::ErrorWrapper> {
                use $crate::lock_ext::RwLockExt;
                let cache = { self.view_cache.read_or_poison()?.clone() };
                if let Some(cache) = cache {
                    cache.invalidate_all();
                }
                Ok(())
            }
        }
    };
}

/// Generate complex vault view methods for a client.
///
/// The client must have these helper methods:
/// - `view<T>(&self, account_id: &NearAccountId, method: &str, args: impl Serialize) -> Result<T>`
/// - `near_id(&self, id: &AccountId) -> Result<NearAccountId, ErrorWrapper>`
/// - `vault` field of type `NearAccountId`
#[macro_export]
macro_rules! impl_vault_view_methods {
    ($client:ty) => {
        #[uniffi::export(async_runtime = "tokio")]
        impl $client {
            #[instrument(skip(self))]
            pub async fn get_cap_groups(
                &self,
            ) -> Result<Vec<$crate::CapGroup>, $crate::ErrorWrapper> {
                let groups = self
                    .view::<Vec<(
                        templar_common::vault::CapGroupId,
                        templar_common::vault::CapGroupRecord,
                    )>>(&self.vault, "get_cap_groups", ())
                    .await
                    .map_err($crate::ErrorWrapper::from)?;

                Ok(groups
                    .into_iter()
                    .map(|(id, rec)| $crate::CapGroup {
                        id: id.into(),
                        cap: rec.cap.absolute_cap.map(|cap| cap.get().to_string()),
                        relative_cap: rec.cap.relative_cap.map(|cap| u128::from(cap).to_string()),
                        principal: rec.principal.to_string(),
                    })
                    .collect())
            }

            #[instrument(skip(self))]
            pub async fn get_pending_governance_actions(
                &self,
            ) -> Result<Vec<$crate::PendingGovernanceAction>, $crate::ErrorWrapper> {
                let pending = self
                    .view::<Vec<$crate::PendingValueSerde>>(
                        &self.vault,
                        "get_pending_governance_actions",
                        (),
                    )
                    .await
                    .map_err($crate::ErrorWrapper::from)?;

                Ok(pending
                    .into_iter()
                    .map(|p| $crate::PendingGovernanceAction {
                        action: p.value.into(),
                        valid_at_ns: p.valid_at_ns,
                    })
                    .collect())
            }

            #[instrument(skip(self, market))]
            pub async fn get_market_id_of_account(
                &self,
                market: &$crate::AccountId,
            ) -> Result<Option<$crate::MarketId>, $crate::ErrorWrapper> {
                let res = self
                    .view::<Option<U64>>(
                        &self.vault,
                        "get_market_id_of_account",
                        (self.near_id(market)?,),
                    )
                    .await
                    .map_err($crate::ErrorWrapper::from)?;

                let Some(u) = res else {
                    return Ok(None);
                };

                let id = templar_common::vault::MarketId::try_from_u64(u.0).ok_or_else(|| {
                    $crate::ErrorWrapper::Wrapped("market id out of u32 range".to_string())
                })?;

                Ok(Some(id.into()))
            }

            #[instrument(skip(self, market_id))]
            pub async fn get_market_account_by_id(
                &self,
                market_id: $crate::MarketId,
            ) -> Result<Option<$crate::AccountId>, $crate::ErrorWrapper> {
                use near_account_id::AccountId as NearAccountId;

                let res = self
                    .view::<Option<NearAccountId>>(
                        &self.vault,
                        "get_market_account_by_id",
                        (U64::from(
                            templar_common::vault::MarketId::from(market_id).as_u64(),
                        ),),
                    )
                    .await
                    .map_err($crate::ErrorWrapper::from)?;

                Ok(res.map(|a| $crate::AccountId::from(a.to_string())))
            }

            #[instrument(skip(self))]
            pub async fn list_markets_with_ids(
                &self,
            ) -> Result<Vec<$crate::MarketWithId>, $crate::ErrorWrapper> {
                use near_account_id::AccountId as NearAccountId;

                let res = self
                    .view::<Vec<(U64, NearAccountId)>>(&self.vault, "list_markets_with_ids", ())
                    .await
                    .map_err($crate::ErrorWrapper::from)?;

                let mapped = res
                    .into_iter()
                    .map(|(id, account)| {
                        let market_id =
                            templar_common::vault::MarketId::try_from_u64(id.0).ok_or_else(|| {
                            $crate::ErrorWrapper::Wrapped("market id out of u32 range".to_string())
                            })?;
                        Ok($crate::MarketWithId {
                            market_id: market_id.into(),
                            account: $crate::AccountId::from(account.to_string()),
                        })
                    })
                    .collect::<Result<Vec<_>, $crate::ErrorWrapper>>()?;

                Ok(mapped)
            }

            #[instrument(skip(self))]
            pub async fn get_vault_snapshot(
                &self,
            ) -> Result<$crate::VaultSnapshot, $crate::ErrorWrapper> {
                let (
                    configuration,
                    total_assets,
                    last_total_assets,
                    idle_balance,
                    total_supply,
                    max_deposit,
                    max_single_market_deposit,
                    fee_anchor,
                    fees,
                    restrictions,
                    cap_groups,
                    pending_governance_actions,
                    withdrawing_op_id,
                    has_pending_market_withdrawal,
                    current_withdraw_request_id,
                    queue_tail,
                    next_pending_withdrawal_id,
                    markets_with_ids,
                ) = tokio::try_join!(
                    self.get_configuration(),
                    self.get_total_assets(),
                    self.get_last_total_assets(),
                    self.get_idle_balance(),
                    self.get_total_supply(),
                    self.get_max_deposit(),
                    self.get_max_single_market_deposit(),
                    self.get_fee_anchor(),
                    self.get_fees(),
                    self.get_restrictions(),
                    self.get_cap_groups(),
                    self.get_pending_governance_actions(),
                    self.get_withdrawing_op_id(),
                    self.has_pending_market_withdrawal(),
                    self.get_current_withdraw_request_id(),
                    self.queue_tail(),
                    self.peek_next_pending_withdrawal_id(),
                    self.list_markets_with_ids(),
                )?;

                Ok($crate::VaultSnapshot {
                    configuration,
                    total_assets,
                    last_total_assets,
                    idle_balance,
                    total_supply,
                    max_deposit,
                    max_single_market_deposit,
                    fee_anchor,
                    fees,
                    restrictions,
                    cap_groups,
                    pending_governance_actions,
                    withdrawing_op_id,
                    has_pending_market_withdrawal,
                    current_withdraw_request_id,
                    queue_tail,
                    next_pending_withdrawal_id,
                    markets_with_ids,
                })
            }

            #[instrument(skip(self, markets))]
            pub async fn resolve_market_ids(
                &self,
                markets: &[$crate::AccountId],
            ) -> Result<Vec<Option<$crate::MarketId>>, $crate::ErrorWrapper> {
                futures::future::try_join_all(
                    markets
                        .iter()
                        .map(|market| self.get_market_id_of_account(market)),
                )
                .await
            }

            #[instrument(skip(self, market_ids))]
            pub async fn resolve_market_accounts(
                &self,
                market_ids: &[$crate::MarketId],
            ) -> Result<Vec<Option<$crate::AccountId>>, $crate::ErrorWrapper> {
                futures::future::try_join_all(
                    market_ids
                        .iter()
                        .map(|market_id| self.get_market_account_by_id(*market_id)),
                )
                .await
            }
        }
    };
}

/// Generate read-only vault methods for a client.
///
/// The client must have these helper methods:
/// - `vault_view_u128(&self, method: &str, args: impl Serialize) -> Result<ForeignU128, ErrorWrapper>`
/// - `view<T>(&self, account_id: &NearAccountId, method: &str, args: impl Serialize) -> Result<T>`
/// - `near_id(&self, id: &AccountId) -> Result<NearAccountId, ErrorWrapper>`
/// - `vault` field of type `NearAccountId`
#[macro_export]
macro_rules! impl_vault_read_methods {
    ($client:ty) => {
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
                self.vault_view_u128("get_max_single_market_deposit", ())
                    .await
            }
        }

        #[uniffi::export(async_runtime = "tokio")]
        impl $client {
            #[instrument(skip(self, assets))]
            pub async fn convert_to_shares(
                &self,
                assets: &ForeignU128,
            ) -> Result<ForeignU128, ErrorWrapper> {
                let assets = U128($crate::parse_u128(assets)?);
                self.vault_view_u128("convert_to_shares", (assets,)).await
            }

            #[instrument(skip(self, shares))]
            pub async fn convert_to_assets(
                &self,
                shares: &ForeignU128,
            ) -> Result<ForeignU128, ErrorWrapper> {
                let shares = U128($crate::parse_u128(shares)?);
                self.vault_view_u128("convert_to_assets", (shares,)).await
            }

            #[instrument(skip(self, assets))]
            pub async fn preview_deposit(
                &self,
                assets: &ForeignU128,
            ) -> Result<ForeignU128, ErrorWrapper> {
                let assets = U128($crate::parse_u128(assets)?);
                self.vault_view_u128("preview_deposit", (assets,)).await
            }

            #[instrument(skip(self, shares))]
            pub async fn preview_mint(
                &self,
                shares: &ForeignU128,
            ) -> Result<ForeignU128, ErrorWrapper> {
                let shares = U128($crate::parse_u128(shares)?);
                self.vault_view_u128("preview_mint", (shares,)).await
            }

            #[instrument(skip(self, assets))]
            pub async fn preview_withdraw(
                &self,
                assets: &ForeignU128,
            ) -> Result<ForeignU128, ErrorWrapper> {
                let assets = U128($crate::parse_u128(assets)?);
                self.vault_view_u128("preview_withdraw", (assets,)).await
            }

            #[instrument(skip(self, shares))]
            pub async fn preview_redeem(
                &self,
                shares: &ForeignU128,
            ) -> Result<ForeignU128, ErrorWrapper> {
                let shares = U128($crate::parse_u128(shares)?);
                self.vault_view_u128("preview_redeem", (shares,)).await
            }
        }

        #[uniffi::export(async_runtime = "tokio")]
        impl $client {
            #[instrument(skip(self))]
            pub async fn get_configuration(&self) -> Result<VaultConfiguration, ErrorWrapper> {
                let cfg = self
                    .view::<templar_common::vault::VaultConfiguration>(
                        &self.vault,
                        "get_configuration",
                        (),
                    )
                    .await
                    .map_err(ErrorWrapper::from)?;
                Ok(cfg.into())
            }

            #[instrument(skip(self))]
            pub async fn get_fee_anchor(&self) -> Result<FeeAccrualAnchor, ErrorWrapper> {
                let anchor = self
                    .view::<templar_common::vault::FeeAccrualAnchor>(
                        &self.vault,
                        "get_fee_anchor",
                        (),
                    )
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
                    .view::<Option<templar_common::vault::Restrictions>>(
                        &self.vault,
                        "get_restrictions",
                        (),
                    )
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
            pub async fn get_current_withdraw_request_id(
                &self,
            ) -> Result<Option<u64>, ErrorWrapper> {
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
            pub async fn peek_next_pending_withdrawal_id(
                &self,
            ) -> Result<Option<u64>, ErrorWrapper> {
                self.view(&self.vault, "peek_next_pending_withdrawal_id", ())
                    .await
                    .map_err(ErrorWrapper::from)
            }

            #[instrument(skip(self))]
            pub async fn build_real_assets_report(&self) -> Result<RealAssetsReport, ErrorWrapper> {
                let res = self
                    .view::<templar_common::vault::RealAssetsReport>(
                        &self.vault,
                        "build_real_assets_report",
                        (),
                    )
                    .await
                    .map_err(ErrorWrapper::from)?;
                Ok(res.into())
            }
        }
    };
}

/// Generate full vault methods (read-only + mutating call methods) for a client.
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
        $crate::impl_vault_read_methods!($client);

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
                self.vault_call_with(
                    "redeem",
                    (shares, self.near_id(receiver)?),
                    None,
                    Some(deposit),
                )
                .await
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
                self.vault_call_with(
                    "withdraw",
                    (assets, self.near_id(receiver)?),
                    None,
                    Some(deposit),
                )
                .await
            }

            #[instrument(skip(self, delta))]
            pub async fn reallocate(&self, delta: &AllocationDelta) -> Result<(), ErrorWrapper> {
                let delta = templar_common::vault::AllocationDelta::try_from(delta.clone())?;
                self.vault_call("reallocate", (delta,)).await
            }

            #[instrument(skip(self, route))]
            pub async fn execute_withdrawal(&self, route: &[MarketId]) -> Result<(), ErrorWrapper> {
                let route: Vec<templar_common::vault::MarketId> =
                    route.iter().copied().map(Into::into).collect();
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
                    (
                        U64::from(op_id),
                        templar_common::vault::MarketId::from(market),
                        batch_limit,
                    ),
                )
                .await
            }

            #[instrument(skip(self, market_id, batch_limit))]
            pub async fn execute_rebalance_withdrawal(
                &self,
                market_id: MarketId,
                batch_limit: Option<u32>,
            ) -> Result<(), ErrorWrapper> {
                self.vault_call(
                    "execute_rebalance_withdrawal",
                    (
                        templar_common::vault::MarketId::from(market_id),
                        batch_limit,
                    ),
                )
                .await
            }

            #[instrument(skip(self))]
            pub async fn refresh_idle_balance(&self) -> Result<ResyncIdleReport, ErrorWrapper> {
                const RESYNC_IDLE_GAS: Gas = 30_000_000_000_000;
                let report: templar_common::vault::ResyncIdleReport = self
                    .vault_call_returning("resync_idle_balance", (), Some(RESYNC_IDLE_GAS), None)
                    .await?;
                Ok(report.into())
            }

            #[instrument(skip(self, token))]
            pub async fn skim(&self, token: &AccountId) -> Result<(), ErrorWrapper> {
                self.vault_call("skim", (self.near_id(token)?,)).await
            }

            #[instrument(skip(self, markets))]
            pub async fn refresh_markets(
                &self,
                markets: &[MarketId],
            ) -> Result<RealAssetsReport, ErrorWrapper> {
                let markets: Vec<templar_common::vault::MarketId> =
                    markets.iter().copied().map(Into::into).collect();
                let report: templar_common::vault::RealAssetsReport = self
                    .vault_call_returning("refresh_markets", (markets,), None, None)
                    .await?;
                Ok(report.into())
            }

            #[instrument(skip(self, account))]
            pub async fn set_curator(&self, account: &AccountId) -> Result<(), ErrorWrapper> {
                self.vault_call("set_curator", (self.near_id(account)?,))
                    .await
            }

            #[instrument(skip(self, account))]
            pub async fn set_is_allocator(
                &self,
                account: &AccountId,
                allowed: bool,
            ) -> Result<(), ErrorWrapper> {
                self.vault_call("set_is_allocator", (self.near_id(account)?, allowed))
                    .await
            }

            #[instrument(skip(self, new_g))]
            pub async fn submit_guardian(&self, new_g: &AccountId) -> Result<(), ErrorWrapper> {
                self.vault_call("submit_guardian", (self.near_id(new_g)?,))
                    .await
            }

            #[instrument(skip(self, new_s))]
            pub async fn submit_sentinel(&self, new_s: &AccountId) -> Result<(), ErrorWrapper> {
                self.vault_call("submit_sentinel", (self.near_id(new_s)?,))
                    .await
            }

            #[instrument(skip(self, account))]
            pub async fn set_skim_recipient(
                &self,
                account: &AccountId,
            ) -> Result<(), ErrorWrapper> {
                self.vault_call("set_skim_recipient", (self.near_id(account)?,))
                    .await
            }

            #[instrument(skip(self, fees))]
            pub async fn set_fees(&self, fees: Fees) -> Result<(), ErrorWrapper> {
                let fees: templar_common::vault::Fees<U128> = fees.try_into()?;
                self.vault_call("set_fees", (fees,)).await
            }

            #[instrument(skip(self, new_timelock_ns, kind))]
            pub async fn submit_timelock(
                &self,
                new_timelock_ns: u64,
                kind: Option<TimelockKind>,
            ) -> Result<(), ErrorWrapper> {
                self.vault_call("submit_timelock", (U64::from(new_timelock_ns), kind))
                    .await
            }

            #[instrument(skip(self, market, new_cap))]
            pub async fn submit_cap(
                &self,
                market: &AccountId,
                new_cap: &ForeignU128,
            ) -> Result<(), ErrorWrapper> {
                let new_cap = U128($crate::parse_u128(new_cap)?);
                self.vault_call("submit_cap", (self.near_id(market)?, new_cap))
                    .await
            }

            #[instrument(skip(self, market))]
            pub async fn accept_cap(&self, market: &AccountId) -> Result<(), ErrorWrapper> {
                self.vault_call("accept_cap", (self.near_id(market)?,))
                    .await
            }

            #[instrument(skip(self, market))]
            pub async fn revoke_pending_cap(&self, market: &AccountId) -> Result<(), ErrorWrapper> {
                self.vault_call("revoke_pending_cap", (self.near_id(market)?,))
                    .await
            }

            #[instrument(skip(self, update))]
            pub async fn submit_cap_group_update(
                &self,
                update: CapGroupUpdate,
            ) -> Result<(), ErrorWrapper> {
                let update: templar_common::vault::CapGroupUpdate = update.try_into()?;
                self.vault_call("submit_cap_group_update", (update,)).await
            }

            #[instrument(skip(self, update))]
            pub async fn accept_cap_group_update(
                &self,
                update: CapGroupUpdateKey,
            ) -> Result<(), ErrorWrapper> {
                let key: templar_common::vault::CapGroupUpdateKey = update.into();
                self.vault_call("accept_cap_group_update", (key,)).await
            }

            #[instrument(skip(self, update))]
            pub async fn revoke_pending_cap_group_update(
                &self,
                update: CapGroupUpdateKey,
            ) -> Result<(), ErrorWrapper> {
                let key: templar_common::vault::CapGroupUpdateKey = update.into();
                self.vault_call("revoke_pending_cap_group_update", (key,))
                    .await
            }

            #[instrument(skip(self, restrictions))]
            pub async fn set_restrictions(
                &self,
                restrictions: Option<Restrictions>,
            ) -> Result<(), ErrorWrapper> {
                let r: Option<templar_common::vault::Restrictions> =
                    restrictions.map(TryInto::try_into).transpose()?;
                self.vault_call("set_restrictions", (r,)).await
            }

            #[instrument(skip(self, market))]
            pub async fn submit_market_removal(
                &self,
                market: &AccountId,
            ) -> Result<(), ErrorWrapper> {
                self.vault_call("submit_market_removal", (self.near_id(market)?,))
                    .await
            }

            #[instrument(skip(self, market))]
            pub async fn accept_market_removal(
                &self,
                market: &AccountId,
            ) -> Result<(), ErrorWrapper> {
                self.vault_call("accept_market_removal", (self.near_id(market)?,))
                    .await
            }

            #[instrument(skip(self, market))]
            pub async fn revoke_pending_market_removal(
                &self,
                market: &AccountId,
            ) -> Result<(), ErrorWrapper> {
                self.vault_call("revoke_pending_market_removal", (self.near_id(market)?,))
                    .await
            }

            #[instrument(skip(self, markets, deposit_yocto))]
            pub async fn set_supply_queue(
                &self,
                markets: &[MarketId],
                deposit_yocto: &ForeignU128,
            ) -> Result<(), ErrorWrapper> {
                let deposit = $crate::parse_u128(deposit_yocto)?;
                let markets: Vec<templar_common::vault::MarketId> =
                    markets.iter().copied().map(Into::into).collect();
                self.vault_call_with("set_supply_queue", (markets,), None, Some(deposit))
                    .await
            }

            #[instrument(skip(self, method_name))]
            pub async fn abdicate(&self, method_name: String) -> Result<(), ErrorWrapper> {
                self.vault_call("abdicate", (method_name,)).await
            }
        }
    };
}
