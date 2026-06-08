use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use templar_soroban_governance::{
    FeeParams, GovernanceActionKind, RestrictionMode, SupplyQueueProposalEntry, TimelockKind,
};
use templar_soroban_shared_types::strkey;
use zeroize::Zeroizing;

pub struct SourceAccount(Zeroizing<String>);

impl SourceAccount {
    #[must_use]
    pub fn as_secret_str(&self) -> &str {
        self.0.as_str()
    }

    #[must_use]
    pub fn clone_secret(&self) -> String {
        self.as_secret_str().to_string()
    }
}

impl Clone for SourceAccount {
    fn clone(&self) -> Self {
        Self(Zeroizing::new(self.as_secret_str().to_string()))
    }
}

impl FromStr for SourceAccount {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if looks_like_secret_source_account(value) {
            return Err(
                "do not pass secret keys or seed phrases via --source-account; use Stellar keystore/default identity or STELLAR_ACCOUNT"
                    .to_string(),
            );
        }
        Ok(Self(Zeroizing::new(value.to_string())))
    }
}

impl fmt::Debug for SourceAccount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SourceAccount(<redacted>)")
    }
}

impl fmt::Display for SourceAccount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

impl PartialEq for SourceAccount {
    fn eq(&self, other: &Self) -> bool {
        self.as_secret_str() == other.as_secret_str()
    }
}

impl Eq for SourceAccount {}

fn looks_like_secret_source_account(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.split_whitespace().count() > 1
        || (trimmed.starts_with('S')
            && trimmed.len() >= 56
            && trimmed.chars().all(|c| c.is_ascii_alphanumeric()))
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct AddressStr(String);

impl AddressStr {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl FromStr for AddressStr {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        strkey::validate_address_strkey(value.as_bytes())
            .map_err(|_| format!("invalid Soroban account/contract address: {value}"))?;
        Ok(Self(value.to_string()))
    }
}

impl fmt::Display for AddressStr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct WasmHash(String);

impl WasmHash {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for WasmHash {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim_start_matches("0x").to_ascii_lowercase();
        if value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit()) {
            Ok(Self(value))
        } else {
            Err("expected a 32-byte hex WASM hash".to_string())
        }
    }
}

impl fmt::Display for WasmHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct GovernanceActionKindArg(pub GovernanceActionKind);

impl FromStr for GovernanceActionKindArg {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = normalize_variant(value);
        let kind = match normalized.as_str() {
            "admin" => GovernanceActionKind::Admin,
            "pause" | "paused" => GovernanceActionKind::Pause,
            "curator" => GovernanceActionKind::Curator,
            "governance" => GovernanceActionKind::Governance,
            "supplyqueue" => GovernanceActionKind::SupplyQueue,
            "fees" => GovernanceActionKind::Fees,
            "restrictions" => GovernanceActionKind::Restrictions,
            "sentinel" => GovernanceActionKind::Sentinel,
            "allocators" => GovernanceActionKind::Allocators,
            "allowedadapters" => GovernanceActionKind::AllowedAdapters,
            "cap" => GovernanceActionKind::Cap,
            "marketremoval" => GovernanceActionKind::MarketRemoval,
            "capgroup" => GovernanceActionKind::CapGroup,
            "skim" => GovernanceActionKind::Skim,
            "upgrade" => GovernanceActionKind::Upgrade,
            "migrate" | "migration" => GovernanceActionKind::Migrate,
            "cancelmigration" => GovernanceActionKind::CancelMigration,
            "timelockconfig" => GovernanceActionKind::TimelockConfig,
            "other" => GovernanceActionKind::Other,
            "withdrawalcooldown" => GovernanceActionKind::WithdrawalCooldown,
            "idleresynccooldown" => GovernanceActionKind::IdleResyncCooldown,
            _ => return Err(format!("unknown governance action kind: {value}")),
        };
        Ok(Self(kind))
    }
}

impl fmt::Display for GovernanceActionKindArg {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.0 {
            GovernanceActionKind::Admin => "Admin",
            GovernanceActionKind::Pause => "Pause",
            GovernanceActionKind::Curator => "Curator",
            GovernanceActionKind::Governance => "Governance",
            GovernanceActionKind::SupplyQueue => "SupplyQueue",
            GovernanceActionKind::Fees => "Fees",
            GovernanceActionKind::Restrictions => "Restrictions",
            GovernanceActionKind::Sentinel => "Sentinel",
            GovernanceActionKind::Allocators => "Allocators",
            GovernanceActionKind::AllowedAdapters => "AllowedAdapters",
            GovernanceActionKind::Cap => "Cap",
            GovernanceActionKind::MarketRemoval => "MarketRemoval",
            GovernanceActionKind::CapGroup => "CapGroup",
            GovernanceActionKind::Skim => "Skim",
            GovernanceActionKind::Upgrade => "Upgrade",
            GovernanceActionKind::Migrate => "Migrate",
            GovernanceActionKind::CancelMigration => "CancelMigration",
            GovernanceActionKind::TimelockConfig => "TimelockConfig",
            GovernanceActionKind::Other => "Other",
            GovernanceActionKind::WithdrawalCooldown => "WithdrawalCooldown",
            GovernanceActionKind::IdleResyncCooldown => "IdleResyncCooldown",
        })
    }
}

impl fmt::Debug for GovernanceActionKindArg {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct TimelockKindArg(pub TimelockKind);

impl FromStr for TimelockKindArg {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = normalize_variant(value);
        let kind = match normalized.as_str() {
            "admin" => TimelockKind::Admin,
            "pause" => TimelockKind::Pause,
            "curator" => TimelockKind::Curator,
            "governance" => TimelockKind::Governance,
            "supplyqueue" => TimelockKind::SupplyQueue,
            "fees" => TimelockKind::Fees,
            "restrictions" => TimelockKind::Restrictions,
            "sentinel" => TimelockKind::Sentinel,
            "allocators" => TimelockKind::Allocators,
            "allowedadapters" => TimelockKind::AllowedAdapters,
            "cap" => TimelockKind::Cap,
            "marketremoval" => TimelockKind::MarketRemoval,
            "capgroup" => TimelockKind::CapGroup,
            "skim" => TimelockKind::Skim,
            "upgrade" => TimelockKind::Upgrade,
            "migration" => TimelockKind::Migration,
            "timelockconfig" => TimelockKind::TimelockConfig,
            "other" => TimelockKind::Other,
            _ => return Err(format!("unknown timelock kind: {value}")),
        };
        Ok(Self(kind))
    }
}

impl fmt::Display for TimelockKindArg {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.0 {
            TimelockKind::Admin => "Admin",
            TimelockKind::Pause => "Pause",
            TimelockKind::Curator => "Curator",
            TimelockKind::Governance => "Governance",
            TimelockKind::SupplyQueue => "SupplyQueue",
            TimelockKind::Fees => "Fees",
            TimelockKind::Restrictions => "Restrictions",
            TimelockKind::Sentinel => "Sentinel",
            TimelockKind::Allocators => "Allocators",
            TimelockKind::AllowedAdapters => "AllowedAdapters",
            TimelockKind::Cap => "Cap",
            TimelockKind::MarketRemoval => "MarketRemoval",
            TimelockKind::CapGroup => "CapGroup",
            TimelockKind::Skim => "Skim",
            TimelockKind::Upgrade => "Upgrade",
            TimelockKind::Migration => "Migration",
            TimelockKind::TimelockConfig => "TimelockConfig",
            TimelockKind::Other => "Other",
        })
    }
}

impl fmt::Debug for TimelockKindArg {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct RestrictionModeArg(pub RestrictionMode);

impl RestrictionModeArg {
    #[must_use]
    pub fn as_u32(self) -> u32 {
        match self.0 {
            RestrictionMode::None => 0,
            RestrictionMode::Blacklist => 1,
            RestrictionMode::Whitelist => 2,
        }
    }
}

impl FromStr for RestrictionModeArg {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = normalize_variant(value);
        let mode = match normalized.as_str() {
            "none" => RestrictionMode::None,
            "blacklist" => RestrictionMode::Blacklist,
            "whitelist" => RestrictionMode::Whitelist,
            _ => return Err(format!("unknown restriction mode: {value}")),
        };
        Ok(Self(mode))
    }
}

impl fmt::Display for RestrictionModeArg {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.0 {
            RestrictionMode::None => "None",
            RestrictionMode::Blacklist => "Blacklist",
            RestrictionMode::Whitelist => "Whitelist",
        })
    }
}

impl fmt::Debug for RestrictionModeArg {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SupplyQueueEntryArg {
    pub target_id: u32,
    pub adapter: AddressStr,
}

impl SupplyQueueEntryArg {
    #[must_use]
    pub fn contract_type_name() -> &'static str {
        std::any::type_name::<SupplyQueueProposalEntry>()
    }
}

impl FromStr for SupplyQueueEntryArg {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (target_id, adapter) = value
            .split_once(':')
            .ok_or_else(|| "expected target_id:adapter_address".to_string())?;
        let target_id = target_id
            .parse::<u32>()
            .map_err(|err| format!("invalid target id: {err}"))?;
        let adapter = adapter.parse::<AddressStr>()?;
        Ok(Self { target_id, adapter })
    }
}

impl fmt::Display for SupplyQueueEntryArg {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.target_id, self.adapter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FeeParamsArg {
    pub performance_fee_wad: i128,
    pub performance_recipient: AddressStr,
    pub management_fee_wad: i128,
    pub management_recipient: AddressStr,
    pub max_growth_rate_wad: Option<i128>,
}

impl FeeParamsArg {
    #[must_use]
    pub fn contract_type_name() -> &'static str {
        std::any::type_name::<FeeParams>()
    }
}

fn normalize_variant(value: &str) -> String {
    value
        .chars()
        .filter(|c| *c != '-' && *c != '_' && !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCOUNT: &str = "GBRFSXJNPLMYJV7EBFTBZT2PU6KN5WWPX3UKHDAAQQT7BNS7QTFCS3AY";
    const CONTRACT: &str = "CDY3B7IXFN5L4OY4UFFS2FA4MAQWJZLJD76LW37S7HFVWRS3RPQ2SIXX";

    #[test]
    fn address_str_validates_strkeys() {
        assert!(ACCOUNT.parse::<AddressStr>().is_ok());
        assert!(CONTRACT.parse::<AddressStr>().is_ok());
        assert!("not-an-address".parse::<AddressStr>().is_err());
    }

    #[test]
    fn supply_queue_entry_is_typed_and_references_contract_type() {
        let entry = format!("7:{CONTRACT}")
            .parse::<SupplyQueueEntryArg>()
            .expect("entry");
        assert_eq!(entry.target_id, 7);
        assert!(SupplyQueueEntryArg::contract_type_name().contains("SupplyQueueProposalEntry"));
    }

    #[test]
    fn parses_governance_kinds_without_free_form_strings() {
        assert!(matches!(
            "cancel-migration"
                .parse::<GovernanceActionKindArg>()
                .expect("kind")
                .0,
            GovernanceActionKind::CancelMigration
        ));
        assert!(matches!(
            "supply_queue".parse::<TimelockKindArg>().expect("kind").0,
            TimelockKind::SupplyQueue
        ));
    }

    #[test]
    fn parses_restriction_mode_from_contract_enum() {
        let mode = "white-list"
            .parse::<RestrictionModeArg>()
            .expect("restriction mode");
        assert_eq!(mode.as_u32(), 2);
        assert!("allow".parse::<RestrictionModeArg>().is_err());
    }

    #[test]
    fn fee_params_arg_references_contract_type() {
        assert!(FeeParamsArg::contract_type_name().contains("FeeParams"));
    }
}
