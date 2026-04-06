use std::{fmt::Display, str::FromStr, sync::Mutex};

use mini_moka::sync::Cache as MokaCache;
use near_account_id::AccountId as NearAccountId;
use near_jsonrpc_client::{
    errors::JsonRpcError,
    methods::{query::RpcQueryError, tx::RpcTransactionError},
};
use near_primitives::errors::InvalidTxError;
use near_primitives::types::Gas;
use near_sdk::json_types::{U128, U64};
use serde::{Deserialize, Serialize};
use templar_vault_kernel::Address as KernelAddress;

pub use client::{VaultClient, VaultClientConfig};
pub use key_pool::{KeyCredential, KeyInfo, KeyPoolClient, KeyPoolConfig, PoolError, PoolHealth};
pub use view_client::VaultViewClient;

mod key_pool;
mod lock_ext;
#[macro_use]
mod methods;
mod client;
mod retry;
mod view_client;
mod view_core;

use lock_ext::MutexExt;

uniffi::setup_scaffolding!();

type ForeignU128 = String;

#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl RetryConfig {
    pub(crate) fn normalized(self) -> Self {
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

impl TryFrom<AccountId> for near_account_id::AccountId {
    type Error = ErrorWrapper;

    fn try_from(value: AccountId) -> Result<Self, Self::Error> {
        near_account_id::AccountId::from_str(&value.0).map_err(|e| {
            ErrorWrapper::InvalidAccountId(format!("Invalid AccountId '{}': {}", value.0, e))
        })
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

/// Generate a UniFFI-compatible newtype wrapper with standard conversions.
///
/// This macro generates:
/// - A newtype struct with standard derives
/// - `uniffi::custom_type!` registration
/// - `From<Inner>` and `From<Wrapper>` for inner type conversions
/// - `From<External>` and `From<Wrapper>` for external type conversions
macro_rules! define_uniffi_wrapper {
    // Variant with external type conversion and extra derives
    ($name:ident, $inner:ty, [$($derive:ident),*], $external:path) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash $(, $derive)*)]
        pub struct $name(pub $inner);

        uniffi::custom_type!($name, $inner);

        impl From<$inner> for $name {
            fn from(value: $inner) -> Self {
                $name(value)
            }
        }

        impl From<$name> for $inner {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl From<$external> for $name {
            fn from(value: $external) -> Self {
                $name(value.0)
            }
        }

        impl From<$name> for $external {
            fn from(value: $name) -> Self {
                $external(value.0)
            }
        }
    };
}

define_uniffi_wrapper!(MarketId, u32, [Copy], templar_common::vault::MarketId);
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapGroupId(pub String);

uniffi::custom_type!(CapGroupId, String);

impl TryFrom<String> for CapGroupId {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        templar_common::vault::CapGroupId::try_from(value.clone())
            .map(Into::into)
            .map_err(|err| anyhow::anyhow!("Invalid CapGroupId '{value}': {err:?}"))
    }
}

impl TryFrom<CapGroupId> for templar_common::vault::CapGroupId {
    type Error = ErrorWrapper;

    fn try_from(value: CapGroupId) -> Result<Self, Self::Error> {
        templar_common::vault::CapGroupId::try_from(value.0.clone()).map_err(|err| {
            ErrorWrapper::InvalidCapGroupId(format!("Invalid CapGroupId '{}': {:?}", value.0, err))
        })
    }
}

impl From<templar_common::vault::CapGroupId> for CapGroupId {
    fn from(value: templar_common::vault::CapGroupId) -> Self {
        Self(String::from(value))
    }
}

impl From<CapGroupId> for String {
    fn from(value: CapGroupId) -> Self {
        value.0
    }
}

impl From<MarketId> for u64 {
    fn from(value: MarketId) -> Self {
        u64::from(templar_common::vault::MarketId::from(value))
    }
}

impl TryFrom<u64> for MarketId {
    type Error = <templar_common::vault::MarketId as TryFrom<u64>>::Error;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        templar_common::vault::MarketId::try_from(value).map(Into::into)
    }
}

/// Generate a UniFFI-compatible builder for a simple struct.
///
/// This macro generates:
/// - An internal state struct with optional fields
/// - A builder struct with `#[derive(uniffi::Object, Default)]`
/// - A `#[uniffi::export]` impl block with setters and build method
///
/// Note: Due to proc-macro limitations, this generates code that must be
/// wrapped in a `paste::paste!` block for identifier concatenation.
macro_rules! define_uniffi_builder {
    (
        $builder:ident,
        $target:ident,
        { $($field:ident: $field_ty:ty),* $(,)? }
    ) => {
        paste::paste! {
            #[derive(Default)]
            struct [<$builder State>] {
                $($field: Option<$field_ty>,)*
            }

            #[derive(uniffi::Object, Default)]
            pub struct $builder {
                state: Mutex<[<$builder State>]>,
            }

            #[uniffi::export]
            impl $builder {
                #[uniffi::constructor]
                pub fn new() -> Self {
                    Self::default()
                }

                $(
                    pub fn [<set_ $field>](&self, $field: $field_ty) -> Result<(), ErrorWrapper> {
                        let mut state = self.state.lock_or_poison()?;
                        state.$field = Some($field);
                        Ok(())
                    }
                )*

                pub fn build(&self) -> Result<$target, ErrorWrapper> {
                    let state = self.state.lock_or_poison()?;
                    $(
                        let Some($field) = state.$field.clone() else {
                            return Err(ErrorWrapper::Wrapped(
                                concat!("missing ", stringify!($field)).to_string()
                            ));
                        };
                    )*
                    Ok($target { $($field),* })
                }
            }
        }
    };
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
            recipient: parse_account_id(&value.recipient)?,
        })
    }
}

define_uniffi_builder!(FeeBuilder, Fee, {
    fee: ForeignU128,
    recipient: AccountId,
});

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
        let mut state = self.state.lock_or_poison()?;
        state.performance_fee = Some(fee);
        Ok(())
    }

    pub fn set_performance_recipient(&self, recipient: AccountId) -> Result<(), ErrorWrapper> {
        let mut state = self.state.lock_or_poison()?;
        state.performance_recipient = Some(recipient);
        Ok(())
    }

    pub fn set_management_fee(&self, fee: ForeignU128) -> Result<(), ErrorWrapper> {
        let mut state = self.state.lock_or_poison()?;
        state.management_fee = Some(fee);
        Ok(())
    }

    pub fn set_management_recipient(&self, recipient: AccountId) -> Result<(), ErrorWrapper> {
        let mut state = self.state.lock_or_poison()?;
        state.management_recipient = Some(recipient);
        Ok(())
    }

    pub fn set_max_total_assets_growth_rate(
        &self,
        rate: Option<ForeignU128>,
    ) -> Result<(), ErrorWrapper> {
        let mut state = self.state.lock_or_poison()?;
        state.max_total_assets_growth_rate = rate;
        Ok(())
    }

    pub fn build(&self) -> Result<Fees, ErrorWrapper> {
        let state = self.state.lock_or_poison()?;

        let Some(performance_fee) = state.performance_fee.clone() else {
            return Err(ErrorWrapper::Wrapped("missing performance_fee".to_string()));
        };

        let Some(performance_recipient) = state.performance_recipient.clone() else {
            return Err(ErrorWrapper::Wrapped(
                "missing performance_recipient".to_string(),
            ));
        };

        let Some(management_fee) = state.management_fee.clone() else {
            return Err(ErrorWrapper::Wrapped("missing management_fee".to_string()));
        };

        let Some(management_recipient) = state.management_recipient.clone() else {
            return Err(ErrorWrapper::Wrapped(
                "missing management_recipient".to_string(),
            ));
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
    Blacklist(Vec<String>),
    Whitelist(Vec<String>),
}

impl From<templar_common::vault::Restrictions> for Restrictions {
    fn from(value: templar_common::vault::Restrictions) -> Self {
        match value {
            templar_common::vault::Restrictions::Paused => Restrictions::Paused,
            templar_common::vault::Restrictions::Blacklist(list) => {
                Restrictions::Blacklist(list.iter().map(address_to_hex).collect())
            }
            templar_common::vault::Restrictions::Whitelist(list) => {
                Restrictions::Whitelist(list.iter().map(address_to_hex).collect())
            }
        }
    }
}

impl TryFrom<Restrictions> for templar_common::vault::Restrictions {
    type Error = ErrorWrapper;

    fn try_from(value: Restrictions) -> Result<Self, Self::Error> {
        Ok(match value {
            Restrictions::Paused => templar_common::vault::Restrictions::Paused,
            Restrictions::Blacklist(addresses) => {
                let list: Vec<KernelAddress> = addresses
                    .iter()
                    .map(String::as_str)
                    .map(parse_hex_address)
                    .collect::<Result<_, _>>()?;
                templar_common::vault::Restrictions::Blacklist(list)
            }
            Restrictions::Whitelist(addresses) => {
                let list: Vec<KernelAddress> = addresses
                    .iter()
                    .map(String::as_str)
                    .map(parse_hex_address)
                    .collect::<Result<_, _>>()?;
                templar_common::vault::Restrictions::Whitelist(list)
            }
        })
    }
}

#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum CapGroupUpdate {
    SetCap {
        cap_group_id: CapGroupId,
        new_cap: Option<ForeignU128>,
    },
    SetRelativeCap {
        cap_group_id: CapGroupId,
        new_relative_cap: Option<ForeignU128>,
    },
    SetMembership {
        market_id: MarketId,
        cap_group_id: Option<CapGroupId>,
    },
}

impl CapGroupUpdate {
    fn try_into_common(self) -> Result<templar_common::vault::CapGroupUpdate, ErrorWrapper> {
        Ok(match self {
            CapGroupUpdate::SetCap {
                cap_group_id,
                new_cap,
            } => templar_common::vault::CapGroupUpdate::SetCap {
                cap_group_id: cap_group_id.try_into()?,
                new_cap: new_cap.map(|cap| parse_u128(&cap)).transpose()?,
            },
            CapGroupUpdate::SetRelativeCap {
                cap_group_id,
                new_relative_cap,
            } => templar_common::vault::CapGroupUpdate::SetRelativeCap {
                cap_group_id: cap_group_id.try_into()?,
                new_relative_cap: new_relative_cap
                    .map(|cap| parse_u128(&cap).map(templar_vault_kernel::Wad::from))
                    .transpose()?,
            },
            CapGroupUpdate::SetMembership {
                market_id,
                cap_group_id,
            } => templar_common::vault::CapGroupUpdate::SetMembership {
                market_id: market_id.0,
                cap_group_id: cap_group_id.map(TryInto::try_into).transpose()?,
            },
        })
    }
}

impl TryFrom<CapGroupUpdate> for templar_common::vault::CapGroupUpdate {
    type Error = ErrorWrapper;

    fn try_from(value: CapGroupUpdate) -> Result<Self, Self::Error> {
        value.try_into_common()
    }
}

impl From<templar_common::vault::CapGroupUpdate> for CapGroupUpdate {
    fn from(value: templar_common::vault::CapGroupUpdate) -> Self {
        match value {
            templar_common::vault::CapGroupUpdate::SetCap {
                cap_group_id,
                new_cap,
            } => Self::SetCap {
                cap_group_id: cap_group_id.into(),
                new_cap: new_cap.map(|cap| cap.to_string()),
            },
            templar_common::vault::CapGroupUpdate::SetRelativeCap {
                cap_group_id,
                new_relative_cap,
            } => Self::SetRelativeCap {
                cap_group_id: cap_group_id.into(),
                new_relative_cap: new_relative_cap.map(|cap| u128::from(cap).to_string()),
            },
            templar_common::vault::CapGroupUpdate::SetMembership {
                market_id,
                cap_group_id,
            } => Self::SetMembership {
                market_id: MarketId(market_id),
                cap_group_id: cap_group_id.map(Into::into),
            },
        }
    }
}

#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum CapGroupUpdateKey {
    SetCap { cap_group_id: CapGroupId },
    SetRelativeCap { cap_group_id: CapGroupId },
    SetMembership { market_id: MarketId },
}

impl CapGroupUpdateKey {
    fn try_into_common(self) -> Result<templar_common::vault::CapGroupUpdateKey, ErrorWrapper> {
        Ok(match self {
            CapGroupUpdateKey::SetCap { cap_group_id } => {
                templar_common::vault::CapGroupUpdateKey::SetCap {
                    cap_group_id: cap_group_id.try_into()?,
                }
            }
            CapGroupUpdateKey::SetRelativeCap { cap_group_id } => {
                templar_common::vault::CapGroupUpdateKey::SetRelativeCap {
                    cap_group_id: cap_group_id.try_into()?,
                }
            }
            CapGroupUpdateKey::SetMembership { market_id } => {
                templar_common::vault::CapGroupUpdateKey::SetMembership {
                    market_id: market_id.0,
                }
            }
        })
    }
}

impl TryFrom<CapGroupUpdateKey> for templar_common::vault::CapGroupUpdateKey {
    type Error = ErrorWrapper;

    fn try_from(value: CapGroupUpdateKey) -> Result<Self, Self::Error> {
        value.try_into_common()
    }
}

impl From<templar_common::vault::CapGroupUpdateKey> for CapGroupUpdateKey {
    fn from(value: templar_common::vault::CapGroupUpdateKey) -> Self {
        match value {
            templar_common::vault::CapGroupUpdateKey::SetCap { cap_group_id } => {
                CapGroupUpdateKey::SetCap {
                    cap_group_id: cap_group_id.into(),
                }
            }
            templar_common::vault::CapGroupUpdateKey::SetRelativeCap { cap_group_id } => {
                CapGroupUpdateKey::SetRelativeCap {
                    cap_group_id: cap_group_id.into(),
                }
            }
            templar_common::vault::CapGroupUpdateKey::SetMembership { market_id } => {
                CapGroupUpdateKey::SetMembership {
                    market_id: MarketId(market_id),
                }
            }
        }
    }
}

#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
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

#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum IdleResyncOutcome {
    Ok,
    BalanceReadFailed,
    UnexpectedState,
    Ignored,
}

impl From<templar_common::vault::IdleResyncOutcome> for IdleResyncOutcome {
    fn from(value: templar_common::vault::IdleResyncOutcome) -> Self {
        match value {
            templar_common::vault::IdleResyncOutcome::Ok => IdleResyncOutcome::Ok,
            templar_common::vault::IdleResyncOutcome::BalanceReadFailed => {
                IdleResyncOutcome::BalanceReadFailed
            }
            templar_common::vault::IdleResyncOutcome::UnexpectedState => {
                IdleResyncOutcome::UnexpectedState
            }
            templar_common::vault::IdleResyncOutcome::Ignored => IdleResyncOutcome::Ignored,
        }
    }
}

#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct ResyncIdleReport {
    pub outcome: IdleResyncOutcome,
    pub before_idle: ForeignU128,
    pub actual_idle: ForeignU128,
    pub after_idle: ForeignU128,
    pub increased_by: ForeignU128,
    pub decreased_by: ForeignU128,
    pub fee_anchor_bump: ForeignU128,
    pub resynced_at_ns: u64,
}

impl From<templar_common::vault::ResyncIdleReport> for ResyncIdleReport {
    fn from(value: templar_common::vault::ResyncIdleReport) -> Self {
        ResyncIdleReport {
            outcome: value.outcome.into(),
            before_idle: value.before_idle.0.to_string(),
            actual_idle: value.actual_idle.0.to_string(),
            after_idle: value.after_idle.0.to_string(),
            increased_by: value.increased_by.0.to_string(),
            decreased_by: value.decreased_by.0.to_string(),
            fee_anchor_bump: value.fee_anchor_bump.0.to_string(),
            resynced_at_ns: value.resynced_at_ns.0,
        }
    }
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
    pub cap: Option<ForeignU128>,
    pub relative_cap: Option<ForeignU128>,
    pub principal: ForeignU128,
}

impl
    From<(
        templar_common::vault::CapGroupId,
        templar_common::vault::CapGroupRecord,
    )> for CapGroup
{
    fn from(
        value: (
            templar_common::vault::CapGroupId,
            templar_common::vault::CapGroupRecord,
        ),
    ) -> Self {
        let (id, rec) = value;
        CapGroup {
            id: id.into(),
            cap: rec.cap.absolute_cap().map(|cap| cap.to_string()),
            relative_cap: rec
                .cap
                .relative_cap()
                .map(|cap| u128::from(cap).to_string()),
            principal: rec.principal.to_string(),
        }
    }
}

#[derive(uniffi::Enum, Debug, Clone)]
pub enum TimelockedAction {
    GuardianChange {
        account: AccountId,
    },
    SentinelChange {
        account: AccountId,
    },
    TimelockConfigChange {
        kind: Option<TimelockKind>,
        new_timelock_ns: u64,
    },
    FeesChange {
        fees: Fees,
    },
    RestrictionsChange {
        restrictions: Option<Restrictions>,
    },
    CapChange {
        market: AccountId,
        new_cap: ForeignU128,
    },
    CapGroupChange {
        cap_group: CapGroupId,
        new_cap: Option<ForeignU128>,
    },
    CapGroupRelativeCapChange {
        cap_group: CapGroupId,
        new_relative_cap: Option<ForeignU128>,
    },
    CapGroupMembership {
        market: MarketId,
        cap_group: Option<CapGroupId>,
    },
    MarketRemoval {
        market: AccountId,
    },
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct PendingGovernanceAction {
    pub action: TimelockedAction,
    pub ready_at_ns: u64,
}

fn timelocked_action_from_common_cap_group_update(
    update: templar_common::vault::CapGroupUpdate,
) -> TimelockedAction {
    match CapGroupUpdate::from(update) {
        CapGroupUpdate::SetCap {
            cap_group_id,
            new_cap,
        } => TimelockedAction::CapGroupChange {
            cap_group: cap_group_id,
            new_cap,
        },
        CapGroupUpdate::SetRelativeCap {
            cap_group_id,
            new_relative_cap,
        } => TimelockedAction::CapGroupRelativeCapChange {
            cap_group: cap_group_id,
            new_relative_cap,
        },
        CapGroupUpdate::SetMembership {
            market_id,
            cap_group_id,
        } => TimelockedAction::CapGroupMembership {
            market: market_id,
            cap_group: cap_group_id,
        },
    }
}

// Wire format types for deserializing NEAR RPC responses.
//
// These exist because NEAR JSON uses U64/U128 wrappers (numbers as strings like "123")
// while UniFFI needs primitive types (u64) or String for large integers. The two
// representations are incompatible, so we deserialize into these intermediate types
// then convert to the UniFFI-exported types via From impls.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub(crate) enum TimelockedActionSerde {
    GuardianChange {
        account: String,
    },
    SentinelChange {
        account: String,
    },
    TimelockConfigChange {
        kind: Option<TimelockKind>,
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
        new_cap: Option<U128>,
    },
    CapGroupRelativeCapChange {
        cap_group: templar_common::vault::CapGroupId,
        new_relative_cap: Option<U128>,
    },
    CapGroupMembership {
        market: templar_common::vault::MarketId,
        cap_group: Option<templar_common::vault::CapGroupId>,
    },
    MarketRemoval {
        market: String,
    },
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
                kind,
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
                timelocked_action_from_common_cap_group_update(
                    templar_common::vault::CapGroupUpdate::SetCap {
                        cap_group_id: cap_group,
                        new_cap: new_cap.map(|value| value.0),
                    },
                )
            }
            TimelockedActionSerde::CapGroupRelativeCapChange {
                cap_group,
                new_relative_cap,
            } => timelocked_action_from_common_cap_group_update(
                templar_common::vault::CapGroupUpdate::SetRelativeCap {
                    cap_group_id: cap_group,
                    new_relative_cap: new_relative_cap
                        .map(|value| templar_vault_kernel::Wad::from(value.0)),
                },
            ),
            TimelockedActionSerde::CapGroupMembership { market, cap_group } => {
                timelocked_action_from_common_cap_group_update(
                    templar_common::vault::CapGroupUpdate::SetMembership {
                        market_id: market.0,
                        cap_group_id: cap_group,
                    },
                )
            }
            TimelockedActionSerde::MarketRemoval { market } => TimelockedAction::MarketRemoval {
                market: market.into(),
            },
        }
    }
}

#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub(crate) struct PendingValueSerde {
    pub value: TimelockedActionSerde,
    pub ready_at_ns: u64,
}

#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum UnderlyingToken {
    Nep141 {
        contract_id: AccountId,
    },
    Nep245 {
        contract_id: AccountId,
        token_id: String,
    },
}

#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FeeWad {
    pub fee_wad: ForeignU128,
    pub recipient: AccountId,
}

impl From<templar_common::vault::Fee<templar_common::vault::prelude::Wad>> for FeeWad {
    fn from(value: templar_common::vault::Fee<templar_common::vault::prelude::Wad>) -> Self {
        FeeWad {
            fee_wad: u128::from(value.fee).to_string(),
            recipient: value.recipient.to_string().into(),
        }
    }
}

#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FeesWad {
    pub performance: FeeWad,
    pub management: FeeWad,
    pub max_total_assets_growth_rate_wad: Option<ForeignU128>,
}

impl From<templar_common::vault::Fees<templar_common::vault::prelude::Wad>> for FeesWad {
    fn from(value: templar_common::vault::Fees<templar_common::vault::prelude::Wad>) -> Self {
        FeesWad {
            performance: value.performance.into(),
            management: value.management.into(),
            max_total_assets_growth_rate_wad: value
                .max_total_assets_growth_rate
                .map(|r| u128::from(r).to_string()),
        }
    }
}

#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct VaultConfiguration {
    pub owner: AccountId,
    pub curator: AccountId,
    pub sentinel: AccountId,
    pub underlying_token: UnderlyingToken,
    pub initial_timelock_ns: u64,
    pub fees: FeesWad,
    pub skim_recipient: AccountId,
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
    pub restrictions: Option<Restrictions>,
    pub withdrawal_cooldown_ns: Option<u64>,
    pub refresh_cooldown_ns: Option<u64>,
    pub idle_resync_cooldown_ns: Option<u64>,
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
            sentinel: value.sentinel.to_string().into(),
            underlying_token,
            initial_timelock_ns: value.initial_timelock_ns.0,
            fees: value.fees.into(),
            skim_recipient: value.skim_recipient.to_string().into(),
            name: value.name,
            symbol: value.symbol,
            decimals: value.decimals.get(),
            restrictions: value.restrictions.map(Into::into),
            withdrawal_cooldown_ns: value.withdrawal_cooldown_ns.map(|u| u.0),
            refresh_cooldown_ns: value.refresh_cooldown_ns.map(|u| u.0),
            idle_resync_cooldown_ns: value.idle_resync_cooldown_ns.map(|u| u.0),
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

/// Storage balance bounds from NEP-145.
#[derive(uniffi::Record, Clone, Debug)]
pub struct StorageBalanceBounds {
    pub min: ForeignU128,
    pub max: Option<ForeignU128>,
}

/// Storage balance from NEP-145.
#[derive(uniffi::Record, Clone, Debug)]
pub struct StorageBalance {
    pub total: ForeignU128,
    pub available: ForeignU128,
}

pub const DEFAULT_GAS: Gas = 300_000_000_000_000;
pub const MAX_POLL_INTERVAL_MILLIS: u64 = 1000;

#[derive(Clone, Hash, PartialEq, Eq)]
pub(crate) struct ViewCacheKey {
    pub account_id: String,
    pub method: String,
    /// JSON-serialized args bytes.
    ///
    /// Cache hit reliability depends on deterministic serialization of `args`.
    /// Prefer structs/tuples or map types with stable ordering (for example
    /// `BTreeMap`) when building view call arguments.
    pub args: Vec<u8>,
}

pub(crate) type ViewCache = MokaCache<ViewCacheKey, Vec<u8>>;

pub(crate) fn parse_u128(s: &str) -> Result<u128, ErrorWrapper> {
    if let Ok(v) = s.parse::<u128>() {
        return Ok(v);
    }

    let inner: String =
        serde_json::from_str(s).map_err(|_| ErrorWrapper::InvalidU128(s.to_string()))?;
    inner
        .parse::<u128>()
        .map_err(|_| ErrorWrapper::InvalidU128(inner))
}

pub(crate) fn parse_account_id(account_id: &AccountId) -> Result<NearAccountId, ErrorWrapper> {
    NearAccountId::try_from(account_id.clone())
}

fn address_to_hex(addr: &KernelAddress) -> String {
    format!("0x{}", hex::encode(addr.0))
}

fn parse_hex_address(input: &str) -> Result<KernelAddress, ErrorWrapper> {
    let trimmed = input.trim();
    let hex_str = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    let bytes = hex::decode(hex_str).map_err(|e| {
        ErrorWrapper::Wrapped(format!("invalid restriction address '{input}': {e}"))
    })?;
    if bytes.len() != 32 {
        return Err(ErrorWrapper::Wrapped(format!(
            "invalid restriction address '{input}': expected 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(KernelAddress(out))
}

#[derive(uniffi::Error, Debug)]
pub enum ErrorWrapper {
    /// The operation timed out.
    Timeout(String),

    /// Transaction submission failed due to a nonce mismatch.
    InvalidNonce,

    /// Input account ID was invalid.
    InvalidAccountId(String),
    InvalidCapGroupId(String),
    /// Input numeric string was invalid.
    InvalidU128(String),

    /// JSON-RPC related error.
    Rpc(String),

    /// (De)serialization error.
    Serde(String),

    /// On-chain transaction failure.
    TransactionFailed(String),

    /// Fallback bucket.
    Wrapped(String),
}

impl<T: Into<anyhow::Error>> From<T> for ErrorWrapper {
    fn from(err: T) -> Self {
        let err: anyhow::Error = err.into();
        let msg = err.to_string();

        // Try to preserve error category for FFI consumers.
        if msg.contains("InvalidNonce") || msg.contains("invalid nonce") {
            return ErrorWrapper::InvalidNonce;
        }

        for cause in err.chain() {
            if cause.is::<tokio::time::error::Elapsed>() {
                return ErrorWrapper::Timeout(msg);
            }
            if cause.is::<serde_json::Error>() {
                return ErrorWrapper::Serde(msg);
            }

            if let Some(rpc_tx_err) = cause.downcast_ref::<JsonRpcError<RpcTransactionError>>() {
                if matches!(
                    rpc_tx_err.handler_error(),
                    Some(RpcTransactionError::InvalidTransaction {
                        context: InvalidTxError::InvalidNonce { .. }
                    })
                ) {
                    return ErrorWrapper::InvalidNonce;
                }

                if matches!(
                    rpc_tx_err.handler_error(),
                    Some(RpcTransactionError::InvalidTransaction { .. })
                ) {
                    return ErrorWrapper::TransactionFailed(msg);
                }

                return ErrorWrapper::Rpc(msg);
            }

            if cause
                .downcast_ref::<JsonRpcError<RpcQueryError>>()
                .is_some()
            {
                return ErrorWrapper::Rpc(msg);
            }
        }

        if msg.starts_with("Transaction failed:") || msg.contains("ActionError") {
            return ErrorWrapper::TransactionFailed(msg);
        }

        ErrorWrapper::Wrapped(msg)
    }
}

impl Display for ErrorWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorWrapper::Timeout(msg) => write!(f, "Timeout: {msg}"),
            ErrorWrapper::InvalidNonce => write!(f, "InvalidNonce"),
            ErrorWrapper::InvalidAccountId(msg)
            | ErrorWrapper::InvalidCapGroupId(msg)
            | ErrorWrapper::InvalidU128(msg)
            | ErrorWrapper::Wrapped(msg) => write!(f, "{msg}"),
            ErrorWrapper::Rpc(msg) => write!(f, "RPC error: {msg}"),
            ErrorWrapper::Serde(msg) => write!(f, "Serde error: {msg}"),
            ErrorWrapper::TransactionFailed(msg) => write!(f, "Transaction failed: {msg}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use near_crypto::{KeyType, SecretKey};
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

    #[test]
    fn market_id_roundtrips_u64() {
        let id = MarketId(42);
        assert_eq!(u64::from(id), 42);
        assert_eq!(MarketId::try_from(42_u64), Ok(id));
    }

    #[test]
    fn market_id_error_wrapper_mapping_rejects_out_of_range() {
        let err = MarketId::try_from(u64::from(u32::MAX) + 1)
            .map_err(|_| ErrorWrapper::Wrapped("market id out of u32 range".to_string()))
            .unwrap_err();
        assert_eq!(err.to_string(), "market id out of u32 range".to_string());
    }

    #[fixture]
    fn sk() -> SecretKey {
        SecretKey::from_random(KeyType::ED25519)
    }

    #[rstest]
    fn account_id_conversion_happy_path(everything: AccountId) {
        let near_id: NearAccountId = everything.clone().try_into().unwrap();
        assert_eq!(near_id.as_str(), "topgunbakugo.testnet");
    }

    #[test]
    fn account_id_conversion_invalid_returns_error() {
        let invalid = AccountId("not a valid account id!!!".to_string());
        let result: Result<NearAccountId, ErrorWrapper> = invalid.try_into();
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ErrorWrapper::InvalidAccountId(msg) => {
                assert!(msg.contains("Invalid AccountId"));
                assert!(msg.contains("not a valid account id!!!"));
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn error_wrapper_display_happy_path() {
        let err = ErrorWrapper::from(anyhow::anyhow!("boom"));
        let s = format!("{err}");
        assert!(s.contains("boom"));
    }

    #[test]
    fn default_gas_is_nonzero() {
        assert_ne!(super::DEFAULT_GAS, 0);
    }

    #[rstest]
    fn can_construct_client_happy_path(
        vault: AccountId,
        everything: AccountId,
        testnet_rpc: String,
        sk: SecretKey,
    ) {
        let credential = KeyCredential {
            account_id: everything,
            secret_key: sk.to_string(),
        };
        VaultClient::new_single_key_default(testnet_rpc, &vault, credential)
            .expect("VaultClient should be created");
    }

    #[test]
    fn parse_u128_accepts_plain_and_json_string() {
        assert_eq!(super::parse_u128("123").unwrap(), 123);
        assert_eq!(super::parse_u128("\"456\"").unwrap(), 456);
    }

    #[test]
    fn parse_u128_rejects_invalid_input_with_specific_variant() {
        let err = super::parse_u128("not-a-number").unwrap_err();
        assert!(matches!(err, ErrorWrapper::InvalidU128(v) if v == "not-a-number"));
    }

    #[test]
    fn cap_group_update_try_into_common_maps_variants() {
        let set_cap = CapGroupUpdate::SetCap {
            cap_group_id: CapGroupId("group-a".to_string()),
            new_cap: Some("10".to_string()),
        };
        let common: templar_common::vault::CapGroupUpdate = set_cap.try_into().unwrap();
        assert!(matches!(
            common,
            templar_common::vault::CapGroupUpdate::SetCap { cap_group_id, new_cap }
                if cap_group_id.as_str() == "group-a" && new_cap == Some(10)
        ));

        let set_relative = CapGroupUpdate::SetRelativeCap {
            cap_group_id: CapGroupId("group-b".to_string()),
            new_relative_cap: Some("20".to_string()),
        };
        let common: templar_common::vault::CapGroupUpdate = set_relative.try_into().unwrap();
        assert!(matches!(
            common,
            templar_common::vault::CapGroupUpdate::SetRelativeCap { cap_group_id, new_relative_cap }
                if cap_group_id.as_str() == "group-b"
                    && new_relative_cap == Some(templar_vault_kernel::Wad::from(20u128))
        ));

        let set_membership = CapGroupUpdate::SetMembership {
            market_id: MarketId(7),
            cap_group_id: Some(CapGroupId("group-c".to_string())),
        };
        let common: templar_common::vault::CapGroupUpdate = set_membership.try_into().unwrap();
        assert!(matches!(
            common,
            templar_common::vault::CapGroupUpdate::SetMembership { market_id, cap_group_id }
                if market_id == 7
                    && cap_group_id.as_ref().map(|id| id.as_str()) == Some("group-c")
        ));
    }

    #[test]
    fn cap_group_update_try_into_common_rejects_invalid_u128() {
        let update = CapGroupUpdate::SetCap {
            cap_group_id: CapGroupId("group-a".to_string()),
            new_cap: Some("not-a-number".to_string()),
        };
        let err = templar_common::vault::CapGroupUpdate::try_from(update).unwrap_err();
        assert!(matches!(err, ErrorWrapper::InvalidU128(v) if v == "not-a-number"));
    }

    #[test]
    fn cap_group_update_key_roundtrips_with_common() {
        let key = CapGroupUpdateKey::SetMembership {
            market_id: MarketId(11),
        };
        let common: templar_common::vault::CapGroupUpdateKey = key.clone().try_into().unwrap();
        let back: CapGroupUpdateKey = common.into();
        assert_eq!(back, key);
    }

    #[test]
    fn cap_group_update_key_try_into_common_rejects_invalid_cap_group_id() {
        let key = CapGroupUpdateKey::SetCap {
            cap_group_id: CapGroupId("invalid cap group".to_string()),
        };

        assert!(matches!(
            templar_common::vault::CapGroupUpdateKey::try_from(key),
            Err(ErrorWrapper::InvalidCapGroupId(_))
        ));
    }

    #[test]
    fn timelocked_serde_cap_group_variants_use_shared_mapping() {
        let action = TimelockedAction::from(TimelockedActionSerde::CapGroupChange {
            cap_group: templar_common::vault::CapGroupId::try_from("group-z".to_string()).unwrap(),
            new_cap: Some(U128(44)),
        });
        assert!(matches!(
            action,
            TimelockedAction::CapGroupChange { cap_group, new_cap }
                if cap_group.0 == "group-z" && new_cap.as_deref() == Some("44")
        ));

        let action = TimelockedAction::from(TimelockedActionSerde::CapGroupRelativeCapChange {
            cap_group: templar_common::vault::CapGroupId::try_from("group-z".to_string()).unwrap(),
            new_relative_cap: None,
        });
        assert!(matches!(
            action,
            TimelockedAction::CapGroupRelativeCapChange {
                cap_group,
                new_relative_cap: None,
            } if cap_group.0 == "group-z"
        ));

        let action = TimelockedAction::from(TimelockedActionSerde::CapGroupMembership {
            market: templar_common::vault::MarketId(9),
            cap_group: Some(
                templar_common::vault::CapGroupId::try_from("group-z".to_string()).unwrap(),
            ),
        });
        assert!(matches!(
            action,
            TimelockedAction::CapGroupMembership { market, cap_group }
                if market.0 == 9
                    && cap_group.as_ref().map(|id| id.0.as_str()) == Some("group-z")
        ));
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
        let credential = KeyCredential {
            account_id: AccountId("alice.testnet".to_string()),
            secret_key: sk.to_string(),
        };
        let client = VaultClient::new_single_key_default(testnet_rpc, &vault, credential)
            .expect("VaultClient should be created");

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
                && file
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    == Some("src");

            let mut in_guard = false;
            let mut brace_depth: i32 = 0;

            for line in content.lines() {
                let l = line.trim();

                if is_self && !in_guard && l.contains("fn no_json_string_api_regressions") {
                    in_guard = true;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                    let open = l.chars().filter(|&c| c == '{').count() as i32;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                    let close = l.chars().filter(|&c| c == '}').count() as i32;
                    brace_depth = open - close;
                    if brace_depth <= 0 {
                        brace_depth = 1;
                    }
                    continue;
                }

                if in_guard {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                    let open = l.chars().filter(|&c| c == '{').count() as i32;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                    let close = l.chars().filter(|&c| c == '}').count() as i32;
                    brace_depth += open;
                    brace_depth -= close;
                    if brace_depth == 0 {
                        in_guard = false;
                    }
                    continue;
                }

                assert!(
                    !l.contains("ForeignJson"),
                    "ForeignJson not allowed: {}: {l}",
                    file.display()
                );

                assert!(
                    !(l.starts_with("pub ") && l.contains("fn ") && l.contains("_json")),
                    "*_json API not allowed: {}: {l}",
                    file.display()
                );
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
        let credential = KeyCredential {
            account_id: everything,
            secret_key: sk.to_string(),
        };
        let client = VaultClient::new_single_key_default(testnet_rpc, &vault, credential).unwrap();
        let receiver = AccountId("topgunbakugo.testnet".to_string());
        client
            .redeem(&"1".to_string(), &receiver, &"1".to_string())
            .await
            .unwrap();
    }
}
