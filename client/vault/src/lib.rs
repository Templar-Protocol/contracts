use std::{
    collections::BTreeSet,
    fmt::Display,
    str::FromStr,
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use near_account_id::AccountId as NearAccountId;
use near_crypto::{InMemorySigner, SecretKey, Signer};
use near_jsonrpc_client::{
    methods::{
        query::RpcQueryRequest,
        send_tx::RpcSendTransactionRequest,
        tx::{RpcTransactionError, RpcTransactionStatusRequest, TransactionInfo},
    },
    JsonRpcClient,
};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::{
    action::{Action, FunctionCallAction},
    hash::CryptoHash,
    transaction::{SignedTransaction, Transaction, TransactionV0},
    types::{BlockReference, Gas},
    views::{FinalExecutionStatus, QueryRequest, TxExecutionStatus},
};
use near_sdk::json_types::{U128, U64};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tracing::{debug, instrument, warn};

uniffi::setup_scaffolding!();

type ForeignU128 = String;
type ForeignJson = String;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountId(String);

uniffi::custom_type!(AccountId, String);

impl From<AccountId> for near_account_id::AccountId {
    fn from(value: AccountId) -> Self {
        near_account_id::AccountId::from_str(&value.0).expect("Invalid AccountId")
    }
}

impl From<String> for AccountId {
    fn from(value: String) -> Self {
        AccountId(value)
    }
}

impl From<AccountId> for String {
    fn from(value: AccountId) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MarketId(pub u32);

uniffi::custom_type!(MarketId, u32);

impl From<u32> for MarketId {
    fn from(value: u32) -> Self {
        MarketId(value)
    }
}

impl From<MarketId> for u32 {
    fn from(value: MarketId) -> Self {
        value.0
    }
}

impl From<templar_common::vault::MarketId> for MarketId {
    fn from(value: templar_common::vault::MarketId) -> Self {
        MarketId(value.0)
    }
}

impl From<MarketId> for templar_common::vault::MarketId {
    fn from(value: MarketId) -> Self {
        templar_common::vault::MarketId(value.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapGroupId(pub String);

uniffi::custom_type!(CapGroupId, String);

impl From<String> for CapGroupId {
    fn from(value: String) -> Self {
        CapGroupId(value)
    }
}

impl From<CapGroupId> for String {
    fn from(value: CapGroupId) -> Self {
        value.0
    }
}

impl From<templar_common::vault::CapGroupId> for CapGroupId {
    fn from(value: templar_common::vault::CapGroupId) -> Self {
        CapGroupId(value.0)
    }
}

impl From<CapGroupId> for templar_common::vault::CapGroupId {
    fn from(value: CapGroupId) -> Self {
        templar_common::vault::CapGroupId(value.0)
    }
}

#[derive(uniffi::Enum)]
pub enum Event {
    Unsupported,
}

impl From<templar_common::vault::Event> for Event {
    fn from(_value: templar_common::vault::Event) -> Self {
        Event::Unsupported
    }
}

#[uniffi::export(callback_interface)]
pub trait EventHandler {
    fn handle(&self, event: Event);
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct Delta {
    pub market: MarketId,
    pub amount: ForeignU128,
}

impl From<templar_common::vault::Delta> for Delta {
    fn from(value: templar_common::vault::Delta) -> Self {
        Delta {
            market: value.market.into(),
            amount: value.amount.0.to_string(),
        }
    }
}

impl TryFrom<Delta> for templar_common::vault::Delta {
    type Error = ErrorWrapper;

    fn try_from(value: Delta) -> Result<Self, Self::Error> {
        Ok(templar_common::vault::Delta {
            market: value.market.into(),
            amount: U128(parse_u128(&value.amount)?),
        })
    }
}

#[derive(uniffi::Enum, Debug, Clone)]
pub enum AllocationDelta {
    Supply(Delta),
    Withdraw(Delta),
}

impl From<templar_common::vault::AllocationDelta> for AllocationDelta {
    fn from(value: templar_common::vault::AllocationDelta) -> Self {
        match value {
            templar_common::vault::AllocationDelta::Supply(delta) => {
                AllocationDelta::Supply(delta.into())
            }
            templar_common::vault::AllocationDelta::Withdraw(delta) => {
                AllocationDelta::Withdraw(delta.into())
            }
        }
    }
}

impl TryFrom<AllocationDelta> for templar_common::vault::AllocationDelta {
    type Error = ErrorWrapper;

    fn try_from(value: AllocationDelta) -> Result<Self, Self::Error> {
        Ok(match value {
            AllocationDelta::Supply(delta) => {
                templar_common::vault::AllocationDelta::Supply(delta.try_into()?)
            }
            AllocationDelta::Withdraw(delta) => {
                templar_common::vault::AllocationDelta::Withdraw(delta.try_into()?)
            }
        })
    }
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct Fee {
    pub fee: ForeignU128,
    pub recipient: AccountId,
}

impl From<templar_common::vault::Fee<U128>> for Fee {
    fn from(value: templar_common::vault::Fee<U128>) -> Self {
        Fee {
            fee: value.fee.0.to_string(),
            recipient: value.recipient.to_string().into(),
        }
    }
}

impl TryFrom<Fee> for templar_common::vault::Fee<U128> {
    type Error = ErrorWrapper;

    fn try_from(value: Fee) -> Result<Self, Self::Error> {
        Ok(templar_common::vault::Fee {
            fee: U128(parse_u128(&value.fee)?),
            recipient: NearAccountId::from(value.recipient),
        })
    }
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct Fees {
    pub performance: Fee,
    pub management: Fee,
    pub max_total_assets_growth_rate: Option<ForeignU128>,
}

impl From<templar_common::vault::Fees<U128>> for Fees {
    fn from(value: templar_common::vault::Fees<U128>) -> Self {
        Fees {
            performance: value.performance.into(),
            management: value.management.into(),
            max_total_assets_growth_rate: value
                .max_total_assets_growth_rate
                .map(|r| r.0.to_string()),
        }
    }
}

impl TryFrom<Fees> for templar_common::vault::Fees<U128> {
    type Error = ErrorWrapper;

    fn try_from(value: Fees) -> Result<Self, Self::Error> {
        Ok(templar_common::vault::Fees {
            performance: value.performance.try_into()?,
            management: value.management.try_into()?,
            max_total_assets_growth_rate: match value.max_total_assets_growth_rate {
                None => None,
                Some(r) => Some(U128(parse_u128(&r)?)),
            },
        })
    }
}

#[derive(uniffi::Enum, Debug, Clone)]
pub enum Restrictions {
    Paused,
    BlackList(Vec<AccountId>),
    WhiteList(Vec<AccountId>),
}

impl From<templar_common::vault::Restrictions> for Restrictions {
    fn from(value: templar_common::vault::Restrictions) -> Self {
        match value {
            templar_common::vault::Restrictions::Paused => Restrictions::Paused,
            templar_common::vault::Restrictions::BlackList(set) => {
                Restrictions::BlackList(set.iter().map(|a| a.to_string().into()).collect())
            }
            templar_common::vault::Restrictions::WhiteList(set) => {
                Restrictions::WhiteList(set.iter().map(|a| a.to_string().into()).collect())
            }
        }
    }
}

impl From<Restrictions> for templar_common::vault::Restrictions {
    fn from(value: Restrictions) -> Self {
        match value {
            Restrictions::Paused => templar_common::vault::Restrictions::Paused,
            Restrictions::BlackList(accounts) => {
                let set: BTreeSet<NearAccountId> =
                    accounts.into_iter().map(NearAccountId::from).collect();
                templar_common::vault::Restrictions::BlackList(set)
            }
            Restrictions::WhiteList(accounts) => {
                let set: BTreeSet<NearAccountId> =
                    accounts.into_iter().map(NearAccountId::from).collect();
                templar_common::vault::Restrictions::WhiteList(set)
            }
        }
    }
}

#[derive(uniffi::Enum, Debug, Clone)]
pub enum CapGroupUpdate {
    SetCap {
        cap_group: CapGroupId,
        new_cap: ForeignU128,
    },
    SetRelativeCap {
        cap_group: CapGroupId,
        new_relative_cap: ForeignU128,
    },
    SetMarketCapGroup {
        market: MarketId,
        cap_group: Option<CapGroupId>,
    },
}

impl TryFrom<CapGroupUpdate> for templar_common::vault::CapGroupUpdate {
    type Error = ErrorWrapper;

    fn try_from(value: CapGroupUpdate) -> Result<Self, Self::Error> {
        Ok(match value {
            CapGroupUpdate::SetCap { cap_group, new_cap } => {
                templar_common::vault::CapGroupUpdate::SetCap {
                    cap_group: cap_group.into(),
                    new_cap: U128(parse_u128(&new_cap)?),
                }
            }
            CapGroupUpdate::SetRelativeCap {
                cap_group,
                new_relative_cap,
            } => templar_common::vault::CapGroupUpdate::SetRelativeCap {
                cap_group: cap_group.into(),
                new_relative_cap: U128(parse_u128(&new_relative_cap)?),
            },
            CapGroupUpdate::SetMarketCapGroup { market, cap_group } => {
                templar_common::vault::CapGroupUpdate::SetMarketCapGroup {
                    market: market.into(),
                    cap_group: cap_group.map(Into::into),
                }
            }
        })
    }
}

#[derive(uniffi::Enum, Debug, Clone)]
pub enum CapGroupUpdateKey {
    SetCap { cap_group: CapGroupId },
    SetRelativeCap { cap_group: CapGroupId },
    SetMarketCapGroup { market: MarketId },
}

impl From<CapGroupUpdateKey> for templar_common::vault::CapGroupUpdateKey {
    fn from(value: CapGroupUpdateKey) -> Self {
        match value {
            CapGroupUpdateKey::SetCap { cap_group } => templar_common::vault::CapGroupUpdateKey::SetCap {
                cap_group: cap_group.into(),
            },
            CapGroupUpdateKey::SetRelativeCap { cap_group } => {
                templar_common::vault::CapGroupUpdateKey::SetRelativeCap {
                    cap_group: cap_group.into(),
                }
            }
            CapGroupUpdateKey::SetMarketCapGroup { market } => {
                templar_common::vault::CapGroupUpdateKey::SetMarketCapGroup {
                    market: market.into(),
                }
            }
        }
    }
}

#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "serde")]
pub enum TimelockKind {
    Guardian,
    Sentinel,
    Config,
    Cap,
    MarketRemoval,
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct FeeAccrualAnchor {
    pub total_assets: ForeignU128,
    pub timestamp_ns: u64,
}

impl From<templar_common::vault::FeeAccrualAnchor> for FeeAccrualAnchor {
    fn from(value: templar_common::vault::FeeAccrualAnchor) -> Self {
        FeeAccrualAnchor {
            total_assets: value.total_assets.0.to_string(),
            timestamp_ns: value.timestamp_ns.0,
        }
    }
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct MarketWithId {
    pub market_id: MarketId,
    pub account: AccountId,
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct MarketAssets {
    pub market_id: MarketId,
    pub assets: ForeignU128,
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct RealAssetsReport {
    pub total_assets: ForeignU128,
    pub per_market: Vec<MarketAssets>,
    pub refreshed_at_ns: u64,
}

impl From<templar_common::vault::RealAssetsReport> for RealAssetsReport {
    fn from(value: templar_common::vault::RealAssetsReport) -> Self {
        RealAssetsReport {
            total_assets: value.total_assets.0.to_string(),
            per_market: value
                .per_market
                .into_iter()
                .map(|(id, amt)| MarketAssets {
                    market_id: id.into(),
                    assets: amt.0.to_string(),
                })
                .collect(),
            refreshed_at_ns: value.refreshed_at.0,
        }
    }
}

pub const DEFAULT_GAS: Gas = 300_000_000_000_000;
pub const MAX_POLL_INTERVAL_MILLIS: u64 = 1000;

#[derive(uniffi::Object)]
pub struct Client {
    inner: JsonRpcClient,
    signer: Signer,
    pub vault: NearAccountId,
    timeout: u64,
}

#[uniffi::export]
impl Client {
    #[uniffi::constructor]
    #[instrument(skip(signer_key, signer_account, vault), fields(rpc_url = %rpc_url, timeout))]
    pub fn new_client(
        rpc_url: String,
        signer_account: &AccountId,
        signer_key: &str,
        vault: &AccountId,
        timeout: u64,
    ) -> Result<Self, ErrorWrapper> {
        let inner = JsonRpcClient::connect(rpc_url);

        let signer = InMemorySigner::from_secret_key(
            NearAccountId::from(signer_account.clone()),
            SecretKey::from_str(signer_key).map_err(ErrorWrapper::from)?,
        );

        let vault: NearAccountId = NearAccountId::from(vault.clone());

        Ok(Self {
            inner,
            signer,
            vault,
            timeout,
        })
    }

    #[instrument(skip(self))]
    pub async fn get_configuration(&self) -> Result<ForeignJson, ErrorWrapper> {
        let cfg = self
            .view::<templar_common::vault::VaultConfiguration>(
                &self.vault,
                "get_configuration",
                (),
                self.timeout,
            )
            .await
            .map_err(ErrorWrapper::from)?;
        serde_json::to_string(&cfg).map_err(ErrorWrapper::from)
    }

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

    #[instrument(skip(self))]
    pub async fn get_fee_anchor(&self) -> Result<FeeAccrualAnchor, ErrorWrapper> {
        let anchor = self
            .view::<templar_common::vault::FeeAccrualAnchor>(
                &self.vault,
                "get_fee_anchor",
                (),
                self.timeout,
            )
            .await
            .map_err(ErrorWrapper::from)?;
        Ok(anchor.into())
    }

    #[instrument(skip(self))]
    pub async fn get_fees(&self) -> Result<Fees, ErrorWrapper> {
        let fees = self
            .view::<templar_common::vault::Fees<U128>>(&self.vault, "get_fees", (), self.timeout)
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
                self.timeout,
            )
            .await
            .map_err(ErrorWrapper::from)?;
        Ok(r.map(Into::into))
    }

    #[instrument(skip(self))]
    pub async fn get_cap_groups_json(&self) -> Result<ForeignJson, ErrorWrapper> {
        let v = self
            .view::<serde_json::Value>(&self.vault, "get_cap_groups", (), self.timeout)
            .await
            .map_err(ErrorWrapper::from)?;
        Ok(v.to_string())
    }

    #[instrument(skip(self))]
    pub async fn get_pending_governance_actions_json(&self) -> Result<ForeignJson, ErrorWrapper> {
        let v = self
            .view::<serde_json::Value>(
                &self.vault,
                "get_pending_governance_actions",
                (),
                self.timeout,
            )
            .await
            .map_err(ErrorWrapper::from)?;
        Ok(v.to_string())
    }

    #[instrument(skip(self, assets))]
    pub async fn convert_to_shares(&self, assets: &ForeignU128) -> Result<ForeignU128, ErrorWrapper> {
        let assets = U128(parse_u128(assets)?);
        self.vault_view_u128("convert_to_shares", (assets,)).await
    }

    #[instrument(skip(self, shares))]
    pub async fn convert_to_assets(&self, shares: &ForeignU128) -> Result<ForeignU128, ErrorWrapper> {
        let shares = U128(parse_u128(shares)?);
        self.vault_view_u128("convert_to_assets", (shares,)).await
    }

    #[instrument(skip(self, assets))]
    pub async fn preview_deposit(&self, assets: &ForeignU128) -> Result<ForeignU128, ErrorWrapper> {
        let assets = U128(parse_u128(assets)?);
        self.vault_view_u128("preview_deposit", (assets,)).await
    }

    #[instrument(skip(self, shares))]
    pub async fn preview_mint(&self, shares: &ForeignU128) -> Result<ForeignU128, ErrorWrapper> {
        let shares = U128(parse_u128(shares)?);
        self.vault_view_u128("preview_mint", (shares,)).await
    }

    #[instrument(skip(self, assets))]
    pub async fn preview_withdraw(&self, assets: &ForeignU128) -> Result<ForeignU128, ErrorWrapper> {
        let assets = U128(parse_u128(assets)?);
        self.vault_view_u128("preview_withdraw", (assets,)).await
    }

    #[instrument(skip(self, shares))]
    pub async fn preview_redeem(&self, shares: &ForeignU128) -> Result<ForeignU128, ErrorWrapper> {
        let shares = U128(parse_u128(shares)?);
        self.vault_view_u128("preview_redeem", (shares,)).await
    }

    #[instrument(skip(self))]
    pub async fn get_withdrawing_op_id(&self) -> Result<Option<u64>, ErrorWrapper> {
        let res = self
            .view::<Option<U64>>(
                &self.vault,
                "get_withdrawing_op_id",
                (),
                self.timeout,
            )
            .await
            .map_err(ErrorWrapper::from)?;
        Ok(res.map(|u| u.0))
    }

    #[instrument(skip(self))]
    pub async fn has_pending_market_withdrawal(&self) -> Result<bool, ErrorWrapper> {
        self.view(&self.vault, "has_pending_market_withdrawal", (), self.timeout)
            .await
            .map_err(ErrorWrapper::from)
    }

    #[instrument(skip(self))]
    pub async fn get_current_withdraw_request_id(&self) -> Result<Option<u64>, ErrorWrapper> {
        let res = self
            .view::<Option<U64>>(
                &self.vault,
                "get_current_withdraw_request_id",
                (),
                self.timeout,
            )
            .await
            .map_err(ErrorWrapper::from)?;
        Ok(res.map(|u| u.0))
    }

    #[instrument(skip(self))]
    pub async fn queue_tail(&self) -> Result<u64, ErrorWrapper> {
        self.view(&self.vault, "queue_tail", (), self.timeout)
            .await
            .map_err(ErrorWrapper::from)
    }

    #[instrument(skip(self))]
    pub async fn peek_next_pending_withdrawal_id(&self) -> Result<Option<u64>, ErrorWrapper> {
        self.view(&self.vault, "peek_next_pending_withdrawal_id", (), self.timeout)
            .await
            .map_err(ErrorWrapper::from)
    }

    #[instrument(skip(self, market))]
    pub async fn get_market_id_of_account(
        &self,
        market: &AccountId,
    ) -> Result<Option<MarketId>, ErrorWrapper> {
        let res = self
            .view::<Option<U64>>(
                &self.vault,
                "get_market_id_of_account",
                (self.near_id(market),),
                self.timeout,
            )
            .await
            .map_err(ErrorWrapper::from)?;

        let Some(u) = res else {
            return Ok(None);
        };

        let id_u32: u32 = u
            .0
            .try_into()
            .map_err(|_| ErrorWrapper::Wrapped("market id out of u32 range".to_string()))?;

        Ok(Some(MarketId(id_u32)))
    }

    #[instrument(skip(self, market_id))]
    pub async fn get_market_account_by_id(
        &self,
        market_id: MarketId,
    ) -> Result<Option<AccountId>, ErrorWrapper> {
        let res = self
            .view::<Option<NearAccountId>>(
                &self.vault,
                "get_market_account_by_id",
                (U64::from(market_id.0 as u64),),
                self.timeout,
            )
            .await
            .map_err(ErrorWrapper::from)?;

        Ok(res.map(|a| a.to_string().into()))
    }

    #[instrument(skip(self))]
    pub async fn list_markets_with_ids(&self) -> Result<Vec<MarketWithId>, ErrorWrapper> {
        let res = self
            .view::<Vec<(U64, NearAccountId)>>(
                &self.vault,
                "list_markets_with_ids",
                (),
                self.timeout,
            )
            .await
            .map_err(ErrorWrapper::from)?;

        let mapped = res
            .into_iter()
            .map(|(id, account)| {
                let id_u32: u32 = id
                    .0
                    .try_into()
                    .map_err(|_| ErrorWrapper::Wrapped("market id out of u32 range".to_string()))?;
                Ok(MarketWithId {
                    market_id: MarketId(id_u32),
                    account: account.to_string().into(),
                })
            })
            .collect::<Result<Vec<_>, ErrorWrapper>>()?;

        Ok(mapped)
    }

    #[instrument(skip(self))]
    pub async fn build_real_assets_report(&self) -> Result<RealAssetsReport, ErrorWrapper> {
        let res = self
            .view::<templar_common::vault::RealAssetsReport>(
                &self.vault,
                "build_real_assets_report",
                (),
                self.timeout,
            )
            .await
            .map_err(ErrorWrapper::from)?;
        Ok(res.into())
    }

    #[instrument(skip(self, shares, receiver, deposit_yocto))]
    pub async fn redeem(
        &self,
        shares: &ForeignU128,
        receiver: &AccountId,
        deposit_yocto: &ForeignU128,
    ) -> Result<(), ErrorWrapper> {
        let shares = U128(parse_u128(shares)?);
        let deposit = parse_u128(deposit_yocto)?;
        self.vault_call_with(
            "redeem",
            (shares, self.near_id(receiver)),
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
        let assets = U128(parse_u128(assets)?);
        let deposit = parse_u128(deposit_yocto)?;
        self.vault_call_with(
            "withdraw",
            (assets, self.near_id(receiver)),
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
            (templar_common::vault::MarketId::from(market_id), batch_limit),
        )
        .await
    }

    #[instrument(skip(self))]
    pub async fn unbrick(&self) -> Result<(), ErrorWrapper> {
        self.vault_call("unbrick", ()).await
    }

    #[instrument(skip(self, token))]
    pub async fn skim(&self, token: &AccountId) -> Result<(), ErrorWrapper> {
        self.vault_call("skim", (self.near_id(token),)).await
    }

    #[instrument(skip(self, markets))]
    pub async fn refresh_markets(
        &self,
        markets: &[MarketId],
    ) -> Result<RealAssetsReport, ErrorWrapper> {
        let markets: Vec<templar_common::vault::MarketId> =
            markets.iter().copied().map(Into::into).collect();
        self.vault_call("refresh_markets", (markets,)).await?;
        self.build_real_assets_report().await
    }

    #[instrument(skip(self, account))]
    pub async fn set_curator(&self, account: &AccountId) -> Result<(), ErrorWrapper> {
        self.vault_call("set_curator", (self.near_id(account),)).await
    }

    #[instrument(skip(self, account))]
    pub async fn set_is_allocator(
        &self,
        account: &AccountId,
        allowed: bool,
    ) -> Result<(), ErrorWrapper> {
        self.vault_call("set_is_allocator", (self.near_id(account), allowed))
            .await
    }

    #[instrument(skip(self, new_g))]
    pub async fn submit_guardian(&self, new_g: &AccountId) -> Result<(), ErrorWrapper> {
        self.vault_call("submit_guardian", (self.near_id(new_g),))
            .await
    }

    #[instrument(skip(self))]
    pub async fn accept_guardian(&self) -> Result<(), ErrorWrapper> {
        self.vault_call("accept_guardian", ()).await
    }

    #[instrument(skip(self))]
    pub async fn revoke_pending_guardian(&self) -> Result<(), ErrorWrapper> {
        self.vault_call("revoke_pending_guardian", ()).await
    }

    #[instrument(skip(self, new_s))]
    pub async fn submit_sentinel(&self, new_s: &AccountId) -> Result<(), ErrorWrapper> {
        self.vault_call("submit_sentinel", (self.near_id(new_s),))
            .await
    }

    #[instrument(skip(self))]
    pub async fn accept_sentinel(&self) -> Result<(), ErrorWrapper> {
        self.vault_call("accept_sentinel", ()).await
    }

    #[instrument(skip(self))]
    pub async fn revoke_pending_sentinel(&self) -> Result<(), ErrorWrapper> {
        self.vault_call("revoke_pending_sentinel", ()).await
    }

    #[instrument(skip(self, account))]
    pub async fn set_skim_recipient(&self, account: &AccountId) -> Result<(), ErrorWrapper> {
        self.vault_call("set_skim_recipient", (self.near_id(account),))
            .await
    }

    #[instrument(skip(self, fees))]
    pub async fn set_fees(&self, fees: Fees) -> Result<(), ErrorWrapper> {
        let fees: templar_common::vault::Fees<U128> = fees.try_into()?;
        self.vault_call("set_fees", (fees,)).await
    }

    #[instrument(skip(self))]
    pub async fn accept_fees(&self) -> Result<(), ErrorWrapper> {
        self.vault_call("accept_fees", ()).await
    }

    #[instrument(skip(self))]
    pub async fn revoke_pending_fees(&self) -> Result<(), ErrorWrapper> {
        self.vault_call("revoke_pending_fees", ()).await
    }

    #[instrument(skip(self, new_timelock_ns, kind))]
    pub async fn submit_timelock(
        &self,
        new_timelock_ns: u64,
        kind: Option<TimelockKind>,
    ) -> Result<(), ErrorWrapper> {
        self.vault_call(
            "submit_timelock",
            (U64::from(new_timelock_ns), kind),
        )
        .await
    }

    #[instrument(skip(self))]
    pub async fn accept_timelock(&self) -> Result<(), ErrorWrapper> {
        self.vault_call("accept_timelock", ()).await
    }

    #[instrument(skip(self))]
    pub async fn revoke_pending_timelock(&self) -> Result<(), ErrorWrapper> {
        self.vault_call("revoke_pending_timelock", ()).await
    }

    #[instrument(skip(self, market, new_cap))]
    pub async fn submit_cap(
        &self,
        market: &AccountId,
        new_cap: &ForeignU128,
    ) -> Result<(), ErrorWrapper> {
        let new_cap = U128(parse_u128(new_cap)?);
        self.vault_call("submit_cap", (self.near_id(market), new_cap))
            .await
    }

    #[instrument(skip(self, market))]
    pub async fn accept_cap(&self, market: &AccountId) -> Result<(), ErrorWrapper> {
        self.vault_call("accept_cap", (self.near_id(market),)).await
    }

    #[instrument(skip(self, market))]
    pub async fn revoke_pending_cap(&self, market: &AccountId) -> Result<(), ErrorWrapper> {
        self.vault_call("revoke_pending_cap", (self.near_id(market),))
            .await
    }

    #[instrument(skip(self, update))]
    pub async fn submit_cap_group_update(&self, update: CapGroupUpdate) -> Result<(), ErrorWrapper> {
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
        self.vault_call("revoke_pending_cap_group_update", (key,)).await
    }

    #[instrument(skip(self, restrictions))]
    pub async fn set_restrictions(
        &self,
        restrictions: Option<Restrictions>,
    ) -> Result<(), ErrorWrapper> {
        let r: Option<templar_common::vault::Restrictions> = restrictions.map(Into::into);
        self.vault_call("set_restrictions", (r,)).await
    }

    #[instrument(skip(self))]
    pub async fn accept_restrictions(&self) -> Result<(), ErrorWrapper> {
        self.vault_call("accept_restrictions", ()).await
    }

    #[instrument(skip(self))]
    pub async fn revoke_pending_restrictions(&self) -> Result<(), ErrorWrapper> {
        self.vault_call("revoke_pending_restrictions", ()).await
    }

    #[instrument(skip(self, market))]
    pub async fn submit_market_removal(&self, market: &AccountId) -> Result<(), ErrorWrapper> {
        self.vault_call("submit_market_removal", (self.near_id(market),))
            .await
    }

    #[instrument(skip(self, market))]
    pub async fn accept_market_removal(&self, market: &AccountId) -> Result<(), ErrorWrapper> {
        self.vault_call("accept_market_removal", (self.near_id(market),))
            .await
    }

    #[instrument(skip(self, market))]
    pub async fn revoke_pending_market_removal(
        &self,
        market: &AccountId,
    ) -> Result<(), ErrorWrapper> {
        self.vault_call(
            "revoke_pending_market_removal",
            (self.near_id(market),),
        )
        .await
    }

    #[instrument(skip(self, markets, deposit_yocto))]
    pub async fn set_supply_queue(
        &self,
        markets: &[MarketId],
        deposit_yocto: &ForeignU128,
    ) -> Result<(), ErrorWrapper> {
        let deposit = parse_u128(deposit_yocto)?;
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

impl Client {
    #[instrument(skip(signer), fields(vault = %vault, timeout))]
    pub fn new(inner: JsonRpcClient, signer: Signer, vault: NearAccountId, timeout: u64) -> Self {
        Self {
            inner,
            signer,
            vault,
            timeout,
        }
    }

    #[instrument(skip(self))]
    pub async fn get_access_key_data(&self) -> Result<(u64, CryptoHash)> {
        let access_key_query_response = self
            .inner
            .call(RpcQueryRequest {
                block_reference: BlockReference::latest(),
                request: QueryRequest::ViewAccessKey {
                    account_id: self.signer.get_account_id(),
                    public_key: self.signer.public_key().clone(),
                },
            })
            .await?;

        let nonce = match access_key_query_response.kind {
            QueryResponseKind::AccessKey(access_key) => access_key.nonce + 1,
            _ => {
                bail!(
                    "Expected AccessKey got {:?}",
                    access_key_query_response.kind
                );
            }
        };
        let block_hash = access_key_query_response.block_hash;

        Ok((nonce, block_hash))
    }

    #[instrument(skip(self, args), fields(account_id = %account_id, method = function_name, timeout))]
    pub async fn view<T: DeserializeOwned>(
        &self,
        account_id: &NearAccountId,
        function_name: &str,
        args: impl Serialize,
        timeout: u64,
    ) -> Result<T> {
        let response = tokio::time::timeout(
            Duration::from_secs(timeout),
            self.inner.call(RpcQueryRequest {
                block_reference: BlockReference::latest(),
                request: QueryRequest::CallFunction {
                    account_id: account_id.clone(),
                    method_name: function_name.to_owned(),
                    args: serde_json::to_vec(&args)?.into(),
                },
            }),
        )
        .await??;

        let QueryResponseKind::CallResult(result) = response.kind else {
            bail!("Expected CallResult got {:?}", response.kind);
        };

        let value = serde_json::from_slice(&result.result)?;
        Ok(value)
    }

    #[instrument(skip(self, args), fields(account_id = %account_id, method = function_name, gas = ?gas, deposit = ?deposit, timeout))]
    pub async fn call(
        &self,
        account_id: &NearAccountId,
        function_name: &str,
        args: impl Serialize,
        gas: Option<Gas>,
        deposit: Option<u128>,
        timeout: u64,
    ) -> Result<FinalExecutionStatus> {
        let (nonce, block_hash) = self.get_access_key_data().await?;

        let tx = Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: account_id.clone(),
            block_hash,
            signer_id: self.signer.get_account_id(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: function_name.to_string(),
                args: serde_json::to_vec(&args)?,
                gas: gas.unwrap_or(DEFAULT_GAS),
                deposit: deposit.unwrap_or(0),
            }))],
        });

        let (tx_hash, _size) = tx.get_hash_and_size();

        let called_at = Instant::now();
        let signature = self.signer.sign(tx_hash.as_ref());
        let deadline = called_at + Duration::from_secs(timeout);
        let result = match self
            .inner
            .call(RpcSendTransactionRequest {
                signed_transaction: SignedTransaction::new(signature, tx),
                wait_until: TxExecutionStatus::Final,
            })
            .await
        {
            Ok(res) => res,
            Err(e) => {
                warn!(
                    "Send transaction error: {:?}. Starting status polling until deadline.",
                    e
                );
                loop {
                    if !matches!(e.handler_error(), Some(RpcTransactionError::TimeoutError)) {
                        return Err(e.into());
                    }

                    let mut poll_interval = Duration::from_millis(500);

                    loop {
                        if Instant::now() >= deadline {
                            warn!("Transaction polling deadline exceeded, aborting");
                            bail!("Transaction timed out");
                        }

                        tokio::time::sleep(poll_interval).await;
                        debug!("Polling transaction status...");

                        poll_interval = std::cmp::min(
                            poll_interval * 2,
                            Duration::from_millis(MAX_POLL_INTERVAL_MILLIS),
                        );

                        let status = self
                            .inner
                            .call(RpcTransactionStatusRequest {
                                transaction_info: TransactionInfo::TransactionId {
                                    sender_account_id: self.signer.get_account_id(),
                                    tx_hash,
                                },
                                wait_until: TxExecutionStatus::Final,
                            })
                            .await;

                        let Err(e) = status else {
                            break;
                        };

                        if !matches!(e.handler_error(), Some(RpcTransactionError::TimeoutError)) {
                            warn!("Transaction status error: {:?}", e);
                            return Err(e.into());
                        }
                    }
                }
            }
        };

        let Some(outcome) = result.final_execution_outcome else {
            bail!("No outcome {}", tx_hash);
        };

        let status = outcome.into_outcome().status;
        if let FinalExecutionStatus::Failure(tx_err) = &status {
            bail!("Transaction failed: {:?}", tx_err);
        }
        Ok(status)
    }

    #[inline]
    fn near_id(&self, id: &AccountId) -> NearAccountId {
        NearAccountId::from(id.clone())
    }

    async fn vault_view_u128(
        &self,
        method: &str,
        args: impl Serialize,
    ) -> Result<ForeignU128, ErrorWrapper> {
        let u = self
            .view::<U128>(&self.vault, method, args, self.timeout)
            .await
            .map_err(ErrorWrapper::from)?;
        Ok(u.0.to_string())
    }

    async fn vault_call_with(
        &self,
        method: &str,
        args: impl Serialize,
        gas: Option<Gas>,
        deposit: Option<u128>,
    ) -> Result<(), ErrorWrapper> {
        self.call(&self.vault, method, args, gas, deposit, self.timeout)
            .await
            .map(|_| ())
            .map_err(ErrorWrapper::from)
    }

    async fn vault_call(&self, method: &str, args: impl Serialize) -> Result<(), ErrorWrapper> {
        self.vault_call_with(method, args, None, None).await
    }
}

fn parse_u128(s: &str) -> Result<u128, ErrorWrapper> {
    if let Ok(v) = s.parse::<u128>() {
        return Ok(v);
    }

    let inner: String = serde_json::from_str(s).map_err(ErrorWrapper::from)?;
    inner.parse::<u128>().map_err(ErrorWrapper::from)
}

#[derive(uniffi::Error, Debug)]
pub enum ErrorWrapper {
    Wrapped(String),
}

impl Display for ErrorWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorWrapper::Wrapped(err) => write!(f, "Error: {}", err),
        }
    }
}

impl<T: Into<anyhow::Error>> From<T> for ErrorWrapper {
    fn from(err: T) -> Self {
        ErrorWrapper::Wrapped(err.into().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_crypto::KeyType;
    use rstest::{fixture, rstest};

    #[fixture]
    fn vault() -> AccountId {
        let _ = tracing_subscriber::fmt::try_init();
        AccountId("metavault.topgunbakugo.testnet".to_string())
    }

    #[fixture]
    fn everything() -> AccountId {
        AccountId("topgunbakugo.testnet".to_string())
    }

    #[fixture]
    fn testnet_rpc() -> String {
        "https://rpc.testnet.fastnear.com/".to_string()
    }

    #[fixture]
    fn sk() -> SecretKey {
        SecretKey::from_random(KeyType::ED25519)
    }

    #[rstest]
    fn account_id_conversion_happy_path(everything: AccountId) {
        let near_id: NearAccountId = everything.clone().into();
        assert_eq!(near_id.as_str(), "topgunbakugo.testnet");
    }

    #[test]
    fn error_wrapper_display_happy_path() {
        let err = ErrorWrapper::from(anyhow::anyhow!("boom"));
        let s = format!("{}", err);
        assert!(s.contains("boom"));
    }

    #[test]
    fn default_gas_is_nonzero() {
        assert!(super::DEFAULT_GAS > 0);
    }

    #[rstest]
    fn can_construct_client_happy_path(
        vault: AccountId,
        everything: AccountId,
        testnet_rpc: String,
        sk: SecretKey,
    ) {
        Client::new_client(testnet_rpc, &everything, &sk.to_string(), &vault, 5)
            .expect("Client should be created");
    }

    #[test]
    fn parse_u128_accepts_plain_and_json_string() {
        assert_eq!(super::parse_u128("123").unwrap(), 123);
        assert_eq!(super::parse_u128("\"456\"").unwrap(), 456);
    }

    #[test]
    fn delta_roundtrip() {
        let d = Delta {
            market: MarketId(7),
            amount: "100".to_string(),
        };

        let common: templar_common::vault::Delta = d.clone().try_into().unwrap();
        assert_eq!(common.market.0, 7);
        assert_eq!(common.amount.0, 100);

        let back: Delta = common.into();
        assert_eq!(back.market.0, 7);
        assert_eq!(back.amount, "100");
    }

    #[rstest]
    #[tokio::test(flavor = "current_thread")]
    #[ignore]
    async fn view_methods_happy_path_smoke(vault: AccountId, testnet_rpc: String) {
        let sk = SecretKey::from_random(KeyType::ED25519);
        let signer_account = AccountId("alice.testnet".to_string());
        let client = Client::new_client(testnet_rpc, &signer_account, &sk.to_string(), &vault, 5)
            .expect("Client should be created");

        let _ = client.get_total_assets().await.unwrap();
        let _ = client.get_total_supply().await.unwrap();
        let _ = client.get_idle_balance().await.unwrap();
        let _ = client.get_max_deposit().await.unwrap();
    }

    #[test]
    fn no_json_string_api_regressions() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(rd) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in rd.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    out.push(path);
                }
            }
        }

        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        walk(&root.join("src"), &mut files);

        for file in files {
            let content = std::fs::read_to_string(&file).unwrap_or_default();

            let is_self = file.file_name().and_then(|n| n.to_str()) == Some("lib.rs")
                && file.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) == Some("src");

            let mut in_guard = false;
            let mut brace_depth: i32 = 0;

            for line in content.lines() {
                let l = line.trim();

                if is_self && !in_guard && l.contains("fn no_json_string_api_regressions") {
                    in_guard = true;
                    brace_depth = l.chars().filter(|&c| c == '{').count() as i32
                        - l.chars().filter(|&c| c == '}').count() as i32;
                    if brace_depth <= 0 {
                        brace_depth = 1;
                    }
                    continue;
                }

                if in_guard {
                    brace_depth += l.chars().filter(|&c| c == '{').count() as i32;
                    brace_depth -= l.chars().filter(|&c| c == '}').count() as i32;
                    if brace_depth == 0 {
                        in_guard = false;
                    }
                    continue;
                }

                if l.contains("ForeignJson") {
                    panic!("ForeignJson not allowed: {}: {}", file.display(), l);
                }

                if l.starts_with("pub ") && l.contains("fn ") && l.contains("_json") {
                    panic!("*_json API not allowed: {}: {}", file.display(), l);
                }
            }
        }
    }

    #[rstest]
    #[tokio::test(flavor = "current_thread")]
    #[ignore]
    async fn redeem_happy_path_smoke(
        vault: AccountId,
        everything: AccountId,
        testnet_rpc: String,
        sk: SecretKey,
    ) {
        let client =
            Client::new_client(testnet_rpc, &everything, &sk.to_string(), &vault, 5).unwrap();
        let receiver = AccountId("topgunbakugo.testnet".to_string());
        client
            .redeem(&"1".to_string(), &receiver, &"1".to_string())
            .await
            .unwrap();
    }
}
