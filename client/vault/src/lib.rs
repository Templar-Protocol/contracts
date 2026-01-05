use std::{
    collections::BTreeSet,
    fmt::Display,
    str::FromStr,
    sync::{Mutex, OnceLock, RwLock},
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use mini_moka::sync::Cache as MokaCache;
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

#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl RetryConfig {
    fn normalized(self) -> Self {
        let max_attempts = self.max_attempts.max(1);
        let initial_backoff_ms = self.initial_backoff_ms.max(1);
        let max_backoff_ms = self.max_backoff_ms.max(initial_backoff_ms);
        Self {
            max_attempts,
            initial_backoff_ms,
            max_backoff_ms,
        }
    }
}

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

#[derive(Default)]
struct FeeBuilderState {
    fee: Option<ForeignU128>,
    recipient: Option<AccountId>,
}

#[derive(uniffi::Object, Default)]
pub struct FeeBuilder {
    state: Mutex<FeeBuilderState>,
}

#[uniffi::export]
impl FeeBuilder {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_fee(&self, fee: ForeignU128) -> Result<(), ErrorWrapper> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ErrorWrapper::Wrapped("poisoned lock".to_string()))?;
        state.fee = Some(fee);
        Ok(())
    }

    pub fn set_recipient(&self, recipient: AccountId) -> Result<(), ErrorWrapper> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ErrorWrapper::Wrapped("poisoned lock".to_string()))?;
        state.recipient = Some(recipient);
        Ok(())
    }

    pub fn build(&self) -> Result<Fee, ErrorWrapper> {
        let state = self
            .state
            .lock()
            .map_err(|_| ErrorWrapper::Wrapped("poisoned lock".to_string()))?;

        let Some(fee) = state.fee.clone() else {
            return Err(ErrorWrapper::Wrapped("missing fee".to_string()));
        };

        let Some(recipient) = state.recipient.clone() else {
            return Err(ErrorWrapper::Wrapped("missing recipient".to_string()));
        };

        Ok(Fee { fee, recipient })
    }
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

#[derive(Default)]
struct FeesBuilderState {
    performance_fee: Option<ForeignU128>,
    performance_recipient: Option<AccountId>,
    management_fee: Option<ForeignU128>,
    management_recipient: Option<AccountId>,
    max_total_assets_growth_rate: Option<ForeignU128>,
}

#[derive(uniffi::Object, Default)]
pub struct FeesBuilder {
    state: Mutex<FeesBuilderState>,
}

#[uniffi::export]
impl FeesBuilder {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_performance_fee(&self, fee: ForeignU128) -> Result<(), ErrorWrapper> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ErrorWrapper::Wrapped("poisoned lock".to_string()))?;
        state.performance_fee = Some(fee);
        Ok(())
    }

    pub fn set_performance_recipient(&self, recipient: AccountId) -> Result<(), ErrorWrapper> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ErrorWrapper::Wrapped("poisoned lock".to_string()))?;
        state.performance_recipient = Some(recipient);
        Ok(())
    }

    pub fn set_management_fee(&self, fee: ForeignU128) -> Result<(), ErrorWrapper> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ErrorWrapper::Wrapped("poisoned lock".to_string()))?;
        state.management_fee = Some(fee);
        Ok(())
    }

    pub fn set_management_recipient(&self, recipient: AccountId) -> Result<(), ErrorWrapper> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ErrorWrapper::Wrapped("poisoned lock".to_string()))?;
        state.management_recipient = Some(recipient);
        Ok(())
    }

    pub fn set_max_total_assets_growth_rate(
        &self,
        rate: Option<ForeignU128>,
    ) -> Result<(), ErrorWrapper> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ErrorWrapper::Wrapped("poisoned lock".to_string()))?;
        state.max_total_assets_growth_rate = rate;
        Ok(())
    }

    pub fn build(&self) -> Result<Fees, ErrorWrapper> {
        let state = self
            .state
            .lock()
            .map_err(|_| ErrorWrapper::Wrapped("poisoned lock".to_string()))?;

        let Some(performance_fee) = state.performance_fee.clone() else {
            return Err(ErrorWrapper::Wrapped("missing performance_fee".to_string()));
        };

        let Some(performance_recipient) = state.performance_recipient.clone() else {
            return Err(ErrorWrapper::Wrapped("missing performance_recipient".to_string()));
        };

        let Some(management_fee) = state.management_fee.clone() else {
            return Err(ErrorWrapper::Wrapped("missing management_fee".to_string()));
        };

        let Some(management_recipient) = state.management_recipient.clone() else {
            return Err(ErrorWrapper::Wrapped("missing management_recipient".to_string()));
        };

        Ok(Fees {
            performance: Fee {
                fee: performance_fee,
                recipient: performance_recipient,
            },
            management: Fee {
                fee: management_fee,
                recipient: management_recipient,
            },
            max_total_assets_growth_rate: state.max_total_assets_growth_rate.clone(),
        })
    }
}

#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
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

#[derive(uniffi::Record, Debug, Clone)]
pub struct CapGroup {
    pub id: CapGroupId,
    pub cap: ForeignU128,
    pub relative_cap: ForeignU128,
    pub principal: ForeignU128,
}

#[derive(uniffi::Enum, Debug, Clone)]
pub enum TimelockedAction {
    GuardianChange { account: AccountId },
    SentinelChange { account: AccountId },
    TimelockConfigChange { kind: Option<TimelockKind>, new_timelock_ns: u64 },
    FeesChange { fees: Fees },
    RestrictionsChange { restrictions: Option<Restrictions> },
    CapChange { market: AccountId, new_cap: ForeignU128 },
    CapGroupChange { cap_group: CapGroupId, new_cap: ForeignU128 },
    CapGroupRelativeCapChange { cap_group: CapGroupId, new_relative_cap: ForeignU128 },
    CapGroupMembership { market: MarketId, cap_group: Option<CapGroupId> },
    MarketRemoval { market: AccountId },
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct PendingGovernanceAction {
    pub action: TimelockedAction,
    pub valid_at_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(crate = "near_sdk::serde")]
enum TimelockKindSerde {
    Guardian,
    Sentinel,
    Config,
    Cap,
    MarketRemoval,
}

impl From<TimelockKindSerde> for TimelockKind {
    fn from(value: TimelockKindSerde) -> Self {
        match value {
            TimelockKindSerde::Guardian => TimelockKind::Guardian,
            TimelockKindSerde::Sentinel => TimelockKind::Sentinel,
            TimelockKindSerde::Config => TimelockKind::Config,
            TimelockKindSerde::Cap => TimelockKind::Cap,
            TimelockKindSerde::MarketRemoval => TimelockKind::MarketRemoval,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(crate = "near_sdk::serde")]
enum TimelockedActionSerde {
    GuardianChange { account: String },
    SentinelChange { account: String },
    TimelockConfigChange {
        kind: Option<TimelockKindSerde>,
        new_timelock_ns: U64,
    },
    FeesChange {
        fees: templar_common::vault::Fees<U128>,
    },
    RestrictionsChange {
        restrictions: Option<templar_common::vault::Restrictions>,
    },
    CapChange {
        market: String,
        new_cap: U128,
    },
    CapGroupChange {
        cap_group: templar_common::vault::CapGroupId,
        new_cap: U128,
    },
    CapGroupRelativeCapChange {
        cap_group: templar_common::vault::CapGroupId,
        new_relative_cap: U128,
    },
    CapGroupMembership {
        market: templar_common::vault::MarketId,
        cap_group: Option<templar_common::vault::CapGroupId>,
    },
    MarketRemoval { market: String },
}

impl From<TimelockedActionSerde> for TimelockedAction {
    fn from(value: TimelockedActionSerde) -> Self {
        match value {
            TimelockedActionSerde::GuardianChange { account } => TimelockedAction::GuardianChange {
                account: account.into(),
            },
            TimelockedActionSerde::SentinelChange { account } => TimelockedAction::SentinelChange {
                account: account.into(),
            },
            TimelockedActionSerde::TimelockConfigChange {
                kind,
                new_timelock_ns,
            } => TimelockedAction::TimelockConfigChange {
                kind: kind.map(Into::into),
                new_timelock_ns: new_timelock_ns.0,
            },
            TimelockedActionSerde::FeesChange { fees } => {
                TimelockedAction::FeesChange { fees: fees.into() }
            }
            TimelockedActionSerde::RestrictionsChange { restrictions } => {
                TimelockedAction::RestrictionsChange {
                    restrictions: restrictions.map(Into::into),
                }
            }
            TimelockedActionSerde::CapChange { market, new_cap } => TimelockedAction::CapChange {
                market: market.into(),
                new_cap: new_cap.0.to_string(),
            },
            TimelockedActionSerde::CapGroupChange { cap_group, new_cap } => {
                TimelockedAction::CapGroupChange {
                    cap_group: cap_group.into(),
                    new_cap: new_cap.0.to_string(),
                }
            }
            TimelockedActionSerde::CapGroupRelativeCapChange {
                cap_group,
                new_relative_cap,
            } => TimelockedAction::CapGroupRelativeCapChange {
                cap_group: cap_group.into(),
                new_relative_cap: new_relative_cap.0.to_string(),
            },
            TimelockedActionSerde::CapGroupMembership { market, cap_group } => {
                TimelockedAction::CapGroupMembership {
                    market: market.into(),
                    cap_group: cap_group.map(Into::into),
                }
            }
            TimelockedActionSerde::MarketRemoval { market } => {
                TimelockedAction::MarketRemoval {
                    market: market.into(),
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(crate = "near_sdk::serde")]
struct PendingValueSerde {
    value: TimelockedActionSerde,
    valid_at_ns: u64,
}

#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum UnderlyingToken {
    Nep141 { contract_id: AccountId },
    Nep245 { contract_id: AccountId, token_id: String },
}

#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FeeWad {
    pub fee_wad: ForeignU128,
    pub recipient: AccountId,
}

#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FeesWad {
    pub performance: FeeWad,
    pub management: FeeWad,
    pub max_total_assets_growth_rate_wad: Option<ForeignU128>,
}

#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct VaultConfiguration {
    pub owner: AccountId,
    pub curator: AccountId,
    pub guardian: AccountId,
    pub sentinel: AccountId,
    pub underlying_token: UnderlyingToken,
    pub initial_timelock_ns: u64,
    pub fees: FeesWad,
    pub skim_recipient: AccountId,
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
    pub restrictions: Option<Restrictions>,
    pub refresh_cooldown_ns: Option<u64>,
}

impl From<templar_common::vault::Fee<templar_common::vault::wad::Wad>> for FeeWad {
    fn from(value: templar_common::vault::Fee<templar_common::vault::wad::Wad>) -> Self {
        FeeWad {
            fee_wad: u128::from(value.fee).to_string(),
            recipient: value.recipient.to_string().into(),
        }
    }
}

impl From<templar_common::vault::Fees<templar_common::vault::wad::Wad>> for FeesWad {
    fn from(value: templar_common::vault::Fees<templar_common::vault::wad::Wad>) -> Self {
        FeesWad {
            performance: value.performance.into(),
            management: value.management.into(),
            max_total_assets_growth_rate_wad: value
                .max_total_assets_growth_rate
                .map(|r| u128::from(r).to_string()),
        }
    }
}

impl From<templar_common::vault::VaultConfiguration> for VaultConfiguration {
    fn from(value: templar_common::vault::VaultConfiguration) -> Self {
        let underlying = value.underlying_token.clone();
        let underlying_token = if let Some(contract_id) = underlying.clone().into_nep141() {
            UnderlyingToken::Nep141 {
                contract_id: contract_id.to_string().into(),
            }
        } else if let Some((contract_id, token_id)) = underlying.into_nep245() {
            UnderlyingToken::Nep245 {
                contract_id: contract_id.to_string().into(),
                token_id,
            }
        } else {
            UnderlyingToken::Nep141 {
                contract_id: value.underlying_token.contract_id().to_string().into(),
            }
        };

        VaultConfiguration {
            owner: value.owner.to_string().into(),
            curator: value.curator.to_string().into(),
            guardian: value.guardian.to_string().into(),
            sentinel: value.sentinel.to_string().into(),
            underlying_token,
            initial_timelock_ns: value.initial_timelock_ns.0,
            fees: value.fees.into(),
            skim_recipient: value.skim_recipient.to_string().into(),
            name: value.name,
            symbol: value.symbol,
            decimals: value.decimals.get(),
            restrictions: value.restrictions.map(Into::into),
            refresh_cooldown_ns: value.refresh_cooldown_ns.map(|u| u.0),
        }
    }
}

impl From<(templar_common::vault::CapGroupId, templar_common::vault::CapGroupRecord)> for CapGroup {
    fn from(value: (templar_common::vault::CapGroupId, templar_common::vault::CapGroupRecord)) -> Self {
        let (id, rec) = value;
        CapGroup {
            id: id.into(),
            cap: rec.cap.0.to_string(),
            relative_cap: u128::from(rec.relative_cap).to_string(),
            principal: rec.principal.to_string(),
        }
    }
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct VaultSnapshot {
    pub configuration: VaultConfiguration,
    pub total_assets: ForeignU128,
    pub last_total_assets: ForeignU128,
    pub idle_balance: ForeignU128,
    pub total_supply: ForeignU128,
    pub max_deposit: ForeignU128,
    pub max_single_market_deposit: ForeignU128,
    pub fee_anchor: FeeAccrualAnchor,
    pub fees: Fees,
    pub restrictions: Option<Restrictions>,
    pub cap_groups: Vec<CapGroup>,
    pub pending_governance_actions: Vec<PendingGovernanceAction>,
    pub withdrawing_op_id: Option<u64>,
    pub has_pending_market_withdrawal: bool,
    pub current_withdraw_request_id: Option<u64>,
    pub queue_tail: u64,
    pub next_pending_withdrawal_id: Option<u64>,
    pub markets_with_ids: Vec<MarketWithId>,
}

pub const DEFAULT_GAS: Gas = 300_000_000_000_000;
pub const MAX_POLL_INTERVAL_MILLIS: u64 = 1000;

static TOKIO_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn tokio_runtime() -> &'static tokio::runtime::Runtime {
    TOKIO_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime")
    })
}

fn run_on_tokio<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    tokio_runtime().block_on(future)
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct ViewCacheKey {
    account_id: String,
    method: String,
    args: Vec<u8>,
}

type ViewCache = MokaCache<ViewCacheKey, Vec<u8>>;

#[derive(uniffi::Object)]
pub struct Client {
    inner: JsonRpcClient,
    signer: Signer,
    pub vault: NearAccountId,
    timeout: u64,
    retry: Option<RetryConfig>,
    view_cache: RwLock<Option<ViewCache>>,
}

#[uniffi::export(async_runtime = "tokio")]
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
            retry: None,
            view_cache: RwLock::new(None),
        })
    }

    #[uniffi::constructor]
    #[instrument(skip(signer_key, signer_account, vault), fields(rpc_url = %rpc_url, timeout))]
    pub fn new_client_with_retry(
        rpc_url: String,
        signer_account: &AccountId,
        signer_key: &str,
        vault: &AccountId,
        timeout: u64,
        retry: RetryConfig,
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
            retry: Some(retry.normalized()),
            view_cache: RwLock::new(None),
        })
    }

    pub fn enable_view_cache(&self, capacity: u32, ttl_seconds: u64) {
        if capacity == 0 {
            let mut w = self.view_cache.write().unwrap();
            *w = None;
            return;
        }

        let cache = ViewCache::builder()
            .max_capacity(capacity as u64)
            .time_to_live(Duration::from_secs(ttl_seconds))
            .build();

        let mut w = self.view_cache.write().unwrap();
        *w = Some(cache);
    }

    pub fn disable_view_cache(&self) {
        let mut w = self.view_cache.write().unwrap();
        *w = None;
    }

    pub async fn clear_view_cache(&self) -> Result<(), ErrorWrapper> {
        let cache = { self.view_cache.read().unwrap().clone() };
        if let Some(cache) = cache {
            cache.invalidate_all();
        }
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn get_configuration(&self) -> Result<VaultConfiguration, ErrorWrapper> {
        let cfg = self
            .view::<templar_common::vault::VaultConfiguration>(
                &self.vault,
                "get_configuration",
                (),
                self.timeout,
            )
            .await
            .map_err(ErrorWrapper::from)?;
        Ok(cfg.into())
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
    pub async fn get_cap_groups(&self) -> Result<Vec<CapGroup>, ErrorWrapper> {
        let groups = self
            .view::<Vec<(templar_common::vault::CapGroupId, templar_common::vault::CapGroupRecord)>>(
                &self.vault,
                "get_cap_groups",
                (),
                self.timeout,
            )
            .await
            .map_err(ErrorWrapper::from)?;

        Ok(groups
            .into_iter()
            .map(|(id, rec)| CapGroup {
                id: id.into(),
                cap: rec.cap.0.to_string(),
                relative_cap: u128::from(rec.relative_cap).to_string(),
                principal: rec.principal.to_string(),
            })
            .collect())
    }

    #[instrument(skip(self))]
    pub async fn get_pending_governance_actions(
        &self,
    ) -> Result<Vec<PendingGovernanceAction>, ErrorWrapper> {
        let pending = self
            .view::<Vec<PendingValueSerde>>(
                &self.vault,
                "get_pending_governance_actions",
                (),
                self.timeout,
            )
            .await
            .map_err(ErrorWrapper::from)?;

        Ok(pending
            .into_iter()
            .map(|p| PendingGovernanceAction {
                action: p.value.into(),
                valid_at_ns: p.valid_at_ns,
            })
            .collect())
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

    #[instrument(skip(self))]
    pub async fn get_vault_snapshot(&self) -> Result<VaultSnapshot, ErrorWrapper> {
        Ok(VaultSnapshot {
            configuration: self.get_configuration().await?,
            total_assets: self.get_total_assets().await?,
            last_total_assets: self.get_last_total_assets().await?,
            idle_balance: self.get_idle_balance().await?,
            total_supply: self.get_total_supply().await?,
            max_deposit: self.get_max_deposit().await?,
            max_single_market_deposit: self.get_max_single_market_deposit().await?,
            fee_anchor: self.get_fee_anchor().await?,
            fees: self.get_fees().await?,
            restrictions: self.get_restrictions().await?,
            cap_groups: self.get_cap_groups().await?,
            pending_governance_actions: self.get_pending_governance_actions().await?,
            withdrawing_op_id: self.get_withdrawing_op_id().await?,
            has_pending_market_withdrawal: self.has_pending_market_withdrawal().await?,
            current_withdraw_request_id: self.get_current_withdraw_request_id().await?,
            queue_tail: self.queue_tail().await?,
            next_pending_withdrawal_id: self.peek_next_pending_withdrawal_id().await?,
            markets_with_ids: self.list_markets_with_ids().await?,
        })
    }

    #[instrument(skip(self, markets))]
    pub async fn resolve_market_ids(
        &self,
        markets: &[AccountId],
    ) -> Result<Vec<Option<MarketId>>, ErrorWrapper> {
        let mut out = Vec::with_capacity(markets.len());
        for market in markets {
            out.push(self.get_market_id_of_account(market).await?);
        }
        Ok(out)
    }

    #[instrument(skip(self, market_ids))]
    pub async fn resolve_market_accounts(
        &self,
        market_ids: &[MarketId],
    ) -> Result<Vec<Option<AccountId>>, ErrorWrapper> {
        let mut out = Vec::with_capacity(market_ids.len());
        for id in market_ids {
            out.push(self.get_market_account_by_id(*id).await?);
        }
        Ok(out)
    }

    #[instrument(skip(self))]
    pub async fn refresh_all_markets(&self) -> Result<RealAssetsReport, ErrorWrapper> {
        let markets = self.list_markets_with_ids().await?;
        let market_ids: Vec<MarketId> = markets.into_iter().map(|m| m.market_id).collect();
        self.refresh_markets(&market_ids).await
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
        let report: templar_common::vault::RealAssetsReport = self
            .vault_call_returning("refresh_markets", (markets,), None, None)
            .await?;
        Ok(report.into())
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
            retry: None,
            view_cache: RwLock::new(None),
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
        let args_bytes = serde_json::to_vec(&args)?;
        let key = ViewCacheKey {
            account_id: account_id.to_string(),
            method: function_name.to_string(),
            args: args_bytes.clone(),
        };

        let cache = { self.view_cache.read().unwrap().clone() };
        if let Some(cache) = &cache {
            if let Some(bytes) = cache.get(&key) {
                let value = serde_json::from_slice(&bytes)?;
                return Ok(value);
            }
        }

        let retry = self.retry.map(RetryConfig::normalized);
        let inner = self.inner.clone();
        let account_id = account_id.clone();
        let function_name = function_name.to_owned();

        let result_bytes = run_on_tokio(async move {
            let mut attempts_left = retry.map(|r| r.max_attempts).unwrap_or(1);
            let mut backoff_ms = retry.map(|r| r.initial_backoff_ms).unwrap_or(0);

            loop {
                attempts_left = attempts_left.saturating_sub(1);

                let response = tokio::time::timeout(
                    Duration::from_secs(timeout),
                    inner.call(RpcQueryRequest {
                        block_reference: BlockReference::latest(),
                        request: QueryRequest::CallFunction {
                            account_id: account_id.clone(),
                            method_name: function_name.clone(),
                            args: args_bytes.clone().into(),
                        },
                    }),
                )
                .await;

                let response = match response {
                    Ok(Ok(r)) => r,
                    Ok(Err(e)) => {
                        let err: anyhow::Error = e.into();
                        if attempts_left == 0 || !should_retry(&err) {
                            return Err(err);
                        }
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                        if let Some(r) = retry {
                            backoff_ms = (backoff_ms.saturating_mul(2)).min(r.max_backoff_ms);
                        }
                        continue;
                    }
                    Err(e) => {
                        let err: anyhow::Error = e.into();
                        if attempts_left == 0 || !should_retry(&err) {
                            return Err(err);
                        }
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                        if let Some(r) = retry {
                            backoff_ms = (backoff_ms.saturating_mul(2)).min(r.max_backoff_ms);
                        }
                        continue;
                    }
                };

                let QueryResponseKind::CallResult(result) = response.kind else {
                    bail!("Expected CallResult got {:?}", response.kind);
                };

                return Ok(result.result);
            }
        })?;

        if let Some(cache) = &cache {
            cache.insert(key.clone(), result_bytes.clone());
        }

        let value = serde_json::from_slice(&result_bytes)?;
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

        let signature = self.signer.sign(tx_hash.as_ref());
        let signed_transaction = SignedTransaction::new(signature, tx);

        let retry = self.retry.map(RetryConfig::normalized);
        let inner = self.inner.clone();
        let signer_account_id = self.signer.get_account_id();

        let result = run_on_tokio(async move {
            let called_at = Instant::now();
            let deadline = called_at + Duration::from_secs(timeout);

            let mut attempts_left = retry.map(|r| r.max_attempts).unwrap_or(1);
            let mut backoff_ms = retry.map(|r| r.initial_backoff_ms).unwrap_or(0);

            let result = loop {
                attempts_left = attempts_left.saturating_sub(1);

                let send_res = inner
                    .call(RpcSendTransactionRequest {
                        signed_transaction: signed_transaction.clone(),
                        wait_until: TxExecutionStatus::Final,
                    })
                    .await;

                match send_res {
                    Ok(res) => break Ok(res),
                    Err(e) => {
                        if matches!(e.handler_error(), Some(RpcTransactionError::TimeoutError)) {
                            break Err(e);
                        }

                        if retry.is_none() || attempts_left == 0 || e.handler_error().is_some() {
                            break Err(e);
                        }

                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                        if let Some(r) = retry {
                            backoff_ms = (backoff_ms.saturating_mul(2)).min(r.max_backoff_ms);
                        }
                    }
                }
            };

            let result = match result {
                Ok(res) => res,
                Err(e) => {
                    warn!(
                        "Send transaction error: {:?}. Starting status polling until deadline.",
                        e
                    );

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

                        let status = inner
                            .call(RpcTransactionStatusRequest {
                                transaction_info: TransactionInfo::TransactionId {
                                    sender_account_id: signer_account_id.clone(),
                                    tx_hash,
                                },
                                wait_until: TxExecutionStatus::Final,
                            })
                            .await;

                        let Err(status_err) = status else {
                            break;
                        };

                        if matches!(
                            status_err.handler_error(),
                            Some(RpcTransactionError::TimeoutError)
                        ) {
                            continue;
                        }

                        if retry.is_some() && status_err.handler_error().is_none() {
                            continue;
                        }

                        warn!("Transaction status error: {:?}", status_err);
                        return Err(status_err.into());
                    }

                    inner
                        .call(RpcTransactionStatusRequest {
                            transaction_info: TransactionInfo::TransactionId {
                                sender_account_id: signer_account_id.clone(),
                                tx_hash,
                            },
                            wait_until: TxExecutionStatus::Final,
                        })
                        .await?
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
        })?;

        Ok(result)
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

    async fn vault_call_returning<T: DeserializeOwned>(
        &self,
        method: &str,
        args: impl Serialize,
        gas: Option<Gas>,
        deposit: Option<u128>,
    ) -> Result<T, ErrorWrapper> {
        let status = self
            .call(&self.vault, method, args, gas, deposit, self.timeout)
            .await
            .map_err(ErrorWrapper::from)?;

        let FinalExecutionStatus::SuccessValue(bytes) = status else {
            return Err(ErrorWrapper::Wrapped(
                "Transaction returned no value".to_string(),
            ));
        };

        serde_json::from_slice(&bytes).map_err(ErrorWrapper::from)
    }
}

fn should_retry(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if cause.is::<tokio::time::error::Elapsed>() {
            return true;
        }
        if cause.is::<std::io::Error>() {
            return true;
        }
    }
    false
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

    #[test]
    fn fee_builder_validates_required_fields() {
        let builder = FeeBuilder::new();
        assert!(builder.build().is_err());

        builder.set_fee("1".to_string()).unwrap();
        assert!(builder.build().is_err());

        builder
            .set_recipient(AccountId("topgunbakugo.testnet".to_string()))
            .unwrap();

        let built = builder.build().unwrap();
        assert_eq!(built.fee, "1");
        assert_eq!(built.recipient.0, "topgunbakugo.testnet");
    }

    #[test]
    fn fees_builder_builds_expected_structure() {
        let builder = FeesBuilder::new();

        builder.set_performance_fee("10".to_string()).unwrap();
        builder
            .set_performance_recipient(AccountId("perf.testnet".to_string()))
            .unwrap();
        builder.set_management_fee("20".to_string()).unwrap();
        builder
            .set_management_recipient(AccountId("mgmt.testnet".to_string()))
            .unwrap();
        builder
            .set_max_total_assets_growth_rate(Some("30".to_string()))
            .unwrap();

        let built = builder.build().unwrap();
        assert_eq!(built.performance.fee, "10");
        assert_eq!(built.performance.recipient.0, "perf.testnet");
        assert_eq!(built.management.fee, "20");
        assert_eq!(built.management.recipient.0, "mgmt.testnet");
        assert_eq!(built.max_total_assets_growth_rate, Some("30".to_string()));

        let common: templar_common::vault::Fees<U128> = built.try_into().unwrap();
        assert_eq!(common.performance.fee.0, 10);
        assert_eq!(common.performance.recipient.as_str(), "perf.testnet");
        assert_eq!(common.management.fee.0, 20);
        assert_eq!(common.management.recipient.as_str(), "mgmt.testnet");
        assert_eq!(common.max_total_assets_growth_rate.unwrap().0, 30);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn view_cache_hit_and_miss() {
        let cache = ViewCache::builder()
            .max_capacity(16)
            .time_to_live(Duration::from_secs(60))
            .build();

        let key = ViewCacheKey {
            account_id: "a".to_string(),
            method: "m".to_string(),
            args: vec![1, 2, 3],
        };

        assert!(cache.get(&key).is_none());

        cache.insert(key.clone(), br"123".to_vec());

        let got = cache.get(&key);
        assert_eq!(got, Some(br"123".to_vec()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn view_cache_ttl_expires() {
        let cache = ViewCache::builder()
            .max_capacity(16)
            .time_to_live(Duration::from_millis(1))
            .build();

        let key = ViewCacheKey {
            account_id: "a".to_string(),
            method: "m".to_string(),
            args: vec![9],
        };

        cache.insert(key.clone(), br"xyz".to_vec());
        tokio::time::sleep(Duration::from_millis(5)).await;

        assert!(cache.get(&key).is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn view_cache_capacity_is_respected() {
        let cache = ViewCache::builder()
            .max_capacity(2)
            .time_to_live(Duration::from_secs(60))
            .build();

        let k1 = ViewCacheKey {
            account_id: "a".to_string(),
            method: "m1".to_string(),
            args: vec![1],
        };
        let k2 = ViewCacheKey {
            account_id: "a".to_string(),
            method: "m2".to_string(),
            args: vec![2],
        };
        let k3 = ViewCacheKey {
            account_id: "a".to_string(),
            method: "m3".to_string(),
            args: vec![3],
        };

        cache.insert(k1.clone(), br"1".to_vec());
        cache.insert(k2.clone(), br"2".to_vec());
        cache.insert(k3.clone(), br"3".to_vec());

        let keys = [k1.clone(), k2.clone(), k3.clone()];

        let mut present = keys.iter().filter(|k| cache.get(*k).is_some()).count();
        for _ in 0..5 {
            if present <= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
            present = keys.iter().filter(|k| cache.get(*k).is_some()).count();
        }

        assert!(present <= 2);
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
